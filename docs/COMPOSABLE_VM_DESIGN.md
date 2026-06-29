# Composable Compute VM — Vision & Architecture

*A visual programming substrate where users compose logic from small primitive
units, package any flow as a new unit, and share units in an open marketplace.*

> **Working codename:** *Patch Bench* (placeholder — rename freely).
> **Status:** core model validated in three interactive prototypes (v1–v3).
> **This document is the north star.** Everything we build is checked against it.
> Every invariant below carries its rationale, because the rule that matters
> most is: *understand why a decision was made before changing it.*

---

## 1. The end state

We are not building a node editor. We are bootstrapping a **programming
language whose syntax is visual**, and a **package ecosystem** for it.

Three independent decisions — *purity*, *content-addressed pinning*, and
*composition by reference* — compound into something larger than their sum:

- **A recursively composable fabric with no ceiling and no floor.** A primitive
  and a composite are indistinguishable. `Sum` is a unit; a unit that *is an
  entire application* is also a unit, evaluated the same way. There is no
  privileged "language level." A novice wiring two blocks and an expert shipping
  a service are playing the identical game, because the blocks never know how
  deep the contraption goes.

- **A trustless, network-effected marketplace.** Because units are pure and
  content-addressed, a result is keyable by `(unit-hash, input-hash)` — a
  *universal* key. The marketplace doesn't just distribute code; it distributes
  **memoized computation** that anyone can reuse and verify by hash alone.
  Popular units are units whose results are already mostly computed. The network
  effect is *structural*, not engineered.

- **A pure language core wrapped in a capability-gated OS.** The cheap interior
  is a pure functional language: analyzable, cacheable, replayable. The
  expensive edges — time in, state, irreversible effects — are the syscalls.
  The moment a flow can be triggered by an endpoint, hold state, and emit an
  effect, a "unit" becomes a **deployable service**, and the marketplace becomes
  an app store for software strangers composed — made safe because the language
  stays pure and every consequence sits behind a visible, inspectable boundary.

This is the *Minecraft of compute* formula at its most literal: **one mechanic,
applied recursively, with no level where the rules change.** Depth comes from a
small orthogonal primitive set composed deeply — not from a large library.

Everything good above is a **theorem of the invariants in §3**, not a feature
that can be bolted on later. The discipline of building *inch by inch, one
boundary at a time* exists precisely because these seams must stay clean. Blur
the pure/effect line or the immutability line and you don't get this system —
you get a generic automation tool with extra steps.

*How this synthesis sits against existing systems — Unison, Nix, Bazel, FRP — and
what in it is genuinely new versus reused, is mapped honestly in §12.*

---

## 2. The core model

A **unit** is a pure typed function with one contract:

```
evaluate(inputs, params) → outputs
```

- **inputs** — values that arrive on wires, at compose time.
- **params** — configuration set when the unit is placed (not flowing on wires).
- **outputs** — values emitted on wires.

A **primitive** unit evaluates natively (`Add` sums its inputs). A **composite**
unit is a flow — a DAG of other units — and evaluates by resolving that inner
graph. *From the outside, the two are identical.* That identity is the spine of
the entire system (Invariant I1).

When a flow becomes a unit, its interface is **derived once** from the inner
graph and then **frozen** as part of that unit's version:

- unbound inner **inputs** promote to the new unit's inputs;
- unfixed inner **params** promote to the new unit's params;
- unconsumed inner **outputs** promote to outputs (with an explicit "also expose"
  for a value used internally *and* wanted as a tap point).

---

## 3. The invariants (load-bearing — do not break casually)

Each is followed by *why*, and *what breaks if violated*. Treat any change here
as a change to the foundation.

1. **Interface parity.** A composite is indistinguishable from a primitive at its
   interface. *Why:* it's what makes recursion free and unbounded. *Breaks:* the
   moment composites present a different shape, composition needs special-case
   code two levels deep — concealment, not a fix.

2. **Purity of the core.** Interior units are pure: no side effects, no memory
   between firings. *Why:* purity is what *earns* determinism, caching, replay,
   fork-safety, and harmless duplicate dependencies. *Breaks:* every one of those
   theorems silently becomes false; correctness rots one hack at a time.

