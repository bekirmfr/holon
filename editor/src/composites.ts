// A composite's inside, as canvases: its children rebuilt as nodes (ungroup puts them in place of
// it; expand opens them on a canvas of their own, with its boundary drawn as two nodes), and such
// a canvas, once edited, frozen back into a new composite and put in place of the old one, its
// wires following renamed ports. The grouping it undoes is canvasOps.ts's.

import { addEdge, type Edge, type Node } from "@xyflow/react";
import { BOUNDARY, buildPreimage, compatible, Evaluator, type WPort, type WUnit } from "./holon";
import type { PrimitiveData, PrimitiveNodeType } from "./PrimitiveNode";
import { outputColor } from "./theme";
import { isWEncapError } from "./diagnostics";
import { groupOf, type OpError } from "./canvasOps";

// Expand a placed composite node back into its children (P4.3.1's dual of grouping). One level
// only, deliberately: a nested composite child is revealed still collapsed, as its own composite
// node. Deep, read-only descent is the inspect panel's job.
/** What a composite was called on a canvas, by hash: a unit keeps no names, the editor does. */
export type NameOf = (hash: string) => string | undefined;

/** A composite's children as canvas nodes, rebuilt from their units (not yet placed). A nested
 *  composite takes the name it had on a canvas (`nameOf`), else the start of its hash. */
function childNodes(childHashes: string[], ev: Evaluator, byName: Map<string, WUnit>, variadic: Set<string>, nextId: () => number, nameOf?: NameOf): PrimitiveNodeType[] {
  return childHashes.map((h) => {
    const cu = ev.get_unit(h) as WUnit;
    const data: PrimitiveData =
      cu.body.kind === "Primitive"
        ? {
            name: cu.body.name,
            body: { kind: "Primitive" },
            inputs: cu.inputs ?? [],
            outputs: cu.outputs ?? [],
            params: cu.params ?? [],
            variadic: variadic.has(cu.body.name),
            // The catalog's own declared arity, not this (possibly grown-then-frozen) child's
            // current one — the same floor a palette drop enforces.
            minInputs: (byName.get(cu.body.name)?.inputs ?? cu.inputs ?? []).length,
          }
        : {
            name: nameOf?.(h) ?? h.slice(0, 8),
            body: { kind: "Composite", hash: h },
            inputs: cu.inputs ?? [],
            outputs: cu.outputs ?? [],
            params: [],
            variadic: false,
            minInputs: 0,
          };
    const id = cu.body.kind === "Primitive" ? `${cu.body.name}-${nextId()}` : `composite-${nextId()}-${h.slice(0, 8)}`;
    return { id, type: "primitive", position: { x: 0, y: 0 }, data };
  });
}

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
  byName: Map<string, WUnit>,
  variadic: Set<string>,
  nameOf?: NameOf,
): { ok: true; nodes: Node[]; edges: Edge[]; childIds: string[] } | { ok: false; message: string } {
  if (target.data.body.kind !== "Composite") return { ok: false, message: "not a composite" };
  try {
    const unit = ev.get_unit(target.data.body.hash) as WUnit;
    if (unit.body.kind !== "Composite") return { ok: false, message: "not a composite" };
    let n = 0;
    const children = childNodes(unit.body.children, ev, byName, variadic, () => n++, nameOf).map((c) => ({ ...c, data: { ...c.data, readOnly: true } }));
    const inner = unit.body.edges;

    // Columns by how far data has come: a child fed only by the boundary is in column 1.
    const depth = new Array<number>(children.length).fill(1);
    for (let pass = 0; pass < children.length; pass++) {
      for (const e of inner) {
        if (e.from_child !== BOUNDARY && e.to_child !== BOUNDARY) depth[e.to_child] = Math.max(depth[e.to_child], depth[e.from_child] + 1);
      }
    }
    const last = Math.max(1, ...depth) + 1;
    const rows = new Map<number, number>();
    const COLUMN = 250;
    const ROW = 150;
    children.forEach((c, i) => {
      const row = rows.get(depth[i]) ?? 0;
      rows.set(depth[i], row + 1);
      c.position = { x: depth[i] * COLUMN, y: row * ROW };
    });

    // What each boundary port is wired to outside: the node (with its port when it has
    // several), or nothing.
    const label = (id: string, handle: string | null | undefined, side: "out" | "in") => {
      const node = nodes.find((x) => x.id === id);
      if (!node) return id;
      const ports = side === "out" ? node.data.outputs : node.data.inputs;
      return ports.length > 1 && handle ? `${node.data.name}.${handle}` : node.data.name;
    };
    const ins: BoundaryPort[] = target.data.inputs.map((p, i) => ({
      name: p.name,
      was: p.name,
      unit: unit.inputs?.[i]?.name,
      ty: p.ty,
      outside: edges.filter((e) => e.target === target.id && e.targetHandle === p.name).map((e) => label(e.source, e.sourceHandle, "out")),
    }));
    const outs: BoundaryPort[] = target.data.outputs.map((p, i) => ({
      name: p.name,
      was: p.name,
      unit: unit.outputs?.[i]?.name,
      ty: p.ty,
      outside: edges.filter((e) => e.source === target.id && e.sourceHandle === p.name).map((e) => label(e.target, e.targetHandle, "in")),
    }));
    const middle = ((Math.max(1, ...rows.values()) - 1) * ROW) / 2;
    const inData: BoundaryData = { side: "in", ports: ins };
    const outData: BoundaryData = { side: "out", ports: outs };
    const boundary: Node[] = [
      { id: BOUNDARY_IN, type: "boundary", position: { x: -60, y: middle }, data: inData },
      { id: BOUNDARY_OUT, type: "boundary", position: { x: last * COLUMN, y: middle }, data: outData },
    ];

    const wires: Edge[] = inner.flatMap((e, i): Edge[] => {
      const from =
        e.from_child === BOUNDARY
          ? { id: BOUNDARY_IN, port: target.data.inputs[e.from_port] }
          : { id: children[e.from_child]?.id, port: children[e.from_child]?.data.outputs[e.from_port] };
      const to =
        e.to_child === BOUNDARY
          ? { id: BOUNDARY_OUT, port: target.data.outputs[e.to_port] }
          : { id: children[e.to_child]?.id, port: children[e.to_child]?.data.inputs[e.to_port] };
      if (!from.id || !from.port || !to.id || !to.port) return [];
      return [
        {
          id: `inner-${i}`,
          source: from.id,
          sourceHandle: from.port.name,
          target: to.id,
          targetHandle: to.port.name,
          style: { stroke: outputColor(from.port.ty), strokeWidth: 1.6 },
        },
      ];
    });
    return { ok: true, nodes: [...boundary, ...children], edges: wires, childIds: children.map((c) => c.id) };
  } catch (err) {
    return { ok: false, message: err instanceof Error ? err.message : String(err) };
  }
}

