// The runs a deployment's log doesn't hold: its unrecorded handlers' (docs/L4s/request-path-L4.md,
// C5), each counted by the server in a tally, shown as one row per handler where its runs would be.

import type { Tally } from "./server";

/** One unrecorded handler's runs in a line: "1,204 runs, 2 failed (the last 503 at 12:01:10: why),
 *  0.4 ms each, the last at 12:03:04". No run of it is logged, failed or not. */
export function tallyLine(t: Tally, time: (ms: number) => string = (ms) => new Date(ms).toLocaleTimeString()): string {
  const n = (x: number) => x.toLocaleString("en-US");
  const parts = [`${n(t.runs)} run${t.runs === 1 ? "" : "s"}`];
  if (t.failed > 0) {
    const f = t.last_failure;
    const last = f ? ` (the last ${f.status} at ${time(f.at_ms)}${f.error ? `: ${f.error}` : ""})` : "";
    parts.push(`${n(t.failed)} failed${last}`);
  }
  if (t.runs > 0) parts.push(`${(t.total_ms / t.runs).toFixed(1)} ms each`);
  if (t.last_at_ms > 0) parts.push(`the last at ${time(t.last_at_ms)}`);
  return parts.join(", ");
}

/** The tallies as rows, by label. */
export function tallyRows(tallies: Record<string, Tally> | undefined): { label: string; line: string }[] {
  return Object.entries(tallies ?? {})
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([label, t]) => ({ label, line: tallyLine(t) }));
}
