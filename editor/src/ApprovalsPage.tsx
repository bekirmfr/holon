// The Approvals page (gaps-web.md G8, C3): what waits for a person. Deploys: versions to review
// and deploy, in the queue the Review page goes through, and what else a deployment waits on a
// person for; a version an agent proposed with its why, and Decline (with a reason its author
// reads back, proposals.rs). Runs waiting: runs parked across flows. An approval (`await
// approval(…)`) is decided here, Approve or Decline with a reason, by people signed in (durable.rs);
// an event goes on through its flow's own route.

import { routeOf, said, type Approval } from "./approvals";
import type { Me } from "./AuthGate";
import { approvalSaid, isApproval, mayDecide, pointKey, type WaitingPoint } from "./decisions";
import { useDialog } from "./Dialog";
import { follow, format, type Route } from "./routes";
import { decideRun, declineProposal, type DeploymentEntry, type Proposal } from "./server";
import { useKeys, useListKeys } from "./ui/Keys";
import { useToast } from "./ui/Toast";
import type { WaitingRunOf } from "./useApprovals";
import { gateOf, useCan } from "./useCan";

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
  decides,
  me,
  navigate,
  onDeclined,
  onDecided,
}: {
  tab: "deploys" | "runs";
  approvals: Approval[];
  deployments: DeploymentEntry[];
  /** The runs waiting in them (useApprovals), null until first read. */
  runs: WaitingRunOf[] | null;
  /** Whether `me` decides each approval point among them (decisions.ts `pointKey`): true, or the
   *  server's refusal. */
  decides: ReadonlyMap<string, true | string>;
  me: Me;
  navigate: (r: Route) => void;
  /** A proposal was declined: what waits is read again. */
  onDeclined: () => void;
  /** An approval was decided: the runs and deployments are read again. */
  onDecided: () => void;
}) {
  const waiting = deployments.filter((d) => (d.waiting ?? 0) > 0);
  const waitingRuns = waiting.reduce((n, d) => n + (d.waiting ?? 0), 0);
  const tabs: ["deploys" | "runs", string, number][] = [
    ["deploys", "Deploys", approvals.length],
    ["runs", "Runs waiting", waitingRuns],
  ];
  // [ and ]: the other tab (there are two).
  const other = () => navigate({ kind: "approvals", tab: tab === "deploys" ? "runs" : "deploys" });
  useKeys("tabs", { "[": other, "]": other });
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
        <RunsWaiting any={waiting.length > 0} runs={runs} decides={decides} me={me} navigate={navigate} onDecided={onDecided} />
      )}
    </div>
  );
}

/** Decline a proposal of `slug`'s, for whoever may approve its deploys (`flow.approve-deploy`
 *  on the flow, as declining asks it); disabled, saying why not, for anyone else. */
function DeclineButton({ slug, onDecline }: { slug: string; onDecline: () => void }) {
  const can = useCan(["flow.approve-deploy"], { flow: slug });
  return (
    <button className="hl-flow__button" onClick={onDecline} aria-label={`decline ${slug}`} {...can.gate("flow.approve-deploy")}>
      Decline with a reason…
    </button>
  );
}

/** A row's key in Deploys' list: what it waits for, on which flow. */
const deployKey = (a: Approval, i: number) => `${a.kind}:${a.slug}:${i}`;

