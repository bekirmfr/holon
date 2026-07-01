// The authoring canvas: a palette (derived from core's catalog) on the left, a blueprint canvas on
// the right. Drag a primitive across to place a typed node. This inch places nodes only — wiring and
// projection/eval are later inches (P4.1-palette-L4 §1). Nothing here re-asserts a primitive's shape;
// every placed node reads its ports from the catalog interface (I1).

import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent } from "react";
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  BackgroundVariant,
  Controls,
  Panel,
  useNodesState,
  useEdgesState,
  useReactFlow,
  addEdge,
  type NodeTypes,
  type Connection,
  type Edge,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";

import {
  ensureReady,
  primitiveCatalog,
  variadicPrimitives,
  compatible,
  buildPreimage,
  Evaluator,
  BOUNDARY,
  type WUnit,
  type WEdge,
  type WValue,
  type WPort,
  type WType,
  type WEncapError,
  type WEncapSpec,
  type WValidationError,
} from "./holon";
import { outputColor, typeColor, typeLabel } from "./theme";
import { Palette, DND_MIME } from "./Palette";
import { PrimitiveNode, type PrimitiveData, type PrimitiveNodeType } from "./PrimitiveNode";

const nodeTypes = { primitive: PrimitiveNode } as unknown as NodeTypes;

function primitiveName(u: WUnit): string | null {
  return u.body.kind === "Primitive" ? u.body.name : null;
}

// Render an evaluated value for the readout. This is core's *pure* evaluator, so a sink shows as the
// Effect it constructs (it doesn't fire here), and a driver / unparsed-const graph comes back null.
function formatValue(v: WValue): string {
  switch (v.type) {
    case "Number":
      return String(v.value);
    case "Boolean":
      return v.value ? "true" : "false";
    case "Text":
      return `"${v.value}"`;
    case "List":
      return `[${v.value.map(formatValue).join(", ")}]`;
    case "Effect":
      return `effect(${v.value.sink})`;
    case "Sum":
      // No core signal says which arm means "Some" vs "None" (a value carries only its taken arm's
      // index, never a name — the same honesty theme.ts's typeLabel already holds to for the TYPE
      // side, not guessing "this looks like Option"). Show the arm plainly instead of guessing.
      return `sum#${v.value.arm}(${formatValue(v.value.payload)})`;
  }
  // Exhaustiveness: this project's tsconfig doesn't enable noImplicitReturns, so a missing case here
  // was previously a silent runtime "undefined", not a compile error — exactly the gap that let
  // `Sum` go unhandled since it was added. A future WValue shape is now a compile error instead.
  const exhaustive: never = v;
  return exhaustive;
}

// A promoted input's starting value: the type's zero value for the three scalar forms that have
// one (Number→0, Boolean→false, Text→"") — the SAME convention `const`'s own default and core's
// `sourced_mock` already use, reused rather than invented fresh. Everything else (List/Product/Sum/
// Effect/Any) has no literal form — same boundary `const`'s CONST_TYPES already draws — so it starts
// `null` ("wire in": honestly unsupplied, not a guessed default).
//
// A `null` slot does NOT mean "eval blocks" — core's `eval` drops absent slots before `compute` ever
// runs (`inputs.iter().flatten()`; "compute's arity check is the firing rule" per eval.rs's own
// comment), uniformly for every primitive, no special-casing here or there. Whether that reads as
// "not ready" depends entirely on the target primitive: `add` destructures exactly 2 values, so a
// dropped slot fails its own arity match ⇒ not ready. `list` has no arity requirement at all, so it
// happily builds from however many values are actually present — including zero, `[]`. Both are
// correct; neither needs different handling in this file.
function defaultBoundaryValue(ty: WType): WValue | null {
  if (ty === "Number") return { type: "Number", value: 0 };
  if (ty === "Boolean") return { type: "Boolean", value: false };
  if (ty === "Text") return { type: "Text", value: "" };
  return null;
}

const boundaryFieldStyle: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  color: "#cfe3ff",
  background: "#0e1a2f",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  padding: "1px 4px",
  width: 64,
};

// One promoted input's field. A scalar gets a real, type-native control — a number input, a
// checkbox, a text input — not a generic text box needing string parsing: unlike `const`, a
// boundary value is never hashed (it's a pure eval-time argument), so there's no reason to route
// it through core's own string parser the way a frozen param has to. A non-scalar honestly has no
// literal form (the same boundary CONST_TYPES already draws) and shows "wire in", not a guess.
// Colour comes from theme.ts — the same "type is colour" legend the canvas nodes already use.

