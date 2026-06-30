# `primitive_catalog()` bridge — the palette's source crosses to JS (L4)

**Status:** short note for a small `holon-wasm` inch. Exposes core's genesis interface catalog
(`catalog::genesis_units`) to the editor, so the next inch's palette is *derived* from core, not
hand-written in TypeScript.

---

## What it adds

One `#[wasm_bindgen]` function:

```
primitive_catalog() -> JsValue   // an array of WUnit, one per §13a genesis leaf
```

It calls `holon_core::genesis_units()` and mirrors each `Unit` to `WUnit`, then serializes the `Vec`
with `serde_wasm_bindgen::to_value` — exactly the shape `load_unit` already returns for a single unit.
The editor reads this once, gets all 34 typed interfaces (names, ports, output types, default
params), and builds both its palette list and its name→interface map from it.

## Two decisions worth recording

- **Reuse `WUnit::from_core`, don't write a second converter.** `load_unit` already turns a core
  `Unit` into a `WUnit`; `primitive_catalog` reuses that exact function over the catalog. A second
  core→JS conversion would be a parallel mechanism free to drift from the first — the precise thing
  the one-encoder/one-evaluator discipline forbids. The mirror stays singular.

- **One whole-catalog call, not per-name.** `primitive_catalog()` returns the entire set in one
  crossing rather than a `primitive_interface(name)` lookup. The palette needs all of them at once to
  render; a per-name bridge call would be N crossings for the same data and tempt the editor to cache
  its own copy (drift again). If a single-name need ever appears, the editor filters the list it
  already holds — no new boundary.

## Scope held

This is the *bridge only*. The palette UI (custom nodes, drag-to-place, the type-checked wiring) is
the next inch, where it consumes this. Nothing here renders anything; it makes the catalog reachable
from JS and proves it crosses intact.

## Tests

- **`primitive_catalog_native`** (native): the Rust-side mirror is correct — `catalog_wunits()` has
  34 entries, and a spot-checked interface survives the `Unit`→`WUnit` mirror (`add` =
  `(a:Number, b:Number) → Number`; `map` keeps its open `fn` param). Fast, runs in the native suite.
- **`primitive_catalog_crosses_js_boundary`** (wasm): the real `#[wasm_bindgen]` `primitive_catalog()`
  is called, the `JsValue` deserialized back to `Vec<WUnit>`, and checked (34 entries, `add` present)
  — proving the `Vec<WUnit>` array actually serializes across the boundary, not just that the Rust
  builds it. This is the new risk a single-`WUnit` `load_unit` test didn't cover.

Correctness of the *interfaces themselves* is already owned by core's catalog tests
(`the_two_catalogs_agree`, `catalogued_units_evaluate`); these two only prove the crossing.

## Invariants

I1 (interface parity): the editor and core read one catalog, so they can't disagree about an atom.
I8 (derive, don't assert): the palette is derived from this, not asserted in TS.
