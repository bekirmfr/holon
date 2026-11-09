// The pages shown over the canvas: which shows each place the address names, and the server's
// activity (home's pane). The pages themselves are files of their own (FlowsPage, LibraryPage…).

import { useEffect, useState } from "react";
import { getActivity, type ActivityEntry } from "./server";
import { flowAddressable, type Route } from "./routes";

/** Which top-level view is showing. The canvas is never unmounted (React Flow's own state —
 *  nodes mid-edit, viewport — lives there and must survive navigation); the other pages render
 *  as overlays on top of it. `marketplace` is deliberately absent: deferred, and a menu entry
 *  that can't be picked shouldn't be representable as a page state either. */
export type AppPage = "canvas" | "approvals" | "flows" | "library" | "flow" | "admin" | "home" | "welcome" | "docs";
/** A page shown in a tab of its own: every page but the canvas (whose tabs are the flows') and a
 *  flow's page (reached from the nav, the flows and the palette). */
export type PageTab = Exclude<AppPage, "canvas" | "flow">;

/** The page that shows the place `r` names (routes.ts). A place the editor has no page of its
 *  own for yet is shown by the page that has what it names: the host's pages are admin's. */
export function pageOf(r: Route): AppPage {
  switch (r.kind) {
    case "editor":
    case "editor-home":
      return "canvas";
    case "approvals":
    case "review":
      return "approvals";
    case "flows":
      return "flows";
    case "library":
      return "library";
    case "flow":
    case "run":
      return "flow";
    case "docs":
      return "docs";
    case "start":
      return "welcome";
    case "admin":
    case "resources":
    case "people":
    case "policy":
      return "admin";
    default:
      return "home";
  }
}

/** The place page `p` is at, when it is asked for by itself. */
export function routeOfPage(p: AppPage): Route {
  switch (p) {
    case "canvas":
      return { kind: "editor", slug: null, view: false, seq: null };
    case "home":
      return { kind: "overview" };
    case "welcome":
      return { kind: "start" };
    case "docs":
      return { kind: "docs", page: null, anchor: null };
    case "admin":
      return { kind: "admin", tab: null };
    case "flow":
      return { kind: "flows" };
    case "library":
      return { kind: "library", tab: "samples" };
    case "approvals":
      return { kind: "approvals", tab: "deploys" };
    default:
      return { kind: p };
  }
}

/** Where the address should say the editor is, when it names a place shown by another's page:
 *  that page's own place (a run is viewed in the editor). Null when `r` is shown as it is. */
export function shownAt(r: Route): Route | null {
  switch (r.kind) {
    case "editor-home":
      return { kind: "editor", slug: null, view: false, seq: null };
    case "flow":
    case "run":
      return flowAddressable(r.slug) ? null : { kind: "flows" };
    case "resources":
    case "people":
    case "policy":
    case "signin":
    case "notfound":
      return routeOfPage(pageOf(r));
    default:
      return null;
  }
}

/** What was done on the server, newest first, and by whom (activity.rs): read again whenever the
 *  deployments change (`version`). */
/** `bare`: without its own heading (inside a home pane, which has one). */
export function ActivityList({ version, bare = false }: { version: string; bare?: boolean }) {
  const [entries, setEntries] = useState<ActivityEntry[] | null>(null);
  useEffect(() => {
    getActivity()
      .then(setEntries)
      .catch(() => setEntries(null));
  }, [version]);
  if (!entries || entries.length === 0) return bare ? <span style={{ color: "var(--faint)", fontSize: 11 }}>nothing yet</span> : null;
  return (
    <div style={{ marginTop: bare ? 0 : 20 }}>
      {!bare && <div style={{ color: "var(--body)", fontSize: 13, letterSpacing: "0.04em", marginBottom: 6 }}>activity</div>}
      {entries.map((e, i) => (
        <div key={i} style={{ color: "var(--muted)", fontSize: 11, padding: "2px 0" }}>
          <span style={{ color: "var(--faint)" }}>{new Date(e.at * 1000).toLocaleString()}</span> · {e.action}{" "}
          <span style={{ color: "var(--accent)" }}>{e.target}</span>
          {e.by && <span style={{ color: "var(--faint)" }}> · by {e.by}</span>}
        </div>
      ))}
    </div>
  );
}
