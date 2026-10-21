import { describe, it, expect } from "vitest";
import { closureOf, fromHex, toHex } from "./flowDoc";
import type { WUnit } from "./holon";

const prim = (name: string, params: WUnit["params"] = []): WUnit => ({
  body: { kind: "Primitive", name },
  params,
  inputs: [],
  outputs: [{ name: "out", ty: "Any" }],
});
const comp = (children: string[]): WUnit => ({
  body: { kind: "Composite", children, edges: [] },
  params: [],
  inputs: [],
  outputs: [],
});

describe("closureOf", () => {
  // Regression: a saved flow stored only each composite node's hash. The unit lived only in the
  // in-memory Evaluator, so after a reload project/group/expand/inspect all failed on it.
  it("lists every reachable unit once, children before parents, following fn params", () => {
    const units: Record<string, WUnit> = {
      add: prim("add"),
      double: prim("map", [{ kind: "Frozen", name: "fn", value: "add" }]),
      inner: comp(["add", "add"]),
      outer: comp(["inner", "double"]),
    };
    const order = closureOf(["outer", "inner"], (h) => units[h]);
    expect(new Set(order)).toEqual(new Set(["add", "double", "inner", "outer"]));
    expect(order).toHaveLength(4);
    for (const [parent, child] of [["outer", "inner"], ["outer", "double"], ["inner", "add"], ["double", "add"]]) {
      expect(order.indexOf(child)).toBeLessThan(order.indexOf(parent));
    }
  });

  it("propagates an unresolvable hash rather than silently skipping it", () => {
    expect(() =>
      closureOf(["gone"], () => {
        throw new Error("unresolved: gone");
      }),
    ).toThrow("unresolved");
  });
});

describe("hex", () => {
  it("round-trips bytes", () => {
    const bytes = new Uint8Array([0, 1, 15, 16, 127, 128, 255]);
    expect(toHex(bytes)).toBe("00010f107f80ff");
    expect(fromHex(toHex(bytes))).toEqual(bytes);
  });
  it("rejects malformed input", () => {
    expect(() => fromHex("abc")).toThrow();
    expect(() => fromHex("zz")).toThrow();
  });
});
