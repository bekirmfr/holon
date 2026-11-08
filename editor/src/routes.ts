// Where the editor is, as the address's hash: every page, a flow's tabs, a run, and the editor's
// flow and mode, so Back and Forward work, a reload keeps the place, and a place can be linked.
// Tokens are joined by dots (`#refunds.runs`, `#editor.refunds.view.run-12`); a flow's name has
// none (letters, digits, - and _), so a dot always separates. A flow's own page is its bare name
// (`#refunds`): the page names below are taken, so a flow named like one is reached from Flows.
//
// The pages the address knows are the console's (gaps-web.md §2), and the ones the editor has
// today besides them (deployed, admin). What each shows is App's; this only reads and
// writes the address, and says what is above a place (Esc goes there).

export const FLOW_TABS = ["status", "runs", "history", "state", "settings"] as const;
export type FlowTab = (typeof FLOW_TABS)[number];

export const LIBRARY_TABS = ["tutorials", "samples", "market"] as const;
export type LibraryTab = (typeof LIBRARY_TABS)[number];

export type Route =
  | { kind: "overview" }
  | { kind: "approvals"; tab: "deploys" | "runs" }
  /** A version waiting for a person, reviewed: `#approvals.<slug>.v<n>`. */
  | { kind: "review"; slug: string; version: number }
  | { kind: "flows" }
  /** A flow's page and one of its tabs: `#<slug>`, `#<slug>.runs`. */
  | { kind: "flow"; slug: string; tab: FlowTab }
  /** One run's record: `#<slug>.run-<seq>`. */
  | { kind: "run"; slug: string; seq: number }
  /** The editor's own home (open a flow, start new, recently closed). */
  | { kind: "editor-home" }
  /** The editor on a flow: `#editor` (whatever tab is open, or an untitled draft),
   *  `#editor.<slug>` to edit it, `.view` to view its deployment, `.run-<seq>` on a run. */
  | { kind: "editor"; slug: string | null; view: boolean; seq: number | null }
  | { kind: "library"; tab: LibraryTab }
  | { kind: "resources"; tab: string | null }
  | { kind: "people"; tab: string | null }
  | { kind: "policy" }
  | { kind: "start" }
  | { kind: "docs"; page: string | null; anchor: string | null }
  | { kind: "signin" }
  | { kind: "deployed" }
  | { kind: "admin"; tab: string | null }
  | { kind: "notfound"; hash: string };

/** The page names: a flow can't be addressed by one of these alone. */
export const PAGE_NAMES = [
  "overview",
  "approvals",
  "flows",
  "editor",
  "library",
  "resources",
  "people",
  "policy",
  "start",
  "docs",
  "signin",
  "deployed",
  "admin",
] as const;

const SLUG = /^[A-Za-z0-9_-]+$/;
const isPage = (t: string): boolean => (PAGE_NAMES as readonly string[]).includes(t);
const isFlowTab = (t: string): t is FlowTab => (FLOW_TABS as readonly string[]).includes(t);
const runOf = (t: string | undefined): number | null => {
  const m = t === undefined ? null : /^run-(\d+)$/.exec(t);
  return m ? Number(m[1]) : null;
};
const tabOf = (t: string | undefined): string | null => (t !== undefined && SLUG.test(t) ? t : null);

