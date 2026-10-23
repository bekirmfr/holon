// The canvas's structural operations, as functions of the canvas: project it into one composite,
// group a selection into a placed composite, expand a placed composite back into its children.
// Each reads what core computed (the boundary edges) rather than re-deciding it, and leaves
// setting React state to the caller.

import { addEdge, type Edge, type Node } from "@xyflow/react";
import { BOUNDARY, buildPreimage, compatible, Evaluator, type WEdge, type WEncapSpec, type WPort, type WUnit, type WValue } from "./holon";
import type { PrimitiveData, PrimitiveNodeType } from "./PrimitiveNode";
import { boundaryLabels, encapErrorNodeIndices, isWEncapError } from "./diagnostics";
import { isTemplate, templateDefaults, templateInterface } from "./templates";
import { outputColor } from "./theme";
import { unitFor } from "./canvasUnits";
import { defaultBoundaryValue } from "./BoundaryField";
import { findRoutes, routeSpec } from "./derive";
import { endpointUnit, serverUnit, type EndpointDraft } from "./endpoint";
import { fromHex } from "./flowDoc";

/** The unit a flow deploys as: its server when it has routes, else the whole canvas as one
 *  composite. Throws why it can't be derived. */
export function flowUnitHash(ns: PrimitiveNodeType[], es: Edge[], ev: Evaluator, byName: Map<string, WUnit>, portText: string): string {
  const derived = deriveServer(ns, es, ev, byName, portText);
  if (derived.errors.length > 0) throw new Error(derived.errors.join("; "));
  if (derived.endpoints.length > 0) {
    if (!derived.serverHash) throw new Error(`"${portText}" is not a port (set it under ⚙)`);
    return derived.serverHash;
  }
  if (ns.length === 0) throw new Error("the canvas is empty");
  const { children, wedges } = groupOf(ns, es, ev, byName);
  const whole = ev.freeze_encapsulation(children, wedges, ev.propose_encapsulation(children, wedges) as WEncapSpec) as WUnit;
  return ev.insert(buildPreimage(whole));
}

/** A group of canvas nodes as core children (each node's hash) plus the wires among them, by
 *  index. */
export function groupOf(group: PrimitiveNodeType[], es: Edge[], ev: Evaluator, byName: Map<string, WUnit>) {
  const indexOf = new Map(group.map((n, i) => [n.id, i]));
  const children = group.map((n) => unitFor(n, ev, byName));
  const wedges: WEdge[] = es
    .filter((e) => indexOf.has(e.source) && indexOf.has(e.target))
    .map((e) => {
      const from = indexOf.get(e.source)!;
      const to = indexOf.get(e.target)!;
      return {
        from_child: from,
        from_port: group[from].data.outputs.findIndex((p) => p.name === e.sourceHandle),
        to_child: to,
        to_port: group[to].data.inputs.findIndex((p) => p.name === e.targetHandle),
      };
    });
  return { children, wedges };
}

/** Which canvas node each child of a frozen unit is, in the order the unit is stored (and run,
 *  and timed) in: core sorts a composite's children canonically when it hashes it. */
export function traceOfUnit(unit: WUnit, group: PrimitiveNodeType[], ev: Evaluator): string[] {
  return unit.body.kind === "Composite"
    ? Array.from(ev.canonical_child_order(unit.body.children, unit.body.edges), (i) => group[i].id)
    : [];
}

/** The server a canvas declares: its endpoints, why any route can't be one, the server unit's
 *  hash once every route is sound and `portText` is a port, and the trace from each route's
 *  stored children to canvas nodes. */
export type DerivedServer = {
  endpoints: EndpointDraft[];
  errors: string[];
  serverHash: string | null;
  trace: Record<string, string[]>;
};

/** Derive the server from the canvas: each connected group holding a request node is one route
 *  (derive.ts). Each group is projected through core like a selection being grouped, narrowed to
 *  the request input and the response/effect outputs, and wrapped as an endpoint. */
