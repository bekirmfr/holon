import { describe, expect, it } from "vitest";
import { collapse, diffCounts, flatValue, keyLines, lineDiff, setDiff, valueLines } from "./diff";
import type { WValue } from "./holon";

const signs = (lines: { sign: string; text: string }[]) => lines.map((l) => `${l.sign}${l.text}`);

describe("lineDiff", () => {
  it("keeps what both share, and says what is removed and added, removals first", () => {
    expect(signs(lineDiff(["a", "b", "c", "d"], ["a", "x", "c", "d", "e"]))).toEqual([" a", "-b", "+x", " c", " d", "+e"]);
  });
  it("is all kept for the same text, and all added from nothing", () => {
    expect(signs(lineDiff(["a", "b"], ["a", "b"]))).toEqual([" a", " b"]);
    expect(signs(lineDiff([], ["a"]))).toEqual(["+a"]);
    expect(signs(lineDiff(["a"], []))).toEqual(["-a"]);
  });
  it("matches the longest shared run, not the first line that matches", () => {
    expect(signs(lineDiff(["x", "a", "b", "c"], ["a", "b", "c", "x"]))).toEqual(["-x", " a", " b", " c", "+x"]);
  });
});

describe("setDiff", () => {
  it("says what only one side has, sorted, and what both keep", () => {
    expect(signs(setDiff(["NetworkIngress", 'StateWrite("b")'], ['StateWrite("a")', "NetworkIngress"]))).toEqual([
      " NetworkIngress",
      '+StateWrite("a")',
      '-StateWrite("b")',
    ]);
  });
});

describe("collapse", () => {
  it("leaves out unchanged runs far from a change, saying how many", () => {
    const lines = lineDiff(["1", "2", "3", "4", "5", "6", "7", "8", "9"], ["1", "2", "3", "4", "5", "6", "7", "8", "X"]);
    expect(signs(collapse(lines, 2))).toEqual(["…6 unchanged lines", " 7", " 8", "-9", "+X"]);
    expect(signs(collapse(lineDiff(["a"], ["a"])))).toEqual(["…1 unchanged line"]);
  });
  it("counts what a diff adds and removes", () => {
    expect(diffCounts(lineDiff(["a", "b"], ["a", "c", "d"]))).toEqual({ added: 2, removed: 1 });
  });
});

describe("valueLines", () => {
  const text = (value: string): WValue => ({ type: "Text", value });
  const num = (value: number): WValue => ({ type: "Number", value });
  it("writes a short value on one line, a text quoted as JSON", () => {
    expect(flatValue({ type: "Record", value: [["n", num(1)], ["t", text('say "hi"\n')]] })).toBe('{n: 1, t: "say \\"hi\\"\\n"}');
    expect(valueLines({ type: "List", value: [num(1), num(2)] })).toEqual(["[1, 2]"]);
  });
  it("writes a long list or record one item per line, nested as deep as it needs", () => {
    const long = "x".repeat(40);
    const v: WValue = { type: "Record", value: [["a", text(long)], ["b", { type: "List", value: [text(long), text(long)] }]] };
    expect(valueLines(v)).toEqual(["{", `  a: "${long}",`, "  b: [", `    "${long}",`, `    "${long}"`, "  ]", "}"]);
    expect(keyLines("cart", v)[0]).toBe("cart: {");
    expect(keyLines("total", num(4.5))).toEqual(["total: 4.5"]);
  });
});
