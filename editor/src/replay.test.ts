import { describe, it, expect } from "vitest";
import { collectionsAtStep, diagnose, executionOrder, isCollectionCell, itemChanged, nodeExchanges, nodeOutputs, nodeTimings, recordedReads, recordedResponses, replay, runGroup, stateAtStep, wireKey, type ReplayInput, type ReplayNode } from "./replay";
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

describe("secrets in a replay", () => {
  it("stand in as a placeholder: the editor never has the value", () => {
    const nodes = [n("s", "secret", [], ["out"], [frozen("name", "api_key")])];
    const { traces } = replay(nodes, [], { kind: "tick", route: null, request: null, sourced: [], state_before: {} }, evalNode);
    expect(traces.get("s")?.outputs).toEqual([{ type: "Text", value: "‹secret:api_key›" }]);
  });

  it("feed a node the run's own recorded outputs, so what follows replays true", () => {
    // secret → verify (computes with it) → count: a signature check, then what depends on it.
    // Recomputed from the placeholder, verify would be wrong; the log says what it produced.
    const nodes = [n("s", "secret", [], ["out"], [frozen("name", "key")]), n("v", "verify", ["key"], ["ok"]), n("c", "count", ["l"], ["out"])];
    const edges = [wire("s", "out", "v", "key"), wire("v", "ok", "c", "l")];
    const entry = { kind: "tick", route: null, request: null, sourced: [], state_before: {} };
    const ran = list(num(1), num(2));
    const { traces } = replay(nodes, edges, entry, evalNode, new Map(), new Map([["v", [ran]]]));
    expect(traces.get("v")).toMatchObject({ outputs: [ran], recorded: true });
    expect(traces.get("c")?.outputs).toEqual([num(2)]);
    expect(traces.get("c")?.recorded).toBeUndefined();

    // Without a record (an older log): recomputed, and flagged as such.
    const guessed = replay(nodes, edges, entry, evalNode).traces;
    expect(guessed.get("v")).toMatchObject({ placeholder: true });
  });

  it("count the writes a holon puts out as the run's own", () => {
    const effect = (sink: string, payload: WValue): WValue => ({ type: "Effect", value: { sink, payload } }) as WValue;
    const nodes = [n("h", "remember", [], ["saved", "put"], [], "Composite")];
    const ran = [effect("store_write:last", { type: "Text", value: "hi" }), effect("collection_put:seen", { type: "Record", value: [["key", { type: "Text", value: "hi" }], ["value", num(3)]] })];
    const run = replay(nodes, [], { kind: "tick", route: null, request: null, sourced: [], state_before: {} }, evalNode, new Map(), new Map([["h", ran]]), [], () => true);
    expect(stateAtStep({}, run, nodes, null)).toEqual({ after: { last: { type: "Text", value: "hi" } }, changed: ["last"] });
    const [seen] = collectionsAtStep({ state_before: {} }, run, nodes, null);
    expect(seen.items).toEqual([{ key: "hi", after: num(3), read: false }]);
  });

  it("replay a holon that reads from outside from what the run recorded", () => {
    // A placed composite holding a clock and a secret: recomputed offline it would produce nothing.
    const nodes = [n("h", "remember", ["text"], ["sig"], [], "Composite"), n("c", "count", ["l"], ["out"])];
    const edges = [wire("h", "sig", "c", "l")];
    const entry = { kind: "tick", route: null, request: null, sourced: [], state_before: {} };
    const ran = list(num(7));
    const outside = (node: ReplayNode) => node.id === "h";
    const { traces } = replay(nodes, edges, entry, evalNode, new Map(), new Map([["h", [ran]]]), [], outside);
    expect(traces.get("h")).toMatchObject({ outputs: [ran], recorded: true });
    expect(traces.get("c")?.outputs).toEqual([num(1)]);
    // A holon that only computes is recomputed as before.
    const pure = replay(nodes, edges, entry, evalNode, new Map(), new Map([["h", [ran]]])).traces;
    expect(pure.get("h")?.recorded).toBeUndefined();
  });

  it("find the run's recorded outputs by canvas node, from its final round", () => {
    const trace = { "*": ["a", "b"] };
    const timeline = [
      { path: [0], start_us: 0, end_us: 1, round: 0, outputs: [num(1)] },
      { path: [0], start_us: 5, end_us: 6, round: 1, outputs: [num(2)] },
      { path: [1], start_us: 6, end_us: 7, round: 1 },
    ];
    const got = nodeOutputs({ kind: "tick", route: null, timeline }, trace);
    expect(got.get("a")).toEqual([num(2)]);
    expect(got.has("b")).toBe(false);
  });
});

