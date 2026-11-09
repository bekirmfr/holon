// The Overview (`#overview`, gaps-web.md G13, C12), where the console opens: what needs a person
// (approvals.ts), four tiles (runs in the last 24 hours, the share that failed, the flows live and
// held, the run log), every deployed flow with its runs hour by hour, the newest runs across
// every flow, the run log with Check now, and what was done on the host. All of it from the
// host's summary (GET /summary, holon-server's summary.rs), read again every 10 s while shown;
// Check now reads every log's lines again (POST /summary/check). Notes and the first steps are
// under Getting started (WelcomePage.tsx).

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { routeOf, said, type Approval } from "./approvals";
import { checkChains, getSummary, getWarnings, type DeploymentEntry, type HostWarning, type Summary } from "./server";
import { chainSaid, failedShare, greeting, hostLine, liveSaid } from "./overview";
import { follow, format, type Route } from "./routes";
import { ActivityList } from "./Pages";
import { Spark } from "./ui/Spark";
import { useToast } from "./ui/Toast";

const STATE_SAID: Record<DeploymentEntry["status"], string> = { running: "live", paused: "paused", stopped: "stopped" };
const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
const short = (hash: string) => (hash ? `${hash.slice(0, 4)}…${hash.slice(-4)}` : "–");

export function OverviewPage({
  name,
  admin,
  deployments,
  approvals,
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
  navigate: (r: Route) => void;
  onNew: () => void;
}) {
  const notify = useToast();
  const [summary, setSummary] = useState<Summary | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [warnings, setWarnings] = useState<HostWarning[]>([]);
  const [checking, setChecking] = useState(false);

  const read = useCallback(() => {
    getSummary(24).then(
      (s) => {
        setSummary(s);
        setFailed(null);
      },
      (e: unknown) => setFailed(e instanceof Error ? e.message : String(e)),
    );
  }, []);
  useEffect(() => {
    read();
    const timer = setInterval(read, 10000);
    return () => clearInterval(timer);
  }, [read, deployments.length]);
  useEffect(() => {
    if (!admin) return;
    getWarnings()
      .then(setWarnings)
      .catch(() => setWarnings([]));
  }, [admin, deployments]);

  const check = async () => {
    setChecking(true);
    try {
      const c = await checkChains();
      if (c.breaks.length === 0) notify(`Checked ${c.runs.toLocaleString()} runs: every line names the one before it.`, "ok");
      else notify(`Checked ${c.runs.toLocaleString()} runs: the chain breaks at ${c.breaks.map((b) => `${b.slug} run ${b.seq}`).join(", ")}.`, "bad");
      read();
    } catch (e) {
      notify(e instanceof Error ? e.message : String(e), "bad");
    } finally {
      setChecking(false);
    }
  };

  const waiting = deployments.reduce((n, d) => n + (d.waiting ?? 0), 0);
  const bySlug = new Map(summary?.deployments.map((d) => [d.slug, d]) ?? []);
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
                <span className="hl-flow__what">
                  {a.slug}
                  {review && a.version !== null ? ` v${a.version}` : ""}
                </span>
                <span className="hl-needs__said">{said(a, (s) => time(s * 1000))}</span>
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
              <span className="hl-flow__what">waiting runs</span>
              <span className="hl-needs__said">
                {waiting} run{waiting === 1 ? " waits" : "s wait"} for an approval or an event
              </span>
              <a className="hl-flow__button" href={format(toWaiting)} onClick={follow(toWaiting, navigate)}>
                Open
              </a>
            </div>
          )}
        </section>
      )}

      {failed && <div className="hl-flow__bad">{failed}</div>}
      <section className="hl-tiles" aria-label="at a glance">
        <Tile value={summary ? summary.totals.runs.toLocaleString() : "…"} label="runs in the last 24 h" />
        <Tile value={summary ? failedShare(summary.totals.runs, summary.totals.failed) : "…"} label="of them failed" bad={(summary?.totals.failed ?? 0) > 0} />
        <Tile value={summary ? String(summary.totals.running) : "…"} label={summary ? liveSaid(summary.totals) : "flows live"} />
        <Tile value={chain?.value ?? "…"} label={chain?.label ?? "run log"} bad={chain?.bad} ok={chain ? !chain.bad : false} />
      </section>

      <div className="hl-overview__cols">
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
                  <th>Runs on</th>
                  <th>State</th>
                  <th>Last 24 h</th>
                  <th className="hl-table__num">Runs</th>
                </tr>
              </thead>
              <tbody>
                {deployments.map((d) => {
                  const s = bySlug.get(d.slug);
                  const to: Route = { kind: "flow", slug: d.slug, tab: "status" };
                  const day = s ? s.runs.reduce((a, b) => a + b, 0) : null;
                  return (
                    <tr key={d.slug}>
                      <td>
                        <a href={format(to)} onClick={follow(to, navigate)}>
                          {d.slug}
                        </a>
                      </td>
                      <td className="hl-table__dim">{[d.port !== null ? `:${d.port}` : null, ...d.triggers].filter(Boolean).join(", ") || "–"}</td>
                      <td>
                        <span className={`hl-chip hl-chip--${d.status}`}>{STATE_SAID[d.status]}</span>
                      </td>
                      <td>{s ? <Spark runs={s.runs} failed={s.failed} /> : <span className="hl-table__dim">…</span>}</td>
                      <td className="hl-table__num">{day === null ? "…" : day.toLocaleString()}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </section>

        <div className="hl-overview__side">
          <section className="hl-flow__section" aria-label="latest runs">
            <h2>Latest runs, all flows</h2>
            {summary && summary.latest.length === 0 && <p className="hl-flow__none">No runs yet.</p>}
            <ul className="hl-overview__runs">
              {summary?.latest.map((r) => {
                const to: Route = { kind: "run", slug: r.slug, seq: r.seq };
                return (
                  <li key={`${r.slug}:${r.seq}`}>
                    <a href={format(to)} onClick={follow(to, navigate)} aria-label={`${r.slug} run ${r.seq}`}>
                      <span className="hl-table__dim">#{r.seq}</span>
                      <span className="hl-flow__what">{r.slug}</span>
                      <span className="hl-overview__route">{r.route ?? r.kind}</span>
                      <span className={r.failed ? "hl-flow__bad" : "hl-flow__now"}>{r.failed ? "failed" : r.waited ? "waited" : (r.status ?? "ok")}</span>
                    </a>
                  </li>
                );
              })}
            </ul>
          </section>

          <section className="hl-flow__section" aria-label="run log">
            <h2>
              Run log
              <button className="hl-flow__button hl-overview__check" onClick={check} disabled={checking}>
                {checking ? "Checking…" : "Check now"}
              </button>
            </h2>
            {summary && (
              <dl className="hl-flow__facts">
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
        </div>
      </div>

      <section className="hl-flow__section" aria-label="activity">
        <h2>What was done</h2>
        <ActivityList bare version={deployments.map((d) => `${d.slug}:${d.status}`).join(",")} />
      </section>
    </div>
  );
}

function Tile({ value, label, bad = false, ok = false }: { value: string; label: string; bad?: boolean; ok?: boolean }) {
  return (
    <div className="hl-tile">
      <div className={`hl-tile__value${bad ? " hl-flow__bad" : ok ? " hl-flow__now" : ""}`}>{value}</div>
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
