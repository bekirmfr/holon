// Building and testing HTTP endpoints in the editor (M1). Pure, like diagnostics.ts: only TYPES
// come from ./holon, so this is testable without the wasm package. The shapes mirror the Rust
// side exactly: holon-core's declare.rs (what an endpoint and a server are) and holon-server's
// data.rs (the request record a handler receives, and how its output becomes a response).

import type { WEdge, WPort, WUnit, WValue } from "./holon";

const BOUNDARY = 4294967295;

export const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"] as const;
export type Method = (typeof METHODS)[number];

/** One endpoint of the server being built: its route, and the endpoint unit's hash. */
export type EndpointDraft = { method: Method; path: string; hash: string };

/** Mirrors core's `parse_path`: `/`, or `/` then segments that are literals or `:name` params. */
export function parsePath(path: string): { literal: string | null; param: string | null }[] | null {
  if (!path.startsWith("/")) return null;
  const rest = path.slice(1);
  if (rest === "") return [];
  const word = /^[A-Za-z0-9\-_.~]+$/;
  const names = new Set<string>();
  const out: { literal: string | null; param: string | null }[] = [];
  for (const part of rest.split("/")) {
    if (part.startsWith(":")) {
      const name = part.slice(1);
      if (!word.test(name) || names.has(name)) return null;
      names.add(name);
      out.push({ literal: null, param: name });
    } else {
      if (!word.test(part)) return null;
      out.push({ literal: part, param: null });
    }
  }
  return out;
}

/**
 * Wrap a projected handler as an endpoint unit. The endpoint's one input is the request record,
 * wired to the handler's input if it has one (a handler with no input, e.g. a plain listing,
 * simply ignores the request). Output 0 is the response: the handler's first non-effect output,
 * or, if it only has effects, its first effect (which the data plane answers with 204). Every
 * other effect output follows, so the host performs it. Returns an error message instead when the
 * handler can't be an endpoint.
 */
export function endpointUnit(
  handlerHash: string,
  handlerInputs: WPort[],
  handlerOutputs: WPort[],
  method: Method,
  path: string,
): WUnit | string {
  if (parsePath(path) === null) {
    return `"${path}" is not a route path (use /todos or /todos/:id)`;
  }
  if (handlerInputs.length > 1) {
    return "a handler takes at most one input: the request record";
  }
  if (handlerOutputs.length === 0) return "a handler needs an output: the response";
  const isEffect = (p: WPort) => p.ty === "Effect";
  const firstData = handlerOutputs.findIndex((p) => !isEffect(p));
  const responseIndex = firstData >= 0 ? firstData : 0;
  const order = [
    responseIndex,
    ...handlerOutputs.map((_, i) => i).filter((i) => i !== responseIndex && isEffect(handlerOutputs[i])),
  ];
  const edges: WEdge[] = order.map((from, to) => ({ from_child: 0, from_port: from, to_child: BOUNDARY, to_port: to }));
  if (handlerInputs.length === 1) {
    edges.unshift({ from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 });
  }
  return {
    body: { kind: "Composite", children: [handlerHash], edges },
    params: [
      { kind: "Frozen", name: "declare", value: "endpoint" },
      { kind: "Frozen", name: "method", value: method },
      { kind: "Frozen", name: "path", value: path },
    ],
    inputs: [{ name: "request", ty: "Record" }],
    outputs: order.map((i, k) => ({ name: k === 0 ? "response" : handlerOutputs[i].name, ty: handlerOutputs[i].ty })),
  };
}

/** A server unit over the given endpoints. Core's `read_server` re-checks everything on deploy. */
export function serverUnit(endpointHashes: string[], port: number): WUnit {
  return {
    body: { kind: "Composite", children: endpointHashes, edges: [] },
    params: [
      { kind: "Frozen", name: "declare", value: "server" },
      { kind: "Frozen", name: "port", value: String(port) },
    ],
    inputs: [],
    outputs: [],
  };
}

const text = (value: string): WValue => ({ type: "Text", value });
const textRecord = (pairs: [string, string][]): WValue => ({
  type: "Record",
  value: [...pairs].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)).map(([k, v]) => [k, text(v)]),
});

/**
 * The request record a handler at `declaredPath` would receive for `url` (path plus optional
 * query string), as data.rs builds it; `null` if the URL doesn't match the route. For local
 * testing only, so headers are empty.
 */
export function requestRecord(method: string, declaredPath: string, url: string, body: string): WValue | null {
  const [path, query = ""] = url.split(/\?(.*)/s, 2);
  const segments = parsePath(declaredPath);
  if (!segments || !path.startsWith("/")) return null;
  const parts = path.slice(1) === "" ? [] : path.slice(1).split("/");
  if (parts.length !== segments.length) return null;
  const params: [string, string][] = [];
  for (let i = 0; i < parts.length; i++) {
    const seg = segments[i];
    if (seg.literal !== null && seg.literal !== parts[i]) return null;
    if (seg.param !== null) {
      if (parts[i] === "") return null;
      params.push([seg.param, decode(parts[i])]);
    }
  }
  const queryPairs: [string, string][] = query
    .split("&")
    .filter((kv) => kv !== "")
    .map((kv) => {
      const at = kv.indexOf("=");
      return at < 0 ? [decode(kv), ""] : [decode(kv.slice(0, at)), decode(kv.slice(at + 1))];
    });
  return {
    type: "Record",
    value: [
      ["body", text(body)],
      ["headers", textRecord([])],
      ["method", text(method)],
      ["params", textRecord(params)],
      ["path", text(path)],
      ["query", textRecord(queryPairs)],
    ],
  };
}

function decode(s: string): string {
  try {
    return decodeURIComponent(s.replace(/\+/g, " "));
  } catch {
    return s;
  }
}

/** How data.rs would answer a handler's output 0: the status, and the body value if any. */
export function responseOf(output: WValue | null | undefined): { status: number; body: WValue | null } {
  if (!output) return { status: 500, body: { type: "Text", value: "the handler produced no response" } };
  if (output.type === "Effect") return { status: 204, body: null };
  if (output.type === "Record") {
    const field = (name: string) => output.value.find(([k]) => k === name)?.[1];
    const status = field("status");
    if (status && status.type === "Number") {
      const ok = Number.isInteger(status.value) && status.value >= 100 && status.value <= 599;
      if (!ok) return { status: 500, body: { type: "Text", value: "the handler returned an invalid status" } };
      return { status: status.value, body: field("body") ?? null };
    }
  }
  return { status: 200, body: output };
}
