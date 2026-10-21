import { describe, it, expect } from "vitest";
import { replay, runGroup, type ReplayInput, type ReplayNode } from "./replay";
import type { WParam, WValue } from "./holon";

const frozen = (name: string, value: string): WParam => ({ kind: "Frozen", name, value });
const num = (value: number): WValue => ({ type: "Number", value });
const list = (...value: WValue[]): WValue => ({ type: "List", value });

function n(id: string, name: string, ins: string[], outs: string[], params: WParam[] = [], kind = "Primitive"): ReplayNode {
  return {
    id,
    data: {
      name,
      body: { kind },
      params,
      inputs: ins.map((p) => ({ name: p, ty: "Any" })),
      outputs: outs.map((p) => ({ name: p, ty: "Any" })),
    },
  };
}
const wire = (source: string, sourceHandle: string, target: string, targetHandle: string) => ({
  source,
  sourceHandle,
  target,
  targetHandle,
});

// A fake evaluator: `append` appends, `count` counts list items, a request template echoes its
// input's "body", anything else passes its first input through.
function evalNode(node: ReplayNode, inputs: (WValue | null)[]): (WValue | null)[] {
  if (node.data.name === "append") {
    const [l, item] = inputs;
    return l?.type === "List" && item ? [list(...l.value, item)] : [null];
  }
  if (node.data.name === "count") return [inputs[0]?.type === "List" ? num(inputs[0].value.length) : null];
  if (node.data.name === "request") {
    const r = inputs[0];
    const body = r?.type === "Record" ? (r.value.find(([k]) => k === "body")?.[1] ?? null) : null;
    return [r, null, null, null, body, body];
  }
  return [inputs[0] ?? null];
}

const req = (method: string, path: string, id: string) =>
  n(id, "request", ["request"], ["request", "params", "query", "headers", "body", "json"], [frozen("method", method), frozen("path", path)], "Template");

describe("replay", () => {
  // POST /todos: request.json → append.item; store_read(todos) → append.list; append → store_write
  const nodes = [
    req("POST", "/todos", "r"),
    n("sr", "store_read", [], ["out"], [frozen("key", "todos")]),
    n("ap", "append", ["list", "item"], ["out"]),
    n("sw", "store_write", ["value"], ["out"], [frozen("key", "todos")]),
    // another route's group, not part of a POST /todos run
    req("GET", "/count", "r2"),
    n("c", "count", ["list"], ["out"]),
  ];
  const edges = [
    wire("ap", "out", "sw", "value"),
    wire("sr", "out", "ap", "list"),
    wire("r", "json", "ap", "item"),
    wire("r2", "request", "c", "list"),
  ];
  const entry: ReplayInput = {
    kind: "request",
    route: "POST /todos",
    request: { type: "Record", value: [["body", num(7)]] },
    sourced: [["Clock", num(1000)]],
    state_before: { todos: list(num(1), num(2)) },
  };

  it("replays only the route's group, in dependency order, from the recorded inputs", () => {
    const { order, traces } = replay(nodes, edges, entry, evalNode);
    expect(order).toEqual(["r", "sr", "ap", "sw"]);
    expect(traces.get("sr")?.outputs).toEqual([list(num(1), num(2))]);
    expect(traces.get("ap")?.inputs).toEqual([list(num(1), num(2)), num(7)]);
    expect(traces.get("sw")?.outputs).toEqual([list(num(1), num(2), num(7))]);
    expect(traces.has("c")).toBe(false);
  });

  it("sources clock and a missing state key, and finds no group for an unknown route", () => {
    const tick: ReplayInput = { kind: "tick", route: null, request: null, sourced: [["Clock", num(42)]], state_before: {} };
    const { traces } = replay(
      [n("k", "clock", [], ["out"]), n("s", "store_read", [], ["out"], [frozen("key", "nope")])],
      [],
      tick,
      evalNode,
    );
    expect(traces.get("k")?.outputs).toEqual([num(42)]);
    expect(traces.get("s")?.outputs).toEqual([null]);
    expect(runGroup(nodes, edges, { ...entry, route: "DELETE /x" })).toEqual([]);
  });
});
