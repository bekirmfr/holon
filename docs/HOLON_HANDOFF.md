# HOLON — Agent Handoff (v2)

*A complete knowledge transfer. Read this top to bottom before touching anything. It is dense
on purpose: every section is load-bearing. This supersedes the end-of-P3 / early-P4 handoff.*

---

## 0 · TL;DR (read this even if you read nothing else)

**Holon** is a Rust visual-programming VM — "the Minecraft of compute." You compose logic from
tiny pure primitives; any flow packages into a new unit; an open marketplace shares units. The
technical heart: a **pure, content-addressed dataflow core** (`holon-core`), an **effectful shell**
(`holon-host`), and a **wasm bridge** (`holon-wasm`) that compiles the same pure core to the browser
for a visual editor. The thesis — the entire reason for the architecture — is:

> **Pure + content-pinned ⟹ universal, trustless memoization.** A unit's hash is its identity;
> a pure unit gives the same output for the same inputs everywhere and forever; therefore its
> result can be cached and *shared without trusting anyone* (you verify by re-hashing). That shared
> memo cache is the moat, and it has been built since P3.

**Status:** **P1 (Language), P2 (OS / effect boundary), and P3 (Marketplace) are all COMPLETE.**
**P4 (the Editor) is well underway** — wasm bridge, palette, wiring, projection, live eval,
multi-output composites (both core and editor), subgraph grouping, expand-to-inspect, transitive
inspection UI, and a first pass of frontend automated tests all exist and are browser/test
confirmed. **255 native tests (185 core + 39 host + 11 wasm-native) + 20 wasm-target tests + 25
frontend tests (Vitest, new this arc — the editor's first automated coverage of any kind).** The
two ready-but-blocked leaves are `P4.5.1` (Registry client) and `P4.6.1` (Design/Deploy toggle) —
both need a browser↔host transport that doesn't exist yet; `P4.6.1`'s *design*-mode half is the one
exception, genuinely buildable today with no transport at all. **This is the real frontier — see
§10.**

**How work happens (critical, unchanged from the prior handoff):** You (the agent) write/review
Rust and TypeScript. **You cannot compile or run Rust** — confirmed again this arc (`cargo`/`rustc`
genuinely absent from the sandbox, not assumed) — the **user runs `cargo test` / `wasm-pack` on
Windows and pastes the output**, which is the only real Rust check. **You CAN run node/npm/tsc and
git**, and now also a general-purpose sandbox shell with broad read/write/edit tools and outbound
access to package registries (crates.io, npm, pypi) — useful for verifying a crate's actual API
before depending on claims about it (see §12, the SmallVec `PartialEq` gotcha, found this way, not
assumed). Editor TypeScript is type-checkable by you; its *visual* behavior stays browser-confirmed
by the user. You proceed in **inches**: (L4 note or surfaced fork if consequential) → ground against
the real code → write → user runs / you typecheck → if green, commit + JOURNAL entry + PLAN update
→ next inch.

---

## 1 · Vision & North Star

*(Unchanged from the prior handoff — this has not moved and should not move casually.)*

- **Name:** "Holon" = a whole that is also a part. The unit model is recursively composable: every
  composite is also a unit, indistinguishable at the interface from a primitive (`I1`).
- **The pitch:** visual programming where you wire small pure primitives into flows, freeze a flow
  into a new reusable unit, and publish it to a permissionless marketplace. Trust is by hashing,
  not by authority.
- **The moat (built):** because the interior is pure and every unit is content-addressed, *any*
  pure computation is memoizable and the memo is verifiable by anyone — so a global shared cache
  needs **no trusted party**. `P3.3.2` built it (a `SharedCache` that admits an entry only if it
  re-keys correctly and catches a liar on read).
- **Models adopted:** Functional Core / Imperative Shell (FCIS); Elm's `(Model, update, Cmd, Sub)`
  runtime shape; Unison's content-addressing *design* (not its runtime). See
  `COMPOSABLE_VM_DESIGN.md`.

---

## 2 · The Nine Invariants (load-bearing — violating one is a bug, not a tradeoff)

*(Unchanged in count and definition. Re-confirmed, not just carried forward on faith, by the P4.7
multi-output audit — see §7 — which re-ran all nine against the real code, not the design's stated
intent, and found I4/I5 strengthened and nothing else moved. That audit is the model for how to
check an invariant when a new leaf lands: read the real code, don't cite the doc.)*

- **I1 — Interface parity.** A composite is indistinguishable from a primitive at its interface.
  *(Now proven, not just structural: `eval` is still the one entry point for both, even after
  multi-output widened its return shape — the widening extends the one path, it doesn't add a
  second one.)*
- **I2 — Purity / determinism of the core.** `holon-core` is pure: no IO, clock, or randomness;
  identical `(unit, inputs)` ⇒ identical output. (Proven in `determinism.rs`.)
- **I3 — Composition by reference, not flattening.** Composites reference children by hash.
- **I4 — Derive-then-freeze the interface.** A unit's interface is *derived* from its body, then
  frozen into its identity. **Strengthened by multi-output:** `Unit.outputs` now gets the exact
  same treatment `inputs` always had (derived types, chosen names/order, frozen into the hash) —
  the old single-output model was the asymmetric special case; multi-output made the two sides
  consistent, not looser.
- **I5 — Immutability + content addressing.** Hash = identity. There is **ONE encoder
  (`encode_preimage`) and ONE evaluator (`eval`)**. Runtime `Value`s are never in the preimage —
  and neither is anything view-space (the editor's node `id`/`position`/`localId`). The *order* of
  exposed outputs is now part of identity too, exactly as promoted-input order already was.
- **I6 — Transitive pinning.** Pinning a unit pins its entire closure.
- **I7 — Effects at the edges, purity in the middle.** Drivers (inputs) and sinks (outputs) sit at
  the boundary; the interior is pure.
- **I8 — Derived, not asserted.** Every label is *computed* from structure, never declared by a unit
  about itself: kind, capabilities, the interface, the memo key, reputation-by-adoption.
- **I9 — Full transitive inspectability.** Every level of a composite is inspectable to the leaves.
  **Now has a real editor face**, not just a core property: the transitive inspection UI
  (`P4.4.1`) walks a placed composite level by level via `get_unit` + `capabilities`/`effects_of`,
  showing exactly this invariant to a human, browser-confirmed.

---

## 3 · Operating Philosophy (how to think, not just what to build)

*(Unchanged — these principles kept proving themselves this arc too, see the fresh examples in
italics below each.)*

1. **Ground in the real system before you act.** Read the actual code/repo before forming a
   hypothesis or proposing a change. *(This arc: the SmallVec production-ripple estimate was
   corrected from "touches everything" to "genuinely narrow" only after tracing `eval_one`,
   `verify_entry`, and the wasm bridge line by line — each turned out to need zero changes,
   discovered by reading, not by re-deriving from the type signature alone.)*
2. **Build the least the architecture needs — and nothing it already provides.** *(This arc: the
   frontend testing setup nearly tested four functions in place inside `App.tsx` before tracing its
   import graph and finding a top-level value import of the real wasm package — extracting to
   `diagnostics.ts`, a module with *zero* runtime dependency on the wasm package, was the more
   correct move once traced, not just a workaround.)*
3. **Dissolve before you build.** *(Multi-output composites were once themselves a "dissolved"
   leaf — `List[Effect]` fan-out solved the *effect* case for good. `P4.7` later established that
   was never the same claim as "a pure data composite can only expose one value" — the two ideas
   had been conflated, and once separated, genuine multi-output for data composites turned out to
   be real, needed work, not a re-litigation of the earlier dissolution. Worth being precise about
   which claim actually dissolved, and which only looked adjacent to it.)*
4. **Never wrap a systemic problem in an artificial control.** *(The standing example, now cited
   twice: `eval_composite`-by-reference looked correct, passed every test, and was reverted anyway
   because its own benchmark fix — a fast-path bolted on to route around a regression the refactor
   itself introduced — was recognized as concealment, not a fix. The SmallVec inch cited this
   precedent explicitly before claiming its own fix was safe, and benchmarked before and after
   rather than assuming.)*
5. **Derive, don't assert (`I8`).** If a property is computable from structure, compute it.
6. **One boundary at a time (the inch).** Pin one thing per inch; don't braid changes.
7. **KISS + security-first.** Simplest correct design; fail closed on the unknown.
8. **Say it plainly, including your own missteps.** *(This arc: reported a benchmark anomaly as
   "plausible, not confirmed" rather than rounding it into a clean success story, then re-ran it in
   isolation and reported the actual resolution — including that a *different* size regressed the
   second time, which is what made "measurement noise" a confirmed conclusion rather than a
   convenient one.)*
9. **Keep the workspace tidy.** Before creating/editing a file, check whether one of that name
   exists; overwrite/rename deliberately. *(Extended this arc to a sharper form for stale
   **agent-local** state specifically: a locally-cached copy of `holon-host/src/lib.rs` turned out
   to predate `P4.7` by a full day — caught by grounding the real current file via search before
   editing, not by trusting the local cache's timestamp-free assumption of currency. When a
   locally-held file's provenance is uncertain, re-ground it before editing or delivering a full
   file from it — see §12.)*

---

## 4 · Workflow Conventions (follow these exactly)

**Repo:** `C:\Users\BMA\source\repos\Holon` (GitHub "holon"). User on Windows, VS2026 Dev PowerShell.
**Cargo workspace, three crates** + an editor app:
`crates/holon-core`, `crates/holon-host`, `crates/holon-wasm`, and `editor/` (Vite+React+TS).

**What you can and cannot run:**
- **Rust: you cannot compile or run it.** No toolchain in the sandbox (`cargo`/`rustc` both
  confirmed absent — `which`/`--version` both fail with "not found," checked directly this arc, not
  assumed from the prior handoff). Self-review hard (exhaustive matches, borrows, imports, brace
  balance — a `python3 -c` brace/paren-count diff is a cheap, real sanity check worth running on
  every edited file before delivery). The **user runs `cargo test --workspace` and
  `wasm-pack test --node crates/holon-wasm`** and pastes output — that is the only real Rust check.
  **State the expected test count before they run it** — it's a tripwire that catches your own
  arithmetic mistakes, not theirs.
- **TypeScript/editor: you CAN run `npm`, `npx tsc`, `npx vitest run`, and `git`** in the sandbox.
  Frontend now has real automated coverage — `npm test` runs Vitest against pure-logic modules
  extracted specifically to avoid pulling in the wasm package (see `diagnostics.ts` in §6). A green
  `npx tsc -b` plus a green `npm test` is real signal, not proof of *visual* behavior — canvas
  behavior (does a node render, does a wire connect, does grouping rewire edges correctly) is still
  **browser-confirmed by the user**; no React/canvas test runner exists.
- **General sandbox tools, worth knowing exist:** a shell with broad file read/write/edit
  (`view`/`str_replace`/`create_file`/`bash_tool`) and outbound network access to package registries
  (`crates.io`, `npmjs.org`, `pypi.org`, `github.com`, `raw.githubusercontent.com` among others —
  see the network-configuration allowlist if unsure whether a domain is reachable). This is genuinely
  useful for **verifying a crate's actual public API before depending on a claim about it** — e.g.
  confirming `SmallVec` has no `PartialEq<Vec<T>>` impl by fetching its real source from GitHub
  rather than guessing from memory (§12). Prefer this over assuming a crate's behavior from general
  knowledge, especially for anything version-sensitive.

**File delivery naming** (write to `/mnt/user-data/outputs/`, then `present_files`; the user copies):
- `<module>.rs` → `crates/holon-core/src/<module>.rs` (module name preserved, e.g. `eval.rs`,
  `encapsulate.rs`, `memo.rs`)
- `wasm_lib_fresh.rs` (or similarly named) → `crates/holon-wasm/src/lib.rs`
- `core_lib.rs` → `crates/holon-core/src/lib.rs` (the curated `pub use` re-export list — a real,
  recurring gap: a new public type/fn added elsewhere in core needs an explicit line here or it
  never reaches the wasm bridge; this has bitten the project more than once, see §12)
- `editor/src/<File>.tsx|ts` → mirror under `editor/src/`
- L4 notes, doc-update patches, commit summaries as `*.md`/`*.txt`
- **When a locally-cached file's currency is uncertain** (see §3.9, §12): do not deliver a full-file
  rewrite from it. Ground the real current state via search first; if only part of the file needs a
  change, deliver a targeted find/replace patch instead of a full file, and say plainly why.

**The inch loop (per unit of work), unchanged:**
1. (If consequential) write an **L4 design note** OR surface the fork to the user *first* — ask,
   don't guess, when a design choice is genuinely open (see the SmallVec inch's own "what's the
   right scope" back-and-forth for a recent example of this working as intended).
2. **Ground against the actual code** before advancing any hypothesis (`view`/search, never memory).
3. Write to outputs; self-review (balance checks, re-grep for every call site a signature change
   touches — this arc's SmallVec inch needed a full-file sweep across nine files to find every
   `assert_eq!(eval(...), vec![...])` comparison that would stop compiling; don't assume a change is
   contained without actually searching for every caller).
4. User runs cargo/wasm-pack (or browser-confirms the editor, or runs `npm test`); pastes result.
5. If green: give the **git commit command(s)** or a commit-summary file, then a **JOURNAL.md**
   entry (one per inch, newest at the bottom, honest about anomalies and dead ends — not just
   successes), then a **PLAN.md** update if a leaf's status genuinely changed. Deliver doc updates
   as **find/replace instructions grounded in the real current text** (search for the exact current
   closing lines first — don't guess where to append).
6. Next inch.

**Build/run the editor:**
`cd crates/holon-wasm && wasm-pack build --target web --out-dir ../../editor/wasm`, then
`cd ../../editor && npm run dev`. `editor/wasm/` and `node_modules/` are git-ignored. Order matters:
a Rust bridge change must be `cargo`-green **and** re-built with `wasm-pack` before the editor sees
it. **Frontend tests:** `cd editor && npm test` (Vitest) — independent of the wasm build, since the
tested modules deliberately have zero runtime dependency on it.

---

## 5 · Architecture

### The purity boundary *is* the crate graph (Cargo-enforced)

```
holon-host  ──┐
              ├──depends on──►  holon-core      (NEVER the reverse, NEVER sideways)
holon-wasm  ──┘                 (pure core)
```

`holon-core` has **two** dependencies now: `blake3` (from the start) and `smallvec` (added
2026-07-02, the eval perf inch — see §7). **This is a real, deliberate change from the prior
handoff's "exactly one dependency" claim** — don't let that line go stale again; if a third
dependency is ever proposed, the bar that was actually applied to `smallvec` (name the specific
property it adds, show the system doesn't already provide it, then accept the cost plainly rather
than minimize it) is the bar to reapply. It still contains no IO, async, clock, or randomness — and
*cannot*, because dependencies only point inward. It compiles **unchanged to both native and
`wasm32`**. `holon-host` (edition 2024) is the impure shell. `holon-wasm` (cdylib+rlib) is the
browser bridge: it depends on **core only**, plus `wasm-bindgen` / `serde` / `serde-wasm-bindgen`.
**The only impure function in the whole system is `Host::perform`** (in `holon-host`); the wasm
crate adds no impurity — it just marshals the pure core across the JS boundary.

### The Elm runtime, closed on a pure core

*(Unchanged.)*

| Elm     | Holon                                                       |
|---------|-------------------------------------------------------------|
| `Model` | the host's storage (a state cell, threaded between rounds)  |
| `update`| the graph (pure `eval`)                                     |
| `Cmd`   | sinks → `Value::Effect`, performed by the host             |
| `Sub`   | drivers → sourced values, injected by the host             |

### Two orthogonal classification axes decide everything the host needs

*(Unchanged.)*

- **effect-bearing ⊥ replayable** — bearing an effect does *not* forfeit caching; only `State` does.
- **sourced ⊥ performed** — inputs (drivers, state reads) are *sourced*; outputs (sinks, state
  writes) are *performed*.

### Three per-name axes for a primitive (memorize this shape)

*(Unchanged — still three functions, still all derived, none asserted.)*

- **behaviour** — `primitives::compute(name, inputs, params)`, with `map`/`fold` handled in `eval`;
- **capability** — `kind::capabilities_of(name)` (the security manifest);
- **interface** — `catalog::interface_of(name)` (typed ports/params — the palette source).
  `catalog::genesis_units()` returns the full frozen genesis set (34 leaves, §13a); a core test
  asserts the three axes agree.

### `eval`'s return shape — the one signature that changed this arc, and why it matters everywhere

**`eval(unit, inputs, resolver) -> EvalOutputs`**, where `EvalOutputs = SmallVec<[Option<Value>;
1]>` — a small-vector type with inline stack storage for 0 or 1 outputs (every genesis primitive,
forever; still the overwhelming majority even now), spilling to the heap only for a genuine
multi-output composite. This is `eval`'s **second** shape change in this project's life:
`Option<Value>` (single output) → `Vec<Option<Value>>` (`P4.7`, multi-output support) →
`EvalOutputs` (this arc, same shape, faster common case). **`eval_composite`'s internal `scratch`
had to change type in lockstep** (`Vec<EvalOutputs>`, not just `eval`'s own signature) — a recursive
`eval` call's result is stored directly into it, so the fix would only have reached the outermost
call otherwise, not nested composites. **A real, narrow Rust gotcha worth knowing before touching
this code again:** `SmallVec` implements `PartialEq` only against another `SmallVec`, never against
a plain `Vec` — so any new test comparing `eval(...)`'s result against a `vec![...]` literal needs
`.into_vec()` appended on the actual side, or it won't compile.

---

## 6 · Codebase Map

Public core API is re-exported from `holon-core/src/lib.rs` — **a curated `pub use` list that must
be updated by hand whenever something new needs to reach the wasm bridge; this has been a repeat
gap** (`Expose`, `eval_one`, and others were each individually caught missing from this list at
different points — check it explicitly whenever adding a new public core type or fn). `pub const
BOUNDARY: u32 = u32::MAX` marks the composite's own interface in edges.

### `holon-core` (pure; native + wasm32)

- **`hash.rs`** — canonical content hashing (BLAKE3). `Type { Number, Boolean, Text, List, Effect,
  Any }` (tags **append-only**). `Port`, `Edge`, `UnitBody { Primitive{name} | Composite{children,
  edges} }`, **`Unit { body, params, inputs: Vec<Port>, outputs: Vec<Port> }`** — `outputs` is now
  plural (was `output: Option<Type>` before `P4.7`), symmetric with `inputs`. `hash_unit`,
  `encode_preimage`, `decode_preimage`, `parse_hash`. Child-order canonicalization (children sorted
  by hash, ties broken by minimizing serialization over within-group permutations, capped at 8! else
  `TooSymmetric`) — `eval` topo-orders by edges, so child order is semantically inert, the linchpin
  that makes canonicalization safe.
- **`catalog.rs`** — the interface axis. `GENESIS: &[&str]` (34 §13a names), `interface_of(name) ->
  Option<Unit>`, `genesis_units()`.
- **`value.rs`** — `Value { Number(f64), Boolean, Text, List, Effect{sink, payload} }`.
- **`resolve.rs`** — `trait Resolver`; `MapResolver`.
- **`validate.rs`** — collect-all, BOUNDARY-aware; `compatible(src, dst)` — the §5 wire rule.
- **`primitives.rs`** — `compute(name, inputs, params)`; pure + sink primitives. `map`/`fold` live in
  `eval`.
- **`eval.rs`** — the **ONE evaluator**. `pub fn eval(...) -> EvalOutputs` (see §5's dedicated
  paragraph — this is the file that changed shape twice). `eval_one(unit, inputs, resolver) ->
  Option<Value>` — the single-output convenience `map`/`fold` use for their `fn` argument, fails
  closed (refuses, doesn't silently take the first slot) on a non-single-output unit.
  `eval_composite` — topological pass; `scratch: Vec<EvalOutputs>`, one entry per child, each
  holding that child's own output vector (a child may itself be a multi-output composite).
- **`kind.rs`** — classification. `Kind { Pure, Driver, State, Sink }`, `Capability`,
  `Capabilities`, `capabilities_of(name)`, `capabilities(unit, resolver)`. **GUARDRAIL: trust is
  always computed from REAL units, never design-mode mocks.**
- **`simulate.rs`** — design mode: substitute *sourced* leaves with type-default constants.
  `design_eval(root, inputs, real) -> Option<Value>` — deliberately kept single-output via
  `eval_one`, not widened to `eval`'s multi-output shape: a `design_eval` root is a *service* graph
  settling to one inspected value, not the multi-output data-composite case `eval` itself exists
  for.
- **`determinism.rs`** — proves `I2` (golden + randomized).
- **`encapsulate.rs`** — derive-then-freeze (`I4`): `derive_and_freeze`, **`EncapSpec { promote:
  Vec<Promote>, expose: Vec<Expose> }`** — `expose` is plural now (was `output: (u32, u32)` before
  `P4.3`), `Expose { child, port, name }` structurally identical to `Promote` but its own type so a
  `promote`/`expose` mixup is a compile error, not a silent bug. **`default_spec` proposes *every*
  free output automatically** now (was: exactly one, ambiguity-erroring on two-or-more) — symmetric
  with how it already promoted every unbound input with zero ambiguity checking. `propose_encapsulation`
  / `freeze_encapsulation` — the bridge-exposed split of what `derive_and_freeze` does in one call,
  letting the editor UI show a proposal and let a person exclude an output before freezing.
- **`promote.rs`** — param promotion: `instantiate`, `open_param(s)`.
- **`inspect.rs`** — `child_refs` / transitive `dependencies`, reputation-by-adoption counting.
- **`memo.rs`** — `CacheEntry.outputs: Vec<Option<Value>>` (was `output: Value`, widened alongside
  `eval`'s own `P4.7` shape change — a multi-output composite is exactly as cacheable as a
  single-output one). `verify_entry` compares the whole outputs vector canonically. Needed **zero**
  changes for the `EvalOutputs`/`SmallVec` inch — `canonical_slots_bytes` takes a slice
  (`&[Option<Value>]`), and `SmallVec` derefs to one automatically.
- **`fork.rs`** — re-pin: replace a child/`fn`-param hash throughout a closure, re-hash upward.
- **`advisory.rs`** — derives replayability/purity/capability-closure-size.

### `holon-host` (impure shell — `lib.rs`)

- **`trait Host { fn perform(&mut self, capability, payload); }`** — the only impure function.
- **`fire(root, inputs, resolver, host) -> Option<Value>`** — deploy firing. Uses `eval_one`
  internally (not `eval` directly) since `P4.7` — deliberately kept single-output, a service graph's
  settled request, not the multi-output data-composite case. **Needed zero changes** for the
  `EvalOutputs` inch, already on `eval_one`.
- **`Memo`** — the **one** place in `holon-host` that genuinely needed a change for the
  `EvalOutputs` inch: `Memo::eval` calls the widened `eval` three times and returns
  `Vec<Option<Value>>` from its own (deliberately still-`Vec`, it's a stored cache not a hot loop)
  signature — `.into_vec()` at each of the three call sites.
- **`SharedCache::get_verified`** — reads `entry.outputs.clone()` off a stored `CacheEntry`, never
  re-derives via `eval` — needed zero changes.
- **`StateHost`**, **`Store`/`MemStore`/`FileStore`**, **`Registry`** — unchanged this arc. **Not
  yet reachable from wasm at all** — this is `P4.5.1`/`P4.6.1`'s actual current blocker, see §10.

### `holon-wasm` (the bridge — `lib.rs`)

Serde mirrors: `WType`, `WUnit`/`WBody`/`WParam`, `WPort`/`WEdge`/`WValue`, and new this arc:
**`WCapability`/`WKind`** (bare-string mirrors, same convention as `WType`), **`WPromote`/`WExpose`/
`WEncapSpec`** (the bridge shape for the propose/freeze split). `#[wasm_bindgen]` exports, current
set:
- `hash_unit_hex`, `canonicalize_preimage`, `build_preimage`, `load_unit`
- `Evaluator { new / insert(preimage)→hex / eval(preimage, inputs)→JsValue }` — `eval` returns an
  **array** of `WValue | null`, one slot per declared output, not a single value (widened `P4.3`)
- `primitive_catalog()` — the palette source
- `compatible(src, dst) -> bool` — core's wire rule, crossed to JS
- **`get_unit(hash) -> WUnit`** — new (`P4.3.1`): direct `resolver.resolve` lookup by hash, no
  preimage round-trip. This is what lets the editor expand a placed composite node to inspect its
  children without re-decoding bytes it never kept.
- **`capabilities(hash)` / `effects_of(hash)`** — new (`P4.4.1`): called fully-qualified as
  `holon_core::capabilities(...)` to avoid a name collision with these method names. Called once per
  hash while the editor walks a composite level by level via `get_unit` — that's the whole
  mechanism behind the transitive inspection UI; no new recursive bridge capability was needed,
  `capabilities`/`effects_of` already recurse internally.
- **`propose_encapsulation` / `freeze_encapsulation`** — new (`P4.3` fork 3): the bridge-exposed
  split of `project()`'s single call, so the editor can show a proposed interface and let a person
  exclude an output before the real, hash-changing freeze happens. `project()` itself is untouched
  and stays the default single-call path.

Tests: **11 wasm-native + 20 wasm-target** (was 6 + 10 before this arc's work — `get_unit`,
`capabilities`/`effects_of`, and the propose/freeze split each added their own crossing tests).
**A real, once-costly gotcha, still worth restating:** `#[cfg(all(test, target_arch = "wasm32"))]`
gates the entire `wasm_tests` module out of every plain `cargo test --workspace` run — that whole
class of tests only compiles and runs under `wasm-pack test`. Always run both commands, not just
one; a regression can hide in the one nobody ran.

### `editor/` (Vite + React 18 + TS strict + @xyflow/react v12.3.5)

Everything is **derived from core** — no hand-written primitive lists, no re-implemented rules.
- **`holon.ts`** — typed facade over the wasm module. `WEncapError`'s variant names have moved more
  than once as core's own error enum evolved (`OutputChildOutOfRange`→`ExposeChildOutOfRange`,
  `AmbiguousDefaultOutput` removed entirely) — a real, recurring class of bug: a TS `switch` with
  now-stale case labels, caught each time only by grounding the real current Rust enum, not assumed
  fixed from memory.
- **`theme.ts`** — a wire type *is* a color; the palette derives from core's type system.
- **`PrimitiveNode.tsx`** — renders one `<Handle>` per entry in a unit's `outputs` array (was a
  single hardcoded `"out"` handle before `P4.7`), keyed by port name. `PlacedBody` type distinguishes
  a primitive-kind node from a composite-kind node (`{kind:"Primitive"} | {kind:"Composite",hash}`)
  — needed once grouping could place composite nodes onto the canvas, not just primitives.
- **`Palette.tsx`** — draggable list from `primitiveCatalog()`.
- **`App.tsx`** — the editor. Has grown substantially this arc: `onProject` (raw graph → core
  composite → eval, resolving `from_port` from the wire's actual `sourceHandle` rather than
  hardcoding 0), `onGroup` (subgraph selection → a new placed composite node, boundary-edge
  rewiring in both directions, port-name disambiguation reusing `boundaryLabels`'s own
  provenance-tracking algorithm rather than a second one), `onExpand` (inverse of grouping,
  one-level-only by design — nested composites stay collapsed), `onInspect` (opens `InspectPanel`),
  `excludeOutput` (a genuine re-freeze via the propose/freeze split, not a client-side display
  filter — a new hash, an identity-affecting choice).
- **`InspectPanel.tsx`** — new (`P4.4.1`). Genuinely read-only — confirmed, not assumed, to never
  touch canvas node/edge state. Collapsible tree, one level of a composite at a time via `get_unit`
  + `capabilities`/`effects_of`.
- **`diagnostics.ts`** — new this arc. Four pure functions extracted *out of* `App.tsx`
  (`boundaryLabels`, `isWEncapError`, `validationErrorNodeIndices`, `encapErrorNodeIndices`)
  specifically so they could be unit-tested without dragging in `App.tsx`'s top-level value import
  of the real wasm package. Only type-only imports from `holon.ts` remain (TypeScript erases these
  entirely), so this module has zero runtime dependency on the wasm package or `holon.ts`'s own
  top-level code. **This is the pattern to repeat** if more of `App.tsx`'s logic needs testing later
  — trace the import graph before assuming a function can be tested in place.
- **`diagnostics.test.ts`** / **`theme.test.ts`** — new this arc, 17 + 8 = 25 tests, the editor's
  first automated coverage of any kind. `diagnostics.test.ts`'s centerpiece deliberately exercises
  `encapErrorNodeIndices` against the *current* `WEncapError` variant names — exactly the check that
  would have caught the stale-names class of bug immediately, instead of by manual review.
- **`vite.config.ts`** — now carries a `test: { environment: "jsdom" }` field, using `vitest/config`'s
  own `defineConfig` (a re-export of Vite's, typed with the `test` field included) rather than a
  second, parallel config file. **A real, once-live risk, now resolved: stale `vite.config.js` /
  `vite.config.d.ts` compiled-artifact files used to sit beside this one** — Vite's config
  resolution could have silently preferred the stale `.js` over this `.ts`, producing a confusing,
  unrelated-looking failure. Confirmed deleted; if either reappears (e.g. from a stray `tsc` run
  without a proper output directory), delete on sight.
- **`package.json`** — `vitest ^3.2.6` (not `4.x` — that major requires Vite `6`/`7`/`8`; checked
  directly against the npm registry rather than assumed, to avoid forcing an unrelated Vite
  major-version bump as a side effect of adding tests), `jsdom ^29.1.1`.
- **`styles.css`** — blueprint tokens (IBM Plex Mono, dark blueprint-ink chrome).

---

## 7 · Key Decisions & Forks (the *why*, so you don't relitigate them)

*(Everything from the prior handoff's list still holds and isn't repeated here except where it
needs a correction. New entries below.)*

- **Multi-output composites — the distinction the prior handoff blurred, worth being precise about
  now.** The prior handoff listed "multi-output composites" as *dissolved* by `List[Effect]`
  fan-out. That was true for exactly one case — an **effect**-bearing composite exposing several
  sinks — and it's still true: `List[Effect]` remains the mechanism for that. It was never the same
  claim as "a pure **data** composite can only ever expose one value," and `P4.7` built genuine
  support for exactly that: `Unit.outputs: Vec<Port>`, `eval` returning one slot per declared
  output, each independently `Some`/`None`. **`Vec<Option<Value>>` over `Option<Vec<Value>>`,
  deliberately** — a composite exposing two children's outputs where one hits a divide-by-zero and
  the other doesn't has a principled answer under the per-slot model (the good one survives) and no
  clean one under the all-or-nothing alternative (a genuinely-computed value gets discarded because
  a sibling failed).
- **The `eval_composite`-by-reference reversion is a standing precedent, not a one-off war
  story.** A refactor that is *correct* (passes every test) is not automatically *worth it* — the
  benchmark showed the fix's own adapter allocated more than it saved, and the fast-path bolted on
  to hide that regression was itself the tell: a control routing around a problem the refactor
  introduced, not a fix. Cited explicitly before the `EvalOutputs`/SmallVec inch, which benchmarked
  before and after specifically because of this precedent, rather than assuming a "standard fix"
  behaves as expected.
- **`eval`'s `EvalOutputs` (SmallVec) shape.** Named property: the P4.7 widening to
  `Vec<Option<Value>>` made *every* eval call heap-allocate, even the overwhelming single-output
  common case — measured at ~45–50% relative regression (58ns–273µs absolute, nothing
  user-perceptibly slow, but a real, measured cost). `SmallVec<[Option<Value>; 1]>` gives inline
  storage for that common case. The real production ripple turned out narrow (`eval`'s own
  signature, `eval_composite`'s internal scratch, one function in `holon-host`); the wide part was
  purely mechanical test-suite fixups (`SmallVec` has no `PartialEq<Vec<T>>`), not a design risk.
- **Frontend testing infrastructure — a debt named four times before it was paid.** Vitest + jsdom,
  chosen and version-pinned deliberately (see §6). The decision to extract pure logic into
  `diagnostics.ts` rather than mock the wasm import was itself a real course-correction mid-inch,
  not the original plan — worth reading as a model of "ground the actual dependency graph before
  picking the workaround."
- **Agent-local file staleness is a real, distinct risk from user-repo staleness.** Discovered when
  a locally-cached `holon-host/src/lib.rs` (last touched a full session earlier) was about to be
  used as a full-file delivery baseline — it would have silently reverted the real, current `fire`
  function (already on `eval_one` since `P4.7`) back to a pre-`P4.7` shape. Caught by grounding via
  search before editing. The fix wasn't "trust the cache more carefully" — it was "deliver a
  targeted patch instead of a full file whenever a local copy's currency is uncertain."

---

## 8 · Pillars & Status

- **P1 · The Language — COMPLETE.** Unchanged this arc.
- **P2 · The OS (effect boundary) — COMPLETE.** Unchanged this arc.
- **P3 · The Marketplace — COMPLETE.** Unchanged this arc.
- **P4 · The Editor — UNDERWAY, substantially further than the prior handoff.**
  **Done:** wasm bridge + CI, eval bridge, construction bridge, primitive-interface catalog,
  palette + custom nodes, wiring, **projection** (raw graph → core composite → eval — this was
  "the current inch" in the prior handoff; long closed), **live eval**, **output exposure /
  `P4.3`** (editable exposed-output proposal, exclude-and-refreeze), **`P4.3.1`** (subgraph
  selection, grouping into a placed composite node, expand-to-inspect), **`P4.4.1`** (transitive
  inspection UI, walks a composite to its leaves via `get_unit` + `capabilities`/`effects_of`),
  **`P4.7`** (native multi-output — core through editor, the data-composite case, §7), the
  **`EvalOutputs`/SmallVec perf inch** (not a plan leaf, a cross-cutting fix), and **frontend
  testing infrastructure** (also not a plan leaf — a debt closed).
  **NEXT — two ready-but-blocked leaves, see §10 for the real state:** `P4.5.1` (Registry client),
  `P4.6.1` (Design/Deploy mode toggle).

**Spine dependency:** P1 ⊳ P2 ⊳ P3 (logical — P2/P3 are theorems of P1). P4 sits beside the spine, a
client of P1 (+ optionally P3). Networked hosting is deliberately NOT a pillar.

---

## 9 · Honestly Deferred (decisions, not gaps)

*(Corrected from the prior handoff — several items listed there as deferred are now built; one item
not previously named is the real current blocker.)*

- **Real platform hosts** (real email/clock/socket/dedup) — behind the one `Host` seam; modeled
  tests stand in. Still deferred, unchanged.
- **Keyed/multiple state cells & drivers** — need parameterized capabilities. Still deferred,
  unchanged.
- **Non-scalar sourcing** — `constant_unit` is scalar-only. Still deferred, unchanged.
- **Async firing + quiescence gate** — eager eval makes it unnecessary now. Still deferred.
- **~~Multi-output composites~~ — no longer deferred, no longer accurate as stated in the prior
  handoff.** Genuine multi-output for pure data composites is built (`P4.7`, §7). What remains
  genuinely open: reorder/rename of promoted inputs or exposed outputs after the fact; re-including
  an excluded output without a full re-projection.
- **The browser↔host transport layer — the real current blocker, not previously named as its own
  item.** `holon-host`'s `Registry`/`Store` and its `fire`/`Host` trait are not reachable from wasm
  at all; no wire path exists between the browser and anything holding a registry or performing a
  real effect. This is what's actually gating `P4.5.1` (Registry client — browse/import/publish all
  need it) and half of `P4.6.1` (Deploy mode needs it; **Design mode does not** — `design_eval` is
  pure core, already wasm-accessible, and could ship today independent of this). Worth its own
  scoping pass (a service running where, talking to what, how the browser reaches it) rather than
  being picked up as an incidental prerequisite of either leaf.
- **Generics** (`P1.3.2`) — built. **Demotion-to-stdlib** — its own future pillar, unchanged.

---

## 10 · The current frontier — two ready-but-blocked leaves, one real fork (ground this first)

Unlike the prior handoff's single clear "next inch" (projection, long since closed), the current
state is genuinely a fork, not a queue — worth surfacing to the user rather than picking silently.

**`P4.5.1` (Registry client)** — blocked. Its own leaf definition needs browse/import-as-pin/publish,
all of which need a working transport to wherever a `Registry` actually lives. Picking this up
without transport existing first means hitting the same wall immediately.

**`P4.6.1` (Design/Deploy mode toggle)** — genuinely split, not uniformly blocked:
- *Design* half: real, available work **today**. `design_eval` (pure core, `simulate.rs`) is
  already wasm-accessible with zero new bridge work needed — a mode toggle showing mocked results
  live, safe and glitch-free, is buildable now.
- *Deploy* half: hits the identical transport wall as `P4.5.1` — `fire`/`fire_sourced`/the `Host`
  trait live in `holon-host`, not reachable from wasm.

**The grounding move for whichever gets picked (do this before any code):**
- If transport: read `holon-host`'s current `Registry`/`Store`/`fire` surface fresh (don't assume
  the shape described in §6 above is still exactly current — re-ground it, per §3.9/§12's own
  lesson about stale local assumptions), and scope a real answer to "a service running where,
  speaking what wire format, reached how from a browser sandboxed by CORS" before writing any code.
  This is a genuinely new kind of leaf for this project — nothing built so far has needed a network
  boundary between the editor and anything holding state.
- If `P4.6.1` design-mode-only: read `simulate.rs`'s `design_eval` and confirm it's still exactly
  the shape described in §6 (pure, `eval_one`-based, single-output by design) before wiring a bridge
  method for it — and be honest in the UI and the JOURNAL entry that this ships *half* a leaf, with
  deploy mode explicitly still blocked, not silently implied as "done."

Open forks to expect either way: whether the transport is meant to ever be more than localhost; how
`P4.6.1`'s "custom mocks / stimuli as ephemeral test-context" (from its own original leaf
definition) interacts with a composite that's been grouped/expanded mid-session. Surface these,
write an L4 first.

---

## 11 · Document Map

- **`PLAN.md`** — the living deductive plan. **Its resolution log is authoritative for status; its
  header block has historically lagged the ledger** (the prior handoff noted this explicitly; it's
  been partially, not fully, caught up since — the top-of-file "Build status" line and the later
  "Trunk shape" line don't fully agree with each other as of this writing). Trust the ledger and the
  latest `JOURNAL.md` entries over the header prose.
- **`JOURNAL.md`** — one entry per inch, newest at the bottom; captures *why*, including dead ends
  and anomalies, not just successes. Read the last several — specifically the `P4.3`, `P4.3.1`,
  `P4.4.1`, `P4.7`, the frontend-testing-infrastructure entry, and the `EvalOutputs`/SmallVec entry,
  all from this arc.
- **`COMPOSABLE_VM_DESIGN.md`** — vision/architecture/invariants; §13/§13a the genesis trust root.
  Unchanged this arc.
- **`REFERENCES.md`** — per-leaf porting map. Unchanged this arc.
- **L4 design notes** (`/mnt/user-data/outputs/` at time of writing, or wherever the project stores
  them going forward): the P1/P2/P3 sets from before, plus the P4 set, now including
  `P4.7-multi-output-L4.md` and a design pass for the SmallVec inch (surfaced as a fork/scoping
  conversation rather than a written L4, given its cross-cutting, non-leaf nature — worth writing
  one retroactively if this pattern (a perf inch that isn't a plan leaf) recurs). **Next L4 owed:**
  whichever of transport-scoping or `P4.6.1`-design-mode gets picked per §10.

**Levels:** L1 = pillars; L2 = entailed requirements; L3 = frozen per-leaf order; L4 = a deep design
note (or a surfaced fork) for one consequential leaf, *before* code.

---

## 12 · Gotchas & Reminders (the things that will bite you)

*(The prior handoff's list still applies in full; new items from this arc below, roughly in the
order they're likely to bite.)*

- **You still can't compile/run Rust — confirmed fresh, don't just carry the assumption forward
  forever.** `which cargo rustc` and `cargo --version`/`rustc --version` all fail in this sandbox as
  of this writing. Re-check this plainly rather than assume it's permanent, since the environment
  could change between sessions and the check costs nothing.
- **A local, agent-cached copy of a file is not automatically current — check its provenance before
  trusting it as a full-file delivery baseline.** The concrete failure mode: delivering a "complete"
  file that silently reverts real, already-shipped changes the local cache predates. If uncertain,
  ground the real current state via search first; if only part of a file needs a change and full
  certainty isn't available, deliver a targeted find/replace patch instead of a full file, and say
  why plainly rather than silently hedging.
- **`SmallVec` (and likely other small-vector/small-string crates) does not implement `PartialEq`
  against `Vec`/`String` directly — only against another instance of itself.** Any test comparing a
  `SmallVec`-returning function's result against a `vec![...]` literal needs `.into_vec()` (or
  `.as_slice()` against a slice literal) on the actual side. Confirmed against the crate's real
  source on GitHub, not assumed from general Rust knowledge — worth the same check for any similar
  crate before writing tests against it.
- **A type-signature change's "ripple" is not uniform — trace every call site, don't estimate from
  the signature alone.** The `EvalOutputs` inch's first-pass estimate ("touches everything downstream
  of `eval`") was wrong in a specific, correctable way: several real call sites (`eval_one`, the wasm
  bridge, `verify_entry`, `fire`) needed zero changes once actually traced, because they either
  already narrowed to a single value or already worked through a slice/iterator interface that
  doesn't care about the concrete container type. The *test suite* ripple was wide but mechanical.
  Don't conflate "the type changed" with "every caller needs editing" — check each one.
- **`eval_composite`'s `scratch` type must track `eval`'s own return type exactly, not just `eval`'s
  own signature.** `scratch[ci] = eval(child, ...)` stores a recursive call's result directly — any
  future change to `eval`'s return shape needs the scratch's element type updated in lockstep, or
  the fix/feature only reaches the outermost call, not nested composites. This is the second time
  this exact class of mistake was avoided by tracing the recursive assignment explicitly rather than
  assuming a signature change is self-contained.
- **`WEncapError`'s (and similarly-shaped bridge error types') variant names drift as core's own
  error enum evolves, and the TypeScript side does not get updated automatically.** A `switch` with
  now-stale case labels is a real, silent bug class, not a hypothetical — it's happened more than
  once. Whenever a core error enum changes, grep the editor's TS for every place that names its
  variants, don't assume a prior "already fixed" note is still accurate.
- **`holon-core/src/lib.rs`'s curated `pub use` re-export list is a real, recurring gap, not a
  one-time miss.** A new public type or function added to any core module needs an explicit line
  added here or it silently never reaches the wasm bridge — caught multiple times this arc alone by
  checking this file directly, not by assuming "I added `pub` so it's exported."
- **`#[cfg(all(test, target_arch = "wasm32"))]` still gates an entire test module out of plain
  `cargo test --workspace`.** Always run both `cargo test --workspace` and
  `wasm-pack test --node crates/holon-wasm`; a regression can hide in whichever one didn't run.
- **Trust uses REAL units, never mocks.** Unchanged from the prior handoff, still worth restating.
- **Derive in the editor; never re-implement a core rule in TS.** Unchanged, still the standing
  discipline — extended this arc by the `diagnostics.ts` extraction discipline (§6): even a *pure,
  logic-only* function benefits from being traced for its actual import dependencies before being
  tested in place, since a seemingly-innocent function can sit behind a module with a heavy,
  wasm-touching top-level import.
- **The user is a design collaborator.** Surface forks; accept pushback; they catch real things —
  this arc's own "what's the right scope for the SmallVec fix" conversation is a clean, recent
  example of this working as intended, and materially corrected the plan before code was written.

---

*End of handoff. The codebase is larger than it was at the last handoff, but the invariants are
still nine, still unmoved, and the method is still consistent: ground first, derive don't assert,
dissolve before you build, don't add what the foundation already gives, pin one boundary at a time,
keep the core pure — and now, additionally proven this arc: don't trust your own cached state
either, and check a dependency's real API before depending on a claim about it. The moat is built;
the editor is making it visible; the marketplace side of the editor (`P4.5.1`/`P4.6.1`'s deploy
half) is the one piece still waiting on a boundary — transport — that hasn't been drawn yet. Welcome
to Holon.*
