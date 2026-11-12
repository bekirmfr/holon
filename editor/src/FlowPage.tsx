// A flow's own page (gaps-web.md G15, web-console-L4.md §8), at `#<slug>[.tab]`: its header (its
// state, version, what starts it and its description; open in the editor, review a version that
// waits, pause or resume, run now) and its tabs, each a place of its own in the address. A flow
// saved and never deployed shows what it would be allowed to, and Deploy. Its buttons follow
// `can` on the flow (useCan.tsx), asked once for every tab's.

import { useEffect, useMemo, useState, type ReactNode } from "react";
import type { Approval } from "./approvals";
import { findRoutes } from "./derive";
import { useDialog } from "./Dialog";
import { StateChips } from "./FlowCells";
import { HistoryTab } from "./HistoryPane";
import { describeItem } from "./holon";
import { LogsPane } from "./LogsPane";
import { StateTab } from "./FlowState";
import { StatusTab } from "./FlowStatus";
import { RunDetail } from "./RunPage";
import { SettingsTab } from "./FlowSettings";
import { firstComment, runNowOptions, startsSaid, wentOut } from "./flowFacts";
import type { CodeText } from "./timeline";
import { outside, runLabel, runOutcome, type Outcome } from "./runs";
import { follow, format, type FlowTab, type Route } from "./routes";
import { downloadRuns, getDeployedDocument, getFlow, getLog, runAction, tickNow, type DeploymentEntry, type FlowDocument, type LogEntry } from "./server";
import { gateOf, useCan, type Answer, type Can } from "./useCan";
import { tallyRows } from "./tallies";
import { useFlowLog } from "./useFlowLog";
import { useKeys, useListKeys } from "./ui/Keys";

/** The tabs a flow's page has, in order, and what each is called. */
const TABS: [FlowTab, string][] = [
  ["status", "Status"],
  ["runs", "Runs"],
  ["logs", "Logs"],
  ["history", "History"],
  ["state", "State"],
  ["settings", "Settings"],
];

/** What a flow's page asks `can` about, on the flow: each its buttons' routes need (can.rs). The
 *  server's secrets' names (resource.view) are the host's, asked with them. */
export const FLOW_ACTIONS = [
  "flow.lifecycle",
  "flow.delete",
  "flow.restore",
  "flow.approve-deploy",
  "flow.deploy",
  "run.call",
  "run.export",
  "secret.set",
  "resource.view",
] as const;

const say = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The version that waits for a person to review it, and where its review is. */
function waitingVersion(slug: string, waiting: Approval[]): { version: number; first: boolean; to: Route } | null {
  for (const a of waiting) {
    if ((a.kind === "first" || a.kind === "update") && a.version !== null) return { version: a.version, first: a.kind === "first", to: { kind: "review", slug, version: a.version } };
  }
  return null;
}

