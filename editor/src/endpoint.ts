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
 * query string), as data.rs builds it (header names lower-cased); `null` if the URL doesn't
 * match the route.
 */
export function requestRecord(
  method: string,
  declaredPath: string,
  url: string,
  body: string,
  headers: [string, string][] = [],
): WValue | null {
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
      ["headers", textRecord(headers.map(([k, v]) => [k.toLowerCase(), v]))],
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

/** How data.rs would answer a handler's output 0: the status, the envelope's headers (if any),
 *  and the body value. */
export function responseOf(output: WValue | null | undefined): {
  status: number;
  headers: [string, string][];
  body: WValue | null;
} {
  const fail = (value: string) => ({ status: 500, headers: [], body: { type: "Text", value } as WValue });
  if (!output) return fail("the handler produced no response");
  if (output.type === "Effect") return { status: 204, headers: [], body: null };
  if (output.type === "Record") {
    const field = (name: string) => output.value.find(([k]) => k === name)?.[1];
    const status = field("status");
    if (status && status.type === "Number") {
      const ok = Number.isInteger(status.value) && status.value >= 100 && status.value <= 599;
      if (!ok) return fail("the handler returned an invalid status");
      const h = field("headers");
      const headers: [string, string][] = [];
      if (h && h.type === "Record") {
        for (const [k, v] of h.value) {
          if (v.type !== "Text") return fail("response header values must be text");
          headers.push([k.toLowerCase(), v.value]);
        }
      } else if (h) return fail("response headers must be a record");
      return { status: status.value, headers, body: field("body") ?? null };
    }
  }
  return { status: 200, headers: [], body: output };
}

/** Which endpoint a request goes to, as core's `match_route` decides: same method and segment
 *  count, the most literal segments wins, ties go to the earlier endpoint. Otherwise 404, or 405
 *  (with the methods that would match) when the path exists under another method. */
export function matchRoute(
  endpoints: EndpointDraft[],
  method: string,
  url: string,
): { endpoint: EndpointDraft } | { status: 404 } | { status: 405; allow: string[] } {
  const path = url.split("?")[0];
  const score = (e: EndpointDraft): number | null => {
    const segs = parsePath(e.path);
    if (!segs || !path.startsWith("/")) return null;
    const parts = path.slice(1) === "" ? [] : path.slice(1).split("/");
    if (parts.length !== segs.length) return null;
    let literals = 0;
    for (let i = 0; i < parts.length; i++) {
      if (segs[i].literal !== null) {
        if (segs[i].literal !== parts[i]) return null;
        literals++;
      } else if (parts[i] === "") return null;
    }
    return literals;
  };
  let best: { endpoint: EndpointDraft; literals: number } | null = null;
  for (const e of endpoints) {
    if (e.method !== method) continue;
    const n = score(e);
    if (n !== null && (best === null || n > best.literals)) best = { endpoint: e, literals: n };
  }
  if (best) return { endpoint: best.endpoint };
  const allow = METHODS.filter((m) => m !== method && endpoints.some((e) => e.method === m && score(e) !== null));
  return allow.length > 0 ? { status: 405, allow } : { status: 404 };
}

/** Parse "name: value" lines (blank lines ignored) into header pairs. */
export function parseHeaders(text: string): [string, string][] | string {
  const out: [string, string][] = [];
  for (const line of text.split("\n")) {
    if (line.trim() === "") continue;
    const at = line.indexOf(":");
    if (at <= 0) return `"${line}" is not a "name: value" header`;
    out.push([line.slice(0, at).trim(), line.slice(at + 1).trim()]);
  }
  return out;
}

/** A value as JSON, the way core's json.rs writes it (keys in order, `some(x)` as x, `none()`
 *  as null); `null` for a value with no JSON form. One known difference: JS writes very large or
 *  small numbers in exponent form, Rust never does. */
export function toJson(v: WValue): string | null {
  switch (v.type) {
    case "Number":
      return Number.isFinite(v.value) ? String(v.value) : null;
    case "Boolean":
      return v.value ? "true" : "false";
    case "Text":
      return JSON.stringify(v.value);
    case "List": {
      const items = v.value.map(toJson);
      return items.includes(null) ? null : `[${items.join(",")}]`;
    }
    case "Record": {
      const sorted = [...v.value].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
      const parts = sorted.map(([k, x]) => {
        const j = toJson(x);
        return j === null ? null : `${JSON.stringify(k)}:${j}`;
      });
      return parts.includes(null) ? null : `{${parts.join(",")}}`;
    }
    case "Sum":
      if (v.value.arm === 0) return toJson(v.value.payload);
      if (v.value.arm === 1 && v.value.payload.type === "List" && v.value.payload.value.length === 0) return "null";
      return null;
    case "Effect":
      return null;
  }
}
