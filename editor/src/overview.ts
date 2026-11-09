// What the Overview says of the host (OverviewPage.tsx), from its summary (GET /summary): the
// tiles' words, a flow's bars hour by hour, and the line under the title. Pure, so tested
// without a browser.

import type { Summary } from "./server";

/** The share of runs that failed, as a tile says it: "0%", "0.2%", "25%"; "–" with no runs. */
export function failedShare(runs: number, failed: number): string {
  if (runs === 0) return "–";
  const pct = (failed / runs) * 100;
  if (failed > 0 && pct < 0.1) return "<0.1%";
  return `${pct >= 10 || Number.isInteger(pct) ? Math.round(pct) : pct.toFixed(1)}%`;
}

/** How many flows run, and how many are held: "3 flows live, 1 paused". */
export function liveSaid(t: Summary["totals"]): string {
  const held = t.paused + t.stopped;
  const live = `flow${t.running === 1 ? "" : "s"} live`;
  if (held === 0) return live;
  return `${live}, ${held} ${t.stopped === 0 ? "paused" : t.paused === 0 ? "stopped" : "paused or stopped"}`;
}

/** The run log tile: whether every line names the one before, by the last check if there was
 *  one (else as the logs were read and written), and when. */
export function chainSaid(chain: Summary["chain"], time: (ms: number) => string): { value: string; label: string; bad: boolean } {
  const c = chain.checked;
  const breaks = c ? c.breaks.length : chain.breaks;
  const value = breaks === 0 ? "intact" : `${breaks} break${breaks === 1 ? "" : "s"}`;
  return { value, label: c ? `run log, checked ${time(c.at_ms)}` : "run log, as written (not checked yet)", bad: breaks > 0 };
}

/** One flow's hours as bars: each its share of the busiest hour (0 to 1, at least a sliver for an
 *  hour that ran), red where a run failed. */
export function bars(runs: number[], failed: number[]): { height: number; bad: boolean }[] {
  const most = Math.max(1, ...runs);
  return runs.map((n, i) => ({ height: n === 0 ? 0 : Math.max(0.08, n / most), bad: (failed[i] ?? 0) > 0 }));
}

/** A flow's hours in words, for those who don't see the bars. */
export function hoursSaid(runs: number[], failed: number[]): string {
  const n = runs.reduce((a, b) => a + b, 0);
  const f = failed.reduce((a, b) => a + b, 0);
  return `${n.toLocaleString()} run${n === 1 ? "" : "s"}${f > 0 ? `, ${f.toLocaleString()} failed` : ""} in the last ${runs.length} hours`;
}

/** "Good morning" before noon, "Good afternoon" until six, "Good evening" after. */
export function greeting(hour: number): string {
  return hour < 12 ? "Good morning" : hour < 18 ? "Good afternoon" : "Good evening";
}

/** The line under the title: how many flows, what waits for approval, and the runs that wait. */
export function hostLine(flows: number, approvals: number, waiting: number): string {
  const parts = [`${flows} flow${flows === 1 ? "" : "s"} on this host.`];
  parts.push(approvals === 0 ? "Nothing waits for approval." : `${approvals === 1 ? "One thing waits" : `${approvals} things wait`} for approval.`);
  if (waiting > 0) parts.push(`${waiting} run${waiting === 1 ? " waits" : "s wait"} for an approval or an event.`);
  return parts.join(" ");
}
