// Pure diagnostic/labeling helpers, extracted out of App.tsx (2026-07-02) so they can be tested
// without pulling in App.tsx's own top-level value import of Evaluator from ./holon — which
// itself imports the actual wasm package (../wasm/holon_wasm.js, built for --target web,
// browser-oriented). Only TYPES are imported from ./holon below; TypeScript erases type-only
// imports entirely, so this module has zero runtime dependency on holon.ts or the wasm package.
//
// BOUNDARY is inlined rather than imported for the same reason: it's a real value (not a type),
// and even a single real-value import would force holon.ts's whole module — including the wasm
// import — to evaluate. Matches core's own u32::MAX; structurally fixed, not at risk of drifting.

import type { WPort, WEdge, WEncapError, WValidationError } from "./holon";

const BOUNDARY = 4294967295;

// default_spec names each promoted/exposed port after its inner port's own name, with no
// uniqueness guarantee across different children — two unrelated unwired "b" ports (extremely
// common; "a"/"b" are this catalog's standard binary-op names) can promote to the same bare
// name. This prefers TRUE provenance ("add · b") over the bare name — the composite's own
// boundary edges (from_child === BOUNDARY) carry exactly that mapping, computed by
// derive_and_freeze and returned in composite.body. Occurrence-counting is only a last-resort
// tiebreaker for the one case even provenance can't separate: two placed instances of the SAME
// primitive, unconfigured, hence the SAME content hash (I3 — identical computation genuinely IS
// the same identity, not a gap), both left unwired on the same port.
export function boundaryLabels(ports: WPort[], source: (string | null)[]): string[] {
  const withProvenance = ports.map((p, i) => (source[i] ? `${source[i]} · ${p.name}` : p.name));
  const seen = new Map<string, number>();
  return withProvenance.map((label) => {
    const n = (seen.get(label) ?? 0) + 1;
    seen.set(label, n);
    return n === 1 ? label : `${label} (${n})`;
  });
}

// project() throws the structured error object itself on an EncapError/ValidationError path
// (not a generic Error) — this is a loose but adequate runtime check: nothing else in this app
// produces an object with both a string `kind` and a string `message`. It doesn't verify every
// field matches a known variant; an unrecognized `kind` would fail the exhaustiveness check
// below at the type level, and the try/catch around the caller keeps that from crashing anything
// user-visible — highlighting is a nicety, never allowed to break error display itself.
export function isWEncapError(x: unknown): x is WEncapError {
  return typeof x === "object" && x !== null && "kind" in x && "message" in x;
}

// Which PLACED NODE indices (into `children`/`nodes` — they share the same order) a
// ValidationError implicates. Most variants carry a `child` directly. `TypeMismatch` only
// carries an `edge` index — resolved here via the SAME `wedges` array the editor already built
// and submitted, so no core-side change was needed just to turn an edge index back into its
// endpoints. `Cycle` and the two `Boundary*OutOfRange` variants carry no attributable node at
// all — see WValidationError's own doc comment for why that's a real, named limit, not an
// oversight: tracing which edges form a cycle is new algorithmic work, out of this inch's scope.
export function validationErrorNodeIndices(err: WValidationError, wedges: WEdge[]): number[] {
  switch (err.kind) {
    case "ChildOutOfRange":
    case "UnresolvedChild":
    case "OutputPortOutOfRange":
    case "InputPortOutOfRange":
      return [err.child];
    case "DuplicateInputEdge":
      return [err.to_child];
    case "TypeMismatch": {
      const e = wedges[err.edge];
      return e ? [e.from_child, e.to_child].filter((c) => c !== BOUNDARY) : [];
    }
    case "BoundaryInputOutOfRange":
    case "BoundaryOutputOutOfRange":
    case "Cycle":
      return [];
  }
  const exhaustive: never = err;
  return exhaustive;
}

// Same idea, one level up: an EncapError's own `child` fields, or — for `Invalid` — every
// wrapped ValidationError's indices, flattened. No `AmbiguousDefaultOutput` case anymore
// (P4.3, fork 2): default_spec no longer errors on multiple free outputs, so that variant —
// and the candidate-list highlighting it drove — is gone on both sides now.
export function encapErrorNodeIndices(err: WEncapError, wedges: WEdge[]): number[] {
  switch (err.kind) {
    case "UnresolvedChild":
    case "PromoteChildOutOfRange":
    case "PromotePortOutOfRange":
    case "PromoteAlreadyBound":
    case "DuplicatePromotion":
    case "ExposeChildOutOfRange":
    case "ExposePortInvalid":
      return [err.child];
    case "Invalid":
      return err.errors.flatMap((e) => validationErrorNodeIndices(e, wedges));
  }
  const exhaustive: never = err;
  return exhaustive;
}
