// The Approvals page (gaps-web.md G8, C3): what waits for a person. Deploys: versions to review
// and deploy, in the queue the Review page goes through, and what else a deployment waits on a
// person for. Runs waiting: runs parked for an event, each answered through its flow's own route
// (a person approves one as the flow says; the host doesn't deliver events yet, G12).

import { useEffect, useState } from "react";
import { routeOf, said, type Approval } from "./approvals";
import { follow, format, type Route } from "./routes";
import { listWaitingRuns, type DeploymentEntry, type WaitingRun } from "./server";

const when = (seconds: number) => new Date(seconds * 1000).toLocaleString();

const KIND_SAID: Record<Approval["kind"], string> = { first: "first deploy", update: "new version", attention: "can't run", port: "port", ask: "agent's ask" };

export function ApprovalsPage({
  tab,
  approvals,
  deployments,
  navigate,
}: {
  tab: "deploys" | "runs";
  approvals: Approval[];
  deployments: DeploymentEntry[];
  navigate: (r: Route) => void;
}) {
  const waiting = deployments.filter((d) => (d.waiting ?? 0) > 0);
  const runs = waiting.reduce((n, d) => n + (d.waiting ?? 0), 0);
  const tabs: ["deploys" | "runs", string, number][] = [
    ["deploys", "Deploys", approvals.length],
    ["runs", "Runs waiting", runs],
  ];
  return (
    <div className="hl-flow hl-approvals">
      <header className="hl-flow__head">
        <h1 className="hl-flow__name">Approvals</h1>
        <span className="hl-flow__status">what waits for a person</span>
      </header>
      <nav className="hl-flow__tabs" aria-label="approvals tabs">
        {tabs.map(([t, label, n]) => {
          const to: Route = { kind: "approvals", tab: t };
          return (
            <a key={t} href={format(to)} aria-current={t === tab ? "page" : undefined} onClick={follow(to, navigate)}>
              {label}
              <span className="hl-flow__count">{n}</span>
            </a>
          );
        })}
      </nav>
      {tab === "deploys" ? <Deploys approvals={approvals} navigate={navigate} /> : <RunsWaiting deployments={waiting} navigate={navigate} />}
    </div>
  );
}

function Deploys({ approvals, navigate }: { approvals: Approval[]; navigate: (r: Route) => void }) {
  if (approvals.length === 0) return <p className="hl-flow__none">Nothing waits for you: every saved version is deployed, and no agent asked for anything.</p>;
  return (
    <section aria-label="deploys">
      <table className="hl-table">
        <thead>
          <tr>
            <th>Flow</th>
            <th>Waits for</th>
            <th>What</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {approvals.map((a, i) => {
            const to = routeOf(a);
            const review = a.kind === "first" || a.kind === "update";
            return (
              <tr key={`${a.kind}:${a.slug}:${i}`}>
                <td>
                  <span className="hl-flow__what">{a.slug}</span>
                  {review && a.version !== null && <span className="hl-flow__note"> v{a.version}</span>}
                </td>
                <td>
                  <span className={`hl-chip${review ? " hl-chip--waits" : a.kind === "ask" ? "" : " hl-chip--bad"}`}>{KIND_SAID[a.kind]}</span>
                </td>
                <td className="hl-flow__said">{said(a, when)}</td>
                <td className="hl-flows__do">
                  {to === null ? (
                    <span className="hl-table__dim">…</span>
                  ) : (
                    <a className="hl-flow__button" href={format(to)} onClick={follow(to, navigate)} aria-label={review ? `review ${a.slug}` : `open ${a.slug}`}>
                      {review ? "Review" : "Open"}
                    </a>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </section>
  );
}

/** Each deployment's parked runs (GET …/runs): what each waits for and until when. */
function RunsWaiting({ deployments, navigate }: { deployments: DeploymentEntry[]; navigate: (r: Route) => void }) {
  const key = deployments.map((d) => `${d.slug}:${d.waiting}`).join(",");
  const [runs, setRuns] = useState<{ slug: string; run: WaitingRun }[] | null>(null);
  useEffect(() => {
    let fresh = true;
    Promise.all(deployments.map((d) => listWaitingRuns(d.slug).then((rs) => rs.map((run) => ({ slug: d.slug, run })), () => []))).then((all) => fresh && setRuns(all.flat()));
    return () => {
      fresh = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  if (deployments.length === 0) return <p className="hl-flow__none">No run is waiting for an event.</p>;
  return (
    <section aria-label="runs waiting">
      <p className="hl-flow__note">A waiting run goes on when its flow is sent what it waits for, through the flow's own route (each sample's "try it" says how).</p>
      <table className="hl-table">
        <thead>
          <tr>
            <th>Flow</th>
            <th>Run</th>
            <th>Waits for</th>
            <th>Since</th>
          </tr>
        </thead>
        <tbody>
          {(runs ?? []).map(({ slug, run }) => {
            const to: Route = { kind: "flow", slug, tab: "settings" };
            return (
              <tr key={`${slug}:${run.run_id}`}>
                <td>
                  <a href={format(to)} onClick={follow(to, navigate)}>
                    {slug}
                  </a>
                </td>
                <td>{run.route ?? run.kind}</td>
                <td>
                  {run.waiting.map((w) => `${w.name} (until ${when(w.expires_at)})`).join(", ")}
                  {run.delivered.length > 0 && <span className="hl-flow__note"> · has {run.delivered.join(", ")}</span>}
                </td>
                <td>{when(run.parked_at)}</td>
              </tr>
            );
          })}
          {runs === null && (
            <tr>
              <td colSpan={4} className="hl-table__dim">
                reading…
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </section>
  );
}
