// The canvas's Grouped level: the nodes the core runs, laid out as the Nodes level's tidy lays them
// (canvasTidy.ts), each statement's nodes framed, and each trigger's statements framed round them.
// The tidy's columns stand: a node is as far right as its wiring puts it in its trigger, so a wire
// still runs left to right, and the frames are worn over that layout, not a layout of their own. A
// statement is a lane as wide as its nodes' columns; lanes go down the trigger in the text's order,
// each as high as the statements above it in the same columns let it, so two statements of
// different columns stand side by side. A node more than one statement shares (a field both an
// insert and an answer read) is in the first that has it; a node no statement has is in a frame of
// its own. It is a way of looking: the positions are computed here and drawn, never written to the
// flow.

import type { Edge } from "@xyflow/react";
import type { FlowGraph, GraphRow, RowKind } from "./holonText";
import { estimateHeight, tidyPositions, type TidyNode } from "./canvasTidy";
import { TIDY } from "./graphLayout";
import { KIND_WORD, rowKey } from "./StatementGraph";

/** A frame drawn behind the nodes: a trigger's, one statement's, or the nodes no statement has. */
export type Frame = {
  key: string;
  level: "band" | "row" | "loose";
  /** Its kind word and line ("reads or writes · line 7"), or the trigger's title. */
  label: string;
  /** The statement's text, its first line. */
  text: string;
  kind: RowKind | null;
  head: boolean;
  x: number;
  y: number;
  width: number;
  height: number;
};

export type Grouped = { positions: Map<string, { x: number; y: number }>; frames: Frame[]; byRow: Map<string, string[]> };

// Room inside a statement's frame, its header, between statements, and the trigger's frame round them.
const PAD = 14;
const HEAD = 30;
const GAP = 14;
const BAND_PAD = 16;
const BAND_HEAD = 34;
const BAND_GAP = 48;
const NODE_WIDTH = 232;
/** How much wider than tall the whole is packed to be, as a screen is. */
const ASPECT = 1.6;

const widthOf = (n: TidyNode) => n.measured?.width ?? n.width ?? NODE_WIDTH;
const heightOf = (n: TidyNode) => n.measured?.height ?? n.height ?? estimateHeight(n);

type Lane = { key: string; label: string; text: string; kind: RowKind | null; head: boolean; ids: string[] };
type Box = { x: number; y: number; width: number; height: number };
const overlapX = (a: Box, b: Box) => a.x < b.x + b.width && b.x < a.x + a.width;

/** A trigger's statements over the tidy of all their nodes, from (0, 0): each lane keeps its nodes'
 *  columns, its nodes of one column stacked in the tidy's order, and is put as high as the lanes
 *  above it in the text, where they share a column, let it. */
function lanes(rows: Lane[], byId: Map<string, TidyNode>, edges: Pick<Edge, "source" | "target">[], level: "row" | "loose") {
  const ids = rows.flatMap((r) => r.ids);
  const inside = new Set(ids);
  const at = tidyPositions(
    ids.map((id) => byId.get(id)!),
    edges.filter((e) => inside.has(e.source) && inside.has(e.target)),
  );
  const left = Math.min(...[...at.values()].map((p) => p.x));
  const frames: Frame[] = [];
  const positions = new Map<string, { x: number; y: number }>();
  for (const r of rows) {
    // Its nodes by column, each column top to bottom as the tidy has them.
    const columns = new Map<number, string[]>();
    for (const id of [...r.ids].sort((a, b) => at.get(a)!.y - at.get(b)!.y)) {
      const x = at.get(id)!.x - left;
      columns.set(x, [...(columns.get(x) ?? []), id]);
    }
    const inner = new Map<string, { x: number; y: number }>();
    let tall = 0;
    for (const [x, column] of columns) {
      let y = 0;
      for (const id of column) {
        inner.set(id, { x, y });
        y += heightOf(byId.get(id)!) + TIDY.rowGap;
      }
      tall = Math.max(tall, y - TIDY.rowGap);
    }
    const from = Math.min(...columns.keys());
    const to = Math.max(...r.ids.map((id) => inner.get(id)!.x + widthOf(byId.get(id)!)));
    const box: Box = { x: BAND_PAD + from, y: BAND_HEAD, width: to - from + 2 * PAD, height: HEAD + tall + PAD };
    // Below every lane already placed that shares a column with it.
    for (const f of frames) if (overlapX(f, box)) box.y = Math.max(box.y, f.y + f.height + GAP);
    frames.push({ key: r.key, level, label: r.label, text: r.text, kind: r.kind, head: r.head, ...box });
    for (const [id, p] of inner) positions.set(id, { x: BAND_PAD + PAD + p.x, y: box.y + HEAD + p.y });
  }
  return { frames, positions };
}

