import { describe, it, expect } from "vitest";
import { closureOf, documentKey, fromHex, openChoice, structuralKey, toHex } from "./flowDoc";
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

describe("structuralKey", () => {
  // Regression: a projection outlived the canvas it was built from (an edit, or New/Open of
  // another flow), so the deploy panel offered the old hash and clicking deploy shipped it.
  // The projection now records this key and is dropped when the canvas's key moves.
  const node = (id: string, value: string, x = 0) => ({
    id,
    position: { x, y: 0 },
    selected: false,
    data: { name: "const", params: [{ kind: "Frozen", name: "value", value }] },
  });
  const wire = (source: string, target: string) => ({ source, sourceHandle: "out", target, targetHandle: "a" });
  const base = structuralKey([node("c1", "3"), node("c2", "4")], [wire("c1", "c2")]);

  it("ignores layout, selection and ordering", () => {
    const moved = { ...node("c1", "3", 500), selected: true };
    expect(structuralKey([node("c2", "4"), moved], [wire("c1", "c2")])).toBe(base);
  });
  it("changes with a param, a node, or a wire", () => {
    expect(structuralKey([node("c1", "5"), node("c2", "4")], [wire("c1", "c2")])).not.toBe(base);
    expect(structuralKey([node("c1", "3")], [])).not.toBe(base);
    expect(structuralKey([node("c1", "3"), node("c2", "4")], [])).not.toBe(base);
  });
});

describe("documentKey", () => {
  // Backs the unsaved-changes prompt: New/Open used to replace an unsaved canvas silently.
  const node = (x: number, extra: object = {}) => ({
    id: "c1",
    position: { x, y: 0 },
    data: { name: "const" },
    ...extra,
  });
  it("counts a move as unsaved, but not selection or React Flow's measurements", () => {
    const saved = documentKey([node(0)], []);
    expect(documentKey([node(10)], [])).not.toBe(saved);
    expect(documentKey([node(0, { selected: true, dragging: false, measured: { width: 160, height: 80 } })], [])).toBe(
      saved,
    );
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

describe("openChoice", () => {
  it("goes back to the open flow's unsaved changes rather than discarding them", () => {
    expect(openChoice("todo", { name: "todo", dirty: true })).toBe("resume");
    expect(openChoice("other", { name: "todo", dirty: true })).toBe("confirm");
    expect(openChoice("todo", { name: null, dirty: true })).toBe("confirm");
    expect(openChoice("todo", { name: "todo", dirty: false })).toBe("load");
    expect(openChoice("other", { name: "todo", dirty: false })).toBe("load");
  });
});
