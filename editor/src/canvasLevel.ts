// The canvas's two levels (web-console-L4.md §5.2): Statements, the flow as its text says it (the
// statement graph, read only), and Nodes, the nodes the core runs (today's canvas, where they are
// placed and wired). A tab keeps its level, and a flow's is remembered in this browser by its name,
// so it opens again where it was left. A row of the statement graph stands for nodes: opening it
// frames them on the Nodes level, and what they may do is what its row may.

import type { Edge } from "@xyflow/react";
import { describeItem, manifestItems, type Evaluator } from "./holon";
import { compileHolon, type FlowGraph, type GraphRow } from "./holonText";
import type { PrimitiveNodeType } from "./PrimitiveNode";
import { rowKey } from "./StatementGraph";
import { sameGraph, type Graph } from "./textSync";

export type CanvasLevel = "statements" | "nodes";

/** The level a flow opens at, when this browser has none kept for it. */
export const DEFAULT_LEVEL: CanvasLevel = "statements";

const KEY = "holon.level";
/** How many flows' levels are kept: the newest. */
const KEPT = 100;

function kept(): Record<string, CanvasLevel> {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "{}") as unknown;
    return raw && typeof raw === "object" ? (raw as Record<string, CanvasLevel>) : {};
  } catch {
    return {};
  }
}

/** The level the flow `name` was left at in this browser, else the default. A draft never named
 *  opens at the default. */
export function levelOf(name: string | null): CanvasLevel {
  if (name === null) return DEFAULT_LEVEL;
  const l = kept()[name];
  return l === "statements" || l === "nodes" ? l : DEFAULT_LEVEL;
}

/** Keep the level the flow `name` is at, for when it opens again. */
export function keepLevel(name: string | null, level: CanvasLevel) {
  if (name === null) return;
  const all = kept();
  delete all[name];
  all[name] = level;
  const names = Object.keys(all);
  for (const n of names.slice(0, Math.max(0, names.length - KEPT))) delete all[n];
  try {
    localStorage.setItem(KEY, JSON.stringify(all));
  } catch {
    // Not kept, then.
  }
}

/** The line (1-based) of `offset` in `text`. */
export function lineAt(text: string, offset: number): number {
  let line = 1;
  const end = Math.min(offset, text.length);
  for (let i = 0; i < end; i++) if (text.charCodeAt(i) === 10) line++;
  return line;
}

/** A text compiled, to tell which nodes its rows stand for: its document, and each node's name in
 *  the text (else its id) and the line it is written on. Null when it doesn't compile. */
export type RowsCompiled = { nodes: PrimitiveNodeType[]; edges: Edge[]; nameOf: (id: string) => string; lineOf: (id: string) => number | null };

export function compileRows(text: string, ev: Evaluator): RowsCompiled | null {
  try {
    const c = compileHolon(text, ev);
    return {
      nodes: c.doc.nodes as PrimitiveNodeType[],
      edges: c.doc.edges as Edge[],
      nameOf: (id) => c.names.get(id) ?? id,
      lineOf: (id) => {
        const s = c.spans.get(id);
        return s ? lineAt(text, s.from) : null;
      },
    };
  } catch {
    return null;
  }
}

/** The nodes of the compiled text a row stands for, by id: each of its parts by name, the one
 *  written on the row's lines where several have it (each route's `req`). */
export function rowNodes(row: GraphRow, compiled: RowsCompiled): string[] {
  const ids: string[] = [];
  for (const part of row.parts) {
    const named = compiled.nodes.filter((n) => compiled.nameOf(n.id) === part.name).map((n) => n.id);
    const here = named.filter((id) => {
      const l = compiled.lineOf(id);
      return l !== null && row.line <= l && l <= row.last;
    });
    for (const id of here.length > 0 ? here : named.slice(0, 1)) if (!ids.includes(id)) ids.push(id);
  }
  return ids;
}

/** The canvas's nodes a row stands for: its nodes in the compiled text, matched to the canvas the
 *  text stands for (textSync.ts's sameGraph). None where the canvas is another flow. */
export function rowOnCanvas(row: GraphRow, compiled: RowsCompiled, canvas: Graph): string[] {
  const to = sameGraph({ nodes: compiled.nodes, edges: compiled.edges } as Graph, canvas);
  return rowNodes(row, compiled).flatMap((id) => to.get(id) ?? []);
}

/** What a row lets the flow do, as the manifest says it. */
export type RowMay = { item: string; said: string };

/** What each row lets the flow do, by rowKey: its nodes' capabilities as manifest items, in
 *  words. A node whose unit can't be had adds nothing. */
export function rowsMay(graph: FlowGraph, compiled: RowsCompiled, ev: Evaluator): Map<string, RowMay[]> {
  const byId = new Map(compiled.nodes.map((n) => [n.id, n]));
  const out = new Map<string, RowMay[]>();
  graph.bands.forEach((band, b) =>
    band.rows.forEach((row, i) => {
      const caps = new Set<string>();
      for (const id of rowNodes(row, compiled)) {
        const node = byId.get(id);
        if (!node) continue;
        try {
          for (const c of ev.capability_names(ev.unit_for(node)) as string[]) caps.add(c);
        } catch {
          // Its unit isn't here: nothing said of it.
        }
      }
      out.set(
        rowKey(b, i),
        manifestItems([...caps].sort()).map((item) => ({ item, said: describeItem(item) })),
      );
    }),
  );
  return out;
}
