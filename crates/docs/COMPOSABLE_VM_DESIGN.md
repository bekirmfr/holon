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
