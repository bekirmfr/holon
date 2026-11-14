// What a run in a deployment's log is, said briefly: its label, how it ended, and what it did
// outside the flow. Shared by view mode's log and a flow's page (FlowPage.tsx).

import type { WValue } from "./holon";
import type { LogEntry } from "./server";
import { shortIds } from "./shortIds";

/** A record's field, or null. */
export const field = (v: WValue | null | undefined, k: string): WValue | null =>
  v?.type === "Record" ? (v.value.find(([key]) => key === k)?.[1] ?? null) : null;
export const text = (v: WValue | null) => (v?.type === "Text" ? v.value : v?.type === "Number" ? String(v.value) : "");

/** A file run's label: what happened to which file (`deleted file a.csv`). */
const fileRun = (handle: WValue | undefined) => `${text(field(handle, "event")) || "new"} file ${text(field(handle, "name")) || ""}`.trim();

/** A mail run's label: who it came from, and its subject (`mail from billing@customer.example: Remittance May`). */
const mailRun = (message: WValue | undefined) => `mail from ${text(field(message, "from")) || text(field(message, "name")) || "?"}${text(field(message, "subject")) ? `: ${text(field(message, "subject"))}` : ""}`;
/** A feed run's label: which feed, and its event's name (`event from prices: message`). */
const feedRun = (sourced: [string, WValue] | undefined) => {
  const feed = sourced ? (/^FeedEvent\("(.*)"\)$/.exec(sourced[0])?.[1] ?? "?") : "?";
  const name = text(field(sourced?.[1], "event"));
  return `event from ${feed}${name ? `: ${name}` : ""}`;
};
/** A queue run's label: which queue, and its message's id (`message from payout_events: 5fb2…`). */
const queueRun = (sourced: [string, WValue] | undefined) => {
  const queue = sourced ? (/^QueueMessage\("(.*)"\)$/.exec(sourced[0])?.[1] ?? "?") : "?";
  const id = text(field(sourced?.[1], "id"));
  return `message from ${queue}${id ? `: ${id}` : ""}`;
};

/** What a run answered: its route, its clock, its file, its mail, feed or queue message, its action. */
export const runLabel = (e: LogEntry) =>
  e.kind === "request"
    ? (e.route ?? `${e.method ?? ""} ${e.url ?? ""} (no route)`)
    : e.kind === "resume"
      ? `resumed ${e.route ?? ""}`.trim()
      : e.kind === "tick"
        ? (e.route ?? "clock tick")
        : e.kind === "file"
          ? `${fileRun(e.sourced.find(([cap]) => cap.startsWith("FileArrival("))?.[1])}${e.route ? ` (${e.route})` : ""}`
          : e.kind === "mail"
            ? `${mailRun(e.sourced.find(([cap]) => cap.startsWith("MailArrival("))?.[1])}${e.route ? ` (${e.route})` : ""}`
            : e.kind === "feed"
              ? `${feedRun(e.sourced.find(([cap]) => cap.startsWith("FeedEvent(")))}${e.route ? ` (${e.route})` : ""}`
              : e.kind === "queue"
                ? `${queueRun(e.sourced.find(([cap]) => cap.startsWith("QueueMessage(")))}${e.route ? ` (${e.route})` : ""}`
                : (e.route ?? "trigger");

/** Who or what started a run, from its kind and its `by` (a waiting run's, or a log entry's):
 *  the person or agent who ran its action or sent its request through the server, else what does
 *  on its own (its clock, its port, a file, a mail, a feed, a queue). A run another flow's run
 *  started names that run by its first eight: the whole `by` is for a title. */
export function startedBy(kind: string, by: string | null | undefined): string {
  if (by) return shortIds(by);
  switch (kind) {
    case "request":
      return "a request on its port";
    case "tick":
      return "its clock";
    case "file":
      return "a file in its folder";
    case "mail":
      return "a mail";
    case "feed":
      return "a feed's event";
    case "queue":
      return "a queue's message";
    case "resume":
      return "what it waited for";
    default:
      return "its action (who ran it isn't on record)";
  }
}

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

/** What a run's claims say (key-claims-L4.md §5), each as `holon explain` says it: a key it held
 *  until it was done and for how long, one it waited for and found changed (it ran again), or one
 *  held by another run it waited for, turned away as busy or not. */
export function claimNotes(e: LogEntry): { said: string; tone: "ok" | "again" | "busy" }[] {
  const busy = e.error?.startsWith("busy:") ?? false;
  const ms = (n: number) => `${Math.round(n)} ms`;
  let retry = 0;
  return (e.claims ?? []).map((c) => {
    if (c.overtaken) {
      retry++;
      return {
        said: `claimed ${c.key} before calling out: waited ${ms(c.waited_ms)} for another run, then found what it read had changed and ran again from there (retry ${retry} of ${e.retries ?? retry}), having sent nothing`,
        tone: "again",
      };
    }
    if (c.held_ms !== undefined) return { said: `claimed ${c.key} before calling out: waited ${ms(c.waited_ms)}, held it ${ms(c.held_ms)} until it was done`, tone: "ok" };
    return busy
      ? { said: `waited ${ms(c.waited_ms)} for ${c.key}, held by another run, and was turned away as busy`, tone: "busy" }
      : { said: `waited ${ms(c.waited_ms)} for ${c.key}, held by another run`, tone: "again" };
  });
}