// Boundary port labels: `default_spec` names a promoted input after its INNER port's own name
// (core's own choice, P1.5.1), with no uniqueness guarantee across different children — two
// unrelated unwired "b" ports (extremely common; "a"/"b" are this catalog's standard binary-op
// names) can promote to the same bare name. Earlier version of this comment claimed the editor
// "genuinely cannot say which child this came from without a core change" — that was wrong: the
// composite's own boundary edges (from_child === BOUNDARY) already carry exactly that mapping,
// computed by derive_and_freeze and returned in composite.body; onProject just wasn't reading
// it. So this prefers TRUE provenance ("add · b") over the bare name. Occurrence-counting is
// only a last-resort tiebreaker for the one case even provenance can't separate: two placed
// instances of the SAME primitive, unconfigured, hence the SAME content hash (I3 — identical
// computation genuinely IS the same identity, not a gap), both left unwired on the same port.
function boundaryLabels(ports: WPort[], source: (string | null)[]): string[] {
  const withProvenance = ports.map((p, i) => (source[i] ? `${source[i]} · ${p.name}` : p.name));
  const seen = new Map<string, number>();
  return withProvenance.map((label) => {
    const n = (seen.get(label) ?? 0) + 1;
    seen.set(label, n);
    return n === 1 ? label : `${label} (${n})`;
  });
}

// A placed node's own hash, for use as a child in project/group (P4.3.1). A genesis primitive
// is rebuilt fresh from its catalog name+interface — I3 (the same primitive always yields the
// same hash) makes "rebuild" and "reference" the same operation, exactly how onProject has
// always worked. A grouped composite is different: it was already frozen when it was grouped,
// so its hash is just read off PlacedBody directly — its own internal structure is neither
// rebuilt nor re-inspected here. Shared by onProject and onGroup so there's one implementation
// of "how do I get this node's hash," not two that could drift.
function unitFor(n: PrimitiveNodeType, ev: Evaluator): string {
  if (n.data.body.kind === "Composite") return n.data.body.hash;
  const wunit: WUnit = {
    body: { kind: "Primitive", name: n.data.name },
    params: n.data.params,
    inputs: n.data.inputs,
    outputs: n.data.outputs,
  };
  return ev.insert(buildPreimage(wunit));
}

// `project()` throws the structured error object itself on an EncapError/ValidationError path
// (not a generic Error) — this is a loose but adequate runtime check: nothing else in this app
// produces an object with both a string `kind` and a string `message`. It doesn't verify every
// field matches a known variant; an unrecognized `kind` would fail the exhaustiveness check
// below at the type level, and the try/catch around the caller keeps that from crashing anything
// user-visible — highlighting is a nicety, never allowed to break error display itself.
function isWEncapError(x: unknown): x is WEncapError {
  return typeof x === "object" && x !== null && "kind" in x && "message" in x;
}

// Which PLACED NODE indices (into `children`/`nodes` — they share the same order) a
// ValidationError implicates. Most variants carry a `child` directly. `TypeMismatch` only
// carries an `edge` index — resolved here via the SAME `wedges` array the editor already built
// and submitted, so no core-side change was needed just to turn an edge index back into its
// endpoints. `Cycle` and the two `Boundary*OutOfRange` variants carry no attributable node at
// all — see WValidationError's own doc comment for why that's a real, named limit, not an
// oversight: tracing which edges form a cycle is new algorithmic work, out of this inch's scope.
function validationErrorNodeIndices(err: WValidationError, wedges: WEdge[]): number[] {
  switch (err.kind) {
    case "ChildOutOfRange":
    case "UnresolvedChild":
    case "OutputPortOutOfRange":
    case "InputPortOutOfRange":
      return [err.child];
    case "DuplicateInputEdge":
      return [err.to_child];
    case "TypeMismatch": {
      const e = wedges[err.edge];
      return e ? [e.from_child, e.to_child].filter((c) => c !== BOUNDARY) : [];
    }
    case "BoundaryInputOutOfRange":
    case "BoundaryOutputOutOfRange":
    case "Cycle":
      return [];
  }
  const exhaustive: never = err;
  return exhaustive;
}

// Same idea, one level up: an EncapError's own `child` fields, or — for `Invalid` — every
// wrapped ValidationError's indices, flattened. No `AmbiguousDefaultOutput` case anymore
// (P4.3, fork 2): default_spec no longer errors on multiple free outputs, so that variant —
// and the candidate-list highlighting it drove — is gone on both sides now.
function encapErrorNodeIndices(err: WEncapError, wedges: WEdge[]): number[] {
  switch (err.kind) {
    case "UnresolvedChild":
    case "PromoteChildOutOfRange":
    case "PromotePortOutOfRange":
    case "PromoteAlreadyBound":
    case "DuplicatePromotion":
    case "ExposeChildOutOfRange":
    case "ExposePortInvalid":
      return [err.child];
    case "Invalid":
      return err.errors.flatMap((e) => validationErrorNodeIndices(e, wedges));
  }
  const exhaustive: never = err;
  return exhaustive;
}