/** The flow's nodes laid out by its statements: `rows` are the canvas's nodes each row stands for,
 *  by rowKey (runRows.ts's rowsOnCanvas). Statements with no node on the canvas have no frame. */
export function groupByStatements(
  graph: FlowGraph,
  rows: Map<string, string[]>,
  nodes: TidyNode[],
  edges: Pick<Edge, "source" | "target">[],
): Grouped {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  const placed = new Set<string>();
  // Each statement's nodes: those it stands for that no earlier statement took.
  const groups: { band: number; title: string; rows: { key: string; row: GraphRow; ids: string[] }[] }[] = graph.bands.map((band, b) => ({
    band: b,
    title: band.title,
    rows: band.rows.flatMap((row, i) => {
      const ids = (rows.get(rowKey(b, i)) ?? []).filter((id) => byId.has(id) && !placed.has(id));
      ids.forEach((id) => placed.add(id));
      return ids.length > 0 ? [{ key: rowKey(b, i), row, ids }] : [];
    }),
  }));
  const loose = nodes.map((n) => n.id).filter((id) => !placed.has(id));

  // Each trigger's column, laid out from (0, 0): its statements stacked in the text's order.
  type Column = { frames: Frame[]; positions: Map<string, { x: number; y: number }>; width: number; height: number };
  const byRow = new Map<string, string[]>();
  const column = (key: string, title: string, rows: Lane[], level: "band" | "loose"): Column | null => {
    if (rows.length === 0) return null;
    const { frames, positions } = lanes(rows, byId, edges, level === "band" ? "row" : "loose");
    rows.forEach((r) => byRow.set(r.key, r.ids));
    const band = {
      width: Math.max(...frames.map((f) => f.x + f.width)) + BAND_PAD,
      height: Math.max(...frames.map((f) => f.y + f.height)) + BAND_PAD,
    };
    frames.unshift({ key, level: "band", label: title, text: "", kind: null, head: false, x: 0, y: 0, ...band });
    return { frames, positions, ...band };
  };
  const columns = [
    ...groups.map((g) =>
      column(
        `band ${g.band}`,
        g.title,
        g.rows.map(({ key, row, ids }) => ({
          key,
          label: row.head ? `on · line ${row.line}` : `${KIND_WORD[row.kind]} · line ${row.line}`,
          text: row.text,
          kind: row.kind,
          head: row.head,
          ids,
        })),
        "band",
      ),
    ),
    column("loose", "not in a statement", loose.length > 0 ? [{ key: "loose nodes", label: "the nodes no statement has", text: "", kind: null, head: false, ids: loose }] : [], "loose"),
  ].filter((c): c is Column => c !== null);

  // The columns on shelves, in the text's order, each shelf as wide as makes the whole about as
  // wide as a screen is to its height: a flow of many triggers wraps instead of running on in one
  // row too long to see whole.
  const area = columns.reduce((sum, c) => sum + (c.width + BAND_GAP) * (c.height + BAND_GAP), 0);
  const shelf = Math.max(...columns.map((c) => c.width), Math.sqrt(area * ASPECT));
  const positions = new Map<string, { x: number; y: number }>();
  const frames: Frame[] = [];
  let x = 0;
  let y = 0;
  let tallest = 0;
  for (const c of columns) {
    if (x > 0 && x + c.width > shelf) {
      x = 0;
      y += tallest + BAND_GAP;
      tallest = 0;
    }
    for (const f of c.frames) frames.push({ ...f, x: f.x + x, y: f.y + y });
    for (const [id, p] of c.positions) positions.set(id, { x: p.x + x, y: p.y + y });
    x += c.width + BAND_GAP;
    tallest = Math.max(tallest, c.height);
  }
  // Every trigger's frame under every statement's.
  frames.sort((a, b) => Number(b.level === "band") - Number(a.level === "band"));
  return { positions, frames, byRow };
}
