import { describe, it, expect } from "vitest";
import { nodeExchanges, nodeTimings, recordedResponses, replay, runGroup, stateAtStep, type ReplayInput, type ReplayNode } from "./replay";
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

describe("stateAtStep", () => {
  const nodes = [
    n("w1", "store_write", ["value"], ["out"], [frozen("key", "a")]),
    n("x", "count", ["list"], ["out"]),
    n("w2", "store_write", ["value"], ["out"], [frozen("key", "b")]),
  ];
  const effect = (key: string, payload: WValue): WValue => ({ type: "Effect", value: { sink: `store_write:${key}`, payload } });
  const run = {
    order: ["w1", "x", "w2"],
    traces: new Map([
      ["w1", { inputs: [], outputs: [effect("a", num(2))] }],
      ["x", { inputs: [], outputs: [num(0)] }],
      ["w2", { inputs: [], outputs: [effect("b", num(9))] }],
    ]),
  };
  const before = { a: num(1), b: num(9) };

  it("applies the writes reached so far and names the keys that changed", () => {
    expect(stateAtStep(before, run, nodes, 0)).toEqual({ after: { a: num(2), b: num(9) }, changed: ["a"] });
    // b is rewritten with the value it already had: not a change
    expect(stateAtStep(before, run, nodes, null)).toEqual({ after: { a: num(2), b: num(9) }, changed: ["a"] });
    expect(stateAtStep({}, run, nodes, null).changed).toEqual(["a", "b"]);
  });
});

describe("nodeTimings", () => {
  const trace = { "POST /todos": ["r", "sw", "ap"], "*": ["k"] };
  it("maps a request's [0, k] steps and a whole flow's [k] steps to canvas nodes", () => {
    const req = {
      kind: "request",
      route: "POST /todos",
      timeline: [
        { path: [0], start_us: 0, end_us: 30 },
        { path: [0, 2], start_us: 10, end_us: 12 },
        { path: [0, 0], start_us: 1, end_us: 3 },
        { path: [0, 2, 1], start_us: 10, end_us: 11 },
      ],
    };
    expect([...nodeTimings(req, trace)]).toEqual([
      ["ap", { start_us: 10, end_us: 12 }],
      ["r", { start_us: 1, end_us: 3 }],
    ]);
    const tick = { kind: "tick", route: null, timeline: [{ path: [0], start_us: 0, end_us: 5 }] };
    expect([...nodeTimings(tick, trace)]).toEqual([["k", { start_us: 0, end_us: 5 }]]);
    expect(nodeTimings(req, undefined).size).toBe(0);
  });
});

describe("outbound requests in a recorded run", () => {
  const trace = { "GET /proxy": ["r", "h", "x"], "*": ["h2"] };
  const rec = (status: number): WValue => ({ type: "Record", value: [["status", num(status)]] });
  const exchange = (path: number[], status: number) => ({ path, request: rec(0), response: rec(status), start_us: 5, end_us: 9 });

  it("places each round's exchanges on their nodes, and replays with the last answer", () => {
    const entry = {
      kind: "request",
      route: "GET /proxy",
      rounds: [{ exchanges: [exchange([0, 1], 503)] }, { exchanges: [exchange([0, 1], 200), exchange([7], 1)] }],
    };
    const xs = nodeExchanges(entry, trace);
    expect(xs.map((x) => [x.id, x.round])).toEqual([["h", 0], ["h", 1]]);
    expect(recordedResponses(xs).get("h")).toEqual(rec(200));

    // replay answers the http_request node from the record; nothing else changes.
    const nodes = [n("h", "http_request", ["path", "headers", "body"], ["response"])];
    const { traces } = replay(nodes, [], { kind: "tick", route: null, request: null, sourced: [], state_before: {} }, evalNode, recordedResponses(xs));
    expect(traces.get("h")?.outputs).toEqual([rec(200)]);
    const unanswered = replay(nodes, [], { kind: "tick", route: null, request: null, sourced: [], state_before: {} }, evalNode);
    expect(unanswered.traces.get("h")?.outputs).toEqual([null]);
  });

  it("times a run by its final round", () => {
    const entry = {
      kind: "tick",
      route: null,
      timeline: [
        { path: [0], start_us: 0, end_us: 1, round: 0 },
        { path: [0], start_us: 50, end_us: 52, round: 1 },
      ],
    };
    expect(nodeTimings(entry, trace).get("h2")).toEqual({ start_us: 50, end_us: 52 });
  });
});
