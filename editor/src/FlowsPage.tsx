// The Flows page (gaps-web.md G14, C2): every flow in one table, saved and deployed alike, each
// with its state (a draft not deployed yet; live, paused or stopped; waiting runs), what it may
// do, its runs and when it last changed. A deployed flow's row opens its page; a draft's opens it
// in the editor. A saved flow is also opened, published, written as text and deleted from here.

import { useCallback, useEffect, useMemo, useState } from "react";
import { describeItem } from "./holon";
import { holonIndex, type HolonUse } from "./holonUsage";
import { deleteFlow, getFlow, getManifest, listDeployments, listFlows, type DeploymentEntry, type FlowDocument, type FlowListEntry, type FlowRecord } from "./server";
import { openChoice } from "./tabs";
import { useDialog } from "./Dialog";
import { follow, format, type Route } from "./routes";
import { useListKeys } from "./ui/Keys";

type Show = "all" | "live" | "waiting" | "paused";
const SHOWS: [Show, string, string][] = [
  ["all", "All", "every flow, saved or deployed"],
  ["live", "Live", "deployed and running"],
  ["waiting", "Waiting", "with runs waiting for an approval or an event"],
  ["paused", "Paused", "deployed, paused or stopped"],
];

/** One flow: its saved copy, its deployment, or both. */
type Row = { name: string; saved?: FlowListEntry; deployed?: DeploymentEntry };

/** Where a flow's row goes: its page when deployed, else the editor on it. */
const placeOf = (r: Row): Route => (r.deployed ? { kind: "flow", slug: r.name, tab: "status" } : { kind: "editor", slug: r.name, view: false, seq: null });

const STATE_SAID: Record<DeploymentEntry["status"], string> = { running: "live", paused: "paused", stopped: "stopped" };

function shows(r: Row, show: Show): boolean {
  const d = r.deployed;
  switch (show) {
    case "all":
      return true;
    case "live":
      return d?.status === "running";
    case "waiting":
      return (d?.waiting ?? 0) > 0;
    case "paused":
      return d !== undefined && d.status !== "running";
  }
}

const when = (seconds: number) => new Date(seconds * 1000).toLocaleString();

