// View mode: a deployed flow's canvas, read only, with its runs. Everything that reads a run
// lives here: the selected run replayed on the canvas (and whether that reproduced it), the
// version it ran on (its own canvas, kept by the deployment), stepping through it, when each node
// ran, the flow state and collections as of a step, how nodes and wires show the run, and the
// side and bottom panes. App owns the canvas and the tab's flow state (tabs.ts's FlowMeta); this
// reads them and asks for changes through the setters it is given.

import { useCallback, useEffect, useMemo, useState, type MutableRefObject } from "react";
import type { Edge } from "@xyflow/react";
import { buildPreimage, readsFromOutside, type Evaluator, type WUnit, type WValue } from "./holon";
import { FAIL, type PrimitiveNodeType } from "./PrimitiveNode";
import { unitFor } from "./canvasUnits";
import { restoreDocument } from "./docUnits";
import { valueChip } from "./diagnostics";
import {
  collectionsAtStep,
  diagnose,
  executionOrder,
  innerSteps,
  nodeExchanges,
  nodeOutputs,
  nodeTimings,
  recordedReads,
  recordedResponses,
  replay,
  stateAtStep,
  wireKey,
  type ReplayNode,
} from "./replay";
import { getDeployedDocument, type DeploymentEntry, type FlowDocument, type LogEntry } from "./server";
import { BottomPane, SidePane, type InsideStep } from "./ViewPanes";
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
  fitView: (options: { nodes: { id: string }[]; duration: number; maxZoom: number; padding: number }) => unknown;
};

