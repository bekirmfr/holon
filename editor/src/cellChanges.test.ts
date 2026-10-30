import { describe, expect, it } from "vitest";
import { cellName, cellRows } from "./cellChanges";
import type { CellHistory, VersionEntry } from "./server";

const n = (value: number) => ({ type: "Number", value }) as const;
const version = (number: number, unit: string) => ({ number, unit }) as VersionEntry;

describe("cellName", () => {
  it("writes the cell as the language does", () => {
    expect(cellName({ name: "moves", item: null, collection: false })).toBe("state.moves");
    expect(cellName({ name: "balances", item: "a", collection: true })).toBe('balances["a"]');
    expect(cellName({ name: "balances", item: null, collection: true })).toBe("balances");
  });
});

describe("cellRows", () => {
  it("names each run with its version and the restore between two runs by its event", () => {
    const h: CellHistory = {
      name: "balances",
      item: "a",
      collection: true,
      now: n(101),
      more: false,
      changes: [
        { seq: 9, at_ms: 5, kind: "request", method: "POST", url: "/accounts/a/move", status: 200, unit: "u2", failed: false, before: n(100), after: n(101) },
        { between: [7, 9], at_ms: 4, events: [{ kind: "restore", what: "put back the state run 5 left, in session 2", by: "maria", at_ms: 4 }], before: n(70), after: n(100) },
        { seq: 5, at_ms: 1, kind: "request", method: "POST", url: "/accounts/a/move", status: 200, unit: "u1", failed: false, by: "ada (agent)", before: null, after: n(100) },
      ],
    };
    const rows = cellRows(h, [version(2, "u2"), version(1, "u1")]);
    expect(rows.map((r) => [r.seq, r.label, r.what, r.values])).toEqual([
      [9, "run 9", "POST /accounts/a/move · 200 · v2", ["100 → 101"]],
      [undefined, "between runs 7 and 9", "restore: put back the state run 5 left, in session 2 by maria", ["70 → 100"]],
      [5, "run 5", "POST /accounts/a/move · 200 · v1 · sent by ada (agent)", ["(none) → 100"]],
    ]);
  });
  it("lists a whole collection's change by the items it changed", () => {
    const h: CellHistory = {
      name: "balances",
      item: null,
      collection: true,
      now: null,
      more: true,
      changes: [{ seq: 3, kind: "tick", unit: "zz", failed: true, items: { a: { before: n(1), after: n(2) }, b: { before: n(5), after: null } }, more_items: true }],
    };
    const [row] = cellRows(h);
    expect([row.what, row.values, row.failed]).toEqual(["tick", ["a: 1 → 2", "b: 5 → (none)", "and more"], true]);
  });
});
