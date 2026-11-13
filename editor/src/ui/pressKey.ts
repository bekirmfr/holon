// For the tests: a key pressed as a browser presses it. jsdom gives a key no default of its own,
// so a test that only dispatched the event would pass where the browser fails: Enter on a button
// the page left alone clicks it, and a Tab the page left alone moves the focus on, out of a dialog
// as readily as through it.

import { act } from "react";

export const settle = () => new Promise((r) => setTimeout(r, 0));

/** Press `key` where the focus is: the page's handlers first, then, unless one prevented it, the
 *  key's own default: Enter clicks a focused button, Tab moves the focus to the next control in
 *  the page (Shift+Tab the previous), and past the last out of it. The default comes after the
 *  handlers' microtasks have run, as in a browser, so a dialog the key opened is already there,
 *  its control focused, when it does. */
export async function press(key: string, init: KeyboardEventInit = {}, target: Element = document.activeElement ?? document.body) {
  const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init });
  await act(async () => {
    target.dispatchEvent(event);
    await settle();
  });
  if (event.defaultPrevented) return;
  await act(async () => {
    const at = document.activeElement;
    if (key === "Enter" && at instanceof HTMLButtonElement) at.click();
    if (key === "Tab") {
      const stops = [...document.querySelectorAll<HTMLElement>("a[href], button, input, select, textarea, [tabindex]")].filter(
        (el) => el.tabIndex >= 0 && !el.matches(":disabled") && !el.closest("[hidden]"),
      );
      const next = stops[stops.indexOf(at as HTMLElement) + (init.shiftKey ? -1 : 1)];
      if (next) next.focus();
      else (at as HTMLElement | null)?.blur();
    }
    await settle();
  });
}

/** The controls Tab (Shift+Tab with `shiftKey`) reaches from where the focus is, round to it again,
 *  each inside `box()`. Throws if the focus leaves it on the way, or never comes back. */
export async function lap(box: () => Element | null, shiftKey = false): Promise<Element[]> {
  const start = document.activeElement;
  const seen: Element[] = [];
  for (let i = 0; i < 30; i++) {
    const at = document.activeElement;
    if (!at || !box()?.contains(at)) throw new Error(`the focus left the dialog for ${what(at)}, after ${seen.map(what).join(", ") || "nothing"}`);
    seen.push(at);
    await press("Tab", { shiftKey });
    if (document.activeElement === start) return seen;
  }
  throw new Error(`Tab never came back to ${what(start)}: ${seen.map(what).join(", ")}`);
}

/** An element as a failure names it: its label, else its text, else its tag. */
export function what(el: Element | null): string {
  if (!el) return "nothing";
  return el.getAttribute("aria-label") ?? (el.textContent?.trim().slice(0, 40) || el.nodeName.toLowerCase());
}
