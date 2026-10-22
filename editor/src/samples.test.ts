// Every sample flow builds from the live catalog and derives through the real core (the wasm
// package, loaded from disk): a server sample becomes a server unit with one endpoint per request
// node, a clock sample projects whole, and none asks for more than it uses (an unknown leaf would
// make core fail closed and ask for everything). A sample that stops building fails here, so the
// samples can't silently rot as core changes (CLAUDE.md).

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { initSync, Evaluator, primitive_catalog, variadic_primitives } from "../wasm/holon_wasm.js";
import { buildPreimage, type WUnit, type WValue } from "./holon";
import { fromHex } from "./flowDoc";
import { deriveServer, flowAsNode, flowUnitHash } from "./canvasOps";
import { capabilityAcknowledgeName, manifestItems, manifestTrigger, permissionKind } from "./diagnostics";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { Edge } from "@xyflow/react";
import { SAMPLES, buildSample } from "./samples";

let catalog: Map<string, WUnit>;
let variadic: Set<string>;

beforeAll(() => {
  initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });
  catalog = new Map();
  for (const u of primitive_catalog() as WUnit[]) if (u.body.kind === "Primitive") catalog.set(u.body.name, u);
  variadic = new Set(variadic_primitives() as string[]);
});

/** What deploying `sample` asks for, by consent name, or why it can't deploy. */
function derive(name: string): { caps: string[]; endpoints: number } {
  const sample = SAMPLES.find((s) => s.name === name)!;
  const doc = buildSample(sample, catalog, variadic, new Evaluator());
  const nodes = doc.nodes as PrimitiveNodeType[];
  const edges = doc.edges as Edge[];
  // Derived in a fresh workspace, from what the document carries: as opening it does.
  const ev = new Evaluator();
  for (const hex of doc.units ?? []) ev.insert(fromHex(hex));
  // The unit deploying it would deploy (and the flows page derives a saved flow's manifest from).
  const hash = flowUnitHash(nodes, edges, ev, catalog, sample.port ?? "8080");
  const endpoints = sample.port ? deriveServer(nodes, edges, ev, catalog, sample.port).endpoints.length : 0;
  const caps = (ev.capabilities(hash) as Parameters<typeof capabilityAcknowledgeName>[0][]).map(capabilityAcknowledgeName).sort();
  return { caps, endpoints };
}

