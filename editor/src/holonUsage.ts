// Saved flows used as holons in other saved flows: which flows place which, from which save, and
// what a new save of a holon takes away from the flows that place it. Pure, so it is tested
// without the server.

import type { WPort } from "./holon";
import { typeLabel } from "./theme";

/** A flow that places another as a node: how many times, and whether from its newest save. */
export type HolonUse = { user: string; nodes: number; current: boolean };

type Record = { name: string; saved_at_epoch_seconds: number; document: { nodes: unknown[] } };
type Sourced = { data?: { source?: { flow: string; savedAt: number } } };

/** For each saved flow placed as a node in others, those others (sorted by name). */
export function holonUses(records: Record[]): Map<string, HolonUse[]> {
  const savedAt = new Map(records.map((r) => [r.name, r.saved_at_epoch_seconds]));
  const uses = new Map<string, HolonUse[]>();
  for (const r of [...records].sort((a, b) => a.name.localeCompare(b.name))) {
    const placed = new Map<string, HolonUse>();
    for (const n of r.document.nodes as Sourced[]) {
      const source = n.data?.source;
      if (!source || source.flow === r.name) continue;
      const use = placed.get(source.flow) ?? { user: r.name, nodes: 0, current: true };
      use.nodes += 1;
      // Behind if any of its nodes is from an older save than the holon's newest.
      if (source.savedAt !== savedAt.get(source.flow)) use.current = false;
      placed.set(source.flow, use);
    }
    for (const [flow, use] of placed) uses.set(flow, [...(uses.get(flow) ?? []), use]);
  }
  return uses;
}

/** What a holon's new interface takes from its old: each port gone, or of another type now,
 *  e.g. `input "author" removed`, `output "slug" is now number (was text)`. */
export function interfaceLoss(before: { inputs: WPort[]; outputs: WPort[] }, after: { inputs: WPort[]; outputs: WPort[] }): string[] {
  const side = (kind: string, was: WPort[], now: WPort[]) =>
    was.flatMap((p) => {
      const q = now.find((x) => x.name === p.name);
      if (!q) return [`${kind} "${p.name}" removed`];
      return typeLabel(q.ty) === typeLabel(p.ty) ? [] : [`${kind} "${p.name}" is now ${typeLabel(q.ty)} (was ${typeLabel(p.ty)})`];
    });
  return [...side("input", before.inputs, after.inputs), ...side("output", before.outputs, after.outputs)];
}