export function expandComposite(
  target: PrimitiveNodeType,
  edges: Edge[],
  ev: Evaluator,
  byName: Map<string, WUnit>,
  variadic: Set<string>,
  nextId: () => number,
  nameOf?: NameOf,
): { ok: true; nodes: PrimitiveNodeType[]; edges: Edge[] } | ({ ok: false } & OpError) {
  if (target.data.body.kind !== "Composite") return { ok: false, message: "expand: not a composite", nodeIds: [] };
  try {
    const wunit = ev.get_unit(target.data.body.hash) as WUnit;
    if (wunit.body.kind !== "Composite") throw new Error("expand: not a composite");
    const childHashes = wunit.body.children;
    const innerEdges = wunit.body.edges;

    // Each child rebuilt fresh: a primitive regains full editability (indistinguishable from
    // one just dropped from the palette), a nested composite stays a placed reference.
    const spacing = 220;
    const startX = target.position.x - ((childHashes.length - 1) * spacing) / 2;
    const newNodes = childNodes(childHashes, ev, byName, variadic, nextId, nameOf).map((n, i) => ({
      ...n,
      position: { x: startX + i * spacing, y: target.position.y },
    }));

    // Canvas edges that pointed at the composite's ports are re-pointed at the right child's
    // handle, by following the boundary edge core built for that port.
    const rewired: Edge[] = edges.flatMap((e): Edge[] => {
      if (e.target === target.id) {
        const inIdx = target.data.inputs.findIndex((p) => p.name === e.targetHandle);
        const be = innerEdges.find((x) => x.from_child === BOUNDARY && x.from_port === inIdx);
        if (!be) return []; // shouldn't happen against an honest unit; drop defensively
        const child = newNodes[be.to_child];
        const handle = child.data.inputs[be.to_port]?.name;
        if (!handle) return [];
        return [{ ...e, target: child.id, targetHandle: handle }];
      }
      if (e.source === target.id) {
        const outIdx = target.data.outputs.findIndex((p) => p.name === e.sourceHandle);
        const be = innerEdges.find((x) => x.to_child === BOUNDARY && x.to_port === outIdx);
        if (!be) return [];
        const child = newNodes[be.from_child];
        const handle = child.data.outputs[be.from_port]?.name;
        if (!handle) return [];
        return [{ ...e, source: child.id, sourceHandle: handle }];
      }
      return [e];
    });

    // The composite's inner wiring becomes real canvas edges, through addEdge like a drawn wire,
    // so they get ids and the same type-coloured styling.
    const withInner = innerEdges
      .filter((e) => e.from_child !== BOUNDARY && e.to_child !== BOUNDARY)
      .reduce((acc: Edge[], e) => {
        const srcNode = newNodes[e.from_child];
        const tgtNode = newNodes[e.to_child];
        const srcPort = srcNode.data.outputs[e.from_port];
        const tgtHandle = tgtNode.data.inputs[e.to_port]?.name;
        if (!srcPort || !tgtHandle) return acc;
        const conn = { source: srcNode.id, sourceHandle: srcPort.name, target: tgtNode.id, targetHandle: tgtHandle };
        return addEdge({ ...conn, style: { stroke: outputColor(srcPort.ty), strokeWidth: 1.6 } }, acc);
      }, rewired);
    return { ok: true, nodes: newNodes, edges: withInner };
  } catch (err) {
    // get_unit fails as a plain string, and every hash here was resolved when the composite was
    // frozen, so there's no node to point at, just the message.
    return { ok: false, message: err instanceof Error ? err.message : String(err), nodeIds: [] };
  }
}

