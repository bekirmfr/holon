import { describe, expect, it } from "vitest";
import { holonUses, interfaceLoss } from "./holonUsage";

const placing = (name: string, saved: number, sources: [string, number][]) => ({
  name,
  saved_at_epoch_seconds: saved,
  document: { nodes: [{ data: {} }, ...sources.map(([flow, savedAt]) => ({ data: { source: { flow, savedAt } } }))] },
});

describe("holons across saved flows", () => {
  it("lists the flows that place each one, and whether from its newest save", () => {
    const uses = holonUses([
      placing("slugify", 20, []),
      placing("blog", 30, [["slugify", 20], ["slugify", 20]]),
      placing("archive", 31, [["slugify", 10]]),
      placing("loop", 32, [["loop", 1]]),
    ]);
    expect(uses.get("slugify")).toEqual([
      { user: "archive", nodes: 1, current: false },
      { user: "blog", nodes: 2, current: true },
    ]);
    expect(uses.has("blog")).toBe(false);
    expect(uses.has("loop"), "a flow doesn't use itself").toBe(false);
  });

  it("says what a new interface takes away: ports removed or retyped, not ones added", () => {
    const before = { inputs: [{ name: "text", ty: "Text" as const }, { name: "sep", ty: "Text" as const }], outputs: [{ name: "slug", ty: "Text" as const }] };
    const after = { inputs: [{ name: "text", ty: "Text" as const }, { name: "max", ty: "Number" as const }], outputs: [{ name: "slug", ty: "Number" as const }] };
    expect(interfaceLoss(before, after)).toEqual(['input "sep" removed', 'output "slug" is now number (was text)']);
    expect(interfaceLoss(before, before)).toEqual([]);
  });
});
