// A deployment's timeline (docs/L4s/history-L4.md §2, §6): its runs, the versions it was deployed
// with, and what was done to it (events.rs), merged as `holon history` merges them; the points it
// can go back to, and what a restore's dry run says, worded as `holon restore` words it.

import { describeItem } from "./holon";
import type { DeploymentEvent, LogEntry, RestoreDone, RestorePoint, RestorePreview, StateChange, VersionEntry } from "./server";

export type TimelineItem =
  | { kind: "run"; key: string; at_ms: number; run: LogEntry }
  /** Something done to it; a deploy names its version. */
  | { kind: "event"; key: string; at_ms: number; event: DeploymentEvent; version?: VersionEntry }
  /** A version no event names: deployed before events were kept. */
  | { kind: "version"; key: string; at_ms: number; version: VersionEntry };

/** Runs, events and versions, newest first. An event comes before the run it names (its
 *  `run_seq`); a version deployed before events were kept goes where its first run would. */
export function buildTimeline(runs: LogEntry[], events: DeploymentEvent[], versions: VersionEntry[]): TimelineItem[] {
  const byNumber = new Map(versions.map((v) => [v.number, v]));
  const named = new Set(events.filter((e) => e.kind === "deploy" && e.version !== undefined).map((e) => e.version));
  const sorted = [...runs].sort((a, b) => a.seq - b.seq);
  const next = sorted.length > 0 ? sorted[sorted.length - 1].seq + 1 : 0;
  // Where a version without its log seq starts: the first run logged after it was deployed.
  const seqAt = (ms: number) => sorted.find((r) => r.at_ms >= ms)?.seq ?? next;
  // Sorted by (seq, rank, order): what was done before a run comes before it.
  const keyed: { seq: number; rank: number; order: number; item: TimelineItem }[] = [
    ...runs.map((run) => ({ seq: run.seq, rank: 1, order: 0, item: { kind: "run" as const, key: `r${run.seq}`, at_ms: run.at_ms, run } })),
    ...events.map((event) => ({
      seq: event.run_seq,
      rank: 0,
      order: event.seq,
      item: {
        kind: "event" as const,
        key: `e${event.seq}`,
        at_ms: event.at_ms,
        event,
        version: event.kind === "deploy" && event.version !== undefined ? byNumber.get(event.version) : undefined,
      },
    })),
    ...versions
      .filter((v) => !named.has(v.number))
      .map((version) => ({
        seq: version.first_seq ?? seqAt(version.deployed_at * 1000),
        rank: 0,
        order: -1_000_000 + version.number,
        item: { kind: "version" as const, key: `v${version.number}`, at_ms: version.deployed_at * 1000, version },
      })),
  ];
  keyed.sort((a, b) => a.seq - b.seq || a.rank - b.rank || a.order - b.order);
  return keyed.reverse().map((k) => k.item);
}

/** The kinds the timeline filters by. */
export type TimelineFilter = "runs" | "versions" | "access" | "lifecycle";
export const TIMELINE_FILTERS: [TimelineFilter, string, string][] = [
  ["runs", "runs", "each request, trigger and tick it ran"],
  ["versions", "deploys", "deploys and restores"],
  ["access", "access", "grants, secrets, folders and databases"],
  ["lifecycle", "lifecycle", "pauses, stops and runs again"],
];

export function filterOf(item: TimelineItem): TimelineFilter {
  if (item.kind === "run") return "runs";
  if (item.kind === "version") return "versions";
  const k = item.event.kind;
  return k === "deploy" || k === "restore" ? "versions" : k === "lifecycle" ? "lifecycle" : "access";
}

/** A point's name, as a restore says it: `version 2`, `run 41`. */
export const pointLabel = (p: RestorePoint) => ("version" in p ? `version ${p.version}` : `run ${p.run}`);

/** What can be restored to a point, and why not when it can't: its code, and its state. */
export type PointOptions = { code: string | null; state: string | null };

/** A version: its code unless it is the one deployed now or from before restores (rc.29); the
 *  state it left once it was replaced, when kept. */
