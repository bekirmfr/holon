// A composite's inside, as canvases: its children rebuilt as nodes (ungroup puts them in place of
// it; expand opens them on a canvas of their own, with its boundary drawn as two nodes), and such
// a canvas, once edited, frozen back into a new composite and put in place of the old one, its
// wires following renamed ports. Opening, expanding and freezing are holon-author's
// (crates/holon-author/src/composites.rs), through the wasm bridge; what's left here is the
// canvas's own bookkeeping (colours, renamed ports, the node replaced). The grouping it undoes is
// canvasOps.ts's.

import type { Edge, Node } from "@xyflow/react";
import { compatible, type Evaluator, type WPort, type WUnit } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { outputColor } from "./theme";
import type { OpError } from "./canvasOps";

/** What composites were called on a canvas, by hash: a unit keeps no names, the editor does. */
export type Names = Record<string, string>;

/** One port of a composite's boundary as its inner canvas shows it, with what it is wired to
 *  outside the composite, in the flow around it. */
export type BoundaryPort = {
  /** As the node shows it (renamable, not identity). */
  name: string;
  ty: WPort["ty"];
  outside: string[];
  /** The unit's own name for this port (part of its identity): kept when frozen again, so an
   *  unchanged composite freezes back to the very same unit. Absent for a port added inside. */
  unit?: string;
  /** Its name on the node when the canvas was opened, so the flow's wires to it follow a rename.
   *  Absent for a port added inside. */
  was?: string;
};
export type BoundaryData = { side: "in" | "out"; ports: BoundaryPort[] };
export const BOUNDARY_IN = "boundary-in";
export const BOUNDARY_OUT = "boundary-out";

/** A wire coloured by the type it carries, as a drawn wire is. */
const styled = (e: Edge, ty: WPort["ty"] | undefined): Edge => ({ ...e, style: { stroke: outputColor(ty ?? "Any"), strokeWidth: 1.6 } });

/**
 * A composite as a canvas of its own, to look inside it without ungrouping: its children as
 * nodes (read only), laid out left to right in the order data flows (a frozen unit keeps no
 * positions), its inner wires, and its boundary as two nodes, its inputs on the left and its
 * outputs on the right, each port labelled with what it is wired to in the flow around it
 * (`nodes`/`edges`). `childIds[i]` is the canvas id of child `i`, to match a run's inner steps.
 */
export function innerCanvas(
  target: PrimitiveNodeType,
  nodes: PrimitiveNodeType[],
  edges: Edge[],
  ev: Evaluator,
  names: Names = {},
): { ok: true; nodes: Node[]; edges: Edge[]; childIds: string[] } | { ok: false; message: string } {
  const opened = ev.inner_canvas(target, nodes, edges, names) as { ok: true; nodes: Node[]; edges: Edge[]; childIds: string[] } | { ok: false; message: string };
  if (!opened.ok) return { ok: false, message: opened.message };
  const tyOf = (e: Edge) =>
    e.source === BOUNDARY_IN
      ? target.data.inputs.find((p) => p.name === e.sourceHandle)?.ty
      : (opened.nodes.find((n) => n.id === e.source)?.data as PrimitiveNodeType["data"] | undefined)?.outputs.find((p) => p.name === e.sourceHandle)?.ty;
  return { ...opened, edges: opened.edges.map((e) => styled(e, tyOf(e))) };
}

/** A placed composite expanded back into its children, in its place (the dual of grouping). One
 *  level only: a nested composite is revealed still collapsed. The flow's wires to it are
 *  re-pointed at the child each port leads to; its inner wiring becomes wires of the flow. */
export function expandComposite(
  target: PrimitiveNodeType,
  edges: Edge[],
  ev: Evaluator,
  nextId: () => number,
  names: Names = {},
): { ok: true; nodes: PrimitiveNodeType[]; edges: Edge[] } | ({ ok: false } & OpError) {
  if (target.data.body.kind !== "Composite") return { ok: false, message: "expand: not a composite", nodeIds: [] };
  let ids: number[];
  try {
    const unit = ev.get_unit(target.data.body.hash) as WUnit;
    ids = unit.body.kind === "Composite" ? unit.body.children.map(() => nextId()) : [];
  } catch (err) {
    return { ok: false, message: err instanceof Error ? err.message : String(err), nodeIds: [] };
  }
  const result = ev.expand_composite(target, edges, Float64Array.from(ids), names) as
    | { ok: true; nodes: PrimitiveNodeType[]; rewired: Edge[]; added: Edge[] }
    | { ok: false; message: string; nodeIds: string[] };
  if (!result.ok) return result;
  const tyOf = (e: Edge) => result.nodes.find((n) => n.id === e.source)?.data.outputs.find((p) => p.name === e.sourceHandle)?.ty;
  return { ok: true, nodes: result.nodes, edges: [...result.rewired, ...result.added.map((e) => styled(e, tyOf(e)))] };
}

