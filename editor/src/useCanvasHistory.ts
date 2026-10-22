// Undo/redo for the canvas: a debounced snapshot history over {nodes, edges}.

import { useCallback, useEffect, useRef, useState } from "react";
import type { Edge } from "@xyflow/react";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { History } from "./history";

export type Snapshot = { nodes: PrimitiveNodeType[]; edges: Edge[] };

export const newHistory = (s: Snapshot) => new History<Snapshot>(s, (a, b) => JSON.stringify(a) === JSON.stringify(b));

// Selection and drag flags are normalized out of snapshots — undoing a *selection* would be
// noise, and restoring a node mid-"dragging" would wedge it to the cursor.
export const snapshotOf = (ns: PrimitiveNodeType[], es: Edge[]): Snapshot => ({
  nodes: ns.map((n) => ({ ...n, selected: false, dragging: false })),
  edges: es.map((e) => ({ ...e, selected: false })),
});

// React Flow has no built-in history, so this is the standard minimal shape: commit a snapshot
// 400ms after the canvas settles (so a drag is ONE entry, not sixty), undo/redo swap between
// past/committed/future. `restoring` marks programmatic setNodes/setEdges (undo, redo, open, new,
// view) so the watcher records them as the new baseline instead of pushing them into history as
// if the user had made an edit. The bookkeeping itself lives in ./history (History), which
// undo/redo hand the live canvas so an edit still inside the debounce window is committed first
// rather than lost. `history` is a ref the caller may swap (view mode sets one aside);
// `histVersion` changes whenever the undo/redo buttons' state may have.
export function useCanvasHistory(
  nodes: PrimitiveNodeType[],
  edges: Edge[],
  setNodes: (ns: PrimitiveNodeType[]) => void,
  setEdges: (es: Edge[]) => void,
  disabled: boolean,
) {
  const history = useRef(newHistory({ nodes: [], edges: [] }));
  const restoring = useRef(false);
  const [histVersion, setHistVersion] = useState(0);
  const bump = useCallback(() => setHistVersion((v) => v + 1), []);
  const live = useRef({ nodes, edges });
  live.current = { nodes, edges };

  useEffect(() => {
    if (restoring.current) {
      restoring.current = false;
      history.current.committed = snapshotOf(nodes, edges);
      return;
    }
    const t = setTimeout(() => {
      if (history.current.commit(snapshotOf(nodes, edges))) bump();
    }, 400);
    return () => clearTimeout(t);
  }, [nodes, edges, bump]);

  const applySnapshot = useCallback(
    (s: Snapshot) => {
      restoring.current = true;
      setNodes(s.nodes);
      setEdges(s.edges);
    },
    [setNodes, setEdges],
  );

  const undo = useCallback(() => {
    if (disabled) return;
    const prev = history.current.undo(snapshotOf(live.current.nodes, live.current.edges));
    if (prev) applySnapshot(prev);
    bump();
  }, [applySnapshot, bump, disabled]);

  const redo = useCallback(() => {
    if (disabled) return;
    const next = history.current.redo(snapshotOf(live.current.nodes, live.current.edges));
    if (next) applySnapshot(next);
    bump();
  }, [applySnapshot, bump, disabled]);

  // Ctrl/Cmd+Z / Shift+Z / Y — skipped entirely while a text field has focus, so undo inside a
  // param input stays the browser's own text undo, never a canvas mutation under the cursor.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable)) {
        return;
      }
      if (!(e.ctrlKey || e.metaKey)) return;
      const k = e.key.toLowerCase();
      if (k === "z") {
        e.preventDefault();
        if (e.shiftKey) redo();
        else undo();
      } else if (k === "y") {
        e.preventDefault();
        redo();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [undo, redo]);

  return { history, restoring, histVersion, bump, undo, redo };
}