export function versionOptions(v: VersionEntry): PointOptions {
  return {
    code: v.current ? "it is the version deployed now" : v.restorable ? null : "it was deployed before Holon kept what a deploy asks (rc.29 or earlier): open its canvas and deploy it again",
    state: v.current ? "the state it left is the live one" : v.state_kept ? null : "the state it left wasn't kept (deployed with rc.29 or earlier)",
  };
}

/** A run: the code it ran on, unless that is deployed now; the state it left, when it committed
 *  one that was kept. */
export function runOptions(run: LogEntry, currentUnit: string | undefined): PointOptions {
  return {
    code: !run.unit ? "the code it ran on isn't recorded" : run.unit === currentUnit ? "it ran on the code deployed now" : null,
    state: run.state_after ? null : "it left no state to restore: it didn't commit, or it ran before Holon kept the state runs leave",
  };
}

/** A capability a run performed, as the dry run counts it. */
function effectWord(name: string): string {
  return (
    { HttpRequest: "call(s) out", FileWrite: "file write(s)", DbWrite: "database write(s)", ChannelSend: "message(s) sent", RunsAction: "action(s) run", Spawn: "run(s) started" }[name] ??
    name
  );
}

/** What putting a state back would change: the keys, and the runs since with what they did outside. */
export function stateLines(change: StateChange): string[] {
  const parts: string[] = [];
  for (const [word, list] of [["changes", change.changed], ["brings back", change.added], ["drops", change.removed]] as const) {
    if (list.length > 0) parts.push(`${word} ${list.join(", ")}`);
  }
  const lines = [
    parts.length === 0
      ? `The state ${change.from} left is the state now: putting it back changes no key.`
      : `The state ${change.from} left is put back: it ${parts.join("; ")}.`,
  ];
  const effects = Object.entries(change.effects_since).map(([name, n]) => `${n} ${effectWord(name)}`);
  const runs = change.runs_since;
  lines.push(
    runs === 0
      ? `No run was logged since ${change.from}.`
      : effects.length === 0
        ? `${runs} run(s) logged since ${change.from}, which did nothing outside its state.`
        : `${runs} run(s) logged since ${change.from} also did, outside its state, what a restore doesn't undo: ${effects.join(", ")}.`,
  );
  lines.push("The state it replaces is kept, as the ending session's.");
  return lines;
}

/** A restore's confirmation, from its dry run; and the button that confirms it. `describe` says a
 *  permission (core's words, through wasm). */
export function restoreMessage(
  slug: string,
  point: RestorePoint,
  what: { code: boolean; state: boolean },
  preview: RestorePreview,
  describe: (item: string) => string = describeItem,
): { lines: string[]; action: string } {
  const which = what.code && what.state ? "code and state" : what.code ? "code" : "state";
  const lines = [`Restore "${slug}" to ${pointLabel(point)}: its ${which}?`];
  const needs = preview.needs_approval ?? [];
  if (what.code) {
    if (preview.code === null) lines.push("", `${pointLabel(point)} ran on the code deployed now: only its state is restored.`);
    else {
      lines.push("", `Version ${preview.code}'s code is deployed again, as a new version, checked against the grant "${slug}" has now.`);
      if (needs.length > 0) lines.push("", "It needs your approval to:", ...needs.map((i) => `• ${describe(i)}`));
      if ((preview.missing_secrets ?? []).length > 0) {
        lines.push("", `Not set now: ${preview.missing_secrets!.join(", ")} (those nodes stay empty until they are).`);
      }
    }
  }
  if (preview.state) lines.push("", ...stateLines(preview.state));
  return { lines, action: needs.length > 0 ? "Approve and restore" : "Restore" };
}

/** What a restore did, in a line. */
export function restoredLine(point: RestorePoint, done: RestoreDone): string {
  const said: string[] = [];
  if (done.code !== undefined) said.push(`version ${done.code}'s code is deployed again, as version ${done.version}`);
  if (done.state) said.push(`the state ${pointLabel(point)} left is live again, in session ${done.state.session} (session ${done.state.session - 1} keeps the one it replaced)`);
  return said.length > 0 ? `restored: ${said.join("; ")}` : "nothing to restore";
}
