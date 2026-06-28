# Holon — Implementation Journal

> Companion to `PLAN.md` and `COMPOSABLE_VM_DESIGN.md`. **One entry per inch**, each
> recording *what was built · which leaf it closed · what the tests proved · any fork
> pinned or surfaced in the doing.* The journal is the retroactive audit trail that
> makes "understand design intent before changing it" possible.

---

## 2026-06-29 · P1.4.1 — Value model · *(Phase A, inch 1)*

**Built.** `holon-core::Value` — the runtime datum that flows on wires:
`Number(f64) | Boolean(bool) | Text(String) | List(Vec<Value>)`. Immutable, value-
semantics (`Clone`, `PartialEq`, `Debug`). `Value::ty(&self) -> Type` projects a value
to its concrete type. Added the `value` module and re-exported `Value`.

**Leaf closed.** **P1.4.1.** First step out of "identity but not evaluation": the core
could hash structure but could not hold a value. Now it can.

**Tests prove** (4): `ty_agrees_with_type`; `no_value_is_any` (`Any` is wire-time only);
`value_semantics_clone_is_independent`; `equality_is_structural` (incl. nested lists).

**Forks pinned.**
- `Number` is one type, `f64` — no int/float split (§5).
- `Value` derives `PartialEq` but not `Eq`/`Hash` (`f64`/NaN). Canonical *value* hashing
  (for P3.3.1) will be a deliberate encoder, not `derive(Hash)`; float discipline → P1.4.4.
- `ty()` is total over concrete types — never `Any`.

---

## 2026-06-29 · P1.3.1 — Graph validation · *(Phase A, inch 2)*

**Built.** Two modules in `holon-core`:
- `resolve` — the `Resolver` trait (`hash → &Unit`) + `MapResolver` (in-memory
  `BTreeMap`, populated as units are built; the on-disk store, P3.8.1, will be another
  `Resolver` behind the same trait).
- `validate` — `validate(unit, resolver) -> Result<(), Vec<ValidationError>>`: three
  checks over a composite's inner child→child wiring — **well-formedness** (edge child /
  port indices exist), **acyclicity** (Kahn's algorithm), **type compatibility**
  (`compatible(src, dst) = dst==Any || src==Any || src==dst`, §5). Errors are
  **collected**, not first-and-stop, so the editor can surface all at once.

**Leaf closed.** **P1.3.1** *(inner-wiring scope — see finding)*. Validation is the gate
upstream of evaluation: the evaluator may now assume a clean, acyclic, type-sound DAG.

**Tests prove** (8): `compatibility_rule`; `primitive_always_valid`;
`valid_composite_passes`; `out_of_range_child_rejected`; `cycle_rejected`;
`type_mismatch_rejected`; `output_port_out_of_range_rejected`; `unresolved_child_rejected`;
`collects_multiple_errors` (a cycle + a type mismatch reported together).

**Forks pinned.**
- **`Resolver` moved one leaf earlier.** It was scoped to the evaluator (P1.4.3), but
  type-checking an edge needs the child's interface, so resolving-by-hash is required at
  validation. Introduced now; the evaluator reuses it unchanged.
- **Errors collect-all** (`Vec<ValidationError>`), not first-error — chosen for the
  editor's benefit.

**Fork SURFACED (not yet pinned) — the next decision.** The `Unit` model has **no
representation of the interface↔inner-graph binding**: which composite inputs feed which
inner ports, and which inner output is exposed. The v3 prototype had this (`inPorts`,
`outNode`); the Rust model does not. Consequences:
- Validation currently covers only inner child→child wiring; the promotion binding can't
  be validated until it exists.
- **The evaluator (P1.4.3) needs this binding** to inject promoted inputs and read the
  exposed output — so it must land **before** P1.4.3, and it is **structural** (affects
  the hash + the canonical encoder).
- Two candidate representations to decide: **(a)** a sentinel "boundary" node so external
  inputs/outputs are ordinary edges, or **(b)** explicit binding fields on `Composite`
  (`input_bindings`, `output_binding`). Decision deferred to the next inch.

**Next inch.** Pin the interface↔inner binding representation (a vs b), then it feeds both
the encoder extension and P1.4.3.

---

## 2026-06-29 · Interface↔inner binding — boundary-as-node · *(Phase A, inch 3)*

**Resolves the fork surfaced in inch 2.** Chose **(a) boundary-as-node**, realized as a
**reserved edge endpoint** rather than new struct fields.

**Built.**
- `holon-core::BOUNDARY` (`= u32::MAX`): an edge `from_child == BOUNDARY` carries a
  composite *input* inward (`from_port` = input index); an edge `to_child == BOUNDARY`
  exposes an inner output as the composite *output* (`to_port` = output index).
- Boundary-aware `validate`: a unified per-edge type check where each endpoint is either
  a child or the boundary, plus two new range checks (`BoundaryInputOutOfRange`,
  `BoundaryOutputOutOfRange`) and a new well-formedness rule
  (`DuplicateInputEdge` — at most one source per destination port).

**Why this realization.** The binding is captured entirely by ordinary, **already-hashed**
edges — so **no encoder change** and **no new structural fields**. One wiring mechanism,
as (a) intended; even an identity composite (input wired straight to output) validates
for free.

**Tests prove** (6 new, 15 in `validate`): `boundary_wiring_valid`;
`boundary_input_type_mismatch`; `boundary_input_out_of_range`;
`boundary_output_out_of_range`; `duplicate_input_edge_rejected`;
`boundary_does_not_create_false_cycle` (a normal `BOUNDARY→…→BOUNDARY` flow is *not* a
cycle).

**Forks pinned.**
- **Boundary = source for inputs, sink for outputs.** This inversion is why it can never
  form a cycle; `has_cycle` already ignores boundary edges (an endpoint `== BOUNDARY` is
  not `< n`), so no special case was needed.
- **One source per input port.** Fan-in to a single port is rejected; variadic units use
  multiple ports, not multi-wire-one-port. The evaluator may now assume each port has ≤1
  source.
- **Single exposed output** falls out of the duplicate rule (two edges to `(BOUNDARY, 0)`
  is a duplicate). Multi-output stays deferred.

**Next inch.** **P1.4.3 — the evaluator** (design pinned in `P1.4.3-evaluator-L4.md`).
The binding it presupposed now exists: inject promoted inputs via `BOUNDARY→child` edges,
read the exposed output via the `child→BOUNDARY` edge.