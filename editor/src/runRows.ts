// A run drawn on the statement graph (web-console-L4.md §4, §5.5). View runs replays the run on
// the canvas of the version it ran on, node by node; each row of the graph stands for some of
// those nodes (its parts, canvasLevel.ts), so what the run did lands on rows: a row ran if any of
// its nodes did, it shows the value its last node made, the step's node marks its row, and the
// node a failure began at marks its row failed.

import type { WValue } from "./holon";
import { valueChip } from "./diagnostics";
import { rowNodes, type RowsCompiled } from "./canvasLevel";
import type { FlowGraph } from "./holonText";
import type { Issue } from "./replay";
import { rowKey, type GraphRun } from "./StatementGraph";
import { sameGraph, type Graph } from "./textSync";

/** The canvas's nodes each row stands for, by rowKey: its nodes in the compiled text, matched to
 *  the canvas the text was printed from. */
export function rowsOnCanvas(graph: FlowGraph, compiled: RowsCompiled, canvas: Graph): Map<string, string[]> {
  const to = sameGraph({ nodes: compiled.nodes, edges: compiled.edges } as Graph, canvas);
  const out = new Map<string, string[]>();
  graph.bands.forEach((band, b) =>
    band.rows.forEach((row, i) => {
      out.set(
        rowKey(b, i),
        rowNodes(row, compiled).flatMap((id) => to.get(id) ?? []),
      );
    }),
  );
  return out;
}

/** The row each canvas node is on, by node id: the first that stands for it. */
export function rowOfNode(rows: Map<string, string[]>): Map<string, string> {
  const out = new Map<string, string>();
  for (const [key, ids] of rows) for (const id of ids) if (!out.has(id)) out.set(id, key);
  return out;
}

/** The run, as view mode replayed it on the canvas. */
export type RunOnCanvas = {
  /** The nodes it ran, in order. */
  order: string[];
  traces: Map<string, { outputs: (WValue | null)[] }>;
  /** The step shown (an index into `order`), or none: the whole run. */
  step: number | null;
  /** Where a failed run went wrong, by node (replay.ts's diagnose). */
  issues?: Map<string, Issue>;
  /** Whether rows show values (the `v` pane toggle). */
  values: boolean;
};

/** A value as a row shows it: an answer by its status and body, anything else as its chip. */
export function rowValue(v: WValue | null): string {
  if (v?.type === "Record") {
    const status = v.value.find(([k]) => k === "status")?.[1];
    const body = v.value.find(([k]) => k === "body")?.[1];
    if (status?.type === "Number" && body !== undefined) return `${status.value} ${valueChip(body)}`;
  }
  return valueChip(v);
}

/** The run over the graph: the rows it ran, the step's row, each row's value (the first output of
 *  its last node that made one) and the rows it failed at, with why. */
export function graphRun(rows: Map<string, string[]>, run: RunOnCanvas): GraphRun {
  const at = new Map(run.order.map((id, i) => [id, i]));
  const ran = new Set<string>();
  const values = new Map<string, string>();
  const failed = new Map<string, string>();
  for (const [key, ids] of rows) {
    const done = ids.filter((id) => at.has(id)).sort((a, b) => at.get(a)! - at.get(b)!);
    if (done.length > 0) ran.add(key);
    const origin = ids.find((id) => run.issues?.get(id)?.role === "origin");
    if (origin) failed.set(key, run.issues!.get(origin)!.reason);
    if (!run.values) continue;
    const last = [...done].reverse().find((id) => (run.traces.get(id)?.outputs.length ?? 0) > 0);
    const outputs = last ? run.traces.get(last)!.outputs : [];
    // Of several (a request's parts), the first: the whole.
    if (outputs.length > 0) values.set(key, rowValue(outputs[0] ?? null));
  }
  const node = run.step === null ? undefined : run.order[run.step];
  const current = node === undefined ? null : (rowOfNode(rows).get(node) ?? null);
  return { ran, current, values, failed };
}
