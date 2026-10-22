import { describe, it, expect } from "vitest";
import { components, findRoutes, routeSpec, type CanvasNode } from "./derive";
import { buildTemplate, templateDefaults, templateInterface } from "./templates";
import type { WParam, WUnit } from "./holon";

const BOUNDARY = 4294967295;
const frozen = (name: string, value: string): WParam => ({ kind: "Frozen", name, value });

function node(id: string, name: string, params: WParam[] = [], template = false): CanvasNode {
  const iface = template
    ? templateInterface(name as "request" | "respond", params)
    : { inputs: [{ name: "a", ty: "Any" as const }], outputs: [{ name: "out", ty: name === "store_write" ? ("Effect" as const) : ("Any" as const) }] };
  return { id, data: { name, body: { kind: template ? "Template" : "Primitive" }, params, ...iface } };
}
const req = (id: string, method: string, path: string) =>
  node(id, "request", [frozen("method", method), frozen("path", path)], true);
const res = (id: string, status = "200") => node(id, "respond", [frozen("status", status)], true);
const wire = (source: string, target: string) => ({ source, target });

describe("components", () => {
  it("groups nodes by wires in either direction, in canvas order", () => {
    const ns = [node("a", "x"), node("b", "x"), node("c", "x"), node("d", "x")];
    expect(components(ns, [wire("b", "a"), wire("c", "b")])).toEqual([["a", "b", "c"], ["d"]]);
  });
});

describe("findRoutes", () => {
  it("makes one route per group with a request node, skipping the rest", () => {
    const ns = [req("r1", "GET", "/todos"), res("s1"), req("r2", "POST", "/todos"), res("s2"), node("helper", "add")];
    const { routes, errors } = findRoutes(ns, [wire("r1", "s1"), wire("r2", "s2")]);
    expect(errors).toEqual([]);
    expect(routes.map((r) => [r.method, r.path, r.requestId, r.nodeIds])).toEqual([
      ["GET", "/todos", "r1", ["r1", "s1"]],
      ["POST", "/todos", "r2", ["r2", "s2"]],
    ]);
  });

  it("explains every group that can't be a route", () => {
    const { routes, errors } = findRoutes(
      [
        req("a", "GET", "/x"), req("b", "GET", "/y"), // wired together
        req("c", "FETCH", "/z"),
        req("d", "GET", "no-slash"),
        req("e", "GET", "/t/:id"), req("f", "GET", "/t/:key"), // clash
      ],
      [wire("a", "b")],
    );
    expect(routes.map((r) => r.path)).toEqual(["/t/:id"]);
    expect(errors).toEqual([
      "GET /x and GET /y are wired together: one request node per route",
      '"FETCH" is not a method (GET, POST, PUT, PATCH, DELETE)',
      '"no-slash" is not a route path (use /todos or /todos/:id)',
      "GET /t/:key clashes with GET /t/:id",
    ]);
  });
});

describe("routeSpec", () => {
  const group = [req("r", "POST", "/todos"), node("f", "field"), node("w", "store_write"), res("s")];
  const label = "POST /todos";
  it("keeps the response first and every effect, dropping other dangling outputs", () => {
    const spec = {
      promote: [{ child: 0, port: 0, name: "request" }],
      expose: [
        { child: 0, port: 2, name: "query" }, // unused request output
        { child: 2, port: 0, name: "write" },
        { child: 3, port: 0, name: "response" },
      ],
    };
    expect(routeSpec(spec, group, label)).toEqual({
      promote: spec.promote,
      expose: [
        { child: 3, port: 0, name: "response" },
        { child: 2, port: 0, name: "write" },
      ],
    });
  });
  it("refuses an unwired input, two responses, or nothing to answer with", () => {
    const base = { promote: [{ child: 0, port: 0, name: "request" }], expose: [] };
    expect(routeSpec({ ...base, promote: [...base.promote, { child: 1, port: 0, name: "a" }] }, group, label)).toBe(
      "POST /todos: field · a is not wired",
    );
    expect(
      routeSpec({ ...base, expose: [{ child: 3, port: 0, name: "r1" }, { child: 3, port: 0, name: "r2" }] }, group, label),
    ).toBe("POST /todos: more than one respond node");
    expect(routeSpec(base, group, label)).toBe("POST /todos: add a respond node (or an effect, answered 204)");
  });
  it("leaves an http_request's optional inputs unbound, but not its path", () => {
    const http: CanvasNode = {
      id: "h",
      data: {
        name: "http_request",
        body: { kind: "Primitive" },
        params: [],
        inputs: ["path", "headers", "body"].map((name) => ({ name, ty: "Any" as const })),
        outputs: [{ name: "out", ty: "Record" as const }],
      },
    };
    const g = [req("r", "GET", "/fetch"), http, res("s")];
    const spec = {
      promote: [
        { child: 0, port: 0, name: "request" },
        { child: 1, port: 1, name: "headers" },
        { child: 1, port: 2, name: "body" },
      ],
      expose: [{ child: 2, port: 0, name: "response" }],
    };
    expect(routeSpec(spec, g, "GET /fetch")).toEqual({ promote: [spec.promote[0]], expose: spec.expose });
    const unwiredPath = { ...spec, promote: [...spec.promote, { child: 1, port: 0, name: "path" }] };
    expect(routeSpec(unwiredPath, g, "GET /fetch")).toBe("GET /fetch: http_request · path is not wired");
  });
});

