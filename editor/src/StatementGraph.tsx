// A flow by its statements (web-console-L4.md §4): one column per trigger, headed by it, and a box
// per statement under it, with its kind word and line, its name, and a bar in its kind's colour
// (computes grey, reads or writes purple, calls out teal, answers in the accent, waits amber). A
// fallback answer (`?? status(404)`) is a side box on its row. Wires are the rows a row takes from
// in its band, drawn as lanes in the column's gutter, as `holon editor` draws them; what it takes
// from outside its band is a label. Against another version, the rows whose text is new are dashed
// amber; over a run, the rows it didn't run are dimmed, the step it is at has a thick border, and a
// row shows the value it made. Drawn from holonGraph (holonText.ts), the terminal's own graph; it
// fetches nothing, and picking a row is the caller's to act on (put the cursor on its mark).

import { useEffect, useMemo, useRef } from "react";
import type { FlowGraph, GraphRow, RowKind } from "./holonText";

/** Each kind as a person reads it. */
export const KIND_WORD: Record<RowKind, string> = {
  compute: "computes",
  data: "reads or writes",
  outside: "calls out",
  answer: "answers",
  waits: "waits",
};

/** A row's key in a graph: its band and its place in the band. */
export const rowKey = (band: number, row: number) => `${band}.${row}`;

/** A run over the graph (View runs): what it ran, the step it is at, and the value each row made,
 *  by rowKey. */
export type GraphRun = { ran: Set<string>; current?: string | null; values?: Map<string, string> };

/** What a row is, to tell it in another version: its text and what its nodes are and are set to
 *  (not their names, which shift as other rows come and go). */
const sameness = (r: GraphRow) => [r.text, ...r.parts.map((p) => `${p.leaf}${JSON.stringify(p.params)}`)].join("\n");

/** The rows of `graph` whose text `against` doesn't have, by rowKey: in the band of the same
 *  trigger, else anywhere in it (a trigger written anew keeps the statements it had). */
export function newRows(graph: FlowGraph, against: FlowGraph): Set<string> {
  const count = (rows: GraphRow[]) => {
    const n = new Map<string, number>();
    for (const r of rows) n.set(sameness(r), (n.get(sameness(r)) ?? 0) + 1);
    return n;
  };
  const anywhere = count(against.bands.flatMap((b) => b.rows));
  const fresh = new Set<string>();
  graph.bands.forEach((band, b) => {
    const same = against.bands.find((o) => o.title === band.title);
    const left = same ? count(same.rows) : anywhere;
    band.rows.forEach((r, i) => {
      const k = sameness(r);
      const n = left.get(k) ?? 0;
      if (n > 0) left.set(k, n - 1);
      else fresh.add(rowKey(b, i));
    });
  });
  return fresh;
}

const ROW_H = 50;
const GAP = 8;
const LANE = 9;
const mid = (i: number) => i * (ROW_H + GAP) + ROW_H / 2;

/** A band's wires as lanes: a row that feeds later rows holds a lane down to the last of them, the
 *  lowest free where it starts. Each lane's x, by the row holding it. */
export function laneOf(rows: GraphRow[]): { lane: Map<number, number>; last: Map<number, number>; width: number } {
  const last = new Map<number, number>();
  rows.forEach((r, t) => r.from.filter((s) => s < t).forEach((s) => last.set(s, Math.max(last.get(s) ?? t, t))));
  const held: (number | null)[] = [];
  const lane = new Map<number, number>();
  rows.forEach((_, r) => {
    for (let l = 0; l < held.length; l++) if (held[l] !== null && last.get(held[l]!)! < r) held[l] = null;
    if (!last.has(r)) return;
    let l = held.indexOf(null);
    if (l < 0) l = held.push(null) - 1;
    held[l] = r;
    lane.set(r, l);
  });
  return { lane, last, width: held.length };
}

