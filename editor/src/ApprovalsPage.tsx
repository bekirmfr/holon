// The Approvals page (gaps-web.md G8, C3): what waits for a person. Deploys: versions to review
// and deploy, in the queue the Review page goes through, and what else a deployment waits on a
// person for; a version an agent proposed with its why, and Decline (with a reason its author
// reads back, proposals.rs). Runs waiting: runs parked across flows that whoever is signed in may
// see or decide (`GET /runs/waiting`), an approval in steps drawn as its chain. An approval
// (`await approval(…)`) is decided here, Approve or Decline with a reason, by people signed in
// (durable.rs); an event goes on through its flow's own route.

import { Fragment } from "react";
import { routeOf, said, type Approval } from "./approvals";
import type { Me } from "./AuthGate";
import { approvalSaid, chainOf, chainSaid, decidedLine, decidedSaid, inSteps, isApproval, shownSaid, stepName, waitsAfter, yourDecision, yourSteps, type ChainStep, type WaitingPoint, type WaitingStep } from "./decisions";
import { useDialog } from "./Dialog";
import { follow, format, type Route } from "./routes";
import { decideRun, declineProposal, type DeploymentEntry, type Proposal } from "./server";
import { useToast } from "./ui/Toast";
import type { WaitingRunOf } from "./useApprovals";

const when = (seconds: number) => new Date(seconds * 1000).toLocaleString();

const KIND_SAID: Record<Approval["kind"], string> = { first: "first deploy", update: "new version", attention: "can't run", port: "port", ask: "agent's ask" };

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** Decline a proposal, asked first with why: kept on its flow's history, and read back by the
 *  agent that proposed it. `onDeclined` once it is. */
export function useDecline(onDeclined: (p: Proposal) => void) {
  const dialog = useDialog();
  const toast = useToast();
  return async (p: Proposal) => {
    const v = p.version === undefined ? "this version" : `v${p.version}`;
    const reason = await dialog.prompt(
      `Why it was declined\n\n${p.slug} ${v}, proposed by ${p.by ?? "someone"}. Tell them what to change. It is kept on the flow's history${p.agent ? ", and the agent reads it through holon mcp" : ""}. The version stays saved, and what runs keeps running.`,
      "",
      "Submit",
      { required: "A decline says why: type a reason." },
    );
    if (reason === null) return;
    try {
      await declineProposal(p.id, reason);
      toast(`Declined ${p.slug} ${v}.${p.agent ? " The agent gets your note." : ""}`, "ok");
      onDeclined(p);
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "bad");
    }
  };
}

export function ApprovalsPage({
  tab,
  approvals,
  deployments,
  runs,
  me,
  navigate,
  onDeclined,
  onDecided,
}: {
  tab: "deploys" | "runs";
  approvals: Approval[];
  /** The deployments whoever is signed in may list: a run's flow is a link only to one of them. */
  deployments: DeploymentEntry[];
  /** The runs waiting across flows that whoever is signed in may see or decide (useApprovals),
   *  null until first read. */
  runs: WaitingRunOf[] | null;
  me: Me;
  navigate: (r: Route) => void;
  /** A proposal was declined: what waits is read again. */
  onDeclined: () => void;
  /** An approval was decided: the runs and deployments are read again. */
  onDecided: () => void;
}) {
  const tabs: ["deploys" | "runs", string, number][] = [
    ["deploys", "Deploys", approvals.length],
    ["runs", "Runs waiting", runs?.length ?? 0],
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
      {tab === "deploys" ? (
        <Deploys approvals={approvals} navigate={navigate} onDeclined={onDeclined} />
      ) : (
        <RunsWaiting runs={runs} listed={new Set(deployments.map((d) => d.slug))} me={me} navigate={navigate} onDecided={onDecided} />
      )}
    </div>
  );
}

