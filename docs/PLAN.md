# Holon — Implementation Plan

> **Companion to `COMPOSABLE_VM_DESIGN.md`** (the north star). The design doc says
> *what is true and why*; this plan is **deduced from it** and says *what we must
> therefore build, in what order*. It is detailed **progressively** — level by
> level across the whole tree, then leaf by leaf — and is not a build target until
> we judge it thorough.
>
> **Current resolution:** Level 2 (pillars decomposed into entailed requirements).
> **Next descent:** Level 3 — expand **P1** to buildable leaves.

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
- **P2.1** Driver / state / sink taxonomy (the reversibility seam) — `I7` · ○ ▣
- **P2.2** Derived, not asserted, labels (kind = union of children) — `I8` · ○ ▣
- **P2.3** Capability declaration + union up the closure, surfaced before import — `I7` · ○ ◇
- **P2.4** Effect-firing model: quiescence / settle-gate before irreversible sinks — `§6` · ○ ◇
  *(the central unsolved boundary)*
- **P2.5** Effect contracts: idempotency, transactional read-then-write, req/resp correlation — `§4` · ○ ◇
- **P2.6** State-cell semantics under replay / fork / pin — `§7` · ○ ◇

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

---

## Cross-cutting findings (banked)

- **Requirements flow downward.** P3.3 (trustless shared cache) is sound *only if*
  P1.4 is bit-deterministic and P1.6 is content-addressed — so two "language design
  choices" are in fact **forced by the marketplace**. The top of the tree dictates
  the floor.
- **The real frontier is narrower than it feels.** Exactly one P1 node is fully
  landed in the production engine (**P1.6**). The evaluator (P1.4) still lives only
  as JS. Deductively, `holon-core` today has **identity but not evaluation** — the
  spine is not done. Porting a deterministic evaluator into Rust is therefore more
  load-bearing than the more exciting param-promotion work, and likely comes first.

---

## Descent control

How we deepen this plan, and in what order:

1. **P1 → Level 3 first** (it is the entailed base; P2/P3 are its theorems).
   Within P1, the open frontier is P1.3 (types), P1.4 (evaluator), P1.5
   (encapsulation + param promotion) — with their own internal order to be derived.
2. Then **P2 → Level 3**, then **P3 → Level 3**.
3. Each Level-3 leaf is specified as: *precondition leaves* · *the invariant it
   serves* · *the test that proves it* · *a rough effort note*. A leaf is "ready"
   only when those four are filled.
4. **Trunk-review checkpoints** gate each descent: before expanding a pillar we
   confirm its Level-2 set is complete and correctly entailed.

### Resolution log
| Branch | Detailed to | Frozen? |
|---|---|---|
| Root (L0) | L0 | pending review |
| Pillars (L1) | L1 | pending review |
| P1 / P2 / P3 (L2) | L2 | pending review |
| P1 leaves (L3) | — | not started |

*(Open trunk-review question: is anything in the Level-0 end-state **not** covered
by exactly one pillar? If yes, the trunk is incomplete and must change before descent.)*

---

## Implementation journal (begins only when the plan is judged thorough)

When we start building, we keep `JOURNAL.md` alongside this plan: dated entries, one
per inch, each recording **what was built · which leaf it closed · what the tests
proved · any fork that got pinned in the doing**. The journal is the retroactive
audit trail that makes "understand design intent before changing it" actually
possible — it is where the *why* of each implementation choice is preserved, so a
future change can tell a real defect from an invariant doing its job.
