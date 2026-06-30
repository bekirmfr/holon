// The authoring canvas: a palette (derived from core's catalog) on the left, a blueprint canvas on
// the right. Drag a primitive across to place a typed node. This inch places nodes only — wiring and
// projection/eval are later inches (P4.1-palette-L4 §1). Nothing here re-asserts a primitive's shape;
// every placed node reads its ports from the catalog interface (I1).

import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent } from "react";
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
  compatible,
  buildPreimage,
  Evaluator,
  type WUnit,
  type WEdge,
  type WValue,
} from "./holon";
import { outputColor } from "./theme";
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
  }
}

function Canvas({ catalog }: { catalog: WUnit[] }) {
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
      const data: PrimitiveData = {
        name,
        inputs: u.inputs ?? [],
        output: u.output ?? null,
        params: u.params ?? [],
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
  const isValidConnection = useCallback(
    (c: Connection | Edge): boolean => {
      const source = getNode(c.source);
      const target = getNode(c.target);
      if (!source || !target) return false;
      const srcTy = source.data.output;
      const dstPort = target.data.inputs.find((p) => p.name === c.targetHandle);
      if (!srcTy || !dstPort) return false;
      const alreadyFed = getEdges().some(
        (e) => e.target === c.target && e.targetHandle === c.targetHandle,
      );
      if (alreadyFed) return false;
      return compatible(srcTy, dstPort.ty);
    },
    [getNode, getEdges],
  );

  // The wire carries a type: colour it by the upstream output's hue — the same legend the ports use.
  const onConnect = useCallback(
    (c: Connection) => {
      const srcTy = getNode(c.source)?.data.output ?? null;
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
  const ev = useMemo(() => new Evaluator(), []);
  const [projection, setProjection] = useState<
    { hash: string; inputs: number; value: WValue | null } | null
  >(null);
  const [projError, setProjError] = useState<string>("");

  const onProject = useCallback(() => {
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
          output: n.data.output,
        };
        return ev.insert(buildPreimage(wunit));
      });

      // Canvas wire → core edge by index. from_port is 0 (single output, the "out" handle); to_port
      // is the named input's position in the target's interface (the editor holds that interface).
      const wedges: WEdge[] = edges.map((e) => {
        const from = indexOf.get(e.source);
        const to = indexOf.get(e.target);
        if (from === undefined || to === undefined) {
          throw new Error("a wire references an unknown node");
        }
        const toPort = nodes[to].data.inputs.findIndex((p) => p.name === e.targetHandle);
        if (toPort < 0) throw new Error(`unknown target port "${String(e.targetHandle)}"`);
        return { from_child: from, from_port: 0, to_child: to, to_port: toPort };
      });

      // Core derives the boundary and freezes; an EncapError — ambiguous output, cycle, type mismatch
      // — throws. `insert` returns the composite's content hash and pins it.
      const composite = ev.project(children, wedges) as WUnit;
      const pre = buildPreimage(composite);
      const hash = ev.insert(pre);

      // Run it through the one Evaluator — but only when it is CLOSED (no promoted inputs): a closed
      // graph (consts feeding pure ops) needs nothing supplied. An open composite waits for the
      // boundary-input-fields inch. null ⇒ not ready (an unparsed const, a divide-by-zero, or an
      // unsupplied driver); a sink graph yields the Effect it constructs (pure eval doesn't fire it).
      const inputs = composite.inputs?.length ?? 0;
      const value = inputs === 0 ? (ev.eval(pre, []) as WValue | null) : null;
      setProjection({ hash, inputs, value });
      setProjError("");
    } catch (err) {
      setProjection(null);
      setProjError(err instanceof Error ? err.message : String(err));
    }
  }, [nodes, edges, ev]);

  return (
    <div className="hl-canvas" onDrop={onDrop} onDragOver={onDragOver}>
      <ReactFlow<PrimitiveNodeType>
        nodes={nodes}
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
                {projection.inputs === 0 ? (
                  <div style={{ color: "#9ad29f" }}>
                    = {projection.value ? formatValue(projection.value) : "not ready"}
                  </div>
                ) : (
                  <div>
                    {projection.inputs} input{projection.inputs === 1 ? "" : "s"} — supply to run
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
  const [error, setError] = useState<string>("");

  useEffect(() => {
    (async () => {
      await ensureReady();
      setCatalog(primitiveCatalog());
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
            {catalog && <Canvas catalog={catalog} />}
          </ReactFlowProvider>
        </div>
      )}
    </div>
  );
}

export default function App() {
  return <Editor />;
}