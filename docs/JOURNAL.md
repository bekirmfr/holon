# Holon — Implementation Journal

> Companion to `PLAN.md` and `COMPOSABLE_VM_DESIGN.md`. **One entry per inch**, each
> recording *what was built · which leaf it closed · what the tests proved · any fork
> pinned in the doing.* The journal is the retroactive audit trail that makes
> "understand design intent before changing it" possible.

---

## 2026-06-29 · P1.4.1 — Value model · *(Phase A, inch 1)*

**Built.** `holon-core::Value` — the runtime datum that flows on wires:
`Number(f64) | Boolean(bool) | Text(String) | List(Vec<Value>)`. Immutable, value-
semantics (`Clone`, `PartialEq`, `Debug`). `Value::ty(&self) -> Type` projects a value
to its concrete type. Added the `value` module to `holon-core` and re-exported `Value`.

**Leaf closed.** **P1.4.1.** The first step toward closing the "identity but not
evaluation" gap: before today the core could *hash structure* but could not *hold a
value*. Now it can.

**Tests prove** (4, all passing locally):
- `ty_agrees_with_type` — each variant maps to the matching `Type`.
- `no_value_is_any` — every value has a concrete type; `Any` is wire-time only, never a
  runtime value.
- `value_semantics_clone_is_independent` — a clone is independent of its source.
- `equality_is_structural` — deep structural equality incl. nested lists; cross-type
  values differ.

**Forks pinned in the doing.**
- **`Number` is one type, `f64`.** No int/float split — matches §5's single `number`.
  Integer-looking results (`15`) are `f64` (`15.0`).
- **`Value` derives `PartialEq` but not `Eq`/`Hash`.** `f64` has no total equality (NaN).
  Canonical *value* hashing — needed by the memoization cache (P3.3.1) and input-hashing —
  will be a deliberate canonical encoder, **not** `derive(Hash)`. NaN / float-
  canonicalization discipline is deferred to **P1.4.4** (determinism harness); the value
  model stays representation-only for now.
- **`ty()` is total over concrete types** — it can never return `Type::Any`, keeping the
  value/type relationship honest: a runtime value always has a definite type.

**Next inch.** **P1.3.1** — graph validation: well-formedness (edge indices in range,
no dangling ports), **acyclicity** (the evaluator presupposes a DAG), and wire-time **type
compatibility** (`dst=any ∨ src=any ∨ src=dst`).
