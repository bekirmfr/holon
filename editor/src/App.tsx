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
  useNodesState,
  useEdgesState,
  useReactFlow,
  addEdge,
  type NodeTypes,
  type Connection,
  type Edge,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";

import { ensureReady, primitiveCatalog, compatible, type WUnit } from "./holon";
import { outputColor } from "./theme";
import { Palette, DND_MIME } from "./Palette";
import { PrimitiveNode, type PrimitiveData, type PrimitiveNodeType } from "./PrimitiveNode";

const nodeTypes = { primitive: PrimitiveNode } as unknown as NodeTypes;

function primitiveName(u: WUnit): string | null {
  return u.body.kind === "Primitive" ? u.body.name : null;
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