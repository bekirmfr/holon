// Every sample flow builds from the live catalog and derives through the real core (the wasm
// package, loaded from disk): a server sample becomes a server unit with one endpoint per request
// node, a clock sample projects whole, and none asks for more than it uses (an unknown leaf would
// make core fail closed and ask for everything). A sample that stops building fails here, so the
// samples can't silently rot as core changes (CLAUDE.md).

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { initSync, Evaluator, primitive_catalog, primitive_docs } from "../wasm/holon_wasm.js";
import { buildPreimage, type WUnit, type WValue } from "./holon";
import { deriveServer, flowAsNode, flowUnitHash } from "./canvasOps";
import {
  BOUNDARY_IN,
  BOUNDARY_OUT,
  boundaryRenames,
  freezeInner,
  innerCanvas,
  replaceComposite,
  settleBoundary,
  type BoundaryData,
  type BoundaryPort,
} from "./composites";
import { foldLevels } from "./CompositeOverlay";
import { documentUnits, knownNames, nameOf, restoreDocument } from "./docUnits";
import { manifestItems, manifestTrigger, permissionKind } from "./holon";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import type { Edge } from "@xyflow/react";
import { buildSample, samples } from "./samples";
import { bindingsOf, compileHolon } from "./holonText";
import { templateDoc, templateNames } from "./templates";
import { nodeDescriber } from "./NodeDocs";
import { restoreUnits } from "./docUnits";

let catalog: Map<string, WUnit>;

// The sample list comes from the wasm module, and tests are made from it: loaded before they are.
initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

beforeAll(() => {
  catalog = new Map();
  for (const u of primitive_catalog() as WUnit[]) if (u.body.kind === "Primitive") catalog.set(u.body.name, u);
});

/** What deploying `sample` asks for, by consent name, or why it can't deploy. */
function derive(name: string): { caps: string[]; endpoints: number } {
  const sample = samples().find((s) => s.name === name)!;
  const doc = buildSample(sample, new Evaluator());
  const nodes = doc.nodes as PrimitiveNodeType[];
  const edges = doc.edges as Edge[];
  // Derived in a fresh workspace, from what the document carries: as opening it does.
  const ev = new Evaluator();
  restoreUnits(doc, ev);
  // The unit deploying it would deploy (and the flows page derives a saved flow's manifest from).
  const hash = flowUnitHash(nodes, edges, ev, sample.port ?? "8080");
  const endpoints = sample.port ? deriveServer(nodes, edges, ev, sample.port).endpoints.length : 0;
  const caps = (ev.capability_names(hash) as string[]).sort();
  return { caps, endpoints };
}