function BoundaryField({
  port,
  label: displayLabel,
  value,
  onChange,
}: {
  port: WPort;
  label: string;
  value: WValue | null;
  onChange: (v: WValue | null) => void;
}) {
  const row: CSSProperties = { display: "flex", alignItems: "center", justifyContent: "flex-end", gap: 6 };
  const label = (
    <span style={{ color: typeColor(port.ty), fontSize: 11 }} title={typeLabel(port.ty)}>
      {displayLabel}
    </span>
  );

  if (port.ty === "Number") {
    const raw = value?.type === "Number" ? String(value.value) : "";
    return (
      <div style={row}>
        {label}
        <input
          type="number"
          style={boundaryFieldStyle}
          value={raw}
          onChange={(e) => {
            const v = e.target.value;
            onChange(v === "" ? null : { type: "Number", value: Number(v) });
          }}
        />
      </div>
    );
  }
  if (port.ty === "Boolean") {
    const checked = value?.type === "Boolean" ? value.value : false;
    return (
      <div style={row}>
        {label}
        <input
          type="checkbox"
          checked={checked}
          onChange={(e) => onChange({ type: "Boolean", value: e.target.checked })}
        />
      </div>
    );
  }
  if (port.ty === "Text") {
    const raw = value?.type === "Text" ? value.value : "";
    return (
      <div style={row}>
        {label}
        <input
          type="text"
          style={{ ...boundaryFieldStyle, width: 84 }}
          value={raw}
          onChange={(e) => onChange({ type: "Text", value: e.target.value })}
        />
      </div>
    );
  }
  // List / Product / Sum / Effect / Any — no literal form. Honestly unsupplied, not defaulted.
  return (
    <div style={row}>
      {label}
      <span style={{ ...boundaryFieldStyle, color: "#5a6d8c", textAlign: "center" }}>wire in</span>
    </div>
  );
}

