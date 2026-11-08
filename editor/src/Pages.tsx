// The pages shown over the canvas: saved flows, and deployments.

import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { holonIndex, type HolonUse } from "./holonUsage";
import { setMarketplaceUrl, type Listing } from "./marketplace";
import { getActivity, type ActivityEntry, type Role } from "./server";
import { deleteFlow, getFlow, listDeployments, listFlows, type DeploymentEntry, type FlowDocument, type FlowListEntry, type FlowRecord } from "./server";
import { ManifestSection, type ManifestView } from "./Manifest";
import { openChoice } from "./tabs";
import { useDialog } from "./Dialog";
import { DeploymentCard } from "./DeploymentCard";
import type { Sample } from "./samples";
import { flowAddressable, type Route } from "./routes";

/** Which top-level view is showing. The canvas is never unmounted (React Flow's own state —
 *  nodes mid-edit, viewport — lives there and must survive navigation); the other pages render
 *  as overlays on top of it. `marketplace` is deliberately absent: deferred, and a menu entry
 *  that can't be picked shouldn't be representable as a page state either. */
export type AppPage = "canvas" | "flows" | "deployed" | "admin" | "home" | "welcome" | "docs";
/** A page shown in a tab of its own: every page but the canvas (whose tabs are the flows'). */
export type PageTab = Exclude<AppPage, "canvas">;

/** The page that shows the place `r` names (routes.ts). A place the editor has no page of its
 *  own for yet is shown by the page that has what it names: the library's samples are on the
 *  flows page, a flow's page is its deployment's card, the host's pages are admin's. */
export function pageOf(r: Route): AppPage {
  switch (r.kind) {
    case "editor":
    case "editor-home":
    case "run":
      return "canvas";
    case "flows":
    case "library":
      return "flows";
    case "deployed":
    case "flow":
      return "deployed";
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
    default:
      return { kind: p };
  }
}

/** Where the address should say the editor is, when it names a place shown by another's page:
 *  that page's own place (a run is viewed in the editor). Null when `r` is shown as it is. */
export function shownAt(r: Route): Route | null {
  switch (r.kind) {
    case "run":
      return { kind: "editor", slug: r.slug, view: true, seq: r.seq };
    case "editor-home":
      return { kind: "editor", slug: null, view: false, seq: null };
    case "flow":
      return r.tab === "status" && flowAddressable(r.slug) ? null : { kind: "flow", slug: r.slug, tab: "status" };
    case "library":
    case "resources":
    case "people":
    case "policy":
    case "approvals":
    case "review":
    case "signin":
    case "notfound":
      return routeOfPage(pageOf(r));
    default:
      return null;
  }
}

/** The saved-flows page: list, open, delete, and publish to the marketplace this browser uses
 *  (set here: a deployment of the "marketplace" sample, marketplace.ts). "open" on a flow open in a tab goes to that tab, unsaved changes and all; only "reload saved"
 *  (shown when that tab has changes) loads the saved copy over them. */
