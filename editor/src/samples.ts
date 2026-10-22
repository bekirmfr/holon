// Sample flows: one per capability, opened from the flows page as an unsaved copy. Each is built
// here from the live catalog (and the template interfaces), never from hand-copied node shapes,
// so a sample can't drift from core: samples.test.ts builds every one and derives it through the
// real core. Every new feature ships with a sample that shows it (CLAUDE.md).

import { buildPreimage, type Evaluator, type WParam, type WPort, type WType, type WUnit } from "./holon";
import type { FlowDocument } from "./server";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { isTemplate, templateDefaults, templateInterface } from "./templates";
import { groupSelection } from "./canvasOps";
import { closureOf, toHex } from "./flowDoc";

/** A flow of its own, grouped into one composite unit: placed as a node in another flow, it is a
 *  holon, a whole that is also a part. Its hash is the same wherever it's placed. */
export type Holon = { name: string; hash: string; inputs: WPort[]; outputs: WPort[] };

/** Places nodes and wires them. Every name is checked: an unknown leaf, param or port throws. */
export type SampleBuilder = {
  /** A leaf from the catalog or a template, with params set by name, at a canvas position. */
  node(name: string, params?: Record<string, string>, at?: [number, number]): string;
  wire(from: string, output: string, to: string, input: string): void;
  /** Build a flow of its own and group it into one composite, as the editor's "group" does: its
   *  free inputs and outputs become the composite's ports, named `ports` in boundary order. */
  holon(name: string, build: (b: SampleBuilder) => void, ports: { inputs: string[]; outputs: string[] }): Holon;
  /** Place a holon as a node, wired by its port names like any other. */
  place(holon: Holon, at?: [number, number]): string;
};

export type Sample = {
  /** The flow's name when opened (and its deployment's slug once deployed). */
  name: string;
  title: string;
  /** One line: what it demonstrates. */
  description: string;
  /** The features it shows, as the palette and docs name them. */
  shows: string[];
  /** Deploy settings: a server's port, or a clock flow's interval in seconds. */
  port?: string;
  interval?: string;
  /** Secrets to set under ⚙ before it works, what each is, and where to keep it when not a
   *  value sealed by the server (`env:HOLON_SECRET_…`, `vault:…`). */
  secrets?: { name: string; what: string; from?: string }[];
  /** Permissions it can run without (deploy settings' `optional`); the rest are required. */
  optional?: string[];
  /** How to try it once deployed. */
  tryIt: string;
  build(b: SampleBuilder): void;
};

/** A literal's wire type, as the editor sets it when a const's or default's type changes. */
const literalType = (ty: string): WType => (ty === "Json" ? "Any" : (ty as WType));

/** Lay out `sample` on a canvas: the flow document opening it loads. Its holons are grouped in
 *  `ev`, and their units (the whole closure) travel in the document, as a saved flow's do. */
