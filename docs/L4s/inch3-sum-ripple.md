# Inch 3 (`Sum`/`Option`) — ripple note

Eight files this inch: `value.rs`, `memo.rs`, `hash.rs`, `validate.rs`, `catalog.rs`, `primitives.rs`,
`kind.rs` (core), plus `crates/holon-wasm/src/lib.rs` (the bridge, both halves — type and value).

## The one compile fix you'll need (unseen file, sibling to inch 2's)

`simulate.rs`'s exhaustive `Type` match needs a second arm, alongside the `Type::Product(_) =>
return None,` you already added:

```rust
Type::Sum(_) => return None,
```

Same reasoning: no sourced leaf (driver / `store_read`) outputs a `Sum`, so this is unreachable
today; `None` is the fail-closed, honest answer.

## Three real design decisions made while grounding (all narrow, all documented in code)

1. **`Value::ty()` for `Sum` is an honest coarsening, not a sound supremum — and that's new.**
   `List`'s `Any`-wildcard coarsening (`list_any()`) is always sound because `List` is
   unary-recursive. A `Product` value (riding `Value::List`) always carries every field, so its
   arity is recoverable — but a `Sum` value carries only its *taken* arm; the untaken alternatives
   are erased by definition. `Type::Sum(vec![Any, Any])` is honest **today** only because
   `some`/`none` (Option) are the only constructors that can ever produce a `Value::Sum` — arity 2
   is the only arity that exists. Documented in `value.rs` with the explicit trigger: a generic
   n-ary sum injector would need this rethought. Worth knowing: `Value::ty()` has zero callers
   outside its own tests — latent precision, not a live bug.

2. **`some`/`none`, not a generic `inject(tag, value)`.** `Option` is the L4's standing example;
   a fully general injector would need declared arity threaded through `compute`, which today only
   sees `name`/`inputs`/`params` — no type info at all. Scoping to `Option` sidesteps that: `some`
   is always arm 0, `none` is always arm 1, hard-coded by which primitive you called. Same
   "type language general, constructors earned-per-use" split that gave `Product` only `pair`.

3. **The wasm bridge needed a real decision, not a copy of inch 2's pattern — `WValue` and
   `WType` are different kinds of bridge.** `WValue` turned out to already be a **lossless**
   structural mirror of `Value` (adjacently-tagged via serde — `Effect{sink,payload}` was already
   shaped exactly like a tagged union), so `WValue::Sum{arm, payload}` slots in with zero
   coarsening, the same shape as `Effect`. But `WType` is genuinely flat, and mapping
   `Type::Sum(_) → WType::List` (the move inch 2 made for `Product`) would have been a **lie**, not
   a coarsening — a `Sum` value is `WValue::Sum`, not list-shaped, so the editor would expect a
   list and receive a tagged union. `WType` gained a new bare-tag variant, `WType::Sum` — same
   shape as `List`/`Effect` (no payload, arm types deferred to the recursive-`WType` editor inch),
   and `into_core` reconstructs it the same way `List` does: the one concrete shape that exists
   (`Sum(vec![Any, Any])`), not a guess at some other arity.

## What's proven

- `sum_arm_is_part_of_identity` (value.rs) / `sum_arm_changes_the_key` (memo.rs): the same payload
  at a different arm is a different value and a different cache key — without this, `some`/`none`
  confusion would be silently possible.
- `sum_type_round_trips` (hash.rs): `Sum([Number, Unit])` survives `encode∘decode`, including the
  nested `Product` (`Unit`) inside the `Sum`.
- `compatible_is_structural` extended: a `Sum` is never compatible with a `Product` of identical
  shape — a tagged choice and a tuple are different things, not just different tags.
- `some_and_none_declare_an_option` (catalog.rs) / `some_and_none_tag_their_arm` (primitives.rs):
  both arms declare the same type; `compute` tags correctly; wrong arity ⇒ `None`.
- `check_wvalue_roundtrip` extended (wasm bridge): `Sum` round-trips through `WValue` losslessly,
  both arms (`some` and `none`'s shapes).

## Counts

Genesis: 35 → **37** (`some`, `none`). Both wasm catalog-count assertions (`check_primitive_catalog`,
`primitive_catalog_crosses_js_boundary`) bumped to 37 in this same `lib.rs` — already applied, not a
separate fix this time.

## Gate

`cargo test --workspace` green (after the `simulate.rs` one-liner), then `wasm-pack test --node
crates/holon-wasm`. Expect every prior test unchanged, plus the new ones above.