export function useViewMode(input: ViewModeInput) {
  const { flow, setSelectedRun, setStep, setInspectNode, setViewTrace, setViewUnit } = input;
  const { nodes, edges, live, canvasKey, setNodes, setEdges, restoring, viewDoc, ev, deployments, fitView } = input;
  const { viewing, viewUnit, viewTrace, selectedRun, step } = flow;

  // Values as chips beside each output (on), or the flow exactly as designed (off); and the wire
  // under the pointer, which shows the value it carried.
  const [showValues, setShowValues] = useState(true);
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
  const note = onOtherVersion
    ? versionNote || "loading the version it ran on…"
    : viewUnit && currentUnit && viewUnit !== currentUnit
      ? `shown on the version it ran on (${viewUnit.slice(0, 8)}…), an earlier deploy`
      : "";

  // One node of a replay: its unit, evaluated on the inputs its wires brought.
  const evalNode = useCallback(
    (node: ReplayNode, inputs: (WValue | null)[]): (WValue | null)[] => {
      try {
        const hash = unitFor(node as PrimitiveNodeType, ev);
        return ev.eval(buildPreimage(ev.get_unit(hash) as WUnit), inputs) as (WValue | null)[];
      } catch {
        return node.data.outputs.map(() => null);
      }
    },
    [ev],
  );

  // The selected run, replayed on the canvas; and whether that reproduced what the server sent.
  const replayed = useMemo(() => {
    if (!viewing || !selectedRun) return null;
    const { nodes: ns, edges: es } = live.current;
    // Outbound requests are answered from the run's record, never the network.
    const run = replay(
      ns,
      es,
      selectedRun,
      evalNode,
      new Map([...recordedResponses(nodeExchanges(selectedRun, viewTrace)), ...recordedReads(selectedRun, viewTrace)]),
      nodeOutputs(selectedRun, viewTrace),
      // Stepped through in the order the run evaluated its nodes.
      executionOrder(nodeTimings(selectedRun, viewTrace)),
      // A holon reading from outside within itself replays from what the run recorded.
      (node) => {
        const body = (node as PrimitiveNodeType).data.body;
        if (body.kind !== "Composite") return false;
        try {
          return readsFromOutside(ev.capability_names(body.hash) as string[]);
        } catch {
          return true;
        }
      },
    );
    const diagnosis = diagnose(ns, es, selectedRun, run);
    let verdict: boolean | null = null;
    const respond = run.order.find((id) => {
      const n = ns.find((x) => x.id === id);
      return n?.data.name === "respond" && n.data.body.kind === "Template";
    });
    if (selectedRun.kind === "request" && respond && selectedRun.outputs.length > 0) {
      verdict = JSON.stringify(run.traces.get(respond)?.outputs[0] ?? null) === JSON.stringify(selectedRun.outputs[0]);
    }
    return { ...run, verdict, diagnosis };
    // live.current is read on purpose: canvasKey changes exactly when the structure does.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewing, selectedRun, canvasKey, evalNode, viewTrace, ev]);

  // Show a node on the canvas and in the side pane: what "show where it failed" does.
  const showNode = useCallback(
    (id: string) => {
      setInspectNode(id);
      fitView({ nodes: [{ id }], duration: 300, maxZoom: 1.1, padding: 0.6 });
    },
    [fitView, setInspectNode],
  );

  // A failed run opens on where it failed.
  const origin = replayed?.diagnosis?.origins[0];
  useEffect(() => {
    if (origin) setInspectNode(origin);
  }, [origin, selectedRun?.seq, setInspectNode]);

  const onSelectRun = useCallback(
    (e: LogEntry | null) => {
      setSelectedRun(e);
      setStep(null);
    },
    [setSelectedRun, setStep],
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

  // The nodes as the run shows them: read only, each with what it produced and where in the run
  // it is (done, active, pending, or outside the run), and its issue if the run failed there.
  const viewNodes = useMemo(() => {
    const order = replayed?.order ?? [];
    return nodes.map((n) => {
      const i = order.indexOf(n.id);
      const state: "active" | "done" | "pending" | "outside" = i < 0 ? "outside" : step === null || i < step ? "done" : i === step ? "active" : "pending";
      const issue = replayed?.diagnosis?.issues.get(n.id);
      const trace = replayed ? { outputs: replayed.traces.get(n.id)?.outputs ?? [], state, issue, showValues } : undefined;
      return { ...n, data: { ...n.data, readOnly: true, trace } };
    });
  }, [nodes, replayed, step, showValues]);

  // The wires show the run: the way a failure took in its colour, a wire that carried nothing
  // faded, and the value a wire carried on hover.
  const viewEdges = useMemo(() => {
    if (!replayed) return edges;
    const ran = new Set(replayed.order);
    return edges.map((e) => {
      if (!ran.has(e.source) || !ran.has(e.target)) return e;
      const from = nodes.find((n) => n.id === e.source);
      const at = from ? from.data.outputs.findIndex((p) => p.name === e.sourceHandle) : -1;
      const value = replayed.traces.get(e.source)?.outputs[at] ?? null;
      const hover =
        hoveredWire === e.id
          ? {
              label: valueChip(value),
              labelStyle: { fill: value === null ? "#7e8ea8" : "#bfe8c2", fontSize: 11, fontFamily: "var(--mono)" },
              labelBgStyle: { fill: "#0b1527", stroke: "#6fd3e0" },
              labelBgPadding: [6, 3] as [number, number],
              labelBgBorderRadius: 6,
            }
          : {};
      if (replayed.diagnosis?.path.has(wireKey(e))) {
        return { ...e, ...hover, animated: true, style: { ...e.style, stroke: FAIL, strokeWidth: 2.6 } };
      }
      return value === null ? { ...e, ...hover, style: { ...e.style, opacity: hoveredWire === e.id ? 1 : 0.35, strokeDasharray: "4 4" } } : { ...e, ...hover };
    });
  }, [edges, nodes, replayed, hoveredWire]);

  return {
    replayed,
    /** Not comparable with the run while its version's canvas isn't shown. */
    verdict: onOtherVersion ? null : (replayed?.verdict ?? null),
    note,
    /** Why the deployment isn't answering on its port, when it isn't. */
    listenError: viewed?.listen_error,
    timings,
    exchanges,
    flowState,
    collections,
    inside,
    viewNodes,
    viewEdges,
    showValues,
    toggleValues: () => setShowValues((v) => !v),
    setHoveredWire,
    showNode,
    /** Picks a node for the side pane, without moving the canvas. */
    pickNode: setInspectNode,
    onSelectRun,
    onStep,
    onNodeClick,
  };
}

export type ViewMode = ReturnType<typeof useViewMode>;

/** View mode's panes: the flow state and component on the side, the log and timeline below. */
export function ViewModePanes({ view, flow, nodes }: { view: ViewMode; flow: FlowMeta; nodes: PrimitiveNodeType[] }) {
  const { viewing, selectedRun, step, inspectNode } = flow;
  if (!viewing) return null;
  const { replayed } = view;
  return (
    <>
      <SidePane
        slug={viewing}
        collections={view.collections}
        run={selectedRun}
        step={step}
        order={replayed?.order ?? []}
        stateAfter={view.flowState.after}
        changed={view.flowState.changed}
        node={nodes.find((n) => n.id === inspectNode) ?? null}
        trace={
          inspectNode && replayed?.traces.get(inspectNode)
            ? { ...replayed.traces.get(inspectNode)!, issue: replayed.diagnosis?.issues.get(inspectNode) }
            : undefined
        }
        timing={inspectNode ? view.timings.get(inspectNode) : undefined}
        inside={view.inside}
      />
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
      />
    </>
  );
}
