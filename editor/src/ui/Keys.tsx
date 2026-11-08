// The editor's keys: each place binds its own with `useKeys`, and the `?` dialog lists them all
// from keymap.ts. A key typed into a field, the text pane or a dialog is that field's: bindings
// stand aside, apart from the few that say they work there too (Ctrl+S in the text, say).

import { useEffect, useRef } from "react";
import { KEYMAP, type KeyScope } from "../keymap";
import { Kbd } from "./Kbd";

/** A key as bindings name it: modifiers first (`ctrl+s`, `shift+F6`), a printed character as
 *  typed (`?`, `N`, `,`), others by name (`Escape`, `ArrowLeft`). Cmd counts as Ctrl. */
export function keyOf(e: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">): string {
  const mods: string[] = [];
  if (e.ctrlKey || e.metaKey) mods.push("ctrl");
  if (e.altKey) mods.push("alt");
  let k = e.key;
  if (k.length === 1) {
    // With Ctrl or Alt, a letter is the letter (Ctrl+Shift+K is ctrl+k): a character typed
    // alone keeps its case, so N is not n.
    if (mods.length) k = k.toLowerCase();
  } else if (e.shiftKey) mods.push("shift");
  return [...mods, k === " " ? "Space" : k].join("+");
}

/** Whether a key pressed at `target` is being typed: into a field, an editable element, or the
 *  text pane (CodeMirror). */
export function typing(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  if (target.closest(".cm-editor")) return true;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  return target instanceof HTMLElement && target.isContentEditable === true;
}

/** What a binding does with its key; false when it didn't take it (the key goes on as usual). */
export type KeyAction = (e: KeyboardEvent) => void | boolean;

export type KeyOptions = {
  /** Off while false. */
  enabled?: boolean;
  /** Keys that work while typing, too. */
  whileTyping?: string[];
  /** Keys that work while a dialog or overlay is open, too. */
  inDialogs?: string[];
};

/** Bind `map`'s keys (as keyOf names them) while mounted. */
export function useKeys(map: Record<string, KeyAction>, { enabled = true, whileTyping = [], inDialogs = [] }: KeyOptions = {}) {
  const current = useRef({ map, whileTyping, inDialogs });
  current.current = { map, whileTyping, inDialogs };
  useEffect(() => {
    if (!enabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const k = keyOf(e);
      const { map, whileTyping, inDialogs } = current.current;
      const act = map[k];
      if (!act) return;
      if (typing(e.target) && !whileTyping.includes(k)) return;
      if (document.querySelector('[role="dialog"]') && !inDialogs.includes(k)) return;
      if (act(e) === false) return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enabled]);
}

const SCOPE_TITLE: Record<KeyScope, string> = {
  anywhere: "Anywhere",
  editor: "Editing a flow",
  view: "Viewing runs",
  dialog: "In a question",
};

/** The `?` dialog: every key, by where it works. */
export function KeysDialog({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === "?") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);
  const scopes = [...new Set(KEYMAP.map((b) => b.scope))];
  return (
    <div className="hl-modal" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="hl-modal__box hl-keys" role="dialog" aria-modal="true" aria-label="keys">
        <div className="hl-modal__head">
          <span>Keys</span>
          <button className="hl-modal__close" aria-label="close" onClick={onClose}>
            ×
          </button>
        </div>
        <div className="hl-keys__groups">
          {scopes.map((s) => (
            <section key={s} className="hl-keys__group" aria-label={SCOPE_TITLE[s]}>
              <h3>{SCOPE_TITLE[s]}</h3>
              <dl>
                {KEYMAP.filter((b) => b.scope === s).map((b) => (
                  <div key={b.keys.join(" ")} className="hl-keys__row">
                    <dt>
                      {b.keys.map((k) => (
                        <Kbd key={k} k={k} />
                      ))}
                    </dt>
                    <dd>{b.does}</dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
