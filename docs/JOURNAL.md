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

---

## 2026-06-29 · P1.4.2 — Primitive registry + native compute · *(Phase A, inch 4)*

**Built.** `holon-core::primitives::compute(name, inputs, params) -> Option<Value>` —
the pure first-order vocabulary, dispatched by name:
- constants `number` `boolean` `text` (value from a param);
- arithmetic `add` `multiply` (variadic), `subtract` `divide` (binary; divide guards ÷0);
- `compare` (op param `> < >= <= = !=`);
- logic `and` `or` (variadic) `not`;
- `select` (control-as-data); text `concat` `uppercase`;
- list `list` `get` `append` `length` `sum` `join`.

**Leaf closed.** **P1.4.2.** The evaluator (next) now has native leaves to dispatch to.

**Tests prove** (16): each primitive matches expectation (`add[3,7,5]=15`,
`compare(72,70,">")=true`, `sum([3,7,5])=15`, `join(["a","b","c"],"-")="a-b-c"`, …);
unknown name → `None`; runtime type mismatch through an `any` port → `None`;
determinism.

**Forks pinned.**
- **`None` = not computable** (unknown name, wrong arity, or runtime type mismatch). The
  evaluator will treat this as `None`-propagation; wire-time checking (P1.3.1) already
  rules out everything except `any`-port mismatches, which `compute` guards at runtime.
- **First-order only.** `compute` takes `&[Value]` and ignores any eval context — it has
  none yet. `Map` (the 21st primitive) and the `ctx` it needs are gated behind P1.5.1, so
  the signature stays first-order until then.
- **Variadic vs binary is per-primitive** (`add`/`multiply`/`and`/`or`/`concat`/`list`
  fold over all inputs; `subtract`/`divide`/`compare`/`get`/`append`/`join` are
  fixed-arity). Arity is enforced by slice patterns; the firing rule (required/optional)
  is the evaluator's job, not `compute`'s.

