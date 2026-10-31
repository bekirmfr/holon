// A deployment's timeline (docs/L4s/history-L4.md §2, §6): its runs, the versions it was deployed
// with, and what was done to it (events.rs), merged as `holon history` merges them; the points it
// can go back to, and what a restore's dry run says, worded as `holon restore` words it.

import { describeItem, type WValue } from "./holon";
import type { Diff, DryRunReport } from "./DryRun";
import { diffCounts, keyLines, lineDiff, setDiff } from "./diff";
import type { DeploymentEvent, FileSince, LogEntry, RestoreDone, RestorePoint, RestorePreview, StateChange, VersionEntry } from "./server";

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
  return k === "deploy" || k === "restore" || k === "ask" ? "versions" : k === "lifecycle" ? "lifecycle" : "access";
}

/** What a version row says: deployed, or restored as it. */
export const versionWhat = (v: VersionEntry) =>
  v.restored_from ? `restored version ${v.restored_from}'s code, as version ${v.number}` : `deployed version ${v.number}`;

/** An item's words, as its row shows them: what the history's filter matches. */
export function itemText(item: TimelineItem): string {
  const by = (who: string | null | undefined) => (who ? `by ${who}` : "by the host");
  if (item.kind === "run") return `run #${item.run.seq} ${item.run.route ?? item.run.kind} s${item.run.session}`;
  if (item.kind === "version") return `deploy ${versionWhat(item.version)} ${by(item.version.by)}`;
  return `${item.event.kind} ${item.event.what} ${by(item.event.by)}`;
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

/** A file the runs since a point wrote, as a dry run says it (as `holon restore --dry-run` does):
 *  how many writes, by which runs, and whether it is on disk as the last one left it. */
export function fileLine(f: FileSince): string {
  const n = f.writes;
  const more = f.runs.length < n ? "…, " : "";
  const how = { write: "written", copy: "copied over", delete: "deleted" }[f.last.kind] ?? "written";
  let line = `${f.folder}/${f.name}: ${n} write${n === 1 ? "" : "s"} since, by run${n === 1 ? "" : "s"} ${more}${f.runs.join(", ")} (the last ${how})`;
  const last = f.last.run;
  if (f.unreadable) line += `; can't be checked on disk now: ${f.unreadable}`;
  else if (f.outside)
    line += f.disk ? `; changed outside a run since run ${last}: on disk now #${f.disk.hash.slice(0, 8)}, not what run ${last} left` : `; gone from disk since run ${last}, which left it`;
  return line;
}

/** What putting a state back would change: the keys, and the runs since with what they did outside. */
export function stateLines(change: StateChange): string[] {
  const parts: string[] = [];
  for (const [word, list] of [["changes", change.changed], ["brings back", change.added], ["drops", change.removed]] as const) {
    // A collection's key in the state (`#collection:prices`) said as a person reads it.
    if (list.length > 0) parts.push(`${word} ${list.map((k) => (k.startsWith("#collection:") ? `the collection ${k.slice("#collection:".length)}` : k)).join(", ")}`);
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

/** Where going back to a row goes: the point its code comes from and the point its state comes
 *  from, and why not, for each, when it can't. A run's and a version's are themselves; anything
 *  else done to it (a restore, a grant, a pause), how it was right after: the version deployed
 *  then, and the state then (what the run before it left, or the restore before it put back). */
export type Target = {
  label: string;
  code: RestorePoint | null;
  state: RestorePoint | null;
  why: PointOptions;
  /** The event it is, when it is one: what happened then. */
  event?: DeploymentEvent;
};

/** A point named as a restore names it (`version 3`, `run 41`), or null. */
export function parsePoint(label: string | undefined): RestorePoint | null {
  const m = /^(version|run) (\d+)$/.exec(label ?? "");
  if (!m) return null;
  return m[1] === "version" ? { version: Number(m[2]) } : { run: Number(m[2]) };
}

/** Where `timeline[i]` goes back to. `timeline` is all of it, newest first, its runs too. */
export function targetOf(timeline: TimelineItem[], i: number, currentUnit: string | undefined): Target {
  const item = timeline[i];
  if (item.kind === "run") {
    const p = { run: item.run.seq };
    return { label: `run ${item.run.seq}`, code: p, state: p, why: runOptions(item.run, currentUnit) };
  }
  const version = item.version;
  if (version) {
    const p = { version: version.number };
    return { label: `version ${version.number}`, code: p, state: p, why: versionOptions(version) };
  }
  if (item.kind !== "event") throw new Error("unreachable: a version item has its version");
  const event = item.event;
  // An agent's restore, saved for a person: what it asks, of the point it names.
  if (event.kind === "ask") {
    const point = parsePoint(event.from);
    const asks = (part: "code" | "state") => (!point ? "it names no point" : event.restores?.includes(part) ? null : `it asks for its ${part === "code" ? "state" : "code"} only`);
    const why = { code: asks("code"), state: asks("state") };
    return { label: event.from ?? `event #${event.seq}`, code: why.code === null ? point : null, state: why.state === null ? point : null, why, event };
  }
  const before = timeline.slice(i + 1);
  // The code: the version deployed then.
  const deployed = before.map((x) => (x.kind === "run" ? undefined : x.version)).find((v) => v !== undefined);
  const code = deployed
    ? { point: { version: deployed.number }, why: deployed.current ? "the code it had then is the code deployed now" : versionOptions(deployed).code }
    : { point: null, why: "no version was deployed before it" };
  // The state: a restore's is what it put back; anything else's, what was there before it.
  const state = event.kind === "restore" ? restoredState(event) : stateBefore(before);
  return { label: `event #${event.seq}`, code: code.point, state: state.point, why: { code: code.why, state: state.why }, event };
}

/** The point a restore put back: its `from`, or, recorded before it had one, its words. */
function restoredState(event: DeploymentEvent): { point: RestorePoint | null; why: string | null } {
  const point = parsePoint(event.from) ?? parsePoint(/^put back the state (.+) left, in session \d+$/.exec(event.what)?.[1]);
  return point ? { point, why: null } : { point: null, why: "it was recorded before a restore named the point it put back" };
}

/** The state as it was after `before` (newest first): the last run's that committed one, or what
 *  a restore after it put back. */
function stateBefore(before: TimelineItem[]): { point: RestorePoint | null; why: string | null } {
  for (const x of before) {
    if (x.kind === "run") {
      if (x.run.state_after) return { point: { run: x.run.seq }, why: null };
      continue;
    }
    if (x.kind !== "event") continue;
    if (x.event.kind === "restore") return restoredState(x.event);
    // A run after a stop starts from a state of its own, unless it goes on from the latest.
    if (x.event.kind === "lifecycle" && x.event.session !== undefined && !x.event.what.startsWith("ran from latest")) {
      return { point: null, why: `its state then came from a stop ("${x.event.what}"), which isn't a point to go back to` };
    }
  }
  return { point: null, why: "no run had left a state before it" };
}

/** One call a restore makes: a point, and what of it. Code and state from one point go together;
 *  from two, one after the other, the code first. */
export type RestorePart = { point: RestorePoint; what: { code: boolean; state: boolean } };

export function restoreParts(target: Target, what: { code: boolean; state: boolean }): RestorePart[] {
  const code = what.code && target.why.code === null ? target.code : null;
  const state = what.state && target.why.state === null ? target.state : null;
  if (code && state && pointLabel(code) === pointLabel(state)) return [{ point: code, what: { code: true, state: true } }];
  return [...(code ? [{ point: code, what: { code: true, state: false } }] : []), ...(state ? [{ point: state, what: { code: false, state: true } }] : [])];
}

/** A restore's dry run, as the dry-run window shows it (DryRun.tsx), from the dry run of each of
 *  its parts: what it changes, a line each, what it goes back to, what its code and its state
 *  would do, and what it asks to approve. `versions` are the deployment's (the one deployed now,
 *  the number the restored code gets); `describe` says a permission (core's words, through wasm). */
export function restoreReport(
  slug: string,
  target: Pick<Target, "label" | "event">,
  parts: (RestorePart & { preview: RestorePreview })[],
  describe: (item: string) => string = describeItem,
  versions: VersionEntry[] = [],
): DryRunReport {
  const code = parts.some((p) => p.what.code);
  const state = parts.some((p) => p.what.state);
  const which = code && state ? "code and state" : code ? "code" : "state";
  const sections: DryRunReport["sections"] = [];
  if (target.event) {
    const had = parts.flatMap((p) => [
      ...(p.what.code ? [`${pointLabel(p.point)}'s code`] : []),
      ...(p.what.state ? [`the state ${pointLabel(p.point)} left`] : []),
    ]);
    sections.push({ heading: "as it was then", lines: [`As it was after "${target.event.what}": ${had.join(", and ")}.`] });
  }
  const approvals: string[] = [];
  for (const { point, what, preview } of parts) {
    if (what.code) {
      if (preview.code === null) sections.push({ heading: "code", lines: [`${pointLabel(point)} ran on the code deployed now: only its state is restored.`] });
      else {
        const lines = [`Version ${preview.code}'s code is deployed again, as a new version, checked against the grant "${slug}" has now.`];
        if ((preview.missing_secrets ?? []).length > 0) lines.push(`Not set now: ${preview.missing_secrets!.join(", ")} (those nodes stay empty until they are).`);
        sections.push({ heading: "code", lines });
        approvals.push(...(preview.needs_approval ?? []).map(describe));
      }
    }
    if (preview.state) sections.push({ heading: "state", lines: stateLines(preview.state) });
    const files = preview.state?.files ?? [];
    if (files.length > 0)
      sections.push({
        heading: "files, not put back",
        lines: [...files.map(fileLine), ...(preview.state?.files_more ? ["and more"] : []), "A restore never changes a folder's files: these stay as they are now."],
      });
  }
  return {
    title: `Restore "${slug}" to ${target.label}: its ${which}?`,
    changes: restoreChanges(parts, versions),
    sections,
    approvals,
    action: approvals.length > 0 ? "Approve and restore" : "Restore",
  };
}

/** What a restore changes, a line each: the code deployed, the grant, the state. */
export function restoreChanges(parts: (RestorePart & { preview: RestorePreview })[], versions: VersionEntry[]): string[] {
  const changes: string[] = [];
  const now = versions.find((v) => v.current)?.number;
  const next = versions.reduce((n, v) => Math.max(n, v.number), 0) + 1;
  for (const { preview } of parts) {
    if (preview.code !== null && preview.code !== undefined) {
      changes.push(`code: version ${preview.code}'s is deployed again, as version ${next}${now !== undefined ? `, in place of version ${now}` : ""}`);
      const grant = grantDiff(preview);
      if (grant) {
        const { added, removed } = diffCounts(grant);
        const kept = grant.length - added - removed;
        changes.push(added + removed === 0 ? `permissions: unchanged (${kept} kept)` : `permissions: ${added} added, ${removed} dropped, ${kept} kept`);
      }
    }
    if (preview.state) {
      const s = preview.state;
      const said = [
        [s.changed.length, "changed"],
        [s.added.length, "brought back"],
        [s.removed.length, "dropped"],
      ]
        .filter(([n]) => n)
        .map(([n, word]) => `${n} key${n === 1 ? "" : "s"} ${word}`);
      const items = Object.entries(s.collections ?? {}).map(([name, c]) => {
        const n = Object.keys(c.items).length;
        return `${name}: ${n}${c.more ? "+" : ""} item${n === 1 && !c.more ? "" : "s"}`;
      });
      const detail = items.length > 0 ? ` (${items.join(", ")})` : "";
      changes.push(said.length === 0 ? `state: as ${s.from} left it, the same as now` : `state: as ${s.from} left it, ${said.join(", ")}${detail}`);
      const files = s.files ?? [];
      if (files.length > 0) {
        const outside = files.filter((f) => f.outside).length;
        const n = `${files.length}${s.files_more ? "+" : ""}`;
        changes.push(`files: left as they are (${n} written since${outside > 0 ? `, ${outside} of them changed outside a run` : ""})`);
      }
    }
  }
  return changes;
}

/** The grant now beside the one the restored code would have (its items, less what is
 *  withheld): null when there is no grant now or no code is deployed. */
function grantDiff(preview: RestorePreview) {
  if (!preview.granted_now || !preview.items) return null;
  const withheld = new Set(preview.withheld ?? []);
  return setDiff(preview.granted_now, preview.items.filter((i) => !withheld.has(i)));
}

/** A version's flow as text: printed with the names its text gave its nodes (`named`), when it
 *  was deployed from text, and without (`plain`) always. */
export type VersionText = { plain: string; named?: string };
/** Print a version (its unit) as text. */
export type CodeText = (unit: string) => Promise<VersionText>;

/** A restore's diff, part by part: the flow's text deployed now beside the restored version's
 *  (`codeText` prints a version's unit), the grant, and each state key it touches. */
export async function restoreDiffs(parts: (RestorePart & { preview: RestorePreview })[], versions: VersionEntry[], codeText: CodeText): Promise<Diff[]> {
  const diffs: Diff[] = [];
  const now = versions.find((v) => v.current);
  for (const { preview } of parts) {
    if (preview.code !== null && preview.code !== undefined) {
      const then = versions.find((v) => v.number === preview.code);
      const legend = `− version ${now?.number ?? "?"}, deployed now   + version ${preview.code}`;
      try {
        if (!now || !then) throw new Error("the versions aren't listed");
        const [before, after] = await Promise.all([codeText(now.unit), codeText(then.unit)]);
        // Names only when both have them: else a name the other lacks reads as a change.
        const named = before.named !== undefined && after.named !== undefined;
        const lines = lineDiff((named ? before.named! : before.plain).split("\n"), (named ? after.named! : after.plain).split("\n"));
        const unnamed = [before.named === undefined && now.number, after.named === undefined && then.number].filter((n) => n !== false);
        const notes = [
          diffCounts(lines).added + diffCounts(lines).removed === 0 ? "The same text: only the version changes." : "",
          !named ? `Printed without the text's names: version ${unnamed.join(" and version ")} ${unnamed.length > 1 ? "were" : "was"} deployed from the canvas, which keeps none.` : "",
        ].filter(Boolean);
        diffs.push({ title: "code", legend, lines, note: notes.join(" ") || undefined });
      } catch (e) {
        diffs.push({ title: "code", legend, lines: [], note: `The text can't be compared: ${e instanceof Error ? e.message : String(e)}` });
      }
      const grant = grantDiff(preview);
      if (grant) diffs.push({ title: "permissions", legend: "− granted now, dropped   + added   kept", lines: grant, whole: true });
    }
    if (preview.state) diffs.push(stateDiff(preview.state));
    for (const f of preview.state?.files ?? []) diffs.push(fileDiff(f, preview.state!.from));
  }
  return diffs;
}

/** How a file the runs since `from` wrote changed since, which a restore leaves as it is: its text
 *  then beside now, when the log has both; else what is known, by hash. */
export function fileDiff(f: FileSince, from: string): Diff {
  const short = (h: string | null | undefined) => (h ? `#${h.slice(0, 8)}` : "none");
  const notes = ["A restore leaves it as it is now."];
  if (f.outside) notes.push(f.disk ? `Changed outside a run since run ${f.last.run}.` : `Gone from disk since run ${f.last.run}.`);
  if (f.unreadable) notes.push(`It can't be checked on disk now: ${f.unreadable}.`);
  const lines = (text: string | null, absent: boolean) => (absent ? [] : text === null ? null : text.replace(/\n$/, "").split("\n"));
  const then = f.at_point ? lines(f.then_text, f.at_point.hash === null) : null;
  const now = f.unreadable ? null : lines(f.now_text, f.disk === null);
  if (!f.at_point) notes.push(`No run wrote it before ${from}: its text then isn't known.`);
  else if (then === null) notes.push(`Its text at ${from} isn't in the log (no run read it): ${short(f.at_point.hash)} then, ${short(f.disk?.hash)} now.`);
  else if (now === null && !f.unreadable) notes.push(`It is too big to show: ${short(f.at_point.hash)} then, ${short(f.disk?.hash)} now.`);
  return {
    title: `file ${f.folder}/${f.name}`,
    legend: `− at ${from}   + now on disk`,
    lines: then !== null && now !== null ? lineDiff(then, now) : [],
    note: notes.join(" "),
  };
}

/** The keys a state restore touches, as they are now beside as they would be. */
export function stateDiff(change: StateChange): Diff {
  type Pair = { now: WValue | null; then: WValue | null };
  // A key's value, or a collection's item as the flow reads it: `prices["tea"]`.
  const shown: [string, Pair][] = [
    ...Object.entries(change.values ?? {}),
    ...Object.entries(change.collections ?? {}).flatMap(([name, c]) => Object.entries(c.items).map(([k, v]): [string, Pair] => [`${name}[${JSON.stringify(k)}]`, v])),
  ].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const side = (pick: (v: Pair) => WValue | null) =>
    shown.flatMap(([k, pair]) => {
      const v = pick(pair);
      return v ? keyLines(k, v) : [];
    });
  const touched = change.changed.length + change.added.length + change.removed.length;
  const keys = Object.keys(change.values ?? {}).length + Object.keys(change.collections ?? {}).length;
  const partial = Object.entries(change.collections ?? {}).filter(([, c]) => c.more).map(([name]) => name);
  const notes = [
    touched === 0 ? "No key changes." : touched > keys ? `${touched - keys} more key(s) change: their values aren't shown.` : "",
    partial.length > 0 ? `Only some of ${partial.join(", ")}'s items are compared or shown.` : "",
  ].filter(Boolean);
  return { title: "state", legend: `− now   + as ${change.from} left it`, lines: lineDiff(side((v) => v.now), side((v) => v.then)), note: notes.join(" ") || undefined };
}

/** What a restore did, in a line. */
export function restoredLine(done: { point: RestorePoint; done: RestoreDone }[]): string {
  const said: string[] = [];
  for (const { point, done: d } of done) {
    if (d.code !== undefined) said.push(`version ${d.code}'s code is deployed again, as version ${d.version}`);
    if (d.state) said.push(`the state ${pointLabel(point)} left is live again, in session ${d.state.session} (session ${d.state.session - 1} keeps the one it replaced)`);
  }
  return said.length > 0 ? `restored: ${said.join("; ")}` : "nothing to restore";
}
