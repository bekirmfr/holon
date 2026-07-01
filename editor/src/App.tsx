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
// wrapped ValidationError's indices, flattened. `AmbiguousDefaultOutput`'s candidates are
// exactly the node indices to highlight — the fix this whole inch exists for.
function encapErrorNodeIndices(err: WEncapError, wedges: WEdge[]): number[] {
  switch (err.kind) {
    case "UnresolvedChild":
    case "PromoteChildOutOfRange":
    case "PromotePortOutOfRange":
    case "PromoteAlreadyBound":
    case "DuplicatePromotion":
    case "OutputChildOutOfRange":
    case "OutputPortInvalid":
      return [err.child];
    case "AmbiguousDefaultOutput":
      return err.candidates;
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
  const ev = useMemo(() => new Evaluator(), []);
  const [projection, setProjection] = useState<{
    hash: string;
    pre: Uint8Array;
    ports: WPort[];
    source: (string | null)[];
    values: (WValue | null)[];
    result: WValue | null;
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

      // Each node is the catalog primitive it was placed as → pinned child hashes (I3), index order.
      const children = nodes.map((n) => {
        const wunit: WUnit = {
          body: { kind: "Primitive", name: n.data.name },
          params: n.data.params,
          inputs: n.data.inputs,
          outputs: n.data.outputs,
        };
        return ev.insert(buildPreimage(wunit));
      });

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

      // Core derives the boundary and freezes; an EncapError — ambiguous output, cycle, type
      // mismatch — throws the structured error object itself (see WEncapError), not a generic
      // Error, so the catch block below can point at the actual node(s) at fault.
      const composite = ev.project(children, wedges) as WUnit;
      const pre = buildPreimage(composite);
      const hash = ev.insert(pre);

      // Run it through the one Evaluator, starting from each promoted input's zero value (or `null`
      // for a non-scalar port — "wire in", nothing to default to). A closed graph is `ports: []`,
      // `ev.eval(pre, [])` — exactly its old behaviour, just no longer a separate branch. `null` in
      // the result ⇒ not ready (an unfilled non-scalar port, a divide-by-zero, an unsupplied driver);
      // a sink graph yields the Effect it constructs (pure eval doesn't fire it).
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
      const result = ev.eval(pre, values) as WValue | null;
      setProjection({ hash, pre, ports, source, values, result });
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

  // A boundary field changed: update its slot and re-eval live. Safe to be reactive (unlike
  // projecting itself) because a boundary value is never part of the composite's hash — it's a
  // pure eval-time argument, not identity — so there is no "cosmetic move" noise to avoid here.
  const setBoundaryValue = useCallback(
    (index: number, value: WValue | null) => {
      setProjection((p) => {
        if (!p) return p;
        const values = p.values.map((v, i) => (i === index ? value : v));
        return { ...p, values, result: ev.eval(p.pre, values) as WValue | null };
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
                <div style={{ color: "#9ad29f" }}>
                  = {projection.result ? formatValue(projection.result) : "not ready"}
                </div>
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