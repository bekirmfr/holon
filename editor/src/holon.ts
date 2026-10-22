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
  variadic_primitives,
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
  | "Record"
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

/** A complete unit. `params`/`inputs`/`outputs` may be omitted (default empty). */
export type WUnit = {
  body: WBody;
  params?: WParam[];
  inputs?: WPort[];
  outputs?: WPort[];
};

/**
 * A structural validation failure. Mirrors `core::ValidationError` via the bridge's
 * `WValidationError`. Every variant carries a `message` (core's own wording — one decider,
 * not a second copy hand-maintained here) plus the structured fields the editor needs to
 * find the actual node(s) at fault: `child`/`edge` indices the editor already has local
 * data for (it built the `children`/`edges` arrays it sent to `project`), so it resolves
 * them back to a specific placed node without core needing to know anything about the
 * canvas. `src`/`dst` on `TypeMismatch` cross as real `WType`s, not text, so they render
 * with the same `typeLabel` every other type in the editor uses.
 */
export type WValidationError =
  | { kind: "ChildOutOfRange"; edge: number; child: number; message: string }
  | { kind: "UnresolvedChild"; child: number; message: string }
  | { kind: "OutputPortOutOfRange"; edge: number; child: number; port: number; message: string }
  | { kind: "InputPortOutOfRange"; edge: number; child: number; port: number; message: string }
  | { kind: "BoundaryInputOutOfRange"; edge: number; port: number; message: string }
  | { kind: "BoundaryOutputOutOfRange"; edge: number; port: number; message: string }
  | { kind: "DuplicateInputEdge"; to_child: number; to_port: number; message: string }
  | { kind: "Cycle"; message: string }
  | { kind: "TypeMismatch"; edge: number; src: WType; dst: WType; message: string };

/** A promotion choice: which inner (child, port) input becomes a boundary input, and what
 *  it's named. Mirrors `core::Promote`. */
export type WPromote = { child: number; port: number; name: string };

/** An exposure choice: which inner (child, port) output becomes a boundary output, and what
 *  it's named. Mirrors `core::Expose`. */
export type WExpose = { child: number; port: number; name: string };

/**
 * The canonical encapsulation proposal. Mirrors `core::EncapSpec` via the bridge's
 * `WEncapSpec` (P4.3, fork 3). Returned by {@link Evaluator.propose_encapsulation} — core's
 * `default_spec`, crossed intact — for a person to review before freezing. Editing `expose`
 * (dropping entries) excludes those outputs from the final composite's interface when the
 * edited spec is passed to {@link Evaluator.freeze_encapsulation}. `promote` isn't editable
 * yet in this editor: no UI exists to rename or reorder promoted inputs either, so exposing
 * more editing power on outputs than inputs currently have would be asymmetric for no reason.
 */
export type WEncapSpec = { promote: WPromote[]; expose: WExpose[] };

/**
 * An encapsulation failure — mirrors `core::EncapError` via `WEncapError`. Thrown by
 * {@link Evaluator.project} (as the structured object itself, not wrapped in a generic
 * `Error`) so the editor can highlight the actual offending node(s) instead of just
 * displaying text.
 *
 * No `AmbiguousDefaultOutput` (P4.3, fork 2): `default_spec` now proposes every free output
 * automatically instead of erroring when there's more than one, so that variant is gone —
 * dead code removed on the Rust side, not just unused here.
 */
export type WEncapError =
  | { kind: "UnresolvedChild"; child: number; message: string }
  | { kind: "PromoteChildOutOfRange"; child: number; message: string }
  | { kind: "PromotePortOutOfRange"; child: number; port: number; message: string }
  | { kind: "PromoteAlreadyBound"; child: number; port: number; message: string }
  | { kind: "DuplicatePromotion"; child: number; port: number; message: string }
  | { kind: "ExposeChildOutOfRange"; child: number; message: string }
  | { kind: "ExposePortInvalid"; child: number; port: number; message: string }
  | { kind: "Invalid"; errors: WValidationError[]; message: string };

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
  | { type: "Sum"; value: { arm: number; payload: WValue } }
  /** Fields as `[key, value]` pairs in key order (a JS `Map` would not cross serde-wasm-bindgen as
   *  a plain object). */
  | { type: "Record"; value: [string, WValue][] };

/** A category of effect a leaf may touch, outside its inputs. Mirrors `core::Capability` via
 *  the bridge's `WCapability` (P4.4.1, keyed as of the keyed-state-capability inch). Every
 *  variant crosses as a bare string (`"Clock"`) via serde's default external tagging, except
 *  `StateRead`/`StateWrite`, which now carry their state key and cross as a single-key object
 *  (`{"StateRead": "var1"}`) — the identical convention {@link WType}'s own nullary-vs-payload
 *  variants already use, needing no extra parsing here beyond the type itself. This was stale
 *  until now (still the old bare-string shape) even though the Rust side changed earlier —
 *  caught only when this file was actually read again for the deploy-UI work, not by anything
 *  that had been exercising it: nothing in the editor called `Evaluator.capabilities()` on a
 *  state-bearing unit before now. (`capabilityAcknowledgeName`, the function that formats one
 *  of these for the deploy UI, lives in `./diagnostics` instead of here — the same reason
 *  `boundaryLabels`/etc. do: this file has a top-level *value* import of the wasm package, so
 *  even a single pure function living here would drag that in for anything that imports it,
 *  tests included.)*/
export type WCapability =
  | "Clock"
  | "NetworkIngress"
  | "FileRead"
  | "SecretAccess"
  | { SecretRead: string }
  | { Await: string }
  | "Random"
  | { StateRead: string }
  | { StateWrite: string }
  | { CollectionRead: string }
  | { CollectionWrite: string }
  | "NetworkEgress"
  | { HttpRequest: string }
  | "FileWrite"
  | "Spawn";

/** The coarse effect bucket a unit belongs to. Mirrors `core::Kind` via the bridge's `WKind` —
 *  the fixed projection of {@link WCapability} above, never computed in TypeScript (one
 *  decider): `Evaluator.effects_of` crosses this directly rather than the editor deriving it
 *  from a capability list itself. */
export type WKind = "Pure" | "Driver" | "State" | "Sink";

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
 * The genesis primitive names whose *placed* port count the editor may grow past the catalog's
 * declared default — core's 4th catalog fact (`catalog::VARIADIC`), derived from whether each
 * leaf's `compute` genuinely tolerates any input count. A `Set` (not an array) since the only
 * use is membership testing — `variadicPrimitives().has(name)`. Call after {@link ensureReady}.
 */
export function variadicPrimitives(): Set<string> {
  return new Set(variadic_primitives() as string[]);
}

/**
 * Wire-time type compatibility (§5), answered by core's `validate::compatible` across the bridge —
 * never a TS reimplementation, so the canvas and core can't disagree about what may connect (I1).
 * `src` is the upstream output type, `dst` the downstream input type. Call after {@link ensureReady}.
 */
export function compatible(src: WType, dst: WType): boolean {
  return wasm_compatible(src, dst) as boolean;
}