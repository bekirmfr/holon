// The editor's keys: each place binds its own with `useKeys`, and the `?` dialog lists them all
// from keymap.ts. A key typed into a field, the text pane or a dialog is that field's: bindings
// stand aside, apart from the few that say they work there too (Ctrl+S in the text, say). A
// page's keys are heard before the anywhere keys (on the document, where those are on the
// window), so a page's `d` is its own and not the docs.

import { useEffect, useRef, useState } from "react";
import { KEYMAP, type KeyIn, type KeyScope } from "../keymap";
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

/** What the keys of `scope` do: only keys KEYMAP lists there. */
export type KeyMap<S extends KeyScope> = Partial<Record<KeyIn<S>, KeyAction>>;

/** Keys of `scope` handled by a part's own handler (a review's, a dialog's): the map as it is,
 *  held to KEYMAP as useKeys's is, and looked up by keyOf's name. */
export const keysFor = <S extends KeyScope>(_scope: S, map: KeyMap<S>): Partial<Record<string, KeyAction>> => map;

export type KeyOptions = {
  /** Off while false. */
  enabled?: boolean;
  /** Keys that work while typing, too. */
  whileTyping?: string[];
  /** Keys that work while a dialog or overlay is open, too. */
  inDialogs?: string[];
};

/** Bind `map`'s keys (as keyOf names them) while mounted: the anywhere keys on the window, a
 *  page's on the document, so a page's key is taken before an anywhere key of the same name. */
export function useKeys<S extends KeyScope>(scope: S, map: KeyMap<S>, { enabled = true, whileTyping = [], inDialogs = [] }: KeyOptions = {}) {
  const loose: Partial<Record<string, KeyAction>> = map;
  const current = useRef({ map: loose, whileTyping, inDialogs });
  current.current = { map: loose, whileTyping, inDialogs };
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
    const on: Window | Document = scope === "anywhere" ? window : document;
    on.addEventListener("keydown", onKey as EventListener);
    return () => on.removeEventListener("keydown", onKey as EventListener);
  }, [enabled, scope]);
}

/** Which row each list has selected, by the list's name (`flows`, `runs:refunds`…): kept while
 *  the editor is open, so a page comes back to where it was. */
const picked = new Map<string, string>();

/** Select row `key` of list `list`, for when it is next shown (the palette going to a row). */
export function pick(list: string, key: string): void {
  picked.set(list, key);
}

/** Whether a key pressed at `target` is a control's own (a button or a link with the focus). */
const onControl = (target: EventTarget | null) => target instanceof Element && target.closest("button, a[href], summary") !== null;

/** A list moved through from the keys (keymap.ts's list scope): j and k select the row below or
 *  above, Enter opens it. `rows` are the rows' keys, in order; the selection is kept by key, per
 *  list. `row(key)` gives a row what marks it selected and brings it into view. */
export function useListKeys(list: string, rows: string[], open: (key: string) => void, { enabled = true }: { enabled?: boolean } = {}) {
  const [at, setAt] = useState<string | null>(() => picked.get(list) ?? null);
  const shown = useRef(new Map<string, HTMLElement>());
  // Another list (another flow's runs): its own selection.
  useEffect(() => setAt(picked.get(list) ?? null), [list]);
  const current = at !== null && rows.includes(at) ? at : null;
  // The row selected is in view: one picked before it was shown (by the palette) once it is.
  useEffect(() => {
    if (current !== null) shown.current.get(current)?.scrollIntoView?.({ block: "nearest" });
  }, [current]);
  const select = (key: string) => {
    setAt(key);
    picked.set(list, key);
  };
  const move = (by: 1 | -1) => {
    if (rows.length === 0) return false;
    const i = current === null ? (by > 0 ? 0 : rows.length - 1) : Math.max(0, Math.min(rows.length - 1, rows.indexOf(current) + by));
    select(rows[i]);
  };
  useKeys(
    "list",
    {
      j: () => move(1),
      k: () => move(-1),
      Enter: (e) => (current === null || onControl(e.target) ? false : open(current)),
    },
    { enabled },
  );
  const row = (key: string) => ({
    ref: (el: HTMLElement | null) => {
      if (el) shown.current.set(key, el);
      else shown.current.delete(key);
    },
    "aria-current": key === current ? ("true" as const) : undefined,
    "data-at": key === current ? "" : undefined,
  });
  return { at: current, row, select };
}

const SCOPE_TITLE: Record<KeyScope, string> = {
  anywhere: "Anywhere",
  list: "In a list",
  tabs: "With tabs",
  flow: "A flow's page",
  run: "A run's page",
  approvals: "On Approvals",
  editor: "Editing a flow",
  view: "Viewing runs",
  review: "Reviewing a version",
  dialog: "In a question",
};

/** A binding's keys as the dialog shows them: a run of three digits or more as its first, "…"
 *  and its last (1 to 8). */
export function shownKeys(keys: readonly string[]): string[] {
  const run = keys.length >= 3 && keys.every((k, i) => /^\d$/.test(k) && (i === 0 || Number(k) === Number(keys[i - 1]) + 1));
  return run ? [keys[0], "…", keys[keys.length - 1]] : [...keys];
}

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
                      {shownKeys(b.keys).map((k) =>
                        k === "…" ? (
                          <span key={k} className="hl-keys__to">
                            to
                          </span>
                        ) : (
                          <Kbd key={k} k={k} />
                        ),
                      )}
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