describe("sample flows", () => {
  it("have unique names, and a port or an interval", () => {
    expect(new Set(SAMPLES.map((s) => s.name)).size).toBe(SAMPLES.length);
    for (const s of SAMPLES) expect(Boolean(s.port) !== Boolean(s.interval), s.name).toBe(true);
    const ports = SAMPLES.flatMap((s) => (s.port ? [s.port] : []));
    expect(new Set(ports).size, "each server its own port").toBe(ports.length);
  });

  for (const s of SAMPLES) {
    it(`${s.name} builds, derives, and asks for exactly what it uses`, () => {
      const doc = buildSample(s, catalog, variadic, new Evaluator());
      const requests = (doc.nodes as PrimitiveNodeType[]).filter((n) => n.data.name === "request" && n.data.body.kind === "Template");
      const { caps, endpoints } = derive(s.name);
      expect(endpoints).toBe(requests.length);
      expect(caps.some((c) => c.includes("FileRead") || c.includes("ProcessSpawn")), `${s.name} saturated: ${caps}`).toBe(false);
      for (const secret of s.secrets ?? []) expect(caps).toContain(`SecretRead("${secret.name}")`);
      // What it can run without is something it uses, and never what starts it.
      for (const item of s.optional ?? []) {
        expect(caps).toContain(item);
        expect(item).not.toBe(manifestTrigger(caps));
      }
    });
  }

  it("list a manifest as the server does: capabilities, then requirements, sorted", () => {
    expect(manifestItems(derive("guarded-api").caps, { limits: { max_requests: 80 }, plain_http: ["a.local:8080"], private_addresses: true })).toEqual([
      "Limit(max_requests=80)",
      "NetworkIngress",
      'PlainHttp("a.local:8080")',
      "PrivateAddresses",
      'SecretRead("api_token")',
      'SecretRead("receipt_key")',
    ]);
    expect(manifestItems(derive("hello-api").caps)).toEqual(["NetworkIngress"]);
  });

  it("tell required permissions from optional ones, the trigger always required", () => {
    const { caps } = derive("guarded-api");
    const optional = SAMPLES.find((s) => s.name === "guarded-api")!.optional;
    const trigger = manifestTrigger(caps);
    expect(trigger).toBe("NetworkIngress");
    expect(manifestTrigger(derive("heartbeat").caps)).toBe("Clock");
    expect(permissionKind('SecretRead("receipt_key")', optional, trigger).kind).toBe("optional");
    expect(permissionKind('SecretRead("api_token")', optional, trigger).kind).toBe("required");
    expect(permissionKind("NetworkIngress", ["NetworkIngress"], trigger).kind, "a trigger even if marked").toBe("required");
  });

  it("place a whole flow as a node: one unit, wherever it's placed, that works", () => {
    const ev = new Evaluator();
    const doc = buildSample(SAMPLES.find((s) => s.name === "slugs")!, catalog, variadic, ev);
    const placed = (doc.nodes as PrimitiveNodeType[]).filter((n) => n.data.body.kind === "Composite" && n.data.name === "slugify");
    expect(placed.length).toBe(2);
    const hashes = new Set(placed.map((n) => (n.data.body.kind === "Composite" ? n.data.body.hash : "")));
    expect(hashes.size, "the same unit twice").toBe(1);
    expect(placed[0].data.inputs.map((p) => p.name)).toEqual(["text"]);
    expect(placed[0].data.outputs.map((p) => p.name)).toEqual(["slug"]);
    const [hash] = hashes;
    const unit = ev.get_unit(hash) as WUnit;
    const out = ev.eval(buildPreimage(unit), [{ type: "Text", value: "Hello Holon World" }]) as (WValue | null)[];
    expect(out).toEqual([{ type: "Text", value: "hello-holon-world" }]);
    expect(doc.units?.length, "its closure travels with the document").toBeGreaterThan(0);
    expect(() =>
      buildSample({ ...SAMPLES[0], build: (b) => void b.holon("h", (i) => void i.node("lowercase"), { inputs: ["a", "b"], outputs: ["c"] }) }, catalog, variadic, new Evaluator()),
    ).toThrow('holon "h" has inputs');
  });

  it("place a holon that reads and writes outside: its ports are what they say, its needs in the manifest", () => {
    const ev = new Evaluator();
    const doc = buildSample(SAMPLES.find((s) => s.name === "slugs")!, catalog, variadic, ev);
    const publish = (doc.nodes as PrimitiveNodeType[]).find((n) => n.data.name === "publish")!;
    if (publish.data.body.kind !== "Composite") throw new Error("not a composite");
    const unit = ev.get_unit(publish.data.body.hash) as WUnit;
    // Evaluated offline: no clock or secret is sourced, so the post isn't put and the receipt is
    // unsigned; the state write still comes out, naming the slug. Each output is what it's named.
    const out = ev.eval(buildPreimage(unit), [{ type: "Text", value: "hello-holon" }, { type: "Text", value: "ada" }]) as (WValue | null)[];
    const at = (name: string) => out[publish.data.outputs.findIndex((p) => p.name === name)] ?? null;
    expect(at("receipt")).toEqual({ type: "Text", value: "unsigned" });
    expect(at("saved")).toEqual({ type: "Effect", value: { sink: "store_write:last_post", payload: { type: "Text", value: "hello-holon" } } });
    expect(at("put")).toBeNull();
    // Everything it reads and writes inside is in the flow's manifest; the receipt key optional.
    const { caps } = derive("slugs");
    for (const c of ["Clock", 'CollectionWrite("posts")', 'CollectionRead("posts")', 'StateWrite("last_post")', 'SecretRead("receipt_key")']) {
      expect(caps).toContain(c);
    }
  });

  it("place a saved flow as one node: grouped whole, holons inside it and all; never a server", () => {
    // A flow without routes that itself holds a holon: slugify, then uppercase.
    const shout: (typeof SAMPLES)[number] = {
      ...SAMPLES.find((s) => s.name === "slugs")!,
      name: "shout",
      build(b) {
        const slugify = b.holon(
          "slugify",
          (inner) => {
            const lower = inner.node("lowercase", {}, [0, 0]);
            const space = inner.node("const", { value: " ", type: "Text" }, [0, 120]);
            const words = inner.node("split", {}, [240, 40]);
            const dash = inner.node("const", { value: "-", type: "Text" }, [240, 160]);
            const slug = inner.node("join", {}, [480, 60]);
            inner.wire(lower, "out", words, "text");
            inner.wire(space, "out", words, "sep");
            inner.wire(words, "out", slug, "list");
            inner.wire(dash, "out", slug, "sep");
          },
          { inputs: ["text"], outputs: ["slug"] },
        );
        const placed = b.place(slugify);
        const upper = b.node("uppercase");
        b.wire(placed, "slug", upper, "s");
      },
    };
    const saved = buildSample(shout, catalog, variadic, new Evaluator());
    // Placed in another flow's workspace, from what the saved document carries.
    const ev = new Evaluator();
    const placed = flowAsNode("shout", saved, ev, catalog, "shout-1", { x: 0, y: 0 });
    if (!placed.ok) throw new Error(placed.message);
    expect(placed.node.data.name).toBe("shout");
    expect([placed.node.data.inputs.length, placed.node.data.outputs.length]).toEqual([1, 1]);
    const body = placed.node.data.body;
    if (body.kind !== "Composite") throw new Error("not a composite");
    const out = ev.eval(buildPreimage(ev.get_unit(body.hash) as WUnit), [{ type: "Text", value: "Hello Holon" }]) as (WValue | null)[];
    expect(out).toEqual([{ type: "Text", value: "HELLO-HOLON" }]);

    const server = buildSample(SAMPLES.find((s) => s.name === "hello-api")!, catalog, variadic, new Evaluator());
    const refused = flowAsNode("hello-api", server, new Evaluator(), catalog, "x", { x: 0, y: 0 });
    expect(refused.ok ? "" : refused.message).toContain("has routes");
    const empty = flowAsNode("e", { nodes: [], edges: [] }, new Evaluator(), catalog, "x", { x: 0, y: 0 });
    expect(empty.ok ? "" : empty.message).toContain("empty");
  });

  it("ask for what each one shows", () => {
    expect(derive("hello-api").caps).toEqual(["NetworkIngress"]);
    expect(derive("notes-api").caps).toEqual(['CollectionRead("notes")', 'CollectionWrite("notes")', "NetworkIngress"]);
    expect(derive("stripe-to-slack").caps).toEqual([
      "Clock",
      'HttpRequest("hooks.slack.com")',
      "NetworkIngress",
      'SecretRead("slack_webhook_path")',
      'SecretRead("stripe_webhook_secret")',
    ]);
    expect(derive("heartbeat").caps).toEqual(["Clock", 'StateRead("beats")', 'StateWrite("beats")', 'StateWrite("last_beat")']);
    expect(derive("approval").caps).toEqual(['Await("paid")', "NetworkIngress", 'StateWrite("last_order")']);
    expect(derive("fetch-json").caps).toEqual(['HttpRequest("httpbin.org")', "NetworkIngress"]);
    expect(derive("sign-payload").caps).toEqual(["NetworkIngress", 'SecretRead("signing_key")']);
    expect(derive("likes").caps).toEqual(['CollectionRead("likes")', 'CollectionWrite("likes")', "NetworkIngress"]);
  });

  it("refuse a misspelt leaf, param or port, naming the sample", () => {
    const broken = { ...SAMPLES[0], build: (b: Parameters<(typeof SAMPLES)[0]["build"]>[0]) => b.wire(b.node("field"), "nope", b.node("concat"), "a") };
    expect(() => buildSample(broken, catalog, variadic, new Evaluator())).toThrow('sample "hello-api": field has no output "nope"');
    expect(() => buildSample({ ...SAMPLES[0], build: (b) => void b.node("fieldd") }, catalog, variadic, new Evaluator())).toThrow('no leaf "fieldd"');
    expect(() => buildSample({ ...SAMPLES[0], build: (b) => void b.node("field", { nam: "x" }) }, catalog, variadic, new Evaluator())).toThrow('has no param "nam"');
  });
});
