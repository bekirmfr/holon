// View mode: a deployed flow's canvas, read only, with its runs. Everything that reads a run
// lives here: the selected run replayed on the canvas (and whether that reproduced it), the
// version it ran on (its own canvas, kept by the deployment), stepping through it, when each node
// ran, the flow state and collections as of a step, how nodes and wires show the run, and the
// side and bottom panes. App owns the canvas and the tab's flow state (tabs.ts's FlowMeta); this
// reads them and asks for changes through the setters it is given.

import { useCallback, useEffect, useMemo, useRef, useState, type MutableRefObject } from "react";
import type { Edge } from "@xyflow/react";
import type { Evaluator, WUnit } from "./holon";
import { FAIL, type PrimitiveNodeType } from "./PrimitiveNode";
import { restoreDocument } from "./docUnits";
import { valueChip } from "./diagnostics";
import {
  collectionsAtStep,
  diagnose,
  heldEffects,
  innerSteps,
  nodeExchanges,
  nodeTimings,
  failureWire,
  stateAtStep,
  whyNotRun,
} from "./replay";
import { replayAsViewed } from "./viewReplay";
import { printHolon } from "./holonText";
import { getDeployedDocument, getRun, wholeRun, type DeploymentEntry, type FlowDocument, type LogEntry } from "./server";
import { BottomPane, type InsideStep, type InspectorRun } from "./ViewPanes";
import type { RunOnCanvas } from "./runRows";
import { runNodes, type Drawn } from "./runNodes";
import type { FlowMeta } from "./tabs";

type Setter<T> = (value: T | ((prev: T) => T)) => void;

export type ViewModeInput = {
  /** The tab's flow, and setters for the view-mode fields of it. */
  flow: FlowMeta;
  setSelectedRun: Setter<LogEntry | null>;
  setStep: Setter<number | null>;
  setInspectNode: Setter<string | null>;
  setViewTrace: Setter<Record<string, string[]> | undefined>;
  setViewUnit: Setter<string | null>;
  /** The canvas: as of this render, and to set when a run's version brings its own. */
  nodes: PrimitiveNodeType[];
  edges: Edge[];
  live: MutableRefObject<{ nodes: PrimitiveNodeType[]; edges: Edge[] }>;
  canvasKey: string;
  setNodes: (ns: PrimitiveNodeType[]) => void;
  setEdges: (es: Edge[]) => void;
  /** Set before a programmatic canvas change, so it isn't recorded as an edit. */
  restoring: MutableRefObject<boolean>;
  /** The deployed document shown. */
  viewDoc: MutableRefObject<FlowDocument | null>;
  ev: Evaluator;
  deployments: DeploymentEntry[];
  /** Whether nodes show their outputs' values as chips (on), or the flow exactly as designed. */
  showValues: boolean;
  fitView: (options: { nodes: { id: string }[]; duration: number; maxZoom: number; padding: number }) => unknown;
};

