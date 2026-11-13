// What waits for a person (gaps-web.md G8), as the workbench and `holon mcp` work it out
// (workbench/client.rs `waiting`, mcp.rs `waits_for`): deployment by deployment, a version saved
// since it was deployed, why it can't run until deployed again, a port it can't listen on, and an
// agent's ask to restore; then each saved flow never deployed. A version an agent proposed
// (the server's proposal records, proposals.rs) carries its proposal, its why and note; one a
// person declined leaves the list until it is saved again. A version to deploy is reviewed on its
// own page (`#approvals.<slug>.v<n>`), in a queue: the Deploys tab's order.

import type { DeploymentEntry, DeploymentEvent, FlowListEntry, Proposal } from "./server";
import type { Route } from "./routes";

export type Approval =
  /** A saved flow never deployed: its first deploy, version 1. */
  | { kind: "first"; slug: string; by: string | null; at: number; version: number; proposal?: Proposal }
  /** A version saved since the deployed one: `version` is the number it would be (null until
   *  its versions are read). */
  | { kind: "update"; slug: string; by: string | null; at: number; version: number | null; proposal?: Proposal }
  /** It can't run until deployed again (its grant no longer covers what it needs). */
  | { kind: "attention"; slug: string; why: string }
  /** It can't listen on its port. */
  | { kind: "port"; slug: string; why: string }
  /** An agent's restore, saved for a person to approve (an `ask` event); `at` in seconds. */
  | { kind: "ask"; slug: string; what: string; by: string | null; at: number; from: string };

/** A version that waits to be deployed: what the Review page reviews. */
export type Reviewable = Extract<Approval, { kind: "first" | "update" }>;

/** The key a version kept saved without deploying is remembered by: a newer save asks again. */
export const keptKey = (slug: string, savedAt: number) => `${slug}@${savedAt}`;

/** An agent's asks not yet answered: those no restore from the same point came after. */
export function pendingAsks(events: DeploymentEvent[]): DeploymentEvent[] {
  return events.filter((e) => e.kind === "ask" && e.from && !events.some((r) => r.kind === "restore" && r.seq > e.seq && r.from === e.from));
}

/** Whether the save stamped `savedAt` of `slug` was proposed and declined: it leaves the list
 *  until it is saved again. */
const declinedSave = (proposals: Proposal[], slug: string, savedAt: number) =>
  proposals.some((p) => p.slug === slug && p.state === "declined" && p.saved_at === savedAt);

/** Everything that waits for a person, in the workbench's order. `versions`: each deployment's
 *  highest version number, where read; `asks`: each deployment's events, where read; `kept`:
 *  the versions kept saved without deploying (keptKey); `proposals`: the server's records. */
export function approvalsOf(
  deployments: DeploymentEntry[],
  flows: FlowListEntry[],
  {
    versions = new Map(),
    asks = new Map(),
    kept = new Set(),
    proposals = [],
  }: { versions?: Map<string, number>; asks?: Map<string, DeploymentEvent[]>; kept?: Set<string>; proposals?: Proposal[] } = {},
): Approval[] {
  const out: Approval[] = [];
  const saved = new Map(flows.map((f) => [f.name, f]));
  const waiting = new Map(proposals.filter((p) => p.state === "waiting").map((p) => [p.slug, p]));
  const left = (slug: string, at: number) => kept.has(keptKey(slug, at)) || declinedSave(proposals, slug, at);
  for (const d of deployments) {
    const slug = d.slug;
    if (d.attention) out.push({ kind: "attention", slug, why: d.attention });
    if (d.listen_error) out.push({ kind: "port", slug, why: d.listen_error });
    const f = saved.get(slug);
    const proposal = waiting.get(slug);
    if (proposal) {
      // Its own record says the number it would be.
      out.push({ kind: "update", slug, by: proposal.by ?? null, at: Math.floor(proposal.at_ms / 1000), version: proposal.version ?? null, proposal });
    } else if (f && f.saved_at_epoch_seconds > d.deployed_at_epoch_seconds && !left(slug, f.saved_at_epoch_seconds)) {
      const now = versions.get(slug);
      out.push({ kind: "update", slug, by: f.saved_by ?? null, at: f.saved_at_epoch_seconds, version: now === undefined ? null : now + 1 });
    }
    for (const e of pendingAsks(asks.get(slug) ?? [])) {
      out.push({ kind: "ask", slug, what: e.what, by: e.by ?? null, at: Math.floor(e.at_ms / 1000), from: e.from! });
    }
  }
  const deployed = new Set(deployments.map((d) => d.slug));
  for (const f of flows) {
    if (deployed.has(f.name)) continue;
    const proposal = waiting.get(f.name);
    if (proposal) {
      out.push({ kind: "first", slug: f.name, by: proposal.by ?? null, at: Math.floor(proposal.at_ms / 1000), version: 1, proposal });
      continue;
    }
    if (left(f.name, f.saved_at_epoch_seconds)) continue;
    out.push({ kind: "first", slug: f.name, by: f.saved_by ?? null, at: f.saved_at_epoch_seconds, version: 1 });
  }
  return out;
}

/** The versions to review, in order: the Review page's queue. */
export function reviewQueue(approvals: Approval[]): Reviewable[] {
  return approvals.filter((a): a is Reviewable => a.kind === "first" || a.kind === "update");
}

/** Where one waits to be dealt with: a version's review (once its number is known), an ask on
 *  its flow's history (where a person restores), the rest on its flow's settings. */
export function routeOf(a: Approval): Route | null {
  switch (a.kind) {
    case "first":
    case "update":
      return a.version === null ? null : { kind: "review", slug: a.slug, version: a.version };
    case "ask":
      return { kind: "flow", slug: a.slug, tab: "history" };
    case "attention":
    case "port":
      return { kind: "flow", slug: a.slug, tab: "status" };
  }
}

/** The versions a review goes from and to: "v3 → v4", or "new, v1" for a first deploy. */
export function versionsSaid(a: Reviewable): string {
  if (a.version === null) return "a new version";
  if (a.kind === "first") return "new, v1";
  return a.version > 1 ? `v${a.version - 1} → v${a.version}` : `v${a.version}`;
}

/** One in words, as `holon mcp` says it. */
export function said(a: Approval, when: (seconds: number) => string): string {
  switch (a.kind) {
    case "first":
      if (a.proposal) return `its first deploy, proposed by ${a.by ?? "someone"} at ${when(a.at)}${a.proposal.why ? `: ${a.proposal.why}` : ""}`;
      return `to deploy it for the first time: saved by ${a.by ?? "someone"}, at ${when(a.at)}`;
    case "update":
      if (a.proposal) return `a new version, proposed by ${a.by ?? "someone"} at ${when(a.at)}${a.proposal.why ? `: ${a.proposal.why}` : ""}`;
      return `to review a version saved since by ${a.by ?? "someone"}, at ${when(a.at)}, and deploy it`;
    case "attention":
      return `to deploy it again: ${a.why}`;
    case "port":
      return `to free its port: ${a.why}`;
    case "ask":
      return `${a.what}${a.by ? ` (${a.by})` : ""}: restore it from its history`;
  }
}
