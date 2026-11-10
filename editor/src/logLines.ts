// The log lines of a deployment (docs/L4s/logs-L4.md §5), as the console asks for them and shows
// them: a level filter, and for a run either the lines up to the end of it or its own.

import type { LogLine } from "./server";

/** What the level filter offers: every line, warnings and louder, errors. */
export type LogLevel = "all" | "warn" | "error";

export const LOG_LEVELS: [LogLevel, string][] = [
  ["all", "All"],
  ["warn", "Warnings"],
  ["error", "Errors"],
];

/** What to ask the server for: lines at a level or louder; with a run, those up to the end of its
 *  (`until`), or only its own (`own`); read on after the time `after` (ms). `limit`: the newest. */
export function logQuery(level: LogLevel, run: number | undefined, own: boolean, limit: number, after?: number) {
  return {
    level: level === "all" ? undefined : level,
    ...(run === undefined ? {} : own ? { run } : { until: run }),
    after,
    limit,
  };
}

/** The time of a line, to the millisecond: `10:49:05.834`. */
export function clock(at: number): string {
  const d = new Date(at);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}

/** A line's level as a terminal prints it, in capitals. */
export const levelName = (l: LogLine): string => l.level.toUpperCase();

/** A line that stands for some that were dropped (a rate over its cap, a file that couldn't be written). */
export const isDropped = (l: LogLine): boolean => (l.dropped ?? 0) > 0;

/** `lines` with `more` after them, the newest `keep` only: a pane that follows a busy flow stays small. */
export function appended(lines: LogLine[], more: LogLine[], keep: number): LogLine[] {
  const all = [...lines, ...more];
  return all.length > keep ? all.slice(all.length - keep) : all;
}