/** The place an address's hash names (with or without its `#`); an empty one is the overview. */
export function parse(hash: string): Route {
  const raw = hash.replace(/^#/, "");
  if (raw === "") return { kind: "overview" };
  const head = raw.split(".")[0];
  // A manual page's id has dots and slashes of its own: the rest is one token.
  if (head === "docs") {
    const rest = raw.slice("docs".length + 1);
    if (rest === "") return { kind: "docs", page: null, anchor: null };
    const [page, anchor] = rest.split("@");
    return { kind: "docs", page: decode(page), anchor: anchor === undefined ? null : decode(anchor) };
  }
  const p = raw.split(".");
  const notFound: Route = { kind: "notfound", hash: raw };
  const done = (r: Route, used: number): Route => (p.length === used ? r : notFound);
  switch (head) {
    case "overview":
    case "flows":
    case "policy":
    case "start":
    case "signin":
    case "deployed":
      return done({ kind: head }, 1);
    case "approvals": {
      if (p.length === 1) return { kind: "approvals", tab: "deploys" };
      if (p[1] === "runs") return done({ kind: "approvals", tab: "runs" }, 2);
      const version = /^v(\d+)$/.exec(p[2] ?? "");
      return SLUG.test(p[1]) && version ? done({ kind: "review", slug: p[1], version: Number(version[1]) }, 3) : notFound;
    }
    case "editor": {
      if (p.length === 1) return { kind: "editor", slug: null, view: false, seq: null };
      if (p[1] === "home") return done({ kind: "editor-home" }, 2);
      if (!SLUG.test(p[1])) return notFound;
      if (p.length === 2) return { kind: "editor", slug: p[1], view: false, seq: null };
      if (p[2] !== "view") return notFound;
      if (p.length === 3) return { kind: "editor", slug: p[1], view: true, seq: null };
      const seq = runOf(p[3]);
      return seq === null ? notFound : done({ kind: "editor", slug: p[1], view: true, seq }, 4);
    }
    case "library": {
      const tab = p[1] ?? "samples";
      return (LIBRARY_TABS as readonly string[]).includes(tab) ? done({ kind: "library", tab: tab as LibraryTab }, p.length === 1 ? 1 : 2) : notFound;
    }
    case "resources":
    case "people":
    case "admin": {
      const tab = tabOf(p[1]);
      if (p.length > 1 && tab === null) return notFound;
      return done({ kind: head, tab }, p.length === 1 ? 1 : 2);
    }
  }
  if (!SLUG.test(head)) return notFound;
  if (p.length === 1) return { kind: "flow", slug: head, tab: "status" };
  const seq = runOf(p[1]);
  if (seq !== null) return done({ kind: "run", slug: head, seq }, 2);
  return isFlowTab(p[1]) ? done({ kind: "flow", slug: head, tab: p[1] }, 2) : notFound;
}

/** The hash (with its `#`) that names `r`; `parse(format(r))` is `r`. */
export function format(r: Route): string {
  return `#${tokens(r)}`;
}

function tokens(r: Route): string {
  switch (r.kind) {
    case "approvals":
      return r.tab === "runs" ? "approvals.runs" : "approvals";
    case "review":
      return `approvals.${r.slug}.v${r.version}`;
    case "flow":
      return r.tab === "status" ? r.slug : `${r.slug}.${r.tab}`;
    case "run":
      return `${r.slug}.run-${r.seq}`;
    case "editor-home":
      return "editor.home";
    case "editor":
      if (r.slug === null) return "editor";
      return `editor.${r.slug}${r.view ? ".view" : ""}${r.view && r.seq !== null ? `.run-${r.seq}` : ""}`;
    case "library":
      return r.tab === "samples" ? "library" : `library.${r.tab}`;
    case "resources":
    case "people":
    case "admin":
      return r.tab === null ? r.kind : `${r.kind}.${r.tab}`;
    case "docs":
      return r.page === null ? "docs" : `docs.${encode(r.page)}${r.anchor === null ? "" : `@${encode(r.anchor)}`}`;
    case "notfound":
      return r.hash;
    default:
      return r.kind;
  }
}

/** Whether two routes name the same place. */
export function same(a: Route, b: Route): boolean {
  return format(a) === format(b);
}

/** What is above `r` (Esc goes there): a run → its flow's runs → the flow → Flows → the
 *  overview; the overview has nothing above it. */
export function parentOf(r: Route): Route | null {
  switch (r.kind) {
    case "overview":
      return null;
    case "review":
      return { kind: "approvals", tab: "deploys" };
    case "approvals":
      return r.tab === "runs" ? { kind: "approvals", tab: "deploys" } : { kind: "overview" };
    case "run":
      return { kind: "flow", slug: r.slug, tab: "runs" };
    case "flow":
      return r.tab === "status" ? { kind: "flows" } : { kind: "flow", slug: r.slug, tab: "status" };
    case "editor":
      if (r.slug === null) return { kind: "overview" };
      if (r.view && r.seq !== null) return { ...r, seq: null };
      if (r.view) return { ...r, view: false };
      return { kind: "editor", slug: null, view: false, seq: null };
    case "editor-home":
      return { kind: "editor", slug: null, view: false, seq: null };
    case "library":
      return r.tab === "samples" ? { kind: "overview" } : { kind: "library", tab: "samples" };
    case "resources":
    case "people":
    case "admin":
      return r.tab === null ? { kind: "overview" } : { kind: r.kind, tab: null };
    case "docs":
      return r.page === null ? { kind: "overview" } : { kind: "docs", page: null, anchor: null };
    default:
      return { kind: "overview" };
  }
}

/** What a place is called in the crumbs: a page by its name, a flow by its own, a tab by its. */
export function labelOf(r: Route): string {
  switch (r.kind) {
    case "overview":
      return "Overview";
    case "approvals":
      return r.tab === "runs" ? "Runs" : "Approvals";
    case "review":
      return `${r.slug} v${r.version}`;
    case "flows":
      return "Flows";
    case "flow":
      return r.tab === "status" ? r.slug : r.tab;
    case "run":
      return `run ${r.seq}`;
    case "editor-home":
      return "home";
    case "editor":
      if (r.slug === null) return "Editor";
      if (r.view && r.seq !== null) return `run ${r.seq}`;
      return r.view ? "deployed" : r.slug;
    case "library":
      return r.tab === "samples" ? "Library" : r.tab;
    case "resources":
      return r.tab ?? "Resources";
    case "people":
      return r.tab ?? "People and access";
    case "admin":
      return r.tab ?? "Admin";
    case "policy":
      return "Policy";
    case "start":
      return "Getting started";
    case "docs":
      return r.page === null ? "Docs" : (r.page.split("/").pop() ?? r.page).replace(/\.md$/, "");
    case "signin":
      return "Sign in";
    case "deployed":
      return "Deployed";
    case "notfound":
      return `#${r.hash}`;
  }
}

/** The places above `r` and `r` itself, from the overview down: the top bar's crumbs. */
export function crumbs(r: Route): Route[] {
  const chain: Route[] = [];
  for (let at: Route | null = r; at; at = parentOf(at)) chain.unshift(at);
  return chain;
}

/** Whether `name` can be a flow's own page address (it is not a page's name). */
export function flowAddressable(name: string): boolean {
  return SLUG.test(name) && !isPage(name);
}

function encode(s: string): string {
  return encodeURIComponent(s).replace(/%2F/g, "/");
}

function decode(s: string): string {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}
