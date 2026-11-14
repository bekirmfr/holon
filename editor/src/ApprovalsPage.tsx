// The Approvals page (gaps-web.md G8, C3; web-console-L4.md §7.2): what waits for a person.
// Deploys: a card for each version proposed, with its place in the queue the Review page goes
// through, who proposed it and why, and the rows of what it may do that change (the Review's,
// reviewMay.ts, read when the card comes into view); then as rows what else a deployment waits on
// a person for. Decline (with a reason its author reads back, proposals.rs). Runs waiting: runs
// parked across flows that whoever is signed in may see or decide (`GET /runs/waiting`), what each
// is and until when, an approval in steps drawn as its chain. An approval (`await approval(…)`) is
// decided here, Approve or Decline with a reason, by people signed in (durable.rs); an event goes
// on through its flow's own route. `a` and `d` approve and decline the one selected. Access
// changes: a change to who may do what that gives someone more, kept while this host has people
// approve one (access_proposals.rs), with what it gives and who could act as whom it gives more;
// Approve and Decline to whoever the server would let, its author's Withdraw, and those decided
// lately.

import { Fragment, useEffect, useState } from "react";
import { changeState, decidedChange, notYours } from "./access";
import { reviewQueue, routeOf, said, versionsSaid, type Approval, type Reviewable } from "./approvals";
import type { Me } from "./AuthGate";
import { canOn, useCan } from "./can";
import { approvalSaid, chainOf, chainSaid, decidedLine, decidedSaid, inSteps, isApproval, shownSaid, stepName, waitsAfter, yourDecision, yourSteps, type ChainStep, type WaitingPoint, type WaitingStep } from "./decisions";
import { useDialog } from "./Dialog";
import { cardRows, mayGroups, textLines, type CardRows } from "./reviewMay";
import { follow, format, type ApprovalsTab, type Route } from "./routes";
import { startedBy } from "./runs";
import {
  approveAccessChange,
  decideRun,
  declineAccessChange,
  declineProposal,
  getProposal,
  previewDeploy,
  type AccessProposal,
  type DeploymentEntry,
  type FlowDocument,
  type Proposal,
} from "./server";
import { useKeys, useListKeys } from "./ui/Keys";
import { useToast } from "./ui/Toast";
import type { WaitingRunOf } from "./useApprovals";

const when = (seconds: number) => new Date(seconds * 1000).toLocaleString();

const KIND_SAID: Record<Approval["kind"], string> = { first: "first deploy", update: "new version", attention: "can't run", port: "port", ask: "agent's ask" };

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
  access,
  me,
  navigate,
  textOf,
  onDeclined,
  onDecided,
}: {
  tab: ApprovalsTab;
  approvals: Approval[];
  /** The deployments whoever is signed in may list: a run's flow is a link only to one of them. */
  deployments: DeploymentEntry[];
  /** The runs waiting across flows that whoever is signed in may see or decide (useApprovals),
   *  null until first read. */
  runs: WaitingRunOf[] | null;
  /** Every access change, newest first (useApprovals), null until first read. */
  access: AccessProposal[] | null;
  me: Me;
  navigate: (r: Route) => void;
  /** A document as its text, for the lines a card's rows point at. Throws when it can't be written. */
  textOf: (slug: string, doc: FlowDocument) => string;
  /** A proposal was declined: what waits is read again. */
  onDeclined: () => void;
  /** An approval, or an access change, was decided: what waits is read again. */
  onDecided: () => void;
}) {
  const tabs: [ApprovalsTab, string, number][] = [
    ["deploys", "Deploys", approvals.length],
    ["runs", "Runs waiting", runs?.length ?? 0],
    ["access", "Access changes", (access ?? []).filter((p) => p.state === "waiting").length],
  ];
  // [ and ]: the tab before and after, round.
  const step = (by: number) => navigate({ kind: "approvals", tab: tabs[(tabs.findIndex(([t]) => t === tab) + by + tabs.length) % tabs.length][0] });
  useKeys("tabs", { "[": () => step(-1), "]": () => step(1) });
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
        <Deploys approvals={approvals} navigate={navigate} textOf={textOf} onDeclined={onDeclined} />
      ) : tab === "runs" ? (
        <RunsWaiting runs={runs} listed={new Set(deployments.map((d) => d.slug))} me={me} navigate={navigate} onDecided={onDecided} />
      ) : (
        <AccessChanges changes={access} me={me} onDecided={onDecided} />
      )}
    </div>
  );
}

