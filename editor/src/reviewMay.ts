// The Review page's left column (gaps-web.md G9, C4): what a version may do, from a deploy's
// preview (`touches`), grouped as a person decides on it: the approvals its runs would wait at that
// no one here can decide yet, first, each saying why; what needs their approval, what is unchanged
// (or the host allows already), what it no longer does. Each points at the line of the
// flow's text it comes from, found by scanning the text for its name, as the workbench marks
// names (review.rs) — the nearest line, not a proof.

import { cannotDecide, touchLines, type SettingChange, type Touch, type TouchLine } from "./touches";

export type MayRow = { line: TouchLine; anchor: number | null };
export type MayGroup = { key: "undecided" | "approve" | "same" | "gone"; title: string; rows: MayRow[] };

/** The text's lines, as the review numbers them. */
export function textLines(text: string): string[] {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

/** The line (1-based) a thing a flow touches comes from: its name quoted, then its name, then
 *  for the requests it receives its first route, for the clock its first `every`. Comments are
 *  passed over: they say what the code does, the code is where it does it. */
export function anchorOf(t: Pick<Touch, "kind" | "name">, lines: string[]): number | null {
  const first = (test: (l: string) => boolean) => {
    const i = lines.findIndex((l) => !/^\s*(\/\/|\*|\/\*)/.test(l) && test(l));
    return i < 0 ? null : i + 1;
  };
  if (t.name !== "") {
    const parts = [t.name, ...t.name.split("/").filter((p) => p !== t.name)];
    for (const p of parts) {
      const at = first((l) => l.includes(`"${p}"`)) ?? first((l) => l.includes(p));
      if (at !== null) return at;
    }
  }
  if (t.kind === "requests") return first((l) => /\.(get|post|put|patch|delete)\s*\(|\broute\s*\(/i.test(l));
  if (t.kind === "clock") return first((l) => /\bevery\s*\(/.test(l));
  return null;
}

/** Which group a row goes in: an approval no one here can decide, a use to approve, a thing no
 *  longer touched, or the rest. */
function groupOf(t: Touch): MayGroup["key"] {
  if (cannotDecide(t)) return "undecided";
  if (t.change === "gone") return "gone";
  return t.uses.some((u) => u.approval && u.change !== "gone") ? "approve" : "same";
}

/** What it may do, in the groups a person decides by, each row said as a dry run says it and
 *  anchored to its line; settings that change are with what needs approval. Empty groups are
 *  left out. */
export function mayGroups(touches: Touch[] = [], settings: SettingChange[] = [], lines: string[] = []): MayGroup[] {
  const groups: MayGroup[] = [
    { key: "undecided", title: "Can't be decided here", rows: [] },
    { key: "approve", title: "Needs your approval", rows: [] },
    { key: "same", title: "Unchanged, or the host allows it", rows: [] },
    { key: "gone", title: "No longer", rows: [] },
  ];
  for (const t of touches) {
    const [, line] = touchLines([t]);
    groups.find((g) => g.key === groupOf(t))!.rows.push({ line, anchor: anchorOf(t, lines) });
  }
  const approve = groups.find((g) => g.key === "approve")!;
  for (const line of touchLines([], settings).slice(1)) approve.rows.push({ line, anchor: null });
  return groups.filter((g) => g.rows.length > 0);
}
