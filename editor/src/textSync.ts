// The text pane and the canvas as one flow (docs/L4s/text-pane-L4.md). A text compiles to fresh
// node ids at the builder's places, so each node is matched to the canvas's: by what it is in the
// text (its name, or where it is written: the input of the node it feeds), so an edit keeps every
// node it didn't touch where it was, with its id (selection, the Inspector and undo hold ids).
// Positions are not part of a program: they live on the canvas only.

import type { Edge } from "@xyflow/react";
import type { PrimitiveNodeType } from "./PrimitiveNode";

export type Graph = { nodes: PrimitiveNodeType[]; edges: Edge[] };

/** A text compiled, and which canvas node each of its nodes is. */
export type Synced = { graph: Graph; names: Map<string, string>; toCanvas: Map<string, string> };

/** JSON with each object's keys in order: the same value says the same, however it was built. */
const stable = (v: unknown): string =>
  Array.isArray(v) ? `[${v.map(stable).join(",")}]` : v !== null && typeof v === "object" ? `{${Object.keys(v).sort().map((k) => `${JSON.stringify(k)}:${stable((v as Record<string, unknown>)[k])}`).join(",")}}` : JSON.stringify(v);

/** What a node is, apart from its wiring, its params and its place. */
const kindOf = (n: PrimitiveNodeType) => stable([n.data.name, n.data.body]);
const paramsOf = (n: PrimitiveNodeType) => stable(n.data.params ?? []);
const param = (n: PrimitiveNodeType, name: string) => {
  const p = n.data.params?.find((q) => q.name === name);
  return p && p.kind === "Frozen" ? p.value : "";
};

/**
 * Which node of `b` each node of `a` is, for two documents of one graph (a canvas, and its text
 * compiled back): nodes are told apart by what they are and, round by round, by what they are
 * wired to and by which ports (colour refinement); nodes still alike are paired by their params,
 * then in order. A node of `a` with no like node in `b` is left out.
 */
export function sameGraph(a: Graph, b: Graph): Map<string, string> {
  const codes = new Map<string, number>();
  const code = (s: string) => {
    let c = codes.get(s);
    if (c === undefined) codes.set(s, (c = codes.size));
    return c;
  };
  const colour = (g: Graph) => new Map(g.nodes.map((n) => [n.id, code(kindOf(n))]));
  let ca = colour(a);
  let cb = colour(b);
  const refine = (g: Graph, c: Map<string, number>) => {
    const ins = new Map<string, string[]>();
    const outs = new Map<string, string[]>();
    for (const e of g.edges) {
      ins.set(e.target, [...(ins.get(e.target) ?? []), `${e.targetHandle}<${e.sourceHandle}:${c.get(e.source)}`]);
      outs.set(e.source, [...(outs.get(e.source) ?? []), `${e.sourceHandle}>${e.targetHandle}:${c.get(e.target)}`]);
    }
    return new Map(g.nodes.map((n) => [n.id, code(JSON.stringify([c.get(n.id), (ins.get(n.id) ?? []).sort(), (outs.get(n.id) ?? []).sort()]))]));
  };
  const classes = (c: Map<string, number>) => new Set(c.values()).size;
  for (let round = 0; round < a.nodes.length + 1; round++) {
    const na = refine(a, ca);
    const nb = refine(b, cb);
    const settled = classes(na) === classes(ca) && classes(nb) === classes(cb);
    ca = na;
    cb = nb;
    if (settled) break;
  }
  const out = new Map<string, string>();
  const taken = new Set<string>();
  for (const pass of [true, false]) {
    for (const n of a.nodes) {
      if (out.has(n.id)) continue;
      const like = b.nodes.find((m) => !taken.has(m.id) && cb.get(m.id) === ca.get(n.id) && (!pass || paramsOf(m) === paramsOf(n)));
      if (like) {
        out.set(n.id, like.id);
        taken.add(like.id);
      }
    }
  }
  return out;
}

/**
 * Each node's key in a compiled text: what it is called (`=name`), a route's request by its
 * route, a node that feeds others by where it is written (`<consumer's key>.<input>`), a node
 * that feeds nothing by what it is and the first named thing it reads. Unique within the text.
 */
