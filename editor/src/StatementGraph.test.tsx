// A flow by its statements (web-console-L4.md §4, §13): every sample has a graph, from the
// terminal's own function through the wasm bridge, and the component draws each of its rows; a row
// new against another version is dashed; clicking a row hands it, with its mark, to the caller;
// the cursor's line selects the row it is in; over a run, what didn't run is dimmed.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { initSync } from "../wasm/holon_wasm.js";
import { holonGraph, type GraphRow } from "./holonText";
import { samples } from "./samples";
import { newRows, rowKey, StatementGraph } from "./StatementGraph";

initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
});

function draw(el: React.ReactElement): HTMLDivElement {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(el));
  return container;
}

const sample = (name: string) => samples().find((s) => s.name === name)!.text;
const rowsOf = (c: HTMLElement) => [...c.querySelectorAll<HTMLButtonElement>(".hl-sg__row")];
/** A drawn row's name, without what it is beside it. */
const nameOf = (b: HTMLElement) => b.querySelector(".hl-sg__name")!.firstChild!.textContent;

describe("the statement graph", () => {
  for (const s of samples()) {
    it(`${s.name} has bands, each headed by its trigger, and each row drawn`, () => {
      const g = holonGraph(s.text);
      expect(g.bands.length, s.name).toBeGreaterThan(0);
      for (const band of g.bands) {
        expect(band.rows[0].head, band.title).toBe(true);
        expect(band.rows.slice(1).every((r) => !r.head && r.line > 0 && r.last >= r.line && r.at.from <= r.at.to)).toBe(true);
        for (const [t, r] of band.rows.entries()) expect(r.from.every((f) => f !== t && f < band.rows.length)).toBe(true);
      }
      const c = draw(<StatementGraph graph={g} />);
      expect(c.querySelectorAll("[role=group]").length).toBe(g.bands.length);
      expect(rowsOf(c).length).toBe(g.bands.reduce((n, b) => n + b.rows.length, 0));
      expect(c.querySelectorAll(".hl-sg__row--new").length, "nothing is new against nothing").toBe(0);
    });
  }

  it("says each row's kind, and a fallback answer beside its row", () => {
    const payout = holonGraph(sample("approved-payout"));
    const kinds = payout.bands[1].rows.map((r) => r.kind);
    expect(kinds).toContain("waits");
    expect(kinds).toContain("data");
    const c = draw(<StatementGraph graph={payout} />);
    const waits = rowsOf(c).find((b) => b.classList.contains("hl-sg--waits"))!;
    expect(waits.textContent).toContain("waits");

    const board = holonGraph(sample("leaderboard"));
    const answer = board.bands.flatMap((b) => b.rows).find((r) => r.text === "return card ?? status(404)")!;
    expect(answer.fallback).toBe("404");
    act(() => root!.render(<StatementGraph graph={board} />));
    expect([...c.querySelectorAll(".hl-sg__fallback")].map((f) => f.textContent)).toContain("missing → 404");
  });

  it("dashes a row new against another version", () => {
    const before = sample("leaderboard");
    const g = holonGraph(before);
    // A statement added before the last route's answer: that row is new, the rest are as they were.
    const band = g.bands.length - 1;
    const answer = g.bands[band].rows.at(-1)!;
    const lines = before.split("\n");
    const after = [...lines.slice(0, answer.line - 1), '  const extra = req.params.player + "!"', ...lines.slice(answer.line - 1)].join("\n");
    const changed = holonGraph(after);
    const fresh = newRows(changed, g);
    expect([...fresh].map((k) => k.split(".").map(Number)).map(([b, r]) => changed.bands[b].rows[r].name)).toEqual(["extra"]);
    const c = draw(<StatementGraph graph={changed} against={g} />);
    const dashed = rowsOf(c).filter((b) => b.classList.contains("hl-sg__row--new"));
    expect(dashed.map(nameOf)).toEqual(["extra"]);
    expect(dashed[0].querySelector(".hl-sg__new")?.textContent).toBe("new");
    expect(c.querySelectorAll(".hl-sg__new")).toHaveLength(1);
    expect(c.textContent).toContain("new since the version deployed");
  });

  it("hands a clicked row to the caller, with its mark, and selects the row the cursor is in", () => {
    const text = sample("approved-payout");
    const g = holonGraph(text);
    const picked: GraphRow[] = [];
    const c = draw(<StatementGraph graph={g} onPick={(r) => picked.push(r)} />);
    const target = g.bands[1].rows[2];
    const button = rowsOf(c).find((b) => nameOf(b) === target.name && b.textContent!.includes(`line ${target.line}`))!;
    act(() => button.click());
    expect(picked).toEqual([target]);
    expect(text.slice(picked[0].at.from, picked[0].at.to)).toBe(target.name);

    act(() => root!.render(<StatementGraph graph={g} cursor={target.line} />));
    const current = c.querySelectorAll("[aria-current=true]");
    expect(current.length).toBe(1);
    expect(current[0].textContent).toContain(target.name);
  });

  it("over a run, dims what didn't run and marks the step it is at, with its value", () => {
    const g = holonGraph(sample("approved-payout"));
    const ran = new Set([rowKey(0, 0), rowKey(0, 1)]);
    const c = draw(<StatementGraph graph={g} run={{ ran, current: rowKey(0, 1), values: new Map([[rowKey(0, 1), '{"balance":5}']]) }} />);
    const rows = rowsOf(c);
    const total = g.bands.reduce((n, b) => n + b.rows.length, 0);
    expect(rows.filter((r) => r.classList.contains("hl-sg__row--idle")).length).toBe(total - 2);
    const current = rows.filter((r) => r.classList.contains("hl-sg__row--current"));
    expect(current.length).toBe(1);
    expect(current[0].textContent).toContain('= {"balance":5}');
  });
});