3. **Composition by reference, not flattening.** A composite stores its children
   as pinned references and runs them as black boxes; it does not inline them.
   *Why:* flattening destroys versioning, updates, and the economic identity of a
   unit. *Breaks:* there is nothing to version, sell, attribute, or fix.

4. **Derive then freeze.** Interface auto-derives from the wiring *once*, at
   encapsulation, and becomes immutable metadata of the version. *Why:* a contract
   that recomputes live from internals shifts under everyone who depends on it.
   *Breaks:* nobody can safely pin, buy, or build on the unit; semver is a lie.

5. **Immutability + content addressing.** A published unit is frozen at a version;
   the pin is to a **content hash**, with the version label as a friendly alias.
   *Why:* identical hash ⟹ identical execution, *provably*. *Breaks:* "stable" is
   a promise instead of a guarantee; reproducibility evaporates.

6. **Transitive pinning (lockfile closure).** A published unit stores the resolved
   hashes of its *entire* dependency closure, not just direct children, and only
   exact pins (no ranges). *Why:* stability leaks if any level floats. *Breaks:*
   a floating reference deep in the tree is a silent supply-chain injection point.

7. **Effects at the edges, purity in the middle.** Time enters at the top
   (drivers), consequences exit at the bottom (sinks), state is explicit and
   fenced. *Why:* it concentrates every guarantee you can't live without into the
   few places it's both necessary and definable. *Breaks:* effects smeared through
   the interior reintroduce ordering, retries, and double-sends everywhere.

8. **Derived, not asserted, labels.** A composite's kind (pure / driver / state /
   sink) is the *union* of its children's, computed — never hand-claimed. *Why:* a
   unit must not be able to launder an effect into a "pure" label. *Breaks:* the
   whole trust and analyzability story collapses to "trust the author."

9. **Full transitive inspectability.** "See everything" means descend the *entire*
   closure to the leaf primitives, by the same pins that guarantee execution.
   *Why:* trust is *earnable* only if a buyer can read all the way down. *Breaks:*
   inspection that stops one level deep lets a mislabeled effect hide below it —
   transparency becomes theater.

---

## 4. The two layers

### 4a. Functional core — *the language*
Pure units, an acyclic dataflow graph, a small type system, control-flow-as-data
(`Select` is an ordinary pure unit; branching needs no engine feature). This
layer is where all the good properties live. It is cheap, analyzable, and the
place users spend most of their time.

### 4b. Boundary — *the operating system*
The edges are not "more vocabulary"; they are syscalls. Three kinds, each
categorically different from the pure interior even though they share the
`evaluate` silhouette:

- **Drivers** (top): the outside world invokes the graph — `Timer`, `Endpoint`,
  `Manual Trigger`, incoming requests. They produce *time / events*, not
  computed values; they decide *when*, not *what*.
- **State cells**: memory across firings — `Store`, a database. The one explicit
  place purity ends. A subgraph containing one forfeits caching and replay,
  *visibly*, at that boundary.
- **Sinks** (bottom): irreversible consequences — `Email`, an order, a response,
  a write. These obey different physics: a glitch here can't be taken back.

**The reversibility seam.** Don't police the graph — police the effect. The
interior is free, independent, and may emit transient values as it converges.
Irreversible units carry their *own* discipline (idempotency, transactional
read-then-write, correlation for request/response), because that's the one place
the discipline is both required and definable. Effects are **capability-bearing**
and a composite inherits the union of its children's capabilities, surfaced
*before* import.

---

## 5. Types

Monomorphic base set plus an `any` wildcard: `number`, `boolean`, `string`,
`list`. Wire-time checking is deliberately trivial:

```
compatible(src, dst)  ==  dst === "any" || src === "any" || src === dst
```

`any` is the single escape hatch that lets `Select`, `Get`, `Store`, and `Map`
be genuinely polymorphic without a unification-based type checker. **Generics are
deferred** — they are the single largest jump in checker complexity and are not
needed to prove the model.

---

## 6. Evaluation semantics

- **Firing rule.** A unit fires when its required inputs are satisfied; each input
  contributes per its policy — *required/optional*, and *consume / persist /
  default*. (A `persist` input gives a port memory and therefore forfeits the
  purity theorems for that subgraph — pay it knowingly.)
