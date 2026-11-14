// The tidy layout of a flow's graph, shared by the editor and the VS Code extension, so a flow is drawn
// the same wherever it is opened. Nodes are given by their heights and wires by the indices of their
// ends; the layout gives each node's position and the domains the nodes form.
//
// A domain is a handler's nodes: the nodes wired together. Each domain is laid out on its own, left to
// right: each node sits in the column of its depth (the longest chain of wires into it), as late as its
// consumers allow, so a branch stays next to what it feeds. A column is as wide as its widest node. Within a column the order comes from a few
// barycentre sweeps, and the heights from a few relaxations toward the nodes each one is wired to. The
// domains are stacked, each with room above its nodes for a label.

/** A node: its height, and its width when it has one of its own (else the options' `nodeWidth`). */
export type LayoutNode = { height: number; width?: number };
export type LayoutWire = { from: number; to: number };
export type LayoutOptions = {
  /** A node's width, for a node without one of its own. */
  nodeWidth: number;
  /** The gap between one column and the next, beyond the width of a node. */
  columnGap: number;
  /** The gap between two nodes in a column. */
  rowGap: number;
  /** The margin a domain keeps round its nodes. */
  pad: number;
  /** The room above a domain's nodes, for its label. */
  label: number;
  /** The gap between one domain and the next. */
  domainGap: number;
};
export type Layout = { positions: { x: number; y: number }[]; domains: number[][] };

export const TIDY: LayoutOptions = { nodeWidth: 232, columnGap: 96, rowGap: 28, pad: 24, label: 34, domainGap: 90 };

/** The layout of `nodes` and `wires`: where each node's top-left corner goes, and the domains (the
 *  indices of each domain's nodes, in the order of the nodes). */
export function tidy(nodes: LayoutNode[], wires: LayoutWire[], options: LayoutOptions = TIDY): Layout {
  const n = nodes.length;
  const parent = nodes.map((_, i) => i);
  const root = (a: number): number => {
    while (parent[a] !== a) a = parent[a];
    return a;
  };
  for (const w of wires) parent[root(w.from)] = root(w.to);
  const byRoot = new Map<number, number[]>();
  for (let i = 0; i < n; i++) {
    const r = root(i);
    let ids = byRoot.get(r);
    if (!ids) {
      ids = [];
      byRoot.set(r, ids);
    }
    ids.push(i);
  }

  const positions: { x: number; y: number }[] = nodes.map(() => ({ x: 0, y: 0 }));
  const domains: number[][] = [];
  let top = 0;
  for (const ids of byRoot.values()) {
    const lay = layoutDomain(
      ids.map((i) => nodes[i]),
      ids,
      wires,
      options,
    );
    const minY = Math.min(...lay.ys);
    const bottom = Math.max(...ids.map((id, k) => lay.ys[k] + nodes[id].height));
    // Each column is as wide as its widest node, then the gap: a narrow node isn't spaced as a wide one.
    const widths: number[] = [];
    ids.forEach((id, k) => (widths[lay.layer[k]] = Math.max(widths[lay.layer[k]] ?? 0, nodes[id].width ?? options.nodeWidth)));
    const lefts: number[] = [];
    for (let l = 0, x = options.pad; l < widths.length; l++) {
      lefts.push(x);
      x += (widths[l] ?? 0) + options.columnGap;
    }
    ids.forEach((id, k) => {
      positions[id] = {
        x: lefts[lay.layer[k]],
        y: top + options.label + lay.ys[k] - minY,
      };
    });
    domains.push(ids);
    top += options.label + (bottom - minY) + options.pad + options.domainGap;
  }
  return { positions, domains };
}