function Canvas({ catalog, variadic }: { catalog: WUnit[]; variadic: Set<string> }) {
  const [nodes, setNodes, onNodesChange] = useNodesState<PrimitiveNodeType>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const { screenToFlowPosition, getNode, getEdges } = useReactFlow<PrimitiveNodeType>();
  const idRef = useRef(0);

  const byName = useMemo(() => {
    const m = new Map<string, WUnit>();
    for (const u of catalog) {
      const n = primitiveName(u);
      if (n) m.set(n, u);
    }
    return m;
  }, [catalog]);

  const onDragOver = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
  }, []);

  const onDrop = useCallback(
    (e: DragEvent<HTMLDivElement>) => {
      e.preventDefault();
      const name = e.dataTransfer.getData(DND_MIME);
      const u = name ? byName.get(name) : undefined;
      if (!u || !name) return;

      const position = screenToFlowPosition({ x: e.clientX, y: e.clientY });
      const inputs = u.inputs ?? [];
      const data: PrimitiveData = {
        name,
        body: { kind: "Primitive" },
        inputs,
        outputs: u.outputs ?? [],
        params: u.params ?? [],
        // The catalog's own declared arity is the floor a variadic node may shrink back to —
        // never below what core actually declared, derived here rather than hardcoded, so a
        // future catalog change to one of these leaves' default arity can't silently drift from
        // what the "−" button enforces.
        variadic: variadic.has(name),
        minInputs: inputs.length,
      };
      const node: PrimitiveNodeType = {
        id: `${name}-${idRef.current++}`,
        type: "primitive",
        position,
        data,
      };
      setNodes((ns) => ns.concat(node));
    },
    [byName, screenToFlowPosition, setNodes],
  );

  // A wire is legal iff the upstream output type and the downstream input type are core-compatible,
  // and the input isn't already fed. The type rule is core's — `compatible` bridges to
  // `validate::compatible`, the canvas never re-decides it (I1). The single-source guard is UI
  // bookkeeping over our own edge state; core's `validate` stays the real gate at projection.
  //
  // srcPort is resolved from `sourceHandle` (P4.7: outputs are a Vec<Port>, symmetric with inputs),
  // the same way dstPort already resolves from `targetHandle` — not just the first output, since a
  // node may eventually declare more than one.
  const isValidConnection = useCallback(
    (c: Connection | Edge): boolean => {
      const source = getNode(c.source);
      const target = getNode(c.target);
      if (!source || !target) return false;
      const srcPort = source.data.outputs.find((p) => p.name === c.sourceHandle);
      const dstPort = target.data.inputs.find((p) => p.name === c.targetHandle);
      if (!srcPort || !dstPort) return false;
      const alreadyFed = getEdges().some(
        (e) => e.target === c.target && e.targetHandle === c.targetHandle,
      );
      if (alreadyFed) return false;
      return compatible(srcPort.ty, dstPort.ty);
    },
    [getNode, getEdges],
  );

  // The wire carries a type: colour it by the upstream output's hue — the same legend the ports use.
  // Resolved from `sourceHandle`, the same as isValidConnection above — not always the first output.
  const onConnect = useCallback(
    (c: Connection) => {
      const srcPort = getNode(c.source)?.data.outputs.find((p) => p.name === c.sourceHandle);
      const srcTy = srcPort?.ty ?? null;
      const edge = { ...c, style: { stroke: outputColor(srcTy), strokeWidth: 1.6 } };
      setEdges((es) => addEdge(edge, es));
    },
    [getNode, setEdges],
  );

  // ── Projection ────────────────────────────────────────────────────────────────────────────────
  // On demand, turn the wired canvas into a real core composite and show its content hash. The editor
  // only marshals STRUCTURE — children by index, edges by index, handle name → port index — then
  // hands it to core's default_spec + derive_and_freeze (behind Evaluator.project), which derive the
  // boundary, validate, and freeze. The editor re-decides none of it (one decider). Cosmetic move ⇒
  // same hash; rewire ⇒ new hash; an ambiguous or invalid graph ⇒ a surfaced error, never a guess.
  //
  // A composite's promoted inputs (`ports`) and the values currently supplied for them (`values`,
  // same length/order, `null` = unsupplied) are held alongside the frozen preimage (`pre`) so a
  // boundary field's onChange can re-eval without re-projecting. `source[i]` is which PLACED NODE
  // promoted input `i` actually came from — read directly off the composite's own boundary edges
  // (`from_child === BOUNDARY`), not guessed: core already computes this mapping and returns it,
  // the editor just wasn't reading `composite.body` before. A CLOSED composite is just the
  // zero-input case of this same shape — `ports: []`, nothing special-cased.
  //
  // `outputs`/`resultSource`/`results` are the exact dual, on the output side (P4.3, fork 2):
  // `default_spec` now proposes every free output automatically, so `results` may hold several
  // values, not one — a composite with zero free outputs is likewise just the empty case of this
  // same shape (`outputs: []`), not an error state.
  const ev = useMemo(() => new Evaluator(), []);
  const [projection, setProjection] = useState<{
    hash: string;
    pre: Uint8Array;
    ports: WPort[];
    source: (string | null)[];
    values: (WValue | null)[];
    outputs: WPort[];
    resultSource: (string | null)[];
    results: (WValue | null)[];
    // The exact inputs this projection was built from, held so excludeOutput below can
    // re-freeze without re-deriving from the (possibly since-changed) canvas — the same
    // "frozen at project time" reasoning `pre` already follows for boundary re-eval.
    children: string[];
    wedges: WEdge[];
    spec: WEncapSpec;
  } | null>(null);
  const [projError, setProjError] = useState<string>("");
  // Which placed nodes a projection error implicates — derived from the structured error's own
  // indices (see encapErrorNodeIndices), not guessed. Purely presentational: never read by
  // onProject/onDrop, never affects hashing — a node's identity doesn't know it's "wrong."
  const [errorNodeIds, setErrorNodeIds] = useState<Set<string>>(new Set());

  const onProject = useCallback(() => {
    // Hoisted out of the try block on purpose: TypeMismatch only carries an `edge` index, and
    // resolving it back to actual node ids (in the catch block below) needs this same array —
    // a `const` declared inside `try {}` isn't visible inside its own `catch {}` in JS.
    let wedges: WEdge[] = [];
    try {
      // Child index = position in `nodes` — arbitrary, because core canonicalizes child order, so a
      // cosmetic reorder cannot move the hash (I5). `id`/`position` are dropped here.
      const indexOf = new Map(nodes.map((n, i) => [n.id, i]));

      // Each node's own hash — a fresh primitive rebuild or a grouped composite reference
      // (P4.3.1), resolved identically to onGroup below via the shared unitFor. Pinned child
      // hashes (I3), index order.
      const children = nodes.map((n) => unitFor(n, ev));

      // Canvas wire → core edge by index. from_port is the named output's position in the source's
      // interface, to_port the named input's position in the target's — symmetric (P4.7: outputs are
      // a Vec<Port>, no longer always port 0), the editor holds both interfaces already.
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

      // Core proposes the boundary (default_spec) and — unedited — freezes it
      // (derive_and_freeze), matching project()'s combined behavior exactly for anyone who
      // never excludes anything. Split apart (not project() itself, which stays untouched
      // below for anyone who calls it directly) so excludeOutput below can re-freeze with
      // fewer exposed outputs without re-deriving the proposal from scratch (P4.3, fork 3).
      // An EncapError — cycle, type mismatch, an unresolvable child — throws the structured
      // error object itself (see WEncapError), not a generic Error, so the catch block below
      // can point at the actual node(s) at fault.
      const spec = ev.propose_encapsulation(children, wedges) as WEncapSpec;
      const composite = ev.freeze_encapsulation(children, wedges, spec) as WUnit;
      const pre = buildPreimage(composite);
      const hash = ev.insert(pre);

      // Run it through the one Evaluator, starting from each promoted input's zero value (or `null`
      // for a non-scalar port — "wire in", nothing to default to). A closed graph is `ports: []`,
      // `ev.eval(pre, [])` — exactly its old behaviour, just no longer a separate branch. `results`
      // now holds one slot per declared output (P4.3, fork 2 — default_spec proposes every free
      // output automatically); `null` in a slot ⇒ that output isn't ready (an unfilled non-scalar
      // port, a divide-by-zero, an unsupplied driver), independently of its siblings — a sink
      // graph yields the Effect it constructs (pure eval doesn't fire it).
      const ports = composite.inputs ?? [];
      // Which PLACED NODE each promoted input actually came from — read directly off the
      // composite's own boundary edges (from_child === BOUNDARY), not guessed and not
      // re-derived: derive_and_freeze already computed exactly this mapping and returned it in
      // composite.body; the editor was only reading composite.inputs/.output before and
      // discarding the rest. `nodes[to_child]` is safe because `children` (above) was built from
      // `nodes` in the same order, so the indices line up by construction.
      const boundaryEdges = composite.body.kind === "Composite" ? composite.body.edges : [];
      const source: (string | null)[] = ports.map((_, i) => {
        const edge = boundaryEdges.find((e) => e.from_child === BOUNDARY && e.from_port === i);
        return edge ? (nodes[edge.to_child]?.data.name ?? null) : null;
      });
      const values = ports.map((p) => defaultBoundaryValue(p.ty));

      // The output side (P4.3, fork 2): default_spec now proposes every free output, so
      // `composite.outputs` may hold several — symmetric with `ports`/`source` above, just
      // with the boundary edge direction reversed (to_child === BOUNDARY, the dual of
      // from_child === BOUNDARY for inputs, exactly as derive_and_freeze's own comment
      // already calls the boundary edges "the dual of how eval reads them"). Reusing
      // boundaryLabels below rather than writing a second disambiguation function — it was
      // already fully generic, never actually input-specific.
      const outputs = composite.outputs ?? [];
      const resultSource: (string | null)[] = outputs.map((_, i) => {
        const edge = boundaryEdges.find((e) => e.to_child === BOUNDARY && e.to_port === i);
        return edge ? (nodes[edge.from_child]?.data.name ?? null) : null;
      });
      const results = ev.eval(pre, values) as (WValue | null)[];
      setProjection({
        hash, pre, ports, source, values, outputs, resultSource, results, children, wedges, spec,
      });
      setProjError("");
      setErrorNodeIds(new Set());
    } catch (err) {
      setProjection(null);
      setProjError(isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err));
      // Highlighting is a nicety layered on top of the (already-set) error message — never let
      // it break error display itself if a shape ever slips past isWEncapError unexpectedly.
      try {
        const indices = isWEncapError(err) ? encapErrorNodeIndices(err, wedges) : [];
        const ids = indices.map((i) => nodes[i]?.id).filter((id): id is string => id !== undefined);
        setErrorNodeIds(new Set(ids));
      } catch {
        setErrorNodeIds(new Set());
      }
    }
  }, [nodes, edges, ev]);

  // Group the current selection into one placed composite node (P4.3.1). React Flow's own
  // selection (shift-click / drag-box) is already live and already used for delete — this is
  // its first use for anything else. Mechanically the same derivation as onProject, just fed a
  // subset: only the selected nodes become children, only edges where BOTH endpoints are
  // selected become the composite's inner wiring — an edge with exactly one endpoint outside
  // the selection is, from the selected subgraph's own perspective, indistinguishable from an
  // ordinary free input or output, so default_spec's existing proposal derives the right
  // boundary with no new core or bridge logic. Uses the full automatic proposal directly, same
  // as onProject's own default path — no exclude-review step for grouping in this inch (fork
  // 3's own scoped-down UI stays a whole-canvas-projection feature, not extended here).
  // Deliberately does NOT include expand-to-inspect — its own separate, later inch once
  // grouping itself is proven, the same staged discipline the encapsulation work already used.
  const onGroup = useCallback(() => {
    const selected = nodes.filter((n) => n.selected);
    if (selected.length < 2) return; // mirrors the button's own disabled guard, defensively
    const selectedIds = new Set(selected.map((n) => n.id));
    const indexOf = new Map(selected.map((n, i) => [n.id, i]));

    let wedges: WEdge[] = [];
    try {
      const children = selected.map((n) => unitFor(n, ev));

      const innerEdges = edges.filter((e) => selectedIds.has(e.source) && selectedIds.has(e.target));
      wedges = innerEdges.map((e) => {
        const from = indexOf.get(e.source)!;
        const to = indexOf.get(e.target)!;
        const fromPort = selected[from].data.outputs.findIndex((p) => p.name === e.sourceHandle);
        const toPort = selected[to].data.inputs.findIndex((p) => p.name === e.targetHandle);
        return { from_child: from, from_port: fromPort, to_child: to, to_port: toPort };
      });

      const composite = ev.project(children, wedges) as WUnit;
      const pre = buildPreimage(composite);
      const hash = ev.insert(pre);

      const boundaryEdges = composite.body.kind === "Composite" ? composite.body.edges : [];
      const rawInputs = composite.inputs ?? [];
      const rawOutputs = composite.outputs ?? [];

      // Which (selected-node, port) each promoted input / exposed output came from — read
      // directly off the boundary edges core already computed, not guessed. Purely
      // index-based (from_port/to_port against BOUNDARY), so this works on the RAW,
      // possibly-colliding ports straight from ev.project — naming below doesn't change it,
      // it only feeds it.
      const promotedFrom = rawInputs.map((_, i) => {
        const be = boundaryEdges.find((e) => e.from_child === BOUNDARY && e.from_port === i);
        return be ? { child: be.to_child, port: be.to_port } : null;
      });
      const exposedFrom = rawOutputs.map((_, i) => {
        const be = boundaryEdges.find((e) => e.to_child === BOUNDARY && e.to_port === i);
        return be ? { child: be.from_child, port: be.from_port } : null;
      });

      // Name the new node's ports by provenance, reusing boundaryLabels' own algorithm rather
      // than a second, plainer disambiguation — "add · b" instead of a bare "b_2". These
      // labels become the ports' REAL name (this node's actual handle ids), not just a
      // display string layered on top: composite.inputs/outputs are renamed here, after
      // ev.project already returned and the hash is already computed, so this can never
      // affect the composite's identity — the same "renaming is purely local view-state"
      // property the plain-suffix version already had. Falls back to occurrence-counting only
      // when provenance itself doesn't separate two ports — two unconfigured instances of the
      // literally same primitive, genuinely identical — exactly boundaryLabels' own last
      // resort for the promoted-input side.
      const inputSource = promotedFrom.map((x) => (x ? selected[x.child]?.data.name ?? null : null));
      const outputSource = exposedFrom.map((x) => (x ? selected[x.child]?.data.name ?? null : null));
      const inputLabels = boundaryLabels(rawInputs, inputSource);
      const outputLabels = boundaryLabels(rawOutputs, outputSource);
      const inputs = rawInputs.map((p, i) => ({ ...p, name: inputLabels[i] }));
      const outputs = rawOutputs.map((p, i) => ({ ...p, name: outputLabels[i] }));

      const groupId = `composite-${idRef.current++}-${hash.slice(0, 8)}`;
      const centroid = {
        x: selected.reduce((s, n) => s + n.position.x, 0) / selected.length,
        y: selected.reduce((s, n) => s + n.position.y, 0) / selected.length,
      };
      const groupNode: PrimitiveNodeType = {
        id: groupId,
        type: "primitive",
        position: centroid,
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
      // handle; drop every edge now fully internal to the frozen composite (it's baked into
      // the hash, no longer a canvas wire); leave every edge untouched by the selection exactly
      // as it was.
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

      setNodes((ns) => [...ns.filter((n) => !selectedIds.has(n.id)), groupNode]);
      setEdges(nextEdges);
      setProjError("");
      setErrorNodeIds(new Set());
    } catch (err) {
      setProjError(isWEncapError(err) ? err.message : err instanceof Error ? err.message : String(err));
      try {
        const indices = isWEncapError(err) ? encapErrorNodeIndices(err, wedges) : [];
        const ids = indices.map((i) => selected[i]?.id).filter((id): id is string => id !== undefined);
        setErrorNodeIds(new Set(ids));
      } catch {
        setErrorNodeIds(new Set());
      }
    }
  }, [nodes, edges, ev, setNodes, setEdges]);

  // Expand a placed composite node back into its children (P4.3.1's last piece — the dual of
  // onGroup). One level only, deliberately: a nested composite child is revealed still
  // collapsed, as its own composite node, not recursively unpacked — the same restraint
  // grouping already has (it only ever acts on the current selection, never implicitly
  // nesting). Deep, read-only, annotated descent is P4.4.1's own, separate job.
  const onExpand = useCallback(() => {
    const selected = nodes.filter((n) => n.selected);
    if (selected.length !== 1) return; // mirrors the button's own disabled guard, defensively
    const target = selected[0];
    if (target.data.body.kind !== "Composite") return;
    const hash = target.data.body.hash;

    try {
      const wunit = ev.get_unit(hash) as WUnit;
      // get_unit reads back exactly what derive_and_freeze produced at grouping time, which is
      // always a Composite — but WBody is a discriminated union either way, so this check is
      // required to narrow it, not just a defensive nicety.
      if (wunit.body.kind !== "Composite") throw new Error("expand: not a composite");
      const childHashes = wunit.body.children;
      const innerEdges = wunit.body.edges;

      // Reconstruct each child fresh — a primitive regains full editability (checked against
      // the same variadicPrimitives() set onDrop uses, so an expanded node is indistinguishable
      // from one just dropped from the palette), a nested composite stays a placed reference
      // (P4.3.1's own one-level rule above). No renaming/dedup needed here the way grouping
      // needed boundaryLabels: expanding SPLITS one already-frozen interface whose names were
      // already unique when it was built, it doesn't COMBINE several into one new one.
      const spacing = 220;
      const startX = target.position.x - ((childHashes.length - 1) * spacing) / 2;
      const newNodes: PrimitiveNodeType[] = childHashes.map((h, i) => {
        const cu = ev.get_unit(h) as WUnit;
        const position = { x: startX + i * spacing, y: target.position.y };
        const data: PrimitiveData =
          cu.body.kind === "Primitive"
            ? {
                name: cu.body.name,
                body: { kind: "Primitive" },
                inputs: cu.inputs ?? [],
                outputs: cu.outputs ?? [],
                params: cu.params ?? [],
                variadic: variadic.has(cu.body.name),
                // The catalog's own declared arity, not this (possibly grown-then-frozen)
                // child's current one — same floor onDrop's own placement already enforces.
                // Falls back to the child's own arity only if the catalog lookup somehow
                // misses, rather than crashing.
                minInputs: (byName.get(cu.body.name)?.inputs ?? cu.inputs ?? []).length,
              }
            : {
                name: h.slice(0, 8),
                body: { kind: "Composite", hash: h },
                inputs: cu.inputs ?? [],
                outputs: cu.outputs ?? [],
                params: [],
                variadic: false,
                minInputs: 0,
              };
        const id =
          cu.body.kind === "Primitive"
            ? `${cu.body.name}-${idRef.current++}`
            : `composite-${idRef.current++}-${h.slice(0, 8)}`;
        return { id, type: "primitive", position, data };
      });

      // Canvas edges that pointed at the composite's own ports get re-pointed at the right new
      // child's handle instead — resolved the same "read what core already computed" way
      // onGroup's own rewiring works, just walked in the opposite direction: find which
      // boundary edge core built for this port, follow it to the (child, port) it actually
      // came from.
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

      // The composite's own inner wiring (both endpoints are children, not BOUNDARY) becomes
      // real canvas edges between the new nodes — through addEdge, same as onConnect, so these
      // get real ids and the same type-colored styling every other wire on the canvas has,
      // not a bare, unstyled object.
      const withInner = innerEdges
        .filter((e) => e.from_child !== BOUNDARY && e.to_child !== BOUNDARY)
        .reduce((acc: Edge[], e) => {
          const srcNode = newNodes[e.from_child];
          const tgtNode = newNodes[e.to_child];
          const srcPort = srcNode.data.outputs[e.from_port];
          const tgtHandle = tgtNode.data.inputs[e.to_port]?.name;
          if (!srcPort || !tgtHandle) return acc;
          const conn = {
            source: srcNode.id,
            sourceHandle: srcPort.name,
            target: tgtNode.id,
            targetHandle: tgtHandle,
          };
          return addEdge({ ...conn, style: { stroke: outputColor(srcPort.ty), strokeWidth: 1.6 } }, acc);
        }, rewired);

      setNodes((ns) => [...ns.filter((n) => n.id !== target.id), ...newNodes]);
      setEdges(withInner);
      setProjError("");
      setErrorNodeIds(new Set());
    } catch (err) {
      // get_unit only ever fails as a plain string (no structured error family a "not found"
      // belongs to), and every hash reached here was already resolved when the composite was
      // frozen — so there's no clean "which node is at fault" the way an EncapError has;
      // nothing to highlight, just the message.
      setProjError(err instanceof Error ? err.message : String(err));
    }
  }, [nodes, edges, ev, byName, setNodes, setEdges]);

  // A boundary field changed: update its slot and re-eval live. Safe to be reactive (unlike
  // projecting itself) because a boundary value is never part of the composite's hash — it's a
  // pure eval-time argument, not identity — so there is no "cosmetic move" noise to avoid here.
  const setBoundaryValue = useCallback(
    (index: number, value: WValue | null) => {
      setProjection((p) => {
        if (!p) return p;
        const values = p.values.map((v, i) => (i === index ? value : v));
        return { ...p, values, results: ev.eval(p.pre, values) as (WValue | null)[] };
      });
    },
    [ev],
  );

  // Exclude one currently-exposed output and re-freeze (P4.3, fork 3). Unlike setBoundaryValue
  // above, this is NOT a lightweight re-eval: excluding an output is a real authoring choice
  // that changes what the composite's interface declares, so it changes the hash too — a
  // genuine re-freeze through derive_and_freeze, not a display filter over the old one.
  // `outputs`/`resultSource` line up 1:1 with `spec.expose` by construction (derive_and_freeze
  // builds them from it in order), so filtering all three by the same index keeps them in
  // sync without re-deriving provenance from composite.body again. No "re-include": the
  // original proposal is recovered by pressing "project" again, already a single click.
  const excludeOutput = useCallback(
    (index: number) => {
      setProjection((p) => {
        if (!p) return p;
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
          // Dropping one entry from an already-valid spec can't create a new validation
          // failure — nothing here should throw. If it somehow does, leave the projection
          // exactly as it was rather than silently corrupt it.
          return p;
        }
      });
    },
    [ev],
  );

  // Purely a rendering decoration for React Flow — never touches the underlying `nodes` state
  // that onProject/onDrop read and write, so flagging a node as errored has zero effect on
  // hashing or identity (the same "cosmetic vs. structural" line I5 already draws elsewhere).
  const decoratedNodes = useMemo(
    () =>
      errorNodeIds.size === 0
        ? nodes
        : nodes.map((n) =>
            errorNodeIds.has(n.id) ? { ...n, data: { ...n.data, error: true } } : n,
          ),
    [nodes, errorNodeIds],
  );

  // How many placed nodes are currently selected — React Flow's own selection state
  // (shift-click / drag-box), already live for delete, now also gating the "group" button
  // (P4.3.1). Cheap enough over a canvas-sized node list not to need memoizing.
  const selectedCount = nodes.filter((n) => n.selected).length;
  // "Expand" is the dual of "group": only meaningful for exactly one selected composite node —
  // mirrors group's own selectedCount<2 guard rather than a new per-node interaction
  // PrimitiveNode.tsx doesn't otherwise have for whole-node operations.
  const canExpand = selectedCount === 1 && nodes.some((n) => n.selected && n.data.body.kind === "Composite");

  return (
    <div className="hl-canvas" onDrop={onDrop} onDragOver={onDragOver}>
      <ReactFlow<PrimitiveNodeType>
        nodes={decoratedNodes}
        onNodesChange={onNodesChange}
        edges={edges}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        isValidConnection={isValidConnection}
        nodeTypes={nodeTypes}
        connectionLineStyle={{ stroke: "#6fd3e0", strokeWidth: 1.6 }}
        fitView={false}
        minZoom={0.4}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
        // Select a node or wire, then Backspace or Delete removes it — React Flow's own built-in
        // mechanism (deleting a node also cleans up any wires attached to it; nothing hand-rolled
        // here). The default is Backspace only; Delete is added because it's the key most people
        // reach for first, especially on Windows. React Flow itself skips this handling while a
        // text input/select inside a node has focus, so editing a const's value or type is safe.
        deleteKeyCode={["Backspace", "Delete"]}
      >
        <Background variant={BackgroundVariant.Lines} gap={26} color="#1b2a45" />
        <Controls showInteractive={false} />
        <Panel position="top-right">
          <div style={{ display: "flex", flexDirection: "column", gap: 6, alignItems: "flex-end" }}>
            <button
              onClick={onProject}
              disabled={nodes.length === 0}
              style={{
                font: "inherit",
                fontSize: 12,
                letterSpacing: "0.04em",
                padding: "5px 12px",
                color: "#0e1a2f",
                background: "#6fd3e0",
                border: "none",
                borderRadius: 3,
                cursor: nodes.length === 0 ? "default" : "pointer",
                opacity: nodes.length === 0 ? 0.4 : 1,
              }}
            >
              project
            </button>
            <button
              onClick={onGroup}
              disabled={selectedCount < 2}
              title={selectedCount < 2 ? "select 2+ nodes to group" : `group ${selectedCount} nodes`}
              style={{
                font: "inherit",
                fontSize: 12,
                letterSpacing: "0.04em",
                padding: "5px 12px",
                color: "#0e1a2f",
                background: "#6fd3e0",
                border: "none",
                borderRadius: 3,
                cursor: selectedCount < 2 ? "default" : "pointer",
                opacity: selectedCount < 2 ? 0.4 : 1,
              }}
            >
              group
            </button>
            <button
              onClick={onExpand}
              disabled={!canExpand}
              title={canExpand ? "expand this composite" : "select one composite node to expand"}
              style={{
                font: "inherit",
                fontSize: 12,
                letterSpacing: "0.04em",
                padding: "5px 12px",
                color: "#0e1a2f",
                background: "#6fd3e0",
                border: "none",
                borderRadius: 3,
                cursor: canExpand ? "pointer" : "default",
                opacity: canExpand ? 1 : 0.4,
              }}
            >
              expand
            </button>
            {projection && (
              <div style={{ fontSize: 12, color: "#9fb4d4", textAlign: "right", lineHeight: 1.4 }}>
                <div style={{ color: "#6fd3e0" }} title={projection.hash}>
                  {projection.hash.slice(0, 12)}…
                </div>
                {projection.ports.length > 0 && (
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      gap: 4,
                      margin: "6px 0",
                      alignItems: "flex-end",
                    }}
                  >
                    {boundaryLabels(projection.ports, projection.source).map((label, i) => (
                      <BoundaryField
                        key={i}
                        port={projection.ports[i]}
                        label={label}
                        value={projection.values[i]}
                        onChange={(v) => setBoundaryValue(i, v)}
                      />
                    ))}
                  </div>
                )}
                {projection.results.length > 0 && (
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      gap: 2,
                      alignItems: "flex-end",
                      color: "#9ad29f",
                    }}
                  >
                    {boundaryLabels(projection.outputs, projection.resultSource).map((label, i) => (
                      <div key={i} style={{ display: "flex", alignItems: "center", gap: 6 }}>
                        <span>
                          {label} ={" "}
                          {projection.results[i] ? formatValue(projection.results[i]!) : "not ready"}
                        </span>
                        <button
                          className="nodrag"
                          onClick={() => excludeOutput(i)}
                          title={`exclude "${label}" from this composite's interface`}
                          style={{
                            font: "inherit",
                            fontSize: 10,
                            lineHeight: 1,
                            color: "#9fb4d4",
                            background: "#0e1a2f",
                            border: "1px solid #1b2a45",
                            borderRadius: 3,
                            width: 14,
                            height: 14,
                            padding: 0,
                            cursor: "pointer",
                          }}
                        >
                          ×
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            )}
            {projError && (
              <div style={{ fontSize: 12, color: "#e0916f", maxWidth: 240, textAlign: "right" }}>
                {projError}
              </div>
            )}
          </div>
        </Panel>
      </ReactFlow>
      {nodes.length === 0 && (
        <div className="hl-canvas__empty">drag a primitive here to place it</div>
      )}
    </div>
  );
}

function Editor() {
  const [catalog, setCatalog] = useState<WUnit[] | null>(null);
  const [variadic, setVariadic] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string>("");

  useEffect(() => {
    (async () => {
      await ensureReady();
      setCatalog(primitiveCatalog());
      setVariadic(variadicPrimitives());
    })().catch((e: unknown) => setError(String(e)));
  }, []);

  return (
    <div className="hl-app">
      <header className="hl-header">
        <span className="hl-header__brand">holon</span>
        <span className="hl-header__sep">·</span>
        <span className="hl-header__mode">authoring</span>
        <span className="hl-header__count">
          {catalog ? `${catalog.length} genesis atoms` : "loading core…"}
        </span>
      </header>

      {error ? (
        <pre className="hl-error">{error}</pre>
      ) : (
        <div className="hl-body">
          {catalog && <Palette catalog={catalog} />}
          <ReactFlowProvider>
            {catalog && <Canvas catalog={catalog} variadic={variadic} />}
          </ReactFlowProvider>
        </div>
      )}
    </div>
  );
}

export default function App() {
  return <Editor />;
}