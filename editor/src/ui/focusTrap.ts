// A modal dialog keeps the focus while it is open: it takes the focus as it opens, Tab and
// Shift+Tab go round its own controls and never out to the page beneath (where Enter and the
// page's keys would act on what the dialog hides), a focus that fell out of it (a click on its
// text) comes back in on the next Tab, and as it closes the focus goes back to where it was. A
// dialog opened over another (a confirm over the folder picker) has the Tab until it closes; then
// the one beneath has it again.

import { useEffect, useRef, type RefObject } from "react";

/** The dialogs open now, the one opened last at the end: only it moves the focus. */
const open: object[] = [];

/** Keep the focus in `box` while `on` is set; a new `on` (the next question in the same dialog)
 *  starts again. `first` is the control it starts on, else its first. */
export function useFocusTrap(box: RefObject<HTMLElement | null>, on: unknown = true, first?: () => HTMLElement | null) {
  const start = useRef(first);
  start.current = first;
  const opener = useRef<HTMLElement | null>(null);
  useEffect(() => {
    const el = box.current;
    if (!on || !el) return;
    const me = {};
    open.push(me);
    // Where the focus goes back to as it closes: what had it as it opened. With the focus already
    // inside, this is the next question in the same dialog, which keeps the one the first had.
    const at = document.activeElement;
    if (!el.contains(at)) opener.current = at instanceof HTMLElement && at !== document.body ? at : null;
    (start.current?.() ?? stops(el)[0])?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Tab" || e.ctrlKey || e.metaKey || e.altKey || open[open.length - 1] !== me) return;
      around(el, e);
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      open.splice(open.indexOf(me), 1);
      // Back where it was, if it went with the dialog: not if something has since taken it (a
      // command the palette ran, the next question's own control).
      const now = document.activeElement;
      const back = opener.current;
      if (back?.isConnected && (now === null || now === document.body || el.contains(now))) back.focus();
    };
  }, [box, on]);
}

/** Move the focus to the next of the dialog's controls (the previous with Shift), from the last
 *  round to the first: every Tab is moved here, so none can take the focus out. */
function around(box: HTMLElement, e: KeyboardEvent) {
  e.preventDefault();
  const all = stops(box);
  if (all.length === 0) return;
  const at = all.indexOf(document.activeElement as HTMLElement);
  const step = e.shiftKey ? -1 : 1;
  const next = at < 0 ? (e.shiftKey ? all.length - 1 : 0) : (at + step + all.length) % all.length;
  all[next].focus();
}

/** The dialog's controls Tab stops at, in order: what is hidden (a review's closed rows) or
 *  disabled is skipped, as the browser would. */
function stops(box: HTMLElement): HTMLElement[] {
  return [...box.querySelectorAll<HTMLElement>("a[href], button, input, select, textarea, summary, [tabindex]")].filter(
    (el) => el.tabIndex >= 0 && !el.matches(":disabled") && !el.closest("[hidden], [inert]") && el.checkVisibility?.({ checkVisibilityCSS: true }) !== false,
  );
}
