// The marketplace's client side (marketplace.ts) against the real core: a saved flow as a
// listing, placed again in a fresh workspace, a tampered one refused, adoption counted.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeAll, describe, expect, it } from "vitest";
import { initSync, Evaluator } from "../wasm/holon_wasm.js";
import { buildPreimage, type WUnit } from "./holon";
import { catalogOf, listingOf, placeListing, type Listing } from "./marketplace";
import { compileHolon } from "./holonText";


beforeAll(() => {
  initSync({ module: readFileSync(join(process.cwd(), "wasm", "holon_wasm_bg.wasm")) });
});

/** A saved flow without routes: upper-cases its text, through a holon of its own. */
function shoutFlow(ev: Evaluator) {
  return compileHolon(
    `flow shout
holon upper(text) -> (out) {
  node up = uppercase
  in.text -> up.s
  up.out -> out.out
}
node shout = upper
`,
    ev,
  ).doc;
}

const published = (l: Omit<Listing, "id" | "published_at">, at: number): Listing => ({ ...l, id: `${l.name}@${l.root}`, published_at: at });

describe("the marketplace, client side", () => {
  it("publishes a saved flow as its units, and places it again where they weren't", () => {
    const ev = new Evaluator();
    const listing = listingOf("shout", shoutFlow(ev), ev, { description: "shouts", publisher: "ada" });
    if (typeof listing === "string") throw new Error(listing);
    expect(listing.contains).toContain(listing.root);
    expect(listing.manifest).toEqual([]);
    expect(listing.inputs.map((p) => p.name)).toHaveLength(1);

    // A workspace that has never seen it.
    const fresh = new Evaluator();
    const node = placeListing(published(listing, 1), fresh, "n1", { x: 0, y: 0 });
    if (typeof node === "string") throw new Error(node);
    expect(node.data.body).toEqual({ kind: "Composite", hash: listing.root });
    const out = fresh.eval(buildPreimage(fresh.get_unit(listing.root) as WUnit), [{ type: "Text", value: "hey" }]) as unknown[];
    expect(JSON.stringify(out)).toContain("HEY");
  });

  it("refuses a listing whose units aren't what its root names", () => {
    const ev = new Evaluator();
    const listing = listingOf("shout", shoutFlow(ev), ev, { description: "shouts", publisher: "ada" });
    if (typeof listing === "string") throw new Error(listing);
    // The root's own unit left out: what's left hashes to something else.
    const swapped = { ...published(listing, 1), units: listing.units.slice(0, -1) };
    expect(placeListing(swapped, new Evaluator(), "n1", { x: 0, y: 0 })).toContain("don't hash to its root");
  });

  it("lists each holon once, its newest unyanked version, with how many others place it", () => {
    const l = (name: string, root: string, at: number, contains: string[], yanked = false): Listing => ({
      id: `${name}@${root}`, name, root, published_at: at, contains: [root, ...contains], yanked,
      description: "", units: [], inputs: [], outputs: [], manifest: [], publisher: "p",
    });
    const entries = catalogOf([
      l("slugify", "a1", 1, []),
      l("slugify", "a2", 5, [], true),
      l("blog", "b1", 3, ["a1"]),
      l("wiki", "c1", 4, ["a1", "b1"]),
      l("wiki", "c2", 6, ["a1"]),
    ]);
    expect(entries.map((e) => `${e.listing.id}:${e.adoption}:${e.versions}`)).toEqual(["wiki@c2:0:2", "blog@b1:1:1", "slugify@a1:2:2"]);
  });
});
