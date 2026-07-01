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

/**
 * A wire type. Mirrors `core::Type` (as `WType`) — now recursive (P1.3.2 editor inch): `List`,
 * `Product`, and `Sum` carry their element type(s). This mirrors serde's own DEFAULT shape for a
 * mixed Rust enum (no `#[serde(tag=...)]` on the Rust side): a nullary variant crosses as a bare
 * string (`"Number"`), a variant carrying data crosses as a single-key object
 * (`{ List: "Number" }`, `{ Product: ["Number", "Text"] }`, `{ Sum: [T, { Product: [] }] }` for
 * `Option<T>`). Every scalar-only comparison in the editor (`ty === "Number"`, the `const`-type
 * dropdown, …) keeps working unchanged — only the three composed shapes changed, from a bare tag
 * to a tagged object, because they're the only variants that now carry a payload.
 */
export type WType =
  | "Number"
  | "Boolean"
  | "Text"
  | "Effect"
  | "Any"
  | { List: WType }
  | { Product: WType[] }
  | { Sum: WType[] };

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

/** A runtime value crossing the boundary. Mirrors `core::Value` (as `WValue`). `Sum` mirrors
 *  `Value::Sum { arm, payload }` — `arm` says which alternative was taken, `payload` is its value;
 *  the untaken alternatives aren't carried (a tagged choice, not a tuple — see `Type::Sum`/`WType`'s
 *  `Sum` case, the type-level sibling of this). */
export type WValue =
  | { type: "Number"; value: number }
  | { type: "Boolean"; value: boolean }
  | { type: "Text"; value: string }
  | { type: "List"; value: WValue[] }
  | { type: "Effect"; value: { sink: string; payload: WValue } }
  | { type: "Sum"; value: { arm: number; payload: WValue } };

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