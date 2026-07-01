// First batch of frontend tests (previously: zero automated coverage at all — confirmed directly,
// not assumed, when the question came up). theme.ts's functions were already exported, so this
// file needed no source changes to become testable — the easiest possible starting point.

import { describe, it, expect } from "vitest";
import { typeKind, typeColor, outputColor, typeLabel, TYPE_COLOR } from "./theme";
import type { WType } from "./holon";

describe("typeKind", () => {
  it("returns scalar variants as themselves", () => {
    expect(typeKind("Number")).toBe("Number");
    expect(typeKind("Boolean")).toBe("Boolean");
    expect(typeKind("Text")).toBe("Text");
    expect(typeKind("Effect")).toBe("Effect");
    expect(typeKind("Any")).toBe("Any");
  });

  it("returns the outer constructor name for composed types, regardless of nesting", () => {
    expect(typeKind({ List: "Number" })).toBe("List");
    expect(typeKind({ List: { List: "Text" } })).toBe("List"); // nesting doesn't change the outer kind
    expect(typeKind({ Product: ["Number", "Text"] })).toBe("Product");
    expect(typeKind({ Product: [] })).toBe("Product"); // Unit = Product<>
    expect(typeKind({ Sum: ["Number", { Product: [] }] })).toBe("Sum"); // Option-shaped
  });
});

describe("typeColor / TYPE_COLOR", () => {
  it("gives every declared kind a real, distinct color", () => {
    const kinds = Object.keys(TYPE_COLOR);
    const colors = kinds.map((k) => TYPE_COLOR[k]);
    expect(new Set(colors).size).toBe(colors.length); // no two kinds silently share a hue
  });

  it("resolves a concrete type to its kind's color, composed types included", () => {
    expect(typeColor("Number")).toBe(TYPE_COLOR.Number);
    expect(typeColor({ List: "Number" })).toBe(TYPE_COLOR.List);
    expect(typeColor({ Product: ["Number", "Text"] })).toBe(TYPE_COLOR.Product);
    expect(typeColor({ Sum: ["Number", "Text"] })).toBe(TYPE_COLOR.Sum);
  });
});

describe("outputColor", () => {
  it("reads null/undefined as the neutral Any color, never a crash or a blank", () => {
    expect(outputColor(null)).toBe(TYPE_COLOR.Any);
    expect(outputColor(undefined)).toBe(TYPE_COLOR.Any);
  });

  it("otherwise defers to typeColor exactly", () => {
    const ty: WType = { List: "Boolean" };
    expect(outputColor(ty)).toBe(typeColor(ty));
  });
});

describe("typeLabel", () => {
  it("lowercases scalars, unchanged from before composed types existed", () => {
    expect(typeLabel("Number")).toBe("number");
    expect(typeLabel("Boolean")).toBe("boolean");
    expect(typeLabel("Any")).toBe("any");
  });

  it("renders composed types recursively, with the exact <> nesting syntax", () => {
    expect(typeLabel({ List: "Number" })).toBe("list<number>");
    expect(typeLabel({ List: { List: "Text" } })).toBe("list<list<text>>");
    expect(typeLabel({ Product: ["Number", "Text"] })).toBe("product<number, text>");
    expect(typeLabel({ Product: [] })).toBe("product<>"); // Unit
    expect(typeLabel({ Sum: ["Number", { Product: [] }] })).toBe("sum<number, product<>>"); // Option<Number>
  });
});
