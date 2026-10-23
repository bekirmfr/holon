// In-app confirm/prompt dialogs. The browser's own window.confirm / window.prompt can't be relied
// on: an embedded browser (the Claude app's pane, some webviews) suppresses them, and they then
// answer "cancel" instantly, without showing anything — which silently turned every delete, New
// and first Save into a no-op there. These render inside the page instead, so they always show.

import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";

type Ask =
  | { kind: "confirm"; message: string; action: string; resolve: (ok: boolean) => void }
  | { kind: "prompt"; message: string; initial: string; resolve: (value: string | null) => void };

type Dialogs = {
  /** Ask a yes/no question; resolves true only if confirmed. `action` labels the confirm button. */
  confirm: (message: string, action?: string) => Promise<boolean>;
  /** Ask for a line of text; resolves the trimmed text, or null if cancelled or left empty. */
  prompt: (message: string, initial?: string) => Promise<string | null>;
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
    (message: string, action = "OK") => new Promise<boolean>((resolve) => setAsk({ kind: "confirm", message, action, resolve })),
    [],
  );
  const prompt = useCallback((message: string, initial = "") => {
    setText(initial);
    return new Promise<string | null>((resolve) => setAsk({ kind: "prompt", message, initial, resolve }));
  }, []);

  const close = useCallback(
    (accepted: boolean) => {
      if (!ask) return;
      if (ask.kind === "confirm") ask.resolve(accepted);
      else ask.resolve(accepted && text.trim() !== "" ? text.trim() : null);
      setAsk(null);
    },
    [ask, text],
  );

  useEffect(() => {
    if (!ask) return;
    (ask.kind === "prompt" ? input.current : ok.current)?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close(false);
      if (e.key === "Enter") close(true);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
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
            background: "rgba(5, 10, 20, 0.6)",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            zIndex: 1000,
          }}
        >
          <div
            style={{
              background: "#0e1a2f",
              border: "1px solid #1b2a45",
              borderRadius: 6,
              padding: 16,
              width: 420,
              maxWidth: "calc(100vw - 32px)",
              color: "#e6eefb",
              fontSize: 13,
              display: "flex",
              flexDirection: "column",
              gap: 12,
            }}
          >
            <div style={{ whiteSpace: "pre-line", maxHeight: "60vh", overflowY: "auto" }}>{ask.message}</div>
            {ask.kind === "prompt" && (
              <input
                ref={input}
                value={text}
                onChange={(e) => setText(e.target.value)}
                style={{
                  font: "inherit",
                  color: "#e6eefb",
                  background: "#0b1527",
                  border: "1px solid #1b2a45",
                  borderRadius: 3,
                  padding: "4px 6px",
                }}
              />
            )}
            <div style={{ display: "flex", justifyContent: "flex-end", gap: 8 }}>
              <button
                onClick={() => close(false)}
                style={{ ...button, color: "#9fb4d4", background: "transparent", border: "1px solid #1b2a45" }}
              >
                Cancel
              </button>
              <button
                ref={ok}
                onClick={() => close(true)}
                style={{ ...button, color: "#0e1a2f", background: "#6fd3e0", border: "none" }}
              >
                {ask.kind === "confirm" ? ask.action : "OK"}
              </button>
            </div>
          </div>
        </div>
      )}
    </DialogContext.Provider>
  );
}
