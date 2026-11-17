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
//
// A question takes no answer but no until it has been shown `ARM_MS` with no key pressed: one that
// opens while you type (a deploy's consent, read a moment after Ctrl+D, which sits beside Ctrl+S)
// can't be answered by the Enter or y meant for the page. A key or a click on an answer while it
// waits is kept from it and starts the wait again; its answers look disabled until then. No (Esc,
// n, Cancel, a click beside it) is taken at once, and a held key never answers (R43).

import { Kbd } from "./ui/Kbd";
import { keyOf, keysFor } from "./ui/Keys";
import { useFocusTrap } from "./ui/focusTrap";
import { SecretLines } from "./SecretSaid";
import { lettersOf } from "./keymap";
import { createContext, useCallback, useContext, useEffect, useId, useRef, useState, type CSSProperties, type ReactNode } from "react";

/** What a prompt takes for its answer: by default a line, left empty answering null as Cancel
 *  does; `optional`, a line it may be answered without (a reason one may give); `required`, a line
 *  it can't be answered without (a decline's reason): its button stays disabled until something
 *  is typed, and `required` says why under the field. */
export type Answer = "optional" | { required: string };

/** How a secret's field is filled by a password manager: the account's password, a new one, or
 *  neither (a key). */
export type Secret = "current-password" | "new-password" | "off";

type Ask =
  | { kind: "confirm"; message: string; action: string; body?: ReactNode; resolve: (ok: boolean) => void }
  | { kind: "prompt"; message: string; initial: string; action: string; answer?: Answer; secret?: Secret; resolve: (value: string | null) => void }
  | { kind: "choose"; message: string; answers: string[]; resolve: (at: number | null) => void }
  | { kind: "notice"; message: string; action: string; body?: ReactNode; resolve: () => void };