export function useViewMode(input: ViewModeInput) {
  const { flow, setSelectedRun, setStep, setInspectNode, setViewTrace, setViewUnit } = input;
  const { nodes, edges, live, canvasKey, setNodes, setEdges, restoring, viewDoc, ev, deployments, fitView, showValues } = input;
  const { viewing, viewUnit, viewTrace, selectedRun, step } = flow;

  // The wire under the pointer, which shows the value it carried.
  const [hoveredWire, setHoveredWire] = useState<string | null>(null);

  // A run shows on the canvas of the version it ran on: selecting one logged on an earlier
  // version brings that version's canvas (kept by the deployment), and one on the current
  // version brings the current one back. Until it's there, or when it isn't kept, the replay
  // isn't compared with the run: it would be the wrong flow.
  const [versionNote, setVersionNote] = useState("");
  const runUnit = viewing ? selectedRun?.unit : undefined;
  const onOtherVersion = !!runUnit && viewUnit !== null && runUnit !== viewUnit;
  const viewed = viewing ? deployments.find((d) => d.slug === viewing) : undefined;
  const currentUnit = viewed?.unit;
  useEffect(() => {
    if (!viewing || !runUnit || runUnit === viewUnit) return;
    let stale = false;
    getDeployedDocument(viewing, runUnit)
      .then((doc) => {
        if (stale) return;
        // A unit that won't decode stays unresolvable, as on Open.
        restoreDocument(doc, ev);
        restoring.current = true;
        setNodes(doc.nodes as PrimitiveNodeType[]);
        setEdges(doc.edges as Edge[]);
        viewDoc.current = doc;
        setViewTrace(doc.trace);
        setViewUnit(runUnit);
        setVersionNote("");
      })
      .catch(() => !stale && setVersionNote(`ran on an earlier version (${runUnit.slice(0, 8)}…) whose canvas isn't kept: not compared`));
    return () => {
      stale = true;
    };
  }, [viewing, runUnit, viewUnit, ev, restoring, setNodes, setEdges, viewDoc, setViewTrace, setViewUnit]);
  // A version's flow as text, for the diff of a restore's dry run: printed, and with the names
  // its text gave its nodes when it was deployed from text, so two versions compare as they read.
  const codeText = useCallback(
    async (unit: string) => {
      const doc = await getDeployedDocument(viewing ?? "", unit);
      const plain = printHolon(viewing, doc, ev);
      return { plain, named: doc.bindings ? printHolon(viewing, doc, ev, new Map(Object.entries(doc.bindings))) : undefined };
    },
    [viewing, ev],
  );
  const note = onOtherVersion
    ? versionNote || "loading the version it ran on…"
    : viewUnit && currentUnit && viewUnit !== currentUnit
        ? `shown on the version it ran on (${viewUnit.slice(0, 8)}…), an earlier deploy`
        : "";

  // The selected run, replayed on the canvas; and whether that reproduced what the server sent.
  const replayed = useMemo(() => {
    if (!viewing || !selectedRun) return null;
    const { nodes: ns, edges: es } = live.current;
    const run = replayAsViewed(ns, es, selectedRun, viewTrace, ev);
    // Why it failed, and the effects it held and didn't make, as the server noted each.
    const diagnosis = diagnose(ns, es, selectedRun, run, heldEffects(selectedRun, viewTrace, ns));
    return { ...run, diagnosis };
    // live.current is read on purpose: canvasKey changes exactly when the structure does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewing, selectedRun, canvasKey, viewTrace, ev]);

  // Show a node on the canvas and in the side pane: what "show where it failed" does.
  const showNode = useCallback(
    (id: string) => {
      setInspectNode(id);
      fitView({ nodes: [{ id }], duration: 300, maxZoom: 1.1, padding: 0.6 });
    },
    [fitView, setInspectNode],
  );

  const fitToNodes = useCallback(
    (ids: string[]) => {
      if (ids.length === 0) return;
      fitView({ nodes: ids.map((id) => ({ id })), duration: 300, maxZoom: 1.1, padding: 0.2 });
    },
    [fitView],
  );

  // A failed run opens on where it failed.
  const origin = replayed?.diagnosis?.origins[0];
  useEffect(() => {
    if (origin) setInspectNode(origin);
  }, [origin, selectedRun?.seq, setInspectNode]);

  // The run picked is shown whole: one the list read brief (its values cut, useFlowLog) is read
  // again by its seq first, the last picked winning; one the log no longer keeps, or that can't be
  // read, is none, never its brief values replayed as if they were the run's.
  const picked = useRef(0);
  const onSelectRun = useCallback(
    (e: LogEntry | null) => {
      const n = ++picked.current;
      setStep(null);
      if (!e?.brief || !viewing) {
        setSelectedRun(e);
        return;
      }
      wholeRun(viewing, e).then(
        (whole) => picked.current === n && setSelectedRun(whole),
        () => picked.current === n && setSelectedRun(null),
      );
    },
    [setSelectedRun, setStep, viewing],
  );
  // A run named by its seq (a cell's history's row), read by it and opened.
  const openRun = useCallback(
    (seq: number) => {
      if (!viewing) return;
      getRun(viewing, seq)
        .then(({ run }) => {
          if (run) onSelectRun(run);
        })
        .catch(() => {});
    },
    [viewing, onSelectRun],
  );

  // Stepping through a run follows the node at that step in the side pane.
  const onStep = useCallback(
    (s: number | null) => {
      setStep(s);
      const id = s === null ? undefined : replayed?.order[s];
      if (id) setInspectNode(id);
    },
    [replayed, setStep, setInspectNode],
  );
  const onNodeClick = useCallback(
    (_: unknown, node: PrimitiveNodeType) => {
      if (viewing) setInspectNode(node.id);
    },
    [viewing, setInspectNode],
  );

  // When each node of the selected run ran, and the flow state as of the current step.
  const timings = useMemo(
    () => (selectedRun ? nodeTimings(selectedRun, viewTrace) : new Map<string, { start_us: number; end_us: number }>()),
    [selectedRun, viewTrace],
  );
  const exchanges = useMemo(() => (selectedRun ? nodeExchanges(selectedRun, viewTrace) : []), [selectedRun, viewTrace]);
  const flowState = useMemo(
    () => (selectedRun && replayed ? stateAtStep(selectedRun.state_before, replayed, live.current.nodes, step) : { after: {}, changed: [] as string[] }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [selectedRun, replayed, step],
  );
  // The collections the run touched, as items, as of the step shown.
  const collections = useMemo(
    () => (selectedRun ? collectionsAtStep(selectedRun, replayed, live.current.nodes, step) : []),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [selectedRun, replayed, step],
  );

  // A selected holon's own nodes, as the run went: each named by what it is (a primitive and its
  // settings, or a group), with what it produced.
  const inside = useMemo((): InsideStep[] | null => {
    const node = nodes.find((n) => n.id === flow.inspectNode);
    if (!selectedRun || !node || node.data.body.kind !== "Composite") return null;
    let children: string[];
    try {
      const unit = ev.get_unit(node.data.body.hash) as WUnit;
      children = unit.body.kind === "Composite" ? unit.body.children : [];
    } catch {
      return null;
    }
    const label = (hash: string | undefined) => {
      try {
        const u = ev.get_unit(hash ?? "") as WUnit;
        if (u.body.kind !== "Primitive") return "group";
        const settings = (u.params ?? []).flatMap((p) => (p.kind === "Frozen" && p.name !== "type" ? [JSON.stringify(p.value)] : []));
        return settings.length > 0 ? `${u.body.name} ${settings.join(" ")}` : u.body.name;
      } catch {
        return "?";
      }
    };
    return innerSteps(selectedRun, viewTrace, node.id).map((st) => ({ label: label(children[st.child]), outputs: st.outputs, us: st.end_us - st.start_us }));
  }, [nodes, flow.inspectNode, selectedRun, viewTrace, ev]);

  // The nodes as the run shows them (runNodes.ts): only those the new run changed made new.
  const drawnNodes = useRef(new Map<string, Drawn>());
  const viewNodes = useMemo(() => {
    const run = replayed && { order: replayed.order, notRun: replayed.notRun, traces: replayed.traces, issues: replayed.diagnosis?.issues };
    const { nodes: out, drawn } = runNodes(nodes, run, step, showValues, drawnNodes.current);
    drawnNodes.current = drawn;
    return out;
  }, [nodes, replayed, step, showValues]);

  // The run as the statement graph draws it (runRows.ts): what it ran, the step, each row's value
  // when values are shown, and where it failed.
  const onCanvas = useMemo(
    (): RunOnCanvas | null =>
      replayed ? { order: replayed.order, traces: replayed.traces, step, issues: replayed.diagnosis?.issues, values: showValues } : null,
    [replayed, step, showValues],
  );

  // The wires show the run: the way a failure took in its colour, a wire that carried nothing
  // faded, and the value a wire carried on hover.
  // A wire drawn as it was keeps its object too (as a node does, above).
  const shownEdges = useRef(new Map<string, { base: Edge; look: string; edge: Edge }>());
  const viewEdges = useMemo(() => {
    if (!replayed) return edges;
    const ran = new Set(replayed.order);
    const byId = new Map(nodes.map((n) => [n.id, n]));
    const was = shownEdges.current;
    const now = new Map<string, { base: Edge; look: string; edge: Edge }>();
    const kept = (e: Edge, look: string, make: () => Edge): Edge => {
      const before = was.get(e.id);
      const edge = before && before.base === e && before.look === look ? before.edge : make();
      now.set(e.id, { base: e, look, edge });
      return edge;
    };
    const out = edges.map((e) => {
      // A wire out of a node not run carried nothing, not even absence.
      if (replayed.notRun.has(e.source)) return kept(e, "not run", () => ({ ...e, style: { ...e.style, opacity: 0.2, strokeDasharray: "2 5" } }));
      if (!ran.has(e.source) || !ran.has(e.target)) return e;
      const from = byId.get(e.source);
      const at = from ? from.data.outputs.findIndex((p) => p.name === e.sourceHandle) : -1;
      const value = replayed.traces.get(e.source)?.outputs[at] ?? null;
      const hover =
        hoveredWire === e.id
          ? {
              label: valueChip(value),
              labelStyle: { fill: value === null ? "var(--faint)" : "var(--ok)", fontSize: 11, fontFamily: "var(--mono)" },
              labelBgStyle: { fill: "var(--bg)", stroke: "var(--accent)" },
              labelBgPadding: [6, 3] as [number, number],
              labelBgBorderRadius: 6,
            }
          : {};
      const failed = !!replayed.diagnosis && failureWire(replayed.diagnosis, e);
      const look = `${failed ? "failed" : value === null ? "empty" : "carried"}${hoveredWire === e.id ? ` hovered ${hover.label}` : ""}`;
      if (failed) return kept(e, look, () => ({ ...e, ...hover, animated: true, style: { ...e.style, stroke: FAIL, strokeWidth: 2.6 } }));
      return kept(e, look, () =>
        value === null ? { ...e, ...hover, style: { ...e.style, opacity: hoveredWire === e.id ? 1 : 0.35, strokeDasharray: "4 4" } } : { ...e, ...hover },
      );
    });
    shownEdges.current = now;
    return out;
  }, [edges, nodes, replayed, hoveredWire]);

  return {
    replayed,
    /** Not comparable with the run while its version's canvas isn't shown. */
    verdict: onOtherVersion ? null : (replayed?.verdict ?? null),
    /** The run is on a version whose canvas isn't shown: why there's no verdict, then. */
    otherVersion: onOtherVersion,
    note,
    /** Why the deployment isn't answering on its port, when it isn't. */
    listenError: viewed?.listen_error,
    codeText,
    timings,
    exchanges,
    flowState,
    collections,
    inside,
    viewNodes,
    viewEdges,
    onCanvas,
    setHoveredWire,
    showNode,
    fitToNodes,
    /** Picks a node for the side pane, without moving the canvas. */
    pickNode: setInspectNode,
    onSelectRun,
    openRun,
    onStep,
    onNodeClick,
  };
}

export type ViewMode = ReturnType<typeof useViewMode>;

/** What the Inspector shows of the run in view mode (null outside it): the flow state as of the
 *  step, and what the inspected node computed. */
export function inspectorRun(view: ViewMode, flow: FlowMeta): InspectorRun | null {
  const { viewing, selectedRun, step, inspectNode } = flow;
  if (!viewing) return null;
  const { replayed } = view;
  const trace = inspectNode ? replayed?.traces.get(inspectNode) : undefined;
  return {
    slug: viewing,
    run: selectedRun,
    step,
    order: replayed?.order ?? [],
    stateAfter: view.flowState.after,
    changed: view.flowState.changed,
    collections: view.collections,
    trace: trace && inspectNode ? { ...trace, issue: replayed?.diagnosis?.issues.get(inspectNode) } : undefined,
    notRun: inspectNode && replayed?.notRun.has(inspectNode) ? whyNotRun(view.viewNodes, replayed.notRun.get(inspectNode)!, replayed.held?.get(inspectNode)) : undefined,
    timing: inspectNode ? view.timings.get(inspectNode) : undefined,
    inside: view.inside,
    openRun: view.openRun,
    verdict: view.verdict,
    otherVersion: view.otherVersion,
  };
}

/** View mode's bottom pane: the log, the history and the timeline (the Inspector, on the right,
 *  is App's). `onRestored`: a restore from the history changed the deployment. */
export function ViewModePanes({ view, flow, nodes, onRestored, keys = true, onClose }: { view: ViewMode; flow: FlowMeta; nodes: PrimitiveNodeType[]; onRestored?: () => void; keys?: boolean; onClose?: () => void }) {
  const { viewing, selectedRun, step } = flow;
  if (!viewing) return null;
  const { replayed } = view;
  return (
    <>
      <BottomPane
        slug={viewing}
        selected={selectedRun}
        onSelect={view.onSelectRun}
        step={step}
        onStep={view.onStep}
        order={replayed?.order ?? []}
        nodes={nodes}
        timings={view.timings}
        exchanges={view.exchanges}
        verdict={view.verdict}
        versionNote={view.note}
        listenError={view.listenError}
        onPickNode={view.pickNode}
        diagnosis={replayed?.diagnosis ?? null}
        onShowNode={view.showNode}
        onFitToNodes={view.fitToNodes}
        onRestored={onRestored}
        codeText={view.codeText}
        keys={keys}
        onClose={onClose}
      />
    </>
  );
}