**Correction banked.** The plan calls the pure set "22 primitives" (and `Map` "the
23rd"); the enumerated set is in fact **20** (`Map` is the 21st). Off-by-two in the
roadmap text, not in the build — noted here, the set is otherwise exactly as listed.

**Next inch.** **P1.4.3 — the evaluator** (`P1.4.3-evaluator-L4.md`): resolve a
composite's DAG in topological order, inject promoted inputs via `BOUNDARY→child` edges,
dispatch primitives to `compute`, read the exposed output via the `child→BOUNDARY` edge,
recurse for nested composites. `None` propagates.

---

## 2026-06-29 · P1.4.3 — Eager graph evaluator · *(Phase A, inch 5 — the load-bearing leaf)*

**Built.** `holon-core::eval(unit, inputs, resolver) -> Option<Value>`. A primitive
dispatches to `primitives::compute`; a composite resolves its inner DAG **by reference**,
in a **single topological pass** (acyclicity from P1.3.1 ⇒ one pass suffices), injecting
promoted inputs via `BOUNDARY→child` edges and reading the exposed output via the
`child→BOUNDARY` edge. Primitive and composite share the **identical** entry point (I1).
Each composite call gets a fresh scratch (nested / repeated composites never collide).
`None` propagates.

**Leaf closed.** **P1.4.3** — and **P1.2.1** (interface parity) discharged by test. The
"identity but not evaluation" gap is **closed**: `holon-core` now computes, not just hashes.
Phase A is effectively complete (only the determinism harness P1.4.4 remains, and the
evaluator already uses ordered structures throughout).

**Tests prove** (7): `eval_primitive_add` (`add[3,7,5]=15`); `eval_composite_add`;
`eval_composite_sum` (the v3 seed minus `Map`: `list(a,b,c)→sum→15`); `eval_nested_composite`
(recursion); `none_propagates` (binary `subtract`, one operand absent ⇒ `None`);
`interface_parity_primitive_eq_composite` (a primitive and an equivalent composite are
substitutable); `deterministic`.

**Forks pinned.**
- **Firing rule = compute's arity check, for now.** The evaluator gathers *present* inputs
  in port order and lets `compute` decide. For the current vocabulary this realizes
  `required/optional` exactly (a fixed-arity op with a gap ⇒ wrong count ⇒ `None`; a
  variadic op folds whatever is present), and skipping an absent port never yields a
  *wrong* value — only `None` or the correct one. The explicit per-port
  `required/optional/default` field (an encoder change) is **deferred** until a primitive
  needs a positional optional with a default. `persist`/`consume` remain out of the pure
  core (state boundary / async, P2.4.2).
- **Eager = one topological pass** (`topo_order`, Kahn's, ties broken by ascending child
  index for determinism). No iteration; the prototype's repeated passes were a shortcut.
- **`Map`/`ctx` still deferred** to P1.5.1: `compute` is first-order; the evaluator will
  be the provider of the `apply(hash, inputs)` context when `Map` lands.

**Next inch.** **P1.4.4 — determinism harness** (ordered structures already in use; add
the repeated-/cross-run identity tests and a fuzz check), then Phase B (**P1.5.1
encapsulation**, where `Map` and `ctx` finally land).

---

## 2026-06-29 · P1.4.4 — Determinism harness · *(Phase A, inch 6 — closes Phase A)*

**Built.** `holon-core::is_finite_number` (the float-discipline predicate) + the
`determinism` harness. The `number` constant now **rejects non-finite values** (NaN,
±inf) — the one place they could enter from user data — so the value domain stays finite
and equality / future canonical hashing are total.

**Leaf closed.** **P1.4.4** — and with it **Phase A** (Evaluation) is complete:
`P1.4.1 → P1.3.1 → P1.4.2 → P1.4.3 → P1.4.4`, plus P1.2.1 discharged. `holon-core` holds
values, validates graphs, computes, and is provably deterministic.

**Tests prove** (3, total now ~52): `golden_nested_arithmetic` (`(2+3)*(10-4)=30`, a
pinned multi-level answer); `deterministic_over_random_graphs` (200 random nested
arithmetic trees, each evaluated **twice** for determinism *and* against a direct Rust
computation for correctness — kept in the exact-integer range so eval must match
bit-for-bit); `non_finite_constants_rejected`.

**Forks pinned.**
- **Determinism is structural, not bolted on.** The evaluator already uses only ordered
  structures (`BTreeMap`/`BTreeSet`, index-keyed scratch); the harness *proves* it rather
  than enforcing it after the fact.
- **Float discipline = finite-only domain.** NaN/±inf rejected at the constant boundary;
  division already guards ÷0. This keeps `Value` equality total — the property the shared
  cache (P3.3) inherits.
- **Cross-target identity deferred to P4.2.1.** Native-vs-WASM bit-identity is the other
  half of I5; it needs the WASM bindings to exist. Single-target determinism is
  established here; the harness is ready to run against WASM later.
- **No external dep for fuzzing.** A tiny inline LCG generates the random graphs — the
  pure core keeps its only dependency (`blake3`).

**Phase A complete. Next: Phase B — P1.5.1 (encapsulation: derive-then-freeze).** This is
where a selection of wired units becomes a new hashed `Unit` with an auto-derived
interface (unbound inputs promote, the chosen output is exposed, kind = union of
children) — and, right after, where **`Map`** and the evaluation **`ctx`** finally land.

---

## 2026-06-29 · Addendum — P1.2.1 parity properly discharged

**Correction.** Inch 5 claimed P1.2.1 "discharged," but its test only showed *standalone*
equality (`eval(prim) == eval(comp)` for one input) — that demonstrates the idea, not the
substitutability I1 actually asserts.

**Strengthened.** Renamed the old test `parity_standalone_primitive_eq_composite` and added
`substitution_in_host_is_invisible`: one host computes `(x + y) * z` through a swappable
"add-like" child; evaluated once with a **primitive** `add` and once with a
**composite-wrapping-add** as that child. The two children hash **differently** (asserted),
so it is a genuine substitution — and the host's output is identical. That is the I1 claim,
now proven, not asserted. (eval tests: 7 → 8.)

**Why parity is robust (banked).** It is structural: primitive and composite are the same
`Unit` type with the same interface fields and one `eval` entry point; a faithful 1:1
wrapper reconstructs the exact input vector (Nones preserved positionally) and delegates,
so identity holds by construction. The primitive path's `flatten` vs the composite path's
positional gather cannot diverge, because the flatten always happens at the primitive leaf
— the same place, both ways. The only future case that could matter (a positional optional
input with a default) is exactly the deferred feature, and faithful-wrapper parity survives
it.