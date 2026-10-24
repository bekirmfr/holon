# WebAssembly modules as units — Level-4 design (the vocabulary's escape hatch)

> *crate:* **`holon-core`** (a third unit body, its validation, a runner hook) **+ a new
> `holon-module`** (the engine) **+ `holon-host` / `holon-wasm`** (wiring) · *pre:* none; the
> language (`docs/LANGUAGE.md`) gains a `module` statement after · *inv:* `I1` / `I2` / `I5` /
> `I6` / `I8` · *effort:* **L** (four inches, about two weeks)
> *Status:* **designed, deliberately not built** (2026-10-24). §7 says what triggers the build.
> *Pinned:* a module is a **unit body**, not a primitive with a blob param · **no imports** ·
> **fuel and memory as frozen params** · engine **out of core**, behind a `Resolver` hook.

---

## 0. What this is, and why it is written down before it is needed

Holon's vocabulary is closed on purpose: 70 genesis leaves, composites over them, and the
admission gate (design §13) for anything else. That closure is what makes the manifest derivable
and replay exact. It also has a ceiling, and the day a real flow hits it, the honest answer is not
a 71st leaf for that one case but a way to run *code*, under the same guarantees.

A WebAssembly module with **no imports** is that way. It cannot read a clock, a file, the network
or anything but its arguments; it is deterministic by specification (one gap, §3); it is a byte
string, so it is content-addressed by the same hash as everything else; and under a fuel budget it
is total. So it fits the invariants without bending any of them, provided the decisions below are
taken as written. This note takes them now, while the reasoning is fresh, so that whoever builds
it, in whatever session, builds the same thing. It builds nothing.

## 1. A module is a unit (I1)

`UnitBody` gains a third variant:

```
UnitBody::Primitive { name }
UnitBody::Composite { children, edges }
UnitBody::Module    { bytes }          # the WebAssembly binary, as is
```

with ports and params like any unit. From the outside a module is indistinguishable from a
primitive or a composite: it is placed, wired, grouped into holons, and placed in routes the same
way, and `eval` is still the one entry point. This is the spine of the design (interface parity),
and it is why the alternative, a `wasm` *primitive* whose param is the hash of a separate blob, is
refused: that would add a second blob kind to the store, the closure walk (`closure_of`),
inspection and the inspector, and buy nothing.

**Consequences that come free:**

- **Content addressing (I5).** The bytes are in the preimage, so the module's hash is its
  identity. Verify-on-read in the store is unchanged: a blob is trusted because it re-hashes.
- **Transitive pinning (I6).** A composite that places a module pins its hash like any child. The
  flow document's `units` closure carries it.
- **Inspection (P3.2).** Descending to the leaves reaches a module and stops at it, as at a
  primitive; what it shows is the module's interface, size and limits (§4). A module is a black
  box to replay: inputs in, outputs out. That is a real cost, named in §7.

## 2. Encoding: a new tag, no version bump

`hash.rs` prefixes every preimage with `ENCODING_VERSION` (5 today) and tags bodies
`TAG_PRIMITIVE = 0x01`, `TAG_COMPOSITE = 0x02`. A module is `TAG_MODULE = 0x03`, followed by the
length-prefixed bytes. **The version byte is not bumped.** It is in every preimage, so a bump
would change every hash of every deployed unit; a new tag changes nothing already encoded, and an
older decoder meeting the tag fails closed with its existing unknown-tag error, which is the
right behaviour for a peer that does not know modules. The rule to record in `hash.rs`: *a new
body tag is a compatible extension; a change to how an existing tag encodes is a version bump.*

`decode_preimage` gains the arm; `encode(decode(b)) = b` holds as for the other bodies.

## 3. Admission: what a module must be to be a unit at all

Validation runs where units are built and stored (core's `validate`, so the editor, the CLI and
the host all refuse the same things), by parsing the binary's own sections. This is a few hundred
lines of LEB128 and section walking, in core, with no dependency: core must be able to say `Pure`
about a module without trusting a crate it cannot see into.

