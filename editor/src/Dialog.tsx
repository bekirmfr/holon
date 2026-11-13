// In-app confirm/prompt dialogs. The browser's own window.confirm / window.prompt can't be relied
// on: an embedded browser (the Claude app's pane, some webviews) suppresses them, and they then
// answer "cancel" instantly, without showing anything — which silently turned every delete, New
// and first Save into a no-op there. These render inside the page instead, so they always show.
//
// A question is answered from the keys as well: Enter answers with what has the focus (Cancel no,
// an answer's button that answer, the prompt's field yes with its text) and Esc cancels, and a
// yes/no question also takes y and n (the workbench's Confirm), shown as chips on its buttons. A
// question with several answers takes each one's letter (keymap.ts's lettersOf), shown on it, and
// a notice (something to read, with one button) takes its button's letter, Enter and Esc alike.
// Tab and Shift+Tab go round the dialog's own controls: the focus stays in it while it is open
// (useFocusTrap, as every modal's).

import { Kbd } from "./ui/Kbd";
import { keyOf, keysFor } from "./ui/Keys";
import { useFocusTrap } from "./ui/focusTrap";
import { lettersOf } from "./keymap";
import { createContext, useCallback, useContext, useEffect, useId, useRef, useState, type ReactNode } from "react";

type Ask =
  | { kind: "confirm"; message: string; action: string; body?: ReactNode; resolve: (ok: boolean) => void }
  | { kind: "prompt"; message: string; initial: string; action: string; empty: boolean; resolve: (value: string | null) => void }
  | { kind: "choose"; message: string; answers: string[]; resolve: (at: number | null) => void }
  | { kind: "notice"; message: string; action: string; body?: ReactNode; resolve: () => void };

type Dialogs = {
  /** Ask a yes/no question; resolves true only if confirmed. `action` labels the confirm button;
   *  `body`, shown under the question, is what it is about (a deploy's review, say). */
  confirm: (message: string, action?: string, body?: ReactNode) => Promise<boolean>;
  /** Ask for a line of text; resolves the trimmed text, or null if cancelled or left empty
   *  (unless `empty` lets it be answered empty, an optional reason). `action` labels the button
   *  that answers. */
  prompt: (message: string, initial?: string, action?: string, empty?: boolean) => Promise<string | null>;
  /** Ask a question with several answers, the last of them the highlighted one (Enter); resolves
   *  the answer's index, or null if cancelled (Esc, or a click beside it). */
  choose: (message: string, answers: string[]) => Promise<number | null>;
  /** Say something, with one button that closes it (`action`, "OK" by default): nothing to
   *  answer. `body` is shown under the message, as a confirm's is. Resolves once closed. */
  notice: (message: string, action?: string, body?: ReactNode) => Promise<void>;
};

const DialogContext = createContext<Dialogs | null>(null);

export function useDialog(): Dialogs {
  const d = useContext(DialogContext);
  if (!d) throw new Error("useDialog needs a <DialogProvider> above it");
  return d;
}

/** The dialogs, when a provider is above (a node rendered on its own has none). */
export function useOptionalDialog(): Dialogs | null {
  return useContext(DialogContext);
}

