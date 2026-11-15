// An approval a run waits at (`await approval(…)`, durable.rs): which of its steps whoever is
// signed in may take, what it says in Runs waiting (in words, or as its chain of steps), what a
// decision did, and the decision a resumed run was given, as its log entry holds it, with each
// step's decision a line on the run's timeline.

import type { Me } from "./AuthGate";
import type { WValue } from "./holon";
import { field, text } from "./runs";
import type { Decided, WaitingDecision, WaitingRun } from "./server";

export type WaitingPoint = WaitingRun["waiting"][number];
export type WaitingStep = NonNullable<WaitingPoint["slots"]>[number];

/** Whether it is an approval, decided by its steps, not an event a route delivers. */
export const isApproval = (w: WaitingPoint) => typeof w.steps === "string";

/** The open steps whoever asked may take now, as the server listed them (`yours`): of their kind,
 *  bound to the step's role where the approval is, and not of a run they started. */
export const yourSteps = (w: WaitingPoint) => (w.slots ?? []).filter((s) => (w.yours ?? []).includes(s.slot));

/** What `me` decided of it, for the row: "you approved as reviewer", "you approved". Null when
 *  they decided no step of it. A decision names its principal as `me` does (durable.rs `decider`). */
export function yourDecision(w: WaitingPoint, me: Me): string | null {
  const d = (w.decisions ?? []).find((x) => x.who === me.name);
  if (!d) return null;
  const did = d.approved ? "you approved" : "you declined";
  return d.role ? `${did} as ${d.role}` : did;
}

/** A step as a line names it: by its role, or its number. */
export const stepName = (s: WaitingStep) => s.role ?? `slot ${s.slot}`;

/** Whether it is the short form: one step of people, with no role, which whoever started the run
 *  doesn't take. */
const alone = (slots: WaitingStep[]): WaitingStep | null => (slots.length === 1 && slots[0].kind === "person" && slots[0].role === undefined && !slots[0].starter ? slots[0] : null);

/** Whether an approval is in steps (roles, kinds, an order), which Runs waiting draws as a chain;
 *  the short form, people alone, it says in words (`approvalSaid`). */
export const inSteps = (w: WaitingPoint) => isApproval(w) && alone(w.slots ?? []) === null;

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

/** What an approval shows those who decide it, in words: what its flow named (`shows`), or else
 *  what its run was started with; a record of plain values as `amount 30 · payee acme`, anything
 *  else as JSON. Null when it shows nothing. */
export function shownSaid(w: WaitingPoint): { label: string; text: string } | null {
  const named = w.shows !== undefined;
  const value = named ? w.shows : w.input;
  if (value === undefined) return null;
  const plain = (v: unknown) => v === null || ["string", "number", "boolean"].includes(typeof v);
  const one = (v: unknown) => (typeof v === "string" ? v : JSON.stringify(v));
  const record = typeof value === "object" && value !== null && !Array.isArray(value) && Object.keys(value).length > 0 && Object.values(value).every(plain);
  const text = record ? Object.entries(value as Record<string, unknown>).map(([k, v]) => `${k} ${one(v)}`).join(" · ") : one(value);
  return { label: named ? "shows" : "started with", text };
}

/** One step of an approval in its chain, as it stands: taken (`done`), `open`, or not open yet
 *  (`later`); in words, who took it or how far it is (`detail`); and the decisions taken on it. */
export type ChainStep = { slot: number; name: string; state: "done" | "open" | "later"; detail: string | null; decisions: WaitingDecision[] };

/** What a note says in a word or two: its `label`, its `confidence` (an agent's classification),
 *  as plain values; null for any other note, which a decision's line shows whole. */
function noteWords(note: unknown): string | null {
  if (typeof note !== "object" || note === null || Array.isArray(note)) return null;
  const { label, confidence } = note as Record<string, unknown>;
  const words = [typeof label === "string" ? label : null, typeof confidence === "number" ? String(confidence) : null].filter((x) => x !== null);
  return words.length > 0 ? words.join(", ") : null;
}

/** An approval's steps in their order, each as it stands: who took a step (with what their note
 *  says in a word or two), how many of its people approved so far, or that it isn't open yet; and
 *  one whoever started the run may take, until it is taken. */