/** Decline a proposal of `slug`'s, for whoever may approve its deploys (`flow.approve-deploy`
 *  on the flow, as declining asks it); disabled, saying why not, for anyone else. */
function DeclineButton({ slug, onDecline }: { slug: string; onDecline: () => void }) {
  const can = useCan(slug);
  return (
    <button className="hl-flow__button" onClick={onDecline} aria-label={`decline ${slug}`} {...can.gate("decline")}>
      Decline with a reason…
    </button>
  );
}

/** A row's key in Deploys' list: what it waits for, on which flow. */
const deployKey = (a: Approval, i: number) => `${a.kind}:${a.slug}:${i}`;

/** A version proposed, shown as a card; the rest are rows. */
const proposed = (a: Approval): a is Reviewable & { proposal: Proposal } => (a.kind === "first" || a.kind === "update") && a.proposal !== undefined;

/** The rows a proposal changes, as its review reads them: its document, and a preview of its unit
 *  against the grant now, each row pointing at its line. */
export async function readCard(p: Proposal, textOf: (slug: string, doc: FlowDocument) => string): Promise<CardRows> {
  const { document } = await getProposal(p.id);
  const preview = await previewDeploy(p.slug, p.unit, document?.server?.requirements);
  let text = document?.source ?? "";
  try {
    if (document) text = textOf(p.slug, document);
  } catch {
    // The lines it had as typed, or none.
  }
  return cardRows(mayGroups(preview.touches, preview.settings, textLines(text)));
}

/** Whether the element `ref` is given has come into view, once (at once where nothing can tell). */
function useSeen(): [(el: Element | null) => void, boolean] {
  const [el, setEl] = useState<Element | null>(null);
  const [seen, setSeen] = useState(false);
  useEffect(() => {
    if (!el || seen) return;
    if (typeof IntersectionObserver === "undefined") {
      setSeen(true);
      return;
    }
    const watch = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) setSeen(true);
    });
    watch.observe(el);
    return () => watch.disconnect();
  }, [el, seen]);
  return [setEl, seen];
}

/** One proposal: its slug and versions, its place in the queue, who proposed it and when, its
 *  why, the rows it changes (+ new, amber; − no longer), each with its line, and how many stay the
 *  same; Decline with a reason and Review. The rows are read once it comes into view. */
