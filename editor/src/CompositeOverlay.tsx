// Inside a composite: "expand" grows the node into a canvas of its own over the flow, showing its
// inner flow (canvasOps.innerCanvas): its nodes, its wires, and its boundary, inputs on the left
// and outputs on the right, each labelled with what it is wired to outside. A composite inside
// expands in turn; the breadcrumb goes back.
//
// In edit mode it changes like the flow: params, wires, nodes dropped from the palette or deleted,
// a composite inside ungrouped, and the boundary's ports added, renamed or removed. A composite is
// content-addressed, so a change makes a new one: "apply" freezes each open level back into a
// composite, deepest first (canvasOps.freezeInner), and puts the outermost in place of the node in
// the flow (undo takes it back). Going back a level folds that level's changes into the one
// above; closing with changes not applied asks first. In view mode it is read only.

import { createContext, useCallback, useContext, useEffect, useRef, useState, type CSSProperties, type DragEvent } from "react";
import {
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
  type NodeProps,
  type NodeTypes,
} from "@xyflow/react";
import { PrimitiveNode, type PrimitiveNodeType } from "./PrimitiveNode";
import { NodeActions, useNodeActions } from "./NodeDocs";
import {
  boundaryRenames,
  replaceComposite,
  settleBoundary,
  type BoundaryData,
  type BoundaryPort,
} from "./composites";
import { compatible, type WPort } from "./holon";
import { outputColor, typeColor, typeLabel } from "./theme";
import { DND_MIME, FLOW_PREFIX } from "./Palette";
import { useOptionalDialog } from "./Dialog";
import { History } from "./history";

/** One level opened: the composite's name, its node in the level above (or in the flow, for the
 *  first), its inner canvas as it stands, and whether that was changed. */
export type OverlayFrame = { title: string; nodeId: string; nodes: Node[]; edges: Edge[]; dirty: boolean };

/** A level frozen back into a composite (canvasOps.freezeInner's answer). */
export type Frozen = { ok: true; hash: string; inputs: WPort[]; outputs: WPort[] } | { ok: false; message: string; nodeIds: string[] };
export type FrozenOk = Extract<Frozen, { ok: true }>;
/** Boundary ports renamed inside: old name → new, per side (canvasOps.boundaryRenames). */
export type Renames = ReturnType<typeof boundaryRenames>;

/** What the overlay asks the editor for. */
export type OverlayOps = {
  /** A composite node found on a level, opened as a level of its own (or why it can't be). */
  inside: (target: PrimitiveNodeType, nodes: Node[], edges: Edge[]) => OverlayFrame | string;
  /** A level frozen back into a composite. */
  freeze: (canvas: { nodes: Node[]; edges: Edge[] }) => Frozen;
  /** A palette entry as a node at `position` (null when it isn't one). */
  paletteNode: (name: string, position: { x: number; y: number }) => Node | null;
  /** A composite on a level replaced by its nodes there (or why it can't be). */
  ungroupIn: (canvas: { nodes: Node[]; edges: Edge[] }, id: string) => { nodes: Node[]; edges: Edge[] } | string;
};

/** A level as undo keeps it: selection and drag flags left out, as on the flow, and the sizes
 *  React Flow measures (a level opens unmeasured; measuring it is no edit). */
type LevelSnapshot = { nodes: Node[]; edges: Edge[]; dirty: boolean };
const levelSnapshot = (f: OverlayFrame): LevelSnapshot => ({
  nodes: f.nodes.map(({ measured: _m, width: _w, height: _h, ...n }) => ({ ...n, selected: false, dragging: false })),
  edges: f.edges.map((e) => ({ ...e, selected: false })),
  dirty: f.dirty,
});
const sameLevel = (a: LevelSnapshot, b: LevelSnapshot) => JSON.stringify(a) === JSON.stringify(b);

/**
 * Undo and redo for the open level, as the flow has (useCanvasHistory): a snapshot committed
 * 400 ms after edits settle, Ctrl/Cmd+Z, Shift+Z and Y outside text fields. Each level keeps its
 * own history while it's open; going back a level drops the deeper ones' (their changes are
 * folded into the level above, one step of its history).
 */
