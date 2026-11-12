import { describe, expect, it } from "vitest";
import { estimateHeight, separateColumns, tidyPositions, type TidyNode } from "./canvasTidy";
import { TIDY } from "./graphLayout";

const node = (id: string, height = 46): TidyNode => ({ id, position: { x: 0, y: 0 }, measured: { width: 232, height } });

// A node as the canvas holds it before it is drawn: its ports and params, and no size.
const drawnLater = (id: string, outputs: number, params: number): TidyNode => ({
  id,
  position: { x: 0, y: 0 },
  data: {
    inputs: [{ name: "in" }],
    outputs: Array.from({ length: outputs }, (_, i) => ({ name: `o${i}` })),
    params: Array.from({ length: params }, (_, i) => ({ name: `p${i}` })),
  },
});

describe("tidyPositions", () => {
  it("places each node's wire target to its right", () => {
    const nodes = [node("request"), node("field"), node("respond")];
    const edges = [
      { source: "request", target: "field" },
      { source: "field", target: "respond" },
    ];
    const at = tidyPositions(nodes, edges);
    expect(at.get("field")!.x).toBeGreaterThan(at.get("request")!.x);
    expect(at.get("respond")!.x).toBeGreaterThan(at.get("field")!.x);
  });

  it("uses the drawn size of the nodes, and their wiring for the columns", () => {
    const drawn = { id: "a", position: { x: 0, y: 0 }, measured: { width: 300, height: 80 } };
    const notYet = { id: "b", position: { x: 0, y: 0 } };
    const at = tidyPositions([drawn, notYet], [{ source: "a", target: "b" }]);
    expect(at.get("b")!.x - at.get("a")!.x).toBe(300 + TIDY.columnGap);
  });

  it("spaces each column by its own widest node, not the widest on the canvas", () => {
    // request (wide) → field (narrow) → respond: the gap after field is field's width, not request's.
    const sized = (id: string, width: number) => ({ id, position: { x: 0, y: 0 }, measured: { width, height: 60 } });
    const at = tidyPositions(
      [sized("request", 370), sized("field", 70), sized("respond", 300)],
      [{ source: "request", target: "field" }, { source: "field", target: "respond" }],
    );
    expect(at.get("field")!.x - at.get("request")!.x).toBe(370 + TIDY.columnGap);
    expect(at.get("respond")!.x - at.get("field")!.x).toBe(70 + TIDY.columnGap);
  });

  it("ignores edges to nodes that are not on the canvas", () => {
    const at = tidyPositions([node("a")], [{ source: "a", target: "gone" }]);
    expect(at.has("a")).toBe(true);
    expect(at.size).toBe(1);
  });

  it("returns a position for every node", () => {
    const nodes = [node("a"), node("b"), node("c")];
    expect(tidyPositions(nodes, []).size).toBe(3);
  });

  it("keeps tall nodes in one column apart, drawn or not yet", () => {
    const tall = drawnLater("tall", 6, 4);
    const other = drawnLater("other", 4, 2);
    const sink = node("sink");
    const at = tidyPositions([tall, other, sink], [
      { source: "tall", target: "sink" },
      { source: "other", target: "sink" },
    ]);
    const a = at.get("tall")!;
    const b = at.get("other")!;
    expect(a.x).toBe(b.x);
    const [top, bottom, topHeight] = a.y < b.y ? [a, b, estimateHeight(tall)] : [b, a, estimateHeight(other)];
    expect(bottom.y - top.y).toBeGreaterThanOrEqual(topHeight + TIDY.rowGap);
  });

  it("estimates a tall node taller than a short one", () => {
    expect(estimateHeight(drawnLater("t", 6, 4))).toBeGreaterThan(estimateHeight(drawnLater("s", 1, 0)));
  });
});

describe("separateColumns", () => {
  it("pushes a node down that would overlap the one above it, and leaves the rest", () => {
    const placed = new Map([
      ["a", { x: 0, y: 0 }],
      ["b", { x: 0, y: 100 }],
      ["c", { x: 0, y: 1000 }],
      ["d", { x: 300, y: 50 }],
    ]);
    const heights = new Map([
      ["a", 200],
      ["b", 50],
      ["c", 50],
      ["d", 50],
    ]);
    const out = separateColumns(placed, heights, 20);
    expect(out.get("b")!.y).toBe(200 + 20);
    expect(out.get("c")!.y).toBe(1000);
    expect(out.get("d")!.y).toBe(50);
    expect(out.get("b")!.x).toBe(0);
  });
});
