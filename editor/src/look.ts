// How the editor looks: light, dark, or as the system says (auto); and its interface, Clean or
// Terminal (gaps-web.md G33: mono, square, `[ ]` buttons, keys on show, a status bar, one dark
// look). Both choices are kept with the account (home.ts's Prefs) and, for the first paint before
// the account is read, in this browser. They are set as `data-theme` and `data-ui="b"` on
// <html>; styles.css has a token set for each. What can't read the tokens (CodeMirror's own theme,
// React Flow's) asks `useDark`.

import { useSyncExternalStore } from "react";

export type Theme = "auto" | "light" | "dark";
export const THEMES: readonly Theme[] = ["auto", "light", "dark"];

const KEY = "holon.theme";
const UI_KEY = "holon.ui";

export type Ui = "clean" | "terminal";
export const UIS: readonly Ui[] = ["clean", "terminal"];

export function isUi(v: unknown): v is Ui {
  return typeof v === "string" && (UIS as readonly string[]).includes(v);
}

/** Show the editor's interface as `u`, and remember it in this browser. */
export function applyUi(u: Ui) {
  const root = document.documentElement;
  if (u === "terminal") root.setAttribute("data-ui", "b");
  else root.removeAttribute("data-ui");
  try {
    localStorage.setItem(UI_KEY, u);
  } catch {
    // As for the theme: shown until the next load.
  }
}

/** The interface this browser last had; Clean when it kept none. */
export function storedUi(): Ui {
  try {
    const u = localStorage.getItem(UI_KEY);
    return isUi(u) ? u : "clean";
  } catch {
    return "clean";
  }
}

export function isTheme(v: unknown): v is Theme {
  return typeof v === "string" && (THEMES as readonly string[]).includes(v);
}

/** Show the editor as `t`, and remember it in this browser. */
export function applyTheme(t: Theme) {
  const root = document.documentElement;
  if (t === "auto") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", t);
  try {
    localStorage.setItem(KEY, t);
  } catch {
    // A browser that keeps nothing still shows the choice, until the next load.
  }
}

/** The look this browser last had; auto when it kept none. */
export function storedTheme(): Theme {
  try {
    const t = localStorage.getItem(KEY);
    return isTheme(t) ? t : "auto";
  } catch {
    return "auto";
  }
}

const systemDark = () => typeof window.matchMedia === "function" && window.matchMedia("(prefers-color-scheme: dark)").matches;

/** Whether the editor is dark now: the Terminal interface always is; otherwise as set, or as the
 *  system says. */
export function isDark(): boolean {
  if (document.documentElement.getAttribute("data-ui") === "b") return true;
  const t = document.documentElement.getAttribute("data-theme");
  return t === "dark" || (t !== "light" && systemDark());
}

function subscribe(changed: () => void): () => void {
  const watch = new MutationObserver(changed);
  watch.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "data-ui"] });
  const media = typeof window.matchMedia === "function" ? window.matchMedia("(prefers-color-scheme: dark)") : null;
  media?.addEventListener("change", changed);
  return () => {
    watch.disconnect();
    media?.removeEventListener("change", changed);
  };
}

/** Whether the editor is dark, kept current as the choice or the system changes. */
export function useDark(): boolean {
  return useSyncExternalStore(subscribe, isDark);
}
