// The canvas's nodes as view mode draws a run on them (useViewMode): read only, each with what it
// produced and where in the run it is (done, active, pending, skipped or outside the run), and its
// issue if the run failed there. A node whose part in the run is as it was keeps its object, so
// the canvas draws again only the nodes a new run changed: React Flow skips a node whose object is
// the same.
// Why: live view follows each new run, and every node made new each time drew all of a 2,000-node
// flow again, about 0.3 s a run, the browser frozen while runs kept coming (2026-11-20).

import type { WValue } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { Issue, NodeTrace } from "./replay";

/** What of a replayed run the nodes show. */
export type RunShown = { order: string[]; notRun: Map<string, unknown>; traces: Map<string, NodeTrace>; issues?: Map<string, Issue> };

/** A node as it was last drawn, and what it was drawn from. */
export type Drawn = { base: PrimitiveNodeType; state: string; issue: Issue | undefined; showValues: boolean; outputs: (WValue | null)[] | null; node: PrimitiveNodeType };

/** `nodes` with `run` on them, at `step`; `was` the last drawing's (`drawn` of the last call),
 *  whose node objects are given again where nothing they show changed. */
export function runNodes(nodes: PrimitiveNodeType[], run: RunShown | null, step: number | null, showValues: boolean, was: Map<string, Drawn>): { nodes: PrimitiveNodeType[]; drawn: Map<string, Drawn> } {
  const order = new Map((run?.order ?? []).map((id, i) => [id, i]));
  const drawn = new Map<string, Drawn>();
  const out = nodes.map((n) => {
    const i = order.get(n.id) ?? -1;
    const state = i < 0 ? (run?.notRun.has(n.id) ? "skipped" : "outside") : step === null || i < step ? "done" : i === step ? "active" : "pending";
    const issue = run?.issues?.get(n.id);
    const outputs = run ? (run.traces.get(n.id)?.outputs ?? []) : null;
    const before = was.get(n.id);
    if (before && before.base === n && before.state === state && before.showValues === showValues && same(before.issue, issue) && same(before.outputs, outputs)) {
      drawn.set(n.id, before);
      return before.node;
    }
    const trace = outputs === null ? undefined : { outputs, state, issue, showValues };
    const node: PrimitiveNodeType = { ...n, data: { ...n.data, readOnly: true, trace } } as PrimitiveNodeType;
    drawn.set(n.id, { base: n, state, issue, showValues, outputs, node });
    return node;
  });
  return { nodes: out, drawn };
}

/** The same object, or the same JSON: a run's values are made new by each replay. */
function same(a: unknown, b: unknown): boolean {
  return a === b || JSON.stringify(a) === JSON.stringify(b);
}
