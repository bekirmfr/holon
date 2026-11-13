// What a flow's page says of it (FlowPage.tsx, web-console-L4.md §8), worked out from what the
// server answers: what starts it, its description, what a run sent out and which secrets it read
// and where they went, whether a witness holds its record, the files its runs wrote, and its
// settings as numbers, each lowered in place or raised by a new version.

import { formatValue } from "./diagnostics";
import type { WValue } from "./holon";
import { field, text } from "./runs";
import type { CellHistory, Grant, LogEntry } from "./server";

/** What starts its runs, said short: its routes and port, then its other triggers
 *  (`2 routes · :8110 · every 10s`). */
export function startsSaid(triggers: string[], port: number | null, routes: number | null): string {
  const http = port !== null ? [routes !== null ? `${routes} route${routes === 1 ? "" : "s"}` : null, `:${port}`] : [];
  const others = triggers.filter((t) => !/^Http :\d+$/.test(t));
  return [...http, ...others].filter((s): s is string => !!s).join(" · ") || "nothing starts it";
}

/** Its description: the first comment line of its text, without the `//`. */
export function firstComment(source: string | undefined): string | null {
  for (const line of (source ?? "").split(/\r?\n/)) {
    const t = line.trim();
    if (t === "") continue;
    const m = /^\/\/\s?(.*)$/.exec(t);
    return m && m[1].trim() ? m[1].trim() : null;
  }
  return null;
}

/** What Run now may start: its clocks, each ticked now, and its actions, each run with nothing. */
export type RunNow = { kind: "clock" | "action"; name: string };
export function runNowOptions(triggers: string[]): RunNow[] {
  return [
    ...triggers.filter((t) => t.startsWith("every ")).map((name): RunNow => ({ kind: "clock", name })),
    ...triggers.filter((t) => t.startsWith("action ")).map((t): RunNow => ({ kind: "action", name: t.slice("action ".length) })),
  ];
}

const short = (s: string, n = 60) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

/** What a run sent out, in a line: its answer's body, or what it produced; "" when nothing. */
export function wentOut(e: LogEntry): string {
  if (e.response_body !== null && e.response_body !== undefined) return short(e.response_body.replace(/\s+/g, " ").trim() || "(empty)");
  const outs = (e.outputs ?? []).filter((v): v is WValue => v !== null && v !== undefined);
  return outs.length > 0 ? short(outs.map((v) => formatValue(v).replace(/\s+/g, " ")).join(", ")) : "";
}

/** A secret a run read: its version and fingerprint (`v3`, `#a1b2c3d4`), and the hosts its
 *  requests took it to (a request whose text holds its mark). Never its value: the log has none. */
export type SecretUse = { name: string; version: string | null; fingerprint: string | null; to: string[] };
export function secretUses(e: LogEntry): SecretUse[] {
  const exchanges = (e.rounds ?? []).flatMap((r) => r.exchanges);
  return Object.entries(e.secrets ?? {}).map(([name, mark]) => {
    const version = /\bv\d+\b/.exec(mark)?.[0] ?? null;
    const fingerprint = /#[0-9a-f]+/i.exec(mark)?.[0] ?? null;
    const to = new Set<string>();
    for (const x of exchanges) {
      if (!JSON.stringify(x.request).includes(`‹secret:${name}`)) continue;
      const url = text(field(x.request, "url"));
      to.add(hostOf(url) || url || "a request");
    }
    return { name, version, fingerprint, to: [...to] };
  });
}

