// A run replayed as view mode replays it: over the canvas, each node evaluated by core through
// the wasm, what the run asked outside answered from its record. View mode (useViewMode) and the
// sweep over every sample's recorded runs (recordedRuns.test.ts) both call it, so the sweep holds what the
// Record tab shows.

import { buildPreimage, passesExtraPrimitives, readsFromOutside, type Evaluator, type WUnit, type WValue } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { unitFor } from "./canvasUnits";
import {
  executionOrder,
  nodeExchanges,
  nodeOutputs,
  nodeTimings,
  recordedReads,
  recordedResponses,
  replay,
  replayVerdict,
  runGroup,
  sameNodesRan,
  type Replay,
  type ReplayEdge,
  type ReplayNode,
} from "./replay";
import type { LogEntry } from "./server";

/** What view mode replays a run with, and what it says of it: whether the replay ran the nodes
 *  the run's timeline ran (`ranSame`), and whether it reproduced the run (`verdict`, shown as
 *  "replay reproduces the run" or "replay differs from the run"). */
export function replayAsViewed(
  nodes: ReplayNode[],
  edges: ReplayEdge[],
  entry: LogEntry,
  trace: Record<string, string[]> | undefined,
  ev: Evaluator,
): Replay & { ranSame: boolean | null; verdict: boolean | null } {
  const evalNode = (node: ReplayNode, inputs: (WValue | null)[]): (WValue | null)[] => {
    try {
      const hash = unitFor(node as PrimitiveNodeType, ev);
      return ev.eval(buildPreimage(ev.get_unit(hash) as WUnit), inputs) as (WValue | null)[];
    } catch {
      return node.data.outputs.map(() => null);
    }
  };
  // Outbound requests are answered from the run's record, never the network.
  const run = replay(
    nodes,
    edges,
    entry,
    evalNode,
    new Map([...recordedResponses(nodeExchanges(entry, trace)), ...recordedReads(entry, trace)]),
    nodeOutputs(entry, trace),
    // Stepped through in the order the run evaluated its nodes.
    executionOrder(nodeTimings(entry, trace)),
    // A holon reading from outside within itself replays from what the run recorded.
    // So does a map, fold or other higher-order leaf whose function reads from outside.
    (node) => {
      const { body, name, params } = (node as PrimitiveNodeType).data;
      const fn = body.kind === "Primitive" && passesExtraPrimitives().has(name) ? params.find((p) => p.kind === "Frozen" && p.name === "fn") : undefined;
      const hash = body.kind === "Composite" ? body.hash : fn?.kind === "Frozen" ? fn.value : "";
      if (!hash) return false;
      try {
        return readsFromOutside(ev.capability_names(hash) as string[]);
      } catch {
        return true;
      }
    },
  );
  // It ran the nodes the run ran (a branch the run took, the replay took), where the timeline
  // can tell.
  const ranSame = sameNodesRan(runGroup(nodes, edges, entry), run, nodeTimings(entry, trace), (entry.parked?.length ?? 0) > 0);
  const verdict = replayVerdict(entry, run, nodes, nodeOutputs(entry, trace), ranSame);
  // Shown: a node held back (core's `held`) wasn't run, though the record notes it as a step and
  // the replay walked to it (held to the record above): out of what ran, and in `notRun`.
  const held = run.held ?? new Map();
  const order = run.order.filter((id) => !held.has(id));
  const traces = new Map([...run.traces].filter(([id]) => !held.has(id)));
  return { ...run, order, traces, ranSame, verdict };
}
