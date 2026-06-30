# Inch 2 (`Product` + `pair`) — ripple note

Six files carry the inch: `hash.rs` (the `Product` type + tag 7 + recursive encode/decode +
`ENCODING_VERSION=3`), `validate.rs` (the `compatible` arity-arm), `catalog.rs` (`pair` in `GENESIS` +
`interface_of`, count → 35), `primitives.rs` (`pair`'s compute), `kind.rs` (`pair` as a pure leaf),
`crates/holon-wasm/src/lib.rs` (the bridge flattens `Product → WType::List`).

## The one compile fix (unseen file)

Adding a `Type` variant makes any **exhaustive match on `Type`** non-exhaustive. There is exactly one
besides the files above:

- **`holon-core/src/simulate.rs`** — the mock-type mapping (around the `Type::List(_) => …` arm). Add:
  ```rust
  Type::Product(_) => return None,
  ```
  No sourced leaf (driver / `store_read`) outputs a product, so this arm is unreachable in practice;
  returning `None` is the honest, fail-closed choice — the same as the `Type::Effect` arm ("can't mock
  this"). The compiler will point at the exact match.

(That's the only one — `value.rs` matches on `Value`, not `Type`, and `Value` gains no variant: a pair
*value* rides `Value::List`.)

## Count assertions that move

- `catalog::tests::genesis_has_35_leaves` is updated here (was 34). If any **other** test asserts the
  genesis count — a wasm `primitive_catalog` test or a host test checking `GENESIS.len()` / catalog
  length — bump it to **35**. The compiler/test run will name it.

## `ENCODING_VERSION` 2 → 3

Every hash moves again (the version byte). Nothing persisted, no manifest to re-pin; the native↔wasm
`hash_parity` / `project` tests compare values *to each other*, not to fixed hex, so they stay green.

## Gate

- `cargo test --workspace` green: all prior tests unchanged, plus the new ones —
  `product_type_round_trips` (hash), `compatible_is_structural` extended (validate),
  `pair_declares_a_product` (catalog), `pair_builds_a_pair` (primitives).
- `wasm-pack test --node crates/holon-wasm` green (the bridge flattens `Product`; no JS-visible change
  beyond the catalog now carrying `pair` as a `List`-typed node).

## What this inch does and doesn't do

`Product` is now a real, encodable, checkable, **promotable** type, and `pair` is the constructor that
produces one — so `Dictionary<K,V>` is expressible as `List<Pair<K,V>>` with no new value kind (the
pair value rides `Value::List`). Core-precise. The **editor** still sees `pair` coarsely (the flat
bridge shows its output as plain "List"), so an editor-placed `pair` behaves like `list` until the
recursive-`WType` editor inch — the same core-precise / editor-coarse staging as `sum`/`join`, and the
reason the editor work is sequenced last.