describe("buildTemplate", () => {
  const catalog = new Map<string, WUnit>(
    ["field", "json_parse", "record", "with_field", "const", "default"].map((n) => [
      n,
      { body: { kind: "Primitive", name: n }, params: [], inputs: [], outputs: [{ name: "out", ty: "Any" }] },
    ]),
  );
  const inserted: WUnit[] = [];
  const insert = (u: WUnit) => {
    inserted.push(u);
    return `h${inserted.length - 1}`;
  };

  it("builds request as field splits of the request record, the record itself passed through", () => {
    const u = buildTemplate("request", [frozen("method", "GET"), frozen("path", "/")], catalog, insert);
    if (typeof u === "string") throw new Error(u);
    expect(u.outputs?.map((p) => p.name)).toEqual(["request", "params", "query", "headers", "body", "json"]);
    expect(u.body.kind === "Composite" && u.body.edges).toContainEqual({ from_child: BOUNDARY, from_port: 0, to_child: BOUNDARY, to_port: 0 });
    expect(u.params).toEqual([]); // the route lives on the endpoint, not in the handler's identity
  });

  it("builds respond with and without a body, and refuses a bad status", () => {
    const withBody = buildTemplate("respond", [frozen("status", "201")], catalog, insert);
    if (typeof withBody === "string") throw new Error(withBody);
    expect(withBody.inputs?.map((p) => p.name)).toEqual(["request", "body"]);
    const noBody = buildTemplate("respond", [frozen("status", "204")], catalog, insert);
    if (typeof noBody === "string") throw new Error(noBody);
    expect(noBody.inputs?.map((p) => p.name)).toEqual(["request"]);
    expect(buildTemplate("respond", [frozen("status", "99")], catalog, insert)).toMatch(/not an HTTP status/);
    expect(buildTemplate("respond", [frozen("status", "2xx")], catalog, insert)).toMatch(/not an HTTP status/);
  });

  it("ends respond in a default that answers the missing status when the body is absent", () => {
    const named = (u: WUnit) => (u.body.kind === "Primitive" ? u.body.name : "");
    const before = inserted.length;
    const u = buildTemplate("respond", [frozen("status", "200"), frozen("missing", "404")], catalog, insert);
    if (typeof u === "string") throw new Error(u);
    const fallback = inserted.slice(before).find((c) => named(c) === "default");
    expect(fallback?.params).toEqual([
      frozen("value", JSON.stringify({ status: 404, body: { error: "not found" } })),
      frozen("type", "Json"),
    ]);
    // with_field(body) → default → the response; nothing else reaches the output.
    const edges = u.body.kind === "Composite" ? u.body.edges : [];
    expect(edges).toContainEqual({ from_child: 3, from_port: 0, to_child: 4, to_port: 0 });
    expect(edges.filter((x) => x.to_child === BOUNDARY)).toEqual([{ from_child: 4, from_port: 0, to_child: BOUNDARY, to_port: 0 }]);
    // Without the param (an older node), or for a 204 (no body to miss), no fallback.
    for (const params of [[frozen("status", "200")], [frozen("status", "204"), frozen("missing", "404")]]) {
      const at = inserted.length;
      expect(typeof buildTemplate("respond", params, catalog, insert)).not.toBe("string");
      expect(inserted.slice(at).some((c) => named(c) === "default")).toBe(false);
    }
    expect(buildTemplate("respond", [frozen("status", "200"), frozen("missing", "200")], catalog, insert)).toMatch(
      /not an error status/,
    );
    expect(templateDefaults("respond")).toEqual([frozen("status", "200"), frozen("missing", "404")]);
  });
});