- **Eager vs async.** Prototypes are **eager and synchronous** (whole graph
  re-evaluates on change) — glitch-free and simplest. The long-term direction is
  **async / autonomous cells**, which *dissolves* eager/lazy rounds but explicitly
  *accepts transient glitches* in the interior. Async is not "no glitches"; it is
  "glitches, tolerated, because the interior converges."
- **Glitch-freedom is a property of the graph, not a unit.** A single independent
  cell cannot be glitch-free; a glitch is a timing relationship *between* inputs.
  Therefore safety lives at the **effect boundary**, not in the engine.
- **Open problem — quiescence.** In async with no rounds, how does an irreversible
  sink know its input has *settled* and isn't a mid-propagation transient? This is
  the central boundary still to design (see §9). The likely answer: free flicker
  in the interior, an explicit settle/commit gate in front of irreversible sinks —
  never a debounce/sleep smeared across the graph.

---

## 7. State

There is **one** explicit state primitive (the state cell / `Store`, of which a
database is the networked form). Statefulness is not the danger; *unmarked*
statefulness is. One clearly-fenced cell keeps "is this subgraph replayable?" a
question answerable by inspection. Idempotency and reconciliation are *not* engine
features — they are things a user composes from the state cell, and the user is
fully responsible. That delegation is only honest because the kit is expressively
complete: the state cell is the universal donor that makes them constructible.

---

## 8. The marketplace & emergent properties

An open, free, fully-inspectable marketplace. Its good properties are
*consequences of the invariants*, not separate features:

- **Universal memoization.** Pure + content-pinned ⟹ results keyable by
  `(closure-hash, input-hash)` and safely shareable across all users, trustlessly,
  because the hash is the proof.
