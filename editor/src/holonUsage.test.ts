import { describe, expect, it } from "vitest";
import { holonIndex, holonUses, interfaceLoss, usesIn } from "./holonUsage";

const doc = (sources: [string, number][]) => ({ nodes: [{ data: {} }, ...sources.map(([flow, savedAt]) => ({ data: { source: { flow, savedAt } } }))] });

describe("holons across saved flows and deployments", () => {
  it("reads what a document places", () => {
    expect(usesIn(doc([["slugify", 20], ["slugify", 20]]))).toEqual([
      { flow: "slugify", saved_at: 20 },
      { flow: "slugify", saved_at: 20 },
    ]);
  });

  it("lists who places each one, and whether from its newest save", () => {
    const uses = holonUses(
      [
        { name: "slugify", kind: "flow", uses: [] },
        { name: "blog", kind: "flow", uses: usesIn(doc([["slugify", 20], ["slugify", 20]])) },
        { name: "archive", kind: "flow", uses: usesIn(doc([["slugify", 10]])) },
        { name: "loop", kind: "flow", uses: usesIn(doc([["loop", 1]])) },
        { name: "blog", kind: "deployment", uses: usesIn(doc([["slugify", 10]])) },
      ],
      new Map([["slugify", 20], ["blog", 30], ["archive", 31], ["loop", 32]]),
    );
    expect(uses.get("slugify")).toEqual([
      { user: "archive", kind: "flow", nodes: 1, current: false },
      { user: "blog", kind: "flow", nodes: 2, current: true },
      { user: "blog", kind: "deployment", nodes: 1, current: false },
    ]);
    expect(uses.has("loop"), "a flow doesn't use itself").toBe(false);
  });

  it("answers from the listings, reading only flows saved before uses were recorded", async () => {
    const read: string[] = [];
    const uses = await holonIndex(
      [
        { name: "slugify", saved_at_epoch_seconds: 20, uses: [] },
        { name: "blog", saved_at_epoch_seconds: 30, uses: [{ flow: "slugify", saved_at: 20 }] },
        { name: "old", saved_at_epoch_seconds: 5 },
      ],
      [{ slug: "blog", uses: [{ flow: "slugify", saved_at: 20 }] }],
      async (name) => {
        read.push(name);
        return doc([["slugify", 10]]);
      },
    );
    expect(read).toEqual(["old"]);
    expect(uses.get("slugify")?.map((u) => `${u.kind}:${u.user}:${u.current}`)).toEqual(["flow:blog:true", "flow:old:false", "deployment:blog:true"]);
  });

  it("says what a new interface takes away: ports removed or retyped, not ones added", () => {
    const before = { inputs: [{ name: "text", ty: "Text" as const }, { name: "sep", ty: "Text" as const }], outputs: [{ name: "slug", ty: "Text" as const }] };
    const after = { inputs: [{ name: "text", ty: "Text" as const }, { name: "max", ty: "Number" as const }], outputs: [{ name: "slug", ty: "Number" as const }] };
    expect(interfaceLoss(before, after)).toEqual(['input "sep" removed', 'output "slug" is now number (was text)']);
    expect(interfaceLoss(before, before)).toEqual([]);
  });
});