export function deriveServer(ns: PrimitiveNodeType[], es: Edge[], ev: Evaluator, byName: Map<string, WUnit>, portText: string): DerivedServer {
  const { routes, errors } = findRoutes(ns, es);
  const endpoints: EndpointDraft[] = [];
  const trace: Record<string, string[]> = {};
  for (const r of routes) {
    const label = `${r.method} ${r.path}`;
    try {
      const group = r.nodeIds.map((id) => ns.find((n) => n.id === id)!);
      const { children, wedges } = groupOf(group, es, ev, byName);
      const spec = routeSpec(ev.propose_encapsulation(children, wedges) as WEncapSpec, group, label);
      if (typeof spec === "string") {
        errors.push(spec);
        continue;
      }
      const handler = ev.freeze_encapsulation(children, wedges, spec) as WUnit;
      trace[label] = traceOfUnit(handler, group, ev);
      const handlerHash = ev.insert(buildPreimage(handler));
      const unit = endpointUnit(handlerHash, handler.inputs ?? [], handler.outputs ?? [], r.method, r.path, r.resume);
      if (typeof unit === "string") {
        errors.push(`${label}: ${unit}`);
        continue;
      }
      endpoints.push({ method: r.method, path: r.path, hash: ev.insert(buildPreimage(unit)) });
    } catch (err) {
      errors.push(`${label}: ${messageOf(err)}`);
    }
  }
  const port = Number(portText);
  const serverHash =
    endpoints.length > 0 && errors.length === 0 && Number.isInteger(port) && port >= 1 && port <= 65535
      ? ev.insert(buildPreimage(serverUnit(endpoints.map((e) => e.hash), port)))
      : null;
  return { endpoints, errors, serverHash, trace };
}

/** A projected canvas: the frozen composite, its promoted inputs with the values supplied for
 *  them, and its outputs with their current results. */
export type Projection = {
  hash: string;
  pre: Uint8Array;
  ports: WPort[];
  source: (string | null)[];
  values: (WValue | null)[];
  outputs: WPort[];
  resultSource: (string | null)[];
  results: (WValue | null)[];
  // The exact inputs this projection was built from, held so excludeOutput can re-freeze
  // without re-deriving from the (possibly since-changed) canvas — the same "frozen at project
  // time" reasoning `pre` already follows for boundary re-eval.
  children: string[];
  wedges: WEdge[];
  spec: WEncapSpec;
  // The canvas's structuralKey when this was projected. Once the canvas no longer matches,
  // the projection is dropped rather than left on screen, deployable.
  basis: string;
};

/** A failed operation: the message, and which placed nodes it implicates (from the structured
 *  error's own indices, never guessed; empty when there's nothing to point at). */
export type OpError = { message: string; nodeIds: string[] };

const messageOf = (err: unknown) => (isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err));

function failure(err: unknown, wedges: WEdge[], nodes: PrimitiveNodeType[]): OpError {
  // Highlighting is a nicety layered on top of the message — never let it break error display
  // itself if a shape ever slips past isWEncapError unexpectedly.
  let nodeIds: string[] = [];
  try {
    const indices = isWEncapError(err) ? encapErrorNodeIndices(err, wedges) : [];
    nodeIds = indices.map((i) => nodes[i]?.id).filter((id): id is string => id !== undefined);
  } catch {
    nodeIds = [];
  }
  return { message: messageOf(err), nodeIds };
}

