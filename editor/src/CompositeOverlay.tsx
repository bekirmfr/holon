// Looking inside a composite without ungrouping it: "expand" grows the node into a canvas of its
// own over the flow, showing its inner flow (canvasOps.innerCanvas): its nodes, its wires, and
// its boundary, inputs on the left and outputs on the right, each labelled with what it is wired
// to outside. A composite inside can be expanded in turn; the breadcrumb goes back. Read only:
// changing what is inside would make it another unit, which is ungroup's job.

import { useEffect, useState, type CSSProperties } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useEdgesState,
  useNodesState,
  useReactFlow,
  type Edge,
  type Node,
  type NodeProps,
  type NodeTypes,
} from "@xyflow/react";
import { PrimitiveNode, type PrimitiveNodeType } from "./PrimitiveNode";
import { NodeActions, useNodeActions } from "./NodeDocs";
import type { BoundaryData } from "./canvasOps";
import { typeColor, typeLabel } from "./theme";

/** One level opened: its title (the composite's name) and its inner canvas. */
export type OverlayFrame = { title: string; nodes: Node[]; edges: Edge[] };

/** A composite's boundary inside its canvas: its ports, and what each is wired to outside. */
function BoundaryNode({ data }: NodeProps<Node<BoundaryData>>) {
  const inbound = data.side === "in";
  return (
    <div className="hl-node" style={{ minWidth: 150, borderStyle: "dashed" }}>
      <div className="hl-node__title" style={{ color: "#9fb4d4" }}>
        {inbound ? "inputs" : "outputs"}
      </div>
      <div style={{ padding: "4px 0" }}>
        {data.ports.length === 0 && <div style={{ padding: "2px 12px", color: "var(--ink-dim)", fontSize: 11 }}>none</div>}
        {data.ports.map((p) => (
          // Two lines per port: its name, and what it is wired to outside; the handle centred on both.
          <div key={p.name} style={{ position: "relative", padding: "4px 14px", textAlign: inbound ? "right" : "left" }}>
            {!inbound && <Handle type="target" position={Position.Left} id={p.name} className="hl-handle" style={{ background: typeColor(p.ty) }} />}
            <div className="hl-port__label" style={{ color: typeColor(p.ty) }} title={typeLabel(p.ty)}>
              {p.name}
            </div>
            <div style={{ color: "var(--ink-dim)", fontSize: 10, whiteSpace: "nowrap" }}>
              {p.outside.length === 0 ? "not wired outside" : `${inbound ? "← " : "→ "}${p.outside.join(", ")}`}
            </div>
            {inbound && <Handle type="source" position={Position.Right} id={p.name} className="hl-handle" style={{ background: typeColor(p.ty) }} />}
          </div>
        ))}
      </div>
    </div>
  );
}

const nodeTypes = { primitive: PrimitiveNode, boundary: BoundaryNode } as unknown as NodeTypes;

/** One level's canvas: its own React Flow, so the nodes in it act on it, not on the flow. */
function Level({ frame, open, grown }: { frame: OverlayFrame; open: (id: string, nodes: Node[], edges: Edge[]) => void; grown: boolean }) {
  const [nodes, , onNodesChange] = useNodesState(frame.nodes);
  const [edges, , onEdgesChange] = useEdgesState(frame.edges);
  // Fitted again once the overlay has grown: it first lays out in the node's small box.
  const { fitView } = useReactFlow();
  useEffect(() => {
    if (!grown) return;
    const t = setTimeout(() => fitView({ padding: 0.15, duration: 200 }), 240);
    return () => clearTimeout(t);
  }, [grown, fitView]);
  const outer = useNodeActions();
  // A composite inside expands in turn; nothing here ungroups or updates.
  const actions = { ...outer, expandComposite: (id: string) => open(id, nodes, edges), ungroup: () => {}, updateHolon: () => {} };
  return (
    <NodeActions.Provider value={actions}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        nodeTypes={nodeTypes}
        nodesConnectable={false}
        deleteKeyCode={null}
        fitView
        fitViewOptions={{ padding: 0.2 }}
        minZoom={0.3}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
      >
        <Background variant={BackgroundVariant.Lines} gap={26} color="#1b2a45" />
        <Controls showInteractive={false} />
      </ReactFlow>
    </NodeActions.Provider>
  );
}

const button: CSSProperties = {
  font: "inherit",
  fontSize: 11,
  padding: "2px 10px",
  color: "#9fb4d4",
  background: "transparent",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  cursor: "pointer",
};

/**
 * The opened composites, outermost first, over the canvas. `from` is where the first one sat on
 * the canvas (relative to it): the overlay grows from there, and shrinks back to it on close.
 * `inside` opens a composite node found on a level (its inner canvas, or why it can't be).
 */
export function CompositeOverlay({
  root,
  frames,
  from,
  inside,
  onFrames,
  onClose,
}: {
  /** The flow's own name, first in the breadcrumb. */
  root: string;
  frames: OverlayFrame[];
  from: { left: number; top: number; width: number; height: number };
  inside: (target: PrimitiveNodeType, nodes: Node[], edges: Edge[]) => OverlayFrame | string;
  onFrames: (frames: OverlayFrame[]) => void;
  onClose: () => void;
}) {
  const [grown, setGrown] = useState(false);
  const [msg, setMsg] = useState("");
  useEffect(() => {
    const t = requestAnimationFrame(() => setGrown(true));
    return () => cancelAnimationFrame(t);
  }, []);
  const close = () => {
    setGrown(false);
    setTimeout(onClose, 220);
  };
  const back = (to: number) => (to < 0 ? close() : onFrames(frames.slice(0, to + 1)));
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && back(frames.length - 2);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  const open = (id: string, nodes: Node[], edges: Edge[]) => {
    const target = nodes.find((n) => n.id === id) as PrimitiveNodeType | undefined;
    if (!target) return;
    const frame = inside(target, nodes, edges);
    if (typeof frame === "string") return setMsg(frame);
    setMsg("");
    onFrames([...frames, frame]);
  };
  const level = frames[frames.length - 1];
  const at: CSSProperties = grown ? { left: 8, top: 8, width: "calc(100% - 16px)", height: "calc(100% - 16px)" } : from;
  return (
    <div
      className="hl-overlay"
      role="dialog"
      aria-label={`inside ${level.title}`}
      style={{ ...at, opacity: grown ? 1 : 0.4 }}
    >
      <div className="hl-overlay__bar">
        <span className="hl-overlay__crumbs">
          <button style={{ ...button, border: "none", padding: "2px 4px" }} onClick={close} title="back to the flow (Esc at the top level)">
            {root}
          </button>
          {frames.map((f, i) => (
            <span key={i}>
              <span style={{ color: "var(--ink-dim)" }}> › </span>
              <button
                style={{ ...button, border: "none", padding: "2px 4px", color: i === frames.length - 1 ? "#e6eefb" : "#9fb4d4" }}
                onClick={() => back(i)}
                disabled={i === frames.length - 1}
              >
                {f.title}
              </button>
            </span>
          ))}
        </span>
        <span style={{ color: "var(--ink-dim)", fontSize: 11 }}>inside, read only · select a composite to expand it too</span>
        {msg && <span style={{ color: "#e0916f", fontSize: 11 }}>{msg}</span>}
        <button style={{ ...button, marginLeft: "auto" }} onClick={close} title="close (Esc)">
          close
        </button>
      </div>
      <div className="hl-overlay__flow">
        <ReactFlowProvider key={frames.length}>
          <Level frame={level} open={open} grown={grown} />
        </ReactFlowProvider>
      </div>
    </div>
  );
}
