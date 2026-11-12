// A deployed flow's own page (gaps-web.md G15), at `#<slug>[.tab]`: its header (name, status,
// open in the editor) and its tabs, each a place of its own in the address. The pages the
// editor had for a deployment (its card, view mode's log) move here a tab at a time. Its buttons
// follow `can` on the flow (useCan.tsx), asked once for every tab's.

import { useMemo, useState, type ReactNode } from "react";
import { HistoryTab } from "./HistoryPane";
import { LogsPane } from "./LogsPane";
import { StateTab } from "./FlowState";
import { StatusTab } from "./FlowStatus";
import { RunDetail } from "./RunPage";
import { SettingsTab } from "./FlowSettings";
import type { CodeText } from "./timeline";
import { outside, runLabel, runOutcome, type Outcome } from "./runs";
import { follow, format, type FlowTab, type Route } from "./routes";
import { downloadRuns, getLog, type DeploymentEntry, type FlowDocument, type LogEntry } from "./server";
import { gateOf, useCan, type Answer } from "./useCan";
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

const STATUS_MARK: Record<DeploymentEntry["status"], string> = { running: "●", paused: "❚❚", stopped: "■" };

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


export function FlowPage({
  slug,
  tab,
  run = null,
  entry,
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
  navigate: (r: Route) => void;
  /** What a version's canvas may do, for since when it may do each thing. */
  manifestOf: (doc: FlowDocument) => { items: string[] };
  /** Its versions printed as text, for a restore's diff. */
  codeText: CodeText;
  /** A restore from its history changed it: the table should be fetched again. */
  onRestored: () => void;
  /** Running, pausing and stopping it (the header's; Settings runs it from a session), its grant
   *  changed, the permissions asked for from the admin page, and removing it. */
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
  const can = useCan(FLOW_ACTIONS, { flow: slug });
  const lifecycle = (title: string) => can.gate("flow.lifecycle", { title });
  const [failed, setFailed] = useState("");
  const act = (action: "run" | "pause" | "stop") => {
    setFailed("");
    card.onAction(action).catch((e: unknown) => setFailed(e instanceof Error ? e.message : String(e)));
  };
  return (
    <div className="hl-flow" aria-label={`flow ${slug}`}>
      <header className="hl-flow__head">
        <h1 className="hl-flow__name">{slug}</h1>
        {entry && (
          <span className={`hl-flow__status hl-flow__status--${entry.status}`}>
            {STATUS_MARK[entry.status]} {entry.status}
            {entry.port !== null && <span className="hl-flow__port"> :{entry.port}</span>}
          </span>
        )}
        <span className="hl-flow__spacer" />
        {entry?.status === "running" && (
          <button className="hl-flow__button" onClick={() => act("pause")} {...lifecycle("stop answering, keep the port and the session")}>
            Pause
          </button>
        )}
        {entry?.status === "paused" && (
          <button className="hl-flow__button" onClick={() => act("run")} {...lifecycle("continue the same session")}>
            Resume
          </button>
        )}
        {entry && entry.status !== "stopped" && (
          <button className="hl-flow__button" onClick={() => act("stop")} {...lifecycle("end the session, save its state, release the port")}>
            Stop
          </button>
        )}
        {entry?.status === "stopped" && (
          <button className="hl-flow__button" onClick={() => act("run")} {...lifecycle("a new session, from the state the last one left")}>
            Run
          </button>
        )}
        <button className="hl-flow__button hl-flow__button--go" onClick={edit}>
          Open in editor
        </button>
      </header>
      {failed && <div className="hl-flow__bad">{failed}</div>}
      {!entry ? (
        <p className="hl-flow__none">{slug} isn't deployed: open it in the editor and deploy it, and its runs, history and state are here.</p>
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
            <StatusTab slug={slug} entry={entry} navigate={navigate} mayCall={can.may("run.call")} manifestOf={manifestOf} />
          ) : (
            <SettingsTab
              slug={slug}
              entry={entry}
              can={can}
              focus={card.focus}
              onChanged={card.onChanged}
              onRunFrom={(session) => card.onAction("run", session)}
              onRemove={card.onRemove}
            />
          )}
        </>
      )}
    </div>
  );
}

type Show = "all" | "failed" | "waiting";
const OUTCOME_SAID: Record<Outcome, string> = { ok: "", failed: "failed", waiting: "waiting" };

/** Its runs, newest first (the log keeps the last 500 here): what each answered, how it ended,
 *  how long it took and what it did outside the flow; each opens its record. Above them, the
 *  chain the log is (each run names the hash of the one before it), checked on asking, and the
 *  runs exported for `holon verify`. */
function RunsTab({ slug, navigate, mayExport = true }: { slug: string; navigate: (r: Route) => void; mayExport?: Answer }) {
  const { entries, tallies, error } = useFlowLog(slug);
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
  const check = () => {
    setSaid("checking…");
    getLog(slug)
      .then(({ entries: all, head, breaks }) =>
        setSaid(
          breaks.length > 0 ? (
            <span className="hl-flow__bad">✗ the chain breaks at run {breaks.join(", ")}: the log was changed outside holon</span>
          ) : (
            <span>
              ✓ the chain is unbroken: {all.length} run{all.length === 1 ? "" : "s"}, each naming the one before it · head <code title={head}>{head.slice(0, 12)}</code>
            </span>
          ),
        ),
      )
      .catch((e: unknown) => setSaid(<span className="hl-flow__bad">{e instanceof Error ? e.message : String(e)}</span>));
  };
  const exportRuns = () =>
    downloadRuns(slug).then(
      () => setSaid(`saved ${slug}-runs.json · check it anywhere: holon verify ${slug}-runs.json`),
      (e: unknown) => setSaid(<span className="hl-flow__bad">{e instanceof Error ? e.message : String(e)}</span>),
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
        <button className="hl-flow__button" onClick={check} title="read the whole log and check each run names the hash of the one before it">
          Check the chain
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
        <table className="hl-table">
          <thead>
            <tr>
              <th>Run</th>
              <th>When</th>
              <th>What</th>
              <th>Answer</th>
              <th>Took</th>
              <th title="what it did outside the flow: ↗ requests it sent, ✎ effects it performed">Did outside</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((e) => {
              const o = runOutcome(e);
              const { sent, did } = outside(e);
              const to: Route = { kind: "run", slug, seq: e.seq };
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
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}