describe("sample flows", () => {
  it("have unique names, and a port or an interval", () => {
    expect(new Set(samples().map((s) => s.name)).size).toBe(samples().length);
    for (const s of samples()) expect(Boolean(s.port) !== Boolean(s.interval), s.name).toBe(true);
    const ports = samples().flatMap((s) => (s.port ? [s.port] : []));
    expect(new Set(ports).size, "each server its own port").toBe(ports.length);
  });

  for (const s of samples()) {
    it(`${s.name} builds, derives, and asks for exactly what it uses`, () => {
      const doc = buildSample(s, new Evaluator());
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
    const optional = samples().find((s) => s.name === "guarded-api")!.optional;
    const trigger = manifestTrigger(caps);
    expect(trigger).toBe("NetworkIngress");
    expect(manifestTrigger(derive("heartbeat").caps)).toBe("Clock");
    expect(permissionKind('SecretRead("receipt_key")', optional, trigger).kind).toBe("optional");
    expect(permissionKind('SecretRead("api_token")', optional, trigger).kind).toBe("required");
    expect(permissionKind("NetworkIngress", ["NetworkIngress"], trigger).kind, "a trigger even if marked").toBe("required");
  });

  it("place a whole flow as a node: one unit, wherever it's placed, that works", () => {
    const ev = new Evaluator();
    const doc = buildSample(samples().find((s) => s.name === "slugs")!, ev);
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
  });

  it("place a holon that reads and writes outside: its ports are what they say, its needs in the manifest", () => {
    const ev = new Evaluator();
    const doc = buildSample(samples().find((s) => s.name === "slugs")!, ev);
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
    const { doc: saved } = compileHolon(
      `flow shout
holon slugify(text) -> (slug) {
  node lower = lowercase
  node space = const { value: " ", type: Text }
  node words = split
  node dash = const { value: "-", type: Text }
  node slug = join
  in.text -> lower.s
  lower.out -> words.text
  space.out -> words.sep
  words.out -> slug.list
  dash.out -> slug.sep
  slug.out -> out.slug
}
node placed = slugify
node upper = uppercase
placed.slug -> upper.s
`,
      new Evaluator(),
    );
    // Placed in another flow's workspace, from what the saved document carries.
    const ev = new Evaluator();
    const placed = flowAsNode("shout", saved, ev, "shout-1", { x: 0, y: 0 });
    if (!placed.ok) throw new Error(placed.message);
    expect(placed.node.data.name).toBe("shout");
    expect([placed.node.data.inputs.length, placed.node.data.outputs.length]).toEqual([1, 1]);
    const body = placed.node.data.body;
    if (body.kind !== "Composite") throw new Error("not a composite");
    const out = ev.eval(buildPreimage(ev.get_unit(body.hash) as WUnit), [{ type: "Text", value: "Hello Holon" }]) as (WValue | null)[];
    expect(out).toEqual([{ type: "Text", value: "HELLO-HOLON" }]);

    const server = buildSample(samples().find((s) => s.name === "hello-api")!, new Evaluator());
    const refused = flowAsNode("hello-api", server, new Evaluator(), "x", { x: 0, y: 0 });
    expect(refused.ok ? "" : refused.message).toContain("has routes");
    const empty = flowAsNode("e", { nodes: [], edges: [] }, new Evaluator(), "x", { x: 0, y: 0 });
    expect(empty.ok ? "" : empty.message).toContain("empty");
  });

  it("say what every node does: core's primitives, the templates, a holon from what it is", () => {
    const raw = primitive_docs() as Map<string, string>;
    const docs = raw instanceof Map ? raw : new Map(Object.entries(raw as Record<string, string>));
    for (const name of catalog.keys()) expect(docs.get(name), name).toBeTruthy();
    for (const t of templateNames()) expect(templateDoc(t)?.length, t).toBeGreaterThan(40);
    const describe = nodeDescriber(docs, () => 5);
    const port = (name: string) => ({ name, ty: "Text" as const });
    expect(describe({ name: "split", body: { kind: "Primitive" }, inputs: [], outputs: [] })).toBe(docs.get("split"));
    expect(describe({ name: "respond", body: { kind: "Template" }, inputs: [], outputs: [] })).toBe(templateDoc("respond"));
    const holon = describe({ name: "slugify", body: { kind: "Composite", hash: "h" }, inputs: [port("text")], outputs: [port("slug")] });
    expect(holon).toContain("of 5 nodes");
    expect(holon).toContain("Takes text; gives slug");
  });

  it("open a holon as a canvas of its own: its nodes, its wires, its boundary and what that is wired to", () => {
    const ev = new Evaluator();
    const doc = buildSample(samples().find((s) => s.name === "slugs")!, ev);
    const nodes = doc.nodes as PrimitiveNodeType[];
    const edges = doc.edges as Edge[];
    const publish = nodes.find((n) => n.data.name === "publish")!;
    const opened = innerCanvas(publish, nodes, edges, ev);
    if (!opened.ok) throw new Error(opened.message);
    // The holon's leaves, plus the pass-through its thrice-used `slug` input shares (a guard and
    // its constant true), as the language lowers it (docs/LANGUAGE.md, step 3).
    expect(opened.childIds.length).toBe(11);
    expect(opened.nodes.length).toBe(13);
    const boundary = (id: string) => opened.nodes.find((n) => n.id === id)!.data as BoundaryData;
    // slug arrives through the `?? ""` at the call site (a default), author straight from slugify.
    expect(boundary(BOUNDARY_IN).ports.map((p) => [p.name, p.outside])).toEqual([
      ["slug", ["default"]],
      ["author", ["slugify"]],
    ]);
    expect(boundary(BOUNDARY_OUT).ports.map((p) => [p.name, p.outside])).toEqual([
      ["put", []],
      ["saved", []],
      ["receipt", ["with_field.value"]],
    ]);
    // Every wire lands on a real port: inside, and through the boundary both ways.
    const ports = new Map(opened.nodes.map((n) => [n.id, n]));
    for (const e of opened.edges) {
      expect(ports.has(e.source) && ports.has(e.target), e.id).toBe(true);
    }
    // slug enters once, through the pass-through that fans out inside.
    expect(opened.edges.filter((e) => e.source === BOUNDARY_IN).length).toBe(2);
    expect(opened.edges.filter((e) => e.target === BOUNDARY_OUT).length).toBe(3);
    // Laid out by how far data has come: no two nodes on the same spot.
    const spots = new Set(opened.nodes.map((n) => `${n.position.x},${n.position.y}`));
    expect(spots.size).toBe(opened.nodes.length);
    // A primitive isn't a composite.
    expect(innerCanvas(nodes.find((n) => n.data.name === "record")!, nodes, edges, ev).ok).toBe(false);
  });

  it("freeze an inner canvas back into a composite: unchanged, the same unit; edited, a new one", () => {
    const ev = new Evaluator();
    const doc = buildSample(samples().find((s) => s.name === "slugs")!, ev);
    const nodes = doc.nodes as PrimitiveNodeType[];
    const publish = nodes.find((n) => n.data.name === "publish")!;
    const opened = innerCanvas(publish, nodes, doc.edges as Edge[], ev);
    if (!opened.ok) throw new Error(opened.message);
    const canvas = { nodes: opened.nodes, edges: opened.edges };
    const same = freezeInner(canvas, ev);
    if (!same.ok) throw new Error(same.message);
    expect(same.hash, "nothing changed: the very same unit").toBe(publish.data.body.kind === "Composite" ? publish.data.body.hash : "");
    expect(same.inputs.map((p) => p.name)).toEqual(["slug", "author"]);

    // Renaming a boundary port renames it on the node only: the unit is the same.
    const renamed = {
      nodes: canvas.nodes.map((n) => (n.id === BOUNDARY_IN ? { ...n, data: { ...(n.data as BoundaryData), ports: (n.data as BoundaryData).ports.map((p) => (p.name === "slug" ? { ...p, name: "key" } : p)) } } : n)),
      edges: canvas.edges.map((e) => (e.source === BOUNDARY_IN && e.sourceHandle === "slug" ? { ...e, sourceHandle: "key" } : e)),
    };
    const same2 = freezeInner(renamed, ev);
    if (!same2.ok) throw new Error(same2.message);
    expect(same2.hash).toBe(same.hash);
    expect(same2.inputs.map((p) => p.name)).toEqual(["key", "author"]);
    // Changing what's inside is another unit: the receipt's fallback, say.
    const edited = {
      ...canvas,
      nodes: canvas.nodes.map((n) => {
        const d = n.data as PrimitiveNodeType["data"];
        if (d.name !== "default" || !d.params?.some((p) => p.kind === "Frozen" && p.value === "unsigned")) return n;
        return { ...n, data: { ...d, params: d.params.map((p) => (p.name === "value" ? { ...p, value: "not signed" } : p)) } };
      }),
    };
    const other = freezeInner(edited, ev);
    if (!other.ok) throw new Error(other.message);
    expect(other.hash).not.toBe(same.hash);
    const out = ev.eval(buildPreimage(ev.get_unit(other.hash) as WUnit), [{ type: "Text", value: "x" }, { type: "Text", value: "y" }]) as (WValue | null)[];
    expect(out[2]).toEqual({ type: "Text", value: "not signed" });

    // An input feeds one port; an output needs a source.
    const authorEdge = canvas.edges.find((e) => e.source === BOUNDARY_IN && e.sourceHandle === "author")!;
    const fanned = { ...canvas, edges: [...canvas.edges, { ...authorEdge, id: "again", target: canvas.edges.find((e) => e.source === BOUNDARY_IN && e.sourceHandle === "slug")!.target }] };
    const twice = freezeInner(fanned, ev);
    expect(twice.ok ? "" : twice.message).toContain("feeds 2 ports");
    const unfed = { ...canvas, edges: canvas.edges.filter((e) => !(e.target === BOUNDARY_OUT && e.targetHandle === "receipt")) };
    const lost = freezeInner(unfed, ev);
    expect(lost.ok ? "" : lost.message).toContain(`output "receipt" isn't fed`);
  });

  it("apply an edited inner canvas: the flow's node becomes the new version, its wires following renamed ports", () => {
    const ev = new Evaluator();
    const doc = buildSample(samples().find((s) => s.name === "slugs")!, ev);
    const nodes = doc.nodes as PrimitiveNodeType[];
    const edges = doc.edges as Edge[];
    const publish = nodes.find((n) => n.data.name === "publish")!;
    const opened = innerCanvas(publish, nodes, edges, ev);
    if (!opened.ok) throw new Error(opened.message);
    // Rename the input "slug" to "key", and drop the output "receipt".
    const port = (side: string, f: (ps: BoundaryPort[]) => BoundaryPort[]) => (n: (typeof opened.nodes)[number]) =>
      n.id === side ? { ...n, data: { ...(n.data as BoundaryData), ports: f((n.data as BoundaryData).ports) } } : n;
    const canvas = {
      nodes: opened.nodes.map(port(BOUNDARY_IN, (ps) => ps.map((p) => (p.name === "slug" ? { ...p, name: "key" } : p)))).map(port(BOUNDARY_OUT, (ps) => ps.filter((p) => p.name !== "receipt"))),
      edges: opened.edges
        .map((e) => (e.source === BOUNDARY_IN && e.sourceHandle === "slug" ? { ...e, sourceHandle: "key" } : e))
        .filter((e) => !(e.target === BOUNDARY_OUT && e.targetHandle === "receipt")),
    };
    const renames = boundaryRenames(canvas);
    expect([...renames.inputs]).toEqual([["slug", "key"]]);
    const frozen = freezeInner(canvas, ev);
    if (!frozen.ok) throw new Error(frozen.message);
    const done = replaceComposite(nodes, edges, publish.id, frozen, renames);
    const after = done.nodes.find((n) => n.id === publish.id)!;
    expect(after.data.body.kind === "Composite" && after.data.body.hash).toBe(frozen.hash);
    expect(after.data.inputs.map((p) => p.name)).toEqual(["key", "author"]);
    expect(done.edges.filter((e) => e.target === publish.id).map((e) => e.targetHandle).sort()).toEqual(["author", "key"]);
    // The wire from "receipt" is gone, and said so.
    expect(done.dropped.map((e) => e.sourceHandle)).toEqual(["receipt"]);
    // Settled: freezing it again unchanged is the same unit, with nothing renamed.
    const settled = settleBoundary(canvas, frozen);
    expect(boundaryRenames(settled).inputs.size).toBe(0);
    const again = freezeInner(settled, ev);
    expect(again.ok && again.hash).toBe(frozen.hash);
  });

  it("fold a changed level into the one above, deepest first", () => {
    const ev = new Evaluator();
    const doc = buildSample(samples().find((s) => s.name === "slugs")!, ev);
    const nodes = doc.nodes as PrimitiveNodeType[];
    const publish = nodes.find((n) => n.data.name === "publish")!;
    const opened = innerCanvas(publish, nodes, doc.edges as Edge[], ev);
    if (!opened.ok) throw new Error(opened.message);
    const top = { title: "publish", nodeId: publish.id, nodes: opened.nodes, edges: opened.edges, dirty: false };
    // A level below it: an inner canvas of its own (here, publish again under a made-up node id),
    // its fallback receipt changed.
    const inner = { ...top, nodeId: "deeper", dirty: true, nodes: top.nodes.map((n) => {
      const d = n.data as PrimitiveNodeType["data"];
      if (d.name !== "default" || !d.params?.some((p) => p.kind === "Frozen" && p.value === "unsigned")) return n;
      return { ...n, data: { ...d, params: d.params.map((p) => (p.name === "value" ? { ...p, value: "not signed" } : p)) } };
    }) };
    const host = { ...top, nodes: [...top.nodes, { ...publish, id: "deeper" }] };
    const folded = foldLevels([host, inner], 0, (c) => freezeInner(c, ev));
    if (typeof folded === "string") throw new Error(folded);
    const placed = folded[0].nodes.find((n) => n.id === "deeper") as PrimitiveNodeType;
    expect(folded[0].dirty).toBe(true);
    expect(placed.data.body.kind === "Composite" && placed.data.body.hash).not.toBe(publish.data.body.kind === "Composite" && publish.data.body.hash);
    // A level that can't be frozen says which and why.
    const broken = { ...inner, edges: inner.edges.filter((e) => e.target !== BOUNDARY_OUT) };
    expect(foldLevels([host, broken], 0, (c) => freezeInner(c, ev))).toContain(`publish: the output "put" isn't fed`);
  });

  it("carry the names of the holons in a flow, nested ones too, and teach them back on open", () => {
    const ev = new Evaluator();
    const { doc } = compileHolon(
      `flow nested
holon inner-holon(text) -> (out) {
  node up = uppercase
  in.text -> up.s
  up.out -> out.out
}
holon outer-holon(text) -> (out) {
  node shout = inner-holon
  in.text -> shout.text
  shout.out -> out.out
}
node outer = outer-holon
`,
      ev,
    );
    expect(Object.values(doc.names ?? {}).sort()).toEqual(["inner-holon", "outer-holon"]);
    const innerHash = Object.entries(doc.names!).find(([, n]) => n === "inner-holon")![0];
    expect(nameOf(innerHash), "not known before the flow is opened").toBeUndefined();
    expect(restoreDocument(doc, new Evaluator())).toBeNull();
    const outer = (doc.nodes as PrimitiveNodeType[])[0];
    const opened = innerCanvas(outer, doc.nodes as PrimitiveNodeType[], doc.edges as Edge[], ev, knownNames());
    if (!opened.ok) throw new Error(opened.message);
    expect(opened.nodes.map((n) => (n.data as { name: string }).name)).toContain("inner-holon");
    // And saved again with the flow.
    expect(documentUnits([innerHash], ev).names).toEqual({ [innerHash]: "inner-holon" });
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
    expect(derive("heartbeat").caps).toEqual([
      "Clock",
      'StateRead("beats")',
      'StateRead("first_beat")',
      'StateWrite("beats")',
      'StateWrite("first_beat")',
      'StateWrite("last_beat")',
    ]);
    expect(derive("approval").caps).toEqual(['Await("paid")', "NetworkIngress", 'StateWrite("last_order")']);
    expect(derive("fetch-json").caps).toEqual(['HttpRequest("httpbin.org")', "NetworkIngress"]);
    expect(derive("sign-payload").caps).toEqual(["NetworkIngress", 'SecretRead("signing_key")']);
    expect(derive("likes").caps).toEqual(['CollectionRead("likes")', 'CollectionWrite("likes")', "NetworkIngress"]);
    expect(derive("marketplace").caps).toEqual(["Clock", 'CollectionRead("listings")', 'CollectionWrite("listings")', "NetworkIngress", 'SecretRead("publish_key")']);
  });

  it("carry their text, whose names land on the canvas's nodes for a deploy", () => {
    const sample = samples().find((s) => s.name === "idempotent")!;
    const ev = new Evaluator();
    const doc = buildSample(sample, ev);
    // The text pane opens on the text (tabs.ts seeds it from `source`).
    expect(doc.source).toBe(sample.text);
    // A deploy names the nodes as the text does (useDeployFlow.ts), by the canvas's own ids.
    const bindings = bindingsOf(doc.source!, { nodes: doc.nodes as PrimitiveNodeType[], edges: doc.edges as Edge[] }, ev)!;
    const ids = new Set((doc.nodes as PrimitiveNodeType[]).map((n) => n.id));
    for (const id of Object.keys(bindings)) expect(ids.has(id)).toBe(true);
    expect(Object.values(bindings)).toEqual(expect.arrayContaining(["key", "fingerprint", "stored", "prior", "replayed", "mismatch", "processed", "fresh", "result"]));
    // A text that isn't this canvas names nothing, rather than something wrong.
    expect(bindingsOf("flow other\nport 1\n\nroute GET /a (req) {\n  x = req.body\n  respond x\n}\n", { nodes: doc.nodes as PrimitiveNodeType[], edges: doc.edges as Edge[] }, ev)).toBeNull();
    expect(bindingsOf("not a flow", { nodes: doc.nodes as PrimitiveNodeType[], edges: doc.edges as Edge[] }, ev)).toBeNull();
  });

  it("are holon-author's, built by name", () => {
    expect(() => buildSample({ ...samples()[0], name: "nope" }, new Evaluator())).toThrow('no sample "nope"');
  });
});
