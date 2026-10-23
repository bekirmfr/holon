// Saved flows used as holons in other flows: which saved flows and deployments place which, from
// which save, and what a new save of a holon takes away from those that place it. A save and a
// deploy tell the server what they place (`usesIn`, sent beside the document), and its listings
// say it back, so this is answered from the listings alone; a flow saved before that, which has
// no `uses`, is read once to find them. Pure but for that read, so it is tested without the
// server.

import type { WPort } from "./holon";
import { typeLabel } from "./theme";

/** One saved flow placed as a node: its name, and which save of it. */
export type Placed = { flow: string; saved_at: number };

/** Who places a holon: a saved flow or a deployment, how many times, and whether from the
 *  holon's newest save. */
export type HolonUse = { user: string; kind: "flow" | "deployment"; nodes: number; current: boolean };

type Sourced = { data?: { source?: { flow: string; savedAt: number } } };

/** What a flow document places: each node placed from a saved flow, with which save. */
export function usesIn(doc: { nodes: unknown[] }): Placed[] {
  return (doc.nodes as Sourced[]).flatMap((n) => (n.data?.source ? [{ flow: n.data.source.flow, saved_at: n.data.source.savedAt }] : []));
}

/** A saved flow or a deployment as listed, with what it places (absent: not recorded). */
type Listed = { name: string; kind: "flow" | "deployment"; uses?: Placed[] };

/** For each saved flow placed in others, those others (saved flows first, each sorted by name).
 *  `savedAt`: each saved flow's newest save, to tell which places an older one. */
export function holonUses(listed: Listed[], savedAt: Map<string, number>): Map<string, HolonUse[]> {
  const uses = new Map<string, HolonUse[]>();
  const order = [...listed].sort((a, b) => (a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === "flow" ? -1 : 1));
  for (const l of order) {
    const placed = new Map<string, HolonUse>();
    for (const u of l.uses ?? []) {
      if (l.kind === "flow" && u.flow === l.name) continue;
      const use = placed.get(u.flow) ?? { user: l.name, kind: l.kind, nodes: 0, current: true };
      use.nodes += 1;
      // Behind if any of its nodes is from an older save than the holon's newest.
      if (u.saved_at !== savedAt.get(u.flow)) use.current = false;
      placed.set(u.flow, use);
    }
    for (const [flow, use] of placed) uses.set(flow, [...(uses.get(flow) ?? []), use]);
  }
  return uses;
}

/** Who places what, from the server's listings: saved flows and deployments, a flow listed
 *  without `uses` (saved before they were recorded) read to find them. */
export async function holonIndex(
  flows: { name: string; saved_at_epoch_seconds: number; uses?: Placed[] }[],
  deployments: { slug: string; uses?: Placed[] }[],
  documentOf: (name: string) => Promise<{ nodes: unknown[] }>,
): Promise<Map<string, HolonUse[]>> {
  const listed: Listed[] = await Promise.all(
    flows.map(async (f) => ({ name: f.name, kind: "flow" as const, uses: f.uses ?? usesIn(await documentOf(f.name).catch(() => ({ nodes: [] }))) })),
  );
  for (const d of deployments) listed.push({ name: d.slug, kind: "deployment", uses: d.uses });
  return holonUses(listed, new Map(flows.map((f) => [f.name, f.saved_at_epoch_seconds])));
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