export function chainOf(w: WaitingPoint): ChainStep[] {
  return (w.slots ?? []).map((s): ChainStep => {
    const decisions = (w.decisions ?? []).filter((d) => d.slot === s.slot);
    const took = and(
      s.by.map((who) => {
        const words = noteWords(decisions.find((d) => d.who === who)?.note);
        return words ? `${who}, ${words}` : who;
      }),
    );
    const many = s.people > 1 ? `${s.people} people` : null;
    const starter = s.starter ? "whoever started it, or one acting for them, may take it" : null;
    const words = (...parts: (string | null)[]) => parts.filter((x) => x !== null).join(", ") || null;
    if (filled(s)) return { slot: s.slot, name: stepName(s), state: "done", detail: took, decisions };
    if (s.open) return { slot: s.slot, name: stepName(s), state: "open", detail: s.by.length > 0 ? words(`${s.by.length} of ${s.people}: ${took}`, starter) : words(many, starter), decisions };
    return { slot: s.slot, name: stepName(s), state: "later", detail: words(many, starter, "not open yet"), decisions };
  });
}

/** An approval's chain in a line: `✓ classifier (scout, invoice, 0.93) → ◻ reviewer → ◻ approver
 *  (not open yet)`, a step taken ✓, any other ◻. */
export const chainSaid = (w: WaitingPoint) => chainOf(w).map((c) => `${c.state === "done" ? "✓" : "◻"} ${c.name}${c.detail ? ` (${c.detail})` : ""}`).join(" → ");

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

/** One decision in a line: when, who (an agent says so), what they decided as which step, why,
 *  what they noted, and the binding that let them, with the access document's version it was
 *  read in, which `holon access show --version N` prints for an auditor. */
export function decidedLine(d: WaitingDecision, when: (seconds: number) => string): string {
  const who = d.kind === "agent" ? `${d.who} (an agent)` : d.who;
  const as = d.role ? ` as ${d.role}` : d.slot > 0 ? ` as slot ${d.slot}` : "";
  const why = d.reason ? `: ${d.reason}` : "";
  const noted = d.note === undefined ? "" : ` · noted ${JSON.stringify(d.note)}`;
  const through = d.binding === undefined ? "" : ` · through binding ${d.binding}${d.access === undefined ? "" : ` (holon access show --version ${d.access})`}`;
  return `${when(d.at)}: ${who} ${d.approved ? "approved" : "declined"}${as}${why}${noted}${through}`;
}

/** A logged value as plain JSON: a note, as its decider gave it. */
function plain(v: WValue): unknown {
  switch (v.type) {
    case "Record":
      return Object.fromEntries(v.value.map(([k, x]) => [k, plain(x)]));
    case "List":
      return v.value.map(plain);
    case "Sum":
      return v.value.arm === 0 ? plain(v.value.payload) : null;
    case "Effect":
    case "Bytes":
      return v.type;
    default:
      return v.value;
  }
}

/** The decisions an approval's event holds (its `decisions`, a step's each), as Runs waiting
 *  lists them; none for a value that isn't one. */
export function decisionsOf(v: WValue | null | undefined): WaitingDecision[] {
  const list = field(v, "decisions");
  if (list?.type !== "List") return [];
  return list.value.flatMap((d): WaitingDecision[] => {
    const number = (k: string) => {
      const x = field(d, k);
      return x?.type === "Number" ? x.value : undefined;
    };
    const approved = field(d, "approved");
    const who = text(field(d, "who"));
    if (approved?.type !== "Boolean" || who === "") return [];
    const note = field(d, "note");
    return [
      {
        slot: number("slot") ?? 0,
        who,
        kind: text(field(d, "kind")) === "agent" ? "agent" : "person",
        ...(text(field(d, "role")) ? { role: text(field(d, "role")) } : {}),
        at: number("at") ?? 0,
        approved: approved.value,
        ...(text(field(d, "reason")) ? { reason: text(field(d, "reason")) } : {}),
        ...(note ? { note: plain(note) } : {}),
        ...(number("binding") !== undefined ? { binding: number("binding") } : {}),
        ...(number("access") !== undefined ? { access: number("access") } : {}),
      },
    ];
  });
}
