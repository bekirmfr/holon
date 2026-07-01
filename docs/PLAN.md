# Holon — Implementation Plan

> **Companion to `COMPOSABLE_VM_DESIGN.md`** (the north star). The design doc says
> *what is true and why*; this plan is **deduced from it** and says *what we must
> therefore build, in what order*. It is detailed **progressively** — level by
> level across the whole tree, then leaf by leaf — and is not a build target until
> we judge it thorough.
>
> **Build status:** **P1 (Language) and P2 (OS) COMPLETE; P3 (Marketplace) underway — `P3.8.1` (the content-addressed store) done.** Two of three spine pillars closed, the third begun. The store is where units live, keyed by their 32-byte hash: a `Store` trait (not a `Resolver` — it returns *owned* units, so eval never touches disk and IO stays host-side) with `MemStore` + on-disk `FileStore` backends, **verify-on-read = pure content-addressing** (the stored bytes are the hash preimage; a blob is trusted only because it re-hashes to the key it was filed under — `I5` made operational, no `blake3` in the host). The one genuinely-new piece is core's `decode_preimage`, the byte-exact inverse of `encode_preimage`. **136 tests (117 core + 19 host).**
> **Current resolution:** trunk + all four pillars at L3 **frozen**; the genesis primitive set + admission gate pinned in design `§13` (trust root; reconcile to it just before the registry's first publish). L4s built across P1/P2: `P1.4.3`/`P1.5.1`/`P1.5.2`/`P2.3.1`/`P2.4.1`, `sink-as-pure-request`, `P2.4.1-deploy-firing`, `P2.6.1-state-cell`, `A-to-B-effect-value`, `drive-sourced-resolver`, `P2.5.1-correlation-token`. **The phase's lesson:** pin the right boundary once and downstream "features" become theorems. **P3 so far confirms the plan's prediction** — the store *consumes* the content-addressing P3 forced at the very start: exposure, not invention.
> **Decisions pinned (P3):** the genesis primitive set is the trust root (`§13`), admission-gated (native iff slower-by-class / hot-and-pervasive / an effect / genuinely inexpressible), totality already secured by refusing `while`; the **local-first** marketplace core (store, registry, inspection, local memo, fork, advisory) needs **no server** — only `P3.3.2`/`P3.4.1`/`P3.8.2` are networked (`P3.7.1` identity dropped). `Store ≠ Resolver`; the byte format **is** the hash preimage (one canonical encoding, now with a decoder); verify-on-read is content-addressing, not a separate check.
> **Trunk shape:** three spine pillars (P1 ⊳ P2 ⊳ **P3**, all complete) + one parallel pillar (P4 · Editor, **begun**). The crate graph (`holon-host → holon-core`, never reverse) **is** the purity boundary, Cargo-enforced; `holon-core` has two dependencies (`blake3`, and `smallvec` since the 2026-07-02 eval perf inch — see JOURNAL.md). **P4 added a 2nd build target:** `holon-wasm` (cdylib, depends on core ONLY) compiles the pure core to wasm32 with zero changes; **hashing, evaluation, AND construction** all cross to JS via the bridge — the editor builds units through the *one* core encoder (`build_preimage`) and evaluates through the *one* evaluator, proven **bit-identical** to native and CI-guarded (the editor's load-bearing invariant — one encoder, one evaluator, never a second). Hashes cross as hex strings. Dependency rule extended: host→core, **wasm→core**, never sideways/up. The editor *surface* (Vite+React+TS canvas, wiring, inspection UI) is the remaining P4 work, now unblocked.
> **Next — genesis reconciliation arc (`P3-genesis-reconciliation-L4`), then `P3.1.1` (registry).** Reconciling the native set to `§13` *before* the registry's first publish freezes the trust root. A breaking arc done one green inch at a time: **R1 ✅ R2 ✅ R3 ✅** (the additive run — pure kit, text kit, `random` driver; no existing hash moved) → **R4 ✅** (clock/ingress/egress) → **R5 ✅** (one `const`) → **R6 ✅ DECISION** (freeze the as-built trust root — Branch B; demotion-to-stdlib deferred to its own pillar; gate stays active; 34-leaf genesis manifest pinned in §13a). **GENESIS RECONCILIATION COMPLETE (R1–R6).** The trust root is frozen in the §13 shape — the hygiene prerequisite before the registry's first publish is met.

**LOCAL-FIRST P3 COMPLETE + networked tier begun:** **P3.1.1 ✅** publish · **P3.2.1 ✅** inspect · **P3.3.1 ✅** memoize · **P3.5.1 ✅** fork · **P3.6.1 ✅** advise · **P3.3.2 ✅** shared trustless cache (the differentiator) · **P3.4.1 ✅** reputation-by-adoption (citation count over the published DAG; derived-not-asserted I8; unit-discovery, NOT result-trust) · **P3.8.2 ✅** transport-trustlessness (the boundary was already secured by content-addressing; built the one gap — `SharedCache::ingest`, the cache's verify-on-read analogue; the wire itself is deferred infra). **NETWORKED-TIER LOGIC COMPLETE. 194 green (156 core + 38 host).**

**Deferred — the wire only (infra, not a gap):** real transport (TCP/HTTP/libp2p, async runtime, peer discovery, framing) — its **security is already handled** by content-addressing (verify-on-read for units, `entry_key`/`ingest` for cache keys, `verify_entry` for results), so it's *plumbing* for a deployment phase behind whatever transport is picked. Also deferred (YAGNI): a `Remote` trait (until a 2nd transport), a networked `Store` impl (a `Store` whose `get` fetches bytes + calls existing `decode_verified` — no new trust logic). **P3.7.1 author-identity — DROPPED** (see `P3.7.1-identity-L4.md`): trust here is from verifiability, not identity. The principle kept *if ever revisited*: identity must be optional, external, an attestation **over** hashes, never in the preimage; core stays identity-free and trustless.

**Two P3.4.1 deferrals — both with identity-free resolutions (don't lose these):** (1) **Sybil-resistance of adoption** — flat citation count is inflatable by junk referrers; fix = recursive/eigenvector weighting (a citation weighted by the citer's own adoption; junk ≈0). A property of the content graph — **no identity**. Heavier (global iteration). *Trigger:* corpus large enough that flat counts are noisy, OR Sybil inflation observed. (2) **Expensive-result-trust** — results are *objectively verifiable*, so a wrong result is caught by anyone who checks regardless of who served it; the only real (narrow) problem is computations expensive enough that re-verifying is also expensive. Resolution is **identity-free**: re-verification / spot-checking for the common case, **validity proofs** (zk) for the expensive case — not reputation/identity. *Trigger:* a real workload where re-verification cost actually bites.

**`Store`→`Resolver` ownership fork — SET ASIDE with a trigger** (see `P3-store-resolver-fork-L4.md`): a `Cow`-trait perf change for lazy `eval`-against-store; reach for it only when a deploy-from-store path exists AND eager `materialize` profiles as the bottleneck.

---

## Method — deduction from the end state

We build the plan top-down by **necessity**, not by task list:

1. The **root** is the end-state, one sentence.
2. Each child is an **entailed requirement**: *for the parent to be true, this must
   hold.* If a child could be deleted and the parent still held, it does not belong.
3. Every node carries an **ID**, the **governing invariant** it serves (from §3 of
   the design doc), a **status**, and a **decision-state**. A node with no invariant
   behind it is cosmetic or mis-derived — flag it.
4. We track two **orthogonal axes**: *necessity* (what must exist) vs. *choice* (how
   we realize it). They are different questions and get different marks.
5. Recurse until leaves are **concrete, testable, buildable** units of work.
6. **Topologically sort the leaves** → that is the build order, and it must agree
   with design-doc §10. If the deductive order and §10 disagree, one is wrong.

A standing rule observed already: **requirements flow downward as well as up.** A
property the marketplace needs in order to be *trustless* lands as a hard constraint
on the core — which is why content-hashing was built before the evaluator was even
ported. Expect more of this; anything P3 needs to be trustless becomes a P1 law.

### Legend
- **Status:** ✅ proven (in engine or prototype) · ◐ partial · ○ open
- **Decision:** ▣ pinned · ◇ fork-open
- **Refs:** `I1`–`I9` = invariants; `§n` = design-doc section

---

## Level 0 — Root

> A recursively-composable, content-addressed computation fabric whose **pure
> interior is a functional language** and whose **effectful edges are a
> capability-gated OS**, exposed through a **permissionless, trustless-by-hashing,
> network-effected marketplace**.

The root is a conjunction of three pillars with a strict entailment order. Read
`A ⊳ B` as "A is presupposed by B" — A must be real before B is even meaningful.

---

## Level 1 — The three pillars

**P1 · The Language ⊳ P2 · The OS ⊳ P3 · The Marketplace**

- **P1 — The Language.** The pure composable core. Presupposes nothing; it is the base.
- **P2 — The OS.** The effect boundary. Presupposes P1 — effects sit at the edges *of*
  a pure graph; there is no "edge" without an interior.
- **P3 — The Marketplace.** The ecosystem. Presupposes P1 and P2 — it distributes
  units, and once units can be services (P2), it distributes those too.

This order is logical dependency, not preference: the spine (P1) must be finished
before P2 or P3 are anything but speculation. Both P2 and P3 are *theorems of P1*.

**P4 · The Editor — a parallel pillar, not in the spine.** It presupposes P1 (you
cannot author units without the unit model) and is an *optional client* of P3 (the
registry), but P2 and P3 do not presuppose *it*. It sits beside the spine because
it is the human authoring/inspection surface — convenience, not foundation.

**Deployment / runtime hosting — deliberately *not* a pillar.** Scoped to a **local
process**: `holon-host` (P2) run on localhost via `cargo run`. There is no
networked-hosting concern until we ever choose to add one. Recorded as a decision,
not an omission.

---

## Level 2 — Entailed requirements

### P1 · The Language — *for a recursively composable fabric to exist:*
- **P1.1** Uniform unit contract `evaluate(inputs,params)→outputs` — `I1` · ✅ ▣
- **P1.2** Interface parity: composite ≡ primitive at the interface — `I1` · ◐ ▣
  *(parity holds at the type/hash level in core; evaluation parity awaits P1.4)*
- **P1.3** Composition-checking type system — `§5` · ◐ ◇
  *(mono + `any` proven in prototype; not in core; generics fork-open)*
- **P1.4** A **deterministic** evaluator — `I2` · ◐ ◇
  *(exists only as throwaway JS prototype; not yet in `holon-core`; async/quiescence fork-open)*
- **P1.5** Encapsulation: derive-then-freeze a flow into a unit — `I4` · ◐ ◇
  *(proven in prototype; param-promotion representation is the open fork)*
- **P1.6** Content addressing + transitive pinning — `I5`/`I6` · ✅ ▣
  *(**landed in `holon-core`** — canonical hashing + closure commitment)*

### P2 · The OS — *for the effect boundary to be safe and legible:*
- **P2.1** Driver / state / sink taxonomy (the reversibility seam) — `I7` · ✅ ▣
- **P2.2** Derived, not asserted, labels (kind = union of children) — `I8` · ✅ ▣
- **P2.3** Capability declaration + union up the closure, surfaced before import — `I7` · ✅ ▣
- **P2.4** Effect-firing model: quiescence / settle-gate before irreversible sinks — `§6` · ◐ ▣ *(design + deploy-minimal built; `drive`, state, B-fan-out, async remain)*
  *(the central unsolved boundary)*
- **P2.5** Effect contracts: idempotency, transactional read-then-write, req/resp correlation — `§4` · ✅ ▣
  *(all three leaves complete — correlation/idempotency both a label riding as data; the real
  multi-process transactional adversary is deferred to platform hosting, not a core gap)*
- **P2.6** State-cell semantics under replay / fork / pin — `§7` · ✅ ▣
  *(P2.6.1 built the cell; P2.6.2/P2.6.3 dissolved into it — corollaries of the host-storage
  decision, not separately built code)*
  
### P3 · The Marketplace — *for a trustless, network-effected ecosystem:*
- **P3.1** Registry: alias→hash resolution, publish, immutability — `I5` · ○ ▣
  *(alias-resolves-to-hash decided; rest open)*
- **P3.2** Transitive inspection to the leaves — `I9` · ○ ▣
- **P3.3** Universal memoization: shared `(closure-hash, input-hash)` cache — `§8` · ○ ▣
  *(mechanism pinned; **preconditions P1.4 + P1.6 now half-met**)*
- **P3.4** Reputation by adoption, not raw usage — `§8` · ○ ▣
- **P3.5** Copy-on-write forking + explicit re-pin (no silent update channel) — `§8` · ○ ▣
- **P3.6** Advisory / yank channel: flag a hash, never mutate frozen bytes — `§8` · ○ ▣
- **P3.7** Author identity, costly to forge — `§9` · ○ ◇
- **P3.8** Content-addressed store to distribute units — `I5` · ○ ◇

### P4 · The Editor (parallel) — *for humans to author, inspect, and run units conveniently:*
*Presupposes P1; optional client of P3; not in the spine.*
- **P4.1** Graph authoring canvas: place units, wire ports, set params; wire-time type checking — `§5`/`I1` · ◐ ▣
  *(proven in JS prototypes v1–v3; not yet in the real stack)*
- **P4.2** Live evaluation that **embeds `holon-core` (WASM)** — one evaluator, never a second, so editor results are bit-identical to native — `I5`/`I2` · ○ ▣
  *(the load-bearing constraint; see findings)*
- **P4.3** Encapsulation UX: group a selection into a unit; expand to inspect — `I4`/`I9` · ✅ ▣
  *(now real in the actual stack, not just v3 — select/group/expand all browser-confirmed, no
  automated coverage; see P4.3.1 below and JOURNAL.md, 2026-07-01, both P4.3.1 entries)*
- **P4.4** Transitive inspection UI: descend a composite to its leaf primitives — `I9` · ✅ ▣
  *(the human face of P3.2 — now real in the actual stack, browser-confirmed; see P4.4.1 below
  and JOURNAL.md, 2026-07-01 · P4.4.1)*
- **P4.5** Registry client: browse / import-as-pin / publish — `§8` · ○ ◇
  *(needs P3; deferred until the marketplace exists)*

---

## Level 3 — P1 · The Language, expanded to leaves

*Each leaf carries: **pre** (precondition leaves) · **inv** (invariant served) ·
**test** (the proof it is done) · **effort** (S/M/L). They are listed in build order
— the topological sort of their preconditions.*

**Fork resolved before descent.** P1.4's eager-vs-async fork is settled by
*relocation*, not choice: the core evaluator is **eager and synchronous**
(glitch-free, simplest, already proven in prototypes). Async / autonomous cells only
matter where a glitch can do harm — the irreversible effect boundary — so that fork
moves to **P2.4**. The Language is eager; the OS decides when effects fire.

1. **P1.4.1 · Value model** — the runtime `Value` (`Number`/`Boolean`/`Text`/`List`),
   immutable, value-semantics.
   *pre:* none (foundational) · *inv:* `I2` · *test:* `type_of(value)` agrees with the
   `Type` tag; clone/equality are by value · *effort:* **S**

2. **P1.3.1 · Graph validation** — well-formedness (edge indices in range, no dangling
   ports), **acyclicity** (the evaluator presupposes a DAG), and wire-time **type
   compatibility** (`dst=any ∨ src=any ∨ src=dst`).
   *pre:* Unit model + `Type` (done) · *inv:* `§5`/`I1` · *test:* a valid graph passes;
   a type-mismatched edge, a cycle, and an out-of-range edge are each rejected ·
   *effort:* **M**
   *Deductive note:* validation is **upstream of** evaluation — a graph must be proven
   acyclic and type-sound before it can be evaluated.

3. **P1.4.2 · Primitive registry + native compute** — `name → fn(inputs,params)→Value`
   for the initial pure set. The complete set (small + orthogonal, KISS):
   *constants* `Number` `Text` `Boolean`; *arithmetic* `Add` `Subtract` `Multiply`
   `Divide`; *comparison* `Compare` (op param: `> < ≥ ≤ =`); *logic* `And` `Or` `Not`;
   *control-as-data* `Select(cond,a,b)`; *text* `Concat` `Uppercase`; *list* `List`
   `Get` `Append` `Length` `Sum` `Join`. That is **22 primitives**; the 23rd, the
   higher-order **`Map(list, fn)`**, is gated behind P1.5.1 (its `fn` is a *unit
   reference*) and lands last. `Filter`/`Reduce` are deferred (§10 step 3).
   *Deliberately absent:* `Timer` / `Endpoint` / `Store` / `Email` — those are P2
   boundary units (driver/state/sink), a different kind, not pure vocabulary.
   *pre:* P1.4.1 · *inv:* `I1`/`I2` · *test:* each primitive matches expectation
   (`Add[3,7,5]=15`) and is deterministic · *effort:* **M**

4. **P1.4.3 · Eager graph evaluator** — resolve a composite's inner DAG in topological
   order, inject promoted inputs, read the exposed output, **recurse** for nested
   composites; a primitive and a composite are evaluated through the identical path.
   *pre:* P1.4.1, P1.4.2, P1.3.1 · *inv:* `I1`/`I2`/`I3` · *test:* port the v3 seed
   (`list→Map(Double)→[6,14,10]`, `list→Sum→15`) to Rust and reproduce it; a nested
   composite evaluates; children resolve as black boxes · *effort:* **L**
   *→ **L4 detailed** in `P1.4.3-evaluator-L4.md`. Headline: the firing rule **splits** —
   the pure core handles `required/optional/default`; `consume`/`persist` (port memory,
   firing rounds) defer to the state boundary and async model (P2.4.2). Eager = a single
   topological pass; children resolved by hash via a `Resolver` trait.*

5. **P1.4.4 · Determinism harness** — ordered structures only (`BTreeMap`/`IndexMap`,
   never `HashMap` iteration), pinned float behaviour, cross-run/cross-target identity.
   This is the precondition P3.3 (shared cache) stands on.
   *pre:* P1.4.3 · *inv:* `I2`/`I5` · *test:* repeated — and later native-vs-WASM —
   evaluation of one graph yields identical results; fuzzed · *effort:* **M**

6. **P1.2.1 · Interface-parity proof** — an equivalence test, not new machinery: a
   primitive and a composite with the same interface are substitutable everywhere.
   *pre:* P1.4.3 · *inv:* `I1` · *test:* swap a primitive for an equivalent composite
   in a host graph; identical results, no special-casing · *effort:* **S**

7. **P1.5.1 · Encapsulation (derive-then-freeze)** — given children + edges + a
   promotion spec, derive the interface (unbound inputs promote, chosen output
   exposed, **kind = union of children**) and freeze it into a hashed `Unit`.
   *pre:* P1.6 (done), P1.3.1, P1.4.1 · *inv:* `I4`/`I8` · *test:* derived interface
   matches expectation; a selection containing a sink yields sink-kind; derivation is
   deterministic · *effort:* **M**
   *→ **L4 detailed** in `P1.5.1-encapsulation-L4.md`. Headline: derive-vs-choose split —
   types + boundary edges are **derived**; which inputs promote, their **order** (= identity),
   names, and the exposed output are **chosen** (in `EncapSpec`). `derive_and_freeze`
   generates exactly the `BOUNDARY` edges eval/validate consume; `default_spec` gives the
   canonical proposal. Kind-derivation stays P2; param promotion stays P1.5.2.*

8. **P1.5.2 · Param promotion** — `Param = Frozen(value) | Open(name)` + a binding
   table on the composite; the encoder tags the two states; the evaluator injects
   supplied params into open slots.
   *→ **L4 detailed** in `P1.5.2-param-promotion-L4.md`. Refines the L3 sketch:
   **instantiate-time specialization** (not eval-time injection) — `instantiate(template,
   bindings)` produces a new concrete content-addressed unit; binding is **by name** (no
   binding-table field); the open-param interface is **derived**. Encoder change = a
   one-byte Frozen/Open tag on params.*
   *pre:* P1.4.3, P1.5.1, P1.6 · *inv:* `I4`/`I5` · *test:* `Double`(b=2) and
   `MultiplyByN`(b=open) hash differently; a promoted composite evaluates with the
   supplied value · *effort:* **M**

---

## Level 3 — P2 · The OS (effect boundary), expanded to leaves

P2 changes *what the engine is*: P1's evaluator runs and returns; P2 makes it a
**runtime that runs** — something owns the clock, holds connections, performs
consequences. So the crate split does real work here:

> **Classification is pure analysis → `holon-core`. Execution — drivers, sinks,
> state storage, the firing loop → `holon-host`.** The crate graph keeps "the core
> cannot perform an effect" a compile-time fact even as the effect layer is built.

**The async fork lands here — and is immediately deferred.** P1.4's relocated
eager-vs-async fork becomes P2.4. The deduction resolves it: in the **eager** model
the graph runs to a fixed point *before* any sink fires, so the settled value *is*
the value the sink sees — **quiescence is free.** §6's "central unsolved boundary"
only bites in the async/autonomous-cell future. Near-term P2 is entirely eager;
async is a fork-open leaf we do not need yet.

*Build order (topological); core/host placement noted per leaf:*

1. **P2.1.1 · Kind taxonomy** *(core)* — `Kind = Pure | Driver | State | Sink`. Kind is
   **informative, not identity**: it is a *computed view*, never a stored or hashed
   field. A primitive's kind follows authoritatively from its name (already in the
   hash); a composite's is derived (P2.2.1). Nothing to forge, nothing to drift.
   *pre:* P1.4.2 · *inv:* `I7` · *test:* the registry resolves the correct kind per
   primitive (pure ops → Pure, `Store` → State, `Email` → Sink) · *effort:* **S**

2. **P2.6.1 · State-cell primitive** *(core decl / host storage)* — **complete ✅.** The cell
   as **two leaves** (`store_read` driver-like + `store_write` Fork-B sink); the one fenced
   place purity ends; kind = State. The **donor primitive** for all effect contracts (P2.5).
   Core: the split, `store_write` identity `compute`, `Capability::is_sourced` (input/output
   axis), `mock_of` mocks the sourced side. Host: `StateHost` threads one anonymous cell —
   `fire` performs `!is_sourced`, a read-substituting resolver injects the snapshot, the
   snapshot→fire→apply round; **the counter climbs 0→1→2→3**. No `eval`/`validate` change, no
   graph cycle (storage is the register). One anonymous scalar cell; keys / non-scalar
   deferred.
   *pre:* P1.4.2, P2.1.1 · *inv:* `§7`/`I7` · *test:* `store_read` mockable, `store_write`
   pure; cell-bearing closure non-replayable; **the counter remembers across firings** ·
   *effort:* **M** *(done)*
   *→ **L4** in `P2.6.1-state-cell-L4.md`: two-leaf, storage = register, snapshot-apply.*

3. **P2.2.1 · Derived-kind computation** *(core)* — `kind(composite) = union of children`,
   computed over the closure each time it is asked; **never stored.** Because there is
   no kind field to set, laundering an effect into a "pure" label is not *prevented* —
   it is *inexpressible*.
   *pre:* P2.1.1, P1.4.3 · *inv:* `I8` · *test:* a composite with one `Sink` leaf at any
   depth computes to Sink-kind; a pure-only closure computes Pure · *effort:* **M**

4. **P2.3.1 · Capability model** *(core)* — the fine-grained, security-relevant manifest
   of what an effect *touches*: `NetworkEgress(host)`, `FileRead/Write(path)`,
   `SecretAccess(name)`, `Clock`, `Spawn`, … A pure primitive declares the **empty set**
   — a positive, checkable claim that it touches nothing outside its inputs. (Kind is
   the four-bucket *projection* of this: "has any irreversible capability" ≈ Sink.)
   *pre:* P2.1.1 · *inv:* `I7` · *test:* an effectful primitive declares its caps; pure
   ones declare ∅ · *effort:* **M**

5. **P2.3.2 · Capability union, surfaced before import** *(core)* — a composite's caps =
   union of its closure's, **computed not stored** (same principle as kind), exposed
   *before* a unit is run or imported.
   *pre:* P2.3.1, P2.2.1 · *inv:* `I7`/`I9` · *test:* a `NetworkEgress` capability buried
   deep in the closure surfaces at the top before import — **nothing exfiltrates
   silently** · *effort:* **M**

6. **P2.4.1 · Eager effect-firing** — **split into two modes by the substituting-resolver
   insight (L4 rev. 2).**
   - **Design mode** *(pure `holon-core` — built)*: effect leaves are substituted by pure
     mocks **at the resolver**, and the closure runs under core `eval` — deterministic,
     replayable, memoizable. `mock_of` + `Simulated` + `design_eval`; the "loop" *is* `eval`,
     and the core never learns a mode exists.
   - **Deploy mode** *(host — minimal built ✅):* the runtime loop in `holon-host` — `fire`
     settles the graph (core `eval`) and the `Host` performs the request its **output**
     carries (the one impurity). Minimal = a single terminal sink (representation A);
     `drive` (real driver sourcing), state threading, and B-fan-out remain.
   *pre:* P1.4.3, P2.1.1 *(design)* · P2.6.1 *(deploy — state threading)* · *inv:* `§6`/`I7`
   · *test (design, ✅):* a graph with a **driver** evaluates purely (drivers mocked at any
   depth); a **sink** evaluates directly (pure under Fork B); classification still reports
   the real effects (the guardrail) · *test (deploy):* the host performs each sink's
   *(capability, payload)* exactly once per settled trigger · *effort:* **M** *(design half done)*
   *→ **L4** in `P2.4.1-firing-loop-L4.md` (rev. 2) + `sink-as-pure-request-L4.md` (Fork B):
   substituting-resolver frame for drivers; sinks are pure request-constructors; α′ demoted
   to a deploy sink-observation option; state is host-threaded (P2.6.1).*

7. **P2.6.3 · State-cell replay/fork/pin semantics** *(core + host)* — **complete ✅ (by
   dissolution).** Every sub-part is a corollary of P2.6.1's host-storage decision: **value-not-
   hashed** is *unrepresentable to violate* (`hash_unit` takes a `Unit`; the value lives in the
   host — `Value` isn't in the preimage); **cell-bearing ⇒ non-replayable** is tested
   (`state_graph_is_not_replayable`, the positive form on `is_replayable`); **fork = copy-on-
   write** is a new host over a cloned storage (tested — original→8, fork→7 diverge); **pin**
   pins structure, state stays runtime → drifts to **P3** (pinning configured-stateful units is
   a marketplace concern).
   *pre:* P2.6.1, P1.6 · *inv:* `§7`/`I5` · *test:* fork CoW diverges; non-replayable (existing);
   value-not-hashed holds by construction · *effort:* **M** *(done — one demo test, no prod code)*

8. **P2.6.2 · Transactional read-then-write** *(host)* — **complete ✅ (by dissolution).** The
   round *is* the transaction: `&mut self` serializes rounds, and snapshot → fire → apply is
   atomic by construction, so the write lands computed from the value read with no exploitable
   gap. A conditional read-modify-write **composes** (`store_read → compare → select →
   store_write`) — falsifying the leaf's "cannot be composed" premise. The genuine adversary
   (a *concurrent* writer / *shared external* state) is multi-process, and there "transactional"
   is the **backing store's** primitive (DB txn / CAS) carried through the `Host` seam — **not a
   core primitive** → deferred to platform hosting.
   *pre:* P2.6.1 · *inv:* `§7` · *test:* saturating counter `0→1→2→2→2` — one atomic conditional
   update · *effort:* **M** *(done — one demo test, no prod code)*

9. **P2.5.1 · Correlation token in the value model** *(core + host)* — **complete ✅.** Resolved
   *against* its own framing: the token is **not** an isolation mechanism — **non-crossing is a
   theorem of purity** (independent requests, independent pure paths, no shared channel; only a
   *state cell* can mix them, by the user's choice). The token is only a **boundary label** (which
   open connection to reply on), carried as **explicit data** in the `[token, reply]` payload — no
   `Value` change (implicit would *be* the shared channel non-crossing's freedom forbids). Caps:
   `request → NetworkIngress`, `respond → NetworkEgress`; a `request→respond` closure is
   effect-bearing yet **replayable**. Demo: `map(respond, ·)` over two `[token,data]` requests —
   tokens don't cross. Deferred: real socket routing (platform host), client-await (async/`P2.4.2`),
   ingress batch sourcing (non-scalar `constant_unit`).
   *pre:* P1.4.1, P2.4.1 · *inv:* `§4` · *test:* two concurrent requests routed correctly, none
   cross · *effort:* **M** *(done)*
   *→ **L4** in `P2.5.1-correlation-token-L4.md`.*

10. **P2.5.2 · Idempotency-key affordance on sinks** *(host)* — **complete ✅.** The key is plain
    data in the `[key, body]` payload (no core change; `perform` already carries it). It works by
    the *opposite* of correlation: purity makes a re-fire **reproduce** the effect, and determinism
    (P1.4.4) makes the re-fire rebuild the **same key** — turning a double-fire into a dedup-able
    event. Engine guarantees stability (= determinism); user derives the key from intent (must be
    **replayable** — a driver/`State` key would vary; `is_replayable` could guard, future). Dedup
    is the external system's (`DedupeHost` models it; real → platform host).
    *pre:* P2.6.1, P2.4.1 · *inv:* `§4` · *test:* same intent fired twice → performed once · *effort:*
    **M** *(done)*

**Execution modes (design / deploy) — simulation doubles at the seam:**

11. **P2.7.1 · Simulation doubles — drivers only** *(core decl)* — **revised by Fork B.**
    Only a **driver** needs a design-mode mock (it is external input the host supplies; the
    mock *injects* a scripted stimulus). A **sink** needs none — under Fork B it purely
    *constructs its request* (identity-on-payload), so `eval` runs it directly. **State**
    is host-threaded, not mockable. Pure primitives are their own mock. **The mock is never
    part of the content hash.**
    **[Built with `P2.4.1` + the Fork B reconciliation:** `mock_of(unit)` substitutes a
    driver with a type-zero constant; `Sink | State | Pure ⇒ None`. The earlier sink
    passthrough mock was retired (it assumed Fork A). Remainder — custom stimuli / ephemeral
    test-context doubles — stays open.**]**
    *pre:* P2.1.1, P1.4.2 · *inv:* `I5`/`§4` · *test:* a driver exposes a mock; a sink does
    not (it evaluates purely); the mock is never in any hash · *effort:* **M** *(done)*

12. **P2.7.2 · Mode-aware firing (design / deploy)** *(host)* — **complete ✅ (the last P2 leaf).**
    `Mode{Design,Deploy}` + `run` name the choice the host already had: `design_eval` (mocks
    drivers → whole graph pure → performs nothing, a safe preview) vs `fire_sourced` (real,
    performs). `fire`/`fire_sourced` now *return* the settled output, so design hands back the
    preview value. The **core never learns of modes** — both call the one `eval`; the mode only
    chooses what the resolver injects and whether effects fire. Test: one `timer→email` graph
    previews `Effect{email,0}` (nothing sent) in design, performs `(NetworkEgress, 42)` in deploy.
    *Decision pinned:* mocks are a **leaf property** — composites always inherit, never
    carry their own. Custom mocks / stimuli are allowed only as **ephemeral test-context
    overrides** a user sets for a session, never baked into a unit's identity.
    *Naming:* this is a *simulation double* (run safely), distinct from a future
    *assertion test* (checked expectation, "output should equal 15"); keep them named apart.

**Guardrail (constraint, not a leaf):** **kind and capabilities are always computed from
the *real* units, never the mocks.** Design mode is never the basis of a trust decision —
otherwise a malicious unit could ship a benign mock around a real effect (a laundering
channel). The capability surface (P2.3.2) reflects what *deploy* mode will actually do, so
a unit can simulate as innocent yet still be forced to declare its real `NetworkEgress`
before import. *Design mode tests your wiring; the capability surface lets you trust the unit.*

**Deferred out of near-term P2 (fork-open, not leaves now):**
- **P2.4.2 · Async firing + quiescence gate** — autonomous cells; an explicit
  settle/commit gate in front of irreversible sinks; never a debounce smeared across
  the graph. Unsolved + unnecessary while eager (see above). `§6` · *effort:* **L**

**P2 deductive findings (banked):**
- **Quiescence is free in the eager model** — the hardest open problem in the design
  (§6) does not block the effect boundary; it defers to the async future.
- **The trust story is pure code.** Derived labels (I8) and capability-union-before-
  import (I7/I9) are *analysis over structure* — they live in `holon-core` and are
  provable **without executing a single effect.** Only the firing loop and the actual
  consequences live in `holon-host`.
- **State forces a value/identity split.** A cell's runtime value is excluded from its
  content hash (P2.6.3), which is the clean resolution that keeps I5 intact in the
  presence of state.
- **Derivable ⟹ derived (never stored).** Kind and capabilities are pure functions of
  the hashed structure (primitive name + child hashes), so they are **computed views,
  never fields in the preimage.** A general principle for the whole project: anything
  recomputable from the closure is computed, not committed — which is *why* forgery is
  inexpressible rather than merely guarded against.
- **Engine provides the donor; the user composes the disciplines.** The P2.6/P2.5 line,
  named precisely: **P2.6** is *what the engine provides* — one stateful primitive plus
  its honest semantics. **P2.5** is *what the user composes with it* — idempotency,
  reconciliation, correlation — the safety disciplines the engine deliberately does
  **not** build in. The engine's sole addition to P2.5 is *carrying* the plumbing the
  user cannot thread purely (correlation token, idempotency key in the value envelope).
  This is "minimal unopinionated engine; user owns correctness; engine makes correctness
  *constructible*" expressed as a build boundary.

**Consistency check vs design-doc §10.** §10 ordered: state semantics → async →
effect contracts. The deduction refines this: the state *primitive* (P2.6.1) must
come **earlier** than §10 implied, because capabilities and effect contracts
presuppose it; and **async (P2.4.2) is deferrable**, because eager evaluation makes
quiescence free. Consistent in spirit, sharpened in order.

---

## Level 3 — P3 · The Marketplace, expanded to leaves

P3 is where holon becomes an ecosystem — but scoped to **run locally**, it splits in
two, and the split is the main finding:

> A **local-first core** (single machine, no server) buildable right after P1/P2, and
> a **networked / social layer** deferred until the local system is real and there is
> a reason to network it. Most of P3 is **exposure, not invention**: its hard
> preconditions — content-addressing (P1.6 ✅), determinism (P1.4.4), derived
> kind/caps (P2.2/P2.3) — are already derived. Each leaf mostly *surfaces a property a
> lower pillar already produces.*

**Local-first leaves (buildable on one machine, in build order):**

1. **P3.8.1 · Content-addressed store (local CAS)** *(host; verify in core)* — **complete ✅.**
   `Store` trait (owned `put`/`get` — **not** a `Resolver`, so eval never touches disk),
   `MemStore` + on-disk `FileStore` sharing one encode/key/verify/decode path. The byte
   format **is** the hash preimage; core gained `decode_preimage` (byte-exact inverse of
   `encode_preimage`) + `Type::from_tag`. Verify-on-read = re-hash the decoded unit, refuse
   unless it equals its key (`I5` operational; no `blake3` in host). 136 green (117c + 19h).
   *pre:* P1.6 · *inv:* `I5` · *test:* `store(unit)→hash`; `fetch(hash)→` identical unit;
   a tampered/mismatched blob is rejected · *effort:* **M**

2. **P3.1.1 · Registry: alias→hash, publish, immutability (local)** *(host)* — a
   `name@version` alias resolves to a hash; publish writes unit→store + alias→hash;
   **a published alias is immutable** (resolves to one hash forever).
   *pre:* P3.8.1 · *inv:* `I5` · *test:* publish `rsi@1.0`→hash; resolve →same hash;
   republishing `rsi@1.0` to a different hash is refused · *effort:* **M**

3. **P3.2.1 · Transitive inspection to the leaves** *(core traversal / host fetch)* —
   descend a closure by the same pins that guarantee execution, to the leaf primitives,
   annotated at every level with derived kind + capabilities.
   *pre:* P3.8.1, P2.2.1, P2.3.2 · *inv:* `I9` · *test:* descend a 3-level composite to
   its primitives; a sink/capability buried deep is reachable and visible; inspection
   never stops one level short · *effort:* **M**

4. **P3.3.1 · Memoization cache, keyed `(closure-hash, input-hash)` (local)** *(host)* —
   cache a **pure** unit's result; reuse on hit. Sound *only* for pure subgraphs — a
   State-bearing closure is never cached.
   *pre:* P1.4.4, P1.4.1, P1.6, P2.2.1, P3.8.1 · *inv:* `§8`/`I5` · *test:* same pure unit
   + same inputs hits cache, identical bytes; a State-bearing subgraph is not cached ·
   *effort:* **M**
   *Deductive note:* the cache's trustlessness is **inherited**, not asserted — it holds
   because units are content-addressed (P1.6), evaluation is deterministic (P1.4.4), and
   only pure closures are keyed (P2.2.1). This is the loop closing: P3.3 is the
   requirement that *forced* P1.6 + P1.4.4 at the start, now consuming exactly them.

5. **P3.5.1 · Copy-on-write fork + explicit re-pin (local)** *(core derive / host)* —
   inspect freely; editing a depended-upon unit yields a **new hash** (original
   immutable); "pull an upstream fix" is an explicit re-pin (bump child hash → re-derive
   parent → new parent hash), never a silent channel.
   *pre:* P3.1.1, P1.5.1, P1.6 · *inv:* `§8`/`I3`/`I5` · *test:* forking a pinned unit
   yields a new hash, original untouched; re-pinning a child produces a new, reviewable
   parent hash · *effort:* **M**

6. **P3.6.1 · Advisory / yank channel (local)** *(host)* — flag a hash as advised-against
   and point to a fixed version; **never mutate the frozen bytes**. A closure containing
   a flagged hash surfaces the advisory.
   *pre:* P3.8.1, P3.2.1 · *inv:* `§8`/`I5` · *test:* flag a hash; a unit whose closure
   contains it reports the advisory + suggested replacement; the flagged bytes remain
   unchanged and resolvable · *effort:* **M**

**Networked / social layer (deferred, fork-open — needs a server + multiple participants):**

7. **P3.3.2 · Shared trustless cache** — the local cache, shared across users; trustless
   because the hash is the proof. *pre:* P3.3.1 + network · `§8` · *effort:* **L**
8. **P3.4.1 · Reputation by adoption** ✅ — count *independent units* that compose a
   unit (the dependency graph counting itself; expensive to forge); identity-free.
9. **P3.7.1 · Author identity — DROPPED** (see `P3.7.1-identity-L4.md`) — over-engineering
   against the thesis; trust here is from verifiability, not identity. Adoption's Sybil-hardening
   is identity-free (recursive weighting).
   *pre:* networked registry · `§9` · *effort:* **L** *(fork-open)*
10. **P3.8.2 · Networked content-addressed store** — distribute units across machines.
    *pre:* P3.8.1 + network · `I5` · *effort:* **M–L**

**P3 deductive findings (banked):**
- **The marketplace's core is single-user.** Store, registry, inspection, local
  memoization, copy-on-write fork, and advisories all run on one machine with **no
  server**. The social features are a later expansion, not a precondition — so P3 is
  reachable right after P1/P2, not a far-future pillar. (Aligns with "run locally.")
- **P3 is mostly exposure, not invention.** Memoization = key the cache by hashes P1.6
  already computes. Inspection = render the closure P1.4.3 already traverses, annotated
  with kind/caps P2 already derives. Adoption = count dependency edges the pinned
  closure already stores. Its preconditions being done is *most* of the work.
- **The deduction's loop is closed.** At L1 we banked "P3.3 forces P1.6 + P1.4.4."
  Descending P3 confirms P3.3 *consumes* exactly those. The top of the tree dictated the
  floor; the floor now supports the top. End-to-end consistent.

**Consistency check vs design-doc §10.** §10 lumped the whole marketplace into one late
step (#7). The deduction **splits** it: the *local* mechanics are near-term (right after
P1/P2); only the *networked/social* layer is genuinely late. §10 under-ordered by
treating "marketplace" as monolithic.

---

## Level 3 — P4 · The Editor, expanded to leaves

P4 is a **client of the core**, and its deductive signature is *delegation*: every
decision — is this wire legal, what does this evaluate to, what is this composite's
interface / hash / kind / capabilities — is answered by `holon-core` (via WASM),
never reimplemented. The editor's genuine job is **presentation, interaction, and the
cosmetic layer** (positions, labels) the core deliberately does not know about.

*Build order; layer = TS/React editor over WASM core:*

1. **P4.2.1 · `holon-core` → WASM bindings** *(the load-bearing leaf)* — compile the
   core to WASM; expose its API (validate, evaluate, hash, derive-interface,
   derived-kind/caps) to the editor. One evaluator, one decider.
   *pre:* P1.4.3, P1.4.4, P1.6 · *inv:* `I5`/`I2` · *test:* a graph evaluated via the
   WASM core in-editor returns bytes **identical** to the native core · *effort:* **M**

2. **P4.1.1 · Authoring canvas** — palette, place/move (cosmetic), wire ports, set
   params; the editor's model (with cosmetics) **projects** to the core's structural
   `Unit`.
   *pre:* P4.2.1 · *inv:* `I1`/`§5` · *test:* a visually-built graph projects to a valid
   core `Unit`; a cosmetic move changes the editor model but **not** the projected hash ·
   *effort:* **L**

3. **P4.1.2 · Wire-time type check (via core)** — wiring calls the core's compatibility
   check (P1.3.1); the editor renders the rejection, never re-judges.
   *pre:* P4.1.1, P1.3.1 · *inv:* `§5` · *test:* a type-mismatched wire is refused
   in-editor with the *core's* answer · *effort:* **S**

4. **P4.3.1 · Encapsulation UX** — select a subgraph → group (editor calls core's
   derive-then-freeze P1.5.1, receives the new `Unit` + hash); expand to inspect.
   *pre:* P4.1.1, P1.5.1 · *inv:* `I4`/`I9` · *test:* grouping in-editor yields a unit
   whose interface + hash match the core's derivation; expand round-trips · *effort:* **M**
*→ **This leaf is DONE** — select, group, and expand all browser-confirmed; no automated
   coverage yet, named explicitly in JOURNAL.md rather than silently carried forward (2026-07-01
   · P4.3.1, both entries). **Bridge:** `propose_encapsulation`/`freeze_encapsulation` (split,
   as this note originally asked for) plus `get_unit` (a direct resolver lookup by hash, not a
   preimage round-trip — `load_unit` turned out to be the wrong primitive for this once the
   actual data flow was traced). `project()` itself untouched throughout. **Two deliberate
   boundaries, not gaps:** expand goes one level only (a nested composite child stays collapsed —
   deep, annotated descent is `P4.4.1`'s job); it's a structural round-trip, not a positional one
   (node ids/canvas positions were never kept, so expand reconstructs the same children/wiring,
   freshly laid out). "Choose the exposed output" is still exclude-only (fork 3), not full
   reorder/rename — a separate, smaller, already-named limit, not part of this leaf's own scope.*

5. **P4.4.1 · Transitive inspection UI** — render the closure tree P3.2.1 produces,
   annotated with derived kind + capabilities at every level, to the leaves.
   *pre:* P4.1.1, P3.2.1 · *inv:* `I9` · *test:* inspect a 3-level composite; every level
   to the primitives shows correct kind/caps; nothing stops one level short · *effort:* **M**
   *→ **DONE**, browser-confirmed (2026-07-01 · P4.4.1, JOURNAL.md). No new recursive bridge
   capability was needed — `capabilities`/`effects_of` (new bridge methods, wrapping already-
   built core functions) already recurse internally, so calling either once per hash while
   walking with `get_unit` (already built for `P4.3.1`) gives each level's own manifest.
   Genuinely read-only — confirmed, not assumed, to never touch canvas state, the real
   distinction from `P4.3.1`'s expand. Scoped to canvas-placed nodes only; nothing in the
   underlying calls is canvas-specific, so registry-browsing (`P4.5.1`) inherits this for free
   later.*

6. **P4.4.2 · Capability-consent gate** — before importing/running a unit, surface its
   capability union (P2.3.2) and require explicit consent. The human face of
   "capabilities surfaced before import."
   *pre:* P4.4.1, P2.3.2 · *inv:* `I7`/`I9` · *test:* importing a unit with `NetworkEgress`
   shows the capability and blocks until acknowledged · *effort:* **S–M**

7. **P4.5.1 · Registry client** — browse; import-as-pin (alias→hash→fetch→pin); publish
   (derive→freeze→store→alias).
   *pre:* P4.1.1, P3.1.1, P3.8.1 · *inv:* `§8` · *test:* import by alias → pinned by hash;
   publish → resolvable by its new alias · *effort:* **M**

8. **P4.6.1 · Design / Deploy mode toggle + mock editing** *(editor)* — a mode switch on
   the canvas; in **design** the graph runs with mocks + driver stimuli and results
   display live (safe, glitch-free); in **deploy** real effects fire. The user can set
   custom mocks / stimuli as ephemeral test-context.
   *pre:* P4.1.1, P2.7.2 · *inv:* `§6` · *test:* toggling to design runs the graph with
   mocks and shows results; a custom mock/stimulus affects the design run but **not** the
   unit's hash · *effort:* **M**

**P4 deductive findings (banked):**
- **The editor invents almost nothing.** It is a *view*; the core is the *model*. Any
  logic duplicated in the editor (a second type-checker, a second evaluator, a second
  interface-deriver) is a future divergence bug. The sideways "one evaluator"
  constraint generalizes to **one decider**.
- **The cosmetic/structural boundary lives in P4's projection.** The editor is the only
  pillar that holds cosmetics; its project-to-core step is where "if it changes the
  result it's structural, else cosmetic" becomes tangible — moving a node changes the
  editor model but not the hash. P4 is where P1.6's normalization is felt by a human.
- **P4 has no new invariant-level difficulty.** Its hardest leaf (the WASM marshalling
  boundary) is engineering, not design — as expected: the editor is convenience, the
  spine is foundation. *(P4 is also a pillar §10 omitted entirely; the plan fills that gap.)*

---

## Cross-cutting findings (banked)

- **Design mode is the seam doing its job.** Substituting mocks at the effectful leaves
  turns the *whole* graph pure, so a design-mode run inherits every pure-graph property
  at once — deterministic, replayable, glitch-free, safe. It is **compositional** (mock
  at leaves; simulation propagates up the closure like kind and capabilities) and it
  costs the pure core nothing — it is a host concept; the core never learns about modes.
  Safety, though, is *always* read from the real units, never the mocks.
- **Requirements flow downward.** P3.3 (trustless shared cache) is sound *only if*
  P1.4 is bit-deterministic and P1.6 is content-addressed — so two "language design
  choices" are in fact **forced by the marketplace**. The top of the tree dictates
  the floor.
- **The real frontier is narrower than it feels.** Exactly one P1 node is fully
  landed in the production engine (**P1.6**). The evaluator (P1.4) still lives only
  as JS. Deductively, `holon-core` today has **identity but not evaluation** — the
  spine is not done. Porting a deterministic evaluator into Rust is therefore more
  load-bearing than the more exciting param-promotion work, and likely comes first.
- **Requirements also flow *sideways*.** P4.2 forces the Editor to embed
  `holon-core` compiled to WASM rather than reimplement evaluation in JS. Two
  evaluators would eventually disagree, and the instant they do, I5 is false and the
  shared cache (P3.3) is poisoned. So "compile the core to WASM" is a hard
  requirement the **Editor places on the Language** — and a reason P1.4 must be
  built WASM-targetable from its first line (which `holon-core` already is).

---

## Master build order — the topological sort of all leaves

The method's payoff (step 6): with every pillar at leaf resolution, the leaves sort by
dependency into **phases**. Within a phase, order is flexible; phases are strict. A
single brittle 1..N list would over-claim precision — the phases are the real contract.

- **Phase A · Evaluation** *(pure `holon-core`)* — `P1.4.1 → P1.3.1 → P1.4.2 → P1.4.3 →
  P1.4.4 → P1.2.1`. Closes the "identity but not evaluation" gap. **Starts now;
  `P1.4.1` is the first inch.**
- **Phase B · Encapsulation** *(pure `holon-core`)* — `P1.5.1 → (Map lands) → P1.5.2`.
  Full recursion + genuinely reusable units.
- **Phase C · Effect boundary** *(`core` classify + `host` execute)* — planned:
  `P2.1.1 → P2.6.1 → P2.2.1 → P2.3.1 → P2.3.2 → P2.4.1 → P2.6.3 → P2.6.2 → P2.5.1 →
  P2.5.2 → P2.7.1 → P2.7.2`.
  *(P2.7 adds design/deploy modes — the whole graph runs purely in design mode.)*
  **Realized so far:** `P2.1.1 → P2.2.1 → P2.3.1 → P2.3.2 → P2.4.1(design) + P2.7.1(minimal)
  → sink reconciliation (Fork B: sinks pure, effect-bearing⊥replayable) → P2.4.1(deploy-minimal):
  holon-host real, the purity seam crossed → P2.6.1(core half): state cell = two leaves,
  is_sourced (sourced⊥performed) → P2.6.1(complete): StateHost threads one cell, the counter
  remembers 0→1→2→3 → A→B: Value::Effect{name,payload} (self-describing, no inversion),
  Type::Effect; map fans out to N sends, write+send in one round — the one-effect ceiling lifted
  → drive: SourcedResolver injects any sourced value (store_read was a driver all along), round
  threads storage AND drivers — the Elm runtime closes (Model/update/Cmd/Sub) on a pure core
  → P2.6.3: cell identity dissolved by host-storage (value-not-hashed unrepresentable,
  non-replayable already held, fork=CoW tested, pin→P3) → P2.6.2: transactional read-then-write
  dissolved — the round IS the transaction (atomic snapshot→fire→apply; conditional RMW composes
  via select; saturating counter 0→1→2→2→2); concurrent-txn → platform host, not core
  → P2.5.1: correlation token = boundary label (explicit data, no Value change); non-crossing is
  purity's theorem (map(respond,·) keeps tokens paired); real routing/await defer
  → P2.5.2: idempotency key = data; determinism (P1.4.4) makes a re-fire rebuild the same key →
  dedup-able (DedupeHost performs once); user derives key from intent (must be replayable)
  → P2.7.2: Mode{Design,Deploy}+run; fire returns the settled output; same graph previews in
  design (nothing performed) / fires in deploy — **P2 COMPLETE** (P1 ⊳ P2 closed; P3 next)
  → §13: genesis primitive set + admission gate pinned (trust root; reconcile before first publish)
  → P3.8.1: content-addressed store — Store≠Resolver (owned units, eval never touches disk),
  Mem+File backends, byte format IS the preimage (+decode_preimage inverse), verify-on-read =
  re-hash the decoded unit against its key (I5 operational, no blake3 in host) — **P3 begins**
  → genesis reconciliation (arc, L4): R1a — cons/head/tail/is_empty added (additive, no hash
  moved); head/tail of [] ⇒ None (get-OOB convention); two-table rule (compute + capabilities_of)
  → R1b — fold (the one iteration primitive, in eval.rs beside map; fn=param, init+list=inputs,
  left-fold fn(acc,elem); fold([])=init). Control flow now provably select+fold, no while ⇒
  total by construction. **R1 (pure irreducibles) complete**
  → R2 — text irreducibles (Text opaque): lowercase/text_length/slice/index_of, char-based,
  casing locale-free (I2), index_of -1 sentinel (not-found is total, not None). Additive
  → R3 — random driver (one capabilities_of row; no compute/simulate change — sourced_mock is
  type-generic). **Additive run R1–R3 complete; no existing hash moved.** Next: breaking renames
  → R4a — timer→clock, request→ingress (first breaking inch; hashes move). Survey-first caught
  `request`-primitive vs `request`-the-word (Fork B); scoped rename, word preserved
  → R4b — email+respond→ONE egress sink (merge; host dispatches on capability not name, so clean).
  Deduped collapsed match arms by hand; 3 explanatory comments preserved; caught email_hash via
  substring grep. **R4 complete: all effects carry §13 names (clock/ingress/egress).** Next: R5
  → R5 — number/boolean/text/true/false → one `const` (reads value+type params; declares type,
  never infers; true/false were never real primitives — removed). 8 files (constants minted
  everywhere). **RECONCILIATION ARC R1–R5 COMPLETE — native set matches §13 on every axis.**
  → R6 — DECISION (no code): freeze the as-built trust root (Branch B), defer demotion-to-stdlib
  to its own pillar (revisit when stdlib mechanism + auditor exist). Gate stays active for future.
  Frozen 34-leaf genesis manifest pinned in §13a. **GENESIS RECONCILIATION COMPLETE (R1–R6).**
  Next: back to P3 local-first (registry → inspection → memo → fork → advisory), now unblocked`,
  all pure core. **`P2.6.1` (state) was deferred to the deploy crossing** rather than built
  before firing: the substituting-resolver insight made design-mode firing pure and
  buildable without state, so the purity seam is crossed once, at deploy, not twice.
- **Phase D · Local marketplace** *(`host`)* — `P3.8.1 → P3.1.1 → P3.2.1 → P3.3.1 →
  P3.5.1 → P3.6.1`. (`P3.8.1` needs only P1.6 — done — so it can begin early in parallel.)
- **Phase E · Editor** *(TS/React over WASM)* — `P4.2.1 → P4.1.1 → P4.1.2 → P4.3.1 →
  P4.4.1 → P4.4.2 → P4.5.1 → P4.6.1`. Gated on Phase A (the WASM core must exist).
- **Deferred (fork-open, no near-term trigger):** async + quiescence `P2.4.2` — genuinely
  still open, unnecessary while eager, waits on autonomous cells. Generics `P1.3.2` and
  networked/social `P3.3.2`, `P3.4.1`, `P3.8.2` are now built — this line had gone stale, not
  reflecting completed work (`P3.7.1` stays dropped, a decision, not a gap).

Phases A–B are the spine and are pure core — they can proceed with no host, no editor,
no network. The plan is **thorough enough at the frontier (Phase A) to begin building**
while deeper phases are detailed just-in-time.

---

## Level-4 readiness — which leaves must descend before building

A leaf needs L4 only if it is **large** *or* hides an **unresolved design sub-decision**
(not mere implementation). Most leaves — the analysis-over-structure ones — are buildable
directly from L3. The exceptions, each flagged with the sub-decision L4 must resolve:

- **P1.4.3 · Evaluator** *(frontier — descend next)* — per-input firing policies
  (required/optional · consume/persist/default, §6), the eager fixed-point strategy,
  promoted-input injection, composite recursion. **The load-bearing leaf of the plan.**
- **P1.5.2 · Param promotion** — touches the canonical encoder (the most hash-sensitive
  code): the `Frozen | Open` encoding + binding-table representation, domain-separated.
- **P2.3.1 · Capability model** — the capability *set* must be enumerated (a design
  decision, as the primitive set was).
- **P2.4.1 · Effect-firing host loop** — the host runtime architecture (event source →
  eval trigger → sink dispatch) is genuinely new structure.
- **P2.5.1 · Correlation token** — whether/how every value carries routing identity is a
  cross-cutting change to the value envelope.
- **P3.5.1 · Copy-on-write re-pin** — the ancestor re-derivation algorithm (bump child →
  re-derive every parent up the closure).
- **P4.2.1 · WASM bindings** — the API surface + TS↔Rust marshalling of the `Unit` model.
- **P4.1.1 · Authoring canvas** *(L)* — large; mostly a porting plan from the prototypes,
  not novel design.

**Buildable directly at L3 (no L4):** P1.4.1, P1.3.1, P1.4.2, P1.4.4, P1.2.1, P1.5.1,
P2.1.1, P2.2.1, P2.3.2, P2.6.1/2/3, P2.7.1/2, P3.8.1, P3.1.1, P3.2.1, P3.3.1, P3.6.1,
P4.1.2, P4.3.1, P4.4.1/2, P4.5.1, P4.6.1.

**Just-in-time rule.** We do *not* descend all eight now. The frontier is Phase A:
`P1.4.1 → P1.3.1 → P1.4.2` are buildable immediately; **`P1.4.3` is the first leaf that
needs L4**, so it is the only descent required before building begins. The other seven
descend as the build reaches them.

---

## Descent control

How we deepen this plan, and in what order:

1. **P1 → Level 3 first** (it is the entailed base; P2/P3/P4 all presuppose it).
   Within P1, the open frontier is P1.3 (types), P1.4 (evaluator), P1.5
   (encapsulation + param promotion) — with their own internal order to be derived.
2. Then **P2 → Level 3**, then **P3 → Level 3**. **P4** descends in parallel once
   P1.4 exists, since the Editor embeds the core it evaluates with.
3. Each Level-3 leaf is specified as: *precondition leaves* · *the invariant it
   serves* · *the test that proves it* · *a rough effort note*. A leaf is "ready"
   only when those four are filled.
4. **Trunk-review checkpoints** gate each descent: before expanding a pillar we
   confirm its Level-2 set is complete and correctly entailed.

### Resolution log
| Branch | Detailed to | Frozen? |
|---|---|---|
| Root (L0) | L0 | ✅ frozen |
| Pillars (L1) | L1 | ✅ frozen |
| P1 / P2 / P3 / P4 (L2) | L2 | ✅ frozen |
| P1 leaves (L3) | L3 | ✅ frozen |
| P2 leaves (L3) | L3 | ✅ frozen |
| P3 leaves (L3) | L3 | ✅ frozen |
| P4 leaves (L3) | L3 | ✅ frozen |
| P1.4.3 · evaluator | L4 | design note (`P1.4.3-evaluator-L4.md`) |
| P1.5.1 · encapsulation | L4 | design note (`P1.5.1-encapsulation-L4.md`) |
| P1.5.2 · param promotion | L4 | design note (`P1.5.2-param-promotion-L4.md`) |

*Level 3 ratified and frozen across all four pillars. Master build order derived.*
*Selected leaves descend to L4 just-in-time (see "Level-4 readiness"). First descent: P1.4.3.*

*(Open trunk-review question: is anything in the Level-0 end-state **not** covered
by exactly one pillar? If yes, the trunk is incomplete and must change before descent.)*

