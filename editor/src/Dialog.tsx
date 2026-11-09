// In-app confirm/prompt dialogs. The browser's own window.confirm / window.prompt can't be relied
// on: an embedded browser (the Claude app's pane, some webviews) suppresses them, and they then
// answer "cancel" instantly, without showing anything — which silently turned every delete, New
// and first Save into a no-op there. These render inside the page instead, so they always show.
//
// A question is answered from the keys as well: Enter is its button and Esc cancels, and a yes/no
// question also takes y and n (the workbench's Confirm), shown as chips on its buttons.

import { Kbd } from "./ui/Kbd";
import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";

type Ask =
  | { kind: "confirm"; message: string; action: string; body?: ReactNode; resolve: (ok: boolean) => void }
  | { kind: "prompt"; message: string; initial: string; action: string; empty: boolean; resolve: (value: string | null) => void };

type Dialogs = {
  /** Ask a yes/no question; resolves true only if confirmed. `action` labels the confirm button;
   *  `body`, shown under the question, is what it is about (a deploy's review, say). */
  confirm: (message: string, action?: string, body?: ReactNode) => Promise<boolean>;
  /** Ask for a line of text; resolves the trimmed text, or null if cancelled or left empty
   *  (unless `empty` lets it be answered empty, an optional reason). `action` labels the button
   *  that answers. */
  prompt: (message: string, initial?: string, action?: string, empty?: boolean) => Promise<string | null>;
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
  const input = useRef<HTMLInputElement>(null);
  const ok = useRef<HTMLButtonElement>(null);

  const confirm = useCallback(
    (message: string, action = "OK", body?: ReactNode) => new Promise<boolean>((resolve) => setAsk({ kind: "confirm", message, action, body, resolve })),
    [],
  );
  const prompt = useCallback((message: string, initial = "", action = "OK", empty = false) => {
    setText(initial);
    return new Promise<string | null>((resolve) => setAsk({ kind: "prompt", message, initial, action, empty, resolve }));
  }, []);

  const close = useCallback(
    (accepted: boolean) => {
      if (!ask) return;
      if (ask.kind === "confirm") ask.resolve(accepted);
      else ask.resolve(accepted && (ask.empty || text.trim() !== "") ? text.trim() : null);
      setAsk(null);
    },
    [ask, text],
  );

  useEffect(() => {
    if (!ask) return;
    (ask.kind === "prompt" ? input.current : ok.current)?.focus();
    const yesNo = ask.kind === "confirm";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" || (yesNo && e.key === "n" && !held(e))) {
        e.preventDefault();
        close(false);
      }
      if (yesNo && e.key === "y" && !held(e)) {
        e.preventDefault();
        close(true);
      }
      // Handled here, so the same Enter can't also press the button of a dialog that opens next
      // (a prompt's answer followed by a confirm would otherwise accept it unread).
      if (e.key === "Enter") {
        e.preventDefault();
        close(true);
      }
    };
    // Capturing, and kept from the page: a key that answers the question is not also one of the
    // page's keys beneath it. Typing in the prompt's own field still reaches the field, and a part
    // with keys of its own (a review's n, space…) has them while it has the focus: there only Esc
    // answers.
    const guard = (e: KeyboardEvent) => {
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
    <DialogContext.Provider value={{ confirm, prompt }}>
      {children}
      {ask && (
        <div
          role="dialog"
          aria-modal="true"
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
            style={{
              background: "var(--surface-2)",
              border: "1px solid var(--border)",
              borderRadius: 6,
              padding: 16,
              width: ask.kind === "confirm" && ask.body ? 760 : 420,
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
            <div style={{ whiteSpace: "pre-line", maxHeight: ask.kind === "confirm" && ask.body ? "30vh" : "60vh", overflowY: "auto", flexShrink: 0 }}>{ask.message}</div>
            {ask.kind === "confirm" && ask.body && <div style={{ flex: "1 1 auto", minHeight: 0, overflowY: "auto" }}>{ask.body}</div>}
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
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
              <button
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
          </div>
        </div>
      )}
    </DialogContext.Provider>
  );
}

/** Whether a modifier is held: Ctrl+N and the like stay the browser's, not an answer. */
function held(e: KeyboardEvent): boolean {
  return e.ctrlKey || e.metaKey || e.altKey;
}