export function FlowPage({
  slug,
  tab,
  run = null,
  entry,
  waiting = [],
  navigate,
  card,
  codeText,
  onRestored,
  manifestOf,
}: {
  slug: string;
  tab: FlowTab;
  /** One of its runs, shown under Runs (`#<slug>.run-<n>`). */
  run?: number | null;
  /** Its row in the server's table; undefined while it isn't deployed. */
  entry: DeploymentEntry | undefined;
  /** What of it waits for a person (approvals.ts): a version to review, its first or its next. */
  waiting?: Approval[];
  navigate: (r: Route) => void;
  /** What a version's canvas may do, for since when it may do each thing. */
  manifestOf: (doc: FlowDocument) => { items: string[] };
  /** Its versions printed as text, for a restore's diff. */
  codeText: CodeText;
  /** A restore from its history changed it: the table should be fetched again. */
  onRestored: () => void;
  /** Running, pausing and stopping it (the header pauses and resumes; Settings stops it, and runs
   *  it from a session), its grant changed, the permissions asked for from the admin page, and
   *  removing it. */
  card: {
    onAction: (action: "run" | "pause" | "stop", from?: "latest" | "fresh" | number) => Promise<void>;
    onChanged: () => void;
    focus?: number;
    onRemove: () => Promise<void>;
  };
}) {
  const edit = () => navigate({ kind: "editor", slug, view: false, seq: null });
  // [ and ]: the tab before or after, wrapping.
  const tabBeside = (by: 1 | -1) => {
    const at = TABS.findIndex(([t]) => t === tab);
    navigate({ kind: "flow", slug, tab: TABS[(at + by + TABS.length) % TABS.length][0] });
  };
  useKeys("tabs", { "[": () => tabBeside(-1), "]": () => tabBeside(1) }, { enabled: entry !== undefined });
  useKeys("flow", { e: edit });
  const can = useCan(FLOW_ACTIONS, { flow: slug });
  const [failed, setFailed] = useState("");
  const [said, setSaid] = useState("");
  const act = (action: "run" | "pause" | "stop") => {
    setFailed("");
    card.onAction(action).catch((e: unknown) => setFailed(say(e)));
  };
  const review = waitingVersion(slug, waiting);
  const doc = useDocument(slug, entry?.unit);
  const routes = useMemo(() => (doc ? findRoutes(doc.nodes, doc.edges).routes.length : null), [doc]);
  const about = firstComment(doc?.source);
  const runNow = useRunNow(slug, entry, can, setSaid, setFailed);
  return (
    <div className="hl-flow" aria-label={`flow ${slug}`}>
      <header className="hl-flow__head">
        <span className={`hl-flow__dot hl-flow__dot--${entry?.status ?? "draft"}`} aria-hidden="true" />
        <h1 className="hl-flow__name">{slug}</h1>
        <span className="hl-flow__version">{entry ? (entry.version ? `v${entry.version}` : "") : "draft"}</span>
        {entry && <StateChips deployed={entry} waits={review && !review.first ? review.version : null} />}
        {entry && <span className="hl-flow__starts">{startsSaid(entry.triggers, entry.port, routes)}</span>}
        <span className="hl-flow__spacer" />
        {entry?.status === "running" && (
          <button className="hl-flow__button" onClick={() => act("pause")} {...can.gate("flow.lifecycle", { title: "stop answering, keep the port and the session" })}>
            Pause
          </button>
        )}
        {entry?.status === "paused" && (
          <button className="hl-flow__button" onClick={() => act("run")} {...can.gate("flow.lifecycle", { title: "continue the same session" })}>
            Resume
          </button>
        )}
        {entry?.status === "stopped" && (
          <button className="hl-flow__button" onClick={() => act("run")} {...can.gate("flow.lifecycle", { title: "a new session, from the state the last one left" })}>
            Run
          </button>
        )}
        {runNow}
        {review && entry && (
          <button className="hl-flow__button hl-flow__button--waits" onClick={() => navigate(review.to)} title={`version ${review.version} waits for a person: review it, then deploy it`}>
            Review v{review.version}
          </button>
        )}
        <button className="hl-flow__button hl-flow__button--go" onClick={edit} title="open it in the editor (e)">
          Open in editor
        </button>
        {about && <p className="hl-flow__about">{about}</p>}
      </header>
      {failed && <div className="hl-flow__bad">{failed}</div>}
      {said && (
        <div className="hl-flow__said" role="status">
          {said}
        </div>
      )}
      {!entry ? (
        <Draft slug={slug} manifestOf={manifestOf} deployTo={review?.to ?? null} onEdit={edit} navigate={navigate} mayDeploy={can.may("flow.deploy")} />
      ) : (
        <>
          <nav className="hl-flow__tabs" aria-label="flow tabs">
            {TABS.map(([t, label]) => {
              const to: Route = { kind: "flow", slug, tab: t };
              return (
                <a key={t} href={format(to)} aria-current={t === tab ? "page" : undefined} onClick={follow(to, navigate)}>
                  {label}
                </a>
              );
            })}
          </nav>
          {run !== null ? (
            <RunDetail slug={slug} seq={run} navigate={navigate} />
          ) : tab === "runs" ? (
            <RunsTab slug={slug} navigate={navigate} mayExport={can.may("run.export")} />
          ) : tab === "logs" ? (
            <LogsPane slug={slug} navigate={navigate} />
          ) : tab === "state" ? (
            <StateTab slug={slug} version={entry.trigger_count} onOpenRun={(seq) => navigate({ kind: "run", slug, seq })} />
          ) : tab === "history" ? (
            <HistoryTab slug={slug} codeText={codeText} onRestored={onRestored} onOpenRun={(seq) => navigate({ kind: "run", slug, seq })} mayRestore={can.may("flow.restore")} />
          ) : tab === "status" ? (
            <StatusTab slug={slug} entry={entry} navigate={navigate} mayCall={can.may("run.call")} manifestOf={manifestOf} waiting={waiting} />
          ) : (
            <SettingsTab slug={slug} entry={entry} can={can} focus={card.focus} onChanged={card.onChanged} onAction={card.onAction} onEdit={edit} onRemove={card.onRemove} />
          )}
        </>
      )}
    </div>
  );
}

/** The canvas of the version deployed now (its text, its routes), read when it changes. */
function useDocument(slug: string, unit: string | undefined): FlowDocument | null {
  const [doc, setDoc] = useState<FlowDocument | null>(null);
  useEffect(() => {
    let live = true;
    setDoc(null);
    if (unit) getDeployedDocument(slug, unit).then((d) => live && setDoc(d), () => undefined);
    return () => {
      live = false;
    };
  }, [slug, unit]);
  return doc;
}