function Deploys({ approvals, navigate, onDeclined }: { approvals: Approval[]; navigate: (r: Route) => void; onDeclined: () => void }) {
  const decline = useDecline(onDeclined);
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
            const proposal = review ? a.proposal : undefined;
            return (
              <tr key={`${a.kind}:${a.slug}:${i}`}>
                <td>
                  <span className="hl-flow__what">{a.slug}</span>
                  {review && a.version !== null && <span className="hl-flow__note"> v{a.version}</span>}
                </td>
                <td>
                  <span className={`hl-chip${review ? " hl-chip--waits" : a.kind === "ask" ? "" : " hl-chip--bad"}`}>{KIND_SAID[a.kind]}</span>
                  {proposal && <span className="hl-chip hl-chip--waits">{plural(proposal.needs.length, "new permission")}</span>}
                </td>
                <td className="hl-flow__said">
                  {said(a, when)}
                  {proposal?.note && <div className="hl-flow__note">{proposal.note}</div>}
                </td>
                <td className="hl-flows__do">
                  {proposal && (
                    <button className="hl-flow__button" onClick={() => void decline(proposal)} aria-label={`decline ${a.slug}`}>
                      Decline with a reason…
                    </button>
                  )}
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

/** Approve, or decline with a reason, a step of an approval a run waits at, asked first: what
 *  the decision did is said, and `onDecided` once it is made. */
function useDecide(onDecided: () => void) {
  const dialog = useDialog();
  const toast = useToast();
  return async (slug: string, runId: string, w: WaitingPoint, step: WaitingStep, approve: boolean) => {
    const as = step.role ? ` as ${step.role}` : "";
    const after = waitsAfter(w, step);
    const reason = approve
      ? await dialog.prompt(
          `Approve "${w.name}"${as} for this run of ${slug}?\n\n${after ? `It then waits for ${after}.` : "The run goes on once you do."} A reason is optional, and kept with the run.`,
          "",
          "Approve",
          "optional",
        )
      : await dialog.prompt(`Decline "${w.name}"${as} for this run of ${slug}?\n\nWhy: it is kept with the run, and the run goes on as declined.`, "", "Decline", {
          required: "A decline says why: type a reason.",
        });
    if (reason === null) return;
    try {
      const d = await decideRun(slug, runId, w.name, step.slot, approve, reason);
      toast(decidedSaid(d), "ok");
      if (d.run_error) toast(`The run then failed: ${d.run_error}`, "bad");
      onDecided();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "bad");
    }
  };
}

/** A step of a chain on hover: each decision taken on it, a line each (who, kind, role, when, why,
 *  the binding), or that it is open, or not open yet. */
const stepTitle = (c: ChainStep) =>
  c.decisions.length > 0 ? c.decisions.map((d) => decidedLine(d, when)).join("\n") : c.state === "open" ? `${c.name}: open` : `${c.name}: opens once the steps before it are approved`;

/** An approval in steps as its chain: each step taken (✓, who took it), open (◻), or not open yet
 *  (dimmed), a decision's details on hover. */
function Chain({ w }: { w: WaitingPoint }) {
  return (
    <span className="hl-chain" role="group" aria-label={`approval "${w.name}": ${chainSaid(w)}`}>
      <span className="hl-chain__name">approval &quot;{w.name}&quot;: </span>
      {chainOf(w).map((c, i) => (
        <Fragment key={c.slot}>
          {i > 0 && <span aria-hidden="true"> → </span>}
          <span className={`hl-chain__step hl-chain__step--${c.state}`} title={stepTitle(c)}>
            {c.state === "done" ? "✓" : "◻"} {c.name}
            {c.detail && <span className="hl-chain__detail"> ({c.detail})</span>}
          </span>
        </Fragment>
      ))}
    </span>
  );
}

/** The runs waiting across flows (GET /runs/waiting): what each waits for and until when. An
 *  approval is decided here a step at a time, each step by whoever the server says may take it. */
function RunsWaiting({
  runs,
  listed,
  me,
  navigate,
  onDecided,
}: {
  runs: WaitingRunOf[] | null;
  /** The flows they may open: whoever only decides an approval of another sees its slug alone. */
  listed: Set<string>;
  me: Me;
  navigate: (r: Route) => void;
  onDecided: () => void;
}) {
  const decide = useDecide(onDecided);
  if (runs !== null && runs.length === 0) return <p className="hl-flow__none">No run is waiting.</p>;
  return (
    <section aria-label="runs waiting">
      <p className="hl-flow__note">
        An approval waits here a step at a time, in its order: each step is approved or declined by someone bound to its role, who decides
        one step, once. An event goes on when its flow is sent it, through the flow&apos;s own route (each sample&apos;s &quot;try it&quot;
        says how).
      </p>
      <table className="hl-table">
        <thead>
          <tr>
            <th>Flow</th>
            <th>Run</th>
            <th>Waits for</th>
            <th>Since</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {(runs ?? []).map(({ slug, run }) => {
            const to: Route = { kind: "flow", slug, tab: "settings" };
            return (
              <tr key={`${slug}:${run.run_id}`}>
                <td>
                  {listed.has(slug) ? (
                    <a href={format(to)} onClick={follow(to, navigate)}>
                      {slug}
                    </a>
                  ) : (
                    slug
                  )}
                </td>
                <td title={`run ${run.run_id}`}>{run.route ?? run.kind}</td>
                <td>
                  {run.waiting.map((w) => {
                    // What it shows those who decide it: what they approve.
                    const shown = isApproval(w) ? shownSaid(w) : null;
                    return (
                      <div key={w.name}>
                        {inSteps(w) ? <Chain w={w} /> : isApproval(w) ? approvalSaid(w) : w.name}
                        {` (until ${when(w.expires_at)})`}
                        {shown && (
                          <div className="hl-flow__shown" aria-label={`what "${w.name}" shows`}>
                            {shown.label}: {shown.text}
                          </div>
                        )}
                      </div>
                    );
                  })}
                  {run.delivered.length > 0 && <span className="hl-flow__note"> · has {run.delivered.join(", ")}</span>}
                </td>
                <td>{when(run.parked_at)}</td>
                <td className="hl-flows__do">
                  {run.waiting.filter(isApproval).map((w) => {
                    const done = yourDecision(w, me);
                    if (done) return <span key={w.name} className="hl-chip">{done}</span>;
                    // Each step theirs to take, a line each; named on its buttons when there are several.
                    const steps = yourSteps(w);
                    return steps.map((step) => {
                      const as = steps.length > 1 ? ` as ${stepName(step)}` : "";
                      const label = `"${w.name}"${as} of ${slug} (${run.route ?? run.kind})`;
                      return (
                        <div key={`${w.name}:${step.slot}`} className="hl-flows__step">
                          <button className="hl-flow__button" onClick={() => void decide(slug, run.run_id, w, step, false)} aria-label={`decline ${label}`}>
                            Decline{as}…
                          </button>
                          <button className="hl-flow__button" onClick={() => void decide(slug, run.run_id, w, step, true)} aria-label={`approve ${label}`}>
                            Approve{as}…
                          </button>
                        </div>
                      );
                    });
                  })}
                </td>
              </tr>
            );
          })}
          {runs === null && (
            <tr>
              <td colSpan={5} className="hl-table__dim">
                reading…
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </section>
  );
}
