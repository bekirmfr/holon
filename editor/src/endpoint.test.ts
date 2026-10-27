// The data plane's request and response shapes, and routing, as the editor's test panel runs
// them. Route paths are parsed by core (the wasm package, loaded from disk). An endpoint's and a
// server's units are holon-author's, tested there (routes.rs).

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeAll, describe, it, expect } from "vitest";
import { initSync } from "../wasm/holon_wasm.js";
import {
  matchRoute,
  parseHeaders,
  parsePath,
  requestRecord,
  responseOf,
  toJson,
} from "./endpoint";
import type { WValue } from "./holon";

beforeAll(() => {
  initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });
});

describe("parsePath", () => {
  it("accepts what core accepts and rejects the rest", () => {
    expect(parsePath("/")).toEqual([]);
    expect(parsePath("/todos/:id")).toEqual([
      { literal: "todos", param: null },
      { literal: null, param: "id" },
    ]);
    for (const bad of ["todos", "/a//b", "/a/:", "/a/:x/:x", "/a b", "/a?b"]) {
      expect(parsePath(bad)).toBeNull();
    }
  });

  it("takes {name} as the same param as :name", () => {
    expect(parsePath("/todos/{title}")).toEqual(parsePath("/todos/:title"));
    for (const bad of ["/todos/{}", "/todos/{title", "/todos/title}", "/a/{x}/:x", "/a/{{x}}"]) {
      expect(parsePath(bad)).toBeNull();
    }
    const r = requestRecord("GET", "/todos/{title}", "/todos/buy%20milk", "");
    expect(r?.type === "Record" && r.value.find(([k]) => k === "params")?.[1]).toEqual({
      type: "Record",
      value: [["title", { type: "Text", value: "buy milk" }]],
    });
  });
});

describe("requestRecord", () => {
  it("matches the route and builds the record data.rs builds", () => {
    const r = requestRecord("GET", "/todos/:id", "/todos/a%20b?q=buy+milk&flag", "");
    expect(r).toEqual({
      type: "Record",
      value: [
        ["body", { type: "Text", value: "" }],
        ["headers", { type: "Record", value: [] }],
        ["method", { type: "Text", value: "GET" }],
        ["params", { type: "Record", value: [["id", { type: "Text", value: "a b" }]] }],
        ["path", { type: "Text", value: "/todos/a%20b" }],
        [
          "query",
          {
            type: "Record",
            value: [
              ["flag", { type: "Text", value: "" }],
              ["q", { type: "Text", value: "buy milk" }],
            ],
          },
        ],
      ],
    });
    const withHeaders = requestRecord("POST", "/todos", "/todos", "{}", [["X-Token", "abc"], ["Content-Type", "application/json"]]);
    expect(withHeaders?.type === "Record" && withHeaders.value.find(([k]) => k === "headers")?.[1]).toEqual({
      type: "Record",
      value: [
        ["content-type", { type: "Text", value: "application/json" }],
        ["x-token", { type: "Text", value: "abc" }],
      ],
    });
    expect(requestRecord("GET", "/todos/:id", "/todos", "")).toBeNull();
    expect(requestRecord("GET", "/todos/:id", "/todos/", "")).toBeNull();
    expect(requestRecord("GET", "/todos", "/other", "")).toBeNull();
  });
});

describe("responseOf", () => {
  const num = (value: number): WValue => ({ type: "Number", value });
  it("follows data.rs: envelope, plain value, effect, nothing", () => {
    const item: WValue = { type: "Record", value: [["id", num(1)]] };
    expect(responseOf(item)).toEqual({ status: 200, headers: [], body: item });
    expect(
      responseOf({
        type: "Record",
        value: [
          ["body", item],
          ["headers", { type: "Record", value: [["Location", { type: "Text", value: "/todos/1" }]] }],
          ["status", num(201)],
        ],
      }),
    ).toEqual({ status: 201, headers: [["location", "/todos/1"]], body: item });
    expect(responseOf({ type: "Effect", value: { sink: "store_write:todos", payload: item } })).toEqual({
      status: 204,
      headers: [],
      body: null,
    });
    expect(responseOf(null).status).toBe(500);
    expect(responseOf({ type: "Record", value: [["status", num(99)]] }).status).toBe(500);
    expect(responseOf({ type: "Record", value: [["status", num(101)]] }).status, "no interim status").toBe(500);
    const framed: WValue = {
      type: "Record",
      value: [
        ["Content-Length", { type: "Text", value: "0" }],
        ["connection", { type: "Text", value: "close" }],
        ["x-kept", { type: "Text", value: "yes" }],
      ],
    };
    expect(responseOf({ type: "Record", value: [["headers", framed], ["status", num(200)]] }).headers).toEqual([
      ["x-kept", "yes"],
    ]);
    expect(
      responseOf({ type: "Record", value: [["headers", item], ["status", num(200)]] }).status,
      "header values must be text",
    ).toBe(500);
  });
});

describe("matchRoute", () => {
  const e = (method: "GET" | "POST" | "DELETE", path: string) => ({ method, path, hash: `${method} ${path}` });
  const eps = [e("GET", "/todos"), e("GET", "/todos/:id"), e("GET", "/todos/new"), e("POST", "/todos"), e("DELETE", "/todos/:id")];
  it("routes the way core's match_route does, with 404 and 405", () => {
    expect(matchRoute(eps, "GET", "/todos/7?full=1")).toEqual({ endpoint: eps[1] });
    expect(matchRoute(eps, "GET", "/todos/new")).toEqual({ endpoint: eps[2] });
    expect(matchRoute(eps, "POST", "/todos")).toEqual({ endpoint: eps[3] });
    expect(matchRoute(eps, "PUT", "/todos/7")).toEqual({ status: 405, allow: ["GET", "DELETE"] });
    expect(matchRoute(eps, "GET", "/nope")).toEqual({ status: 404 });
    expect(matchRoute(eps, "GET", "/todos/")).toEqual({ status: 404 });
  });
});

describe("parseHeaders", () => {
  it("reads name: value lines, CRLF included, and rejects junk", () => {
    expect(parseHeaders("Content-Type: application/json\r\n\r\nX-Id:  7 ")).toEqual([
      ["Content-Type", "application/json"],
      ["X-Id", "7"],
    ]);
    expect(parseHeaders("")).toEqual([]);
    expect(parseHeaders("no colon here")).toMatch(/not a "name: value" header/);
  });
});

describe("toJson", () => {
  it("writes JSON the way core's json.rs does", () => {
    const v: WValue = {
      type: "Record",
      value: [
        ["title", { type: "Text", value: 'say "hi"' }],
        ["done", { type: "Boolean", value: false }],
        ["due", { type: "Sum", value: { arm: 1, payload: { type: "List", value: [] } } }],
        ["tags", { type: "List", value: [{ type: "Sum", value: { arm: 0, payload: { type: "Number", value: 2 } } }] }],
      ],
    };
    expect(toJson(v)).toBe('{"done":false,"due":null,"tags":[2],"title":"say \\"hi\\""}');
    expect(toJson({ type: "Number", value: NaN })).toBeNull();
    expect(toJson({ type: "Effect", value: { sink: "egress", payload: { type: "Number", value: 1 } } })).toBeNull();
  });
});
