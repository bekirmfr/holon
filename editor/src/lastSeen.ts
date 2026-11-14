// When a principal was last seen, and a token last used (web-console-L4.md §9.2; the server keeps
// them, accounts.rs's `saw`), as People says them: how long ago, with the exact time as its title.

import { span } from "./storage";

/** `at` (epoch seconds; absent: never) said against `now` (milliseconds): "never", "just now",
 *  "7 min ago"; and the exact time, for its title. */
export function seenSaid(at: number | null | undefined, now: number): { text: string; title: string } {
  if (at === null || at === undefined) return { text: "never", title: "no request has come from it yet" };
  const ms = now - at * 1000;
  return { text: ms < 60_000 ? "just now" : `${span(ms)} ago`, title: new Date(at * 1000).toLocaleString() };
}
