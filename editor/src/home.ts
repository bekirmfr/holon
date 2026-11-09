// What the editor keeps for each person, on the server (holon-server's `/auth/prefs`), so it
// follows them to any browser: whether Getting started opens on start, their notes there, how the
// editor looks and which of its panes are hidden. Read back as pure functions, tested without a
// browser.

import { isTheme, isUi, type Theme, type Ui } from "./look";

export type Prefs = {
  /** Whether Getting started opens on start (absent: it does). */
  welcome?: boolean;
  /** Their own notes, under Getting started. */
  notes?: string;
  /** How the editor looks (look.ts; absent: as the system says). */
  theme?: Theme;
  /** Its interface, Clean or Terminal (look.ts; absent: Clean). */
  ui?: Ui;
  /** Which of the editor's panes are hidden (`c i v b`; absent: all shown). The text is each
   *  tab's own (tabs.ts's TextState). */
  hidden?: EditorPane[];
};

/** The editor's panes a key shows or hides, and remembered with the account: the canvas, the
 *  Inspector, view mode's values beside each output and its runs below. */
export const EDITOR_PANES = ["canvas", "inspector", "values", "bottom"] as const;
export type EditorPane = (typeof EDITOR_PANES)[number];

/** `hidden` with `pane` shown, or hidden. */
export function withPane(hidden: EditorPane[] | undefined, pane: EditorPane, shown: boolean): EditorPane[] {
  const rest = (hidden ?? []).filter((p) => p !== pane);
  return shown ? rest : [...rest, pane];
}

/** Prefs as read back: anything not understood dropped, so a bad save can't break the editor. */
export function readPrefs(raw: unknown): Prefs {
  if (!raw || typeof raw !== "object") return {};
  const r = raw as Record<string, unknown>;
  const hidden = Array.isArray(r.hidden) ? EDITOR_PANES.filter((p) => (r.hidden as unknown[]).includes(p)) : undefined;
  return {
    ...(typeof r.welcome === "boolean" ? { welcome: r.welcome } : {}),
    ...(typeof r.notes === "string" ? { notes: r.notes } : {}),
    ...(isTheme(r.theme) ? { theme: r.theme } : {}),
    ...(isUi(r.ui) ? { ui: r.ui } : {}),
    ...(hidden ? { hidden } : {}),
  };
}
