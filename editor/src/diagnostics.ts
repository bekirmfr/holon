// Pure diagnostic/labeling helpers, extracted out of App.tsx (2026-07-02) so they can be tested
// without pulling in App.tsx's own top-level value import of Evaluator from ./holon — which
// itself imports the actual wasm package (../wasm/holon_wasm.js, built for --target web,
// browser-oriented). Only TYPES are imported from ./holon below; TypeScript erases type-only
// imports entirely, so this module has zero runtime dependency on holon.ts or the wasm package.
//
// BOUNDARY is inlined rather than imported for the same reason: it's a real value (not a type),
// and even a single real-value import would force holon.ts's whole module — including the wasm
// import — to evaluate. Matches core's own u32::MAX; structurally fixed, not at risk of drifting.

import type { WPort, WEdge, WEncapError, WValidationError, WCapability, WValue } from "./holon";

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

// The exact string a WCapability would cross as in holon-server's own `acknowledge` field —
// `{c:?}` (Rust Debug) on the real Capability, e.g. "Clock" or `StateRead("counter")`. One
// decider for this formatting: every caller that needs an acknowledge string (the deploy UI,
// InspectPanel's capability readout) goes through this, rather than each hand-building the
// same string differently. Lives here, not holon.ts, for the same reason every other function
// in this file does: holon.ts has a top-level VALUE import of the wasm package, so even one
// pure function living there would drag that in for anything that imports it — tests included.
export function capabilityAcknowledgeName(c: WCapability): string {
  if (typeof c === "string") return c;
  if ("StateRead" in c) return `StateRead("${c.StateRead}")`;
  if ("HttpRequest" in c) return `HttpRequest("${c.HttpRequest}")`;
  if ("SecretRead" in c) return `SecretRead("${c.SecretRead}")`;
  if ("Await" in c) return `Await("${c.Await}")`;
  if ("CollectionRead" in c) return `CollectionRead("${c.CollectionRead}")`;
  if ("CollectionWrite" in c) return `CollectionWrite("${c.CollectionWrite}")`;
  return `StateWrite("${c.StateWrite}")`;
}

// Render an evaluated value for the readout. This is core's *pure* evaluator, so a sink shows
// as the Effect it constructs (it doesn't fire here), and a driver / unparsed-const graph comes
// back null. Moved out of App.tsx (this inch) for the same reason every other function in this
// file was: pure, no wasm dependency, testable in isolation — and worth actually testing, since
// the Effect case just shipped a real, silent bug (dropping the payload entirely, showing only
// the sink name) that sat unnoticed until someone built a state-write flow and looked closely
// at the result. That's exactly the class of regression this file exists to catch early instead
// of by manual review.
/**
 * A value as a chip on a canvas port: short by construction, so it never needs cutting mid-value
 * and never sizes anything. Text is quoted (cut at 16 characters), a record or list says what it
 * holds, a secret or token stays its placeholder's kind. `null` (absent) is a dash.
 */
export function valueChip(v: WValue | null): string {
  if (v === null) return "—";
  switch (v.type) {
    case "Number":
      return String(Number.isInteger(v.value) ? v.value : Number(v.value.toPrecision(6)));
    case "Boolean":
      return String(v.value);
    case "Text": {
      const placeholder = /^‹(secret|token):[^›]*›$/.exec(v.value);
      if (placeholder) return `‹${placeholder[1]}›`;
      return v.value.length > 16 ? `"${v.value.slice(0, 15)}…"` : JSON.stringify(v.value);
    }
    case "Record":
      return `{${v.value.length} field${v.value.length === 1 ? "" : "s"}}`;
    case "List":
      return `[${v.value.length} item${v.value.length === 1 ? "" : "s"}]`;
    case "Effect":
      return `effect ${v.value.sink.split(":")[0]}`;
    case "Sum":
      return v.value.arm === 0 ? `some ${valueChip(v.value.payload)}` : "none";
  }
}

