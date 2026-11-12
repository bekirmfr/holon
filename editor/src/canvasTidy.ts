// The canvas's tidy: the shared layout (graphLayout.ts) applied to a flow's nodes and edges, so the canvas
// draws a flow as the VS Code graph does. The positions are only computed here; the canvas sets them, so
// the change is one undoable step like any other move.

import type { Edge } from "@xyflow/react";
import { tidy, TIDY } from "./graphLayout";

/** What the layout reads of a canvas node: its id, its position, its ports and params (its data), and its
 *  size once the canvas has drawn it. */
export type TidyNode = {
  id: string;
  position: { x: number; y: number };
  data?: { inputs?: unknown[]; outputs?: unknown[]; params?: unknown[] };
  width?: number;
  height?: number;
  measured?: { width?: number; height?: number };
};

// The same rows as the VS Code graph draws (sidebar.js's graphModel): a header, one row per port or
// param, and a little room at the foot.
const HEAD = 30;
const ROW = 22;

/** A node's height from what it draws, for a node the canvas has not measured yet. */
export function estimateHeight(n: TidyNode): number {
  const inputs = n.data?.inputs?.length ?? 0;
  const outputs = n.data?.outputs?.length ?? 0;
  const params = n.data?.params?.length ?? 0;
  const rows = Math.max(inputs, outputs, 1);
  return HEAD + rows * ROW + (params ? 8 + params * ROW : 0) + 8;
}

/** Keeps the nodes of each column apart: a node that would overlap the one above it is pushed down, by
 *  the row gap. The columns and their order are the layout's; this only corrects heights it could not
 *  see exactly. */
export function separateColumns(
  positions: Map<string, { x: number; y: number }>,
  heights: Map<string, number>,
  gap: number = TIDY.rowGap,
): Map<string, { x: number; y: number }> {
  const columns = new Map<number, string[]>();
  for (const [id, p] of positions) {
    const column = columns.get(p.x) ?? [];
    column.push(id);
    columns.set(p.x, column);
  }
  const out = new Map(positions);
  for (const ids of columns.values()) {
    ids.sort((a, b) => out.get(a)!.y - out.get(b)!.y);
    for (let k = 1; k < ids.length; k++) {
      const above = out.get(ids[k - 1])!;
      const below = out.get(ids[k])!;
      const floor = above.y + (heights.get(ids[k - 1]) ?? 0) + gap;
      if (below.y < floor) out.set(ids[k], { x: below.x, y: floor });
    }
  }
  return out;
}

/** Where each node goes: the tidy layout of `nodes`, with each wire of `edges` between two of them. */
export function tidyPositions(nodes: TidyNode[], edges: Pick<Edge, "source" | "target">[]): Map<string, { x: number; y: number }> {
  const index = new Map(nodes.map((n, i) => [n.id, i]));
  const wires = edges.flatMap((e) => {
    const from = index.get(e.source);
    const to = index.get(e.target);
    return from === undefined || to === undefined ? [] : [{ from, to }];
  });
  const heights = new Map(nodes.map((n) => [n.id, n.measured?.height ?? n.height ?? estimateHeight(n)]));
  // Each node's drawn width, so each column is as wide as its own widest node (a node not drawn yet
  // takes the layout's width).
  const layout = tidy(
    nodes.map((n) => ({ height: heights.get(n.id)!, width: n.measured?.width ?? n.width })),
    wires,
    TIDY,
  );
  const placed = new Map(nodes.map((n, i) => [n.id, layout.positions[i]]));
  return separateColumns(placed, heights, TIDY.rowGap);
}