function Deploys({ approvals, navigate, onDeclined }: { approvals: Approval[]; navigate: (r: Route) => void; onDeclined: () => void }) {
  const decline = useDecline(onDeclined);
  const list = useListKeys(
    "approvals.deploys",
    approvals.map(deployKey),
    (key) => {
      const i = approvals.findIndex((a, k) => deployKey(a, k) === key);
      const to = i < 0 ? null : routeOf(approvals[i]);
      if (to) navigate(to);
    },
  );
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
              <tr key={deployKey(a, i)} {...list.row(deployKey(a, i))}>
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
                  {proposal && <DeclineButton slug={a.slug} onDecline={() => void decline(proposal)} />}
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

/** Approve, or decline with a reason, an approval a run waits at, asked first: what the decision
 *  did is said, and `onDecided` once it is made. */
function useDecide(onDecided: () => void) {
  const dialog = useDialog();
  const toast = useToast();
  return async (slug: string, runId: string, w: WaitingPoint, approve: boolean) => {
    const people = w.people ?? 1;
    const left = people - (w.approved ?? []).length;
    const reason = approve
      ? await dialog.prompt(
          `Approve "${w.name}" for this run of ${slug}?\n\n${left > 1 ? `It needs ${people} different people: ${left - 1} more after you.` : "The run goes on once you do."} A reason is optional, and kept with the run.`,
          "",
          "Approve",
          true,
        )
      : await dialog.prompt(`Decline "${w.name}" for this run of ${slug}?\n\nWhy: it is kept with the run, and the run goes on as declined.`, "", "Decline", true);
    if (reason === null) return;
    if (!approve && reason.trim() === "") {
      toast("A decline says why: give a reason.", "bad");
      return;
    }
    try {
      const d = await decideRun(slug, runId, w.name, approve, reason);
      const more = d.people - d.approved_by.length;
      toast(
        d.outcome === "waiting"
          ? `Approved: ${d.approved_by.length} of ${d.people}. It waits for ${more} more ${more === 1 ? "person" : "people"}.`
          : d.outcome === "approved"
            ? `Approved by ${d.approved_by.join(" and ")}: the run goes on.`
            : "Declined: the run goes on as declined.",
        "ok",
        d.outcome === "waiting" ? undefined : { label: "Open its runs", to: { kind: "flow", slug, tab: "runs" } },
      );
      if (d.run_error) toast(`The run then failed: ${d.run_error}`, "bad");
      onDecided();
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "bad");
    }
  };
}

/** Runs waiting's list, by its name (ui/Keys.tsx's `pick`: the palette goes to a row of it). */
export const WAITING_LIST = "approvals.runs";
/** A run's key in Runs waiting's list. */
export const waitingKey = (slug: string, runId: string) => `${slug}:${runId}`;

/** The runs waiting across flows (GET …/runs): what each waits for and until when. An approval is
 *  decided here, by a person who may. */
function RunsWaiting({
  any,
  runs,
  decides,
  me,
  navigate,
  onDecided,
}: {
  any: boolean;
  runs: WaitingRunOf[] | null;
  decides: ReadonlyMap<string, true | string>;
  me: Me;
  navigate: (r: Route) => void;
  onDecided: () => void;
}) {
  const decide = useDecide(onDecided);
  // j and k move through the runs; Enter opens the flow the run is of.
  const list = useListKeys(
    WAITING_LIST,
    (runs ?? []).map(({ slug, run }) => waitingKey(slug, run.run_id)),
    (key) => {
      const r = (runs ?? []).find(({ slug, run }) => waitingKey(slug, run.run_id) === key);
      if (r) navigate({ kind: "flow", slug: r.slug, tab: "settings" });
    },
  );
  if (!any) return <p className="hl-flow__none">No run is waiting.</p>;
  return (
    <section aria-label="runs waiting">
      <p className="hl-flow__note">
        An approval waits for people here: each person who may decide it approves or declines it, and counts once. An event goes on when its
        flow is sent it, through the flow&apos;s own route (each sample&apos;s &quot;try it&quot; says how).
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
              <tr key={waitingKey(slug, run.run_id)} {...list.row(waitingKey(slug, run.run_id))}>
                <td>
                  <a href={format(to)} onClick={follow(to, navigate)}>
                    {slug}
                  </a>
                </td>
                <td title={`run ${run.run_id}`}>{run.route ?? run.kind}</td>
                <td>
                  {run.waiting.map((w) => `${isApproval(w) ? approvalSaid(w) : w.name} (until ${when(w.expires_at)})`).join(", ")}
                  {run.delivered.length > 0 && <span className="hl-flow__note"> · has {run.delivered.join(", ")}</span>}
                </td>
                <td>{when(run.parked_at)}</td>
                <td className="hl-flows__do">
                  {run.waiting.filter(isApproval).map((w) => {
                    const answer = decides.get(pointKey(slug, w.name));
                    const may = mayDecide(w, me, answer === true);
                    if (may === "approved") return <span key={w.name} className="hl-chip">you approved</span>;
                    // Not theirs to decide: the buttons say why, once the server has (run.decide).
                    if (may === "no" && typeof answer !== "string") return null;
                    const gate = gateOf(answer ?? true);
                    const label = `"${w.name}" of ${slug} (${run.route ?? run.kind})`;
                    return (
                      <span key={w.name}>
                        <button className="hl-flow__button" onClick={() => void decide(slug, run.run_id, w, false)} aria-label={`decline ${label}`} {...gate}>
                          Decline…
                        </button>
                        <button className="hl-flow__button" onClick={() => void decide(slug, run.run_id, w, true)} aria-label={`approve ${label}`} {...gate}>
                          Approve…
                        </button>
                      </span>
                    );
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