export function formatValue(v: WValue): string {
  switch (v.type) {
    case "Number":
      return String(v.value);
    case "Boolean":
      return v.value ? "true" : "false";
    case "Text":
      return `"${v.value}"`;
    case "List":
      return `[${v.value.map(formatValue).join(", ")}]`;
    case "Effect":
      return `effect(${v.value.sink}, ${formatValue(v.value.payload)})`;
    case "Sum":
      // No core signal says which arm means "Some" vs "None" (a value carries only its taken
      // arm's index, never a name — the same honesty theme.ts's typeLabel already holds to for
      // the TYPE side, not guessing "this looks like Option"). Show the arm plainly instead of
      // guessing.
      return `sum#${v.value.arm}(${formatValue(v.value.payload)})`;
    case "Record":
      // Same shape as core's to_text: `{id: 7, title: "buy milk"}`, fields in key order.
      return `{${v.value.map(([k, x]) => `${k}: ${formatValue(x)}`).join(", ")}}`;
  }
  // Exhaustiveness: this project's tsconfig doesn't enable noImplicitReturns, so a missing case
  // here was previously a silent runtime "undefined", not a compile error — exactly the gap
  // that let `Sum` go unhandled since it was added. A future WValue shape is now a compile
  // error instead.
  const exhaustive: never = v;
  return exhaustive;
}

// Render a fire/trigger's WHOLE outputs array — every settled slot a unit declared, not just
// one, now that fire (holon-host) genuinely supports multi-output roots instead of refusing
// them. Preserves the exact prior single-output wording for the common case: an all-null
// array (including the one-slot case that always used to just say this) still reads
// "(no output)", and a single real value still reads as bare formatValue(v), no separators —
// nothing about the common case changes shape just because the type widened under it. Only a
// GENUINELY multi-output settle (more than one slot, or a mix of real/absent) shows the
// comma-joined form, since that's the only case where there's more than one thing to show.
export function formatOutputs(outputs: (WValue | null)[]): string {
  if (outputs.every((o) => o === null)) return "(no output)";
  return outputs.map((o) => (o ? formatValue(o) : "(none)")).join(", ");
}

/** A one-line summary of a deployment's state for a session row: lists and records by size,
 *  other values as themselves (cut short). `todos: 5 items, count: 3`. */
export function summarizeState(state: Record<string, WValue> | null): string {
  if (!state) return "no saved state";
  const keys = Object.keys(state).sort();
  if (keys.length === 0) return "empty";
  const one = (v: WValue): string => {
    if (v.type === "List") return `${v.value.length} item${v.value.length === 1 ? "" : "s"}`;
    if (v.type === "Record") return `${v.value.length} field${v.value.length === 1 ? "" : "s"}`;
    const s = formatValue(v);
    return s.length > 24 ? `${s.slice(0, 23)}…` : s;
  };
  return keys.map((k) => `${k}: ${one(state[k])}`).join(", ");
}
/**
 * A manifest item (permissions.rs's names: `HttpRequest("a.com")`, `Limit(max_requests=80)`, …)
 * as a person reads it on the consent screen. An item this doesn't know is shown as it is.
 */
export function describeItem(item: string): string {
  const arg = (prefix: string) => {
    if (!item.startsWith(`${prefix}(`) || !item.endsWith(")")) return null;
    const inner = item.slice(prefix.length + 1, -1);
    return inner.startsWith('"') && inner.endsWith('"') ? inner.slice(1, -1) : inner;
  };
  const named: [string, (a: string) => string][] = [
    ["HttpRequest", (a) => `call https://${a}`],
    ["StateRead", (a) => `read its state "${a}"`],
    ["StateWrite", (a) => `write its state "${a}"`],
    ["CollectionRead", (a) => `read items of its collection "${a}"`],
    ["CollectionWrite", (a) => `put and delete items of its collection "${a}"`],
    ["PlainHttp", (a) => `call ${a} over plain HTTP (not encrypted)`],
    ["SecretRead", (a) => `read the secret "${a}"`],
    ["Await", (a) => `wait for an event "${a}" and continue later`],
    ["Limit", (a) => `raise ${a.replace("=", " to ")}`],
  ];
  for (const [prefix, say] of named) {
    const a = arg(prefix);
    if (a !== null) return say(a);
  }
  const plain: Record<string, string> = {
    NetworkIngress: "receive requests",
    NetworkEgress: "send outbound messages",
    Clock: "read the clock",
    Random: "use randomness",
    PrivateAddresses: "reach private addresses (this machine and its network)",
    SecretAccess: "read secrets",
    FileRead: "read files",
    FileWrite: "write files",
    Spawn: "start processes",
  };
  return plain[item] ?? item;
}
