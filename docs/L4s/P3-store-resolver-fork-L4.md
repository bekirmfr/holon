# L4 — The `Store`→`Resolver` ownership fork (SET ASIDE, with trigger)

**Status:** examined, deliberately *not* built. This note records the analysis so the decision
is a ten-minute call when the trigger arrives, not a re-investigation. It also **corrects** an
earlier mis-framing in PLAN/JOURNAL: this was repeatedly noted as an "adapter refactor … appeared
4×, earned." That was wrong on three counts (below). It is not an adapter, it did not appear 4×,
and it is a *performance optimization with a precondition*, not structural debt accruing interest.

## The impedance mismatch (why a literal adapter can't exist)

- `Resolver::resolve(&self, &Hash) -> Option<&Unit>` hands back a **borrow** tied to `&self`.
- `Store::get(&self, &Hash) -> Result<Option<Unit>, StoreError>` produces an **owned** unit —
  decoded fresh each call (verify-on-read re-hashes the bytes; the store holds *bytes*, not live
  `Unit`s; even `MemStore` decodes per `get`).

A store-backed `Resolver` would have to lend a `&Unit` borrowing something `&self`-lived, but the
store has only bytes to lend. Caching decoded units behind a `RefCell` doesn't rescue it: a
`RefCell` guard can't yield a `&Unit` that outlives the guard. So under the present trait the
*only* way to turn a store into a `Resolver` is to eagerly load the whole closure into an owned
`MapResolver` first — which **is** fork's `materialize`. An "adapter" would rename the glue, not
dissolve it. To get a genuinely store-backed resolver, the **trait return type must change**.

## The correction to "appeared 4×, earned"

Checked against the code, the store→resolver *materialize* glue exists in exactly **one** place:
fork's `materialize`. The others were miscounted:
- `verify_closure` (publish) walks the *store* checking presence — never builds a resolver.
- `dependencies` (inspect) and `advise` are pure core over a `Resolver` — they never touch the
  store; no host wrapper exists for either.

So the duplication is hypothetical (rule of *one*, not three). The cheap `materialize` helper
should be extracted the moment a **second** real caller lands (a host `advise(hash)`/`inspect(hash)`),
not before.

## The signature options (if the trait ever changes)

| return type            | in-memory (`MapResolver`)          | store path        | verdict |
|------------------------|------------------------------------|-------------------|---------|
| `Option<Unit>`         | **clones every lookup** (hot eval) | trivial (owned)   | ✗ taxes the common case |
| `Option<Rc<Unit>>`     | needs MapResolver to store `Rc`    | cheap clone       | ✗ changes repr; `Rc`/thread-affinity in pure core for no extra gain |
| `Option<Cow<'_,Unit>>` | `Cow::Borrowed` — **zero-copy**    | `Cow::Owned`      | ✓ only option that doesn't tax in-memory |

If it changes at all, it changes to **`Cow`** (`Unit: Clone` already holds).

## Blast radius (grounded, bounded)

- **2 `Resolver` impls** change (`MapResolver`, the mock in `simulate.rs` that delegates to
  `real.resolve`); a host `StoreResolver` would be the 3rd.
- **~14 `.resolve()` call sites.** ~6 are already `…unwrap().clone()` (5 in `promote.rs` *tests*,
  1 in `determinism.rs`) → get *simpler* (`.into_owned()`). ~7 (`encapsulate`, `kind`, `promote`,
  `validate`, `eval`, `fork`) use the unit locally → mechanical `&*u`/`.as_ref()` or a local
  collection's element type flips.
- **1 genuine return-type change:** `dependencies`/`collect` in `inspect.rs` store resolver-borrows
  in their returned `BTreeMap<Hash, &'r Unit>`; a store resolver's units are transient, so it must
  become `BTreeMap<Hash, Unit>` (owned). This **drops** the `'r` param (simpler), and the only
  caller is `advise`, which does `.len()` (identical on an owned map). Fully contained to
  `inspect.rs` + tests.

Mechanical, ~a day, nothing hard once `Cow` is chosen.

## What it actually buys

- Whole-closure functions (`capabilities`, `dependencies`, `advise`) gain **nothing** — they
  traverse every unit regardless of eager vs lazy.
- The single real win is **`eval`**, which is *not* whole-closure: `select` evaluates only the
  taken branch, `fold` only its combiner. A lazy store-backed resolver lets `eval` decode **only
  the units on the path actually taken**, where eager `materialize` loads the entire closure. For
  a large, branchy program run straight from the store, that is a genuine saving.

This makes the change a **performance optimization for lazy `eval`-against-store**, not a
correctness requirement: eager `materialize` is always *correct*, just potentially wasteful.

## TRIGGER CONDITION (set aside until all hold)

Reach for the `Cow` trait change only when **all three** are true:
1. A **deploy/operate-on-published path exists** that runs `eval` directly against the `Store`
   (today `eval` runs against in-memory `MapResolver`s; the `Store` is publish/registry only — so
   this benefit is currently *unrealizable*).
2. The programs are **large and branchy** enough that `select`/`fold` pruning skips a meaningful
   fraction of the closure.
3. Eager full-closure `materialize` is **measurably** the bottleneck (profiled, not assumed).

## Decision rule when the deploy tier arrives

Look at the **first concrete need**:
- whole-closure op on a published hash (advise/inspect) → the **cheap eager `materialize` helper**
  (`Cow` buys it nothing);
- evaluating a large branchy published program where pruning matters → the **`Cow` trait change**,
  migrated once, paid for by a benefit you can measure.

Standalone today it fails the same test the "adapter" failed: real cost, no realized benefit. The
difference is we now know *exactly* the cost (bounded, mechanical; `inspect`'s return type the only
real edit) and *exactly* the justification, so it's a quick call later, not an open question.
