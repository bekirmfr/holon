// The canvas's tidy: the shared layout (graphLayout.ts) applied to a flow's nodes and edges, so the canvas
// draws a flow as the VS Code graph does. The positions are only computed here; the canvas sets them, so
// the change is one undoable step like any other move.

import type { Edge } from "@xyflow/react";
import { tidy, TIDY } from "./graphLayout";

/** What the layout reads of a canvas node: its id, its position, and its size once it is drawn. */
export type TidyNode = {
  id: string;
  position: { x: number; y: number };
  width?: number;
  height?: number;
  measured?: { width?: number; height?: number };
};

/** A node's height when the canvas has not drawn it yet. */
const FALLBACK_HEIGHT = 120;

/** Where each node goes: the tidy layout of `nodes`, with each wire of `edges` between two of them. */
export function tidyPositions(nodes: TidyNode[], edges: Pick<Edge, "source" | "target">[]): Map<string, { x: number; y: number }> {
  const index = new Map(nodes.map((n, i) => [n.id, i]));
  const wires = edges.flatMap((e) => {
    const from = index.get(e.source);
    const to = index.get(e.target);
    return from === undefined || to === undefined ? [] : [{ from, to }];
  });
  const widest = nodes.reduce((w, n) => Math.max(w, n.measured?.width ?? n.width ?? 0), 0);
  const options = { ...TIDY, nodeWidth: widest > 0 ? widest : TIDY.nodeWidth };
  const layout = tidy(
    nodes.map((n) => ({ height: n.measured?.height ?? n.height ?? FALLBACK_HEIGHT })),
    wires,
    options,
  );
  return new Map(nodes.map((n, i) => [n.id, layout.positions[i]]));
}