type Dialogs = {
  /** Ask a yes/no question; resolves true only if confirmed. `action` labels the confirm button;
   *  `body`, shown under the question, is what it is about (a deploy's review, say). */
  confirm: (message: string, action?: string, body?: ReactNode) => Promise<boolean>;
  /** Ask for a line of text; resolves the trimmed text, or null if cancelled or left empty
   *  (`answer` says otherwise: an optional reason, or one that is required). `action` labels the
   *  button that answers. */
  prompt: (message: string, initial?: string, action?: string, answer?: Answer) => Promise<string | null>;
  /** Ask for a secret (a password, a key): typed into a field that hides it, which `autoComplete`
   *  says how a password manager fills; resolves it as typed, or null if cancelled or left empty.
   *  Every question for one is asked so, never with `prompt` (Dialog.test.tsx). */
  secret: (message: string, autoComplete: Secret, action?: string) => Promise<string | null>;
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

/** How long a question waits, once shown and after each key pressed while it waits, before it takes
 *  an answer other than no: the double-click interval, as Chrome's guard on its own prompts has it
 *  (InputEventActivationProtector). Long enough to be past the keys of a word being typed, short
 *  enough not to be noticed by whoever reads the question first. */
export const ARM_MS = 500;

/** Why its answers are dimmed while a question waits, as a screen reader says it (each answer is
 *  described by it then): what the dimming says to whoever sees it. */
export const WAIT_SAID = "Not yet: it takes an answer a moment after it shows, and after the last key pressed.";
/** Said aloud (a polite status) when an answer comes while it waits, and isn't taken. */
export const TOO_SOON = "Too soon: not taken. Answer again in a moment.";

/** `armMs`: how long a question waits before it takes an answer (`ARM_MS`); a test of something
 *  else gives 0, and its questions take one at once. */
export function DialogProvider({ children, armMs = ARM_MS }: { children: ReactNode; armMs?: number }) {
  const [ask, setAsk] = useState<Ask | null>(null);
  const [text, setText] = useState("");
  const box = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const ok = useRef<HTMLButtonElement>(null);
  const question = useId();
  const waitNote = useId();
  // Said to a screen reader when an answer comes too soon: what it saw dimmed, in words.
  const [tooSoon, setTooSoon] = useState("");

  // Whether the question takes an answer yet: `armed` for what it shows, `armedNow` for the key or
  // click that comes before the render does.
  const [armed, setArmed] = useState(true);
  const armedNow = useRef(true);
  const arming = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Wait `armMs` again before the question takes an answer. */
  const wait = useCallback(() => {
    if (arming.current !== null) clearTimeout(arming.current);
    arming.current = null;
    if (armMs <= 0) return;
    armedNow.current = false;
    setArmed(false);
    arming.current = setTimeout(() => {
      arming.current = null;
      armedNow.current = true;
      setArmed(true);
    }, armMs);
  }, [armMs]);
  useEffect(() => () => {
    if (arming.current !== null) clearTimeout(arming.current);
  }, []);
  /** Show a question, waiting before it takes an answer. */
  const show = useCallback(
    (a: Ask) => {
      setAsk(a);
      setTooSoon("");
      wait();
    },
    [wait],
  );

  const confirm = useCallback(
    (message: string, action = "OK", body?: ReactNode) => new Promise<boolean>((resolve) => show({ kind: "confirm", message, action, body, resolve })),
    [show],
  );
  const prompt = useCallback(
    (message: string, initial = "", action = "OK", answer?: Answer) => {
      setText(initial);
      return new Promise<string | null>((resolve) => show({ kind: "prompt", message, initial, action, answer, resolve }));
    },
    [show],
  );
  const secret = useCallback(
    (message: string, autoComplete: Secret, action = "OK") => {
      setText("");
      return new Promise<string | null>((resolve) => show({ kind: "prompt", message, initial: "", action, secret: autoComplete, resolve }));
    },
    [show],
  );
  const choose = useCallback((message: string, answers: string[]) => new Promise<number | null>((resolve) => show({ kind: "choose", message, answers, resolve })), [show]);
  const notice = useCallback(
    (message: string, action = "OK", body?: ReactNode) => new Promise<void>((resolve) => show({ kind: "notice", message, action, body, resolve })),
    [show],
  );

  // Why it can't be answered yet: a required line, still empty.
  const required = ask?.kind === "prompt" && typeof ask.answer === "object" ? ask.answer.required : null;
  const blocked = required !== null && text.trim() === "";
  /** `accepted`: the highlighted button (for a choice, the last answer; a number, that answer). */
  const close = useCallback(
    (accepted: boolean | number) => {
      if (!ask) return;
      if (accepted !== false && blocked) return;
      // Too soon: kept from it, and it waits again (a click on an answer, or Space on it, comes
      // here as Enter and the letters do).
      if (accepted !== false && !armedNow.current) {
        setTooSoon((was) => (was === TOO_SOON ? `${TOO_SOON}\u00a0` : TOO_SOON));
        return wait();
      }
      if (ask.kind === "notice") ask.resolve();
      else if (ask.kind === "confirm") ask.resolve(accepted !== false);
      else if (ask.kind === "choose") ask.resolve(accepted === false ? null : accepted === true ? ask.answers.length - 1 : accepted);
      else {
        // A secret as typed, its spaces its own; and not kept once answered.
        const typed = ask.secret ? text : text.trim();
        ask.resolve(accepted !== false && (ask.answer !== undefined || typed !== "") ? typed : null);
        if (ask.secret) setText("");
      }
      setAsk(null);
    },
    [ask, text, blocked, wait],
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
      // A key held down from before is not an answer; Esc's no still is.
      if (e.repeat && e.key !== "Escape") return;
      act(e);
    };
    // Capturing, and kept from the page: a key that answers the question is not also one of the
    // page's keys beneath it. Typing in the prompt's own field still reaches the field, and a part
    // with keys of its own (a review's n, space…) has them while it has the focus: there only Esc
    // answers. Tab is the focus trap's.
    const guard = (e: KeyboardEvent) => {
      if (e.key === "Tab") return;
      // While it waits, a key pressed starts the wait again: someone still typing hasn't read it.
      // Not the letters typed into the prompt's own field, which may be its answer's (an Enter
      // there still waits).
      const typing = e.target === input.current && e.key !== "Enter";
      if (!armedNow.current && !typing && e.key !== "Escape") wait();
      if (e.target instanceof Element && e.target.closest("[data-own-keys]")) {
        if (e.key === "Escape") onKey(e);
        return;
      }
      onKey(e);
      if (e.target !== input.current) e.stopPropagation();
    };
    window.addEventListener("keydown", guard, true);
    return () => window.removeEventListener("keydown", guard, true);
  }, [ask, close, wait]);

  const button = {
    font: "inherit",
    fontSize: 12,
    padding: "4px 12px",
    borderRadius: 3,
    cursor: "pointer",
  } as const;
  // An answer while the question waits: shown disabled, and kept from it (`close`). It keeps the
  // focus it starts with, so Enter can answer once the wait is over.
  // Described by why, for whoever hears it rather than sees it dimmed.
  const waiting: { "aria-disabled"?: true; "aria-describedby"?: string; style?: CSSProperties } = armed
    ? {}
    : { "aria-disabled": true, "aria-describedby": waitNote, style: { opacity: 0.5, cursor: "default" } };

  return (
    <DialogContext.Provider value={{ confirm, prompt, secret, choose, notice }}>
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
            <div id={question} style={{ whiteSpace: "pre-line", maxHeight: (ask.kind === "confirm" || ask.kind === "notice") && ask.body ? "30vh" : "60vh", overflowY: "auto", flexShrink: 0 }}>
              <SecretLines text={ask.message} />
            </div>
            {/* For a screen reader: why its answers are dimmed, and that one came too soon. */}
            <span id={waitNote} hidden>
              {WAIT_SAID}
            </span>
            <span role="status" style={{ position: "absolute", width: 1, height: 1, overflow: "hidden", clipPath: "inset(50%)", whiteSpace: "nowrap" }}>
              {tooSoon}
            </span>
            {(ask.kind === "confirm" || ask.kind === "notice") && ask.body && <div style={{ flex: "1 1 auto", minHeight: 0, overflowY: "auto" }}>{ask.body}</div>}
            {ask.kind === "prompt" && (
              <input
                ref={input}
                type={ask.secret ? "password" : "text"}
                autoComplete={ask.secret}
                spellCheck={ask.secret ? false : undefined}
                value={text}
                onChange={(e) => setText(e.target.value)}
                aria-required={required !== null}
                aria-describedby={blocked ? "hl-dialog-required" : undefined}
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
            {blocked && (
              <div id="hl-dialog-required" style={{ color: "var(--muted)", fontSize: 12, marginTop: -6 }}>
                {required}
              </div>
            )}
            {ask.kind === "notice" ? (
              <div style={{ display: "flex", justifyContent: "flex-end" }}>
                <button
                  ref={ok}
                  onClick={() => close(true)}
                  aria-keyshortcuts={[letters[0]?.toUpperCase(), "Enter", "Escape"].filter(Boolean).join(" ")}
                  aria-disabled={waiting["aria-disabled"]}
                  aria-describedby={waiting["aria-describedby"]}
                  style={{ ...button, color: "var(--accent-ink)", background: "var(--accent)", border: "none", ...waiting.style }}
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
                      aria-disabled={waiting["aria-disabled"]}
                      aria-describedby={waiting["aria-describedby"]}
                      style={{
                        ...(last
                          ? { ...button, color: "var(--accent-ink)", background: "var(--accent)", border: "none" }
                          : { ...button, color: "var(--muted)", background: "transparent", border: "1px solid var(--border)" }),
                        ...waiting.style,
                      }}
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
                disabled={blocked}
                title={blocked ? (required ?? undefined) : undefined}
                aria-keyshortcuts={ask.kind === "confirm" ? "Y Enter" : "Enter"}
                aria-disabled={blocked ? undefined : waiting["aria-disabled"]}
                aria-describedby={blocked ? undefined : waiting["aria-describedby"]}
                style={{ ...button, color: "var(--accent-ink)", background: "var(--accent)", border: "none", ...waiting.style, ...(blocked ? { opacity: 0.5, cursor: "not-allowed" } : {}) }}
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
