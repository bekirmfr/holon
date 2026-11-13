// The Overview (`#overview`, gaps-web.md G13, C12; web-console-L4.md §7.1), where the console
// opens: what needs a person (approvals.ts: each version waiting, who proposed it and what new it
// asks for; then the runs waiting), four tiles (the flows live, runs in the last 24 hours, the
// share that failed, what waits for a person), every deployed flow in the Flows page's columns
// (FlowCells.tsx), the newest runs across every flow, the run log with Check now, and what was
// done on the host. All of it from the host's summary (GET /summary, holon-server's summary.rs),
// read again every 10 s while shown; Check now reads every log's lines again (POST
// /summary/check). Notes and the first steps are under Getting started (WelcomePage.tsx).

import { useEffect, useState, type ReactNode } from "react";
import { routeOf, said, type Approval, type Reviewable } from "./approvals";
import { isApproval } from "./decisions";
import { describeItem } from "./holon";
import { itemParts } from "./holdings";
import { checkChains, getWarnings, type DeploymentEntry, type HostWarning } from "./server";
import { chainSaid, failedShare, greeting, hostLine, liveOf, tookSaid } from "./overview";
import { MayCount, StateChips, startsOn, SummaryCells, time, useSummary, versionWaiting } from "./FlowCells";
import { follow, format, type Route } from "./routes";
import { ActivityList } from "./Pages";
import { useToast, type Notify } from "./ui/Toast";
import { useCan } from "./useCan";
import type { WaitingRunOf } from "./useApprovals";

const short = (hash: string) => (hash ? `${hash.slice(0, 4)}…${hash.slice(-4)}` : "–");

/** Check every run log's lines again (POST /summary/check) and say what it found: each line naming
 *  the one before, or where the chain breaks, with a link to the first run it breaks at. The
 *  Overview's Check now, and the palette's Check the run log. */
export async function checkRunLog(notify: Notify): Promise<void> {
  try {
    const c = await checkChains();
    const first = c.breaks[0];
    if (!first) notify(`Checked ${c.runs.toLocaleString()} runs: every line names the one before it.`, "ok");
    else
      notify(`Checked ${c.runs.toLocaleString()} runs: the chain breaks at ${c.breaks.map((b) => `${b.slug} run ${b.seq}`).join(", ")}.`, "bad", {
        label: "Open the run",
        to: { kind: "run", slug: first.slug, seq: first.seq },
      });
  } catch (e) {
    notify(e instanceof Error ? e.message : String(e), "bad");
  }
}