function useLevelHistory(frames: OverlayFrame[], onFrames: (update: (fs: OverlayFrame[]) => OverlayFrame[]) => void, enabled: boolean) {
  const top = frames.length - 1;
  const level = frames[top];
  const histories = useRef(new Map<number, History<LevelSnapshot>>());
  const restoring = useRef(false);
  const [, setVersion] = useState(0);
  const historyOf = (k: number) => {
    let h = histories.current.get(k);
    if (!h) histories.current.set(k, (h = new History(levelSnapshot(frames[k]), sameLevel)));
    return h;
  };
  // Levels closed: their histories go with them.
  for (const k of [...histories.current.keys()]) if (k > top) histories.current.delete(k);
  useEffect(() => {
    const h = historyOf(top);
    if (restoring.current) {
      restoring.current = false;
      h.committed = levelSnapshot(level);
      return;
    }
    const t = setTimeout(() => h.commit(levelSnapshot(level)) && setVersion((v) => v + 1), 400);
    return () => clearTimeout(t);
    // historyOf reads the ref; the level's content and depth are what matter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [level.nodes, level.edges, level.dirty, top]);
  const step = useCallback(
    (back: boolean) => {
      if (!enabled) return;
      const h = historyOf(top);
      const s = back ? h.undo(levelSnapshot(level)) : h.redo(levelSnapshot(level));
      if (s) {
        restoring.current = true;
        onFrames((fs) => fs.map((f, i) => (i === top ? { ...f, ...s } : f)));
      }
      setVersion((v) => v + 1);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [enabled, top, level, onFrames],
  );
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) return;
      if (!(e.ctrlKey || e.metaKey)) return;
      const k = e.key.toLowerCase();
      if (k === "z") {
        e.preventDefault();
        step(!e.shiftKey);
      } else if (k === "y") {
        e.preventDefault();
        step(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [step]);
  const h = historyOf(top);
  return { undo: () => step(true), redo: () => step(false), canUndo: enabled && h.past.length > 0, canRedo: enabled && h.future.length > 0 };
}

/** Whether the levels can be changed (edit mode), for the boundary nodes. */
const Editable = createContext(false);


/** A composite's boundary inside its canvas: its ports, and what each is wired to outside. In
 *  edit mode a port can be added (+), renamed (double-click its name) or removed (×). */
function BoundaryNode({ id, data }: NodeProps<Node<BoundaryData>>) {
  const inbound = data.side === "in";
  const kind = inbound ? "input" : "output";
  const editable = useContext(Editable);
  const dialog = useOptionalDialog();
  const { updateNodeData, setEdges } = useReactFlow();
  const setPorts = (ports: BoundaryPort[]) => updateNodeData(id, { ports });
  const named = async (ask: string, initial: string, except?: string) => {
    const name = await dialog?.prompt(ask, initial);
    if (!name || name === except) return null;
    if (data.ports.some((p) => p.name === name)) {
      await dialog?.confirm(`"${name}" is already one of its ${kind}s.`, "OK");
      return null;
    }
    return name;
  };
  const add = async () => {
    const name = await named(`A new ${kind}'s name (then wire it inside):`, "");
    if (name) setPorts([...data.ports, { name, ty: "Any", outside: [] }]);
  };
  // A renamed port keeps its wires inside; outside, the flow's wires follow it (App's onApply).
  const rename = async (old: string) => {
    const name = await named(`Rename the ${kind} "${old}":`, old, old);
    if (!name) return;
    setPorts(data.ports.map((p) => (p.name === old ? { ...p, name } : p)));
    setEdges((es) =>
      es.map((e) =>
        inbound
          ? e.source === id && e.sourceHandle === old
            ? { ...e, id: `${e.id}~${name}`, sourceHandle: name }
            : e
          : e.target === id && e.targetHandle === old
            ? { ...e, id: `${e.id}~${name}`, targetHandle: name }
            : e,
      ),
    );
  };
  const remove = (name: string) => {
    setPorts(data.ports.filter((p) => p.name !== name));
    setEdges((es) => es.filter((e) => !(inbound ? e.source === id && e.sourceHandle === name : e.target === id && e.targetHandle === name)));
  };
  return (
    <div className="hl-node" style={{ minWidth: 150, borderStyle: "dashed" }}>
      <div className="hl-node__title" style={{ color: "#9fb4d4", display: "flex", justifyContent: "space-between", alignItems: "center", gap: 8 }}>
        <span>{inbound ? "inputs" : "outputs"}</span>
        {editable && (
          <button className="nodrag" onClick={add} style={smallButton} title={`add an ${kind}, then wire it inside`} aria-label={`add an ${kind}`}>
            +
          </button>
        )}
      </div>
      <div style={{ padding: "4px 0" }}>
        {data.ports.length === 0 && <div style={{ padding: "2px 12px", color: "var(--ink-dim)", fontSize: 11 }}>none</div>}
        {data.ports.map((p) => (
          // Two lines per port: its name, and what it is wired to outside; the handle centred on both.
          <div key={p.name} style={{ position: "relative", padding: "4px 14px", textAlign: inbound ? "right" : "left" }}>
            {!inbound && <Handle type="target" position={Position.Left} id={p.name} className="hl-handle" style={{ background: typeColor(p.ty) }} />}
            <div
              className={editable ? "hl-port__label nodrag" : "hl-port__label"}
              style={{ color: typeColor(p.ty), cursor: editable ? "text" : undefined }}
              title={typeLabel(p.ty) + (editable ? " · double-click to rename" : "")}
              onDoubleClick={editable ? () => rename(p.name) : undefined}
            >
              {p.name}
              {editable && (
                <button className="nodrag" onClick={() => remove(p.name)} style={{ ...smallButton, marginLeft: 6 }} title={`remove this ${kind}`} aria-label={`remove ${p.name}`}>
                  ×
                </button>
              )}
            </div>
            <div style={{ color: "var(--ink-dim)", fontSize: 10, whiteSpace: "nowrap" }}>
              {p.unit === undefined ? "new: wire it outside once applied" : p.outside.length === 0 ? "not wired outside" : `${inbound ? "← " : "→ "}${p.outside.join(", ")}`}
            </div>
            {inbound && <Handle type="source" position={Position.Right} id={p.name} className="hl-handle" style={{ background: typeColor(p.ty) }} />}
          </div>
        ))}
      </div>
    </div>
  );
}

const nodeTypes = { primitive: PrimitiveNode, boundary: BoundaryNode } as unknown as NodeTypes;

const smallButton: CSSProperties = {
  font: "inherit",
  fontSize: 10,
  lineHeight: 1,
  padding: "1px 5px",
  color: "#9fb4d4",
  background: "transparent",
  border: "1px solid #1b2a45",
  borderRadius: 3,
  cursor: "pointer",
};

/** Whether a change to a level's nodes changes the unit: moving or selecting one doesn't. */
const changesUnit = (c: NodeChange | EdgeChange) => c.type !== "select" && c.type !== "position" && c.type !== "dimensions";

/** One level's canvas: its own React Flow, so the nodes in it act on it, not on the flow. Its
 *  state is the frame's, kept by the overlay, so going between levels keeps every change. */
function Level({
  frame,
  onFrame,
  open,
  grown,
  editable,
  ops,
  onMessage,
}: {
  frame: OverlayFrame;
  onFrame: (update: (f: OverlayFrame) => OverlayFrame) => void;
  open: (id: string) => void;
  grown: boolean;
  editable: boolean;
  ops: OverlayOps;
  onMessage: (m: string) => void;
}) {
  const { fitView, screenToFlowPosition, getNode, getEdges } = useReactFlow();
  // Fitted again once the overlay has grown: it first lays out in the node's small box.
  useEffect(() => {
    if (!grown) return;
    const t = setTimeout(() => fitView({ padding: 0.15, duration: 200 }), 240);
    return () => clearTimeout(t);
  }, [grown, fitView]);

  const onNodesChange = (changes: NodeChange[]) => {
    // The boundary stays: an empty one is how a composite says it has no inputs or outputs.
    const kept = changes.filter((c) => !(c.type === "remove" && getNode(c.id)?.type === "boundary"));
    onFrame((f) => ({ ...f, nodes: applyNodeChanges(kept, f.nodes), dirty: f.dirty || kept.some(changesUnit) }));
  };
  const onEdgesChange = (changes: EdgeChange[]) =>
    onFrame((f) => {
      const edges = applyEdgeChanges(changes, f.edges);
      // A port added inside that lost its last wire takes any type again.
      const wired = (id: string, name: string) => edges.some((e) => (e.source === id && e.sourceHandle === name) || (e.target === id && e.targetHandle === name));
      const nodes = f.nodes.map((n) => {
        if (n.type !== "boundary") return n;
        const data = n.data as BoundaryData;
        if (!data.ports.some((p) => p.unit === undefined && p.ty !== "Any" && !wired(n.id, p.name))) return n;
        return { ...n, data: { ...data, ports: data.ports.map((p) => (p.unit === undefined && !wired(n.id, p.name) ? { ...p, ty: "Any" as const } : p)) } };
      });
      return { ...f, nodes, edges, dirty: f.dirty || changes.some(changesUnit) };
    });
  // As on the flow: a wire joins compatible types, and an input takes one wire.
  const portOf = (id: string, handle: string | null | undefined, side: "out" | "in") => {
    const node = getNode(id);
    if (!node) return undefined;
    if (node.type === "boundary") return (node.data as BoundaryData).ports.find((p) => p.name === handle);
    const data = node.data as PrimitiveNodeType["data"];
    return (side === "out" ? data.outputs : data.inputs).find((p) => p.name === handle);
  };
  const isValidConnection = (c: Connection | Edge) => {
    const from = portOf(c.source, c.sourceHandle, "out");
    const to = portOf(c.target, c.targetHandle, "in");
    if (!from || !to) return false;
    if (getEdges().some((e) => e.target === c.target && e.targetHandle === c.targetHandle)) return false;
    return compatible(from.ty, to.ty);
  };
  const onConnect = (c: Connection) => {
    const from = portOf(c.source, c.sourceHandle, "out");
    const to = portOf(c.target, c.targetHandle, "in");
    // A port added to the boundary takes its type from its first wire, so the next wire to it is
    // checked as it's drawn, not only when applied.
    const typed = (n: Node, handle: string | null | undefined, ty: WPort["ty"] | undefined) => {
      if (n.type !== "boundary" || !ty) return n;
      const data = n.data as BoundaryData;
      return { ...n, data: { ...data, ports: data.ports.map((p) => (p.name === handle && p.ty === "Any" ? { ...p, ty } : p)) } };
    };
    onFrame((f) => ({
      ...f,
      nodes: f.nodes.map((n) => (n.id === c.source ? typed(n, c.sourceHandle, to?.ty) : n.id === c.target ? typed(n, c.targetHandle, from?.ty) : n)),
      edges: addEdge({ ...c, style: { stroke: outputColor(from?.ty ?? null), strokeWidth: 1.6 } }, f.edges),
      dirty: true,
    }));
  };
  const onDrop = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    if (!editable) return;
    const name = e.dataTransfer.getData(DND_MIME);
    if (!name) return;
    if (name.startsWith(FLOW_PREFIX)) return onMessage("a saved flow goes on the flow's own canvas; group it in from there");
    const node = ops.paletteNode(name, screenToFlowPosition({ x: e.clientX, y: e.clientY }));
    if (node) onFrame((f) => ({ ...f, nodes: [...f.nodes, node], dirty: true }));
  };
  const outer = useNodeActions();
  // A composite inside expands in turn, and in edit mode ungroups in place, here.
  const actions = {
    ...outer,
    expandComposite: open,
    updateHolon: () => {},
    ungroup: (id: string) => {
      const done = ops.ungroupIn(frame, id);
      if (typeof done === "string") return onMessage(done);
      onFrame((f) => ({ ...f, ...done, dirty: true }));
    },
  };
  return (
    <NodeActions.Provider value={actions}>
      <div style={{ position: "absolute", inset: 0 }} onDragOver={(e) => editable && e.preventDefault()} onDrop={onDrop}>
        <ReactFlow
          nodes={frame.nodes}
          edges={frame.edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={editable ? onConnect : undefined}
          isValidConnection={isValidConnection}
          nodeTypes={nodeTypes}
          nodesConnectable={editable}
          deleteKeyCode={editable ? ["Backspace", "Delete"] : null}
          fitView
          fitViewOptions={{ padding: 0.2 }}
          minZoom={0.3}
          maxZoom={2}
          proOptions={{ hideAttribution: true }}
        >
          <Background variant={BackgroundVariant.Lines} gap={26} color="#1b2a45" />
          <Controls showInteractive={false} />
        </ReactFlow>
      </div>
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
 * Levels `to+1`… folded into level `to`, deepest first: each changed one frozen into a new version
 * of its composite, put in place of it in the level above (wires following renamed ports), and
 * itself settled on what it now is. Every level as it now stands, or why one can't be frozen.
 */
export function foldLevels(frames: OverlayFrame[], to: number, freeze: OverlayOps["freeze"]): OverlayFrame[] | string {
  const next = [...frames];
  for (let k = next.length - 1; k > to; k--) {
    if (!next[k].dirty) continue;
    const frozen = freeze(next[k]);
    if (!frozen.ok) return `${next[k].title}: ${frozen.message}`;
    const above = next[k - 1];
    const replaced = replaceComposite(above.nodes, above.edges, next[k].nodeId, frozen, boundaryRenames(next[k]));
    next[k - 1] = { ...above, nodes: replaced.nodes, edges: replaced.edges, dirty: true };
    next[k] = { ...settleBoundary(next[k], frozen), dirty: false };
  }
  return next;
}

/**
 * The opened composites, outermost first, over the canvas. `from` is where the first one sat on
 * the canvas (relative to it): the overlay grows from there, and shrinks back on close.
 * `onApply`: the outermost level frozen, to put in place of its node in the flow.
 */
export function CompositeOverlay({
  root,
  frames,
  from,
  editable,
  ops,
  onFrames,
  onApply,
  onClose,
}: {
  /** The flow's own name, first in the breadcrumb. */
  root: string;
  frames: OverlayFrame[];
  from: { left: number; top: number; width: number; height: number };
  editable: boolean;
  ops: OverlayOps;
  onFrames: (update: (frames: OverlayFrame[]) => OverlayFrame[]) => void;
  onApply: (nodeId: string, frozen: FrozenOk, renames: Renames) => void;
  onClose: () => void;
}) {
  const dialog = useOptionalDialog();
  const [grown, setGrown] = useState(false);
  const [msg, setMsg] = useState("");
  useEffect(() => {
    const t = requestAnimationFrame(() => setGrown(true));
    return () => cancelAnimationFrame(t);
  }, []);
  const dirty = frames.some((f) => f.dirty);
  const history = useLevelHistory(frames, onFrames, editable);

  const close = async () => {
    if (dirty && dialog && !(await dialog.confirm(`Discard the changes inside "${frames[0].title}"? They weren't applied to the flow.`, "Discard"))) return;
    setGrown(false);
    setTimeout(onClose, 220);
  };
  const back = (to: number) => {
    if (to < 0) return void close();
    const folded = foldLevels(frames, to, ops.freeze);
    if (typeof folded === "string") return setMsg(folded);
    setMsg("");
    onFrames(() => folded.slice(0, to + 1));
  };
  // Every open level frozen, deepest first, and the outermost put in place of its node in the
  // flow. The levels stay open, each now the composite it was frozen into.
  const apply = () => {
    const folded = foldLevels(frames, 0, ops.freeze);
    if (typeof folded === "string") return setMsg(folded);
    const frozen = ops.freeze(folded[0]);
    if (!frozen.ok) return setMsg(`${folded[0].title}: ${frozen.message}`);
    onApply(folded[0].nodeId, frozen, boundaryRenames(folded[0]));
    setMsg(`applied: "${frames[0].title}" in the flow is its new version (undo takes it back)`);
    onFrames(() => [{ ...settleBoundary(folded[0], frozen), dirty: false }, ...folded.slice(1)]);
  };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Escape in one of the editor's dialogs is that dialog's.
      if (e.key !== "Escape" || document.querySelector('[role="dialog"][aria-modal="true"]')) return;
      back(frames.length - 2);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
  const top = frames.length - 1;
  const level = frames[top];
  const open = (id: string) => {
    const target = level.nodes.find((n) => n.id === id) as PrimitiveNodeType | undefined;
    if (!target) return;
    const frame = ops.inside(target, level.nodes, level.edges);
    if (typeof frame === "string") return setMsg(frame);
    setMsg("");
    onFrames((fs) => [...fs, frame]);
  };
  const at: CSSProperties = grown ? { left: 8, top: 8, width: "calc(100% - 16px)", height: "calc(100% - 16px)" } : from;
  return (
    <Editable.Provider value={editable}>
      <div className="hl-overlay" role="dialog" aria-label={`inside ${level.title}`} style={{ ...at, opacity: grown ? 1 : 0.4 }}>
        <div className="hl-overlay__bar">
          <span className="hl-overlay__crumbs">
            <button style={{ ...button, border: "none", padding: "2px 4px" }} onClick={close} title="back to the flow">
              {root}
            </button>
            {frames.map((f, i) => (
              <span key={i}>
                <span style={{ color: "var(--ink-dim)" }}> › </span>
                <button
                  style={{ ...button, border: "none", padding: "2px 4px", color: i === top ? "#e6eefb" : "#9fb4d4" }}
                  onClick={() => back(i)}
                  disabled={i === top}
                  title={i === top ? undefined : "back to this level, the changes below folded into it"}
                >
                  {f.title}
                  {f.dirty ? " •" : ""}
                </button>
              </span>
            ))}
          </span>
          <span style={{ color: "var(--ink-dim)", fontSize: 11 }}>{editable ? "change it like the flow, then apply" : "inside, read only"}</span>
          {msg && <span style={{ color: msg.startsWith("applied") ? "#9ad29f" : "#e0916f", fontSize: 11 }}>{msg}</span>}
          <span style={{ marginLeft: "auto", display: "inline-flex", gap: 6 }}>
            {editable && (
              <>
                <button style={{ ...button, opacity: history.canUndo ? 1 : 0.4 }} onClick={history.undo} disabled={!history.canUndo} title="undo (Ctrl+Z)" aria-label="undo">
                  ⟲
                </button>
                <button style={{ ...button, opacity: history.canRedo ? 1 : 0.4 }} onClick={history.redo} disabled={!history.canRedo} title="redo (Ctrl+Shift+Z)" aria-label="redo">
                  ⟳
                </button>
              </>
            )}
            {editable && (
              <button
                style={{ ...button, ...(dirty ? { color: "#0e1a2f", background: "#6fd3e0", border: "none" } : { opacity: 0.4, cursor: "default" }) }}
                onClick={apply}
                disabled={!dirty}
                title="make these changes a new version of the composite, in its place in the flow"
              >
                apply
              </button>
            )}
            <button style={button} onClick={close} title="close (Esc at the top level)">
              close
            </button>
          </span>
        </div>
        <div className="hl-overlay__flow">
          <ReactFlowProvider key={top}>
            <Level
              frame={level}
              onFrame={(update) => onFrames((fs) => fs.map((x, i) => (i === top ? update(x) : x)))}
              open={open}
              grown={grown}
              editable={editable}
              ops={ops}
              onMessage={setMsg}
            />
          </ReactFlowProvider>
        </div>
      </div>
    </Editable.Provider>
  );
}