export function buildSample(sample: Sample, catalog: Map<string, WUnit>, variadic: Set<string>, ev: Evaluator): FlowDocument {
  const nodes: PrimitiveNodeType[] = [];
  const edges: FlowDocument["edges"] = [];
  const counter = { next: 0 };
  const holons: string[] = [];
  const fail = (what: string): never => {
    throw new Error(`sample "${sample.name}": ${what}`);
  };
  const builder = (nodes: PrimitiveNodeType[], edges: FlowDocument["edges"], prefix: string): SampleBuilder => ({
    node(name, set = {}, at = [40 + 260 * (counter.next % 5), 40 + 160 * Math.floor(counter.next / 5)]) {
      const id = `${prefix}${name}-${counter.next++}`;
      const position = { x: at[0], y: at[1] };
      if (isTemplate(name)) {
        const defaults = templateDefaults(name);
        for (const k of Object.keys(set)) if (!defaults.some((p) => p.name === k)) fail(`${name} has no param "${k}"`);
        const params: WParam[] = defaults.map((p) => (p.name in set ? { kind: "Frozen", name: p.name, value: set[p.name] } : p));
        const iface = templateInterface(name, params);
        nodes.push({
          id,
          type: "primitive",
          position,
          data: { name, body: { kind: "Template" }, ...iface, params, variadic: false, minInputs: 0 },
        } as PrimitiveNodeType);
        return id;
      }
      const u = catalog.get(name) ?? fail(`no leaf "${name}" in the catalog`);
      for (const k of Object.keys(set)) if (!(u.params ?? []).some((p) => p.name === k)) fail(`${name} has no param "${k}"`);
      const params: WParam[] = (u.params ?? []).map((p) => (p.name in set ? { kind: "Frozen", name: p.name, value: set[p.name] } : p));
      let inputs: WPort[] = u.inputs ?? [];
      let outputs: WPort[] = u.outputs ?? [];
      if (name === "const" || name === "default") {
        const typeParam = params.find((p) => p.name === "type");
        const ty = literalType(typeParam?.kind === "Frozen" ? typeParam.value : "Number");
        inputs = inputs.map((p) => ({ ...p, ty }));
        outputs = outputs.map((p) => ({ ...p, ty }));
      }
      nodes.push({
        id,
        type: "primitive",
        position,
        data: { name, body: { kind: "Primitive" }, inputs, outputs, params, variadic: variadic.has(name), minInputs: inputs.length },
      } as PrimitiveNodeType);
      return id;
    },
    wire(from, output, to, input) {
      const src = nodes.find((n) => n.id === from) ?? fail(`no node ${from}`);
      const dst = nodes.find((n) => n.id === to) ?? fail(`no node ${to}`);
      if (!src.data.outputs.some((p) => p.name === output)) fail(`${src.data.name} has no output "${output}"`);
      if (!dst.data.inputs.some((p) => p.name === input)) fail(`${dst.data.name} has no input "${input}"`);
      edges.push({
        id: `xy-edge__${from}${output}-${to}${input}`,
        source: from,
        sourceHandle: output,
        target: to,
        targetHandle: input,
        style: { stroke: "#8295b0", strokeWidth: 1.6 },
      });
    },
    holon(name, build, ports) {
      const inner: PrimitiveNodeType[] = [];
      const innerEdges: FlowDocument["edges"] = [];
      build(builder(inner, innerEdges, `${name}.`));
      const grouped = groupSelection(inner, innerEdges as Parameters<typeof groupSelection>[1], ev, catalog, () => counter.next++);
      if (!grouped.ok) return fail(`holon "${name}" doesn't group: ${grouped.message}`);
      const { inputs, outputs, body } = grouped.node.data;
      const named = (got: WPort[], want: string[], side: string) =>
        got.length === want.length ? got.map((p, i) => ({ ...p, name: want[i] })) : fail(`holon "${name}" has ${side} ${got.map((p) => p.name).join(", ")}, not ${want.join(", ")}`);
      if (body.kind !== "Composite") return fail(`holon "${name}" isn't a composite`);
      holons.push(body.hash);
      return { name, hash: body.hash, inputs: named(inputs, ports.inputs, "inputs"), outputs: named(outputs, ports.outputs, "outputs") };
    },
    place(holon, at = [40 + 260 * (counter.next % 5), 40 + 160 * Math.floor(counter.next / 5)]) {
      const id = `${prefix}${holon.name}-${counter.next++}`;
      nodes.push({
        id,
        type: "primitive",
        position: { x: at[0], y: at[1] },
        data: { name: holon.name, body: { kind: "Composite", hash: holon.hash }, inputs: holon.inputs, outputs: holon.outputs, params: [], variadic: false, minInputs: 0 },
      } as PrimitiveNodeType);
      return id;
    },
  });
  sample.build(builder(nodes, edges, ""));
  // Zoomed so the whole sample shows at once, clear of the canvas's panels.
  const right = Math.max(...nodes.map((n) => n.position.x)) + 320;
  const bottom = Math.max(...nodes.map((n) => n.position.y)) + 220;
  const zoom = Math.min(0.8, 820 / right, 640 / bottom);
  return {
    nodes,
    edges,
    viewport: { x: 10, y: 50, zoom: Math.round(zoom * 100) / 100 },
    units: closureOf(holons, (h) => ev.get_unit(h) as WUnit).map((h) => toHex(buildPreimage(ev.get_unit(h) as WUnit))),
    server: {
      port: sample.port ?? "",
      interval: sample.interval ?? "",
      ...(sample.optional ? { requirements: { optional: sample.optional } } : {}),
    },
  };
}

