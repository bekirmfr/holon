// A review in hunks (hunks.ts): the code's hunk numbered and folded, the touches' hunks counted,
// and n / N going between the hunks that change something.

import { describe, expect, it } from "vitest";
import { codeHunk, diffHunk, isFold, nextHunk, touchHunks, type Row } from "./hunks";
import type { TouchLine } from "./touches";

const lines = (n: number, from = 1) => Array.from({ length: n }, (_, i) => `line ${i + from}`);

describe("codeHunk", () => {
  it("numbers rows as the new text does, a removed one as the old did", () => {
    const h = codeHunk("a\nb\nc\n", "a\nB\nc\n");
    expect(h.summary).toBe("+1 · −1");
    expect((h.rows as Row[]).map((r) => [r.n, r.sign, r.text])).toEqual([
      [1, " ", "a"],
      [2, "−", "b"],
      [2, "+", "B"],
      [3, " ", "c"],
    ]);
  });

  it("folds unchanged runs more than 3 lines from a change", () => {
    const before = lines(20).join("\n");
    const after = [...lines(10), "changed", ...lines(9, 12)].join("\n");
    const h = codeHunk(before, after);
    const folds = h.rows.filter(isFold);
    expect(folds.map((f) => f.fold.length)).toEqual([7, 6]);
    expect((h.rows.find((r) => !isFold(r) && r.sign === "+") as Row).n).toBe(11);
  });

  it("says a first deploy is new, and an unchanged text the same", () => {
    expect(codeHunk(null, "a\nb\n").summary).toBe("new · 2 lines");
    expect(codeHunk(null, "a").opens).toBe(false);
    const same = codeHunk("a\nb", "a\nb");
    expect(same.summary).toBe("the same");
    expect(same.changes).toBe(false);
  });
});

describe("diffHunk", () => {
  it("numbers the code as the restored text, says which side is which, and keeps a grant whole", () => {
    const code = diffHunk({ title: "code", legend: "− now   + then", lines: [{ sign: " ", text: "a" }, { sign: "-", text: "b" }, { sign: "+", text: "c" }], note: "names left out" });
    expect(code.summary).toBe("+1 · −1");
    expect(code.note).toBe("− now   + then · names left out");
    expect((code.rows as Row[]).map((r) => r.n)).toEqual([1, 2, 2]);
    const grant = diffHunk({ title: "permissions", legend: "", lines: lines(12).map((text) => ({ sign: " " as const, text })), whole: true });
    expect(grant.rows.filter(isFold)).toEqual([]);
    expect(grant.summary).toBe("the same");
    expect(grant.opens).toBe(false);
    const none = diffHunk({ title: "code", legend: "", lines: [], note: "The text can't be compared" });
    expect([none.summary, none.opens]).toEqual(["", true]);
  });
});

describe("touchHunks", () => {
  const dry: TouchLine[] = [
    { mark: "head", text: "What it reaches outside", note: "" },
    { mark: "approve", text: "host api.example.com: calls", note: "approved by confirming" },
    { mark: "same", text: "the clock", note: "" },
    { mark: "head", text: "What it keeps", note: "" },
    { mark: "same", text: "key count", note: "" },
  ];

  it("makes a hunk per heading, counted", () => {
    const hs = touchHunks(dry);
    expect(hs.map((h) => [h.title, h.summary, h.changes])).toEqual([
      ["What it reaches outside", "1 to approve · 1 the same", true],
      ["What it keeps", "1 the same", false],
    ]);
    expect((hs[0].rows[0] as Row).sign).toBe("+");
  });

  it("n and N go between the hunks that change something, wrapping", () => {
    const hs = [codeHunk("a", "b"), ...touchHunks(dry)];
    expect(nextHunk(hs, 0, 1)).toBe(1);
    expect(nextHunk(hs, 1, 1)).toBe(0);
    expect(nextHunk(hs, 0, -1)).toBe(1);
    expect(nextHunk([hs[2]], 0, 1)).toBe(0);
  });
});