export function FlowsPage({
  deployments,
  error,
  openTabs,
  onOpen,
  onSwitch,
  onNew,
  navigate,
  onImport,
  exportOf,
  manifestOf,
  marketplace,
}: {
  /** The server's table (useDeployments), kept fresh by App. */
  deployments: DeploymentEntry[];
  error: string;
  /** The open tabs: a flow open in one is gone back to, not opened again. */
  openTabs: { id: number; name: string | null; dirty: boolean }[];
  /** Open a saved flow in a new tab, or (`reload`) over its open tab. */
  onOpen: (rec: FlowRecord, reload?: boolean) => void;
  onSwitch: (tab: number) => void;
  onNew: () => void;
  navigate: (r: Route) => void;
  /** A `.holon.ts` file opened as an unsaved flow; why not, when it can't be. */
  onImport: (text: string) => string | null;
  /** A saved flow written as text. */
  exportOf: (name: string) => Promise<string>;
  /** A saved document's manifest items, derived as deploying it would (App). */
  manifestOf: (doc: FlowDocument) => { items: string[] };
  /** The marketplace this browser publishes to ("" for none, set on the Library's Marketplace
   *  tab), and publishing a saved flow to it (answers what happened). */
  marketplace: { url: string; onPublish: (name: string) => Promise<string> };
}) {
  const dialog = useDialog();
  const [flows, setFlows] = useState<FlowListEntry[]>([]);
  const [msg, setMsg] = useState("");
  // What a publish answered.
  const [note, setNote] = useState("");
  const [search, setSearch] = useState("");
  const [show, setShow] = useState<Show>("all");
  // Which saved flows place which as a node (holonUsage.ts).
  const [uses, setUses] = useState<Map<string, HolonUse[]>>(new Map());
  const say = (e: unknown) => setMsg(e instanceof Error ? e.message : String(e));

  const refresh = useCallback(() => {
    listFlows()
      .then(async (f) => {
        setFlows(f);
        setMsg("");
        const deployed = await listDeployments().catch(() => []);
        setUses(await holonIndex(f, deployed, (name) => getFlow(name).then((r) => r.document)));
      })
      .catch(say);
  }, []);
  useEffect(() => {
    refresh();
  }, [refresh]);

  const rows = useMemo(() => {
    const byName = new Map<string, Row>();
    for (const f of flows) byName.set(f.name, { name: f.name, saved: f });
    for (const d of deployments) byName.set(d.slug, { ...(byName.get(d.slug) ?? { name: d.slug }), deployed: d });
    const changed = (r: Row) => Math.max(r.saved?.saved_at_epoch_seconds ?? 0, r.deployed?.deployed_at_epoch_seconds ?? 0);
    return [...byName.values()].sort((a, b) => changed(b) - changed(a) || a.name.localeCompare(b.name));
  }, [flows, deployments]);
  const counts = useMemo(() => Object.fromEntries(SHOWS.map(([s]) => [s, rows.filter((r) => shows(r, s)).length])) as Record<Show, number>, [rows]);
  const q = search.trim().toLowerCase();
  const shown = rows.filter((r) => shows(r, show) && (q === "" || r.name.toLowerCase().includes(q)));

  // j and k move through the rows shown, Enter opens one as its link does.
  const list = useListKeys(
    "flows",
    shown.map((r) => r.name),
    (name) => {
      const r = shown.find((x) => x.name === name);
      if (r) navigate(placeOf(r));
    },
  );

  const tabOf = (name: string) => openTabs.find((t) => t.name === name);
  const load = (name: string, reload: boolean) => {
    getFlow(name)
      .then((rec) => onOpen(rec, reload))
      .catch(say);
  };
  const open = (r: Row) => {
    const tab = tabOf(r.name);
    if (openChoice(r.name, openTabs) === "switch" && tab) return onSwitch(tab.id);
    if (r.saved) return load(r.name, false);
    navigate({ kind: "editor", slug: r.name, view: false, seq: null });
  };
  const remove = async (name: string) => {
    if (!(await dialog.confirm(`Delete the saved flow "${name}"? This can't be undone.`, "Delete"))) return;
    deleteFlow(name).then(refresh).catch(say);
  };
  const download = (name: string) =>
    exportOf(name)
      .then((text) => {
        const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
        const a = document.createElement("a");
        a.href = url;
        a.download = `${name}.holon.ts`;
        a.click();
        URL.revokeObjectURL(url);
      })
      .catch(say);

  return (
    <div className="hl-flow hl-flows">
      <header className="hl-flow__head">
        <h1 className="hl-flow__name">Flows</h1>
        <span className="hl-flow__status">
          {rows.length} flow{rows.length === 1 ? "" : "s"} · {deployments.length} deployed
        </span>
        <span className="hl-flow__spacer" />
        <label className="hl-flow__button" title="open a flow written as text (NAME.holon.ts) as an unsaved flow, in a tab of its own">
          Import
          <input
            type="file"
            accept=".ts,text/plain"
            aria-label="import a flow's text"
            style={{ display: "none" }}
            onChange={async (e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (!file) return;
              const problem = onImport(await file.text());
              setMsg(problem ? `${file.name}: ${problem}` : "");
            }}
          />
        </label>
        <button className="hl-flow__button hl-flow__button--go" onClick={onNew}>
          New flow
        </button>
      </header>
      <div className="hl-flow__bar">
        <input className="hl-flow__filter" type="search" placeholder="Search flows" aria-label="search flows" value={search} onChange={(e) => setSearch(e.target.value)} />
        <div className="hl-seg" role="group" aria-label="show">
          {SHOWS.map(([s, label, title]) => (
            <button key={s} className="hl-seg__opt" aria-pressed={show === s} title={title} onClick={() => setShow(s)}>
              {label}
              <span className="hl-flow__count">{counts[s]}</span>
            </button>
          ))}
        </div>
      </div>
      {(msg || error) && <div className="hl-flow__bad">{msg || error}</div>}
      {note && <div className="hl-flow__said">{note}</div>}
      {rows.length === 0 && !msg ? (
        <p className="hl-flow__none">Nothing yet: start a new flow, or open a sample from the Library.</p>
      ) : (
        <section aria-label="flows">
          <table className="hl-table">
            <thead>
              <tr>
                <th>Flow</th>
                <th>State</th>
                <th title="what deploying it asks for, or its deployment was granted">It may</th>
                <th>Runs</th>
                <th>Changed</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {shown.map((r) => (
                <FlowRow
                  key={r.name}
                  row={r}
                  at={list.row(r.name)}
                  dirty={tabOf(r.name)?.dirty === true}
                  uses={uses.get(r.name) ?? []}
                  navigate={navigate}
                  manifestOf={manifestOf}
                  onOpen={() => open(r)}
                  onReload={() => load(r.name, true)}
                  onDownload={() => download(r.name)}
                  onDelete={() => remove(r.name)}
                  publish={
                    marketplace.url
                      ? () => marketplace.onPublish(r.name).then((m) => m && setNote(m))
                      : null
                  }
                  publishTo={marketplace.url}
                />
              ))}
              {shown.length === 0 && (
                <tr>
                  <td colSpan={6} className="hl-table__dim">
                    no flow {q ? `matches "${search.trim()}"` : `is ${SHOWS.find(([s]) => s === show)![1].toLowerCase()}`}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </section>
      )}
    </div>
  );
}

function FlowRow({
  row,
  at,
  dirty,
  uses,
  navigate,
  manifestOf,
  onOpen,
  onReload,
  onDownload,
  onDelete,
  publish,
  publishTo,
}: {
  row: Row;
  /** What marks it selected from the keys (useListKeys). */
  at: ReturnType<ReturnType<typeof useListKeys>["row"]>;
  dirty: boolean;
  uses: HolonUse[];
  navigate: (r: Route) => void;
  manifestOf: (doc: FlowDocument) => { items: string[] };
  onOpen: () => void;
  onReload: () => void;
  onDownload: () => void;
  onDelete: () => void;
  publish: (() => void) | null;
  publishTo: string;
}) {
  const { name, saved, deployed } = row;
  const to = placeOf(row);
  const may = useMay(row, manifestOf);
  return (
    <tr {...at}>
      <td>
        <a href={format(to)} onClick={follow(to, navigate)}>
          {name}
        </a>
        <UsedBy uses={uses.filter((u) => u.kind === "flow")} lead="placed in" title="saved flows that place this one as a node; each keeps the version it placed until updated (↻ on the node)" />
        <UsedBy uses={uses.filter((u) => u.kind === "deployment")} lead="deployed in" title="deployments whose version places this one as a node; a redeploy takes the version its flow places" />
      </td>
      <td>
        <span className="hl-chips">
          {!deployed && <span className="hl-chip">draft</span>}
          {deployed && <span className={`hl-chip hl-chip--${deployed.status}`}>{STATE_SAID[deployed.status]}</span>}
          {(deployed?.waiting ?? 0) > 0 && <span className="hl-chip hl-chip--waits">waits {deployed!.waiting}</span>}
          {deployed?.attention && (
            <span className="hl-chip hl-chip--bad" title={deployed.attention}>
              needs a deploy
            </span>
          )}
          {dirty && <span className="hl-flow__waiting">unsaved changes</span>}
        </span>
      </td>
      <td title={may && "items" in may ? may.items.map(describeItem).join("\n") : may?.error}>{may === null ? "…" : "error" in may ? <span className="hl-table__dim">?</span> : may.items.length}</td>
      <td>
        {deployed ? (
          <>
            {deployed.trigger_count}
            {deployed.error_count > 0 && <span className="hl-flow__failed"> · {deployed.error_count} failed</span>}
          </>
        ) : (
          <span className="hl-table__dim">—</span>
        )}
      </td>
      <td className="hl-flows__changed">
        {saved && <div>saved {when(saved.saved_at_epoch_seconds)}{saved.saved_by ? ` by ${saved.saved_by}` : ""}</div>}
        {deployed && <div>deployed {when(deployed.deployed_at_epoch_seconds)}{deployed.deployed_by ? ` by ${deployed.deployed_by}` : ""}</div>}
      </td>
      <td className="hl-flows__do">
        <button className="hl-flow__button" onClick={onOpen} title={dirty ? "go to its tab, changes kept" : "open it in the editor"}>
          open
        </button>
        {saved && dirty && (
          <button className="hl-flow__button" onClick={onReload} title="discard your changes and load the saved copy">
            reload saved
          </button>
        )}
        {saved && (
          <>
            <button className="hl-flow__button" disabled={!publish} title={publish ? `publish it to ${publishTo}, as it is saved now` : "set a marketplace first (Library, Marketplace)"} onClick={() => publish?.()}>
              publish
            </button>
            <button className="hl-flow__button" title="download it as text (.holon.ts): edit it anywhere, keep it in git, check it with `holon check`" onClick={onDownload}>
              .holon.ts
            </button>
            <button className="hl-flow__button" onClick={onDelete} aria-label={`delete ${name}`} title="delete this saved flow">
              ×
            </button>
          </>
        )}
      </td>
    </tr>
  );
}

/** What a flow may do: its deployment's manifest when deployed, else what deploying its saved
 *  copy would ask for. Read again when either changes. */
function useMay(row: Row, manifestOf: (doc: FlowDocument) => { items: string[] }): { items: string[] } | { error: string } | null {
  const key = row.deployed ? `d:${row.deployed.unit}` : `s:${row.saved?.saved_at_epoch_seconds}`;
  const [may, setMay] = useState<{ key: string; value: { items: string[] } | { error: string } } | null>(null);
  useEffect(() => {
    let fresh = true;
    const read = row.deployed ? getManifest(row.name) : getFlow(row.name).then((rec) => manifestOf(rec.document));
    read.then(
      (m) => fresh && setMay({ key, value: { items: m.items } }),
      (e: unknown) => fresh && setMay({ key, value: { error: e instanceof Error ? e.message : String(e) } }),
    );
    return () => {
      fresh = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return may?.key === key ? may.value : null;
}

/** Who places a holon, of one kind (saved flows or deployments), and whether from an older save. */
function UsedBy({ uses, lead, title }: { uses: HolonUse[]; lead: string; title: string }) {
  if (uses.length === 0) return null;
  return (
    <div className="hl-flows__used" title={title}>
      {lead}{" "}
      {uses.map((u, i) => (
        <span key={u.user}>
          {i > 0 && ", "}
          <span className="hl-flows__user">{u.user}</span>
          {u.nodes > 1 && ` ×${u.nodes}`}
          {!u.current && <span className="hl-flow__waiting"> (an older save)</span>}
        </span>
      ))}
    </div>
  );
}
