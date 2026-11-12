// Flows as text (holonText.ts) against the real core: written by hand, every sample printed and
// compiled back to the identical unit, and mistakes named by their line.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import type { Edge } from "@xyflow/react";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { deriveServer, flowUnitHash } from "./canvasOps";
import { compileHolon, inspectAt, printHolon, printHolonLines, sourceLineOf } from "./holonText";
import { manifestItems } from "./holon";
import { buildSample, samples } from "./samples";


// The sample list comes from the wasm module, and tests are made from it: loaded before they are.
initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

const HELLO = `import { concat, flow, route } from "holon"

flow("hello-api", { port: 8101 })

route.get("/hello/:name", (req) => concat("Hello, ", req.params.name))
`;

const unitOf = (doc: { nodes: unknown[]; edges: unknown[]; server?: { port: string; tags?: string[]; taint?: string[] } }, ev: Evaluator) =>
  flowUnitHash(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, doc.server?.port ?? "", doc.server ?? {});

describe("flows as text", () => {
  it("compiles a flow written by hand into a server with its route", () => {
    const ev = new Evaluator();
    const { name, doc } = compileHolon(HELLO, ev);
    expect(name).toBe("hello-api");
    expect((doc.nodes as PrimitiveNodeType[]).map((n) => n.data.name).sort()).toEqual(["concat", "const", "field", "request", "respond"]);
    expect(doc.server?.port).toBe("8101");
    const server = deriveServer(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, "8101", {});
    expect(server.errors).toEqual([]);
    expect(server.endpoints.map((e) => `${e.method} ${e.path}`)).toEqual(["GET /hello/:name"]);
  });

  it("carries a handler's log level to its draft, whatever the handler does", () => {
    const ev = new Evaluator();
    const board = `import { flow, route } from "holon"
import notes from "collection:notes"

flow("board", { port: 8170 })

route.get("/health", () => "ok", { log: "unrecorded" })

route.get("/notes", () => notes.list({ limit: 50 }), { log: "lazy" })

route.post("/notes", (req) => {
  notes[req.json.id] = req.json.text
  return "saved"
})
`;
    const { doc } = compileHolon(board, ev);
    const derive = (d: typeof doc) => deriveServer(d.nodes as PrimitiveNodeType[], d.edges as Edge[], ev, "8170", {});
    const fine = derive(doc);
    expect(fine.errors).toEqual([]);
    expect(fine.endpoints.map((e) => `${e.method} ${e.path} ${e.log}`)).toEqual(["GET /health unrecorded", "GET /notes lazy", "POST /notes "]);
    expect(printHolon("board", doc, ev)).toBe(board);
    // An unrecorded handler is an item to approve, by its label, as the server asks for it.
    expect(manifestItems(["NetworkIngress"], { unrecorded: ["GET /health"] })).toEqual(["NetworkIngress", 'Unrecorded("GET /health")']);
    // A write is as its author writes it: unrecorded, it compiles, and the server has a unit.
    const unrecorded = board.replace('  return "saved"\n})', '  return "saved"\n}, { log: "unrecorded" })');
    expect(unrecorded).not.toBe(board);
    const server = derive(compileHolon(unrecorded, ev).doc);
    expect(server.errors).toEqual([]);
    expect(server.endpoints.map((e) => `${e.method} ${e.path} ${e.log}`)).toContain("POST /notes unrecorded");
    expect(server.serverHash).not.toBeNull();
  });

  it("a template literal is one as_text more than the concat written by hand", () => {
    const ev = new Evaluator();
    const hand = compileHolon(HELLO, ev).doc;
    // The sample interpolates, which is one as_text more: a different unit, the same answer.
    const sample = buildSample(samples().find((s) => s.name === "hello-api")!, ev);
    const leaves = (d: { nodes: unknown[] }) => (d.nodes as PrimitiveNodeType[]).map((n) => n.data.name).sort();
    expect(leaves(sample)).toEqual([...leaves(hand), "as_text"].sort());
    expect(unitOf(sample, ev)).not.toBe(unitOf(hand, ev));
  });

  it.each(samples().map((s) => [s.name]))("round-trips the %s sample: printed, compiled back, the identical unit", (sampleName) => {
    const ev = new Evaluator();
    const original = buildSample(samples().find((s) => s.name === sampleName)!, ev);
    const text = printHolon(sampleName, original, ev);
    // A fresh workspace: everything it needs comes from the text.
    const fresh = new Evaluator();
    const back = compileHolon(text, fresh);
    expect(unitOf(back.doc, fresh), text).toBe(unitOf(original, ev));
    expect(printHolon(sampleName, back.doc, fresh)).toBe(text);
  });

  it("variadic nodes keep their inputs, and functions their boundary as written", () => {
    const ev = new Evaluator();
    const text = `import { concat, flow, route } from "holon"

flow("joined", { port: 8000 })

function shout(words) {
  const said = words.toUpperCase()
  return said
}

route.post("/j", (req) => concat(shout(req.json.words), "-", "!"))
`;
    const { doc } = compileHolon(text, ev);
    const c = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "concat")!;
    expect(c.data.inputs.map((p) => p.name)).toEqual(["a", "b", "c"]);
    const s = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "shout")!;
    expect(s.data.inputs.map((p) => p.name)).toEqual(["words"]);
    expect(s.data.outputs.map((p) => p.name)).toEqual(["said"]);
    expect(printHolon("joined", doc, ev)).toContain("function shout(words) {");
  });

  it("names the line and column of each mistake", () => {
    const ev = new Evaluator();
    const fails = (text: string) => {
      try {
        compileHolon(text, ev);
        return "";
      } catch (e) {
        return e instanceof Error ? e.message : String(e);
      }
    };
    const flow = (body: string) => `import { flow, route } from "holon"\n\nflow("f", { port: 8200 })\n\n${body}\n`;
    expect(fails(flow('route.get("/a", () => nope)'))).toBe('line 5, column 23: unknown name "nope"');
    // Into a template literal too.
    expect(fails(flow('route.get("/a", () => `${nope} likes`)'))).toBe('line 5, column 26: unknown name "nope"');
    expect(fails('import { fieldd, flow } from "holon"\n\nflow("f")\n')).toMatch(/^line 1, column 10: "holon" has no "fieldd"/);
    expect(fails(flow('route.get("/a", () => "oops)'))).toMatch(/^line 5, column \d+: /);
    expect(fails('import { route } from "holon"\n\nroute.get("/a", () => 1)\n')).toContain("a flow says what it is first");
  });

  it("writes a canvas as text, keeps a text's names through it, and knows each node's line", () => {
    const ev = new Evaluator();
    const src = 'import { flow, route } from "holon"\n\nflow("f", { port: 8200 })\n\nroute.get("/x", (req) => {\n  const n = req.params.id\n  return `${n} likes`\n})\n';
    const { name, doc, names } = compileHolon(src, ev);
    expect(printHolon(name, doc, ev, names)).toBe(src);
    const { text, lines } = printHolonLines(name, doc, ev, names);
    expect(text).toBe(src);
    const field = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "field")!;
    expect(lines.get(field.id)).toBe(6);
    // The Inspector's line is of the canvas as it prints without a text's names: inline.
    expect(sourceLineOf(field.id, name, doc, ev)).toEqual({ line: 5, text: 'route.get("/x", (req) => `${req.params.id} likes`)' });
  });
});

describe("the name at the cursor", () => {
  it("says its hover, the forms it is written in and a sample's lines, also just after it", () => {
    const at = HELLO.indexOf("route.get");
    const said = inspectAt(HELLO, at + 2);
    expect(said?.word).toBe("route");
    expect(said?.hover).not.toBe("");
    expect(said?.usage?.forms.some(([form]) => form.startsWith("route."))).toBe(true);
    expect(said?.usage?.example?.lines.some((l) => l.includes("route."))).toBe(true);
    // Just typed: the cursor right after the name.
    expect(inspectAt(HELLO, at + "route".length)?.word).toBe("route");
    expect(inspectAt(HELLO, HELLO.indexOf("8101") + 1)).toBeNull();
  });
});