export function FlowsPage({
  openTabs,
  onOpen,
  onSwitch,
  samples,
  onOpenSample,
  holonText,
  manifestOf,
  marketplace,
}: {
  /** The marketplace this browser uses ("" for none), told when it changes, and publishing a
   *  saved flow to it (answers what happened). */
  marketplace: {
    url: string;
    onChange: () => void;
    onPublish: (name: string) => Promise<string>;
    /** Its listings, every version, and yanking one (answers what happened). */
    listings: Listing[];
    adoption: (root: string, name: string) => number;
    onYank: (id: string) => Promise<string>;
  };
  /** The open tabs: a flow open in one is gone back to, not opened again. */
  openTabs: { id: number; name: string | null; dirty: boolean }[];
  /** A saved document's manifest items, derived as deploying it would (App: core's capabilities
   *  plus its deploy settings' requirements). Throws why it can't be derived. */
  manifestOf: (doc: FlowDocument) => ManifestView;
  /** Open a saved flow in a new tab, or (`reload`) over its open tab. */
  onOpen: (rec: FlowRecord, reload?: boolean) => void;
  onSwitch: (tab: number) => void;
  /** Sample flows (holon-author's samples.rs), one per capability: opening one starts an unsaved copy. */
  samples: Sample[];
  onOpenSample: (sample: Sample) => void;
  /** Flows as text (holonText.ts): a `.holon.ts` file opened as an unsaved flow
   *  (answers why not, when it can't be), and a saved flow written as one. */
  holonText: { onImport: (text: string) => string | null; exportOf: (name: string) => Promise<string>; textOfSample: (sample: Sample) => string };
}) {
  const dialog = useDialog();
  const [flows, setFlows] = useState<FlowListEntry[]>([]);
  const [msg, setMsg] = useState("");
  // Which saved flows place which as a node (holonUsage.ts).
  const [uses, setUses] = useState<Map<string, HolonUse[]>>(new Map());

  const refresh = useCallback(() => {
    listFlows()
      .then(async (f) => {
        setFlows(f);
        setMsg("");
        const deployed = await listDeployments().catch(() => []);
        setUses(await holonIndex(f, deployed, (name) => getFlow(name).then((r) => r.document)));
      })
      .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
  }, []);
  useEffect(() => {
    refresh();
  }, [refresh]);

  const tabOf = (name: string) => openTabs.find((t) => t.name === name);
  const open = (name: string) => {
    const tab = tabOf(name);
    if (openChoice(name, openTabs) === "switch" && tab) return onSwitch(tab.id);
    load(name, false);
  };
  const load = (name: string, reload: boolean) => {
    getFlow(name)
      .then((rec) => onOpen(rec, reload))
      .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
  };
  const remove = async (name: string) => {
    if (!(await dialog.confirm(`Delete the saved flow "${name}"? This can't be undone.`, "Delete"))) return;
    deleteFlow(name)
      .then(refresh)
      .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)));
  };

  const smallBtn: CSSProperties = {
    font: "inherit",
    fontSize: 11,
    padding: "2px 8px",
    color: "var(--muted)",
    background: "var(--surface-2)",
    border: "1px solid var(--border)",
    borderRadius: 3,
    cursor: "pointer",
  };

  return (
    <div style={{ maxWidth: 560 }}>
      <div style={{ color: "var(--body)", fontSize: 14, letterSpacing: "0.04em", marginBottom: 12, display: "flex", alignItems: "center", gap: 10 }}>
        <span style={{ flex: 1 }}>flows</span>
        <label style={{ ...smallBtn, fontSize: 11, letterSpacing: 0 }} title="open a flow written as text (NAME.holon.ts) as an unsaved flow, in a tab of its own">
          import text
          <input
            type="file"
            accept=".ts,text/plain"
            aria-label="import a flow's text"
            style={{ display: "none" }}
            onChange={async (e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (!file) return;
              const problem = holonText.onImport(await file.text());
              setMsg(problem ? `${file.name}: ${problem}` : "");
            }}
          />
        </label>
      </div>
      {msg && <div style={{ color: "var(--bad)", fontSize: 12, marginBottom: 8 }}>{msg}</div>}
      {flows.length === 0 && !msg && (
        <div style={{ color: "var(--muted)", fontSize: 12 }}>
          nothing saved yet — build something on the canvas and hit save
        </div>
      )}
      {flows.map((f) => (
        <div key={f.name} style={{ padding: "8px 10px", marginBottom: 6, background: "var(--surface-2)", border: "1px solid var(--border)", borderRadius: 4 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <span style={{ color: "var(--accent)", fontSize: 13, flex: 1 }}>{f.name}</span>
            <span style={{ color: "var(--muted)", fontSize: 11 }}>
              saved {new Date(f.saved_at_epoch_seconds * 1000).toLocaleString()}
              {f.saved_by ? ` by ${f.saved_by}` : ""}
            </span>
            {tabOf(f.name)?.dirty ? (
              <>
                <span style={{ color: "var(--warn)", fontSize: 11 }}>unsaved changes</span>
                <button style={smallBtn} onClick={() => open(f.name)} title="go to its tab, changes kept">
                  open
                </button>
                <button style={smallBtn} onClick={() => load(f.name, true)} title="discard your changes and load the saved copy">
                  reload saved
                </button>
              </>
            ) : (
              <button style={smallBtn} onClick={() => open(f.name)}>
                open
              </button>
            )}
            <button
              style={{ ...smallBtn, ...(marketplace.url ? {} : { opacity: 0.4, cursor: "default" }) }}
              disabled={!marketplace.url}
              title={marketplace.url ? `publish it to ${marketplace.url}, as it is saved now` : "set a marketplace first (below)"}
              onClick={() => marketplace.onPublish(f.name).then((m) => m && setMsg(m))}
            >
              publish
            </button>
            <button
              style={smallBtn}
              title="download it as text (.holon.ts): edit it anywhere, keep it in git, check it with `holon check`"
              onClick={() =>
                holonText
                  .exportOf(f.name)
                  .then((text) => {
                    const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
                    const a = document.createElement("a");
                    a.href = url;
                    a.download = `${f.name}.holon.ts`;
                    a.click();
                    URL.revokeObjectURL(url);
                  })
                  .catch((e: unknown) => setMsg(e instanceof Error ? e.message : String(e)))
              }
            >
              .holon.ts
            </button>
            <button style={smallBtn} onClick={() => remove(f.name)} title="delete this saved flow">
              ×
            </button>
          </div>
          <UsedBy uses={(uses.get(f.name) ?? []).filter((u) => u.kind === "flow")} lead="placed in" title="saved flows that place this one as a node; each keeps the version it placed until updated (↻ on the node)" />
          <UsedBy uses={(uses.get(f.name) ?? []).filter((u) => u.kind === "deployment")} lead="deployed in" title="deployments whose version places this one as a node; a redeploy takes the version its flow places" />
          <ManifestSection
            version={String(f.saved_at_epoch_seconds)}
            load={() => getFlow(f.name).then((rec) => manifestOf(rec.document))}
          />
        </div>
      ))}

      <div style={{ color: "var(--body)", fontSize: 14, letterSpacing: "0.04em", margin: "24px 0 4px" }}>marketplace</div>
      <div style={{ color: "var(--muted)", fontSize: 11, marginBottom: 10, display: "flex", gap: 8, alignItems: "center" }}>
        <span>
          {marketplace.url ? (
            <>
              publishing to and placing from <span style={{ color: "var(--body)" }}>{marketplace.url}</span>
            </>
          ) : (
            "none set: deploy the Marketplace sample (or use someone's), then set its address"
          )}
        </span>
        <button
          style={smallBtn}
          onClick={async () => {
            const url = await dialog.prompt("The marketplace's address (a Marketplace deployment, e.g. http://127.0.0.1:8120); empty for none:", marketplace.url);
            if (url === null) return;
            setMarketplaceUrl(url);
            marketplace.onChange();
          }}
        >
          {marketplace.url ? "change" : "set"}
        </button>
      </div>
      {marketplace.listings.map((l) => (
        <div key={l.id} style={{ display: "flex", gap: 8, alignItems: "center", fontSize: 11, color: "var(--muted)", padding: "2px 0" }}>
          <span style={{ color: "var(--accent)" }}>{l.name}</span>
          <span style={{ color: "var(--faint)" }} title={l.root}>
            {l.root.slice(0, 8)} · by {l.publisher} · {new Date(l.published_at * 1000).toLocaleString()}
            {marketplace.adoption(l.root, l.name) > 0 && ` · placed in ${marketplace.adoption(l.root, l.name)} other(s)`}
          </span>
          <span style={{ flex: 1 }} />
          {l.yanked ? (
            <span style={{ color: "var(--bad)" }}>yanked</span>
          ) : (
            <button style={smallBtn} onClick={() => marketplace.onYank(l.id).then((m) => m && setMsg(m))} title="flag this version as yanked (it stays listed, never edited)">
              yank
            </button>
          )}
        </div>
      ))}

      <div style={{ color: "var(--body)", fontSize: 14, letterSpacing: "0.04em", margin: "24px 0 4px" }}>samples</div>
      <div style={{ color: "var(--muted)", fontSize: 11, marginBottom: 10 }}>
        one per capability; opening one starts an unsaved copy you can change, save and deploy
      </div>
      {samples.map((s) => (
        <div
          key={s.name}
          style={{
            display: "flex",
            alignItems: "flex-start",
            gap: 10,
            padding: "8px 10px",
            marginBottom: 6,
            background: "var(--surface-2)",
            border: "1px solid var(--border)",
            borderRadius: 4,
          }}
        >
          <div style={{ flex: 1 }}>
            <div style={{ color: "var(--accent)", fontSize: 13 }}>{s.title}</div>
            <div style={{ color: "var(--body)", fontSize: 11, margin: "2px 0" }}>{s.description}</div>
            <div style={{ color: "var(--faint)", fontSize: 10 }}>
              shows {s.shows.join(" · ")}
              {s.secrets && s.secrets.length > 0 && <> · needs secrets {s.secrets.map((x) => (x.from ? `${x.name} (${x.from})` : x.name)).join(", ")} (set under ⚙)</>}
            </div>
            <div style={{ color: "var(--faint)", fontSize: 10 }} title="once deployed">
              try: {s.tryIt}
            </div>
          </div>
          <button style={smallBtn} onClick={() => onOpenSample(s)} title={`open "${s.name}" as an unsaved copy`}>
            open
          </button>
          <button
            style={smallBtn}
            title="download it as text (.holon.ts), as it is written: read it, change it anywhere, check it with `holon check`"
            onClick={() => {
              try {
                const url = URL.createObjectURL(new Blob([holonText.textOfSample(s)], { type: "text/plain" }));
                const a = document.createElement("a");
                a.href = url;
                a.download = `${s.name}.holon.ts`;
                a.click();
                URL.revokeObjectURL(url);
              } catch (e) {
                setMsg(e instanceof Error ? e.message : String(e));
              }
            }}
          >
            .holon.ts
          </button>
        </div>
      ))}
    </div>
  );
}

/** Who places a holon, of one kind (saved flows or deployments), and whether from an older save. */
function UsedBy({ uses, lead, title }: { uses: HolonUse[]; lead: string; title: string }) {
  if (uses.length === 0) return null;
  return (
    <div style={{ color: "var(--faint)", fontSize: 11, marginTop: 4 }} title={title}>
      {lead}{" "}
      {uses.map((u, i) => (
        <span key={u.user}>
          {i > 0 && ", "}
          <span style={{ color: "var(--body)" }}>{u.user}</span>
          {u.nodes > 1 && ` ×${u.nodes}`}
          {!u.current && <span style={{ color: "var(--warn)" }}> (an older save)</span>}
        </span>
      ))}
    </div>
  );
}

/** The deployed page: the server's live table, one card per deployment. */
export function DeployedPage({
  deployments,
  error,
  onAction,
  onDelete,
  onView,
  onChanged,
  focus,
  role,
}: {
  deployments: DeploymentEntry[];
  error: string;
  onAction: (slug: string, action: "run" | "pause" | "stop", from?: "latest" | "fresh" | number) => Promise<void>;
  onDelete: (slug: string) => void;
  /** Open a deployment in view mode, on one of its runs when `seq` is given. */
  onView: (slug: string, seq?: number) => void;
  onChanged: () => void;
  /** The card asked for (from the admin page): shown with its permissions open, again at each `n`. */
  focus?: { slug: string; n: number } | null;
  /** Who is signed in: a viewer sees a deployment's actions, never runs one. */
  role?: Role;
}) {
  return (
    <div style={{ maxWidth: 760 }}>
      <div style={{ color: "var(--body)", fontSize: 14, letterSpacing: "0.04em", marginBottom: 4 }}>
        deployed
      </div>
      <div style={{ color: "var(--muted)", fontSize: 11, marginBottom: 12 }}>
        {deployments.length} deployment{deployments.length === 1 ? "" : "s"} ·{" "}
        {deployments.reduce((a, d) => a + d.trigger_count, 0)} fires ·{" "}
        {deployments.reduce((a, d) => a + d.error_count, 0)} errors
      </div>
      {error && <div style={{ color: "var(--bad)", fontSize: 12, marginBottom: 8 }}>{error}</div>}
      {deployments.length === 0 && (
        <div style={{ color: "var(--muted)", fontSize: 12 }}>
          nothing deployed — project a flow on the canvas, then deploy it
        </div>
      )}
      {deployments.map((entry) => (
        <DeploymentCard
          key={entry.slug}
          entry={entry}
          onAction={(action, from) => onAction(entry.slug, action, from)}
          onDelete={() => onDelete(entry.slug)}
          onView={(seq) => onView(entry.slug, seq)}
          onChanged={onChanged}
          role={role}
          focus={focus?.slug === entry.slug ? focus.n : undefined}
        />
      ))}
      <ActivityList version={deployments.map((d) => `${d.slug}:${d.status}:${d.unit}`).join(",")} />
    </div>
  );
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