// ── Projection ────────────────────────────────────────────────────────────────────────────────
// On demand, turn the wired canvas into a real core composite and show its content hash. The editor
// only marshals STRUCTURE — children by index, edges by index, handle name → port index — then
// hands it to core's default_spec + derive_and_freeze, which derive the boundary, validate, and
// freeze. The editor re-decides none of it (one decider). Cosmetic move ⇒ same hash; rewire ⇒ new
// hash; an ambiguous or invalid graph ⇒ a surfaced error, never a guess.
//
// A composite's promoted inputs (`ports`) and the values currently supplied for them (`values`,
// same length/order, `null` = unsupplied) are held alongside the frozen preimage (`pre`) so a
// boundary field's onChange can re-eval without re-projecting. `source[i]` is which PLACED NODE
// promoted input `i` actually came from — read directly off the composite's own boundary edges
// (`from_child === BOUNDARY`), not guessed. A CLOSED composite is just the zero-input case of this
// same shape — `ports: []`, nothing special-cased.
//
// `outputs`/`resultSource`/`results` are the exact dual, on the output side (P4.3, fork 2):
// `default_spec` proposes every free output automatically, so `results` may hold several values,
// not one — a composite with zero free outputs is likewise just the empty case (`outputs: []`).
export function projectCanvas(
  nodes: PrimitiveNodeType[],
  edges: Edge[],
  ev: Evaluator,
  byName: Map<string, WUnit>,
  basis: string,
): { ok: true; projection: Projection } | ({ ok: false } & OpError) {
  // Outside the try on purpose: TypeMismatch only carries an `edge` index, and resolving it
  // back to node ids needs this same array.
  let wedges: WEdge[] = [];
  try {
    // Child index = position in `nodes` — arbitrary, because core canonicalizes child order, so a
    // cosmetic reorder cannot move the hash (I5). `id`/`position` are dropped here.
    const indexOf = new Map(nodes.map((n, i) => [n.id, i]));
    const children = nodes.map((n) => unitFor(n, ev, byName));

    // Canvas wire → core edge by index. from_port is the named output's position in the source's
    // interface, to_port the named input's position in the target's.
    wedges = edges.map((e) => {
      const from = indexOf.get(e.source);
      const to = indexOf.get(e.target);
      if (from === undefined || to === undefined) {
        throw new Error("a wire references an unknown node");
      }
      const fromPort = nodes[from].data.outputs.findIndex((p) => p.name === e.sourceHandle);
      if (fromPort < 0) throw new Error(`unknown source port "${String(e.sourceHandle)}"`);
      const toPort = nodes[to].data.inputs.findIndex((p) => p.name === e.targetHandle);
      if (toPort < 0) throw new Error(`unknown target port "${String(e.targetHandle)}"`);
      return { from_child: from, from_port: fromPort, to_child: to, to_port: toPort };
    });

    // Core proposes the boundary (default_spec) and — unedited — freezes it (derive_and_freeze).
    // Split apart so excludeOutput can re-freeze with fewer exposed outputs without re-deriving
    // the proposal (P4.3, fork 3). An EncapError throws the structured error object itself, so
    // the failure can point at the node(s) at fault.
    const spec = ev.propose_encapsulation(children, wedges) as WEncapSpec;
    const composite = ev.freeze_encapsulation(children, wedges, spec) as WUnit;
    const pre = buildPreimage(composite);
    const hash = ev.insert(pre);

    // Evaluated from each promoted input's zero value (or `null` for a non-scalar port — "wire
    // in"). `null` in a result slot ⇒ that output isn't ready, independently of its siblings; a
    // sink graph yields the Effect it constructs (pure eval doesn't fire it).
    const ports = composite.inputs ?? [];
    // `nodes[to_child]` is safe because `children` was built from `nodes` in the same order.
    const boundaryEdges = composite.body.kind === "Composite" ? composite.body.edges : [];
    const source: (string | null)[] = ports.map((_, i) => {
      const edge = boundaryEdges.find((e) => e.from_child === BOUNDARY && e.from_port === i);
      return edge ? (nodes[edge.to_child]?.data.name ?? null) : null;
    });
    const values = ports.map((p) => defaultBoundaryValue(p.ty));
    const outputs = composite.outputs ?? [];
    const resultSource: (string | null)[] = outputs.map((_, i) => {
      const edge = boundaryEdges.find((e) => e.to_child === BOUNDARY && e.to_port === i);
      return edge ? (nodes[edge.from_child]?.data.name ?? null) : null;
    });
    const results = ev.eval(pre, values) as (WValue | null)[];
    return {
      ok: true,
      projection: { hash, pre, ports, source, values, outputs, resultSource, results, children, wedges, spec, basis },
    };
  } catch (err) {
    return { ok: false, ...failure(err, wedges, nodes) };
  }
}

/** Exclude one exposed output and re-freeze (P4.3, fork 3): a real authoring choice that changes
 *  the interface, so the hash too. `outputs`/`resultSource` line up 1:1 with `spec.expose` by
 *  construction. No "re-include": projecting again recovers the original proposal. */
export function excludeProjectedOutput(p: Projection, index: number, ev: Evaluator): Projection {
  try {
    const expose = p.spec.expose.filter((_, i) => i !== index);
    const spec: WEncapSpec = { promote: p.spec.promote, expose };
    const composite = ev.freeze_encapsulation(p.children, p.wedges, spec) as WUnit;
    const pre = buildPreimage(composite);
    const hash = ev.insert(pre);
    const outputs = p.outputs.filter((_, i) => i !== index);
    const resultSource = p.resultSource.filter((_, i) => i !== index);
    const results = ev.eval(pre, p.values) as (WValue | null)[];
    return { ...p, hash, pre, outputs, resultSource, results, spec };
  } catch {
    // Dropping one entry from an already-valid spec can't create a new validation failure. If it
    // somehow does, leave the projection exactly as it was rather than silently corrupt it.
    return p;
  }
}