/** The layers and heights of one domain: `ids` are its nodes, in the order of `heights`. */
function layoutDomain(heights: LayoutNode[], ids: number[], wires: LayoutWire[], options: LayoutOptions): { layer: number[]; ys: number[] } {
  const g = ids.length;
  const hgt = heights.map((h) => h.height);
  const loc = new Map<number, number>(ids.map((id, k) => [id, k]));
  const preds: number[][] = ids.map(() => []);
  const succs: number[][] = ids.map(() => []);
  for (const w of wires) {
    const a = loc.get(w.from);
    const b = loc.get(w.to);
    if (w.from !== w.to && a !== undefined && b !== undefined) {
      succs[a].push(b);
      preds[b].push(a);
    }
  }

  // The layers: the longest chain into each node, then each node as late as its consumers allow.
  const layer: number[] = ids.map(() => 0);
  const indeg = preds.map((p) => p.length);
  const queue: number[] = [];
  indeg.forEach((d, i) => {
    if (d === 0) queue.push(i);
  });
  const topo: number[] = [];
  for (let q = 0; q < queue.length; q++) {
    const u = queue[q];
    topo.push(u);
    for (const s of succs[u]) {
      layer[s] = Math.max(layer[s], layer[u] + 1);
      indeg[s] -= 1;
      if (indeg[s] === 0) queue.push(s);
    }
  }
  for (let k = topo.length - 1; k >= 0; k--) {
    const v = topo[k];
    if (succs[v].length) layer[v] = Math.min(...succs[v].map((s) => layer[s])) - 1;
  }

  const depth = layer.reduce((a, c) => Math.max(a, c), 0);
  const rows: number[][] = Array.from({ length: depth + 1 }, () => []);
  layer.forEach((c, i) => rows[c].push(i));
  const pos: number[] = new Array(g).fill(0);
  rows.forEach((row) => row.forEach((x, j) => (pos[x] = j)));

  // The order in each column: each node by the average place of what it is wired to in the column beside.
  const order = (row: number[], neighbours: (x: number) => number[]) => {
    const key = new Map<number, number>();
    for (const x of row) {
      const around = neighbours(x);
      key.set(x, around.length ? around.reduce((a, b) => a + b, 0) / around.length : pos[x]);
    }
    row.sort((a, b) => (key.get(a)! - key.get(b)!) || (pos[a] - pos[b]));
    row.forEach((x, j) => (pos[x] = j));
  };
  for (let sweep = 0; sweep < 4; sweep++) {
    for (let l = 1; l <= depth; l++) order(rows[l], (x) => preds[x].map((p) => pos[p]));
    for (let l = depth - 1; l >= 0; l--) order(rows[l], (x) => succs[x].map((s) => pos[s]));
  }

  // The heights: a column is packed from its top and from its bottom, each keeping the gap; the average
  // of the two keeps the gap too, and sits as near the wanted tops as the column allows.
  const gap = options.rowGap;
  const pack = (row: number[], wanted: number[]): number[] => {
    const down: number[] = [];
    const up: number[] = new Array(row.length).fill(0);
    let end = -Infinity;
    row.forEach((x, j) => {
      const top = Math.max(wanted[j], end);
      down.push(top);
      end = top + hgt[x] + gap;
    });
    let start = Infinity;
    for (let j = row.length - 1; j >= 0; j--) {
      const bottom = Math.min(wanted[j], start - hgt[row[j]] - gap);
      up[j] = bottom;
      start = bottom;
    }
    return row.map((_, j) => (down[j] + up[j]) / 2);
  };
  const ys: number[] = new Array(g).fill(0);
  for (let l = 0; l <= depth; l++) {
    const placed = pack(
      rows[l],
      rows[l].map(() => 0),
    );
    rows[l].forEach((x, j) => (ys[x] = placed[j]));
  }
  const centre = (x: number) => ys[x] + hgt[x] / 2;
  for (let iter = 0; iter < 8; iter++) {
    for (let l = 0; l <= depth; l++) {
      const wanted = rows[l].map((x) => {
        const around = preds[x].concat(succs[x]);
        if (!around.length) return ys[x];
        return around.reduce((a, y) => a + centre(y), 0) / around.length - hgt[x] / 2;
      });
      const placed = pack(rows[l], wanted);
      rows[l].forEach((x, j) => (ys[x] = placed[j]));
    }
  }
  return { layer, ys };
}
