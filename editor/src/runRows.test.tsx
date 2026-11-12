// A run on the statement graph (web-console-L4.md §4, §5.5), as View runs draws it: every sample,
// deployed as its text compiles and printed back as the text pane prints it, has each of its nodes
// on a row, so a run replayed on that canvas lands on rows without loss; the step's node marks its
// row, a row shows its last node's value, and a failure's origin marks its row. The Record tab's
// Claims say whether the run held each key it read until it was done, and for how long; the run
// strip says the run.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { Evaluator, initSync } from "../wasm/holon_wasm.js";
import { compileRows } from "./canvasLevel";
import type { WValue } from "./holon";
import { holonGraph, printHolon } from "./holonText";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { replay } from "./replay";
import { graphRun, rowOfNode, rowsOnCanvas, rowValue } from "./runRows";
import { claimNotes } from "./runs";
import { runStripFacts } from "./RunStrip";
import { buildSample, samples } from "./samples";
import type { LogEntry } from "./server";
import { rowKey, StatementGraph } from "./StatementGraph";
import type { Graph } from "./textSync";
import { Claims } from "./ViewPanes";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
});

function draw(el: React.ReactElement): HTMLDivElement {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(el));
  return container;
}

/** A sample as View runs shows it: deployed from its text (the canvas), printed back as the text
 *  pane prints it, and that text's graph with each row's nodes on the canvas. */
function viewed(name: string) {
  const ev = new Evaluator();
  const doc = buildSample(samples().find((s) => s.name === name)!, ev);
  const canvas = { nodes: doc.nodes, edges: doc.edges } as Graph;
  const text = printHolon(name, doc, ev);
  const graph = holonGraph(text);
  const compiled = compileRows(text, ev);
  expect(compiled, name).not.toBeNull();
  return { canvas, graph, rows: rowsOnCanvas(graph, compiled!, canvas) };
}

/** A run of the whole flow, replayed on the canvas as view mode replays one: every node it can
 *  reach, each giving nothing. */
const replayAll = (canvas: Graph) =>
  replay(canvas.nodes, canvas.edges, { kind: "request", route: null, request: null, sourced: [], state_before: {} }, (n) => n.data.outputs.map(() => null));

const entry = (over: Partial<LogEntry>): LogEntry => ({
  seq: 7,
  session: 1,
  at_ms: Date.parse("2026-11-12T10:00:00Z"),
  duration_ms: 3,
  kind: "request",
  route: "GET /hello/:name",
  method: "GET",
  url: "/hello/Ada",
  status: 200,
  request: null,
  sourced: [],
  state_before: {},
  outputs: [],
  performed: [],
  response_body: "Hello, Ada",
  error: null,
  ...over,
});

describe("a run's trace on the rows", () => {
  for (const s of samples()) {
    it(`${s.name}: every node it runs is on a row`, () => {
      const { canvas, rows } = viewed(s.name);
      const on = rowOfNode(rows);
      const lost = canvas.nodes.filter((n) => !on.has(n.id)).map((n) => `${n.id} ${(n as PrimitiveNodeType).data.name}`);
      expect(lost, s.name).toEqual([]);
      const run = replayAll(canvas);
      const drawn = graphRun(rows, { order: run.order, traces: run.traces, step: run.order.length > 0 ? 0 : null, values: true });
      for (const id of run.order) expect(drawn.ran.has(on.get(id)!), id).toBe(true);
      if (run.order.length > 0) expect(drawn.current).toBe(on.get(run.order[0]));
    });
  }
});

