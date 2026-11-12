// An approval a run waits at (`await approval(…)`, durable.rs): who may decide it, what it says
// in Runs waiting, and the decision a resumed run was given, as its log entry holds it.

import type { Me } from "./AuthGate";
import type { WValue } from "./holon";
import { field, text } from "./runs";
import type { WaitingRun } from "./server";

export type WaitingPoint = WaitingRun["waiting"][number];

/** Whether people decide it: an approval, not an event a route delivers. */
export const isApproval = (w: WaitingPoint) => (w.people ?? 0) > 0;

/** Who approved it so far, in the order they did: its decisions that are approvals. */
export const approvalsOf = (w: WaitingPoint) => (w.decisions ?? []).filter((d) => d.approved);

/** Who `me` decides as, as the server says (durable.rs `decider`): their name, the OS user's
 *  local admin by the name it runs as. */
export const deciderOf = (me: Me) => me.name.replace(/^local:/, "");

/** Where whether `me` decides an approval point is kept: its flow's slug and its name. */
export const pointKey = (slug: string, name: string) => `${slug}\n${name}`;

/** What `me` can do about `w`: decide it, nothing more (they approved it), or nothing (an event,
 *  or not theirs to decide: `may` is what the server's `can` answered for `run.decide` there, a
 *  person bound a role allowing it on the host, on the flow, or at that approval point). */
export function mayDecide(w: WaitingPoint, me: Me, may: boolean): "decide" | "approved" | "no" {
  if (!isApproval(w) || !may) return "no";
  return approvalsOf(w).some((a) => a.who === deciderOf(me)) ? "approved" : "decide";
}

const and = (names: string[]) => (names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`);

/** An approval as Runs waiting says it: how many people it needs, and who approved so far. */
export function approvalSaid(w: WaitingPoint): string {
  const people = w.people ?? 0;
  const by = approvalsOf(w).map((a) => a.who);
  const needs = people === 1 ? "a person's approval" : `${people} people's approval`;
  return by.length === 0 ? `${needs} "${w.name}"` : `${needs} "${w.name}" · ${by.length} of ${people}: ${and(by)} approved`;
}

/** How many approvals `me` can still decide, over these runs, `decides` saying which points are
 *  theirs (`pointKey`): what the nav adds to its count. */
export const toDecide = (runs: { slug: string; run: WaitingRun }[], me: Me, decides: ReadonlyMap<string, boolean>) =>
  runs.reduce((n, { slug, run }) => n + run.waiting.filter((w) => mayDecide(w, me, decides.get(pointKey(slug, w.name)) === true) === "decide").length, 0);

/** The decision an approval's event is, said: who approved, who declined and why, or that it
 *  ran out. Null for a value that isn't one (an event's request). */
export function decisionSaid(v: WValue | null | undefined): string | null {
  const approved = field(v, "approved");
  const by = field(v, "by");
  if (approved?.type !== "Boolean" || by?.type !== "List") return null;
  const names = by.value.map((b) => text(b));
  if (approved.value) return `approved by ${and(names)}`;
  const declined = text(field(v, "declined_by"));
  if (declined) return `declined by ${declined}: ${text(field(v, "reason"))}`;
  const expired = field(v, "expired");
  const before = names.length > 0 ? `, approved by ${and(names)} before` : "";
  return expired?.type === "Boolean" && expired.value ? `ran out undecided${before}` : `not approved${before}`;
}
