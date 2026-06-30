// The editor's one bold, grounded choice: a wire type *is* a color. Both the palette and the canvas
// nodes read a port's hue from here, so the whole editor's color is derived from core's type system —
// and a legal wire (next inch) will be two ports of the same hue (or `Any`, the neutral). See
// P4.1-palette-L4 §4. Structural colors (background, hairlines, ink) live as CSS vars in styles.css;
// only the type hues are data-driven, so they live in TypeScript where the data is.

import type { WType } from "./holon";

/** A wire type's hue. Six types, six colors — the editor's whole palette comes from this. */
export const TYPE_COLOR: Record<WType, string> = {
  Number: "#f2b441", // amber
  Boolean: "#46c8d8", // cyan
  Text: "#5fd38a", // green
  List: "#b48cf0", // violet
  Effect: "#ff6f6f", // coral — the effectful edge
  Any: "#8295b0", // slate — polymorphic, uncommitted
};

/** The hue for an output slot; `null` (no output) reads as the neutral. */
export function outputColor(ty: WType | null | undefined): string {
  return ty ? TYPE_COLOR[ty] : TYPE_COLOR.Any;
}