// Group a selection into one placed composite node (P4.3.1). Mechanically the same derivation as
// projecting, just fed a subset: only the selected nodes become children, only edges where BOTH
// endpoints are selected become the composite's inner wiring — an edge with exactly one endpoint
// outside the selection is, from the subgraph's own perspective, an ordinary free input or output,
// so default_spec's proposal derives the right boundary with no new core or bridge logic.
export function groupSelection(
  selected: PrimitiveNodeType[],
  edges: Edge[],
  ev: Evaluator,
  byName: Map<string, WUnit>,
  nextId: () => number,
): { ok: true; node: PrimitiveNodeType; edges: Edge[] } | ({ ok: false } & OpError) {
  const selectedIds = new Set(selected.map((n) => n.id));
  const indexOf = new Map(selected.map((n, i) => [n.id, i]));
  let wedges: WEdge[] = [];
  try {
    const children = selected.map((n) => unitFor(n, ev, byName));
    const innerEdges = edges.filter((e) => selectedIds.has(e.source) && selectedIds.has(e.target));
    wedges = innerEdges.map((e) => {
      const from = indexOf.get(e.source)!;
      const to = indexOf.get(e.target)!;
      const fromPort = selected[from].data.outputs.findIndex((p) => p.name === e.sourceHandle);
      const toPort = selected[to].data.inputs.findIndex((p) => p.name === e.targetHandle);
      return { from_child: from, from_port: fromPort, to_child: to, to_port: toPort };
    });

    const composite = ev.project(children, wedges) as WUnit;
    const hash = ev.insert(buildPreimage(composite));
    const boundaryEdges = composite.body.kind === "Composite" ? composite.body.edges : [];
    const rawInputs = composite.inputs ?? [];
    const rawOutputs = composite.outputs ?? [];

    // Which (selected-node, port) each promoted input / exposed output came from — read off the
    // boundary edges core computed. Purely index-based, so it works on the raw, possibly-colliding
    // ports; naming below doesn't change it, it only feeds it.
    const promotedFrom = rawInputs.map((_, i) => {
      const be = boundaryEdges.find((e) => e.from_child === BOUNDARY && e.from_port === i);
      return be ? { child: be.to_child, port: be.to_port } : null;
    });
    const exposedFrom = rawOutputs.map((_, i) => {
      const be = boundaryEdges.find((e) => e.to_child === BOUNDARY && e.to_port === i);
      return be ? { child: be.from_child, port: be.from_port } : null;
    });

    // Name the new node's ports by provenance ("add · b", not a bare "b_2"). These become the
    // ports' real handle ids, renamed after the hash is computed, so they never affect identity.
    const inputSource = promotedFrom.map((x) => (x ? selected[x.child]?.data.name ?? null : null));
    const outputSource = exposedFrom.map((x) => (x ? selected[x.child]?.data.name ?? null : null));
    const inputLabels = boundaryLabels(rawInputs, inputSource);
    const outputLabels = boundaryLabels(rawOutputs, outputSource);
    const inputs = rawInputs.map((p, i) => ({ ...p, name: inputLabels[i] }));
    const outputs = rawOutputs.map((p, i) => ({ ...p, name: outputLabels[i] }));

    const groupId = `composite-${nextId()}-${hash.slice(0, 8)}`;
    const node: PrimitiveNodeType = {
      id: groupId,
      type: "primitive",
      position: {
        x: selected.reduce((s, n) => s + n.position.x, 0) / selected.length,
        y: selected.reduce((s, n) => s + n.position.y, 0) / selected.length,
      },
      data: {
        name: hash.slice(0, 8),
        body: { kind: "Composite", hash },
        inputs,
        outputs,
        params: [],
        variadic: false,
        minInputs: 0,
      },
    };

    // Rewire every edge that crossed the selection boundary to the new node's corresponding
    // handle; drop every edge now inside the frozen composite; leave every other edge as it was.
    const nextEdges: Edge[] = edges.flatMap((e): Edge[] => {
      const sIn = selectedIds.has(e.source);
      const tIn = selectedIds.has(e.target);
      if (sIn && tIn) return [];
      if (sIn) {
        const ci = indexOf.get(e.source)!;
        const cp = selected[ci].data.outputs.findIndex((p) => p.name === e.sourceHandle);
        const oi = exposedFrom.findIndex((x) => x?.child === ci && x?.port === cp);
        if (oi < 0) return []; // shouldn't happen against an honest proposal; drop defensively
        return [{ ...e, source: groupId, sourceHandle: outputs[oi].name }];
      }
      if (tIn) {
        const ci = indexOf.get(e.target)!;
        const cp = selected[ci].data.inputs.findIndex((p) => p.name === e.targetHandle);
        const ii = promotedFrom.findIndex((x) => x?.child === ci && x?.port === cp);
        if (ii < 0) return [];
        return [{ ...e, target: groupId, targetHandle: inputs[ii].name }];
      }
      return [e];
    });
    return { ok: true, node, edges: nextEdges };
  } catch (err) {
    return { ok: false, ...failure(err, wedges, selected) };
  }
}

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
 * A saved flow placed in another as one node: a holon. Its whole canvas is grouped into one
 * composite exactly as "group" does, its free inputs and outputs becoming the node's ports, and
 * the node takes the flow's name. It is the flow as saved now (a unit is content-addressed, so
 * later edits to the flow make a new unit and leave this one as it is). The flow's own composites
 * are put back into `ev` first. A flow with routes is a server, answered by the host, not a part
 * of another flow: it is refused, as is an empty one.
 */
