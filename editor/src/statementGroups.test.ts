import { describe, expect, it } from "vitest";
import type { FlowGraph, GraphRow, RowKind } from "./holonText";
import type { TidyNode } from "./canvasTidy";
import { rowKey } from "./StatementGraph";
import { groupByStatements, type Frame } from "./statementGroups";

const KINDS: RowKind[] = ["compute", "data", "outside", "answer", "waits"];

// A seeded generator, so a failing layout is the same layout on every run.
function random(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

const row = (line: number, kind: RowKind, head: boolean): GraphRow =>
  ({ line, last: line, name: `r${line}`, does: "", text: `line ${line}`, kind, head, effect: false, fallback: null, from: [], outside: [], parts: [] }) as unknown as GraphRow;

/** A flow of a few triggers, each a few statements of a few wired nodes, some nodes shared by two
 *  statements and some in none: what the canvas hands the grouping. */
function flow(seed: number) {
  const next = random(seed);
  const pick = (n: number) => Math.floor(next() * n);
  const bands: FlowGraph["bands"] = [];
  const rows = new Map<string, string[]>();
  const nodes: TidyNode[] = [];
  const edges: { source: string; target: string }[] = [];
  let line = 1;
  const add = () => {
    const id = `n${nodes.length}`;
    nodes.push({ id, position: { x: 0, y: 0 }, measured: { width: 232, height: 46 + 22 * pick(5) } });
    return id;
  };
  for (let b = 0, bs = 1 + pick(9); b < bs; b++) {
    const band: GraphRow[] = [];
    const earlier: string[] = [];
    for (let i = 0, rs = 1 + pick(6); i < rs; i++) {
      band.push(row(line++, KINDS[pick(KINDS.length)], i === 0));
      const ids = Array.from({ length: 1 + pick(7) }, add);
      ids.slice(1).forEach((id, k) => edges.push({ source: ids[pick(k + 1)], target: id }));
      // Now and then a statement reads one before it in its trigger.
      if (earlier.length > 0 && next() < 0.6) edges.push({ source: earlier[pick(earlier.length)], target: ids[0] });
      earlier.push(...ids);
      // Now and then a node an earlier statement has too.
      if (nodes.length > ids.length && next() < 0.3) ids.push(nodes[pick(nodes.length - ids.length)].id);
      rows.set(rowKey(b, i), ids);
    }
    bands.push({ title: `band ${b}`, rows: band });
  }
  for (let i = 0, loose = pick(3); i < loose; i++) add();
  return { graph: { bands, nodes: nodes.length } as FlowGraph, rows, nodes, edges };
}

const inside = (a: { x: number; y: number; width: number; height: number }, f: Frame) =>
  a.x >= f.x && a.y >= f.y && a.x + a.width <= f.x + f.width && a.y + a.height <= f.y + f.height;
const apart = (a: Frame, b: Frame) =>
  a.x + a.width <= b.x || b.x + b.width <= a.x || a.y + a.height <= b.y || b.y + b.height <= a.y;

describe("groupByStatements", () => {
  it("puts every node once, inside its statement's frame, inside its trigger's, with no frames overlapping", () => {
    for (let seed = 1; seed <= 200; seed++) {
      const { graph, rows, nodes, edges } = flow(seed);
      const { positions, frames, byRow } = groupByStatements(graph, rows, nodes, edges);
      const at = `seed ${seed}`;

      expect(positions.size, at).toBe(nodes.length);
      const owners = [...byRow.values()].flat();
      expect(new Set(owners).size, at).toBe(owners.length);

      const bands = frames.filter((f) => f.level === "band");
      const statements = frames.filter((f) => f.level !== "band");
      // Every trigger's frame first, so the statements' draw over them.
      expect(frames.slice(0, bands.length).every((f) => f.level === "band"), at).toBe(true);
      for (let i = 0; i < bands.length; i++)
        for (let j = i + 1; j < bands.length; j++) expect(apart(bands[i], bands[j]), at).toBe(true);
      for (let i = 0; i < statements.length; i++)
        for (let j = i + 1; j < statements.length; j++) expect(apart(statements[i], statements[j]), at).toBe(true);

      for (const s of statements) {
        expect(bands.some((b) => inside(s, b)), `${at}: ${s.key} outside every trigger`).toBe(true);
        for (const id of byRow.get(s.key)!) {
          const n = nodes.find((x) => x.id === id)!;
          const p = positions.get(id)!;
          expect(inside({ ...p, width: n.measured!.width!, height: n.measured!.height! }, s), `${at}: ${id} outside ${s.key}`).toBe(true);
        }
      }
      // The tidy's columns stand: inside a trigger, every wire runs left to right.
      const bandOf = new Map([...byRow].flatMap(([key, ids]) => ids.map((id) => [id, key.split(".")[0]] as const)));
      for (const e of edges)
        if (bandOf.get(e.source) === bandOf.get(e.target))
          expect(positions.get(e.target)!.x, `${at}: ${e.source} -> ${e.target}`).toBeGreaterThan(positions.get(e.source)!.x);
      // Nodes inside a statement don't overlap each other.
      for (const ids of byRow.values()) {
        const boxes = ids.map((id) => ({ ...positions.get(id)!, width: 232, height: nodes.find((x) => x.id === id)!.measured!.height! }));
        for (let i = 0; i < boxes.length; i++)
          for (let j = i + 1; j < boxes.length; j++) expect(apart(boxes[i] as Frame, boxes[j] as Frame), at).toBe(true);
      }
    }
  });

  it("gives a shared node to the first statement that has it, and frames the nodes no statement has", () => {
    const nodes = ["a", "b", "c", "d"].map((id): TidyNode => ({ id, position: { x: 0, y: 0 }, measured: { width: 232, height: 46 } }));
    const graph = { bands: [{ title: "on GET /", rows: [row(1, "compute", true), row(2, "answer", false)] }], nodes: 4 } as FlowGraph;
    const rows = new Map([
      [rowKey(0, 0), ["a", "b"]],
      [rowKey(0, 1), ["b", "c"]],
    ]);
    const { byRow, frames } = groupByStatements(graph, rows, nodes, [{ source: "a", target: "b" }]);
    expect(byRow.get(rowKey(0, 0))).toEqual(["a", "b"]);
    expect(byRow.get(rowKey(0, 1))).toEqual(["c"]);
    expect(byRow.get("loose nodes")).toEqual(["d"]);
    expect(frames.map((f) => f.label)).toEqual(["on GET /", "not in a statement", "on · line 1", expect.stringMatching(/ · line 2$/), "the nodes no statement has"]);
  });

  it("wraps many triggers onto shelves instead of one long row", () => {
    const { graph, rows, nodes, edges } = flow(7);
    const many = { ...graph, bands: Array.from({ length: 12 }, (_, b) => graph.bands[b % graph.bands.length]) };
    const repeated = new Map([...Array(12).keys()].flatMap((b) => many.bands[b].rows.map((_, i) => [rowKey(b, i), (rows.get(rowKey(b % graph.bands.length, i)) ?? []).map((id) => id)] as const)));
    // Each band its own nodes, so all twelve have frames.
    const copies: TidyNode[] = [];
    const wired: { source: string; target: string }[] = [];
    for (const [key, ids] of repeated) {
      const renamed = ids.map((id) => `${key}:${id}`);
      renamed.forEach((id, k) => copies.push({ ...nodes.find((n) => n.id === ids[k])!, id }));
      repeated.set(key, renamed);
    }
    for (const e of edges) for (const key of repeated.keys()) wired.push({ source: `${key}:${e.source}`, target: `${key}:${e.target}` });
    const { frames } = groupByStatements(many, repeated, copies, wired);
    const bands = frames.filter((f) => f.level === "band");
    const width = Math.max(...bands.map((f) => f.x + f.width));
    const height = Math.max(...bands.map((f) => f.y + f.height));
    expect(new Set(bands.map((f) => f.y)).size).toBeGreaterThan(1);
    expect(width / height).toBeLessThan(4);
  });
});
