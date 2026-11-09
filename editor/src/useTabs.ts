// The open flows, one tab each (tabs.ts): the canvas shows the active tab; the others are kept
// whole in `stash` and put back when picked. A flow is open in one tab at most: opening it again
// goes to its tab. A closed tab is kept whole too, the last few (`closed`, newest first), so
// the editor's Home can open it again as it was left. Leaving the page asks first while any tab
// has unsaved changes.

import { useCallback, useEffect, useRef, useState, type MutableRefObject } from "react";
import type { Edge, Viewport } from "@xyflow/react";
import { freshTab, nextAfterClose, type FlowMeta, type TabState } from "./tabs";
import type { History } from "./history";
import type { Snapshot } from "./useCanvasHistory";
import type { FlowDocument } from "./server";
import type { PrimitiveNodeType } from "./PrimitiveNode";

/** How many closed tabs are kept to open again. */
const RECENTLY_CLOSED = 8;

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
  // No flow is open to begin with: the canvas keeps an empty slot (0) the first flow opened takes.
  const [tabIds, setTabIds] = useState<number[]>([]);
  const [activeId, setActiveId] = useState(0);
  const stash = useRef(new Map<number, TabState>());
  const nextTabId = useRef(1);
  const [closed, setClosed] = useState<TabState[]>([]);
  /** Keep `t` among the recently closed: not an empty, untitled draft, and a flow once. */
  const keepClosed = useCallback((t: TabState) => {
    if (t.meta.name === null && t.meta.viewing === null && t.nodes.length === 0) return;
    const same = (x: TabState) => x.meta.name === t.meta.name && x.meta.viewing === t.meta.viewing && (x.meta.name !== null || x === t);
    setClosed((cs) => [t, ...cs.filter((x) => !same(x))].slice(0, RECENTLY_CLOSED));
  }, []);

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
      keepClosed(t);
      if (id !== activeId) {
        stash.current.delete(id);
        setTabIds((ids) => ids.filter((x) => x !== id));
        return;
      }
      const next = nextAfterClose(tabIds, id);
      if (next === null) {
        // The last flow closes: none is open, the canvas is cleared for the next one.
        setTabIds([]);
        applyTab(freshTab(null));
        return;
      }
      const nt = stash.current.get(next) ?? freshTab(null);
      stash.current.delete(next);
      setTabIds((ids) => ids.filter((x) => x !== id));
      applyTab(nt);
      setActiveId(next);
    },
    [activeId, captureTab, confirm, tabIds, applyTab, keepClosed],
  );

  /** Open the recently closed tab `at` again, as it was left (in its tab, if its flow is open
   *  again since). */
  const reopen = useCallback(
    (at: number) => {
      const t = closed[at];
      if (!t) return;
      setClosed((cs) => cs.filter((x) => x !== t));
      const open = t.meta.name === null ? null : tabOf(t.meta.name);
      showTab(open, t);
    },
    [closed, tabOf, showTab],
  );

  // Leaving the page asks first while any tab has unsaved changes.
  const anyDirty = dirty || [...stash.current.values()].some((t) => t.dirty);
  useEffect(() => {
    if (!anyDirty) return;
    const warn = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [anyDirty]);

  return { tabIds, activeId, stash, captureTab, applyTab, showTab, switchTab, tabOf, closeTab, closed, reopen };
}