export function ProposalCard({
  a,
  place,
  of,
  textOf,
  selected,
  rowRef,
  navigate,
  onDecline,
}: {
  a: Reviewable & { proposal: Proposal };
  /** Its place in the queue (1-based), of how many. */
  place: number;
  of: number;
  textOf: (slug: string, doc: FlowDocument) => string;
  selected: boolean;
  rowRef?: (el: HTMLElement | null) => void;
  navigate: (r: Route) => void;
  onDecline: () => void;
}) {
  const [seenRef, seen] = useSeen();
  const [rows, setRows] = useState<CardRows | null>(null);
  const [error, setError] = useState("");
  const p = a.proposal;
  useEffect(() => {
    if (!seen) return;
    let fresh = true;
    setError("");
    readCard(p, textOf).then(
      (r) => fresh && setRows(r),
      (e: unknown) => fresh && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      fresh = false;
    };
    // Read again for another proposal of it, not on every poll.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seen, p.id]);
  const to = routeOf(a);
  const line = (n: number | null) => n !== null && <span className="hl-approval-card__line">line {n}</span>;
  return (
    <article
      ref={(el) => {
        seenRef(el);
        rowRef?.(el);
      }}
      className="hl-approval-card"
      aria-label={`proposal ${a.slug}`}
      aria-current={selected ? "true" : undefined}
      data-at={selected ? "" : undefined}
    >
      <header className="hl-approval-card__head">
        <span className="hl-flow__what">{a.slug}</span>
        <span className="hl-approval-card__versions">{versionsSaid(a)}</span>
        <span className="hl-flow__spacer" />
        <span className="hl-approval-card__place" title="its place in the queue the reviews go through">
          {place} of {of}
        </span>
      </header>
      <div className="hl-approval-card__by">
        proposed by{" "}
        <span className={`hl-chip ${p.agent ? "hl-chip--agent" : "hl-chip--person"}`} title={p.agent ? `the agent ${a.by ?? ""}` : undefined}>
          {a.by ?? "someone"}
        </span>
        , {when(a.at)}
      </div>
      <p className="hl-approval-card__why">{p.why || <span className="hl-table__dim">no why was given</span>}</p>
      {p.note && <blockquote className="hl-review-page__note">{p.note}</blockquote>}
      {error ? (
        <p className="hl-flow__note">What it may do couldn&apos;t be read: {error}</p>
      ) : rows === null ? (
        <p className="hl-flow__note">{seen ? "reading what it may do…" : " "}</p>
      ) : (
        <>
          {rows.approve.length + rows.gone.length > 0 ? (
            <ul className="hl-approval-card__rows" aria-label="what changes">
              {rows.approve.map((r, i) => (
                <li key={`a${i}`} className="hl-approval-card__row hl-approval-card__row--new">
                  <span className="hl-approval-card__sign">+</span>
                  <span>{r.line.text}</span>
                  {line(r.anchor)}
                </li>
              ))}
              {rows.gone.map((r, i) => (
                <li key={`g${i}`} className="hl-approval-card__row hl-approval-card__row--gone">
                  <span className="hl-approval-card__sign">−</span>
                  <span>{r.line.text}</span>
                  {line(r.anchor)}
                </li>
              ))}
            </ul>
          ) : (
            <p className="hl-flow__note">It asks for nothing new, and gives nothing up.</p>
          )}
          <p className="hl-approval-card__same">{rows.same} the same</p>
        </>
      )}
      <footer className="hl-approval-card__do">
        <DeclineButton slug={a.slug} onDecline={onDecline} />
        {to === null ? (
          <span className="hl-table__dim">…</span>
        ) : (
          <a className="hl-flow__button" href={format(to)} onClick={follow(to, navigate)} aria-label={`review ${a.slug}`}>
            Review
          </a>
        )}
      </footer>
    </article>
  );
}

function Deploys({
  approvals,
  navigate,
  textOf,
  onDeclined,
}: {
  approvals: Approval[];
  navigate: (r: Route) => void;
  textOf: (slug: string, doc: FlowDocument) => string;
  onDeclined: () => void;
}) {
  const decline = useDecline(onDeclined);
  const toast = useToast();
  const queue = reviewQueue(approvals);
  // The cards first, then the rows: the order j and k go through.
  const keyed = approvals.map((a, i) => ({ a, key: deployKey(a, i) }));
  const cards = keyed.filter(({ a }) => proposed(a));
  const rows = keyed.filter(({ a }) => !proposed(a));
  const order = [...cards, ...rows];
  const open = (key: string) => {
    const a = order.find((o) => o.key === key)?.a;
    const to = a ? routeOf(a) : null;
    if (to) navigate(to);
  };
  const list = useListKeys(
    "approvals.deploys",
    order.map((o) => o.key),
    open,
  );
  const selected = order.find((o) => o.key === list.at)?.a;
  // a: a version is approved from its review, read first; d: a proposal declined, with a reason.
  useKeys("approvals", {
    a: () => {
      if (!selected) return toast("Select one first: j and k.", "info");
      open(list.at!);
    },
    d: () => {
      if (!selected) return toast("Select one first: j and k.", "info");
      if (!proposed(selected)) return toast(`Only a version proposed is declined here: the ${KIND_SAID[selected.kind]} of ${selected.slug} isn't one.`, "info");
      const p = selected.proposal;
      void canOn(p.slug).then((can) => {
        const answer = can.may("decline");
        if (answer === true) void decline(p);
        else toast(answer, "bad");
      });
    },
  });
  if (approvals.length === 0) return <p className="hl-flow__none">Nothing waits for you: every saved version is deployed, and no agent asked for anything.</p>;
  return (
    <section aria-label="deploys">
      {cards.length > 0 && (
        <div className="hl-approval-cards" aria-label="proposals">
          {cards.map(({ a, key }) => {
            if (!proposed(a)) return null;
            const r = list.row(key);
            return (
              <ProposalCard
                key={key}
                a={a}
                place={queue.indexOf(a) + 1}
                of={queue.length}
                textOf={textOf}
                selected={r["aria-current"] !== undefined}
                rowRef={r.ref}
                navigate={navigate}
                onDecline={() => void decline(a.proposal)}
              />
            );
          })}
        </div>
      )}
      {rows.length > 0 && (
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
            {rows.map(({ a, key }) => {
              const to = routeOf(a);
              const review = a.kind === "first" || a.kind === "update";
              return (
                <tr key={key} {...list.row(key)}>
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
      )}
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
      toast(decidedSaid(d), "ok", d.outcome === "waiting" ? undefined : { label: "Open its runs", to: { kind: "flow", slug, tab: "runs" } });
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


/** Runs waiting's list, by its name (ui/Keys.tsx's `pick`: the palette goes to a row of it). */
export const WAITING_LIST = "approvals.runs";
/** A run's key in Runs waiting's list. */
export const waitingKey = (slug: string, runId: string) => `${slug}:${runId}`;

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
  const toast = useToast();
  // j and k move through the runs; Enter opens the flow the run is of.
  const list = useListKeys(
    WAITING_LIST,
    (runs ?? []).map(({ slug, run }) => waitingKey(slug, run.run_id)),
    (key) => {
      const r = (runs ?? []).find(({ slug, run }) => waitingKey(slug, run.run_id) === key);
      if (r) navigate({ kind: "flow", slug: r.slug, tab: "settings" });
    },
  );
  // a and d: the selected run's first step whoever is signed in may take, asked first (the question
  // is its review); else why not.
  const decideSelected = (approve: boolean) => {
    const r = (runs ?? []).find(({ slug, run }) => waitingKey(slug, run.run_id) === list.at);
    if (!r) return toast("Select a run first: j and k.", "info");
    const points = r.run.waiting.filter(isApproval);
    const mine = points.find((w) => yourSteps(w).length > 0);
    if (mine) return void decide(r.slug, r.run.run_id, mine, yourSteps(mine)[0], approve);
    if (points.length === 0) return toast("It waits for an event, which its flow's own route delivers: nobody approves it here.", "info");
    const done = points.map((w) => yourDecision(w, me)).find((d) => d);
    if (done) return toast(`You decided it (${done}): it waits for others.`, "info");
    toast("No step of it is yours to take now.", "bad");
  };
  useKeys("approvals", { a: () => decideSelected(true), d: () => decideSelected(false) });
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
            <th>What it is</th>
            <th>Started by</th>
            <th>Waits for</th>
            <th>Expires</th>
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
                  {listed.has(slug) ? (
                    <a href={format(to)} onClick={follow(to, navigate)}>
                      {slug}
                    </a>
                  ) : (
                    slug
                  )}
                </td>
                <td title={`run ${run.run_id}`}>
                  <span className="hl-flow__what">{run.route ?? run.kind}</span>
                  {run.waiting.map((w) => (
                    <div key={w.name} className="hl-flow__note">
                      {isApproval(w) ? "approval" : "event"} &quot;{w.name}&quot;
                    </div>
                  ))}
                  {run.delivered.length > 0 && <div className="hl-flow__note">has {run.delivered.join(", ")}</div>}
                </td>
                <td className={run.by ? undefined : "hl-table__dim"} title={run.by ?? undefined}>
                  {startedBy(run.kind, run.by)}
                </td>
                <td>
                  {run.waiting.map((w) => {
                    // What it shows those who decide it: what they approve.
                    const shown = isApproval(w) ? shownSaid(w) : null;
                    return (
                      <div key={w.name}>
                        {inSteps(w) ? <Chain w={w} /> : isApproval(w) ? approvalSaid(w) : "its event"}
                        {shown && (
                          <div className="hl-flow__shown" aria-label={`what "${w.name}" shows`}>
                            {shown.label}: {shown.text}
                          </div>
                        )}
                      </div>
                    );
                  })}
                </td>
                <td>
                  {run.waiting.map((w) => (
                    <div key={w.name}>{when(w.expires_at)}</div>
                  ))}
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
              <td colSpan={7} className="hl-table__dim">
                reading…
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </section>
  );
}

/** Approve, or decline with a reason, an access change that waits for people, asked first (its
 *  author withdraws it so): what deciding it did is said, and `onDecided` once it is. */
function useDecideChange(me: string, onDecided: () => void) {
  const dialog = useDialog();
  const toast = useToast();
  return async (p: AccessProposal, approve: boolean) => {
    const more = p.needs - p.approvals.length;
    if (approve) {
      const then = more > 1 ? `It then waits for ${more - 1} more ${more - 1 === 1 ? "person" : "people"}.` : "It is made once you do, checked again then.";
      const ok = await dialog.confirm(`Approve access change ${p.id}?\n\n${p.what}, proposed by ${p.author}${p.why ? `: ${p.why}` : ""}. It gives ${p.gives.join("; ")}. ${then}`, "Approve");
      if (!ok) return;
    }
    const reason = approve
      ? ""
      : p.author === me
        ? await dialog.prompt(`Withdraw access change ${p.id}?\n\n${p.what}. Why: it is kept with it.`, "", "Withdraw", { required: "Withdrawing says why: type a reason." })
        : await dialog.prompt(`Decline access change ${p.id}?\n\n${p.what}, proposed by ${p.author}. Why: it is kept with it, for them to read.`, "", "Decline", {
            required: "A decline says why: type a reason.",
          });
    if (reason === null) return;
    try {
      toast(decidedChange(approve ? await approveAccessChange(p.id) : await declineAccessChange(p.id, reason)), "ok");
    } catch (e) {
      toast(e instanceof Error ? e.message : String(e), "bad");
    }
    onDecided();
  };
}

/** The access changes that wait for people (`GET /access/proposals`): what each changes and
 *  gives, who could act as whom it gives more, who proposed it and why, and who approved it so far;
 *  Approve and Decline to whoever the server would let (can.tsx), Withdraw to its author. Then the
 *  last ten decided, and how. */
function AccessChanges({ changes, me, onDecided }: { changes: AccessProposal[] | null; me: Me; onDecided: () => void }) {
  const can = useCan();
  const decide = useDecideChange(me.name, onDecided);
  const waiting = (changes ?? []).filter((p) => p.state === "waiting");
  const ended = (changes ?? []).filter((p) => p.state !== "waiting").slice(0, 10);
  return (
    <section aria-label="access changes">
      <p className="hl-flow__note">
        While this host has people approve an access change (<code>holon access approvals --changes N</code>), a change that gives someone more of run.decide or
        of an admin&apos;s waits here until N people other than whoever proposed it approve it: each holding what it gives, and none who could act as whom it
        gives more. Taking access away applies at once.
      </p>
      {changes === null ? (
        <p className="hl-table__dim">reading…</p>
      ) : (
        waiting.length === 0 && <p className="hl-flow__none">No access change waits.</p>
      )}
      {waiting.length > 0 && (
        <table className="hl-table" aria-label="access changes waiting">
          <thead>
            <tr>
              <th>Change</th>
              <th>Proposed</th>
              <th>Approved</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {waiting.map((p) => {
              const no = notYours(p, me.name);
              const label = `access change ${p.id}`;
              return (
                <tr key={p.id} aria-label={label}>
                  <td>
                    <span className="hl-flow__what">{p.what}</span>
                    <span className="hl-flow__note"> ({label})</span>
                    <div className="hl-flow__said">gives {p.gives.join("; ")}</div>
                    {(p.ways_in ?? []).length > 0 && (
                      <div className="hl-flow__note">who could act as whom it gives more, and so doesn&apos;t approve it: {(p.ways_in ?? []).join("; ")}</div>
                    )}
                  </td>
                  <td>
                    {p.author}, {when(p.at)}
                    {p.why && <div className="hl-flow__note">why: {p.why}</div>}
                  </td>
                  <td>{changeState(p)}</td>
                  <td className="hl-flows__do">
                    <button className="hl-flow__button" onClick={() => void decide(p, false)} aria-label={`${p.author === me.name ? "withdraw" : "decline"} ${label}`} {...can.gate("declineAccess")}>
                      {p.author === me.name ? "Withdraw…" : "Decline…"}
                    </button>
                    {p.author !== me.name && (
                      <button className="hl-flow__button" onClick={() => void decide(p, true)} aria-label={`approve ${label}`} {...can.gate("approveAccess", { disabled: no !== null, title: no ?? undefined })}>
                        Approve…
                      </button>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      {ended.length > 0 && (
        <table className="hl-table" aria-label="access changes decided">
          <thead>
            <tr>
              <th>Decided lately</th>
              <th>Proposed</th>
              <th>How</th>
            </tr>
          </thead>
          <tbody>
            {ended.map((p) => (
              <tr key={p.id}>
                <td>
                  {p.what}
                  <span className="hl-flow__note"> (access change {p.id})</span>
                </td>
                <td>
                  {p.author}, {when(p.at)}
                </td>
                <td>{changeState(p)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
