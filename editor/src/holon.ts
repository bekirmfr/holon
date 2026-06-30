// Typed facade over the holon-wasm bridge.
//
// These TypeScript types mirror the Rust serde types in `crates/holon-wasm/src/lib.rs` EXACTLY.
// They are discriminated unions, so the compiler checks them the same way the Rust side matches
// exhaustively — the whole reason the editor is TypeScript and not plain JS. The wasm-pack-generated
// `.d.ts` types `build_preimage`/`eval` as `any` (they cross a JsValue boundary), so this file is
// where that boundary becomes typed.
//
// The `wasm/` package is generated — never edit it. Rebuild it whenever the Rust bridge changes:
//   cd crates/holon-wasm && wasm-pack build --target web --out-dir ../../editor/wasm

import init, {
  Evaluator,
  build_preimage,
  primitive_catalog,
  compatible as wasm_compatible,
} from "../wasm/holon_wasm.js";

export { Evaluator };

/** A wire type. Mirrors `core::Type` (as `WType`). */
export type WType = "Number" | "Boolean" | "Text" | "List" | "Effect" | "Any";

/** A parameter: a frozen value, or an open hole filled at instantiate time. Mirrors `core::Param`. */
export type WParam =
  | { kind: "Frozen"; name: string; value: string }
  | { kind: "Open"; name: string; bind: string };

/** A named, typed interface port. Mirrors `core::Port`. */
export type WPort = { name: string; ty: WType };

/**
 * An internal wire of a composite, addressed by child *index*. `from_child`/`to_child` === 4294967295
 * (u32::MAX, {@link BOUNDARY}) is the composite's own interface. Mirrors `core::Edge`.
 */
export type WEdge = { from_child: number; from_port: number; to_child: number; to_port: number };

/** What a unit is: a named primitive, or a composite of children (hex hashes) wired together. */
export type WBody =
  | { kind: "Primitive"; name: string }
  | { kind: "Composite"; children: string[]; edges: WEdge[] };

/** A complete unit. `params`/`inputs`/`output` may be omitted (default empty / null). */
export type WUnit = {
  body: WBody;
  params?: WParam[];
  inputs?: WPort[];
  output?: WType | null;
};

/** A runtime value crossing the boundary. Mirrors `core::Value` (as `WValue`). */
export type WValue =
  | { type: "Number"; value: number }
  | { type: "Boolean"; value: boolean }
  | { type: "Text"; value: string }
  | { type: "List"; value: WValue[] }
  | { type: "Effect"; value: { sink: string; payload: WValue } };

/** The composite's own boundary index (u32::MAX) — its inputs feed children; one child feeds out. */
export const BOUNDARY = 4294967295;

let ready: Promise<void> | null = null;

/** Load the wasm module once. Safe to call repeatedly; the actual init runs a single time. */
export function ensureReady(): Promise<void> {
  if (ready === null) {
    ready = init().then(() => undefined);
  }
  return ready;
}

/** Compile a unit to its canonical preimage bytes through the ONE core encoder. */
export function buildPreimage(unit: WUnit): Uint8Array {
  return build_preimage(unit);
}

/**
 * The authoritative interface of every genesis primitive (§13a), straight from core — the source the
 * palette is derived from, never a hand-written copy (I1). Call after {@link ensureReady}.
 */
export function primitiveCatalog(): WUnit[] {
  return primitive_catalog() as WUnit[];
}

/**
 * Wire-time type compatibility (§5), answered by core's `validate::compatible` across the bridge —
 * never a TS reimplementation, so the canvas and core can't disagree about what may connect (I1).
 * `src` is the upstream output type, `dst` the downstream input type. Call after {@link ensureReady}.
 */
export function compatible(src: WType, dst: WType): boolean {
  return wasm_compatible(src, dst) as boolean;
}