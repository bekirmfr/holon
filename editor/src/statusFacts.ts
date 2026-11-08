// What a flow's Status tab says (FlowPage.tsx), computed: its last day of runs, and since when it
// may do each thing it may, and who approved it then.

import { runOutcome } from "./runs";
import type { LogEntry } from "./server";

const DAY_MS = 24 * 60 * 60 * 1000;

/** Its runs in the day before `now` (of those the log holds here): how many, how many failed,
 *  and how long they took, the median and the 95th percentile. */
export function lastDay(entries: LogEntry[], now: number): { runs: number; failed: number; p50: number | null; p95: number | null } {
  const day = entries.filter((e) => e.at_ms > now - DAY_MS);
  const took = day.map((e) => e.duration_ms).sort((a, b) => a - b);
  const at = (q: number) => (took.length === 0 ? null : took[Math.min(took.length - 1, Math.floor(q * took.length))]);
  return { runs: day.length, failed: day.filter((e) => runOutcome(e) === "failed").length, p50: at(0.5), p95: at(0.95) };
}

/** A version, newest first, with what it may do (null: not known, its canvas isn't kept). */
export type VersionItems = { number: number; by: string | null; items: string[] | null };

/** Since which version each of `items` has been asked for without a break, and who deployed
 *  (approved) that version. `known` is false when the walk stopped at a version whose items
 *  aren't known: it may be older still. */
export function sinceOf(items: string[], versions: VersionItems[]): Map<string, { since: number; by: string | null; known: boolean }> {
  const out = new Map<string, { since: number; by: string | null; known: boolean }>();
  if (versions.length === 0) return out;
  for (const item of items) {
    let at: VersionItems | null = null;
    let known = true;
    for (const v of versions) {
      if (v.items === null) {
        known = false;
        break;
      }
      if (!v.items.includes(item)) break;
      at = v;
    }
    // The version deployed now asks for it, whatever its items say: since it, at least.
    const from = at ?? versions[0];
    out.set(item, { since: from.number, by: from.by, known: at !== null && known });
  }
  return out;
}
