// What the editor keeps for each person, on the server (holon-server's `/auth/prefs`), so it
// follows them to any browser: whether the welcome tab opens on start, and their home tab's
// panes. The home layout's operations are pure, so they are tested without a browser.

/** The panes a home tab can hold. */
export const PANE_KINDS = ["start", "recent", "deployments", "activity", "marketplace", "samples", "notes", "guide"] as const;
export type PaneKind = (typeof PANE_KINDS)[number];

/** What each pane is, as the "add a pane" menu offers it. */
export const PANE_INFO: Record<PaneKind, { title: string; about: string }> = {
  start: { title: "start", about: "new flow, the flows page, the welcome" },
  recent: { title: "recent flows", about: "your saved flows, newest first" },
  deployments: { title: "deployments", about: "what runs, and how it's doing" },
  activity: { title: "activity", about: "what was done on the server, by whom" },
  marketplace: { title: "marketplace", about: "the newest holons on your marketplace" },
  samples: { title: "samples", about: "flows to start from, one per feature" },
  notes: { title: "notes", about: "your own notes, kept with your account" },
  guide: { title: "getting started", about: "the first steps, ticked off as you go" },
};

/** One pane on the home tab: what it shows, whether it spans both columns, and (for notes) its
 *  text. */
export type Pane = { id: string; kind: PaneKind; wide: boolean; note?: string };

export type Prefs = {
  /** Whether the welcome tab opens on start (absent: it does). */
  welcome?: boolean;
  /** The home tab's panes, in order (absent: the default). */
  home?: Pane[];
};

export const defaultHome = (): Pane[] => [
  { id: "start", kind: "start", wide: false },
  { id: "recent", kind: "recent", wide: false },
  { id: "deployments", kind: "deployments", wide: true },
  { id: "activity", kind: "activity", wide: false },
  { id: "samples", kind: "samples", wide: false },
];

/** A pane of `kind` added at the end, with an id of its own. */
export function addPane(panes: Pane[], kind: PaneKind): Pane[] {
  let n = 1;
  while (panes.some((p) => p.id === `${kind}-${n}`)) n++;
  return [...panes, { id: `${kind}-${n}`, kind, wide: kind === "deployments" }];
}

export const removePane = (panes: Pane[], id: string): Pane[] => panes.filter((p) => p.id !== id);

/** The pane `id` moved one place earlier (-1) or later (1); unchanged at either end. */
export function movePane(panes: Pane[], id: string, by: -1 | 1): Pane[] {
  const i = panes.findIndex((p) => p.id === id);
  const j = i + by;
  if (i < 0 || j < 0 || j >= panes.length) return panes;
  const next = [...panes];
  [next[i], next[j]] = [next[j], next[i]];
  return next;
}

export const changePane = (panes: Pane[], id: string, change: Partial<Omit<Pane, "id" | "kind">>): Pane[] =>
  panes.map((p) => (p.id === id ? { ...p, ...change } : p));

/** Prefs as read back: anything not understood dropped, so a bad save can't break the editor. */
export function readPrefs(raw: unknown): Prefs {
  if (!raw || typeof raw !== "object") return {};
  const r = raw as Record<string, unknown>;
  const home = Array.isArray(r.home)
    ? (r.home as Pane[]).filter((p) => p && typeof p.id === "string" && (PANE_KINDS as readonly string[]).includes(p.kind)).map((p) => ({ ...p, wide: !!p.wide }))
    : undefined;
  return { ...(typeof r.welcome === "boolean" ? { welcome: r.welcome } : {}), ...(home ? { home } : {}) };
}