export const SAMPLES: Sample[] = [
  {
    name: "hello-api",
    title: "Hello API",
    description: "A route with a path parameter, answered with text: the smallest server.",
    shows: ["request", "respond", "path params", "concat"],
    port: "8101",
    tryIt: "curl http://127.0.0.1:8101/hello/Ada",
    build(b) {
      const req = b.node("request", { method: "GET", path: "/hello/:name" }, [40, 40]);
      const who = b.node("field", { name: "name" }, [380, 60]);
      const hello = b.node("const", { value: "Hello, ", type: "Text" }, [380, 180]);
      const text = b.node("concat", {}, [640, 100]);
      const res = b.node("respond", { status: "200", missing: "404" }, [880, 60]);
      b.wire(req, "params", who, "record");
      b.wire(hello, "out", text, "a");
      b.wire(who, "out", text, "b");
      b.wire(req, "request", res, "request");
      b.wire(text, "out", res, "body");
    },
  },
  {
    name: "notes-api",
    title: "Notes API",
    description: "Create, read, list (a page at a time) and delete notes kept in a collection, keyed by id.",
    shows: ["collections (get, list, put, delete)", "paging with after", "JSON bodies", "404 and 204", "tracing a failed run"],
    port: "8102",
    tryIt:
      'curl -X PUT -H "Content-Type: application/json" -d \'{"text":"buy milk"}\' http://127.0.0.1:8102/notes/1, then GET /notes (?after=1 for the next page), GET /notes/1, DELETE /notes/1. GET /notes/9 fails (404): open the run in view mode to see where, and why',
    build(b) {
      const id = (req: string, x: number, y: number) => {
        const f = b.node("field", { name: "id" }, [x, y]);
        b.wire(req, "params", f, "record");
        return f;
      };

      // GET /notes: a page of notes in id order, from after ?after= when it's given.
      const list = b.node("request", { method: "GET", path: "/notes" }, [40, 40]);
      const after = b.node("field", { name: "after" }, [480, 60]);
      const from = b.node("default", { value: "", type: "Text" }, [720, 60]);
      const page = b.node("collection_list", { collection: "notes", limit: "50" }, [960, 60]);
      const listRes = b.node("respond", { status: "200", missing: "404" }, [1220, 40]);
      b.wire(list, "query", after, "record");
      b.wire(after, "out", from, "value");
      b.wire(from, "out", page, "after");
      b.wire(list, "request", listRes, "request");
      b.wire(page, "out", listRes, "body");

      // PUT /notes/:id: keep the body under the id, answer with it.
      const put = b.node("request", { method: "PUT", path: "/notes/:id" }, [40, 340]);
      const keep = b.node("collection_put", { collection: "notes" }, [960, 380]);
      const putRes = b.node("respond", { status: "200", missing: "400" }, [1220, 320]);
      b.wire(id(put, 480, 360), "out", keep, "key");
      b.wire(put, "json", keep, "value");
      b.wire(put, "request", putRes, "request");
      b.wire(put, "json", putRes, "body");

      // GET /notes/:id: one note, or 404.
      const get = b.node("request", { method: "GET", path: "/notes/:id" }, [40, 640]);
      const found = b.node("collection_get", { collection: "notes" }, [960, 680]);
      const getRes = b.node("respond", { status: "200", missing: "404" }, [1220, 640]);
      b.wire(id(get, 480, 660), "out", found, "key");
      b.wire(get, "request", getRes, "request");
      b.wire(found, "out", getRes, "body");

      // DELETE /notes/:id: drop it, answer 204.
      const del = b.node("request", { method: "DELETE", path: "/notes/:id" }, [40, 940]);
      const drop = b.node("collection_delete", { collection: "notes" }, [960, 980]);
      const delRes = b.node("respond", { status: "204" }, [1220, 940]);
      b.wire(id(del, 480, 960), "out", drop, "key");
      b.wire(del, "request", delRes, "request");
    },
  },
  {
    name: "stripe-to-slack",
    title: "Stripe → Slack",
    description: "Verify a Stripe webhook's signature and post the event to a Slack channel; anything unsigned gets 400 and posts nothing.",
    shows: ["stripe_verify", "secrets", "http_request", "clock", "hmac_sha256", "split"],
    port: "8103",
    secrets: [
      { name: "stripe_webhook_secret", what: "the endpoint's signing secret from Stripe (whsec_…)" },
      { name: "slack_webhook_path", what: "your Slack incoming webhook's path (/services/…)" },
    ],
    tryIt: "stripe listen --forward-to 127.0.0.1:8103/stripe, then stripe trigger payment_intent.succeeded",
    build(b) {
      const req = b.node("request", { method: "POST", path: "/stripe" }, [40, 40]);
      const signing = b.node("secret", { name: "stripe_webhook_secret" }, [40, 300]);
      const clock = b.node("clock", {}, [40, 400]);
      const verify = b.node("stripe_verify", { tolerance: "300" }, [380, 240]);
      const type = b.node("field", { name: "type" }, [680, 160]);
      const prefix = b.node("const", { value: "Stripe: ", type: "Text" }, [680, 40]);
      const text = b.node("concat", {}, [940, 80]);
      const empty = b.node("record", {}, [940, 220]);
      const message = b.node("with_field", { name: "text" }, [1180, 140]);
      const body = b.node("json_stringify", {}, [1420, 140]);
      const noHeaders = b.node("record", {}, [940, 340]);
      const jsonType = b.node("const", { value: "application/json", type: "Text" }, [940, 430]);
      const headers = b.node("with_field", { name: "content-type" }, [1180, 360]);
      const path = b.node("secret", { name: "slack_webhook_path" }, [1180, 520]);
      const post = b.node("http_request", { method: "POST", host: "hooks.slack.com" }, [1660, 300]);
      const status = b.node("field", { name: "status" }, [1920, 300]);
      const res = b.node("respond", { status: "200", missing: "400" }, [2160, 120]);
      b.wire(req, "request", verify, "request");
      b.wire(signing, "out", verify, "secret");
      b.wire(clock, "out", verify, "now");
      b.wire(verify, "event", type, "record");
      b.wire(prefix, "out", text, "a");
      b.wire(type, "out", text, "b");
      b.wire(empty, "out", message, "record");
      b.wire(text, "out", message, "value");
      b.wire(message, "out", body, "value");
      b.wire(noHeaders, "out", headers, "record");
      b.wire(jsonType, "out", headers, "value");
      b.wire(path, "out", post, "path");
      b.wire(headers, "out", post, "headers");
      b.wire(body, "out", post, "body");
      b.wire(post, "out", status, "record");
      b.wire(req, "request", res, "request");
      b.wire(status, "out", res, "body");
    },
  },
  {
    name: "guarded-api",
    title: "Token-guarded API",
    description:
      "Answer only callers holding the API token, with a receipt signed by a key from the server's environment: secrets sealed by the server or kept elsewhere, versioned, rolled back; the receipt key is an optional permission.",
    shows: ["secrets", "secret sources", "secret versions", "optional permissions", "secure_equals", "guard", "hmac_sha256", "default"],
    port: "8109",
    secrets: [
      { name: "api_token", what: "the bearer token callers send; set a new one to rotate it, roll back under ⚙ → ▸" },
      { name: "receipt_key", what: "the key receipts are signed with", from: "env:HOLON_SECRET_RECEIPT_KEY" },
    ],
    // Without the receipt key it still answers, the receipt "unsigned": revoking it doesn't pause it.
    optional: ['SecretRead("receipt_key")'],
    tryIt:
      "curl -H \"Authorization: Bearer <api_token>\" http://127.0.0.1:8109/me answers a receipt; without it, or after the token is rotated, 401; revoke receipt_key and it answers \"unsigned\"",
    build(b) {
      const req = b.node("request", { method: "GET", path: "/me" }, [40, 40]);
      const auth = b.node("field", { name: "authorization" }, [320, 40]);
      const bearer = b.node("const", { value: "Bearer ", type: "Text" }, [40, 300]);
      const token = b.node("secret", { name: "api_token" }, [40, 400]);
      const expected = b.node("concat", {}, [320, 320]);
      const match = b.node("secure_equals", {}, [580, 160]);
      const path = b.node("field", { name: "path" }, [320, 480]);
      const key = b.node("secret", { name: "receipt_key" }, [320, 600]);
      const receipt = b.node("hmac_sha256", { encoding: "hex" }, [580, 520]);
      const orUnsigned = b.node("default", { value: "unsigned", type: "Text" }, [840, 540]);
      const empty = b.node("record", {}, [580, 360]);
      const body = b.node("with_field", { name: "receipt" }, [840, 400]);
      const allowed = b.node("guard", {}, [1080, 220]);
      const res = b.node("respond", { status: "200", missing: "401" }, [1320, 40]);
      b.wire(req, "headers", auth, "record");
      b.wire(bearer, "out", expected, "a");
      b.wire(token, "out", expected, "b");
      b.wire(auth, "out", match, "a");
      b.wire(expected, "out", match, "b");
      b.wire(req, "request", path, "record");
      b.wire(key, "out", receipt, "key");
      b.wire(path, "out", receipt, "message");
      b.wire(empty, "out", body, "record");
      b.wire(receipt, "out", orUnsigned, "value");
      b.wire(orUnsigned, "out", body, "value");
      b.wire(match, "out", allowed, "cond");
      b.wire(body, "out", allowed, "value");
      b.wire(req, "request", res, "request");
      b.wire(allowed, "out", res, "body");
    },
  },
  {
    name: "slugs",
    title: "A flow inside a flow (holon)",
    description:
      "Two holons, each a whole flow placed as one node: slugify (lowercase, split, join with -) only computes, placed twice; publish reads and writes outside (the clock, a collection, state, an optional secret) and its manifest, permissions, replay and flow state count all of it. A saved flow without routes is placed the same way, from the palette's flows.",
    shows: ["holons", "group / expand", "saved flows as nodes", "composite reuse", "holons with effects", "optional permissions"],
    port: "8110",
    secrets: [{ name: "receipt_key", what: "signs each publish receipt; optional: without it receipts read \"unsigned\"" }],
    optional: ['SecretRead("receipt_key")'],
    tryIt:
      "curl -X POST -H \"Content-Type: application/json\" -d '{\"title\": \"Hello Holon World\", \"author\": \"Ada Lovelace\"}' http://127.0.0.1:8110/slugs answers the two slugs and a receipt; GET /slugs lists what was published; view a run to see the holons replayed and the post written, and expand a holon to see inside",
    build(b) {
      // A holon that only computes: one Text in, its slug out.
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
      // A holon that reads and writes outside: the post goes into the "posts" collection under its
      // slug, with its author and the time; the last slug into state; and a receipt, signed when
      // the receipt key is granted, "unsigned" when it isn't.
      const publish = b.holon(
        "publish",
        (inner) => {
          const slug = inner.node("default", { value: "", type: "Text" }, [0, 0]);
          const none = inner.node("record", {}, [0, 120]);
          const byAuthor = inner.node("with_field", { name: "author" }, [240, 120]);
          const clock = inner.node("clock", {}, [240, 260]);
          const post = inner.node("with_field", { name: "at" }, [480, 160]);
          const put = inner.node("collection_put", { collection: "posts" }, [720, 40]);
          const last = inner.node("store_write", { key: "last_post" }, [720, 180]);
          const key = inner.node("secret", { name: "receipt_key" }, [240, 380]);
          const sig = inner.node("hmac_sha256", { encoding: "hex" }, [480, 360]);
          const receipt = inner.node("default", { value: "unsigned", type: "Text" }, [720, 360]);
          inner.wire(none, "out", byAuthor, "record");
          inner.wire(byAuthor, "out", post, "record");
          inner.wire(clock, "out", post, "value");
          inner.wire(slug, "out", put, "key");
          inner.wire(post, "out", put, "value");
          inner.wire(slug, "out", last, "value");
          inner.wire(key, "out", sig, "key");
          inner.wire(slug, "out", sig, "message");
          inner.wire(sig, "out", receipt, "value");
        },
        { inputs: ["slug", "author"], outputs: ["put", "saved", "receipt"] },
      );
      // POST /slugs: slug the title and the author, publish the post, answer with both and the receipt.
      const req = b.node("request", { method: "POST", path: "/slugs" }, [40, 40]);
      const title = b.node("field", { name: "title" }, [380, 40]);
      const author = b.node("field", { name: "author" }, [380, 240]);
      const titleSlug = b.place(slugify, [640, 40]);
      const authorSlug = b.place(slugify, [640, 240]);
      const published = b.place(publish, [900, 380]);
      const empty = b.node("record", {}, [640, 560]);
      const withTitle = b.node("with_field", { name: "title" }, [900, 120]);
      const withAuthor = b.node("with_field", { name: "author" }, [1140, 200]);
      const withReceipt = b.node("with_field", { name: "receipt" }, [1380, 300]);
      const res = b.node("respond", { status: "200", missing: "400" }, [1620, 40]);
      b.wire(req, "json", title, "record");
      b.wire(req, "json", author, "record");
      b.wire(title, "out", titleSlug, "text");
      b.wire(author, "out", authorSlug, "text");
      b.wire(titleSlug, "slug", published, "slug");
      b.wire(authorSlug, "slug", published, "author");
      b.wire(empty, "out", withTitle, "record");
      b.wire(titleSlug, "slug", withTitle, "value");
      b.wire(withTitle, "out", withAuthor, "record");
      b.wire(authorSlug, "slug", withAuthor, "value");
      b.wire(withAuthor, "out", withReceipt, "record");
      b.wire(published, "receipt", withReceipt, "value");
      b.wire(req, "request", res, "request");
      b.wire(withReceipt, "out", res, "body");
      // GET /slugs: what was published, by slug.
      const list = b.node("request", { method: "GET", path: "/slugs" }, [40, 720]);
      const posts = b.node("collection_list", { collection: "posts", limit: "100" }, [640, 740]);
      const listRes = b.node("respond", { status: "200", missing: "404" }, [1620, 720]);
      b.wire(list, "request", listRes, "request");
      b.wire(posts, "out", listRes, "body");
    },
  },
  {
    name: "heartbeat",
    title: "Heartbeat",
    description: "A clock flow: every 10 seconds it records the time and counts the beats in its state.",
    shows: ["clock", "interval", "state across runs", "default", "add"],
    interval: "10",
    tryIt: "deploy it, wait, then look at its state on the deployed page (or GET /deployments/heartbeat/state)",
    build(b) {
      const clock = b.node("clock", {}, [40, 40]);
      const last = b.node("store_write", { key: "last_beat" }, [300, 40]);
      const read = b.node("store_read", { key: "beats" }, [40, 200]);
      const count = b.node("default", { value: "0", type: "Number" }, [260, 200]);
      const one = b.node("const", { value: "1", type: "Number" }, [260, 320]);
      const more = b.node("add", {}, [520, 240]);
      const save = b.node("store_write", { key: "beats" }, [760, 240]);
      b.wire(clock, "out", last, "value");
      b.wire(read, "out", count, "value");
      b.wire(count, "out", more, "a");
      b.wire(one, "out", more, "b");
      b.wire(more, "out", save, "value");
    },
  },
  {
    name: "approval",
    title: "Order approval (durable run)",
    description: "An order waits for payment: the run hands out a pay link, parks, and finishes when the link is opened, hours or days later.",
    shows: ["await", "resume_token", "resume routes", "durable runs"],
    port: "8105",
    tryIt:
      'curl -X POST -H "Content-Type: application/json" -d \'{"item":"book"}\' http://127.0.0.1:8105/orders (answers /paid/<token>), then open that path; the order is written to state "last_order"',
    build(b) {
      // POST /orders: answer with the pay link, then wait at "paid".
      const order = b.node("request", { method: "POST", path: "/orders" }, [40, 40]);
      const token = b.node("resume_token", { await: "paid" }, [380, 40]);
      const base = b.node("const", { value: "/paid/", type: "Text" }, [380, 140]);
      const link = b.node("concat", {}, [640, 60]);
      const res = b.node("respond", { status: "200", missing: "404" }, [880, 40]);
      const paid = b.node("await", { name: "paid", expires: "86400" }, [380, 300]);
      const via = b.node("field", { name: "method" }, [640, 300]);
      const done = b.node("with_field", { name: "paid_via" }, [880, 300]);
      const save = b.node("store_write", { key: "last_order" }, [1120, 300]);
      b.wire(base, "out", link, "a");
      b.wire(token, "out", link, "b");
      b.wire(order, "request", res, "request");
      b.wire(link, "out", res, "body");
      b.wire(paid, "out", via, "record");
      b.wire(order, "json", done, "record");
      b.wire(via, "out", done, "value");
      b.wire(done, "out", save, "value");

      // GET /paid/:token: delivers itself to the waiting run, and thanks the payer.
      const pay = b.node("request", { method: "GET", path: "/paid/:token", resume: "paid" }, [40, 520]);
      const thanks = b.node("const", { value: "thanks, the order is paid", type: "Text" }, [380, 560]);
      const payRes = b.node("respond", { status: "200", missing: "404" }, [640, 520]);
      b.wire(pay, "request", payRes, "request");
      b.wire(thanks, "out", payRes, "body");
    },
  },
  {
    name: "fetch-json",
    title: "Fetch and reshape",
    description: "Call another API, parse its JSON and answer with the one field you need.",
    shows: ["http_request", "json_parse", "field", "host permissions"],
    port: "8106",
    tryIt: "curl http://127.0.0.1:8106/slideshow (calls https://httpbin.org/json, answers its title)",
    build(b) {
      const req = b.node("request", { method: "GET", path: "/slideshow" }, [40, 40]);
      const path = b.node("const", { value: "/json", type: "Text" }, [40, 260]);
      const call = b.node("http_request", { method: "GET", host: "httpbin.org" }, [300, 240]);
      const body = b.node("field", { name: "body" }, [560, 240]);
      const parsed = b.node("json_parse", {}, [780, 240]);
      const show = b.node("field", { name: "slideshow" }, [1000, 240]);
      const title = b.node("field", { name: "title" }, [1220, 240]);
      const res = b.node("respond", { status: "200", missing: "404" }, [1460, 40]);
      b.wire(path, "out", call, "path");
      b.wire(call, "out", body, "record");
      b.wire(body, "out", parsed, "text");
      b.wire(parsed, "out", show, "record");
      b.wire(show, "out", title, "record");
      b.wire(req, "request", res, "request");
      b.wire(title, "out", res, "body");
    },
  },
  {
    name: "sign-payload",
    title: "Sign a payload",
    description: "Sign whatever is posted with a secret key (HMAC-SHA256) and answer the signature with the body in base64.",
    shows: ["hmac_sha256", "base64", "secrets", "record building"],
    port: "8107",
    secrets: [{ name: "signing_key", what: "any secret of 6 characters or more to sign with" }],
    tryIt: "curl -X POST -H \"Content-Type: application/json\" -d 'hello' http://127.0.0.1:8107/sign",
    build(b) {
      const req = b.node("request", { method: "POST", path: "/sign" }, [40, 40]);
      const key = b.node("secret", { name: "signing_key" }, [40, 300]);
      const sig = b.node("hmac_sha256", { encoding: "hex" }, [380, 200]);
      const b64 = b.node("base64", { mode: "encode", alphabet: "standard" }, [380, 360]);
      const empty = b.node("record", {}, [620, 80]);
      const withSig = b.node("with_field", { name: "signature" }, [860, 140]);
      const withBody = b.node("with_field", { name: "body_base64" }, [1100, 220]);
      const res = b.node("respond", { status: "200", missing: "400" }, [1340, 40]);
      b.wire(key, "out", sig, "key");
      b.wire(req, "body", sig, "message");
      b.wire(req, "body", b64, "text");
      b.wire(empty, "out", withSig, "record");
      b.wire(sig, "out", withSig, "value");
      b.wire(withSig, "out", withBody, "record");
      b.wire(b64, "out", withBody, "value");
      b.wire(req, "request", res, "request");
      b.wire(withBody, "out", res, "body");
    },
  },
  {
    name: "likes",
    title: "Likes counter (runs side by side)",
    description:
      "Each like reads a post's count from a collection, adds one and writes it back. Likes arriving at once run side by side; one whose count changed under it runs again, so none is lost.",
    shows: ["concurrent runs", "retries", "read-modify-write on a collection item", "collection_list"],
    port: "8108",
    tryIt:
      "seq 50 | xargs -P 20 -I{} curl -s -X POST http://127.0.0.1:8108/posts/1/like, then GET /posts/1/likes (50) and GET /likes; the log marks runs that ran again",
    build(b) {
      const id = (req: string, x: number, y: number) => {
        const f = b.node("field", { name: "id" }, [x, y]);
        b.wire(req, "params", f, "record");
        return f;
      };
      const countOf = (key: string, x: number, y: number) => {
        const got = b.node("collection_get", { collection: "likes" }, [x, y]);
        const count = b.node("default", { value: "0", type: "Number" }, [x + 260, y]);
        b.wire(key, "out", got, "key");
        b.wire(got, "out", count, "value");
        return count;
      };

      // POST /posts/:id/like: read the count, add one, write it back, answer the new count.
      const like = b.node("request", { method: "POST", path: "/posts/:id/like" }, [40, 40]);
      const post = id(like, 480, 60);
      const one = b.node("const", { value: "1", type: "Number" }, [740, 200]);
      const more = b.node("add", {}, [1240, 80]);
      const keep = b.node("collection_put", { collection: "likes" }, [1480, 140]);
      const likeRes = b.node("respond", { status: "200", missing: "400" }, [1480, 20]);
      b.wire(countOf(post, 740, 60), "out", more, "a");
      b.wire(one, "out", more, "b");
      b.wire(post, "out", keep, "key");
      b.wire(more, "out", keep, "value");
      b.wire(like, "request", likeRes, "request");
      b.wire(more, "out", likeRes, "body");

      // GET /posts/:id/likes: the count, 0 before the first like.
      const show = b.node("request", { method: "GET", path: "/posts/:id/likes" }, [40, 380]);
      const showRes = b.node("respond", { status: "200", missing: "404" }, [1240, 380]);
      b.wire(show, "request", showRes, "request");
      b.wire(countOf(id(show, 480, 400), 740, 400), "out", showRes, "body");

      // GET /likes: every post's count, in id order.
      const all = b.node("request", { method: "GET", path: "/likes" }, [40, 680]);
      const counts = b.node("collection_list", { collection: "likes", limit: "100" }, [740, 700]);
      const allRes = b.node("respond", { status: "200", missing: "404" }, [1240, 680]);
      b.wire(all, "request", allRes, "request");
      b.wire(counts, "out", allRes, "body");
    },
  },
];
