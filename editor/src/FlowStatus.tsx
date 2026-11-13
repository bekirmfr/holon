// A flow's Status tab (FlowPage.tsx): its last day (its runs hour by hour from the host's summary,
// its timings from the runs its log holds here), where it runs, what it may do (since which
// version, approved by whom; changed in Settings), its latest runs, its versions (one that waits
// for a person first, with its review), and its actions to run.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import { ActionRows } from "./ActionRows";
import type { Approval } from "./approvals";
import { lastDay, sinceOf, type VersionItems } from "./statusFacts";
import { describeItem } from "./holon";
import { follow, format, type Route } from "./routes";
import { runLabel, runOutcome } from "./runs";
import { getDeployedDocument, getManifest, getSummary, getVersions, hostAddress, type DeploymentEntry, type FlowDocument, type VersionEntry } from "./server";
import type { Answer } from "./useCan";
import { useFlowLog } from "./useFlowLog";
import { Spark } from "./ui/Spark";

/** How many versions back "since" looks. */
const VERSIONS_READ = 20;

const ms = (n: number | null) => (n === null ? "—" : n < 1 ? "<1 ms" : `${Math.round(n)} ms`);
const when = (secs: number) => new Date(secs * 1000).toLocaleString();


export function StatusTab({
  slug,
  entry,
  navigate,
  mayCall = true,
  manifestOf,
  waiting = [],
}: {
  slug: string;
  entry: DeploymentEntry;
  navigate: (r: Route) => void;
  /** Whether whoever is signed in may run its actions (`run.call`), or why not. */
  mayCall?: Answer;
  /** What a version's canvas may do, derived as deploying it would. */
  manifestOf: (doc: FlowDocument) => { items: string[] };
  /** What of it waits for a person (approvals.ts): a version saved since, or proposed. */
  waiting?: Approval[];
}) {
  const { entries } = useFlowLog(slug);
  const day = useMemo(() => lastDay(entries, Date.now()), [entries]);
  // Its hours, counted by the server from every run its log keeps (GET /summary): read again as
  // its runs come in.
  const [hours, setHours] = useState<{ runs: number[]; failed: number[] } | null>(null);
  useEffect(() => {
    let stale = false;
    getSummary(24).then(
      (s) => !stale && setHours(s.deployments.find((d) => d.slug === slug) ?? null),
      () => undefined,
    );
    return () => {
      stale = true;
    };
  }, [slug, entries.length]);
  const runs = hours ? hours.runs.reduce((a, b) => a + b, 0) : day.runs;
  const failed = hours ? hours.failed.reduce((a, b) => a + b, 0) : day.failed;
  const [may, setMay] = useState<{ items: string[]; notGranted: string[] } | null>(null);
  const [versions, setVersions] = useState<VersionEntry[]>([]);
  const [since, setSince] = useState<ReturnType<typeof sinceOf>>(new Map());
  const [error, setError] = useState("");
  // What it may do now, and the versions before it: read when it changes (a deploy, a restore).
  useEffect(() => {
    let stale = false;
    (async () => {
      const [m, vs] = await Promise.all([getManifest(slug), getVersions(slug)]);
      if (stale) return;
      setMay({ items: m.items, notGranted: m.not_granted });
      setVersions(vs);
      // Each version's items: the server's for the one deployed now, each earlier one's from its
      // canvas (where it is kept).
      const read: VersionItems[] = await Promise.all(
        vs.slice(0, VERSIONS_READ).map(async (v, i) => ({
          number: v.number,
          by: v.by ?? null,
          items: i === 0 ? m.items : v.canvas ? await getDeployedDocument(slug, v.unit).then((d) => manifestOf(d).items, () => null) : null,
        })),
      );
      if (!stale) setSince(sinceOf(m.items, read));
    })().catch((e: unknown) => !stale && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      stale = true;
    };
  }, [slug, entry.unit, entry.attention, manifestOf]);

  const latest = entries.slice(-5).reverse();
  const actions = entry.triggers.filter((t) => t.startsWith("action ")).map((t) => t.slice("action ".length));
  const current = versions.find((v) => v.current);
  const host = hostAddress().replace(/:\d+$/, "");
  return (
    <div className="hl-flow__status-tab">
      {entry.attention && <div className="hl-flow__banner hl-flow__bad">⚠ {entry.attention}</div>}
      {entry.listen_error && <div className="hl-flow__banner hl-flow__bad">⚠ not answering: {entry.listen_error}</div>}
      {error && <div className="hl-flow__bad">{error}</div>}

      <section className="hl-flow__section" aria-label="last 24 hours">
        <h2>Last 24 h</h2>
        <div className="hl-tiles">
          <Tile label="runs" value={runs.toLocaleString()} />
          <Tile label="failed" value={failed.toLocaleString()} bad={failed > 0} />
          <Tile label="waiting now" value={String(entry.waiting ?? 0)} />
          <Tile label="median" value={ms(day.p50)} />
          <Tile label="95th percentile" value={ms(day.p95)} />
        </div>
        {hours && <Spark runs={hours.runs} failed={hours.failed} width={560} height={48} />}
        {entries.length >= 500 && <p className="hl-flow__note">timings of the last 500 runs its log holds here</p>}
      </section>

      <div className="hl-flow__columns">
        <section className="hl-flow__section" aria-label="where it runs">
          <h2>Where it runs</h2>
          <dl className="hl-flow__facts">
            {entry.port !== null && (
              <Fact name="address">
                <code>
                  http://{host}:{entry.port}
                </code>
              </Fact>
            )}
            <Fact name="starts on">{entry.triggers.join(", ") || "nothing yet"}</Fact>
            <Fact name="session">
              {entry.session} of {entry.sessions}
            </Fact>
            <Fact name="version">
              {current ? `v${current.number}` : "…"} · deployed {when(entry.deployed_at_epoch_seconds)}
              {entry.deployed_by ? ` by ${entry.deployed_by}` : ""}
            </Fact>
            <Fact name="unit">
              <code title={entry.unit}>{entry.unit.slice(0, 12)}</code>
            </Fact>
          </dl>
        </section>

        <section className="hl-flow__section" aria-label="it may">
          <h2>
            It may{" "}
            <a className="hl-flow__more" href={format({ kind: "flow", slug, tab: "settings" })} onClick={follow({ kind: "flow", slug, tab: "settings" }, navigate)} title="what it may do, its limits and its network, in its Settings">
              Change…
            </a>
          </h2>
          {may === null ? (
            <p className="hl-flow__note">…</p>
          ) : may.items.length === 0 ? (
            <p className="hl-flow__note">nothing outside the flow</p>
          ) : (
            <ul className="hl-flow__may">
              {may.items.map((item) => {
                const s = since.get(item);
                const lacking = may.notGranted.includes(item);
                return (
                  <li key={item} title={item}>
                    <span className={lacking ? "hl-flow__bad" : "hl-flow__what"}>{describeItem(item)}</span>
                    {lacking && <span className="hl-flow__bad"> · not granted</span>}
                    {s && (
                      <span className="hl-table__dim">
                        {" "}
                        · since v{s.since}
                        {s.known ? "" : " or before"}
                        {s.by ? ` · approved by ${s.by}` : ""}
                      </span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </section>
      </div>

      <section className="hl-flow__section" aria-label="latest runs">
        <h2>
          Latest runs{" "}
          <a className="hl-flow__more" href={format({ kind: "flow", slug, tab: "runs" })} onClick={follow({ kind: "flow", slug, tab: "runs" }, navigate)}>
            all runs →
          </a>
        </h2>
        {latest.length === 0 ? (
          <p className="hl-flow__note">no runs yet</p>
        ) : (
          <table className="hl-table">
            <tbody>
              {latest.map((e) => {
                const to: Route = { kind: "run", slug, seq: e.seq };
                const o = runOutcome(e);
                return (
                  <tr key={e.seq} className="hl-table__row" onClick={() => navigate(to)}>
                    <td>
                      <a href={format(to)} onClick={(ev) => ev.stopPropagation()}>
                        #{e.seq}
                      </a>
                    </td>
                    <td className="hl-flow__what">{runLabel(e)}</td>
                    <td className={o === "ok" ? "" : `hl-flow__${o}`}>
                      {e.status ?? ""} {o === "ok" ? "" : o}
                    </td>
                    <td className="hl-table__dim">{new Date(e.at_ms).toLocaleTimeString()}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </section>

      <section className="hl-flow__section" aria-label="versions">
        <h2>Versions</h2>
        <ul className="hl-flow__versions">
          {waiting.map((a) =>
            (a.kind === "update" || a.kind === "first") && a.version !== null ? (
              <li key="waits" className="hl-flow__waits">
                <span className="hl-flow__what">v{a.version}</span>
                <span className="hl-flow__waiting"> · waits for a person</span>
                <span className="hl-table__dim">
                  {" "}
                  · {a.proposal ? `proposed by ${a.by ?? "someone"}` : `saved by ${a.by ?? "someone"}`} · {when(a.at)}
                  {a.proposal?.why ? ` · ${a.proposal.why}` : ""}
                </span>{" "}
                <a className="hl-flow__more" href={format({ kind: "review", slug, version: a.version })} onClick={follow({ kind: "review", slug, version: a.version }, navigate)}>
                  Review →
                </a>
              </li>
            ) : null,
          )}
          {versions.map((v) => (
            <li key={v.number}>
              <span className="hl-flow__what">v{v.number}</span>
              <span className="hl-table__dim">
                {" "}
                · {when(v.deployed_at)}
                {v.by ? ` · by ${v.by}` : ""}
                {v.restored_from ? ` · restores v${v.restored_from}'s code` : ""}
              </span>
              {v.current && <span className="hl-flow__now"> · deployed now</span>}
            </li>
          ))}
        </ul>
      </section>

      {actions.length > 0 && (
        <section className="hl-flow__section" aria-label="actions">
          <h2>Actions</h2>
          <div className="hl-flow__actions">
            <ActionRows names={actions} slug={entry.status === "running" ? slug : ""} may={mayCall} onViewRun={(seq) => navigate({ kind: "run", slug, seq })} />
          </div>
        </section>
      )}
    </div>
  );
}

function Tile({ label, value, bad = false }: { label: string; value: string; bad?: boolean }) {
  return (
    <div className="hl-tile">
      <div className={bad ? "hl-tile__value hl-flow__bad" : "hl-tile__value"}>{value}</div>
      <div className="hl-tile__label">{label}</div>
    </div>
  );
}

function Fact({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div>
      <dt>{name}</dt>
      <dd>{children}</dd>
    </div>
  );
}
