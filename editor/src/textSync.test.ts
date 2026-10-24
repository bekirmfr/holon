// The text pane's matching (textSync.ts) against the real core: every sample's canvas is the same
// graph as its text compiled back, and an edit keeps what it didn't touch where it was.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { compileHolon, printHolon } from "./holonText";
import { buildSample, samples } from "./samples";
import { carryOver, commentCount, sameGraph, syncedFromCanvas, textKeys, type Graph } from "./textSync";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

const compiled = (text: string, ev: Evaluator) => {
  const { doc, names } = compileHolon(text, ev);
  return { graph: { nodes: doc.nodes, edges: doc.edges } as Graph, names };
};

const TEXT = `flow notes
port 8200
collection notes

route PUT /notes/:id (req) {
  body = lowercase(req.body)
  notes[req.params.id] = body
  respond 204
}

route GET /notes/:id (req) {
  respond notes[req.params.id], missing: 404
}
`;

/** The canvas `text` stands for, spread out so places are told apart, and its sync. */
function start(text: string, ev: Evaluator) {
  const c = compiled(text, ev);
  const canvas = c.graph.nodes.map((n, i) => ({ ...n, id: `n${i}`, position: { x: 1000 + i * 7, y: 500 + i * 13 } })) as PrimitiveNodeType[];
  const ids = new Map(c.graph.nodes.map((n, i) => [n.id, `n${i}`]));
  return { canvas, synced: { ...c, toCanvas: ids } };
}

let fresh = 0;
const freshId = (base: string) => `${base}-new${fresh++}`;

describe("the text and the canvas, one flow", () => {
  it.each(samples().map((s) => [s.name]))("matches every node of the %s sample's canvas to its text compiled back", (name) => {
    const ev = new Evaluator();
    const sample = buildSample(samples().find((s) => s.name === name)!, ev);
    const canvas = { nodes: sample.nodes, edges: sample.edges } as Graph;
    const back = compiled(printHolon(name, sample, ev), ev);
    const synced = syncedFromCanvas(canvas, back.graph, back.names);
    expect(synced.toCanvas.size).toBe(canvas.nodes.length);
    expect(new Set(synced.toCanvas.values()).size).toBe(canvas.nodes.length);
    const byId = new Map(canvas.nodes.map((n) => [n.id, n]));
    for (const n of back.graph.nodes) expect(byId.get(synced.toCanvas.get(n.id)!)!.data.name).toBe(n.data.name);
  });

  it("keys each node by its name or where it is written, uniquely", () => {
    const ev = new Evaluator();
    const c = compiled(TEXT, ev);
    const keys = [...textKeys(c.graph, c.names).values()];
    expect(new Set(keys).size).toBe(keys.length);
    expect(keys).toContain("route PUT /notes/:id");
    expect(keys).toContain("=body");
  });

  it("keeps every node where it was through an edit that changes none of them", () => {
    const ev = new Evaluator();
    const { canvas, synced } = start(TEXT, ev);
    const next = compiled(TEXT.replace("collection notes", "collection notes # kept by id"), ev);
    const out = carryOver(synced, next, canvas, freshId);
    expect(out.nodes.map((n) => [n.id, n.position])).toEqual(canvas.map((n) => [n.id, n.position]));
    expect(out.edges.every((e) => canvas.some((n) => n.id === e.source) && canvas.some((n) => n.id === e.target))).toBe(true);
  });

  it("keeps the others where they were when a line changes, and places a new node beside what it feeds", () => {
    const ev = new Evaluator();
    const { canvas, synced } = start(TEXT, ev);
    const next = compiled(TEXT.replace("  respond 204", '  respond lowercase("saved"), status: 201'), ev);
    const out = carryOver(synced, next, canvas, freshId);
    const kept = out.nodes.filter((n) => canvas.some((c) => c.id === n.id && c.position === n.position));
    expect(kept.length).toBeGreaterThanOrEqual(canvas.length - 1);
    const added = out.nodes.filter((n) => !canvas.some((c) => c.id === n.id));
    const lowercase = added.find((n) => n.data.name === "lowercase")!;
    expect(lowercase).toBeDefined();
    const fed = out.nodes.find((n) => n.id === out.edges.find((e) => e.source === lowercase.id)!.target)!;
    expect(lowercase.position.x).toBeLessThan(fed.position.x);
    expect(Math.abs(lowercase.position.y - fed.position.y)).toBeLessThan(400);
  });

  it("removes the node of a deleted line, and a renamed binding is a new node", () => {
    const ev = new Evaluator();
    const { canvas, synced } = start(TEXT, ev);
    const gone = carryOver(synced, compiled(TEXT.replace("  body = lowercase(req.body)\n  notes[req.params.id] = body\n", "  notes[req.params.id] = req.body\n"), ev), canvas, freshId);
    expect(gone.nodes.length).toBe(canvas.length - 1);
    expect(gone.nodes.some((n) => n.data.name === "lowercase")).toBe(false);
    const renamed = carryOver(synced, compiled(TEXT.replace("body = lowercase", "doc = lowercase").replace("= body", "= doc"), ev), canvas, freshId);
    const bodyNode = canvas.find((n) => synced.names.get([...synced.toCanvas].find(([, c]) => c === n.id)![0]) === "body")!;
    expect(renamed.nodes.some((n) => n.id === bodyNode.id)).toBe(false);
  });

  it("counts the comments a reprint would drop, not a # in a string", () => {
    expect(commentCount('# one\nflow f\nx = "#no" # two\n')).toBe(2);
  });

  it("matches nodes alike only by their wiring", () => {
    const ev = new Evaluator();
    const c = compiled('flow f\nport 8200\n\nroute GET /a (req) {\n  respond concat(lowercase("A"), lowercase("B"))\n}\n', ev);
    const reversed = { nodes: [...c.graph.nodes].reverse(), edges: [...c.graph.edges].reverse() };
    const m = sameGraph(c.graph, reversed);
    for (const [a, b] of m) expect(a).toBe(b);
  });
});