- **Reputation by adoption, not raw usage.** Raw firing-count is trivially faked
  on a free platform. The trust signal is **how many independent units/authors
  compose a unit** — the dependency graph counting itself (PageRank's insight),
  which is expensive to forge. Show both numbers; know which one is trust.
- **Earnable trust via transitive inspection.** A buyer can descend a stranger's
  unit to the leaves before pinning it. Combined with derived labels (I8), an
  effect cannot hide beneath a "pure" claim.
- **Forking is copy-on-write.** Inspect freely; editing a depended-upon unit
  produces a new version/derived unit. With everything pinned, "pull an upstream
  fix" is an explicit, reviewable **re-pin** — there is no silent update channel.
- **Security advisories, not mutation.** A pinned unit can't rot functionally
  while pure, but the first effectful primitive with a bug stays pinned forever.
  The answer is a **yank/advisory channel** that *flags a hash* and points to a
  fixed version — never an in-place edit of frozen bytes.

*(This is the project's own ecosystem, described in its own terms. It borrows the
Minecraft philosophy — simple primitives, emergent complexity, community
ownership — not any specific external platform's mechanics.)*

---

## 9. Known simplifications & open forks

Each is a *boundary to pin later*, deliberately deferred to keep the spine clean.

- **Param promotion** — inner params are currently frozen at grouping; promoting
  them is what turns a one-off (`Double`) into a general unit (`Multiply by N`).
  *This is the next thing that makes units genuinely reusable.*
- **Multi-output composites** — today a composite exposes one output.
- **Generics** — monomorphic + `any` for now; parametric types deferred.
- **Async + quiescence** — the firing model and "when may an effect fire" (§6).
- **Effect contracts** — idempotency keys, transactional read-then-write,
  request/response correlation, exactly-once across failures.
- **Content-hashing & versioning** — the floor everything in §8 stands on (below).
- **Author identity** — the adoption metric is only as forgeable as the cheapest
  fake author; identity model still open.
- **Value payload sharing (`Arc`/`Rc`)** — `Value`'s heap variants (`Text(String)`,
  `List(Vec<Value>)`) are deep-cloned at every wire crossing, so fan-out and depth
  multiply copies. Values are already immutable with value-semantics, so sharing the
  payloads by refcount (`Text(Arc<str>)`, `List(Arc<[Value]>)`) turns those clones
  into pointer bumps — invariant-safe and invisible to the hash (runtime `Value` is
  *not* in the hash preimage at all). **Deferred to P3.3.1**, not done speculatively:
  it is a constant-factor win behind the order-of-magnitude one (memoization), and the
  cache's *input-hash* is the first thing to canonically encode a `Value` — so the
  representation is best settled in one coordinated touch with `Value`-hashing.
  *Open sub-choice:* `Arc` (thread-safe, for parallel cache population) vs `Rc`
  (cheaper, single-threaded) depends on the host's threading model (P2.4 / P3.3).

---

## 10. Build roadmap (inch by inch)

**Already proven in prototypes:**
- **v1** — typed evaluator, live push-dataflow, the four-kind taxonomy as color,
  per-input model, `Manual Trigger` / `Interval` / `Store` / `Display`.
- **v2** — value type system (`number`/`boolean`/`string`/`list`), string & list
  primitives, wire-time type checking with the `any` wildcard.
- **v3** — **encapsulation**: group a selection into a new registry unit with an
  auto-derived interface, evaluate composites *by reference*, nest them, **expand
  to inspect**, and a higher-order **`Map`** that applies a referenced unit per
  element (the proof that flow-as-unit unlocks higher-order composition).

**Next — in dependency order:**
1. **Content-hashing & versioning** *(the critical-path floor — do first).*
   Give every unit a content hash; pin composites by hash; store the transitive
   closure. Invariants I5 & I6, and *everything* in §8, stand on this.
2. **Param promotion** — choose which inner params survive (I4 fully realized);
   unlocks genuinely reusable marketplace units.
3. **`filter` / `reduce`** — the rest of the higher-order family, now that the
   function-as-reference model works.
4. **State & persistence semantics** — define the state cell's behavior under
   replay, fork, and pin; keep the pure/impure boundary inspectable.
5. **Async firing + quiescence gate** — move from eager to autonomous cells;
   design the settle/commit gate in front of irreversible sinks.
6. **Effect-boundary contracts** — idempotency, transactional state, correlation.
7. **Marketplace** — registry, transitive inspection UI, adoption-based reputation,
   copy-on-write forking, advisory/yank channel.
8. **Author identity** — make the adoption metric costly to forge.

---

## 11. How we build (design principles)

- **Align to the natural boundary the system already provides** — the
  reversibility seam, the unit interface, the dependency closure. Don't impose
  control; find the line that's already there and make it *legible*.
- **Understand design intent before changing it.** A choice that looks wrong in
  isolation may be preserving an invariant above. Fix the real defect without
  disturbing the design.
- **No artificial controls over systemic problems.** A sleep, a debounce, a retry
  count, or a status override applied to a systemic problem is concealment. Stop
  and align to the boundary instead.
- **KISS.** A small orthogonal primitive set composed deeply beats a large
  library. Resist features that the invariants would otherwise give for free.
- **Security-first.** Effects are capability-gated; trust is earnable by
  inspection; updates are explicit re-pins; nothing frozen is ever mutated.
- **One boundary at a time.** The end state is only reachable if each seam stays
  clean. Pin one decision, prove it, move to the next.

---

## 12. Prior art & honest positioning

*This section exists because the rule that matters most — understand why a
decision was made before changing it — applies to the novelty claim too. A
commissioned prior-art review (June 2026) asked whether Holon already exists. The
honest answer sharpens the vision rather than dulling it: it tells us which wheels
not to reinvent, and narrows the claim to ground we can actually defend.*

**The bundle is not unprecedented; the intersection is.** The closest whole-system
analogue, by a wide margin, is **Unison** (with Unison Cloud and UCM). It
independently arrived at content-addressed code by hash, effect inference as the
*union of callees*, hash-keyed memoization, and a code marketplace — and reached
production in late 2025. So the "compounding" of §1 is real but not first: Unison
got there in a *textual* language. Holon's open ground is the **intersection**
below, not any single mechanism in it.

**Almost every load-bearing mechanism is established prior art.** This is a gift,
not a threat — each row is a place to adopt a proven design instead of deriving
one:

| Mechanism (Holon) | Closest prior art | Verdict |
|---|---|---|
| Content-addressed units; version = alias to hash | Unison (hash of name-stripped AST), Git, IPFS/IPLD, Nix, Bazel CAS | established |
| Composition by reference (pinned-hash black boxes) | Unison, Nix derivations, FBP composite components | established |
| Transitive pinned closure, exact pins, no ranges | Nix flakes (`flake.lock`), Cargo.lock, Go modules | established |
| Universal memo keyed by (closure-hash, input-hash) | Bazel remote cache, Unison result caches (local), Nix binary caches | mostly prior art; *trustless cross-user* framing partly new |
| Visual dataflow DAG of typed units; control-flow-as-data | Max/Pure Data, TouchDesigner, Houdini, Node-RED, Enso/Luna, FBP | established |
| Recursive encapsulation (composite ≡ primitive) | FBP black-box composites, Houdini HDAs, Unreal Blueprints, Enso | established |
| Higher-order units (`Map` applies a referenced unit) | FP standard; Houdini / Geometry-Nodes for-each | established |
| Derive-then-freeze auto-derived interface | Houdini HDA promoted params, Blueprint collapse | partial — *freezing it as a hashed interface* is the new combination |
| Pure interior, effects only at edges | Functional Core / Imperative Shell, Haskell IO, Roc platforms | established |
| Driver / state / sink as a capability-gated "OS" | Roc platforms, object-capability langs (E, Pony) | partly new — the *tripartite taxonomy on a visual graph* is not a named pattern |
| Kind/capabilities derived as union of children, non-launderable, surfaced before import | **Unison abilities** (required set = union of callees, propagated into callers' signatures); Koka / Eff / Effekt row effects | established — Unison is the match |
| Quiescence before an irreversible sink | FRP glitch-freedom (FrTime, Flapjax, REScala): height/topological propagation | solved — adopt, don't reinvent |
| Yank/advisory channel that flags a hash, never mutates bytes | crates.io yank + RustSec, Go checksum DB, Sigstore/SLSA | established |
| Reputation by dependency-adoption (PageRank over the dep graph) | npm/Julia package-centrality studies | established technique |

**The actual novelty — narrow and defensible.** *Not* content-addressing, effect
inference, memoization, or recursive encapsulation (each is prior art, and Unison
pre-empts the bundle in text). Holon's novelty is the **intersection**: visual
dataflow authoring fused with content-addressed immutable packaging *and* a
trustless cross-user memoization marketplace, with a **derived driver/state/sink
capability boundary on a visual graph**. Two points inside that intersection are
genuinely under-explored: (1) a *shipped* global, trustless, cross-user
*memoized-computation* marketplace (Unison's own materials describe its shared
result cache as a future direction; today it is local/ephemeral); and (2) the
capability boundary as a *derived, non-launderable property of a visual graph*,
with an explicit settle/commit gate for irreversible sinks.

**Where Holon is deliberately stricter or simpler than its closest reference.**
Two design choices already pinned elsewhere in this document turn out to *improve
on* Unison for our use-case, and the comparison is worth recording so they are not
"fixed" later by someone who mistakes them for omissions:

- **No effect discharge at the kind level.** In Unison an ability propagates up
  *until terminated by a handler* — a handler discharges the effect. Holon has
  **no discharge**: a state cell makes outputs history-dependent, a driver makes
  them world-dependent, a sink mutates the world, and that observability does not
  vanish under encapsulation. The only handler-analogue we have — design-mode
  mocks — is *explicitly excluded from trust decisions* (the guardrail in P2.7:
  kind and capabilities are always computed from the real units, never the mocks).
  This is a stricter non-laundering stance than the reference, on purpose.
- **Content-pinned function references sidestep effect polymorphism.** The review
  names "when to infer concrete effects" as Unison's hard open problem — the
  higher-order case where a function's effects depend on a function *argument* not
  known until applied. Holon avoids it: `Map`'s function rides a *content-pinned
  hash* (a frozen param, concrete at authoring), so its effects are always
  derivable now. Following `children ∪ hash-valued params` is therefore sufficient
  — there is no effect-polymorphism uncertainty to defer. This is a direct
  dividend of content-addressing.

**Solved problems to adopt rather than reinvent** (cross-referenced to the
roadmap): FRP height/topological propagation for the settle/commit gate (§6, P2.4)
— *if we find ourselves inserting per-edge debounces or sleeps, stop; that is the
anti-pattern glitch-freedom already eliminated* (§11); Bazel's Remote Execution
API split of action-cache vs CAS for hash-verified result sharing (§8, P3); Nix
`flake.lock` and Go's checksum/transparency-log model for tamper-evident pinning
(I6, P3); Unison's ability-inference rule as the reference for derived kind/caps
(I8, P2.2/P2.3); crates.io-yank + RustSec for the advisory channel (§8, P3);
Salsa/Adapton for the eventual incremental/reactive direction (P2.4.2).

**Strategic watch-item.** The single biggest risk to the distribution thesis: if
Unison Cloud ships a *global, trustless, cross-user* result cache (its stated
future direction), Holon's largest differentiator on that axis erodes. This is a
marketplace-era (P3) concern, not a near-term one — but it is worth monitoring,
especially given Unison's late-2025 production milestone.

**Caveats on this verdict.** "Genuinely new" is a claim about *absence of
evidence*: visual/no-code and dataflow tooling is a large, fast-moving, partly
proprietary space, so the intersection claim is strong but not exhaustively
provable. Unison Cloud's closed beta may already do more than its public docs
state. And much of Holon is still aspirational (holon-host, async cells, the
settle/commit gate, the editor) — these comparisons treat design intent as intent,
and the verdict can shift as those pieces meet (or collide with) the prior art
named above.

---

## 13. The genesis primitive set (the atoms & the admission gate)

The primitives are the **leaves of the hash DAG**. A composite's identity is taken
over its children's hashes, transitively, until the chain terminates at primitive
units whose identity is a *name* (verified in `hash.rs`: a composite encodes each
child *by hash only*; a primitive encodes its name). So the primitive set is the
one place identity stops being "a composition of…" and becomes "a name you trust" —
which makes it the **entire surface of trusted semantics in the system.** Everything
above it is mechanically verifiable by re-hashing; nothing above it must be trusted,
only checked. The atoms are the axioms; every composite is a theorem whose proof is
its hash chain (§2, §8).

Two consequences fix the shape of this set.

**It is genesis-frozen, and the freeze is now-or-never for *removal* only.**
*Adding* a primitive later is append-only-safe — it never re-hashes anything below
it (exactly as `Type::Effect` took tag 6 with no existing hash moved). *Redefining or
removing* one re-hashes the whole closure that depended on it. So the irreducibles
are frozen at genesis; later atoms (a fixpoint, `spawn`) are admitted by deliberate
append, never by editing what exists.

**Admission is decided by a gate, not by taste.** A unit earns *native* status only
if a composite could not give the same thing. Concretely, native iff one of:

- **Speed — complexity class.** The composite form would be in a worse class. (`head`
  from `fold` walks the whole list: O(1) → O(n).)
- **Speed — hot and pervasive.** Same class, but paid in the inner loop of everything,
  finer than memoization's grain (caching a single `a − b` costs more than the
  subtraction). Arithmetic, comparison, logic, `select`.
- **Capability.** Impossible to compose because it is an effect — you cannot build
  `egress` from pure parts. This leg keeps every effect leaf native, which is why the
  primitive list and the permission list are the *same* list.
- **Expressibility.** Genuinely inexpressible from the existing atoms, such that no
  user could reasonably wire it. The test is constructive: *try to build it.* A `for`
  loop falls out cleanly as `fold` (so `for` is **not** a primitive — it is surface
  syntax over `fold`); iteration *itself* does not (the acyclic graph cannot express a
  loop), so `fold` is native. `while` is the case we **refuse** — not because it is
  hard but because admitting it would forfeit totality.

Everything that fails the gate is a **blessed standard-library composite**: pinned,
canonical, and — by interface parity (I1) — indistinguishable from a primitive to
whoever drags the block. Convenience is fully served *one tier out*, in the open
marketplace where it is forkable and improvable, without enlarging the privileged,
unforkable centre. The prize is not the one-time count: it is that the gate is
decidable, so the native set **cannot balloon** over the years while the convenience
surface stays rich and open. The smaller the privileged physics, the more fully Holon
is the permissionless thing it claims to be.

### The set

**Language core — 15** *(non-text)*

- `const` — emit a literal *(subsumes typed constants and `true`/`false`)*. *introduce*
- `add`, `subtract`, `multiply`, `divide` — *speed (hot)*; `divide` carries the zero-guard.
- `compare` — operator by param (`< > <= >= = !=`), **polymorphic over scalar base types**
  (Number, Text, Boolean), so there is no separate text comparison. *speed (hot)*
- `and`, `or`, `not` — *speed/legibility* (`or` is derivable but the minimal form is
  slower and uglier for no gain — the gate keeps it native).
- `select(cond, a, b)` — the only branch. *expressibility (irreducible control flow)*
- `cons`, `head`, `tail`, `is_empty` — list structure; *speed* (O(1); O(n) via `fold`).
- `fold` — the one iteration primitive; *expressibility*. **The language's universal
  donor:** `map`, `filter`, `sum`, `length`, `join`, `append`, `get`, `reverse`,
  `repeat`, and the surface `for` loop are all `fold` composites.

**Text — 6** *(`Text` is an opaque base type)*

Kept opaque, not as a list of char-codes: the composite form regresses UTF-8 handling
to O(n) and launders text through `List` — failing both the speed and expressibility
legs. The native ops are exactly those that must touch the opaque encoding:

- `text_length` — character count *(encoding-aware; nothing to `fold` over an opaque
  value)*.
- `concat` — join two texts *(hot)*.
- `slice(start, len)` — substring by position *(safe indexing into the encoding is the
  engine's job)*.
- `index_of(needle)` — the irreducible search; `contains` / `starts_with` /
  `ends_with` / `split` / `replace` / `trim` / `pad` all compose from it.
- `uppercase`, `lowercase` — **native, using locale-independent Unicode default case
  mapping.** This is a *determinism* (I2) decision, not a convenience one:
  locale-tailored casing (e.g. the Turkish dotted/dotless `i`) makes `uppercase`
  locale-dependent, which would break "identical input ⇒ identical output" and poison
  every hash and cache entry downstream. The atom is therefore a fixed, locale-free
  mapping the engine owns.

The entire rich text surface (`split`, `replace`, `trim`, `contains`, `join`, …) is
stdlib composition over `concat` / `slice` / `index_of` / `compare` / `fold`.

**Effects — 6** *(all native by capability; each atom **is** one permission)*

- *Sourced (read the world):* `clock`, `random`, `ingress`, `store_read`.
- *Performed (act on the world):* `egress` *(subsumes `email`/`respond` — send with a
  destination in the payload)*, `store_write`.
- `store_read` + `store_write` are the **state cell — the OS's universal donor** (§7):
  queue, mutex, database, file, and the P3 trustless cache are all composites over it.

**Deferred — append-only**

- `spawn` (concurrency) — admitted when P-concurrency is actually entered; appending it
  re-hashes nothing.

**Frozen genesis total: 27 atoms** (15 language + 6 text + 6 effect); 28 with `spawn`.

### Two properties this set buys

**Totality by construction.** All control flow is exactly two atoms — `select`
(choice) and `fold` (bounded iteration) — with `while` deliberately absent. No atom in
the base can express non-termination, so *every composite anyone ever builds is
guaranteed to halt.* This is the airtight form of the memo thesis (§8): verification
can never be asked to confirm a result that might not exist, so a pure, total composite
is always safely cacheable and re-hashable. Minimizing iteration into `fold` and the
"everything halts" guarantee are the same fact.

**A neutral minimal centre.** Every atom is privileged, unforkable runtime; every
composite is open, ownable, contestable marketplace content. The gate pushes the entire
expressive surface of the language *out* of the engine and into the economy, leaving the
runtime as close to bare irreducible physics as speed and capability allow. That
boundary — privileged physics vs open content — is the natural one the whole project
keeps rewarding, not an imposed cap.

### What this changes from the as-built set

Gained: `fold`, `cons`/`head`/`tail`/`is_empty` (real irreducibles the current set
lacks). Demoted to stdlib (derivables that were masquerading as primitives): the unary
inverses behind `subtract`/`divide`, the bundled-but-derivable comparison operators, the
list library (`sum`/`length`/`join`/`append`/`get`), and the typed constants +
`true`/`false` (now `const`). Merged: `email`/`respond` → `egress`. The count is about
the same as today's ~22 — but it is the *right* set, every atom now passing a decidable
test instead of sitting there by habit. The demotions are the one breaking move
(re-hashing), hence genesis-timed; the gate protects the boundary, for free, forever
after.