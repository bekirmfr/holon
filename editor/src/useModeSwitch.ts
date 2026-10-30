// Going between a flow's two modes (tabs.ts's Mode). View: a deployment, read only, with its runs,
// in its flow's tab (that tab's edit copy set aside until edit mode) or a tab of its own. Edit:
// the flow as edited, put back as it was left; for a tab opened to view, the saved flow of that
// name, else a draft of what is deployed. What view mode then shows is useViewMode.tsx's.

import { useCallback, type MutableRefObject } from "react";
import type { Edge, Viewport } from "@xyflow/react";
import { restoreDocument } from "./docUnits";
import { blankTab, settingsOf, type FlowMeta, type Mode, type TabState } from "./tabs";
import { getDeployedDocument, getFlow, getLog, type DeploymentEntry, type FlowDocument } from "./server";
import { newHistory, snapshotOf, type Snapshot } from "./useCanvasHistory";
import type { History } from "./history";
import type { Evaluator } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";

export function useModeSwitch({
  flow,
  ev,
  deployments,
  tabs,
  tabFromDocument,
  canvas,
  setters,
  setError,
}: {
  flow: FlowMeta;
  ev: Evaluator;
  deployments: DeploymentEntry[];
  /** The open tabs (useTabs.ts). */
  tabs: {
    activeId: number;
    stash: MutableRefObject<Map<number, TabState>>;
    captureTab: () => TabState;
    showTab: (id: number | null, t: TabState) => void;
    tabOf: (name: string) => number | null;
  };
  tabFromDocument: (name: string | null, doc: FlowDocument, saved: boolean) => TabState;
  /** The canvas, to put an edit copy back. */
  canvas: {
    beforeView: MutableRefObject<TabState["beforeView"]>;
    viewDoc: MutableRefObject<FlowDocument | null>;
    history: MutableRefObject<History<Snapshot>>;
    restoring: MutableRefObject<boolean>;
    setNodes: (ns: PrimitiveNodeType[]) => void;
    setEdges: (es: Edge[]) => void;
    setViewport: (v: Viewport) => void;
    bump: () => void;
  };
  /** The flow's view-mode fields. */
  setters: {
    setViewing: (slug: string | null) => void;
    setViewTrace: (t: FlowMeta["viewTrace"]) => void;
    setInspectNode: (id: FlowMeta["inspectNode"]) => void;
    setSelectedRun: (run: FlowMeta["selectedRun"]) => void;
    setStep: (step: FlowMeta["step"]) => void;
  };
  setError: (msg: string) => void;
}) {
  const { activeId, stash, captureTab, showTab, tabOf } = tabs;
  const { beforeView, viewDoc, history, restoring, setNodes, setEdges, setViewport, bump } = canvas;
  const { setViewing, setViewTrace, setInspectNode, setSelectedRun, setStep } = setters;
  const viewing = flow.viewing;
  const flowName = flow.name;

  const onView = useCallback(
    async (slug: string, seq?: number) => {
      let doc: FlowDocument;
      try {
        doc = await getDeployedDocument(slug);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return;
      }
      // Opened on a run (an action's view run): that run selected, when the log has it.
      const run = seq === undefined ? null : await getLog(slug, seq - 1).then((l) => l.entries.find((e) => e.seq === seq) ?? null, () => null);
      // A unit that won't decode just stays unresolvable, like on Open.
      restoreDocument(doc, ev);
      const open = tabOf(slug);
      const base = open === null ? blankTab(slug) : open === activeId ? captureTab() : (stash.current.get(open) ?? blankTab(slug));
      const editCopy = base.meta.viewing
        ? base.beforeView
        : open === null
          ? null
          : { nodes: base.nodes, edges: base.edges, viewport: base.viewport, history: base.history, dirty: base.dirty };
      showTab(open, {
        ...base,
        beforeView: editCopy,
        nodes: doc.nodes as PrimitiveNodeType[],
        edges: doc.edges as Edge[],
        viewport: doc.viewport,
        history: newHistory(snapshotOf([], [])),
        viewDoc: doc,
        meta: {
          ...base.meta,
          settings: settingsOf(doc.server),
          viewing: slug,
          viewUnit: deployments.find((d) => d.slug === slug)?.unit ?? null,
          viewTrace: doc.trace,
          selectedRun: run,
          step: null,
          inspectNode: null,
        },
      });
    },
    [ev, tabOf, activeId, captureTab, stash, showTab, deployments, setError],
  );

  const onEditMode = useCallback(async () => {
    const before = beforeView.current;
    if (!viewing) return;
    if (before) {
      beforeView.current = null;
      viewDoc.current = null;
      setViewing(null);
      setViewTrace(undefined);
      setInspectNode(null);
      setSelectedRun(null);
      setStep(null);
      history.current = before.history;
      restoring.current = true;
      setNodes(before.nodes);
      setEdges(before.edges);
      setViewport(before.viewport);
      bump();
      return;
    }
    // What is deployed now, not an earlier version a run may be shown on.
    const doc = await getDeployedDocument(viewing).catch(() => viewDoc.current);
    let t: TabState | null = null;
    if (flowName) {
      try {
        t = tabFromDocument(flowName, (await getFlow(flowName)).document, true);
      } catch {
        // not saved: start from what is deployed
      }
    }
    if (!t && doc) t = tabFromDocument(flowName, doc, false);
    if (t) showTab(activeId, t);
  }, [viewing, flowName, beforeView, viewDoc, history, restoring, setNodes, setEdges, setViewport, bump, tabFromDocument, showTab, activeId, setViewing, setViewTrace, setInspectNode, setSelectedRun, setStep]);

  const onMode = useCallback(
    (m: Mode) => {
      if (m === "edit") onEditMode();
      else if (flowName) onView(flowName);
    },
    [onEditMode, onView, flowName],
  );

  return { onView, onMode };
}
