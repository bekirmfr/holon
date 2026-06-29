# L4 · Genesis reconciliation — sequencing the native set to §13

**Goal.** Bring the *native primitive set* (the leaves of the hash DAG — the trust root)
into line with design `§13`, **before** the registry's first publish freezes hashes
permanently. This is a **breaking arc, not one inch**: most of it re-hashes existing units.
It must be done one boundary at a time, each step independently green.

## The change, grounded (current native set → §13 target)

Three kinds of change, by risk:

- **ADD (additive — no existing hash moves, safe to do anytime):** `cons`, `head`, `tail`,
  `is_empty`, `fold` (pure irreducibles the set is missing); `text_length`, `slice`,
  `index_of`, `lowercase` (text irreducibles, `Text` stays opaque); `random` (driver).
- **RENAME / MERGE (breaking, but localized):** `timer → clock`; `request → ingress`;
  `email` + `respond` → one `egress`; the five constants `number`/`boolean`/`text`/`true`/
  `false` → one `const`. These change the hashes of the affected units only.
- **DEMOTE (breaking + heavy — needs a standard-library mechanism):** remove `list`, `get`,
  `append`, `length`, `sum`, `join`, `map` from native; re-express them as **composites over
  `fold`/`cons`**. This is the one part that cannot land until there are authored stdlib
  units to replace the demoted primitives.

Every additive/rename step must also update **two** core tables, never one: `compute`
(primitives.rs) **and** the pure/effect allowlist in `capabilities_of` (kind.rs). Renamed
effects also touch `sink_effect` names, `sourced_mock` (simulate.rs), and the capability
table; `fold` additionally touches `eval.rs` (higher-order machinery, like `map` today).

## The fork (decide at R6, not before)

Both options give a **stable trust root for the registry** — they differ only on whether we
pay the demotion cost now:

- **(A) Full §13.** Do the demotions: build the stdlib composites, remove the derivables from
  native. The trust root becomes the *minimal* §13 set. Cost: a stdlib mechanism + re-hashing
  every unit that used a demoted op. Maximal fidelity.
- **(B) Trust-root freeze + gate.** Add the irreducibles, clean the names, merge the
  constants — but **keep** the convenience derivables (`sum`/`map`/`length`/…) native. The
  set is stable and slightly larger than §13's ideal; the admission **gate** (no *new*
  derivable primitives) still prevents future bloat. `§13` itself called the demotions "the
  one breaking move… optional; the gate is the prize, not the one-time shrink," so (B) honors
  §13's spirit while declining the shrink. Lower cost, no stdlib needed yet.

Recommendation: defer this call until R1–R5 are in. They are required by **both** options, so
nothing is blocked by leaving A/B open.

## Ordered inches (each independently green)

- **R1 — pure irreducibles (additive).**
  - **R1a · `cons`/`head`/`tail`/`is_empty`** — O(1) list build/inspect. `head`/`tail` of `[]`
    ⇒ `None` (the existing `get`-out-of-bounds convention; partial-via-`None`, totality intact
    — it halts, it just yields no value). `is_empty` is total. *primitives.rs + kind.rs.*
  - **R1b · `fold`** — the one iteration primitive. Higher-order (reads a `fn`-hash param,
    threads an accumulator over a list), so it lands in `eval.rs` beside `map`. The universal
    list donor; every demoted op (R6) is a `fold` composite.
- **R2 — text irreducibles (additive).** `text_length`, `slice`, `index_of`, `lowercase`.
  `Text` stays opaque. Casing uses **locale-independent Unicode default mapping** (an `I2`
  determinism call — locale-tailored casing would poison every downstream hash).
- **R3 — `random` driver (additive).** Sourced like `clock`; capability `Random` already
  exists in `kind.rs`. Adds the driver name + `sourced_mock` arm.
- **R4 — effect-name reconciliation (breaking, localized).** `timer→clock`, `request→ingress`,
  `email`+`respond`→`egress`. Real destination routing stays behind the `Host` seam (deferred);
  the payload already carries what egress needs.
- **R5 — constant merge (breaking, localized).** `number`/`boolean`/`text`/`true`/`false` →
  one `const` reading a value + a type tag from params. Touches every constant unit-builder
  and its tests.
- **R6 — demotions (breaking, heavy) — the A/B decision point.** Either build the stdlib
  composites and remove the derivables from native (A), or stop and apply gate-only (B).

R1–R5 reconcile the trust root (add every irreducible, clean every name); R6 is the optional
minimization. The registry (`P3.1.1`) can follow once the trust root is stable — i.e. after
R1–R5, and after R6 only if option A is chosen.

## First inch: R1a

Smallest, safest, needed under both A and B: add `cons`/`head`/`tail`/`is_empty` to `compute`
and to the pure allowlist in `capabilities_of`. Pure, additive, no existing hash moves.