/** Run now, for a flow with a clock or an action: a clock ticks now (`flow.lifecycle`), an action
 *  runs with nothing sent (`run.call`); with several, a question asks which. Only while it runs. */
function useRunNow(slug: string, entry: DeploymentEntry | undefined, can: Can, onSaid: (s: string) => void, onFailed: (s: string) => void): ReactNode {
  const dialog = useDialog();
  const options = entry ? runNowOptions(entry.triggers) : [];
  if (!entry || options.length === 0) return null;
  const running = entry.status === "running";
  const action = options.every((o) => o.kind === "clock") ? "flow.lifecycle" : "run.call";
  const go = async () => {
    onFailed("");
    onSaid("");
    const at = options.length === 1 ? 0 : await dialog.choose("Run which now?", options.map((o) => (o.kind === "clock" ? `Tick ${o.name}` : `Run ${o.name}`)));
    if (at === null) return;
    const o = options[at];
    try {
      if (o.kind === "clock") {
        const t = await tickNow(slug, o.name);
        onSaid(`${o.name}: ticked, ${t.performed.length} effect${t.performed.length === 1 ? "" : "s"} performed`);
      } else {
        const r = await runAction(slug, o.name, "");
        onSaid(`${o.name}: run #${r.seq}${r.waiting ? ", waiting" : ""}`);
      }
    } catch (e) {
      onFailed(say(e));
    }
  };
  return (
    <button className="hl-flow__button" onClick={() => void go()} {...can.gate(action, { disabled: !running, title: running ? "run it now: a clock ticks, an action runs with nothing sent" : "it isn't running" })}>
      Run now
    </button>
  );
}

/** A flow saved and never deployed: what it would be allowed to, in the rows its manifest makes,
 *  and Deploy, which reviews it first. */
function Draft({ slug, manifestOf, deployTo, onEdit, navigate, mayDeploy }: { slug: string; manifestOf: (doc: FlowDocument) => { items: string[] }; deployTo: Route | null; onEdit: () => void; navigate: (r: Route) => void; mayDeploy: Answer }) {
  const [may, setMay] = useState<{ items: string[] } | { error: string } | null>(null);
  useEffect(() => {
    let live = true;
    getFlow(slug)
      .then((rec) => live && setMay(manifestOf(rec.document)))
      .catch((e: unknown) => live && setMay({ error: say(e) }));
    return () => {
      live = false;
    };
  }, [slug, manifestOf]);
  if (may !== null && "error" in may) return <p className="hl-flow__none">{slug} isn't deployed, and isn't saved here: {may.error}</p>;
  return (
    <section className="hl-flow__section" aria-label="it would be allowed to">
      <h2>It would be allowed to</h2>
      {may === null ? (
        <p className="hl-flow__note">…</p>
      ) : may.items.length === 0 ? (
        <p className="hl-flow__note">nothing outside the flow</p>
      ) : (
        <ul className="hl-flow__may">
          {may.items.map((item) => (
            <li key={item} title={item} className="hl-flow__what">
              {describeItem(item)}
            </li>
          ))}
        </ul>
      )}
      <p className="hl-flow__note">It isn't deployed. A deploy shows what it asks for, for a person to approve; its runs, history and state are here once it is.</p>
      <div className="hl-flow__bar">
        <button className="hl-flow__button hl-flow__button--go" onClick={() => (deployTo ? navigate(deployTo) : onEdit())} {...gateOf(mayDeploy, { title: deployTo ? "review it, then deploy it" : "open it in the editor, and deploy it from there" })}>
          Deploy…
        </button>
      </div>
    </section>
  );
}

type Show = "all" | "failed" | "waiting";
const OUTCOME_SAID: Record<Outcome, string> = { ok: "", failed: "failed", waiting: "waiting" };

/** Its runs, newest first (the log keeps the last 500 here): what came in, how it ended, how long
 *  it took, what it did outside the flow and what it sent out; each opens its record. Above them,
 *  the chain the log is (each run names the hash of the one before it), checked on asking in a
 *  dialog, and the runs exported for `holon verify`. */