export function OverviewPage({
  name,
  admin,
  deployments,
  approvals,
  runs,
  navigate,
  onNew,
}: {
  /** Who is signed in. */
  name: string;
  /** An admin sees what the host warns of (storage.rs), first. */
  admin: boolean;
  deployments: DeploymentEntry[];
  /** What waits for a person (useApprovals). */
  approvals: Approval[];
  /** The runs waiting (useApprovals), null until read: those at an approval wait for a person. */
  runs: WaitingRunOf[] | null;
  navigate: (r: Route) => void;
  onNew: () => void;
}) {
  const notify = useToast();
  const { summary, failed, read } = useSummary(deployments.length);
  const [warnings, setWarnings] = useState<HostWarning[]>([]);
  const [checking, setChecking] = useState(false);
  // Checking the run logs again (`POST /summary/check`) is host.check's.
  const can = useCan(["host.check"]);

  useEffect(() => {
    if (!admin) return;
    getWarnings()
      .then(setWarnings)
      .catch(() => setWarnings([]));
  }, [admin, deployments]);

  const check = async () => {
    setChecking(true);
    await checkRunLog(notify);
    read();
    setChecking(false);
  };

  const waiting = deployments.reduce((n, d) => n + (d.waiting ?? 0), 0);
  // What waits for a person: each change, and each run waiting at an approval.
  const atApproval = (runs ?? []).filter(({ run }) => run.waiting.some(isApproval)).length;
  const forPerson = approvals.length + atApproval;
  const chain = summary && chainSaid(summary.chain, time);
  const toWaiting: Route = { kind: "approvals", tab: "runs" };
  return (
    <div className="hl-flow hl-overview" aria-label="Overview">
      {warnings.length > 0 && (
        <div role="alert" aria-label="what the host warns of" className="hl-flow__banner">
          {warnings.map((w) => (
            <div key={w.key}>
              <span className="hl-flow__bad">⚠ {w.says}</span>
              <div className="hl-flow__note">{w.todo}</div>
            </div>
          ))}
        </div>
      )}
      <header className="hl-overview__head">
        <div>
          <h1 className="hl-flow__name">
            {greeting(new Date().getHours())}, {name}
          </h1>
          <p className="hl-flow__status">{hostLine(deployments.length, approvals.length, waiting)}</p>
        </div>
        <button className="hl-flow__button hl-flow__button--go" onClick={onNew}>
          New flow
        </button>
      </header>

      {(approvals.length > 0 || waiting > 0) && (
        <section className="hl-needs" aria-label="Needs a person">
          <b>Needs a person</b>
          {approvals.map((a, i) => {
            const to = routeOf(a);
            const review = a.kind === "first" || a.kind === "update";
            return (
              <div key={`${a.kind}:${a.slug}:${i}`} className="hl-needs__row">
                <span className="hl-flow__what">{a.slug}</span>
                {review ? <Change a={a} /> : <span className="hl-needs__said">{said(a, (s) => time(s * 1000))}</span>}
                {to && (
                  <a className="hl-flow__button" href={format(to)} onClick={follow(to, navigate)} aria-label={review ? `review ${a.slug}` : `open ${a.slug}`}>
                    {review ? "Review" : "Open"}
                  </a>
                )}
              </div>
            );
          })}
          {waiting > 0 && (
            <div className="hl-needs__row">
              <span className="hl-flow__what">runs waiting</span>
              <span className="hl-needs__said">
                {waiting} run{waiting === 1 ? " waits" : "s wait"} for an approval or an event
              </span>
              <a className="hl-flow__button" href={format(toWaiting)} onClick={follow(toWaiting, navigate)}>
                Runs waiting
              </a>
            </div>
          )}
        </section>
      )}

      {failed && <div className="hl-flow__bad">{failed}</div>}
      <section className="hl-tiles" aria-label="at a glance">
        <Tile value={summary ? liveOf(summary.totals) : "…"} label={summary && summary.totals.running === 1 ? "flow live" : "flows live"} />
        <Tile value={summary ? summary.totals.runs.toLocaleString() : "…"} label="runs in the last 24 h" />
        <Tile value={summary ? failedShare(summary.totals.runs, summary.totals.failed) : "…"} label="of them failed" bad={(summary?.totals.failed ?? 0) > 0} />
        <Tile value={String(forPerson)} label="waiting for a person" warn={forPerson > 0} />
      </section>

      <div className="hl-overview__cols">
        <div className="hl-overview__main">
          <section className="hl-flow__section" aria-label="flows">
            <h2>
              Flows
              <a className="hl-flow__more" href={format({ kind: "flows" })} onClick={follow({ kind: "flows" }, navigate)}>
                All flows
              </a>
            </h2>
            {deployments.length === 0 ? (
              <p className="hl-flow__none">Nothing deployed yet: start a new flow, or open a sample from the Library.</p>
            ) : (
              <table className="hl-table">
                <thead>
                  <tr>
                    <th>Flow</th>
                    <th>State</th>
                    <th>Starts on</th>
                    <th>Last 24 h</th>
                    <th className="hl-table__num">p50</th>
                    <th className="hl-table__num">It may</th>
                    <th>Last run</th>
                  </tr>
                </thead>
                <tbody>
                  {deployments.map((d) => {
                    const to: Route = { kind: "flow", slug: d.slug, tab: "status" };
                    return (
                      <tr key={d.slug}>
                        <td>
                          <a href={format(to)} onClick={follow(to, navigate)}>
                            {d.slug}
                          </a>
                          {d.version != null && <span className="hl-table__dim"> v{d.version}</span>}
                        </td>
                        <td>
                          <span className="hl-chips">
                            <StateChips deployed={d} waits={versionWaiting(approvals, d.slug)} />
                          </span>
                        </td>
                        <td className="hl-table__dim">{startsOn(d)}</td>
                        <SummaryCells slug={d.slug} summary={summary} may={<MayCount name={d.slug} deployed={d} />} />
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </section>

          <section className="hl-flow__section" aria-label="latest runs">
            <h2>Latest runs, all flows</h2>
            {summary && summary.latest.length === 0 ? (
              <p className="hl-flow__none">No runs yet.</p>
            ) : (
              <table className="hl-table">
                <thead>
                  <tr>
                    <th>Run</th>
                    <th>Flow</th>
                    <th>What came in</th>
                    <th>Status</th>
                    <th className="hl-table__num">Took</th>
                    <th>When</th>
                  </tr>
                </thead>
                <tbody>
                  {summary?.latest.map((r) => {
                    const to: Route = { kind: "run", slug: r.slug, seq: r.seq };
                    return (
                      <tr key={`${r.slug}:${r.seq}`}>
                        <td>
                          <a href={format(to)} onClick={follow(to, navigate)} aria-label={`${r.slug} run ${r.seq}`}>
                            #{r.seq}
                          </a>
                        </td>
                        <td className="hl-flow__what">{r.slug}</td>
                        <td className="hl-overview__route">{r.route ?? r.kind}</td>
                        <td>
                          {r.failed ? (
                            <span className="hl-flow__bad">{r.status ? `${r.status} failed` : "failed"}</span>
                          ) : r.waited ? (
                            <span className="hl-flow__waiting">waited</span>
                          ) : (
                            (r.status ?? "ok")
                          )}
                        </td>
                        <td className="hl-table__num">{tookSaid(r.duration_ms)}</td>
                        <td className="hl-table__dim">{time(r.at_ms)}</td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </section>
        </div>

        <div className="hl-overview__side">
          <section className="hl-flow__section" aria-label="run log">
            <h2>
              Run log
              <button className="hl-flow__button hl-overview__check" onClick={check} {...can.gate("host.check", { disabled: checking })}>
                {checking ? "Checking…" : "Check now"}
              </button>
            </h2>
            {summary && chain && (
              <dl className="hl-flow__facts">
                <Fact name="chain">
                  <span className={chain.bad ? "hl-flow__bad" : "hl-flow__now"} title={chain.label}>
                    {chain.value}
                  </span>
                  {summary.chain.checked ? "" : ", as written"}
                </Fact>
                <Fact name="heads">{summary.deployments.length === 1 ? <code title={summary.deployments[0].head}>{short(summary.deployments[0].head)}</code> : `one per flow (${summary.deployments.length})`}</Fact>
                <Fact name="on record">{summary.chain.on_record.toLocaleString()} runs</Fact>
                <Fact name="witnessed">
                  {summary.chain.witness
                    ? `${summary.chain.witness.taken_ms ? time(summary.chain.witness.taken_ms) : "not yet"}, by ${summary.chain.witness.address}${summary.chain.witness.waiting ? ` (${summary.chain.witness.waiting} heads wait)` : ""}`
                    : "no witness set (--export-heads)"}
                </Fact>
                <Fact name="last check">
                  {summary.chain.checked ? (
                    <span className={summary.chain.checked.breaks.length ? "hl-flow__bad" : "hl-flow__now"}>
                      {summary.chain.checked.breaks.length ? `breaks at ${summary.chain.checked.breaks.map((b) => `${b.slug} #${b.seq}`).join(", ")}` : "every line names the one before"}, {time(summary.chain.checked.at_ms)}
                    </span>
                  ) : (
                    "not since the server started"
                  )}
                </Fact>
              </dl>
            )}
          </section>

          <section className="hl-flow__section" aria-label="activity">
            <h2>What was done</h2>
            <ActivityList bare version={deployments.map((d) => `${d.slug}:${d.status}`).join(",")} />
          </section>
        </div>
      </div>
    </div>
  );
}

/** A version that waits, as Needs a person says it: the versions it goes from and to, who proposed
 *  it (an agent in the accent, a person outlined), and up to two of the new things it asks for:
 *  a secret it reads purple, as data, anything else teal. */
function Change({ a }: { a: Reviewable }) {
  const from = a.kind === "update" && a.version !== null && a.version > 1 ? `v${a.version - 1} → ` : "";
  const to = a.version === null ? "a new version" : a.kind === "first" ? "new, v1" : `v${a.version}`;
  const needs = a.proposal?.needs ?? [];
  return (
    <>
      <span className="hl-needs__versions">
        {from}
        {to}
      </span>
      {a.by && (
        <span className={`hl-chip ${a.proposal?.agent ? "hl-chip--agent" : "hl-chip--person"}`} title={a.proposal?.agent ? `proposed by the agent ${a.by}` : `saved by ${a.by}`}>
          {a.by}
        </span>
      )}
      {needs.slice(0, 2).map((n) => (
        <span key={n} className={`hl-chip ${itemParts(n).kind === "SecretRead" ? "hl-chip--secret" : "hl-chip--ext"}`} title="a new permission it asks for">
          {describeItem(n)}
        </span>
      ))}
      <span className="hl-needs__said">
        {needs.length > 2 ? `and ${needs.length - 2} more` : ""}
        {a.proposal?.why ? `${needs.length > 2 ? " · " : ""}${a.proposal.why}` : ""}
      </span>
    </>
  );
}

function Tile({ value, label, bad = false, warn = false }: { value: string; label: string; bad?: boolean; warn?: boolean }) {
  return (
    <div className="hl-tile">
      <div className={`hl-tile__value${bad ? " hl-flow__bad" : warn ? " hl-flow__waiting" : ""}`}>{value}</div>
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
