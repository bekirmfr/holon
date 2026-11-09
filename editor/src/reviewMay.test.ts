// The Review page's left column (reviewMay.ts): what a version may do, grouped and anchored.

import { describe, expect, it } from "vitest";
import { anchorOf, mayGroups, textLines } from "./reviewMay";
import type { Touch } from "./touches";

const TEXT = `import { flow, route, secret } from "holon"
// It refunds through https://api.stripe.com, with the secret stripe_key.

flow("refunds")

const key = secret("stripe_key")
const shop = database("shop")
route.post("/refund/:id", async (req) => {
  return fetch("https://api.stripe.com/v1/refunds")
})
`;
const lines = textLines(TEXT);

const touch = (kind: string, name: string, change: Touch["change"], approval: boolean, group = "outside"): Touch => ({
  group,
  kind,
  name,
  change,
  uses: [{ item: `${kind}(${name})`, what: kind === "requests" ? "" : "read", words: "", change, approval, optional: false }],
  ready: null,
  has: null,
});

describe("the review's what it may do", () => {
  it("points at the line a thing comes from: its name, quoted first", () => {
    expect(textLines("a\nb\n")).toEqual(["a", "b"]);
    expect(anchorOf({ kind: "secret", name: "stripe_key" }, lines), "past the comment naming it").toBe(6);
    expect(anchorOf({ kind: "host", name: "api.stripe.com" }, lines)).toBe(9);
    expect(anchorOf({ kind: "database", name: "shop/refunds" }, lines), "a part of its name").toBe(7);
    expect(anchorOf({ kind: "requests", name: "" }, lines), "its first route").toBe(8);
    expect(anchorOf({ kind: "clock", name: "" }, lines)).toBeNull();
  });

  it("groups it as a person decides: to approve, unchanged, no longer", () => {
    const groups = mayGroups(
      [touch("host", "api.stripe.com", "new", true), touch("requests", "", "same", false, "triggers"), touch("secret", "old_key", "gone", false, "resources")],
      [{ name: "every", now: "1m", then: "5m" }],
      lines,
    );
    expect(groups.map((g) => [g.key, g.rows.map((r) => [r.line.mark, r.line.text, r.anchor])])).toEqual([
      [
        "approve",
        [
          ["approve", "host api.stripe.com: read", 9],
          ["changed", "every: 1m → 5m", null],
        ],
      ],
      ["same", [["same", "requests", 8]]],
      ["gone", [["gone", "secret old_key: read", null]]],
    ]);
  });
});