const hostOf = (url: string) => /^[a-z]+:\/\/([^/?#]+)/i.exec(url)?.[1] ?? "";

/** The witness the host's heads go to (`/summary`'s `witness`), as far as the editor knows it. */
export type Witness = { address: string; taken_ms: number | null; waiting: number } | null;

/** Whether a witness holds the record of a run that ended at `endedMs`: it took a batch of heads
 *  at or after it. The server keeps only when it last took one (web-console-L4.md §12, Q7), so a
 *  run after that waits for the next. */
export function witnessed(endedMs: number, witness: Witness): { yes: boolean; said: string } {
  if (witness === undefined || witness === null) return { yes: false, said: "no witness set: start the host with --export-heads" };
  if (witness.taken_ms !== null && witness.taken_ms >= endedMs) return { yes: true, said: `yes, by ${witness.address}, at ${new Date(witness.taken_ms).toLocaleString()}` };
  return { yes: false, said: `not yet: ${witness.address} takes the next batch${witness.waiting > 0 ? ` (${witness.waiting} head${witness.waiting === 1 ? "" : "s"} wait)` : ""}` };
}

/** A file its runs wrote: how many runs wrote it, and the last one, with what it did. */
export type FileWrote = { folder: string; name: string; runs: number; last: number; kind: "write" | "copy" | "delete" };
export function filesWritten(entries: LogEntry[]): FileWrote[] {
  const files = new Map<string, FileWrote & { seen: Set<number> }>();
  for (const e of entries) {
    if (e.error !== null) continue;
    for (const p of e.performed) {
      const folder = /^FileWrite\("(.*)"\)$/.exec(p.capability)?.[1];
      if (folder === undefined) continue;
      const v = p.payload as WValue | null;
      const name = v?.type === "Text" ? v.value : text(field(v, "name"));
      if (!name) continue;
      const kind = v?.type === "Text" ? "delete" : field(v, "from") ? "copy" : "write";
      const key = `${folder}/${name}`;
      const f = files.get(key) ?? { folder, name, runs: 0, last: e.seq, kind, seen: new Set<number>() };
      f.seen.add(e.seq);
      f.runs = f.seen.size;
      if (e.seq >= f.last) Object.assign(f, { last: e.seq, kind });
      files.set(key, f);
    }
  }
  return [...files.values()].map(({ seen: _, ...f }) => f).sort((a, b) => b.last - a.last || a.name.localeCompare(b.name));
}

/** The limits Settings shows, by name, in words and their unit, whether asked for or not: what a
 *  person most often changes (§8.4). Any other the flow asked for is shown after them. */
export const SETTING_LIMITS: { name: string; label: string; unit: string; group: "limits" | "runs kept" }[] = [
  { name: "max_requests", label: "calls out per run", unit: "", group: "limits" },
  { name: "run_budget_ms", label: "run budget", unit: "ms", group: "limits" },
  { name: "max_request_bytes", label: "request body size", unit: "bytes", group: "limits" },
  { name: "keep_runs", label: "newest runs kept", unit: "runs", group: "runs kept" },
  { name: "keep_run_days", label: "none older than", unit: "days", group: "runs kept" },
];

/** One limit as its runs are held to it: what it asked for (granted, else the host's default),
 *  what the host lowered it to, the policy's ceiling, and the value that holds. */
export type LimitRow = { name: string; label: string; unit: string; group: "limits" | "runs kept"; now: number; asked: number | null; base: number; lowered: number | null; ceiling: number | null };

/** The rows for `grant` under the host's `defaults` and `ceilings` (`GET /permissions`): the
 *  named ones first, then any other it asked for; a limit the host doesn't know is left out. */
export function limitRows(grant: Grant, defaults: Record<string, number>, ceilings: Record<string, number>): LimitRow[] {
  const asked = grant.requirements.limits ?? {};
  const lowered = grant.lowered ?? {};
  const named = new Set(SETTING_LIMITS.map((l) => l.name));
  const extra = Object.keys(asked)
    .filter((n) => !named.has(n))
    .sort()
    .map((name) => ({ name, label: name, unit: name.endsWith("_ms") ? "ms" : name.endsWith("_bytes") ? "bytes" : "", group: "limits" as const }));
  return [...SETTING_LIMITS, ...extra].flatMap((l) => {
    const def = defaults[l.name];
    if (def === undefined) return [];
    const a = asked[l.name];
    const granted = a !== undefined && grant.items.includes(`Limit(${l.name}=${a})`);
    const base = granted ? a : def;
    const low = lowered[l.name] ?? null;
    const ceiling = ceilings[l.name] ?? null;
    const now = Math.min(base, low ?? Infinity, ceiling ?? Infinity);
    return [{ ...l, now, asked: a ?? null, base, lowered: low, ceiling }];
  });
}

/** How a limit typed for `row` applies: at once when it is lower, by a new version a person
 *  approves when it is higher, not at all when it is the same or not a number. */
export function limitChange(row: LimitRow, typed: string): "lower" | "raise" | "same" | "bad" {
  if (typed.trim() === "") return "same";
  const n = Number(typed);
  if (!Number.isInteger(n) || n < 0) return "bad";
  return n < row.now ? "lower" : n > row.now ? "raise" : "same";
}

/** A cell's numeric values over time, oldest first (each change's value after it): what its line
 *  over time draws. A change to something that isn't a number leaves a gap. */
export function cellPoints(h: CellHistory): { seq: number | null; at_ms: number | null; value: number | null }[] {
  return h.changes
    .slice()
    .reverse()
    .map((c) => ({ seq: c.seq ?? null, at_ms: c.at_ms ?? null, value: c.after?.type === "Number" ? c.after.value : null }));
}