export function StatementGraph({
  graph,
  against = null,
  cursor = null,
  onPick,
  run = null,
}: {
  graph: FlowGraph;
  /** Another version (the one deployed): the rows new since it are dashed amber. */
  against?: FlowGraph | null;
  /** The line the text cursor is on: the row it is in is selected. */
  cursor?: number | null;
  /** A row was clicked: the caller puts the cursor on its mark. */
  onPick?: (row: GraphRow, band: number, index: number) => void;
  run?: GraphRun | null;
}) {
  const fresh = useMemo(() => (against ? newRows(graph, against) : new Set<string>()), [graph, against]);
  // The row the cursor is in: a statement's lines, else its trigger's line.
  const selected = useMemo(() => {
    if (cursor === null) return null;
    const at = (head: boolean) => {
      for (const [b, band] of graph.bands.entries())
        for (const [i, r] of band.rows.entries()) if (r.head === head && r.line <= cursor && cursor <= r.last) return rowKey(b, i);
      return null;
    };
    return at(false) ?? at(true);
  }, [graph, cursor]);
  const picked = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    picked.current?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [selected]);

  const used = new Set(graph.bands.flatMap((b) => b.rows.filter((r) => !r.head).map((r) => r.kind)));
  return (
    <div className="hl-sg" role="region" aria-label="statement graph">
      <div className="hl-sg__key" aria-label="key">
        {(Object.keys(KIND_WORD) as RowKind[])
          .filter((k) => used.has(k))
          .map((k) => (
            <span key={k} className={`hl-sg__swatch hl-sg--${k}`}>
              {KIND_WORD[k]}
            </span>
          ))}
        {against && fresh.size > 0 && <span className="hl-sg__swatch hl-sg__swatch--new">new in this version</span>}
        <span className="hl-sg__count">
          {graph.bands.reduce((n, b) => n + b.rows.length - 1, 0)} statements, {graph.nodes} nodes
        </span>
      </div>
      <div className="hl-sg__bands">
        {graph.bands.map((band, b) => {
          const { lane, last, width } = laneOf(band.rows);
          const gutter = 8 + width * LANE;
          const x = (l: number) => 6 + l * LANE;
          const height = band.rows.length * (ROW_H + GAP) - GAP;
          const side = band.rows.some((r) => r.fallback !== null);
          return (
            <section key={b} className="hl-sg__band" role="group" aria-label={band.title}>
              <div className="hl-sg__rows" style={{ paddingLeft: gutter }}>
                <svg className="hl-sg__wires" width={gutter} height={height} aria-hidden="true">
                  {[...lane].map(([s, l]) => (
                    <g key={s}>
                      <path d={`M${gutter},${mid(s)} H${x(l)} V${mid(last.get(s)!)}`} />
                      {band.rows.map((r, t) =>
                        t > s && r.from.includes(s) ? (
                          <g key={t}>
                            <path d={`M${x(l)},${mid(t)} H${gutter - 1}`} />
                            <circle cx={x(l)} cy={mid(t)} r={2.5} />
                          </g>
                        ) : null,
                      )}
                    </g>
                  ))}
                </svg>
                {band.rows.map((r, i) => {
                  const key = rowKey(b, i);
                  const value = run?.values?.get(key);
                  const cls = [
                    "hl-sg__row",
                    `hl-sg--${r.kind}`,
                    r.head && "hl-sg__row--head",
                    fresh.has(key) && "hl-sg__row--new",
                    run && !run.ran.has(key) && "hl-sg__row--idle",
                    run?.current === key && "hl-sg__row--current",
                  ]
                    .filter(Boolean)
                    .join(" ");
                  return (
                    <div key={i} className="hl-sg__slot" style={{ height: ROW_H }}>
                      <button
                        ref={key === selected ? picked : undefined}
                        className={cls}
                        aria-current={key === selected ? "true" : undefined}
                        data-new={fresh.has(key) ? "true" : undefined}
                        title={[r.text, r.does && `is ${r.does}`, r.outside.length > 0 && `takes ${r.outside.join(", ")} from outside`, value !== undefined && `= ${value}`].filter(Boolean).join("\n")}
                        onClick={() => onPick?.(r, b, i)}
                      >
                        <span className="hl-sg__said">
                          <span className="hl-sg__kind">{r.head ? "on" : KIND_WORD[r.kind]}</span>
                          <span className="hl-sg__line">line {r.line}</span>
                          {r.outside.length > 0 && <span className="hl-sg__outside">← {r.outside.join(", ")}</span>}
                        </span>
                        <span className="hl-sg__name">
                          {r.name}
                          {value !== undefined ? <span className="hl-sg__value"> = {value}</span> : r.does && !r.head && <span className="hl-sg__does"> {r.does}</span>}
                        </span>
                      </button>
                      {side && <span className="hl-sg__side">{r.fallback !== null && <span className="hl-sg__fallback">missing → {r.fallback}</span>}</span>}
                    </div>
                  );
                })}
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}
