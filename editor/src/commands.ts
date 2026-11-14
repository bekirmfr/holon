// The command palette's (ui/CommandPalette.tsx) pure part: how well what is typed matches an
// entry, the same way the terminal workbench's Ctrl+P does (crates/holon/src/workbench/palette.rs),
// a run asked for by its number, and what a `:` command says.

import { isTheme, type Theme } from "./look";

/** One thing the palette can open or do. */
export type Command = {
  label: string;
  /** Said beside it, dimmed: what kind of thing it is. */
  kind: string;
  run: () => void;
};

/** How well `query` matches `label` (lower is better): each of its words in the label, best at
 *  the label's start, then at a word's start, then inside one; else the word's letters in order,
 *  last. Null when a word matches neither way. */
export function score(label: string, query: string): number | null {
  const l = label.toLowerCase();
  let total = 0;
  for (const word of query.toLowerCase().split(/\s+/).filter(Boolean)) {
    const i = l.indexOf(word);
    if (i === 0) continue;
    if (i > 0) {
      total += /[a-z0-9]/.test(l[i - 1]) ? 3 : 1;
      continue;
    }
    let at = 0;
    for (const c of word) {
      at = l.indexOf(c, at) + 1;
      if (at === 0) return null;
    }
    total += 8;
  }
  return total;
}

/** The entries that match, best first and otherwise in their order; all of them for no query. */
export function ranked<T extends { label: string }>(items: T[], query: string): T[] {
  return items
    .map((it, i) => ({ it, i, s: score(it.label, query) }))
    .filter((x): x is { it: T; i: number; s: number } => x.s !== null)
    .sort((a, b) => a.s - b.s || a.i - b.i)
    .map((x) => x.it);
}

/** The run a query asks for by its number (`4812`, `#4812`, `run 4812`), or null. */
export function runNumber(query: string): number | null {
  const m = /^\s*(?:run\s+)?#?(\d+)\s*$/i.exec(query);
  return m ? Number(m[1]) : null;
}

/** What the palette shows for `query`: a run asked for by its number first (`byNumber` says
 *  which runs that is: that run of each deployed flow), then the entries that match, best
 *  first; an entry the number already gave isn't shown twice. */
export function matches(commands: Command[], query: string, byNumber: (seq: number) => Command[] = () => []): Command[] {
  const seq = runNumber(query);
  const first = seq === null ? [] : byNumber(seq);
  const given = new Set(first.map((c) => c.label));
  return [...first, ...ranked(commands, query).filter((c) => !given.has(c.label))];
}

/** What a `:` command asks for. */
export type ColonCommand =
  | { kind: "line"; line: number }
  | { kind: "deploy" }
  | { kind: "save" }
  | { kind: "fmt" }
  | { kind: "new" }
  | { kind: "restore" }
  | { kind: "keys" }
  | { kind: "check" }
  | { kind: "call" }
  | { kind: "theme"; theme: Theme };

/** The `:` commands, as the palette offers them while one is being typed. */
export const COLON_HELP: [string, string][] = [
  [":12", "go to line 12 of the text"],
  [":deploy", "deploy the flow"],
  [":save", "save it"],
  [":fmt", "format the text as holon fmt writes it"],
  [":check", "look for mistakes in the text now, as holon check does"],
  [":call", "try a request: the request tester"],
  [":new", "a new, untitled flow"],
  [":restore", "go back to an earlier version of the flow viewed"],
  [":theme light", "how the editor looks: auto, light or dark"],
  [":keys", "every key"],
];

/** The command `text` says (with or without its `:`), or why it says none. */
export function parseColon(text: string): ColonCommand | { error: string } {
  const [head = "", ...rest] = text.replace(/^:/, "").trim().split(/\s+/);
  if (/^\d+$/.test(head)) return Number(head) > 0 ? { kind: "line", line: Number(head) } : { error: "lines start at 1" };
  switch (head.toLowerCase()) {
    case "deploy":
    case "save":
    case "fmt":
    case "new":
    case "restore":
    case "keys":
    case "check":
    case "call":
      return { kind: head.toLowerCase() as "deploy" };
    case "w":
      return { kind: "save" };
    case "theme": {
      const t = (rest[0] ?? "").toLowerCase();
      return isTheme(t) ? { kind: "theme", theme: t } : { error: "a theme is auto, light or dark" };
    }
    case "":
      return { error: "type a command after the colon" };
    default:
      return { error: `there is no command :${head}` };
  }
}
