// The canvas's structural operations, as functions of the canvas: project it into one composite,
// group a selection into a placed composite, place a palette entry or a saved flow. A composite's
// inside (ungroup, expand, freeze back) is composites.ts's.
// Each reads what core computed (the boundary edges) rather than re-deciding it, and leaves
// setting React state to the caller.

import type { Edge } from "@xyflow/react";
import { BOUNDARY, buildPreimage, Evaluator, type WEdge, type WEncapSpec, type WPort, type WUnit, type WValue } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { encapErrorNodeIndices, isWEncapError } from "./diagnostics";
import { isTemplate, templateDefaults, templateInterface } from "./templates";
import { unitFor } from "./canvasUnits";
import { defaultBoundaryValue } from "./BoundaryField";
import type { EndpointDraft } from "./endpoint";
import { restoreDocument } from "./docUnits";

// What a flow turns into (its nodes' units, its routes, its server, the unit it deploys as) is
// holon-author's (crates/holon-author/src/flow.rs), through the Evaluator, which stores every
// unit it derives: the same code the command line runs.

/** The unit a flow deploys as: its server when it has routes, else the whole canvas as one
 *  composite. Throws why it can't be derived. */
export function flowUnitHash(ns: PrimitiveNodeType[], es: Edge[], ev: Evaluator, portText: string): string {
  return ev.flow_unit(ns, es, portText);
}

/** A group of canvas nodes as core children (each node's hash) plus the wires among them, by
 *  index. */
export function groupOf(group: PrimitiveNodeType[], es: Edge[], ev: Evaluator): { children: string[]; wedges: WEdge[] } {
  return ev.group_of(group, es);
}

/** Which canvas node each child of a frozen unit is, in the order the unit is stored (and run,
 *  and timed) in: core sorts a composite's children canonically when it hashes it. */
export function traceOfUnit(hash: string, group: PrimitiveNodeType[], ev: Evaluator): string[] {
  return ev.trace_of(hash, group);
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

/** Derive the server from the canvas: each connected group holding a request node is one route.
 *  Each group is projected through core like a selection being grouped, narrowed to the request
 *  input and the response/effect outputs, and wrapped as an endpoint. */
export function deriveServer(ns: PrimitiveNodeType[], es: Edge[], ev: Evaluator, portText: string): DerivedServer {
  return ev.derive_server(ns, es, portText) as DerivedServer;
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
  basis: string,
): { ok: true; projection: Projection } | ({ ok: false } & OpError) {
  // Outside the try on purpose: TypeMismatch only carries an `edge` index, and resolving it
  // back to node ids needs this same array.
  let wedges: WEdge[] = [];
  try {
    // Child index = position in `nodes` — arbitrary, because core canonicalizes child order, so a
    // cosmetic reorder cannot move the hash (I5). `id`/`position` are dropped here.
    const indexOf = new Map(nodes.map((n, i) => [n.id, i]));
    const children = nodes.map((n) => unitFor(n, ev));

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

// Group a selection into one placed composite node: only the selected nodes become children, only
// wires with both ends selected its inner wiring; a wire with one end outside is, from the
// selection's side, an open input or output, so core's proposal derives the boundary. The new
// node's ports are named by provenance ("add · b"), and the wires crossing the selection follow
// them. holon-author's (group.rs), through the wasm bridge.
export function groupSelection(
  selected: PrimitiveNodeType[],
  edges: Edge[],
  ev: Evaluator,
  nextId: () => number,
): { ok: true; node: PrimitiveNodeType; edges: Edge[] } | ({ ok: false } & OpError) {
  return ev.group_selection(selected, edges, nextId());
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
  doc: { nodes: unknown[]; edges: unknown[]; units?: string[]; names?: Record<string, string> },
  ev: Evaluator,
  id: string,
  position: { x: number; y: number },
  /** When the flow was saved: the node remembers it, so an update can be offered later. */
  savedAt?: number,
): { ok: true; node: PrimitiveNodeType } | { ok: false; message: string } {
  const failed = doc.nodes.length > 0 ? restoreDocument(doc, ev) : null;
  if (failed) return { ok: false, message: `"${name}": its grouped nodes can't be restored: ${failed}` };
  return ev.flow_as_node(name, doc.nodes, doc.edges, id, position, savedAt);
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