export function textKeys(g: Graph, names: Map<string, string>): Map<string, string> {
  const keys = new Map<string, string>();
  const used = new Map<string, number>();
  const unique = (k: string) => {
    const n = used.get(k) ?? 0;
    used.set(k, n + 1);
    return n === 0 ? k : `${k}~${n}`;
  };
  const byId = new Map(g.nodes.map((n) => [n.id, n]));
  const feeds = (id: string) => g.edges.filter((e) => e.source === id);
  // Anchors: a route's request, then names.
  for (const n of g.nodes) {
    if (n.data.name === "request" && n.data.body.kind !== "Composite") keys.set(n.id, unique(`route ${param(n, "method")} ${param(n, "path")}`));
    else if (names.has(n.id)) keys.set(n.id, unique(`=${names.get(n.id)}`));
  }
  // What feeds nothing: by what it is and the first anchored thing it reads.
  for (const n of g.nodes) {
    if (keys.has(n.id) || feeds(n.id).length > 0) continue;
    const read = g.edges.filter((e) => e.target === n.id && keys.has(e.source)).sort((x, y) => String(x.targetHandle).localeCompare(String(y.targetHandle)))[0];
    keys.set(n.id, unique(read ? `${n.data.name}<${keys.get(read.source)}.${read.targetHandle}` : n.data.name));
  }
  // The rest: where it is written, the input of the first node it feeds.
  const keyOf = (id: string, seen: Set<string>): string => {
    const known = keys.get(id);
    if (known !== undefined) return known;
    const first = feeds(id).find((e) => !seen.has(e.target));
    const k = unique(first ? `${keyOf(first.target, new Set([...seen, id]))}.${first.targetHandle}` : (byId.get(id)?.data.name ?? id));
    keys.set(id, k);
    return k;
  };
  for (const n of g.nodes) keyOf(n.id, new Set([n.id]));
  return keys;
}

/** The text the canvas was printed as, compiled: `toCanvas` from the two being one graph. */
export function syncedFromCanvas(canvas: Graph, compiled: Graph, names: Map<string, string>): Synced {
  return { graph: compiled, names, toCanvas: sameGraph(compiled, canvas) };
}

const BESIDE = { x: 240, y: 110 };

/**
 * The canvas a text stands for (`next`, compiled), after the text `prev` stood for `canvas`:
 * a node `prev` and `next` share by key keeps its canvas id and place (and what the canvas
 * knows of it beyond the text, such as the saved flow it was placed from); a new node gets an id
 * of `freshId` and a place beside the first node it feeds, else beside the first it reads, else
 * the builder's.
 */
export function carryOver(prev: Synced, next: { graph: Graph; names: Map<string, string> }, canvas: PrimitiveNodeType[], freshId: (base: string) => string): Graph & { toCanvas: Map<string, string> } {
  const before = new Map([...textKeys(prev.graph, prev.names)].map(([id, k]) => [k, id]));
  const keys = textKeys(next.graph, next.names);
  const onCanvas = new Map(canvas.map((n) => [n.id, n]));
  const toCanvas = new Map<string, string>();
  const placed = new Map<string, { x: number; y: number }>();
  const nodes = new Map<string, PrimitiveNodeType>();
  for (const n of next.graph.nodes) {
    const was = before.get(keys.get(n.id) ?? "");
    const old = was === undefined ? undefined : onCanvas.get(prev.toCanvas.get(was) ?? "");
    const free = old !== undefined && ![...toCanvas.values()].includes(old.id);
    // The same node only if it is the same kind; another kind written in its place takes its
    // place, under an id of its own.
    const same = free && kindOf(old) === kindOf(n);
    const id = same ? old.id : freshId(n.data.name);
    toCanvas.set(n.id, id);
    const source = same && old.data.source ? { source: old.data.source } : {};
    if (free) placed.set(id, old.position);
    nodes.set(n.id, { ...n, id, data: { ...n.data, ...source } });
  }
  // New nodes beside what they feed or read, as those get places.
  const beside = new Map<string, number>();
  for (let pass = 0; pass < next.graph.nodes.length && placed.size < nodes.size; pass++) {
    for (const n of next.graph.nodes) {
      const id = toCanvas.get(n.id)!;
      if (placed.has(id)) continue;
      const fed = next.graph.edges.find((e) => e.source === n.id && placed.has(toCanvas.get(e.target)!));
      const read = fed ? undefined : next.graph.edges.find((e) => e.target === n.id && placed.has(toCanvas.get(e.source)!));
      const anchor = fed ? toCanvas.get(fed.target)! : read ? toCanvas.get(read.source)! : undefined;
      if (!anchor) continue;
      const k = beside.get(anchor) ?? 0;
      beside.set(anchor, k + 1);
      const at = placed.get(anchor)!;
      placed.set(id, { x: at.x + (fed ? -BESIDE.x : BESIDE.x), y: at.y + k * BESIDE.y });
    }
  }
  const out = next.graph.nodes.map((n) => {
    const node = nodes.get(n.id)!;
    return { ...node, position: placed.get(node.id) ?? n.position };
  });
  const edges = next.graph.edges.map((e) => {
    const source = toCanvas.get(e.source) ?? e.source;
    const target = toCanvas.get(e.target) ?? e.target;
    return { ...e, id: `${source}.${e.sourceHandle}-${target}.${e.targetHandle}`, source, target };
  });
  return { nodes: out, edges, toCanvas };
}

/** How many `#` comments a text has (outside strings): what a reprint would drop. */
export function commentCount(text: string): number {
  let count = 0;
  for (const line of text.split("\n")) {
    let quoted = false;
    for (let i = 0; i < line.length; i++) {
      const c = line[i];
      if (c === "\\" && quoted) i++;
      else if (c === '"') quoted = !quoted;
      else if (c === "#" && !quoted) {
        count++;
        break;
      }
    }
  }
  return count;
}
