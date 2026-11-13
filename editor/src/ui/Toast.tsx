// Toasts: what just happened, said once in the corner (saved, updated, not saved), instead of a
// line that stays beside the button until something else replaces it. A notice fades after a few
// seconds; a failure stays until it is dismissed, since it may need reading. What is wrong now (a
// flow that can't deploy, a missing secret) is not a toast: it stays where it is, as long as it is
// true. A toast may carry one link, to where what it says can be seen (the flow deployed, the run).

import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { format, type Route } from "../routes";

export type Tone = "ok" | "info" | "bad";

/** A toast's one link: "Open the run", say. */
export type ToastLink = { label: string; to: Route };

export type Toast = { id: number; text: string; tone: Tone; link?: ToastLink };

/** Say `text` once, with a link when there is a place to go; a `bad` one stays until dismissed. */
export type Notify = (text: string, tone?: Tone, link?: ToastLink) => void;

/** How long a notice stays, in milliseconds. */
export const TOAST_MS = 4000;
/** At most this many are shown; an older one gives way to a newer. */
const MOST = 4;

const ToastContext = createContext<Notify | null>(null);

/** Say something once (a provider above it shows it); without a provider, nothing is said. */
export function useToast(): Notify {
  return useContext(ToastContext) ?? quiet;
}

const quiet: Notify = () => {};

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const next = useRef(0);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  const dismiss = useCallback((id: number) => {
    const t = timers.current.get(id);
    if (t) clearTimeout(t);
    timers.current.delete(id);
    setToasts((ts) => ts.filter((x) => x.id !== id));
  }, []);

  const notify = useCallback<Notify>(
    (text, tone = "ok", link) => {
      if (!text) return;
      const id = next.current++;
      setToasts((ts) => [...ts.filter((x) => x.text !== text), { id, text, tone, link }].slice(-MOST));
      if (tone !== "bad") timers.current.set(id, setTimeout(() => dismiss(id), TOAST_MS));
    },
    [dismiss],
  );

  useEffect(() => {
    const all = timers.current;
    return () => all.forEach(clearTimeout);
  }, []);

  return (
    <ToastContext.Provider value={notify}>
      {children}
      <div className="hl-toasts" role="region" aria-label="notices">
        {toasts.map((t) => (
          <div key={t.id} className={`hl-toast hl-toast--${t.tone}`} role={t.tone === "bad" ? "alert" : "status"}>
            <span className="hl-toast__text">
              {t.text}
              {t.link && (
                <>
                  {" "}
                  {/* The address is the place (routes.ts): following it is going there. */}
                  <a className="hl-toast__link" href={format(t.link.to)} onClick={() => dismiss(t.id)}>
                    {t.link.label}
                  </a>
                </>
              )}
            </span>
            <button className="hl-toast__close" aria-label="dismiss" onClick={() => dismiss(t.id)}>
              ×
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}
