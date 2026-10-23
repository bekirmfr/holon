// The open flows, one tab each (tabs.ts): the canvas shows the active tab; the others are kept
// whole in `stash` and put back when picked. A flow is open in one tab at most: opening it again
// goes to its tab. Leaving the page asks first while any tab has unsaved changes.

import { useCallback, useEffect, useRef, useState, type MutableRefObject } from "react";
import type { Edge, Viewport } from "@xyflow/react";
import { freshTab, nextAfterClose, type FlowMeta, type TabState } from "./tabs";
import type { History } from "./history";
import type { Snapshot } from "./useCanvasHistory";
import type { FlowDocument } from "./server";
import type { PrimitiveNodeType } from "./PrimitiveNode";

export function useTabs({
  flow,
  setFlow,
  live,
  dirty,
  getViewport,
  setViewport,
  history,
  restoring,
  bump,
  idRef,
  beforeView,
  viewDoc,
  setNodes,
  setEdges,
  resetProjectionView,
  showCanvas,
  confirm,
}: {
  /** The active tab's flow (FlowMeta), and setting it whole. */
  flow: FlowMeta;
  setFlow: (meta: FlowMeta) => void;
  live: MutableRefObject<{ nodes: PrimitiveNodeType[]; edges: Edge[] }>;
  dirty: boolean;
  getViewport: () => Viewport;
  setViewport: (v: Viewport) => void;
  history: MutableRefObject<History<Snapshot>>;
  /** Marks the next canvas change as a restore, not an edit (useCanvasHistory). */
  restoring: MutableRefObject<boolean>;
  bump: () => void;
  idRef: MutableRefObject<number>;
  beforeView: MutableRefObject<TabState["beforeView"]>;
  viewDoc: MutableRefObject<FlowDocument | null>;
  setNodes: (ns: PrimitiveNodeType[]) => void;
  setEdges: (es: Edge[]) => void;
  /** No projection, error highlight or inspect panel carried over from another flow. */
  resetProjectionView: () => void;
  /** Bring the canvas page to the front. */
  showCanvas: () => void;
  confirm: (message: string, action?: string) => Promise<boolean>;
}) {
  const flowName = flow.name;
  const viewing = flow.viewing;
  const [tabIds, setTabIds] = useState<number[]>([0]);
  const [activeId, setActiveId] = useState(0);
  const stash = useRef(new Map<number, TabState>());
  const nextTabId = useRef(1);

  const captureTab = useCallback(
    (): TabState => ({
      meta: flow,
      nodes: live.current.nodes,
      edges: live.current.edges,
      viewport: getViewport(),
      history: history.current,
      idCounter: idRef.current,
      beforeView: beforeView.current,
      viewDoc: viewDoc.current,
      dirty,
    }),
    [flow, live, getViewport, history, idRef, beforeView, viewDoc, dirty],
  );

  const applyTab = useCallback(
    (t: TabState) => {
      restoring.current = true;
      setNodes(t.nodes);
      setEdges(t.edges);
      setViewport(t.viewport);
      history.current = t.history;
      idRef.current = t.idCounter;
      beforeView.current = t.beforeView;
      viewDoc.current = t.viewDoc;
      setFlow(t.meta);
      resetProjectionView();
      bump();
    },
    [restoring, history, idRef, beforeView, viewDoc, setFlow, setNodes, setEdges, setViewport, resetProjectionView, bump],
  );

  /** Show `t` in tab `id`, or in a new tab (null) unless the active one is an empty, untouched,
   *  unnamed draft, which it takes over. The active tab is kept if it is another. */
  const showTab = useCallback(
    (id: number | null, t: TabState) => {
      const takeOver = id === null && flowName === null && !viewing && live.current.nodes.length === 0 && !dirty;
      const target = id ?? (takeOver ? activeId : nextTabId.current++);
      if (target !== activeId) stash.current.set(activeId, captureTab());
      stash.current.delete(target);
      setTabIds((ids) => (ids.includes(target) ? ids : [...ids, target]));
      applyTab(t);
      setActiveId(target);
      showCanvas();
    },
    [flowName, viewing, dirty, live, activeId, captureTab, applyTab, showCanvas],
  );

  const switchTab = useCallback(
    (id: number) => {
      const t = stash.current.get(id);
      if (id === activeId || !t) return showCanvas();
      showTab(id, t);
    },
    [activeId, showTab, showCanvas],
  );

  /** The tab `name` is open in, if any. */
  const tabOf = useCallback(
    (name: string): number | null => {
      if (flowName === name) return activeId;
      for (const [id, t] of stash.current) if (t.meta.name === name) return id;
      return null;
    },
    [flowName, activeId],
  );

  const closeTab = useCallback(
    async (id: number) => {
      const t = id === activeId ? captureTab() : stash.current.get(id);
      if (!t) return;
      if (t.dirty && !(await confirm(`Close "${t.meta.name ?? "untitled"}" and discard its unsaved changes?`, "Discard"))) return;
      if (id !== activeId) {
        stash.current.delete(id);
        setTabIds((ids) => ids.filter((x) => x !== id));
        return;
      }
      const next = nextAfterClose(tabIds, id);
      if (next === null) {
        // The last tab closes into a fresh draft.
        applyTab(freshTab(null));
        return;
      }
      const nt = stash.current.get(next) ?? freshTab(null);
      stash.current.delete(next);
      setTabIds((ids) => ids.filter((x) => x !== id));
      applyTab(nt);
      setActiveId(next);
    },
    [activeId, captureTab, confirm, tabIds, applyTab],
  );

  // Leaving the page asks first while any tab has unsaved changes.
  const anyDirty = dirty || [...stash.current.values()].some((t) => t.dirty);
  useEffect(() => {
    if (!anyDirty) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [anyDirty]);

  return { tabIds, activeId, stash, captureTab, applyTab, showTab, switchTab, tabOf, closeTab };
}