/**
 * An inner canvas (as `innerCanvas` opened it, then edited) frozen back into a composite: its
 * nodes the children, its wires the inner edges, and its boundary exactly as drawn, each input
 * of the inputs node promoting the one inner port it feeds and each output of the outputs node
 * exposing the inner output that feeds it, in their order and under their names (order and
 * names are part of the unit's identity, and the flow around it is wired by name). An input
 * wired to nothing, or to more than one port, and an output fed by nothing, are refused, saying
 * which; so is anything core's freeze refuses. A port keeps the unit's own name (`unit`), so an
 * unchanged canvas freezes back to the very same unit, and renaming a port only renames it on
 * the node. Answers the unit's hash and the node's ports (the boundary's names).
 */
export function freezeInner(
  canvas: { nodes: Node[]; edges: Edge[] },
  ev: Evaluator,
  byName: Map<string, WUnit>,
): { ok: true; hash: string; inputs: WPort[]; outputs: WPort[] } | { ok: false; message: string; nodeIds: string[] } {
  const boundaryOf = (id: string) => canvas.nodes.find((n) => n.id === id)?.data as BoundaryData | undefined;
  const ins = boundaryOf(BOUNDARY_IN)?.ports ?? [];
  const outs = boundaryOf(BOUNDARY_OUT)?.ports ?? [];
  const inner = canvas.nodes.filter((n) => n.id !== BOUNDARY_IN && n.id !== BOUNDARY_OUT) as PrimitiveNodeType[];
  if (inner.length === 0) return { ok: false, message: "nothing inside: a composite holds at least one node", nodeIds: [] };
  const index = new Map(inner.map((n, i) => [n.id, i]));
  const innerEdges = canvas.edges.filter((e) => index.has(e.source) && index.has(e.target));
  const promote: { child: number; port: number; name: string }[] = [];
  for (const p of ins) {
    const fed = canvas.edges.filter((e) => e.source === BOUNDARY_IN && e.sourceHandle === p.name);
    if (fed.length === 0) return { ok: false, message: `the input "${p.name}" isn't wired to anything inside: wire it, or remove it`, nodeIds: [] };
    if (fed.length > 1) {
      return {
        ok: false,
        message: `the input "${p.name}" feeds ${fed.length} ports: an input feeds one, so pass it through a node (a default, say) to share it`,
        nodeIds: fed.map((e) => e.target),
      };
    }
    const node = inner[index.get(fed[0].target)!];
    promote.push({ child: index.get(node.id)!, port: node.data.inputs.findIndex((q) => q.name === fed[0].targetHandle), name: p.unit ?? p.name });
  }
  const expose: { child: number; port: number; name: string }[] = [];
  for (const p of outs) {
    const from = canvas.edges.find((e) => e.target === BOUNDARY_OUT && e.targetHandle === p.name);
    if (!from) return { ok: false, message: `the output "${p.name}" isn't fed by anything inside: wire it, or remove it`, nodeIds: [] };
    const node = inner[index.get(from.source)!];
    expose.push({ child: index.get(node.id)!, port: node.data.outputs.findIndex((q) => q.name === from.sourceHandle), name: p.unit ?? p.name });
  }
  try {
    const { children, wedges } = groupOf(inner, innerEdges, ev, byName);
    const unit = ev.freeze_encapsulation(children, wedges, { promote, expose }) as WUnit;
    const hash = ev.insert(buildPreimage(unit));
    // The node keeps showing the boundary's names; the unit keeps its own.
    return {
      ok: true,
      hash,
      inputs: (unit.inputs ?? []).map((q, i) => ({ ...q, name: ins[i]?.name ?? q.name })),
      outputs: (unit.outputs ?? []).map((q, i) => ({ ...q, name: outs[i]?.name ?? q.name })),
    };
  } catch (err) {
    return { ok: false, message: isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err), nodeIds: [] };
  }
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
