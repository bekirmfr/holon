// View mode draws a run on the canvas making new only the nodes whose part in it changed
// (runNodes.ts): a node outside the run, or whose values, place in the run and issue are as they
// were, keeps its object, which React Flow doesn't draw again.
// Why: live view follows each new run, and all of a 2,000-node flow drawn again each time froze
// the browser about 0.3 s a run while runs kept coming (2026-11-20).
// If it fails: a node is made new where nothing it shows changed, or kept where something did.

import { describe, expect, it } from "vitest";
import type { WValue } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { NodeTrace } from "./replay";
import { runNodes, type Drawn, type RunShown } from "./runNodes";

const node = (id: string) => ({ id, type: "primitive", position: { x: 0, y: 0 }, data: { outputs: [] } }) as unknown as PrimitiveNodeType;
const text = (value: string): WValue => ({ type: "Text", value });
/** A run of `ran`, each node's outputs made new as a replay makes them. */
const run = (ran: Record<string, string>, failedAt?: string): RunShown => ({
  order: Object.keys(ran),
  notRun: new Map(),
  traces: new Map(Object.entries(ran).map(([id, v]): [string, NodeTrace] => [id, { inputs: [], outputs: [text(v)] }])),
  issues: failedAt ? new Map([[failedAt, { role: "origin", reason: "it failed" }]]) : undefined,
});

describe("a run drawn on the canvas", () => {
  const nodes = ["a", "b", "c"].map(node);
  const draw = (r: RunShown | null, was: Map<string, Drawn>, step: number | null = null, values = true) => runNodes(nodes, r, step, values, was);
  const ids = (out: PrimitiveNodeType[], before: PrimitiveNodeType[]) => out.filter((n, i) => n !== before[i]).map((n) => n.id);

  it("makes new only the nodes the next run changed", () => {
    const first = draw(run({ a: "1", b: "x" }), new Map());
    expect(ids(first.nodes, nodes), "all drawn the first time").toEqual(["a", "b", "c"]);
    const next = draw(run({ a: "2", b: "x" }), first.drawn);
    expect(ids(next.nodes, first.nodes), "a's value changed; b's is the same, made new by the replay; c is outside both").toEqual(["a"]);
    expect(next.nodes[0].data.trace?.outputs).toEqual([text("2")]);
    const same = draw(run({ a: "2", b: "x" }), next.drawn);
    expect(ids(same.nodes, next.nodes), "a run like the last").toEqual([]);
  });

  it("makes new a node whose place in the run, issue, values shown or canvas changed", () => {
    const first = draw(run({ a: "1", b: "x" }), new Map());
    expect(ids(draw(run({ a: "1", b: "x" }), first.drawn, 1).nodes, first.nodes), "stepped to b: a done, b active").toEqual(["b"]);
    expect(ids(draw(run({ a: "1", b: "x" }, "b"), first.drawn).nodes, first.nodes), "b failed").toEqual(["b"]);
    expect(ids(draw(run({ a: "1", b: "x" }), first.drawn, null, false).nodes, first.nodes), "values hidden").toEqual(["a", "b", "c"]);
    expect(ids(draw(run({ a: "1" }), first.drawn).nodes, first.nodes), "b not in this run").toEqual(["b"]);
    expect(ids(draw(null, first.drawn).nodes, first.nodes), "no run shown").toEqual(["a", "b", "c"]);
    const moved = [node("a"), nodes[1], nodes[2]];
    expect(ids(runNodes(moved, run({ a: "1", b: "x" }), null, true, first.drawn).nodes, first.nodes), "a's node changed on the canvas").toEqual(["a"]);
  });
});