export function DialogProvider({ children }: { children: ReactNode }) {
  const [ask, setAsk] = useState<Ask | null>(null);
  const [text, setText] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const ok = useRef<HTMLButtonElement>(null);
  const question = useId();

  const confirm = useCallback(
    (message: string, action = "OK", body?: ReactNode) => new Promise<boolean>((resolve) => setAsk({ kind: "confirm", message, action, body, resolve })),
    [],
  );
  const prompt = useCallback((message: string, initial = "", action = "OK", empty = false) => {
    setText(initial);
    return new Promise<string | null>((resolve) => setAsk({ kind: "prompt", message, initial, action, empty, resolve }));
  }, []);
  const choose = useCallback((message: string, answers: string[]) => new Promise<number | null>((resolve) => setAsk({ kind: "choose", message, answers, resolve })), []);
  const notice = useCallback(
    (message: string, action = "OK", body?: ReactNode) => new Promise<void>((resolve) => setAsk({ kind: "notice", message, action, body, resolve })),
    [],
  );

  /** `accepted`: the highlighted button (for a choice, the last answer; a number, that answer). */
  const close = useCallback(
    (accepted: boolean | number) => {
      if (!ask) return;
      if (ask.kind === "notice") ask.resolve();
      else if (ask.kind === "confirm") ask.resolve(accepted !== false);
      else if (ask.kind === "choose") ask.resolve(accepted === false ? null : accepted === true ? ask.answers.length - 1 : accepted);
      else ask.resolve(accepted !== false && (ask.empty || text.trim() !== "") ? text.trim() : null);
      setAsk(null);
    },
    [ask, text],
  );

  // A choice's answers each take a letter, as a notice's button does; a yes/no question takes y and n.
  const letters = ask?.kind === "choose" ? lettersOf(ask.answers) : ask?.kind === "notice" ? lettersOf([ask.action]) : [];

  // Each question starts on its field or its highlighted button, once (not on every letter typed
  // into the prompt), even when the answer to one opens the next with no render between.
  useFocusTrap(box, ask, () => (ask?.kind === "prompt" ? input.current : ok.current));

  useEffect(() => {
    if (!ask) return;
    const yesNo = ask.kind === "confirm";
    // Enter answers as a click on what has the focus would: Cancel no, an answer's button that
    // answer, the prompt's field yes, anything else nothing (a notice, with one button, closes).
    // Handled here and never left to the browser, so the same Enter can't also press the button
    // of a dialog that opens next (a prompt's answer followed by a confirm would otherwise accept
    // it unread). The letters keyOf names with a modifier held (ctrl+n) are the browser's, not an
    // answer.
    const keys = keysFor("dialog", {
      Enter: (e) => {
        const at = e.target;
        if (ask.kind === "notice" || at === ok.current || at === input.current) close(true);
        else if (at === cancel.current) close(false);
        else if (ask.kind === "choose" && at instanceof HTMLElement && box.current?.contains(at) && at.dataset.answer !== undefined) close(Number(at.dataset.answer));
      },
      Escape: () => close(false),
      ...(yesNo ? { y: () => close(true), n: () => close(false) } : {}),
    });
    const onKey = (e: KeyboardEvent) => {
      const k = keyOf(e);
      const letter = ask.kind === "choose" ? lettersOf(ask.answers).indexOf(k) : ask.kind === "notice" && lettersOf([ask.action])[0] === k ? 0 : -1;
      const act = letter >= 0 ? () => close(letter) : keys[k];
      if (!act) return;
      e.preventDefault();
      act(e);
    };
    // Capturing, and kept from the page: a key that answers the question is not also one of the
    // page's keys beneath it. Typing in the prompt's own field still reaches the field, and a part
    // with keys of its own (a review's n, space…) has them while it has the focus: there only Esc
    // answers. Tab is the focus trap's.
    const guard = (e: KeyboardEvent) => {
      if (e.key === "Tab") return;
      if (e.target instanceof Element && e.target.closest("[data-own-keys]")) {
        if (e.key === "Escape") onKey(e);
        return;
      }
      onKey(e);
      if (e.target !== input.current) e.stopPropagation();
    };
    window.addEventListener("keydown", guard, true);
    return () => window.removeEventListener("keydown", guard, true);
  }, [ask, close]);

  const button = {
    font: "inherit",
    fontSize: 12,
    padding: "4px 12px",
    borderRadius: 3,
    cursor: "pointer",
  } as const;

  return (
    <DialogContext.Provider value={{ confirm, prompt, choose, notice }}>
      {children}
      {ask && (
        <div
          onMouseDown={(e) => e.target === e.currentTarget && close(false)}
          style={{
            position: "fixed",
            inset: 0,
            background: "var(--overlay)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            zIndex: 1000,
          }}
        >
          <div
            ref={box}
            role="dialog"
            aria-modal="true"
            aria-labelledby={question}
            style={{
              background: "var(--surface-2)",
              border: "1px solid var(--border)",
              borderRadius: 6,
              padding: 16,
              width: (ask.kind === "confirm" || ask.kind === "notice") && ask.body ? 760 : 420,
              maxWidth: "calc(100vw - 32px)",
              // As tall as the window at most: the buttons stay in it, and what is above scrolls.
              maxHeight: "calc(100vh - 32px)",
              color: "var(--ink)",
              fontSize: 13,
              display: "flex",
              flexDirection: "column",
              gap: 12,
            }}
          >
            <div id={question} style={{ whiteSpace: "pre-line", maxHeight: (ask.kind === "confirm" || ask.kind === "notice") && ask.body ? "30vh" : "60vh", overflowY: "auto", flexShrink: 0 }}>{ask.message}</div>
            {(ask.kind === "confirm" || ask.kind === "notice") && ask.body && <div style={{ flex: "1 1 auto", minHeight: 0, overflowY: "auto" }}>{ask.body}</div>}
            {ask.kind === "prompt" && (
              <input
                ref={input}
                value={text}
                onChange={(e) => setText(e.target.value)}
                style={{
                  font: "inherit",
                  color: "var(--ink)",
                  background: "var(--bg)",
                  border: "1px solid var(--border)",
                  borderRadius: 3,
                  padding: "4px 6px",
                }}
              />
            )}
            {ask.kind === "notice" ? (
              <div style={{ display: "flex", justifyContent: "flex-end" }}>
                <button
                  ref={ok}
                  onClick={() => close(true)}
                  aria-keyshortcuts={[letters[0]?.toUpperCase(), "Enter", "Escape"].filter(Boolean).join(" ")}
                  style={{ ...button, color: "var(--accent-ink)", background: "var(--accent)", border: "none" }}
                >
                  {letters[0] && <Kbd k={letters[0]} />}
                  {ask.action}
                </button>
              </div>
            ) : ask.kind === "choose" ? (
              <div style={{ display: "flex", justifyContent: "flex-end", gap: 8, flexWrap: "wrap" }}>
                {ask.answers.map((answer, i) => {
                  const last = i === ask.answers.length - 1;
                  return (
                    <button
                      key={answer}
                      ref={last ? ok : undefined}
                      data-answer={i}
                      onClick={() => close(i)}
                      aria-keyshortcuts={[letters[i]?.toUpperCase(), last ? "Enter" : ""].filter(Boolean).join(" ") || undefined}
                      style={
                        last
                          ? { ...button, color: "var(--accent-ink)", background: "var(--accent)", border: "none" }
                          : { ...button, color: "var(--muted)", background: "transparent", border: "1px solid var(--border)" }
                      }
                    >
                      {letters[i] && <Kbd k={letters[i]} />}
                      {answer}
                    </button>
                  );
                })}
              </div>
            ) : (
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
              <button
                ref={cancel}
                onClick={() => close(false)}
                aria-keyshortcuts={ask.kind === "confirm" ? "N Escape" : "Escape"}
                style={{ ...button, color: "var(--muted)", background: "transparent", border: "1px solid var(--border)" }}
              >
                {ask.kind === "confirm" && <Kbd k="n" />}
                Cancel
              </button>
              <button
                ref={ok}
                onClick={() => close(true)}
                aria-keyshortcuts={ask.kind === "confirm" ? "Y Enter" : "Enter"}
                style={{ ...button, color: "var(--accent-ink)", background: "var(--accent)", border: "none" }}
              >
                {ask.kind === "confirm" && <Kbd k="y" />}
                {ask.action}
              </button>
            </div>
            )}
          </div>
        </div>
      )}
    </DialogContext.Provider>
  );
}
