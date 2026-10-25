// The text and the graph, linked by what each node is written as (docs/L4s/text-graph-link-L4.md):
// the node the cursor is on, and what of a text didn't run. Ranges are UTF-16 offsets, as
// CodeMirror counts them (compileHolon's `spans`).

import type { TextRange } from "./holonText";

/** The nodes the cursor at `at` is on: those with the smallest range holding it. Several nodes one
 *  expression lowers to (a `match` arm, a `when`) share its range and are picked together. */
export function pickAt(spans: Map<string, TextRange>, at: number): string[] {
  let best = Infinity;
  let picked: string[] = [];
  for (const [id, r] of spans) {
    if (r.to <= r.from || at < r.from || at > r.to) continue;
    const size = r.to - r.from;
    if (size < best) {
      best = size;
      picked = [id];
    } else if (size === best) picked.push(id);
  }
  return picked;
}

/** What to dim: each character whose innermost node (the smallest range over it) didn't run. Of
 *  several nodes written over the same range, one that ran keeps it lit. Characters no node is
 *  written over (keywords between statements, comments) are never dimmed. */
export function dimmed(spans: Map<string, TextRange>, ran: (id: string) => boolean): TextRange[] {
  const groups = new Map<string, { r: TextRange; ran: boolean }>();
  for (const [id, r] of spans) {
    if (r.to <= r.from) continue;
    const k = `${r.from}:${r.to}`;
    const g = groups.get(k) ?? { r, ran: false };
    g.ran ||= ran(id);
    groups.set(k, g);
  }
  // Widest first, so each narrower range overwrites the ones around it.
  const sorted = [...groups.values()].sort((a, b) => b.r.to - b.r.from - (a.r.to - a.r.from));
  const end = sorted.reduce((m, g) => Math.max(m, g.r.to), 0);
  const state = new Uint8Array(end); // 0: no node, 1: ran, 2: didn't
  for (const g of sorted) state.fill(g.ran ? 1 : 2, g.r.from, g.r.to);
  const out: TextRange[] = [];
  for (let i = 0; i < end; i++) {
    if (state[i] !== 2) continue;
    let j = i;
    while (j < end && state[j] === 2) j++;
    out.push({ from: i, to: j });
    i = j - 1;
  }
  return out;
}
