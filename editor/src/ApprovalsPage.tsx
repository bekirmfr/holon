// The Approvals page (gaps-web.md G8, C3; web-console-L4.md §7.2): what waits for a person.
// Deploys: a card for each version proposed, with its place in the queue the Review page goes
// through, who proposed it and why, and the rows of what it may do that change (the Review's,
// reviewMay.ts, read when the card comes into view); then as rows what else a deployment waits on
// a person for. Decline (with a reason its author reads back, proposals.rs). Runs waiting: runs
// parked across flows, what each is and until when. An approval (`await approval(…)`) is decided
// here, Approve or Decline with a reason, by people signed in (durable.rs); an event goes on
// through its flow's own route. `a` and `d` approve and decline the one selected.

import { useEffect, useState } from "react";
import { reviewQueue, routeOf, said, versionsSaid, type Approval, type Reviewable } from "./approvals";
import type { Me } from "./AuthGate";
import { isApproval, mayDecide, pointKey, waitsFor, type WaitingPoint } from "./decisions";
import { useDialog } from "./Dialog";
import { cardRows, mayGroups, textLines, type CardRows } from "./reviewMay";
import { follow, format, type Route } from "./routes";
import { decideRun, declineProposal, getProposal, previewDeploy, type DeploymentEntry, type FlowDocument, type Proposal } from "./server";
import { useKeys, useListKeys } from "./ui/Keys";
import { useToast } from "./ui/Toast";
import type { WaitingRunOf } from "./useApprovals";
import { askCan, gateOf, useCan } from "./useCan";

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
  textOf,
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
  /** A document as its text, for the lines a card's rows point at. Throws when it can't be written. */
  textOf: (slug: string, doc: FlowDocument) => string;
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
        <Deploys approvals={approvals} navigate={navigate} textOf={textOf} onDeclined={onDeclined} />
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
      void askCan(["flow.approve-deploy"], { flow: p.slug }).then(
        (may) => {
          const answer = may["flow.approve-deploy"] ?? true;
          if (answer === true) void decline(p);
          else toast(answer, "bad");
        },
        () => void decline(p),
      );
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
  // a and d: the selected run's first approval `me` may decide, asked first (the question is its
  // review); else why not.
  const decideSelected = (approve: boolean) => {
    const r = (runs ?? []).find(({ slug, run }) => waitingKey(slug, run.run_id) === list.at);
    if (!r) return toast("Select a run first: j and k.", "info");
    const points = r.run.waiting.filter(isApproval);
    const mine = points.find((w) => mayDecide(w, me, decides.get(pointKey(r.slug, w.name)) === true) === "decide");
    if (mine) return void decide(r.slug, r.run.run_id, mine, approve);
    if (points.length === 0) return toast("It waits for an event, which its flow's own route delivers: nobody approves it here.", "info");
    if (points.some((w) => mayDecide(w, me, decides.get(pointKey(r.slug, w.name)) === true) === "approved")) return toast("You approved it: it waits for others.", "info");
    const refused = decides.get(pointKey(r.slug, points[0].name));
    toast(typeof refused === "string" ? refused : "It isn't yours to decide.", "bad");
  };
  useKeys("approvals", { a: () => decideSelected(true), d: () => decideSelected(false) });
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
            <th>What it is</th>
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
                  <a href={format(to)} onClick={follow(to, navigate)}>
                    {slug}
                  </a>
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
                <td>
                  {run.waiting.map((w) => (
                    <div key={w.name}>{waitsFor(w)}</div>
                  ))}
                </td>
                <td>
                  {run.waiting.map((w) => (
                    <div key={w.name}>{when(w.expires_at)}</div>
                  ))}
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
              <td colSpan={6} className="hl-table__dim">
                reading…
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </section>
  );
}
