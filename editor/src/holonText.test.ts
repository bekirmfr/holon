// Flows as text (holonText.ts) against the real core: written by hand, every sample printed and
// compiled back to the identical unit, and mistakes named by their line.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { initSync, Evaluator, primitive_catalog, variadic_primitives } from "../wasm/holon_wasm.js";
import type { WUnit } from "./holon";
import type { Edge } from "@xyflow/react";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { deriveServer, flowUnitHash } from "./canvasOps";
import { compileHolon, parseHolon, printHolon } from "./holonText";
import { SAMPLES, buildSample } from "./samples";

let catalog: Map<string, WUnit>;
let variadic: Set<string>;

beforeAll(() => {
  initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });
  catalog = new Map((primitive_catalog() as WUnit[]).map((u) => [u.body.kind === "Primitive" ? u.body.name : "", u]));
  variadic = new Set(variadic_primitives() as string[]);
});

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
  flowUnitHash(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, catalog, doc.server?.port ?? "");

describe("flows as text", () => {
  it("compiles a flow written by hand into a server with its route", () => {
    const ev = new Evaluator();
    const { name, doc } = compileHolon(HELLO, catalog, variadic, ev);
    expect(name).toBe("hello-api");
    expect(doc.server?.port).toBe("8101");
    const server = deriveServer(doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, catalog, "8101");
    expect(server.errors).toEqual([]);
    expect(server.endpoints.map((e) => `${e.method} ${e.path}`)).toEqual(["GET /hello/:name"]);
  });

  it("is the same unit as the same flow built on the canvas (the Hello API sample)", () => {
    const ev = new Evaluator();
    const sample = buildSample(SAMPLES.find((s) => s.name === "hello-api")!, catalog, variadic, ev);
    expect(unitOf(compileHolon(HELLO, catalog, variadic, ev).doc, ev)).toBe(unitOf(sample, ev));
  });

  it.each(SAMPLES.map((s) => [s.name]))("round-trips the %s sample: printed, compiled back, the identical unit", (sampleName) => {
    const ev = new Evaluator();
    const original = buildSample(SAMPLES.find((s) => s.name === sampleName)!, catalog, variadic, ev);
    const text = printHolon(sampleName, original, catalog, variadic, ev);
    // A fresh workspace: everything it needs comes from the text.
    const fresh = new Evaluator();
    const back = compileHolon(text, catalog, variadic, fresh);
    expect(unitOf(back.doc, fresh), text).toBe(unitOf(original, ev));
    expect(printHolon(sampleName, back.doc, catalog, variadic, fresh)).toBe(text);
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
    const { doc } = compileHolon(text, catalog, variadic, ev);
    const c = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "concat")!;
    expect(c.data.inputs.map((p) => p.name)).toEqual(["a", "b", "c"]);
    const s = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "shout")!;
    expect(s.data.inputs.map((p) => p.name)).toEqual(["words"]);
    expect(printHolon("joined", doc, catalog, variadic, ev)).toContain(`holon shout(words "join · list") -> (said`);
  });

  it("names the line of each mistake", () => {
    const ev = new Evaluator();
    const fails = (text: string) => {
      try {
        compileHolon(text, catalog, variadic, ev);
        return "";
      } catch (e) {
        return e instanceof Error ? e.message : String(e);
      }
    };
    expect(fails("node a = fieldd")).toBe('line 1: no leaf "fieldd" in the catalog');
    expect(fails("node a = field { nam: x }")).toBe('line 1: field has no param "nam"');
    expect(fails("node a = field\nnode b = concat\na.nope -> b.a")).toBe('line 3: field has no output "nope"');
    expect(fails("node a = field\nx.out -> a.record")).toContain('line 2: no node "x"');
    expect(fails('node a = const { value: "oops }')).toBe("line 1: a quote isn't closed");
    expect(fails("holon h() -> () {\nnode a = field")).toBe('line 1: the holon "h" isn\'t closed with }');
    expect(fails("node a = field\nnode a = field")).toBe('line 2: there is a node "a" already');
    expect(() => parseHolon("node in = field")).toThrow("names a holon's boundary");
  });
});
