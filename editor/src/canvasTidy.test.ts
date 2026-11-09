import { describe, expect, it } from "vitest";
import { tidyPositions, type TidyNode } from "./canvasTidy";
import { TIDY } from "./graphLayout";

const node = (id: string, height = 46): TidyNode => ({ id, position: { x: 0, y: 0 }, measured: { width: 232, height } });

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

  it("uses the drawn size of the nodes, falling back when a node is not drawn yet", () => {
    const drawn = { id: "a", position: { x: 0, y: 0 }, measured: { width: 300, height: 80 } };
    const notYet = { id: "b", position: { x: 0, y: 0 } };
    const at = tidyPositions([drawn, notYet], [{ source: "a", target: "b" }]);
    expect(at.get("b")!.x - at.get("a")!.x).toBe(300 + TIDY.columnGap);
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
});
