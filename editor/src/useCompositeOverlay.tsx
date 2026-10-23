// A composite opened over the canvas (CompositeOverlay.tsx), as the editor drives it: "expand"
// opens it, grown from where the node sits; its levels are read only in view mode (with a run
// selected, its nodes show what they produced in it: the log keeps one level inside each node)
// and changed like the flow's in edit mode; "apply" puts the new version in place of the node in
// the flow. Another tab, or the other mode, closes it.

import { useCallback, useEffect, useMemo, useState, type MutableRefObject } from "react";
import type { Edge, Node } from "@xyflow/react";
import { CompositeOverlay, type FrozenOk, type OverlayFrame, type OverlayOps, type Renames } from "./CompositeOverlay";
import { expandComposite, freezeInner, innerCanvas, replaceComposite } from "./composites";
import { nodeFromPalette } from "./canvasOps";
import { nameOf } from "./docUnits";
import { innerSteps } from "./replay";
import type { Evaluator, WUnit } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { LogEntry } from "./server";

type Box = { left: number; top: number; width: number; height: number };

export function useCompositeOverlay({
  live,
  ev,
  byName,
  variadic,
  nextId,
  viewing,
  run,
  closeOn,
  setNodes,
  setEdges,
  onMessage,
  onError,
}: {
  /** The flow's canvas as of this render. */
  live: MutableRefObject<{ nodes: PrimitiveNodeType[]; edges: Edge[] }>;
  ev: Evaluator;
  byName: Map<string, WUnit>;
  variadic: Set<string>;
  /** A fresh number for a node id. */
  nextId: () => number;
  /** View mode: read only, and the selected run's values shown. */
  viewing: boolean;
  run: { selectedRun: LogEntry | null; viewTrace: Record<string, string[]> | undefined };
  /** Closes it when it changes (the active tab). */
  closeOn: unknown;
  setNodes: (ns: PrimitiveNodeType[]) => void;
  setEdges: (es: Edge[]) => void;
  /** Said once a new version is applied. */
  onMessage: (m: string) => void;
  /** Why a composite can't be opened. */
  onError: (m: string) => void;
}) {
  const [overlay, setOverlay] = useState<{ frames: OverlayFrame[]; from: Box } | null>(null);
  useEffect(() => setOverlay(null), [closeOn, viewing]);

  const editableInside = useCallback(
    (ns: Node[]) => (viewing ? ns : ns.map((n) => (n.type === "primitive" ? { ...n, data: { ...n.data, readOnly: false } } : n))),
    [viewing],
  );
  const inside = useCallback(
    (target: PrimitiveNodeType, around: Node[], wires: Edge[]): OverlayFrame | string => {
      const opened = innerCanvas(target, around.filter((n) => n.type === "primitive") as PrimitiveNodeType[], wires, ev, byName, variadic, nameOf);
      return opened.ok ? { title: target.data.name, nodeId: target.id, nodes: editableInside(opened.nodes), edges: opened.edges, dirty: false } : opened.message;
    },
    [ev, byName, variadic, editableInside],
  );
  // What its levels need to change: freeze a level, place a palette entry, ungroup.
  const ops = useMemo<OverlayOps>(
    () => ({
      inside,
      freeze: (canvas) => freezeInner(canvas, ev),
      paletteNode: (name, position) => nodeFromPalette(name, byName, variadic, `${name}-${nextId()}`, position),
      ungroupIn: (canvas, id) => {
        const target = canvas.nodes.find((n) => n.id === id) as PrimitiveNodeType | undefined;
        if (!target || target.data.body.kind !== "Composite") return "not a composite";
        const result = expandComposite(target, canvas.edges, ev, byName, variadic, nextId, nameOf);
        return result.ok ? { nodes: [...canvas.nodes.filter((n) => n.id !== id), ...result.nodes], edges: result.edges } : result.message;
      },
    }),
    [inside, ev, byName, variadic, nextId],
  );
  // Apply: the composite in the flow replaced by its new version; wires follow renamed ports,
  // and wires to ports it no longer has are removed, saying so. The flow's undo takes it back.
  const apply = useCallback(
    (id: string, frozen: FrozenOk, renames: Renames) => {
      const done = replaceComposite(live.current.nodes, live.current.edges, id, frozen, renames);
      setNodes(done.nodes);
      setEdges(done.edges);
      const name = live.current.nodes.find((n) => n.id === id)?.data.name ?? "composite";
      onMessage(`"${name}" is a new version${done.dropped.length ? `; ${done.dropped.length} wire(s) to ports it no longer has removed` : ""}`);
    },
    [live, setNodes, setEdges, onMessage],
  );
  const expand = useCallback(
    (id: string) => {
      const target = live.current.nodes.find((n) => n.id === id);
      if (!target) return;
      const opened = innerCanvas(target, live.current.nodes, live.current.edges, ev, byName, variadic, nameOf);
      if (!opened.ok) return onError(opened.message);
      let frameNodes = opened.nodes;
      if (viewing && run.selectedRun) {
        const steps = innerSteps(run.selectedRun, run.viewTrace, id);
        frameNodes = frameNodes.map((n) => {
          const step = steps.find((st) => opened.childIds[st.child] === n.id);
          return step ? { ...n, data: { ...n.data, trace: { outputs: step.outputs ?? [], state: "done", showValues: true } } } : n;
        });
      }
      const box = document.querySelector(".hl-canvas__flow")?.getBoundingClientRect();
      const at = document.querySelector(`.hl-canvas__flow .react-flow__node[data-id="${CSS.escape(id)}"]`)?.getBoundingClientRect();
      const from = box && at ? { left: at.left - box.left, top: at.top - box.top, width: at.width, height: at.height } : { left: 0, top: 0, width: 200, height: 100 };
      setOverlay({ frames: [{ title: target.data.name, nodeId: id, nodes: editableInside(frameNodes), edges: opened.edges, dirty: false }], from });
    },
    [live, ev, byName, variadic, viewing, run.selectedRun, run.viewTrace, editableInside, onError],
  );

  /** The overlay itself, over the canvas (null when nothing is open). */
  const render = (root: string) =>
    overlay && (
      <CompositeOverlay
        root={root}
        frames={overlay.frames}
        from={overlay.from}
        editable={!viewing}
        ops={ops}
        onFrames={(update) => setOverlay((o) => (o ? { ...o, frames: update(o.frames) } : o))}
        onApply={apply}
        onClose={() => setOverlay(null)}
      />
    );
  return { open: overlay !== null, expand, render };
}
