// An approval a run waits at (`await approval(…)`, durable.rs): which of its steps whoever is
// signed in may take, what it says in Runs waiting, what a decision did, and the decision a
// resumed run was given, as its log entry holds it.

import type { Me } from "./AuthGate";
import type { WValue } from "./holon";
import { field, text } from "./runs";
import type { Decided, WaitingRun } from "./server";

export type WaitingPoint = WaitingRun["waiting"][number];
export type WaitingStep = NonNullable<WaitingPoint["slots"]>[number];

/** Whether it is an approval, decided by its steps, not an event a route delivers. */
export const isApproval = (w: WaitingPoint) => typeof w.steps === "string";

/** Who `me` decides as, as the server says (durable.rs `decider`): their name, the OS user's
 *  local admin by the name it runs as. */
export const deciderOf = (me: Me) => me.name.replace(/^local:/, "");

/** The open steps whoever asked may take now, as the server listed them (`yours`): of their kind,
 *  bound to the step's role where the approval is, and not of a run they started. */
export const yourSteps = (w: WaitingPoint) => (w.slots ?? []).filter((s) => (w.yours ?? []).includes(s.slot));

/** What `me` decided of it, for the row: "you approved as reviewer", "you approved". Null when
 *  they decided no step of it. */
export function yourDecision(w: WaitingPoint, me: Me): string | null {
  const d = (w.decisions ?? []).find((x) => x.who === deciderOf(me));
  if (!d) return null;
  const did = d.approved ? "you approved" : "you declined";
  return d.role ? `${did} as ${d.role}` : did;
}

/** A step as a line names it: by its role, or its number. */
export const stepName = (s: WaitingStep) => s.role ?? `slot ${s.slot}`;

/** Whether it is the short form: one step of people, with no role. */
const alone = (slots: WaitingStep[]): WaitingStep | null => (slots.length === 1 && slots[0].kind === "person" && slots[0].role === undefined ? slots[0] : null);

const filled = (s: WaitingStep) => s.by.length >= s.people;

const and = (names: string[]) => (names.length < 2 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`);

/** An approval as Runs waiting says it: by people alone, how many and who approved so far; in
 *  steps, who decides it and each step as it stands. */
export function approvalSaid(w: WaitingPoint): string {
  const slots = w.slots ?? [];
  const one = alone(slots);
  if (one) {
    const needs = one.people === 1 ? "a person's approval" : `${one.people} people's approval`;
    return one.by.length === 0 ? `${needs} "${w.name}"` : `${needs} "${w.name}" · ${one.by.length} of ${one.people}: ${and(one.by)} approved`;
  }
  const state = (s: WaitingStep) =>
    filled(s) ? `${and(s.by)} approved` : s.by.length > 0 ? `${s.by.length} of ${s.people}: ${and(s.by)} approved` : s.open ? "open" : "not open yet";
  return `approval "${w.name}" (${w.said ?? w.steps}): ${slots.map((s) => `${stepName(s)} ${state(s)}`).join("; ")}`;
}

/** What the approval still waits for once `step` is approved: more people for it, or the steps
 *  left; null when it is then decided. */
export function waitsAfter(w: WaitingPoint, step: WaitingStep): string | null {
  const slots = w.slots ?? [];
  const more = step.people - step.by.length - 1;
  const left = slots.filter((s) => s.slot !== step.slot && !filled(s)).map(stepName);
  if (alone(slots) && more > 0) return `${more} more ${more === 1 ? "person" : "people"} after you`;
  const after = [...(more > 0 ? [`${more} more for ${stepName(step)}`] : []), ...left];
  return after.length === 0 ? null : and(after);
}

/** What a decision did, from the route's answer: as which step, and what the approval waits for
 *  now, or that the run goes on. */
export function decidedSaid(d: Decided): string {
  const taken = d.slots.find((s) => s.slot === d.slot);
  const as = taken?.role ? ` as ${taken.role}` : "";
  if (d.outcome === "declined") return `Declined${as}: the run goes on as declined.`;
  if (d.outcome === "approved") return `Approved by ${and(d.slots.flatMap((s) => s.by))}: the run goes on.`;
  const one = alone(d.slots);
  if (one) {
    const more = one.people - one.by.length;
    return `Approved: ${one.by.length} of ${one.people}. It waits for ${more} more ${more === 1 ? "person" : "people"}.`;
  }
  const left = d.slots.filter((s) => !filled(s)).map((s) => (s.people > 1 ? `${stepName(s)} (${s.by.length} of ${s.people})` : stepName(s)));
  return `Approved${as}. It waits for ${and(left)}.`;
}

/** How many approvals whoever is signed in can still decide, over these runs: what the nav adds
 *  to its count. */
export const toDecide = (runs: { slug: string; run: WaitingRun }[]) => runs.reduce((n, { run }) => n + run.waiting.filter((w) => yourSteps(w).length > 0).length, 0);

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
