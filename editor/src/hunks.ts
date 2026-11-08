// A review in hunks (gaps-web.md §4.1, as the workbench's review.rs shows one): what a deploy or a
// restore changes, a hunk for the code and one for each group of what it touches, each with a
// summary in its header, unchanged lines far from a change folded. Review.tsx shows them; this is
// the pure part.

import { collapse, lineDiff, type DiffLine } from "./diff";
import type { Mark, TouchLine } from "./touches";
import type { Diff } from "./DryRun";

/** A row's tone: its ground and its sign. */
export type Tone = "add" | "del" | "approve" | "changed" | "unready" | "same";

/** One row of a hunk: its line number in the text (code rows), its sign, what it says, a note. */
export type Row = { n: number | null; sign: string; text: string; note: string; tone: Tone };
/** A run of unchanged rows, folded: how many, and the rows to show when it is opened. */
export type Fold = { fold: Row[] };
export type Shown = Row | Fold;

export type Hunk = {
  /** What it is, as its header names it ("code", "What it reaches outside"). */
  title: string;
  /** Its changes, counted: `+2 · −1`, `1 to approve · 1 new · 2 the same`. */
  summary: string;
  rows: Shown[];
  /** Whether it changes anything (n and N stop at it). */
  changes: boolean;
  /** Whether it starts open: one that changes something does, apart from a first deploy's text,
   *  all new, which would hide what it touches below it. */
  opens: boolean;
  /** Said under its header: which side is which, why there is no diff, what it leaves out. */
  note?: string;
};

export const isFold = (s: Shown): s is Fold => "fold" in s;

/** The code's hunk: `before` (null: none deployed yet) to `after`, line by line, numbered as
 *  `after` is (a removed line as `before` was), unchanged runs more than 3 lines from a change
 *  folded. */
export function codeHunk(before: string | null, after: string, title = "code"): Hunk {
  const was = before === null ? [] : splitLines(before);
  const is = splitLines(after);
  const lines = lineDiff(was, is);
  let a = 0;
  let b = 0;
  const rows: Row[] = lines.map((l) => {
    if (l.sign === "-") return { n: ++b, sign: "−", text: l.text, note: "", tone: "del" };
    if (l.sign === " ") b++;
    return { n: ++a, sign: l.sign === "+" ? "+" : " ", text: l.text, note: "", tone: l.sign === "+" ? "add" : "same" };
  });
  const added = lines.filter((l) => l.sign === "+").length;
  const removed = lines.filter((l) => l.sign === "-").length;
  const summary = before === null ? `new · ${is.length} line${is.length === 1 ? "" : "s"}` : added + removed === 0 ? "the same" : `+${added} · −${removed}`;
  return { title, summary, rows: folded(lines, rows), changes: before === null || added + removed > 0, opens: before !== null && added + removed > 0 };
}

/** A dry run's diff part (DryRun.tsx: the code, the grant, the state, a file) as a hunk: the
 *  code's rows numbered as the restored text is, a grant's whole, others folded like the code. */
export function diffHunk(d: Diff): Hunk {
  let a = 0;
  let b = 0;
  const numbered = d.title === "code";
  const rows: Row[] = d.lines.map((l) => {
    if (l.sign === "-") return { n: numbered ? ++b : null, sign: "−", text: l.text, note: "", tone: "del" };
    if (l.sign === " ") b++;
    a++;
    return { n: numbered ? a : null, sign: l.sign === "+" ? "+" : " ", text: l.text, note: "", tone: l.sign === "+" ? "add" : "same" };
  });
  const added = d.lines.filter((l) => l.sign === "+").length;
  const removed = d.lines.filter((l) => l.sign === "-").length;
  const changes = added + removed > 0;
  const note = [d.legend, d.note].filter(Boolean).join(" · ");
  return {
    title: d.title,
    summary: d.lines.length === 0 ? "" : changes ? `+${added} · −${removed}` : "the same",
    rows: d.whole ? rows : folded(d.lines, rows),
    changes,
    opens: changes || d.lines.length === 0,
    note,
  };
}

const TONE: Record<Exclude<Mark, "head">, Tone> = { new: "add", approve: "approve", gone: "del", same: "same", changed: "changed", unready: "unready" };
const SIGN: Record<Exclude<Mark, "head">, string> = { new: "+", approve: "+", gone: "−", same: "·", changed: "~", unready: "!" };
const SAID: [Exclude<Mark, "head">, string][] = [
  ["approve", "to approve"],
  ["unready", "not ready"],
  ["new", "new"],
  ["changed", "changed"],
  ["gone", "no longer"],
  ["same", "the same"],
];

/** A dry run's lines (touches.ts's touchLines) as hunks, one per heading. */
export function touchHunks(lines: TouchLine[]): Hunk[] {
  const out: Hunk[] = [];
  for (const l of lines) {
    if (l.mark === "head") {
      out.push({ title: l.text, summary: "", rows: [], changes: false, opens: false });
      continue;
    }
    if (out.length === 0) out.push({ title: "Also", summary: "", rows: [], changes: false, opens: false });
    out[out.length - 1].rows.push({ n: null, sign: SIGN[l.mark], text: l.text, note: l.note, tone: TONE[l.mark] });
  }
  for (const h of out) {
    const tones = (h.rows as Row[]).map((r) => r.tone);
    h.summary = SAID.flatMap(([m, said]) => {
      const k = tones.filter((t) => t === TONE[m]).length;
      return k > 0 ? [`${k} ${said}`] : [];
    }).join(" · ");
    h.changes = tones.some((t) => t !== "same");
    h.opens = h.changes;
  }
  return out;
}

/** Where `n` (1) or `N` (-1) goes from `at`: the next or previous hunk with changes, wrapping;
 *  `at` itself when none other has. */
export function nextHunk(hunks: Hunk[], at: number, by: -1 | 1): number {
  for (let k = 1; k <= hunks.length; k++) {
    const i = (((at + by * k) % hunks.length) + hunks.length) % hunks.length;
    if (hunks[i].changes) return i;
  }
  return at;
}

function splitLines(text: string): string[] {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** `rows` (one per line of `lines`) with the runs `collapse` leaves out as folds. */
function folded(lines: DiffLine[], rows: Row[]): Shown[] {
  const shown = collapse(lines);
  const out: Shown[] = [];
  let i = 0;
  for (const s of shown) {
    if (s.sign === "…") {
      const k = Number(s.text.split(" ")[0]);
      out.push({ fold: rows.slice(i, i + k) });
      i += k;
    } else out.push(rows[i++]);
  }
  return out;
}