1. **No imports.** The import section is empty. Not a clock, not a logger, not a host memory.
   Anything the module needs from outside arrives as an input value; anything it wants done
   leaves as an output a flow wires to a sink. This single rule is what makes `kind_of(module) =
   Pure` a fact rather than a label, and it is the rule most likely to be "relaxed" later for
   convenience. Do not.
2. **The ABI's exports, exactly.** One exported memory `memory`; `alloc(len: i32) -> i32`;
   `run(ptr: i32, len: i32) -> i64` (the high 32 bits the output pointer, the low 32 its
   length). Nothing else is required and other exports are ignored.
3. **Values cross as core's canonical value encoding.** The host encodes the inputs as one
   `List` of the input values with `encode_value`, writes it at `alloc(len)`, calls `run`, and
   decodes the returned bytes with `decode_value` as one `List` of outputs, one per declared
   output port. Absence is a `Sum` arm the encoding already has (the optional's `none`): an
   absent input is `none`, an absent output is `none`. No JSON, no second format: the one
   encoding that already defines equality and the memo key defines the ABI.
4. **Determinism, closed at the edge.** WebAssembly's one nondeterminism is the payload of a NaN
   produced by some float operations. A NaN, or any non-finite number, has no form in the value
   encoding (`const` refuses it, `is_finite_number` is the rule), so a module that produces one
   produces an output that fails to decode, which is absent. Observable behaviour is therefore
   the same on every engine and every machine. This is the same move `coalesce`'s design made:
   close the gap at the encoding, not inside the engine.
5. **Totality, by budget.** Two frozen params, `fuel` (instructions, metered by the engine) and
   `memory` (pages or bytes, the cap on `memory.grow`). A module that runs out of either produces
   nothing. Both are in the preimage, so the same module under two budgets is two units, and a
   result is reproducible because its budget is part of its identity. Defaults to pin when
   building: 10 000 000 fuel and 16 MiB, enough for real code, done in milliseconds.
6. **Types.** Ports are declared when the module is added (§6) and frozen into the unit; the ABI
   carries values, not types, so wire-time checking is the existing compatibility check over the
   declared ports, and a value of the wrong kind at run time is an output that does not fit, i.e.
   absent. Modules are allowed to declare `Any`, like the universal leaves.

## 4. Kind, capabilities and the manifest (I8)

`kind_of` and `capabilities_of` are keyed by primitive name today, with `collect_caps` walking a
composite's children and `fn` params. A module is a third arm: `Pure`, no capabilities, *because*
§3.1 holds, and the two budgets surface as manifest items the deployer approves like any limit,
through the existing `Limit(name=n)` form: `Limit(module_fuel=10000000)`,
`Limit(module_memory=16777216)`. A flow that places a module therefore asks for nothing new, and
says exactly how much computation it may consume. Design-mode mocks never apply to a module: it
runs for real in the editor too (§5), so there is nothing to launder.

## 5. The engine stays out of core

`eval` must not grow an interpreter: core has four pure dependencies and the purity-by-dependency
argument depends on keeping it that way, and the editor's bundle should not carry an engine that
most flows never use. So:

- `Resolver` gains `fn run_module(&self, bytes: &[u8], limits: &Limits, inputs: &[Option<Value>])
  -> Option<Vec<Option<Value>>>`, with a **default body returning `None`**. `eval_at`'s module
  arm calls it. A resolver without an engine makes every module absent, which is honest: "this
  host cannot run code" shows up as a flow that produces nothing, never as a wrong answer.
- A new crate **`holon-module`** depends on core and on the engine, implements the ABI of §3 over
  `encode_value`/`decode_value`, and is used by `holon-host`'s resolver and by `holon-wasm`'s. One
  implementation, compiled twice, exactly as core itself is: native for the host, wasm32 for the
  editor. Design mode runs the real module under the real budget, so a check in the editor is a
  check.
- **Engine: `wasmi`.** Pure Rust, `no_std`, compiles to wasm32 (it runs inside browsers today),
  interprets rather than compiles (so no JIT, no platform code paths), and meters fuel natively.
  `wasmtime` is refused: a JIT, heavy, and not a wasm32 target, so the editor would need a
  second engine, which is the "two evaluators" the one-evaluator invariant forbids.

The one-evaluator invariant is kept at its real boundary: core's `eval` is still the only
evaluator of units; the engine is the evaluator of *one body kind*, behind a trait, and it is the
same engine everywhere.

## 6. Surface (last, and only after the trigger)

- **CLI.** `holon module add greet.wasm --in text:Text --out Text [--fuel n --memory n]`
  validates (§3), stores the unit, prints its hash and a one-line manifest. `holon show` on a
  module prints its interface, byte size, budgets and hash.
- **Language.** `module greet = "greet.wasm" (text: Text) -> Text`, a definition like `holon`,
  then `greet(x)` like a holon call. Nothing else in the language changes; a module is a unit.
- **Editor.** A module is a node with its declared ports, an ⓘ saying it is code (hash, size,
  budgets), and in view mode its inputs and outputs, no inside. Placed from the palette's flows
  section like a saved flow, or by hash.
- **Sample.** A module written in Rust (`fixtures/modules/<name>/`, built in CI with
  `cargo build --target wasm32-unknown-unknown`, the `.wasm` committed beside it so a checkout
  needs no toolchain) doing one thing the vocabulary genuinely cannot, chosen from whatever
  triggered the build (§7). Per `CLAUDE.md`: one sample, deployed by the end-to-end tests.
- **Design doc §13.** One sentence to add when building: *a module is not a leaf admitted through
  the gate; it is the gate's escape hatch, pure by having no imports and total by its budget, and
  it is the reason no further "general" leaf needs admitting.* Without that sentence someone will
  later propose a `script` primitive with a host API, and that is the end of the thesis.

## 7. Why not now, and what triggers the build

Not built on 2026-10-24, on purpose:

- Nothing depends on it: no sample, no step of the language, no buyer conversation.
- The hash-bearing parts (the tag, the ABI, the budget defaults) are better designed against a
  few real modules, and there are none. Guessing now means a migration later; waiting is free.
- An escape hatch changes behaviour the day it exists: gaps get closed with code instead of with
  the vocabulary or a holon, and a module is opaque in replay. While the vocabulary is cheap to
  grow, that pressure is useful; it produced `as_text` and `coalesce` in one day.
- It adds weight: an engine in the editor's bundle, a crate, a body kind in every inspector.

**Build when either happens:**

1. An agent or a person writing a flow hits something the 70 leaves and holons cannot express in
   a sane number of nodes, **twice**, and neither case is a candidate for one small pure leaf
   through the gate.
2. A buyer conversation names it.

Then the inches, in order, each with its tests, a live check and a commit: (1) core: the body,
the tag, validation, `Pure`, the `run_module` hook and its default, 2 to 3 days; (2)
`holon-module` on `wasmi` with the ABI, fuel and memory, tests over tiny modules written as
text and assembled at test time with the `wat` crate, 3 to 4 days; (3) host and bridge wiring,
limits in the manifest, store round-trip, 2 days; (4) CLI, language statement, editor node,
sample, 3 to 4 days.

## 8. Invariant check

- **I1 interface parity:** a module is a unit body; `eval` is the one entry point. ✓
- **I2 purity:** no imports; the engine is deterministic and behind the encoding, which has no
  form for the one nondeterministic value. ✓
- **I3 by reference:** placed by hash, never inlined. ✓
- **I5 content addressing:** the bytes are the preimage. ✓
- **I6 transitive pinning:** a child like any other; the closure carries it. ✓
- **I7 the effect boundary:** a module can neither source nor sink; it is interior. ✓
- **I8 derived labels:** `Pure` follows from the empty import section, read by core, not from a
  declaration. ✓
- **One evaluator:** core evaluates units; one engine, compiled twice, evaluates module bodies.
  ✓, at the boundary stated in §5.
- **Totality:** fuel and memory in the identity. ✓
