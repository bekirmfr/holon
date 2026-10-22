// Every sample flow builds from the live catalog and derives through the real core (the wasm
// package, loaded from disk): a server sample becomes a server unit with one endpoint per request
// node, a clock sample projects whole, and none asks for more than it uses (an unknown leaf would
// make core fail closed and ask for everything). A sample that stops building fails here, so the
// samples can't silently rot as core changes (CLAUDE.md).

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { initSync, Evaluator, primitive_catalog, variadic_primitives } from "../wasm/holon_wasm.js";
import { type WUnit } from "./holon";
import { deriveServer, flowUnitHash } from "./canvasOps";
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
  const doc = buildSample(sample, catalog, variadic);
  const nodes = doc.nodes as PrimitiveNodeType[];
  const edges = doc.edges as Edge[];
  const ev = new Evaluator();
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
      const doc = buildSample(s, catalog, variadic);
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
    expect(() => buildSample(broken, catalog, variadic)).toThrow('sample "hello-api": field has no output "nope"');
    expect(() => buildSample({ ...SAMPLES[0], build: (b) => void b.node("fieldd") }, catalog, variadic)).toThrow('no leaf "fieldd"');
    expect(() => buildSample({ ...SAMPLES[0], build: (b) => void b.node("field", { nam: "x" }) }, catalog, variadic)).toThrow('has no param "nam"');
  });
});