/**
 * An inner canvas (as `innerCanvas` opened it, then edited) frozen back into a composite: its
 * nodes the children, its wires the inner edges, and its boundary exactly as drawn, each input
 * of the inputs node promoting the one inner port it feeds and each output of the outputs node
 * exposing the inner output that feeds it, in their order and under their names. An input wired
 * to nothing, or to more than one port, and an output fed by nothing, are refused, saying which;
 * so is anything core's freeze refuses. A port keeps the unit's own name (`unit`), so an
 * unchanged canvas freezes back to the very same unit. Answers the unit's hash and the node's
 * ports (the boundary's names).
 */
export function freezeInner(
  canvas: { nodes: Node[]; edges: Edge[] },
  ev: Evaluator,
): { ok: true; hash: string; inputs: WPort[]; outputs: WPort[] } | { ok: false; message: string; nodeIds: string[] } {
  const boundaryOf = (id: string) => canvas.nodes.find((n) => n.id === id)?.data as BoundaryData | undefined;
  const inner = canvas.nodes.filter((n) => n.id !== BOUNDARY_IN && n.id !== BOUNDARY_OUT);
  return ev.freeze_inner(boundaryOf(BOUNDARY_IN)?.ports ?? [], boundaryOf(BOUNDARY_OUT)?.ports ?? [], inner, canvas.edges);
}

/** An inner canvas's boundary ports renamed since it was opened: old name → new, per side. */
export function boundaryRenames(canvas: { nodes: Node[] }): { inputs: Map<string, string>; outputs: Map<string, string> } {
  const side = (id: string) =>
    new Map(
      ((canvas.nodes.find((n) => n.id === id)?.data as BoundaryData | undefined)?.ports ?? [])
        .filter((p) => p.was !== undefined && p.was !== p.name)
        .map((p) => [p.was!, p.name] as [string, string]),
    );
  return { inputs: side(BOUNDARY_IN), outputs: side(BOUNDARY_OUT) };
}

/** An inner canvas once frozen into its composite: each boundary port now is what the composite
 *  has (its unit name, its name on the node), so a later freeze starts from there. */
export function settleBoundary<T extends { nodes: Node[] }>(canvas: T, frozen: { inputs: WPort[]; outputs: WPort[] }): T {
  return {
    ...canvas,
    nodes: canvas.nodes.map((n) => {
      if (n.id !== BOUNDARY_IN && n.id !== BOUNDARY_OUT) return n;
      const data = n.data as BoundaryData;
      const ports = n.id === BOUNDARY_IN ? frozen.inputs : frozen.outputs;
      return { ...n, data: { ...data, ports: data.ports.map((p, i) => ({ ...p, ty: ports[i]?.ty ?? p.ty, unit: p.unit ?? p.name, was: p.name })) } };
    }),
  };
}

/**
 * The composite node `id` replaced by a new version of itself (a frozen inner canvas): the same
 * node, place and name, its new unit and ports. Wires to a renamed port follow it; wires to a
 * port it no longer has, or now of a type they don't fit, are dropped (and answered, to say so).
 */
export function replaceComposite<N extends Node>(
  nodes: N[],
  edges: Edge[],
  id: string,
  frozen: { hash: string; inputs: WPort[]; outputs: WPort[] },
  renames: { inputs: Map<string, string>; outputs: Map<string, string> },
): { nodes: N[]; edges: Edge[]; dropped: Edge[] } {
  const ins = new Map(frozen.inputs.map((p) => [p.name, p]));
  const outs = new Map(frozen.outputs.map((p) => [p.name, p]));
  const portTy = (nodeId: string, handle: string | null | undefined, side: "inputs" | "outputs") => {
    const n = nodes.find((x) => x.id === nodeId);
    const ports = (n?.data as { inputs?: WPort[]; outputs?: WPort[] } | undefined)?.[side];
    return ports?.find((p) => p.name === handle)?.ty;
  };
  const kept: Edge[] = [];
  const dropped: Edge[] = [];
  for (const e of edges) {
    let next = e;
    if (e.target === id) {
      const handle = renames.inputs.get(e.targetHandle ?? "") ?? e.targetHandle ?? "";
      const port = ins.get(handle);
      const from = portTy(e.source, e.sourceHandle, "outputs");
      if (!port || (from !== undefined && !compatible(from, port.ty))) {
        dropped.push(e);
        continue;
      }
      next = { ...next, targetHandle: handle };
    }
    if (e.source === id) {
      const handle = renames.outputs.get(e.sourceHandle ?? "") ?? e.sourceHandle ?? "";
      const port = outs.get(handle);
      const to = portTy(e.target, e.targetHandle, "inputs");
      if (!port || (to !== undefined && !compatible(port.ty, to))) {
        dropped.push(e);
        continue;
      }
      next = { ...next, sourceHandle: handle };
    }
    kept.push(next);
  }
  return {
    nodes: nodes.map((n) =>
      n.id === id ? { ...n, data: { ...n.data, body: { kind: "Composite", hash: frozen.hash }, inputs: frozen.inputs, outputs: frozen.outputs } } : n,
    ),
    edges: kept,
    dropped,
  };
}
