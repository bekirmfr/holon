// What a run in a deployment's log is, said briefly: its label, how it ended, and what it did
// outside the flow. Shared by view mode's log and a flow's page (FlowPage.tsx).

import type { WValue } from "./holon";
import type { LogEntry } from "./server";

/** A record's field, or null. */
export const field = (v: WValue | null | undefined, k: string): WValue | null =>
  v?.type === "Record" ? (v.value.find(([key]) => key === k)?.[1] ?? null) : null;
export const text = (v: WValue | null) => (v?.type === "Text" ? v.value : v?.type === "Number" ? String(v.value) : "");

/** A file run's label: what happened to which file (`deleted file a.csv`). */
const fileRun = (handle: WValue | undefined) => `${text(field(handle, "event")) || "new"} file ${text(field(handle, "name")) || ""}`.trim();

/** What a run answered: its route, its clock, its file, its action. */
export const runLabel = (e: LogEntry) =>
  e.kind === "request"
    ? (e.route ?? `${e.method ?? ""} ${e.url ?? ""} (no route)`)
    : e.kind === "resume"
      ? `resumed ${e.route ?? ""}`.trim()
      : e.kind === "tick"
        ? (e.route ?? "clock tick")
        : e.kind === "file"
          ? `${fileRun(e.sourced.find(([cap]) => cap.startsWith("FileArrival("))?.[1])}${e.route ? ` (${e.route})` : ""}`
          : (e.route ?? "trigger");

/** Compact capability badges derived from a run's log entry. */
export function runActions(e: LogEntry): { label: string; title: string; color: string }[] {
  const acts: { label: string; title: string; color: string }[] = [];
  if (e.sourced.some(([c]) => c === "Random")) acts.push({ label: "rng", title: "random value read", color: "var(--faint)" });
  if ((e.rounds ?? []).some((r) => r.exchanges.length > 0)) acts.push({ label: "http", title: "outbound http request", color: "var(--ok)" });
  if (Object.keys(e.secrets ?? {}).length > 0) acts.push({ label: "sec", title: "secret read", color: "var(--warn)" });
  if ((e.reads ?? []).some((r) => r.file != null)) acts.push({ label: "frd", title: "file read", color: "var(--muted)" });
  if (e.performed.some((p) => /^file/i.test(p.capability))) acts.push({ label: "fwr", title: "file write", color: "var(--warn)" });
  return acts;
}

/** How a run ended: answered, failed (an error, or a 5xx it answered), or waiting for an
 *  approval or an event at a waiting point (a durable run's part). */
export type Outcome = "ok" | "failed" | "waiting";
export function runOutcome(e: LogEntry): Outcome {
  if (e.error !== null || (e.status ?? 0) >= 500) return "failed";
  if ((e.parked ?? []).length > 0) return "waiting";
  return "ok";
}

/** What a run did outside the flow: the requests it sent (↗) and the effects it performed (✎:
 *  writes, messages, files). */
export function outside(e: LogEntry): { sent: number; did: number } {
  return { sent: (e.rounds ?? []).reduce((n, r) => n + r.exchanges.length, 0), did: e.performed.length };
}