describe("a run drawn on the graph", () => {
  it("marks the step's row, shows each row's last value, and the row a failure began at", () => {
    const { graph, rows, canvas } = viewed("hello-api");
    const answer = graph.bands[0].rows.findIndex((r) => r.kind === "answer");
    const key = rowKey(0, answer);
    const ids = rows.get(key)!;
    expect(ids.length).toBeGreaterThan(0);
    const respond = ids.find((id) => canvas.nodes.find((n) => n.id === id)!.data.name === "respond")!;
    const order = [...rows.values()].flat();
    const said: WValue = { type: "Record", value: [["status", { type: "Number", value: 200 }], ["body", { type: "Text", value: "Hello, Ada" }]] };
    const traces = new Map(order.map((id) => [id, { outputs: id === respond ? [said] : [] }]));
    const issues = new Map([[respond, { role: "origin" as const, reason: "it answered nothing" }]]);
    const run = graphRun(rows, { order, traces, step: order.indexOf(respond), issues, values: true });
    expect(run.current).toBe(key);
    expect(run.values!.get(key)).toBe('200 "Hello, Ada"');
    expect(run.failed!.get(key)).toBe("it answered nothing");
    const c = draw(<StatementGraph graph={graph} run={run} />);
    const row = c.querySelectorAll<HTMLButtonElement>(".hl-sg__row")[answer];
    expect(row.dataset.run).toBe("failed");
    expect(row.textContent).toContain("failed here");
    expect(row.textContent).toContain('= 200 "Hello, Ada"');
    // Values off (v): none shown.
    expect(graphRun(rows, { order, traces, step: null, values: false }).values!.size).toBe(0);
  });

  it("says an answer by its status and body, anything else as its chip", () => {
    expect(rowValue({ type: "Number", value: 5 })).toBe("5");
    expect(rowValue(null)).toBe("—");
    expect(rowValue({ type: "Record", value: [["status", { type: "Number", value: 404 }], ["body", { type: "Text", value: "" }]] })).toBe('404 ""');
  });
});

describe("Claims", () => {
  it("says a key held until it was done, and for how long", () => {
    const run = entry({ claims: [{ key: "charges[k2]", waited_ms: 0.2, overtaken: false, held_ms: 935.4 }] });
    expect(claimNotes(run)).toEqual([{ said: "claimed charges[k2] before calling out: waited 0 ms, held it 935 ms until it was done", tone: "ok" }]);
    const c = draw(<Claims run={run} />);
    expect(c.querySelector("[aria-label=claims]")!.textContent).toContain("held it 935 ms until it was done");
  });

  it("says a key it waited for and found changed, and one it was turned away over", () => {
    const again = entry({
      retries: 1,
      claims: [
        { key: "charges[k2]", waited_ms: 681.4, overtaken: true, held_ms: 0.1 },
        { key: "charges[k2]", waited_ms: 0.2, overtaken: false, held_ms: 2 },
      ],
    });
    expect(claimNotes(again).map((n) => n.said)).toEqual([
      "claimed charges[k2] before calling out: waited 681 ms for another run, then found what it read had changed and ran again from there (retry 1 of 1), having sent nothing",
      "claimed charges[k2] before calling out: waited 0 ms, held it 2 ms until it was done",
    ]);
    const busy = entry({ status: 503, error: 'busy: "charges[k2]" is held by another run', claims: [{ key: "charges[k2]", waited_ms: 50, overtaken: false }] });
    expect(claimNotes(busy)).toEqual([{ said: "waited 50 ms for charges[k2], held by another run, and was turned away as busy", tone: "busy" }]);
  });

  it("says none when the run held no key", () => {
    const c = draw(<Claims run={entry({})} />);
    expect(c.textContent).toBe("none: it held no key it read while calling out");
  });
});

describe("the run strip", () => {
  it("says the run: how it ended, what came in, its version and what it did outside", () => {
    const versions = [
      { number: 2, unit: "aa".repeat(32), deployed_at: 0, current: false, restorable: true, canvas: true, state_kept: false },
      { number: 3, unit: "bb".repeat(32), deployed_at: 0, current: true, restorable: true, canvas: true, state_kept: false },
    ];
    const run = entry({
      unit: "aa".repeat(32),
      rounds: [{ exchanges: [{ path: [3], request: { type: "Record", value: [["method", { type: "Text", value: "POST" }], ["url", { type: "Text", value: "https://api.example/x" }]] }, response: { type: "Record", value: [] }, start_us: 0, end_us: 1 }] }],
      performed: [{ capability: "StateWrite", payload: { type: "Number", value: 1 } }],
    });
    const f = runStripFacts(run, versions, run.at_ms);
    expect([f.ended, f.came, f.version, f.current, f.sent, f.did]).toEqual(["200", "GET /hello/Ada", "v2", false, 1, 1]);
    expect(f.calls).toEqual(["POST https://api.example/x"]);
    const failed = runStripFacts(entry({ status: 500, error: "boom", unit: "cc".repeat(32) }), versions);
    expect([failed.outcome, failed.ended, failed.version]).toEqual(["failed", "500 failed", "cccccccc…"]);
  });
});
