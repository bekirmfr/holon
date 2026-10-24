// Flows as text (holonText.ts) against the real core: written by hand, every sample printed and
// compiled back to the identical unit, and mistakes named by their line.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import type { Edge } from "@xyflow/react";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { deriveServer, flowUnitHash } from "./canvasOps";
import { compileHolon, printHolon, printHolonLines, sourceLineOf } from "./holonText";
import { buildSample, samples } from "./samples";


// The sample list comes from the wasm module, and tests are made from it: loaded before they are.
initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

const HELLO = `flow hello-api
port 8101

node req   = request { method: GET, path: "/hello/:name" }
node name  = field { name: name }
node hi    = const { value: "Hello, ", type: Text }
node greet = concat
node res   = respond { status: "200", missing: "404" }

req.params  -> name.record
hi.out      -> greet.a
name.out    -> greet.b
req.request -> res.request
greet.out   -> res.body
`;

const unitOf = (doc: { nodes: unknown[]; edges: unknown[]; server?: { port: string } }, ev: Evaluator) =>
  flowUnitHash(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, doc.server?.port ?? "");

describe("flows as text", () => {
  it("compiles a flow written by hand into a server with its route", () => {
    const ev = new Evaluator();
    const { name, doc, names } = compileHolon(HELLO, ev);
    expect(name).toBe("hello-api");
    // Each node by its name in the text (what `holon show` prints).
    expect((doc.nodes as PrimitiveNodeType[]).map((n) => `${names.get(n.id)}=${n.data.name}`)).toEqual(["req=request", "name=field", "hi=const", "greet=concat", "res=respond"]);
    expect(doc.server?.port).toBe("8101");
    const server = deriveServer(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, "8101");
    expect(server.errors).toEqual([]);
    expect(server.endpoints.map((e) => `${e.method} ${e.path}`)).toEqual(["GET /hello/:name"]);
  });

  it("is the same unit as the same flow built on the canvas (the Hello API sample)", () => {
    const ev = new Evaluator();
    const sample = buildSample(samples().find((s) => s.name === "hello-api")!, ev);
    expect(unitOf(compileHolon(HELLO, ev).doc, ev)).toBe(unitOf(sample, ev));
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

  it("variadic nodes keep their inputs, and holons their boundary as given", () => {
    const ev = new Evaluator();
    const text = `flow joined
holon shout(words "join · list") -> (said) {
  node j = join
  node up = uppercase
  j.out -> up.s
  in.words -> j.list
  up.out -> out.said
}
node c = concat[3]
node s = shout
`;
    const { doc } = compileHolon(text, ev);
    const c = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "concat")!;
    expect(c.data.inputs.map((p) => p.name)).toEqual(["a", "b", "c"]);
    const s = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "shout")!;
    expect(s.data.inputs.map((p) => p.name)).toEqual(["words"]);
    expect(printHolon("joined", doc, ev)).toContain(`holon shout(words "join · list") -> said`);
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
    expect(fails("node a = fieldd")).toBe('line 1, column 10: no leaf "fieldd" in the catalog');
    expect(fails("node a = field { nam: x }")).toBe('line 1, column 10: field has no param "nam"');
    expect(fails("node a = field\nnode b = concat\na.nope -> b.a")).toBe('line 3, column 1: field has no output "nope"');
    expect(fails("node a = field\nx.out -> a.record")).toContain('line 2, column 1: no node "x"');
    expect(fails('node a = const { value: "oops }')).toBe("line 1, column 25: a quote isn't closed");
    expect(fails("holon h() -> () {\nnode a = field")).toBe("line 1, column 17: the block isn't closed with }");
    expect(fails("node a = field\nnode a = field")).toBe('line 2, column 6: there is a node "a" already');
    expect(fails("node in = field")).toContain("names a holon's boundary");
    // The expression form too, into the string.
    expect(fails('route GET /a (req) {\n  respond "{nope} likes"\n}')).toBe('line 2, column 13: unknown name "nope"');
  });

  it("writes the expression form, keeps a text's names through it, and knows each node's line", () => {
    const ev = new Evaluator();
    const src = "flow f\nport 8200\n\nroute GET /x (req) {\n  n = req.params.id\n  respond \"{n} likes\"\n}\n";
    const { name, doc, names } = compileHolon(src, ev);
    expect(printHolon(name, doc, ev, names)).toBe(src);
    const { text, lines } = printHolonLines(name, doc, ev, names);
    expect(text).toBe(src);
    const field = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "field")!;
    expect(lines.get(field.id)).toBe(5);
    // The Inspector's line is of the canvas as it prints without a text's names: inline.
    expect(sourceLineOf(field.id, name, doc, ev)).toEqual({ line: 5, text: '  respond "{req.params.id} likes"' });
    // A canvas the expression form can't spell (an unwired input) is written as nodes and wires.
    const { doc: solo, names: soloNames } = compileHolon("flow solo\nnode up = uppercase\n", ev);
    expect(printHolon("solo", solo, ev, soloNames)).toBe("flow solo\n\nnode up = uppercase\n");
    expect(printHolon("solo", solo, ev)).toBe("flow solo\n\nnode uppercase = uppercase\n");
  });
});