describe("collection reads in a replay", () => {
  it("are answered from the run's record, by node, as it found the collection", () => {
    const nodes = [n("g", "collection_get", ["key"], ["out"], [frozen("collection", "notes")]), n("l", "collection_list", ["prefix", "after"], ["out"], [frozen("collection", "notes")])];
    const note: WValue = { type: "Record", value: [["text", { type: "Text", value: "buy milk" }]] };
    const entry = {
      kind: "tick",
      route: null,
      reads: [
        { path: [0], request: num(0), answer: note },
        { path: [1], request: num(0), answer: null },
      ],
    };
    const reads = recordedReads(entry, { "*": ["g", "l"] });
    expect(reads.get("g")).toEqual(note);
    expect(reads.has("l")).toBe(false);
    const { traces } = replay(nodes, [], { kind: "tick", route: null, request: null, sourced: [], state_before: {} }, evalNode, reads);
    expect(traces.get("g")?.outputs).toEqual([note]);
    expect(traces.get("l")?.outputs).toEqual([null]);
  });
});

describe("collections as a run saw and changed them", () => {
  const txt = (value: string): WValue => ({ type: "Text", value });
  const rec = (...fields: [string, WValue][]): WValue => ({ type: "Record", value: fields });
  const root = "8ec0".padEnd(64, "0");
  const entry = {
    state_before: { "#collection:likes": txt(root), count: num(1) },
    reads: [
      { request: rec(["collection", txt("likes")], ["get", txt("1")]), answer: num(49) },
      { request: rec(["collection", txt("likes")], ["list", rec(["limit", num(100)], ["prefix", txt("")])]), answer: rec(["1", num(49)], ["2", num(2)]) },
    ],
  };
  const put = (key: string, value: WValue): WValue => ({ type: "Effect", value: { sink: "collection_put:likes", payload: rec(["key", txt(key)], ["value", value]) } });
  const nodes = [n("p", "collection_put", ["key", "value"], ["out"], [frozen("collection", "likes")]), n("d", "collection_delete", ["key"], ["out"], [frozen("collection", "likes")])];
  const run = {
    order: ["p", "d"],
    traces: new Map([
      ["p", { inputs: [], outputs: [put("1", num(50))] }],
      ["d", { inputs: [], outputs: [{ type: "Effect", value: { sink: "collection_delete:likes", payload: txt("7") } } as WValue] }],
    ]),
  };

  it("shows the items it read, what it wrote over them, and its listings, by collection", () => {
    const [likes] = collectionsAtStep(entry, run, nodes, null);
    expect(likes.name).toBe("likes");
    expect(likes.root).toBe(root);
    expect(likes.items).toEqual([
      { key: "1", before: num(49), after: num(50), read: true },
      { key: "7", after: null, read: false },
    ]);
    expect(likes.listed).toEqual([{ prefix: "", after: null, count: 2 }]);
    expect(likes.items.filter(itemChanged).map((i) => i.key)).toEqual(["1", "7"]);
    expect(isCollectionCell("#collection:likes") && !isCollectionCell("count")).toBe(true);
  });

  it("follows the step-through: before the put, nothing is written yet", () => {
    const [likes] = collectionsAtStep(entry, { ...run, order: ["p", "d"] }, nodes, -1);
    expect(likes.items).toEqual([{ key: "1", before: num(49), read: true }]);
    expect(likes.items.some(itemChanged)).toBe(false);
  });
});

describe("execution order", () => {
  it("steps through the nodes as the run evaluated them, where that's known", () => {
    const nodes = [n("a", "x", [], ["out"]), n("b", "x", [], ["out"]), n("c", "x", ["in"], ["out"])];
    const edges = [wire("a", "out", "c", "in")];
    const tick = { kind: "tick", route: null, request: null, sourced: [], state_before: {} };
    expect(replay(nodes, edges, tick, evalNode).order).toEqual(["a", "b", "c"]);
    const timings = new Map([["b", { start_us: 1, end_us: 2 }], ["a", { start_us: 3, end_us: 4 }], ["c", { start_us: 5, end_us: 6 }]]);
    expect(executionOrder(timings)).toEqual(["b", "a", "c"]);
    expect(replay(nodes, edges, tick, evalNode, new Map(), new Map(), executionOrder(timings)).order).toEqual(["b", "a", "c"]);
    // An order that breaks a dependency can't be followed past it: c still waits for a.
    expect(replay(nodes, edges, tick, evalNode, new Map(), new Map(), ["c", "b", "a"]).order).toEqual(["b", "a", "c"]);
  });
});

