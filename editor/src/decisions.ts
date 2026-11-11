// An approval a run waits at (`await approval(…)`, durable.rs): who may decide it, what it says
// in Runs waiting, and the decision a resumed run was given, as its log entry holds it.

import type { Me } from "./AuthGate";
import type { WValue } from "./holon";
import { field, text } from "./runs";
import type { WaitingRun } from "./server";

export type WaitingPoint = WaitingRun["waiting"][number];

/** Whether people decide it: an approval, not an event a route delivers. */
export const isApproval = (w: WaitingPoint) => (w.people ?? 0) > 0;

/** Who `me` decides as, as the server says (durable.rs `decider`): a person who is an editor or
 *  an admin, by their name, the OS user's local admin by the name it runs as; null for a viewer,
 *  an agent or a service, who don't decide. */
export function deciderOf(me: Me): string | null {
  if (me.kind !== "person") return null;
  if (me.role !== "editor" && me.role !== "admin") return null;
  return me.name.replace(/^local:/, "");
}

/** What `me` can do about `w`: decide it, nothing more (they approved it), or nothing (an event,
 *  or not theirs to decide). */
export function mayDecide(w: WaitingPoint, me: Me): "decide" | "approved" | "no" {
  const who = deciderOf(me);
  if (!isApproval(w) || who === null) return "no";
  return (w.approved ?? []).some((a) => a.by === who) ? "approved" : "decide";
}

const and = (names: string[]) => (names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`);

/** An approval as Runs waiting says it: how many people it needs, and who approved so far. */
export function approvalSaid(w: WaitingPoint): string {
  const people = w.people ?? 0;
  const by = (w.approved ?? []).map((a) => a.by);
  const needs = people === 1 ? "a person's approval" : `${people} people's approval`;
  return by.length === 0 ? `${needs} "${w.name}"` : `${needs} "${w.name}" · ${by.length} of ${people}: ${and(by)} approved`;
}

/** How many approvals `me` can still decide, over these runs: what the nav adds to its count. */
export const toDecide = (runs: { run: WaitingRun }[], me: Me) => runs.reduce((n, { run }) => n + run.waiting.filter((w) => mayDecide(w, me) === "decide").length, 0);

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