export function flowAsNode(
  name: string,
  doc: { nodes: unknown[]; edges: unknown[]; units?: string[] },
  ev: Evaluator,
  byName: Map<string, WUnit>,
  id: string,
  position: { x: number; y: number },
  /** When the flow was saved: the node remembers it, so an update can be offered later. */
  savedAt?: number,
): { ok: true; node: PrimitiveNodeType } | { ok: false; message: string } {
  const ns = doc.nodes as PrimitiveNodeType[];
  const es = doc.edges as Edge[];
  if (ns.length === 0) return { ok: false, message: `"${name}" is empty: nothing to place` };
  if (ns.some((n) => n.data.body.kind === "Template" && (n.data.name === "request" || n.data.name === "respond"))) {
    return { ok: false, message: `"${name}" has routes: it is a server, deployed on its own, not a node of another flow` };
  }
  try {
    for (const hex of doc.units ?? []) ev.insert(fromHex(hex));
  } catch (e) {
    return { ok: false, message: `"${name}": its grouped nodes can't be restored: ${e instanceof Error ? e.message : String(e)}` };
  }
  const grouped = groupSelection(ns, es, ev, byName, () => 0);
  if (!grouped.ok) return { ok: false, message: `"${name}" can't be placed as a node: ${grouped.message}` };
  const source = savedAt === undefined ? {} : { source: { flow: name, savedAt } };
  return { ok: true, node: { ...grouped.node, id, position, data: { ...grouped.node.data, name, ...source } } };
}

/**
 * A node for a palette entry dropped at `position`: a template (templates.ts) or a genesis
 * primitive from the catalog, default-configured as the palette offers it. Null for a name that
 * is neither. (A saved flow is placed by `flowAsNode`.)
 */
export function nodeFromPalette(
  name: string,
  byName: Map<string, WUnit>,
  variadic: Set<string>,
  id: string,
  position: { x: number; y: number },
): PrimitiveNodeType | null {
  if (isTemplate(name)) {
    const params = templateDefaults(name);
    const iface = templateInterface(name, params);
    return {
      id,
      type: "primitive",
      position,
      data: {
        name,
        body: { kind: "Template" },
        inputs: iface.inputs,
        outputs: iface.outputs,
        params,
        variadic: false,
        minInputs: 0,
        hiddenInputs: iface.hiddenInputs,
      },
    };
  }
  const u = byName.get(name);
  if (!u) return null;
  const inputs = u.inputs ?? [];
  return {
    id,
    type: "primitive",
    position,
    data: {
      name,
      body: { kind: "Primitive" },
      inputs,
      outputs: u.outputs ?? [],
      params: u.params ?? [],
      // The catalog's own declared arity is the floor a variadic node may shrink back to.
      variadic: variadic.has(name),
      minInputs: inputs.length,
    },
  };
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
