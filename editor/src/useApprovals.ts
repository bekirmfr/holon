// What waits for a person (approvals.ts), kept fresh: the saved flows and the proposals read every 4 s while
// Approvals or the overview is shown (every 30 s otherwise, for the nav's count), each
// deployment's events (for an agent's asks) while Approvals is, the runs waiting in the deployments
// that have any (Runs waiting lists them; the nav counts the approvals among them) with whether
// whoever is signed in decides each approval point among them, and the versions of a deployment
// that has a newer save, so its review has a number.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { approvalsOf, keptKey, reviewQueue } from "./approvals";
import { isApproval, pointKey } from "./decisions";
import { getEvents, getVersions, listFlows, listProposals, listWaitingRuns, mayI, type DeploymentEntry, type DeploymentEvent, type FlowListEntry, type Proposal, type WaitingRun } from "./server";

/** A run waiting in a deployment. */
export type WaitingRunOf = { slug: string; run: WaitingRun };

const KEPT = "holon.approvals.kept";

/** The versions this browser kept saved without deploying (keptKey). */
function readKept(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(KEPT) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}

export function useApprovals(deployments: DeploymentEntry[], { shown, asks: readAsks }: { shown: boolean; asks: boolean }) {
  const [flows, setFlows] = useState<FlowListEntry[]>([]);
  const [proposals, setProposals] = useState<Proposal[]>([]);
  const [versions, setVersions] = useState<Map<string, number>>(new Map());
  const [asks, setAsks] = useState<Map<string, DeploymentEvent[]>>(new Map());
  const [kept, setKept] = useState<Set<string>>(readKept);
  const [runs, setRuns] = useState<WaitingRunOf[] | null>(null);
  // Whether whoever is signed in decides each approval point a run waits at (pointKey).
  const [decides, setDecides] = useState<ReadonlyMap<string, boolean>>(new Map());
  const now = useRef(deployments);
  now.current = deployments;

  const readRuns = useCallback(() => {
    const waiting = now.current.filter((d) => (d.waiting ?? 0) > 0);
    Promise.all(waiting.map((d) => listWaitingRuns(d.slug).then((rs) => rs.map((run) => ({ slug: d.slug, run })), () => []))).then(async (all) => {
      const runs = all.flat();
      // The server says who decides each point: bound on the host, on the flow, or at the point.
      const points = [...new Map(runs.flatMap(({ slug, run }) => run.waiting.filter(isApproval).map((w) => [pointKey(slug, w.name), { slug, name: w.name }] as const))).entries()];
      const asked = points.map(([key, { slug, name }]) =>
        mayI(["run.decide"], { flow: slug, approval: name }).then(
          (may) => [key, may["run.decide"] === true] as const,
          () => [key, false] as const,
        ),
      );
      setDecides(new Map(await Promise.all(asked)));
      setRuns(runs);
    });
  }, []);
  const refresh = useCallback(() => {
    listFlows()
      .then(setFlows)
      .catch(() => undefined);
    listProposals()
      .then(setProposals)
      .catch(() => undefined);
    readRuns();
  }, [readRuns]);
  // Read again as soon as a deployment's count of waiting runs changes.
  const waitingKey = deployments.map((d) => `${d.slug}:${d.waiting ?? 0}`).join(",");
  useEffect(readRuns, [readRuns, waitingKey]);
  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, shown ? 4000 : 30000);
    return () => clearInterval(timer);
  }, [refresh, shown]);

  useEffect(() => {
    if (!readAsks) return;
    let fresh = true;
    const read = () =>
      Promise.all(now.current.map((d) => getEvents(d.slug).then((r) => [d.slug, r.events] as const, () => null))).then((got) => {
        if (fresh) setAsks(new Map(got.filter((g): g is readonly [string, DeploymentEvent[]] => g !== null)));
      });
    void read();
    const timer = setInterval(read, 4000);
    return () => {
      fresh = false;
      clearInterval(timer);
    };
  }, [readAsks]);

  // A deployment with a newer save: its versions, read again when another is deployed.
  const saved = useMemo(() => new Map(flows.map((f) => [f.name, f.saved_at_epoch_seconds])), [flows]);
  const read = useRef(new Map<string, string>());
  useEffect(() => {
    for (const d of deployments) {
      if ((saved.get(d.slug) ?? 0) <= d.deployed_at_epoch_seconds || read.current.get(d.slug) === d.unit) continue;
      read.current.set(d.slug, d.unit);
      getVersions(d.slug).then(
        (vs) => setVersions((m) => new Map(m).set(d.slug, Math.max(0, ...vs.map((v) => v.number)))),
        () => read.current.delete(d.slug),
      );
    }
  }, [deployments, saved]);

  const approvals = useMemo(() => approvalsOf(deployments, flows, { versions, asks, kept, proposals }), [deployments, flows, versions, asks, kept, proposals]);
  const queue = useMemo(() => reviewQueue(approvals), [approvals]);

  /** Keep a version saved without deploying it: it leaves the queue until it is saved again. */
  const keep = useCallback((slug: string, savedAt: number) => {
    setKept((k) => {
      const next = new Set(k).add(keptKey(slug, savedAt));
      try {
        localStorage.setItem(KEPT, JSON.stringify([...next]));
      } catch {
        // Kept for this visit only.
      }
      return next;
    });
  }, []);

  return { approvals, queue, flows, proposals, runs, decides, keep, refresh };
}
