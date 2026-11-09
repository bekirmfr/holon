// The editor's one bold, grounded choice: a wire type *is* a color. Both the palette and the canvas
// nodes read a port's hue from here, so the whole editor's color is derived from core's type system —
// and a legal wire is two ports of the same hue (or `Any`, the neutral). See P4.1-palette-L4 §4.
// Each hue is a token in styles.css (--type-<kind>), so it has a light and a dark value; which kind
// takes which token is decided here, where the types are.
//
// `WType` is recursive (P1.3.2 editor inch): `List`/`Product`/`Sum` carry their element type(s), so
// a wire type is no longer always a bare string. This file is where the editor WALKS that shape to
// draw it — pick a hue, render a label — and only that (I1 discipline): it never uses the shape to
// DECIDE whether a wire is legal. That judgment stays exactly one call: `compatible()`, in holon.ts,
// which asks core. Nothing here re-derives or shortcuts that answer.

import type { WType } from "./holon";

/** The outer constructor of a type, regardless of nesting — `"Number"` for a scalar, `"List"` for
 *  any `List<T>`, and so on. This is a STRUCTURAL read for rendering (which hue, which bracket),
 *  never a compatibility judgment — see the file header. */
export function typeKind(ty: WType): string {
  if (typeof ty === "string") return ty;
  if ("List" in ty) return "List";
  if ("Product" in ty) return "Product";
  if ("Sum" in ty) return "Sum";
  // Exhaustiveness: if `WType` ever grows a new shape, this is a compile error here, not a silent
  // `undefined` color at runtime — the same guarantee Rust's `match` gives the bridge.
  const exhaustive: never = ty;
  return exhaustive;
}

/** A type's hue, by outer kind. Ten kinds, ten colors — the editor's whole palette comes from
 *  this. `Product`/`Sum` are new hues (this inch); everything else is unchanged from before. */
export const TYPE_COLOR: Record<string, string> = {
  Number: "var(--type-number)", // amber
  Boolean: "var(--type-boolean)", // cyan
  Text: "var(--type-text)", // green
  List: "var(--type-list)", // violet
  Product: "var(--type-product)", // rose — a tuple, adjacent to List's violet but visibly distinct
  Sum: "var(--type-sum)", // blue — the one hue family nothing else was using
  Effect: "var(--type-effect)", // coral — the effectful edge
  Any: "var(--type-any)", // slate — polymorphic, uncommitted
  Record: "var(--type-record)", // orange — named fields, distinct from Number's amber
  Bytes: "var(--type-bytes)", // sand — raw bytes, a file's or a digest's, muted beside Text's green
};

/** The hue for any concrete wire type (an input port's type, e.g.) — never `null`. */
export function typeColor(ty: WType): string {
  return TYPE_COLOR[typeKind(ty)];
}

/** The hue for an output slot; `null`/`undefined` (no output) reads as the neutral. */
export function outputColor(ty: WType | null | undefined): string {
  return ty ? typeColor(ty) : TYPE_COLOR.Any;
}

/** A short, human label for a type, composed types included: `"number"`, `"list<number>"`,
 *  `"product<number, text>"`, `"sum<number, product<>>"`. Lowercase throughout, matching the
 *  editor's existing single-word convention (`output.toLowerCase()`) so a scalar's label is
 *  unchanged and only a composed type gains the `<…>` detail — the "hue + inner in the label"
 *  the L4 asks for. Purely a rendering choice — it never affects what `compatible()` decides. */
export function typeLabel(ty: WType): string {
  if (typeof ty === "string") return ty.toLowerCase();
  if ("List" in ty) return `list<${typeLabel(ty.List)}>`;
  if ("Product" in ty) return `product<${ty.Product.map(typeLabel).join(", ")}>`;
  if ("Sum" in ty) return `sum<${ty.Sum.map(typeLabel).join(", ")}>`;
  const exhaustive: never = ty;
  return exhaustive;
}