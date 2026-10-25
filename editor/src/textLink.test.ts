// The text and the graph, linked by each node's range (textLink.ts; docs/L4s/text-graph-link-L4.md),
// against the real core: the cursor picks the node under it, past an emoji too; nodes one
// expression lowers to are picked together; and a run of `calc` dims exactly what it didn't run.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { Edge } from "@xyflow/react";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import { buildPreimage, type WUnit, type WValue } from "./holon";
import { unitFor } from "./canvasUnits";
import { compileHolon, type TextRange } from "./holonText";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { replay, type ReplayNode } from "./replay";
import { dimmed, pickAt } from "./textLink";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

const SIGN = 'flow f\nport 8199\n\nroute POST /x (req) {\n  respond { said: "😀 hi", body_base64: base64(uppercase(req.body)) }\n}\n';

/** What each picked node is, by its primitive's name. */
const named = (doc: { nodes: unknown[] }, ids: string[]) => ids.map((id) => (doc.nodes as PrimitiveNodeType[]).find((n) => n.id === id)?.data.name);
const written = (text: string, rs: TextRange[]) => rs.map((r) => text.slice(r.from, r.to));

describe("the text and the graph, linked", () => {
  it("picks the innermost node under the cursor, in UTF-16 offsets past an emoji", () => {
    const { doc, spans } = compileHolon(SIGN, new Evaluator());
    const at = (s: string, k = 0) => SIGN.indexOf(s) + k;
    expect(named(doc, pickAt(spans, at("base64(")))).toEqual(["base64"]);
    expect(named(doc, pickAt(spans, at("uppercase(", 3)))).toEqual(["uppercase"]);
    // Inside the call's argument: still the call written over it (req.body is a port, no node).
    expect(named(doc, pickAt(spans, at("req.body", 2)))).toEqual(["uppercase"]);
    expect(named(doc, pickAt(spans, at("😀")))).toEqual(["const"]);
    expect(pickAt(spans, at("port"))).toEqual([]);
  });

  it("picks the nodes one expression lowers to together, and a choice by what is written of it alone", () => {
    const text = readFileSync(join(process.cwd(), "..", "crates", "holon-author", "samples", "calc.holon"), "utf8");
    const { doc, spans } = compileHolon(text, new Evaluator());
    expect(named(doc, pickAt(spans, text.indexOf('"add"') + 1))).toEqual(["const", "equals"]);
    expect(named(doc, pickAt(spans, text.indexOf("->")))).toEqual(["guard"]);
    expect(named(doc, pickAt(spans, text.indexOf("a + b") + 2))).toEqual(["add"]);
  });

  it("dims what a run of calc didn't run: the values of the arms not chosen, not their patterns", () => {
    const text = readFileSync(join(process.cwd(), "..", "crates", "holon-author", "samples", "calc.holon"), "utf8");
    const ev = new Evaluator();
    const { doc, spans } = compileHolon(text, ev);
    const evalNode = (node: ReplayNode, inputs: (WValue | null)[]) => {
      const hash = unitFor(node as PrimitiveNodeType, ev);
      return ev.eval(buildPreimage(ev.get_unit(hash) as WUnit), inputs) as (WValue | null)[];
    };
    const json = '{"op": "div", "a": 1, "b": 0}';
    const request: WValue = {
      type: "Record",
      value: [
        ["method", { type: "Text", value: "POST" }],
        ["path", { type: "Text", value: "/calc" }],
        ["params", { type: "Record", value: [] }],
        ["query", { type: "Record", value: [] }],
        ["headers", { type: "Record", value: [] }],
        ["body", { type: "Text", value: json }],
      ],
    };
    const run = replay(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], { kind: "request", route: "POST /calc", request, sourced: [], state_before: {} }, evalNode);
    const ran = new Set(run.order);
    expect(named(doc, run.order.filter((id) => (doc.nodes as PrimitiveNodeType[]).find((n) => n.id === id)?.data.name === "divide"))).toEqual(["divide"]);
    expect(written(text, dimmed(spans, (id) => ran.has(id)))).toEqual(["a + b", "a - b", "a * b"]);
    // The divide ran and made no value: the route answers 400.
    const divide = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "divide")!;
    expect(run.traces.get(divide.id)?.outputs).toEqual([null]);
  });

  it("keeps a range lit when any node written over it ran, and never dims what no node is written over", () => {
    const spans = new Map<string, TextRange>([
      ["outer", { from: 0, to: 10 }],
      ["inner", { from: 2, to: 5 }],
      ["twin-a", { from: 6, to: 8 }],
      ["twin-b", { from: 6, to: 8 }],
      ["apart", { from: 12, to: 14 }],
    ]);
    expect(dimmed(spans, (id) => id === "outer" || id === "twin-b")).toEqual([
      { from: 2, to: 5 },
      { from: 12, to: 14 },
    ]);
    expect(dimmed(spans, (id) => id !== "outer")).toEqual([
      { from: 0, to: 2 },
      { from: 5, to: 6 },
      { from: 8, to: 10 },
    ]);
  });
});