function RunsTab({ slug, navigate, mayExport = true }: { slug: string; navigate: (r: Route) => void; mayExport?: Answer }) {
  const { entries, tallies, error } = useFlowLog(slug);
  const dialog = useDialog();
  const [show, setShow] = useState<Show>("all");
  const [said, setSaid] = useState<ReactNode>("");
  const counts = useMemo(() => {
    const c: Record<Show, number> = { all: entries.length, failed: 0, waiting: 0 };
    for (const e of entries) {
      const o = runOutcome(e);
      if (o !== "ok") c[o] += 1;
    }
    return c;
  }, [entries]);
  const shown = useMemo(() => entries.filter((e) => show === "all" || runOutcome(e) === show).slice().reverse(), [entries, show]);
  const check = async () => {
    let body: ReactNode;
    try {
      const { entries: all, head, breaks } = await getLog(slug);
      body =
        breaks.length > 0 ? (
          <p className="hl-flow__bad">✗ The chain breaks at run {breaks.join(", ")}: the log was changed outside holon.</p>
        ) : (
          <div className="hl-run__offline">
            <p>
              ✓ The chain is unbroken: {all.length} run{all.length === 1 ? "" : "s"}, each naming the hash of the one before it.
            </p>
            <p>
              Its head, what a witness holds: <code title={head}>{head.slice(0, 16)}…</code>
            </p>
            <p>Anywhere, without this server, after Export:</p>
            <pre>
              holon verify {slug}-runs.json --head {head}
            </pre>
          </div>
        );
    } catch (e) {
      body = <p className="hl-flow__bad">{say(e)}</p>;
    }
    await dialog.confirm(`The chain of ${slug}'s runs`, "Close", body);
  };
  const exportRuns = () =>
    downloadRuns(slug).then(
      () => setSaid(`saved ${slug}-runs.json · check it anywhere: holon verify ${slug}-runs.json`),
      (e: unknown) => setSaid(<span className="hl-flow__bad">{say(e)}</span>),
    );
  const open = (e: LogEntry) => navigate({ kind: "run", slug, seq: e.seq });
  const list = useListKeys(
    `runs:${slug}`,
    shown.map((e) => String(e.seq)),
    (seq) => navigate({ kind: "run", slug, seq: Number(seq) }),
  );
  return (
    <section className="hl-flow__runs" aria-label="runs">
      <div className="hl-flow__bar">
        <div className="hl-seg" role="group" aria-label="show">
          {(["all", "failed", "waiting"] as Show[]).map((s) => (
            <button key={s} className="hl-seg__opt" aria-pressed={show === s} onClick={() => setShow(s)}>
              {s === "all" ? "All" : s === "failed" ? "Failed" : "Waiting"} <span className="hl-flow__count">{counts[s]}</span>
            </button>
          ))}
        </div>
        <span className="hl-flow__spacer" />
        <button className="hl-flow__button" onClick={() => void check()} title="read the whole log and check each run names the hash of the one before it">
          Check the chain…
        </button>
        <button className="hl-flow__button" onClick={exportRuns} {...gateOf(mayExport, { title: "save the runs and the units they ran, for holon verify" })}>
          Export
        </button>
      </div>
      {said && (
        <div className="hl-flow__said" role="status">
          {said}
        </div>
      )}
      {error && <div className="hl-flow__bad">{error}</div>}
      {tallyRows(tallies).map((t) => (
        <div key={t.label} className="hl-flow__tally" title="its line says unrecorded: its runs are counted, not logged">
          <span className="hl-flow__what">{t.label}</span> not recorded · {t.line}
        </div>
      ))}
      {shown.length === 0 ? (
        <p className="hl-flow__none">{entries.length === 0 ? "no runs yet: send it a request" : `no ${show} runs`}</p>
      ) : (
        <table className="hl-table hl-flow__runlist">
          <thead>
            <tr>
              <th>Run</th>
              <th>When</th>
              <th>What came in</th>
              <th>Status</th>
              <th>Took</th>
              <th title="what it did outside the flow: ↗ requests it sent, ✎ effects it performed">Did outside</th>
              <th title="what it answered, or what it produced">What went out</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((e) => {
              const o = runOutcome(e);
              const { sent, did } = outside(e);
              const to: Route = { kind: "run", slug, seq: e.seq };
              const out = wentOut(e);
              return (
                <tr key={e.seq} {...list.row(String(e.seq))} className={`hl-table__row hl-table__row--${o}`} onClick={() => open(e)}>
                  <td>
                    <a href={format(to)} onClick={(ev) => ev.stopPropagation()}>
                      #{e.seq}
                    </a>
                  </td>
                  <td className="hl-table__dim">{new Date(e.at_ms).toLocaleTimeString()}</td>
                  <td className="hl-flow__what">{runLabel(e)}</td>
                  <td className={o === "ok" ? "" : `hl-flow__${o}`} title={e.error ?? undefined}>
                    {e.status ?? ""} {OUTCOME_SAID[o]}
                  </td>
                  <td className="hl-table__dim">{e.duration_ms < 1 ? "<1 ms" : `${Math.round(e.duration_ms)} ms`}</td>
                  <td>
                    {sent > 0 && <span title={`${sent} request${sent === 1 ? "" : "s"} sent`}>↗{sent} </span>}
                    {did > 0 && <span title={`${did} effect${did === 1 ? "" : "s"} performed`}>✎{did}</span>}
                  </td>
                  <td className="hl-flow__out" title={out || undefined}>
                    {out || <span className="hl-table__dim">–</span>}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}
