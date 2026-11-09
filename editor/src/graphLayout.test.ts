import { describe, expect, it } from "vitest";
import { TIDY, tidy } from "./graphLayout";

const COLUMN = TIDY.nodeWidth + TIDY.columnGap;
const box = (h = 46) => ({ height: h });

describe("tidy", () => {
  it("places a chain left to right, one column a step", () => {
    const layout = tidy([box(), box(), box()], [{ from: 0, to: 1 }, { from: 1, to: 2 }]);
    const [a, b, c] = layout.positions;
    expect(b.x - a.x).toBe(COLUMN);
    expect(c.x - b.x).toBe(COLUMN);
    expect(layout.domains).toEqual([[0, 1, 2]]);
  });

  it("keeps a source next to what it feeds, not at the far left", () => {
    // request → field → as_text → concat ← const: const is wired to concat only.
    const nodes = [box(), box(), box(), box(), box()];
    const wires = [
      { from: 0, to: 1 },
      { from: 1, to: 2 },
      { from: 2, to: 3 },
      { from: 4, to: 3 },
    ];
    const layout = tidy(nodes, wires);
    expect(layout.positions[4].x).toBe(layout.positions[3].x - COLUMN);
  });

  it("never draws a wire backwards", () => {
    const nodes = [box(), box(), box(), box(), box(), box()];
    const wires = [
      { from: 0, to: 2 },
      { from: 1, to: 2 },
      { from: 2, to: 3 },
      { from: 0, to: 4 },
      { from: 4, to: 5 },
      { from: 3, to: 5 },
    ];
    const { positions } = tidy(nodes, wires);
    for (const w of wires) expect(positions[w.to].x).toBeGreaterThan(positions[w.from].x);
  });

  it("stacks separate handlers as separate domains that do not overlap", () => {
    const layout = tidy([box(), box(), box(), box()], [{ from: 0, to: 1 }, { from: 2, to: 3 }]);
    expect(layout.domains).toEqual([
      [0, 1],
      [2, 3],
    ]);
    const bottomOfFirst = Math.max(layout.positions[0].y, layout.positions[1].y) + 46;
    const topOfSecond = Math.min(layout.positions[2].y, layout.positions[3].y);
    expect(topOfSecond - bottomOfFirst).toBeGreaterThanOrEqual(TIDY.domainGap);
  });

  it("keeps the nodes of one column apart", () => {
    const layout = tidy([box(), box(), box()], [{ from: 0, to: 2 }, { from: 1, to: 2 }]);
    const [a, b] = [layout.positions[0], layout.positions[1]];
    expect(Math.abs(a.y - b.y)).toBeGreaterThanOrEqual(46 + TIDY.rowGap);
  });
});
