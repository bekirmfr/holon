import { describe, it, expect } from "vitest";
import { History } from "./history";

const make = (initial = "A") => new History<string>(initial, (a, b) => a === b);

describe("History", () => {
  it("undoes and redoes committed edits", () => {
    const h = make();
    h.commit("B");
    h.commit("C");
    expect(h.undo("C")).toBe("B");
    expect(h.undo("B")).toBe("A");
    expect(h.undo("A")).toBeNull();
    expect(h.redo("A")).toBe("B");
    expect(h.redo("B")).toBe("C");
    expect(h.redo("C")).toBeNull();
  });

  // Regression: Ctrl+Z inside the 400 ms debounce window cancelled the pending commit and then
  // popped one step too far. The last edit was gone from both undo and redo.
  it("an edit still inside the debounce window is undone first, and can be redone", () => {
    const h = make();
    h.commit("B");
    expect(h.undo("C")).toBe("B"); // "C" was on the canvas but not yet committed
    expect(h.redo("B")).toBe("C");
    expect(h.undo("C")).toBe("B");
    expect(h.undo("B")).toBe("A");
  });

  it("a pending edit before redo is a new edit, which ends the redo branch", () => {
    const h = make();
    h.commit("B");
    expect(h.undo("B")).toBe("A");
    expect(h.redo("X")).toBeNull();
    expect(h.undo("X")).toBe("A");
  });

  it("ignores no-op commits and caps the past", () => {
    const h = new History<number>(0, (a, b) => a === b, 3);
    expect(h.commit(0)).toBe(false);
    for (let i = 1; i <= 5; i++) h.commit(i);
    expect(h.past).toEqual([2, 3, 4]);
  });
});