describe("why a run failed", () => {
  const txt = (value: string): WValue => ({ type: "Text", value });
  const rec = (...fields: [string, WValue][]): WValue => ({ type: "Record", value: fields });
  // A store read → field "kind" → respond, which answers 400 when its body never arrives.
  const nodes = [
    n("read", "store_read", [], ["out"], [frozen("key", "event")]),
    n("kind", "field", ["record"], ["out"], [frozen("name", "kind")]),
    n("res", "respond", ["body"], ["response"], [frozen("status", "200"), frozen("missing", "400")], "Template"),
  ];
  const edges = [wire("read", "out", "kind", "record"), wire("kind", "out", "res", "body")];
  const evaluate = (node: ReplayNode, inputs: (WValue | null)[]): (WValue | null)[] => {
    if (node.data.name === "field") {
      const [r] = inputs;
      const name = node.data.params.find((p) => p.name === "name");
      const want = name?.kind === "Frozen" ? name.value : "";
      return [r?.type === "Record" ? (r.value.find(([k]) => k === want)?.[1] ?? null) : null];
    }
    if (node.data.name === "respond") return [inputs[0] ?? rec(["status", num(400)])];
    return evalNode(node, inputs);
  };
  const run = (event: WValue) =>
    replay(nodes, edges, { kind: "tick", route: null, request: null, sourced: [], state_before: { event } }, evaluate);

  it("finds the node that got its inputs and produced nothing, says why, and marks the way to the answer", () => {
    const d = diagnose(nodes, edges, { status: 400, error: null }, run(rec(["id", txt("evt_1")], ["type", txt("paid")])))!;
    expect(d.summary).toBe("answered 400");
    expect(d.origins).toEqual(["kind"]);
    expect(d.issues.get("kind")).toEqual({ role: "origin", reason: 'no field "kind" (it has id, type)' });
    expect(d.issues.get("res")).toEqual({ role: "starved", reason: "fell back: body (from field) never arrived" });
    expect(d.issues.has("read")).toBe(false);
    expect([...d.path]).toEqual([wireKey(edges[1])]);
  });

  it("says a starved node didn't run, even when the evaluator hands back undefined for absent", () => {
    // The wasm bridge returns an absent output as undefined; that must still read as absent.
    const chain = [...nodes.slice(0, 2), n("next", "field", ["record"], ["out"], [frozen("name", "x")])];
    const wires = [edges[0], wire("kind", "out", "next", "record")];
    const undefinedForAbsent = (node: ReplayNode, inputs: (WValue | null)[]) =>
      evaluate(node, inputs).map((o) => (o === null ? (undefined as unknown as null) : o));
    const r = replay(chain, wires, { kind: "tick", route: null, request: null, sourced: [], state_before: { event: rec(["id", txt("e")]) } }, undefinedForAbsent);
    expect(r.traces.get("next")?.outputs).toEqual([null]);
    expect(diagnose(chain, wires, { status: 400, error: null }, r)!.issues.get("next")).toEqual({
      role: "starved",
      reason: "didn't run: record (from field) never arrived",
    });
  });

  it("has nothing to say about a run that didn't fail", () => {
    expect(diagnose(nodes, edges, { status: 200, error: null }, run(rec(["kind", txt("paid")])))).toBeNull();
  });

  it("names a failed outbound call as the origin", () => {
    const call = [n("h", "http_request", [], ["out"], [frozen("method", "GET"), frozen("host", "api.example.com")]), n("f", "field", ["record"], ["out"], [frozen("name", "body")])];
    const blocked = rec(["status", num(0)], ["error", txt("blocked_host")], ["message", txt("api.example.com wasn't granted")]);
    const r = replay(call, [wire("h", "out", "f", "record")], { kind: "tick", route: null, request: null, sourced: [], state_before: {} }, evaluate, new Map([["h", blocked]]));
    const d = diagnose(call, [wire("h", "out", "f", "record")], { status: null, error: "a test" }, r)!;
    expect(d.summary).toBe("the run failed: a test");
    expect(d.issues.get("h")).toEqual({ role: "origin", reason: "the request failed: blocked_host (api.example.com wasn't granted)" });
  });
});

describe("durable runs in a replay", () => {
  it("answer a waiting point from the logged event, and show a token as the log does", () => {
    const nodes = [
      n("w", "await", [], ["out"], [frozen("name", "paid"), frozen("expires", "60")]),
      n("t", "resume_token", [], ["out"], [frozen("await", "paid")]),
    ];
    const base = { kind: "resume", route: null, request: null, sourced: [], state_before: {} };
    const resumed = replay(nodes, [], { ...base, events: { paid: num(1) } }, evalNode);
    expect(resumed.traces.get("w")?.outputs).toEqual([num(1)]);
    expect(resumed.traces.get("t")?.outputs).toEqual([{ type: "Text", value: "‹token:paid›" }]);
    // not delivered yet
    expect(replay(nodes, [], base, evalNode).traces.get("w")?.outputs).toEqual([null]);
  });
});
