// Pure helpers for saved flow documents. Like diagnostics.ts, this imports only TYPES from
// ./holon, so it can be tested without loading the wasm package.

import type { WUnit } from "./holon";

// Every hash reachable from `roots` (children, plus a `map`/`fold` `fn` param, the same closure
// stageClosure walks), each listed once, children before the units that reference them. A saved
// flow carries these units' preimages: a grouped composite exists only in the in-memory
// Evaluator, so without them a reopened flow references hashes nothing can resolve.
export function closureOf(roots: string[], getUnit: (hash: string) => WUnit): string[] {
  const seen = new Set<string>();
  const order: string[] = [];
  const visit = (hash: string) => {
    if (seen.has(hash)) return;
    seen.add(hash);
    const unit = getUnit(hash);
    if (unit.body.kind === "Composite") unit.body.children.forEach(visit);
    const fn = (unit.params ?? []).find((p) => p.kind === "Frozen" && p.name === "fn");
    if (fn && fn.kind === "Frozen") visit(fn.value);
    order.push(hash);
  };
  roots.forEach(visit);
  return order;
}

// What a projection depends on: each node's id and data (primitive, params, ports, composite
// hash) and each wire's endpoints. Positions, selection and drag state are left out, so moving a
// node keeps a projection current; any edit that could change what it computes does not.
export function structuralKey(
  nodes: { id: string; data: unknown }[],
  edges: { source: string; sourceHandle?: string | null; target: string; targetHandle?: string | null }[],
): string {
  const ns = nodes.map((n) => [n.id, n.data] as const).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const es = edges
    .map((e) => [e.source, e.sourceHandle ?? null, e.target, e.targetHandle ?? null])
    .sort((a, b) => (JSON.stringify(a) < JSON.stringify(b) ? -1 : 1));
  return JSON.stringify([ns, es]);
}

// What saving a flow would record: the structure plus each node's position. Selection, drag
// state and React Flow's own measurements are left out, so only a real change marks the canvas
// as unsaved.
export function documentKey(
  nodes: { id: string; data: unknown; position: { x: number; y: number } }[],
  edges: Parameters<typeof structuralKey>[1],
): string {
  const positions = nodes
    .map((n) => [n.id, n.position.x, n.position.y] as const)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return JSON.stringify([structuralKey(nodes, edges), positions]);
}

export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function fromHex(hex: string): Uint8Array {
  if (hex.length % 2 !== 0 || /[^0-9a-f]/i.test(hex)) throw new Error("not a hex string");
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/**
 * What opening a saved flow from the flows page does, given the flow on the canvas: go back to
 * it (`"resume"`, when it is that same flow with unsaved changes, so they are never thrown away
 * by a plain "open"), ask before discarding them (`"confirm"`, another flow while this one has
 * changes), or just load it.
 */
export function openChoice(target: string, current: { name: string | null; dirty: boolean }): "resume" | "confirm" | "load" {
  if (!current.dirty) return "load";
  return target === current.name ? "resume" : "confirm";
}
