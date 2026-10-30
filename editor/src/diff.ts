// Diffs for a dry run (DryRun.tsx): what an action would change beside what is there now, line by
// line (a flow's text, the state's values written out) or item by item (a grant).

import type { WValue } from "./holon";

/** A line of a diff: kept (" "), only before ("-"), only after ("+"). */
export type DiffLine = { sign: " " | "+" | "-"; text: string };
/** A diff as shown: unchanged runs far from a change are one line saying how many ("…"). */
export type ShownLine = DiffLine | { sign: "…"; text: string };

/** Past this many cells (before × after lines, once the common start and end are set aside) the
 *  middle is shown as all removed then all added, not matched line by line. */
const MAX_CELLS = 4_000_000;

/** `before` to `after`, line by line: the longest run of lines they share is kept, the rest is
 *  removed or added, removals first where both happen at one place. */
export function lineDiff(before: string[], after: string[]): DiffLine[] {
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start++;
  let end = 0;
  while (end < before.length - start && end < after.length - start && before[before.length - 1 - end] === after[after.length - 1 - end]) end++;
  const a = before.slice(start, before.length - end);
  const b = after.slice(start, after.length - end);
  const middle: DiffLine[] =
    a.length * b.length > MAX_CELLS ? [...a.map((text) => ({ sign: "-" as const, text })), ...b.map((text) => ({ sign: "+" as const, text }))] : matched(a, b);
  const same = (text: string): DiffLine => ({ sign: " ", text });
  return [...before.slice(0, start).map(same), ...middle, ...before.slice(before.length - end).map(same)];
}

function matched(a: string[], b: string[]): DiffLine[] {
  const w = b.length + 1;
  // longest[i * w + j]: the longest common run of a[i..] and b[j..].
  const longest = new Uint32Array((a.length + 1) * w);
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      longest[i * w + j] = a[i] === b[j] ? longest[(i + 1) * w + j + 1] + 1 : Math.max(longest[(i + 1) * w + j], longest[i * w + j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      out.push({ sign: " ", text: a[i] });
      i++;
      j++;
    } else if (j >= b.length || (i < a.length && longest[(i + 1) * w + j] >= longest[i * w + j + 1])) {
      out.push({ sign: "-", text: a[i++] });
    } else {
      out.push({ sign: "+", text: b[j++] });
    }
  }
  return out;
}

/** What is only in `before`, only in `after`, and in both, sorted: a grant's items, say. */
export function setDiff(before: Iterable<string>, after: Iterable<string>): DiffLine[] {
  const was = new Set(before);
  const is = new Set(after);
  return [...new Set([...was, ...is])].sort().map((text) => ({ sign: was.has(text) ? (is.has(text) ? " " : "-") : "+", text }));
}

/** `lines` with unchanged runs more than `context` lines from a change left out, each as one
 *  line saying how many. */
export function collapse(lines: DiffLine[], context = 3): ShownLine[] {
  const near = new Uint8Array(lines.length);
  lines.forEach((l, i) => {
    if (l.sign === " ") return;
    for (let k = Math.max(0, i - context); k <= Math.min(lines.length - 1, i + context); k++) near[k] = 1;
  });
  const out: ShownLine[] = [];
  let skipped = 0;
  const flush = () => {
    if (skipped > 0) out.push({ sign: "…", text: `${skipped} unchanged line${skipped === 1 ? "" : "s"}` });
    skipped = 0;
  };
  lines.forEach((l, i) => {
    if (near[i]) {
      flush();
      out.push(l);
    } else skipped++;
  });
  flush();
  return out;
}

/** How many lines a diff adds and removes. */
export function diffCounts(lines: DiffLine[]): { added: number; removed: number } {
  return { added: lines.filter((l) => l.sign === "+").length, removed: lines.filter((l) => l.sign === "-").length };
}

/** Up to this many characters, a list or record stays on one line. */
const WIDTH = 72;

/** `v` on one line, as the log writes it; a text is quoted as JSON, so it stays one line. */
export function flatValue(v: WValue): string {
  switch (v.type) {
    case "Number":
      return String(v.value);
    case "Boolean":
      return v.value ? "true" : "false";
    case "Text":
      return JSON.stringify(v.value);
    case "List":
      return `[${v.value.map(flatValue).join(", ")}]`;
    case "Effect":
      return `effect(${v.value.sink}, ${flatValue(v.value.payload)})`;
    case "Sum":
      return `sum#${v.value.arm}(${flatValue(v.value.payload)})`;
    case "Record":
      return `{${v.value.map(([k, x]) => `${k}: ${flatValue(x)}`).join(", ")}}`;
  }
  const exhaustive: never = v;
  return exhaustive;
}

/** `v` written out for a diff: on one line when it is short, else a list's items and a record's
 *  fields one per line, so a diff shows which of them changed. */
export function valueLines(v: WValue, indent = ""): string[] {
  const flat = flatValue(v);
  if (indent.length + flat.length <= WIDTH || (v.type !== "List" && v.type !== "Record") || v.value.length === 0) return [indent + flat];
  const inner = indent + "  ";
  const items = v.type === "List" ? v.value.map((x) => valueLines(x, inner)) : v.value.map(([k, x]) => labelled(`${k}: `, valueLines(x, inner), inner));
  const body = items.flatMap((lines, i) => (i < items.length - 1 ? [...lines.slice(0, -1), lines[lines.length - 1] + ","] : lines));
  return [indent + (v.type === "List" ? "[" : "{"), ...body, indent + (v.type === "List" ? "]" : "}")];
}

/** `lines` (written at `indent`) with `label` before the first. */
function labelled(label: string, lines: string[], indent: string): string[] {
  return [indent + label + lines[0].slice(indent.length), ...lines.slice(1)];
}

/** A state key and its value, for a diff: `key: value`, the value as `valueLines` writes it. */
export function keyLines(key: string, v: WValue): string[] {
  return labelled(`${key}: `, valueLines(v), "");
}
