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

---

## 2026-06-29 · P1.5.1 — Encapsulation (derive-then-freeze) · *(Phase B, inch 1)*

**Built.** `holon-core::encapsulate` — `derive_and_freeze(children, inner_edges, spec,
resolver) -> Result<Unit, EncapError>` plus `default_spec(...)`. Turns a self-contained
selection into a new hashed composite: **derives** interface types from inner ports,
**generates** the `BOUNDARY` edges, and honors the spec's **choices** (which inputs
promote, their order, names, the exposed output). `default_spec` gives the canonical
proposal (promote all unbound in `(child, port)` order; expose the unique free output).

**Leaf closed.** **P1.5.1.** Encapsulation is the inverse of evaluation — it *produces*
the boundary edges P1.4.3/P1.3.1 *consume* — so the two are now provably in lockstep.
The system is recursive in practice: a flow can become a referenceable unit.

**Tests prove** (8, total now ~61): `encapsulates_single_unit_to_expected_hash` (wrapping
a bare `add` yields **exactly** the hand-built add2 — equal hashes, edge order normalized);
`encapsulated_evaluates_correctly` (`(a+b)*q` selection evaluates to 20 — behavior
preserved); `default_spec_promotes_unbound_canonically`; `order_is_identity` (same ports,
different order ⇒ **different hashes**); `rejects_promoting_bound_port`;
`rejects_invalid_output`; `ambiguous_default_output`; `derived_unit_validates`.

**Forks pinned.**
- **Derive vs choose.** Types + boundary edges derived; promotion subset/order/names +
  exposed output chosen (in `EncapSpec`). Order is identity — the `order_is_identity` test
  makes that concrete.
- **"Freeze" is construct-and-hash**, not a flag. Re-deriving from edited children yields a
  different hash by I5; nothing can drift.
- **Defensive validation.** `derive_and_freeze` validates its own output, so encapsulation
  can never emit an invalid unit (the test confirms it always passes).
- **Empty params / no kind yet.** Param promotion is P1.5.2; kind = union of children is
  P2.2.1. P1.5.1 derives the *type* interface only.

**Next inch.** **`Map`** (the 21st primitive) + the eval **`ctx`**: `compute` gains
`ctx.apply(&Hash, &[Value]) -> Option<Value>` (the evaluator provides it); `Map` takes its
function as a param hash and applies it per list element. Then **P1.5.2** (param promotion).

---

## 2026-06-29 · `map` (higher-order) · *(Phase B, inch 2)*

**Built.** `map` handled **in `eval`** (not `compute`): input 0 is a list, the `fn` param
holds the hex hash of the unit to apply; the evaluator resolves `fn` and applies it per
element through its own recursion. Added `eval_map` + `parse_hash` (hex→`Hash`). `compute`
and the 20 first-order primitives are unchanged.

**Leaf closed.** `map` — the 21st primitive. The higher-order story is complete: the v3
seed `list(3,7,5) → Map(Double) → [6,14,10]` now runs end-to-end.

**Tests prove** (2, total ~63): `map_doubles_list` (the v3 seed); `map_none_propagates`
(`Double` on a `Text` element fails ⇒ the whole map is `None`).

**Deviation from the L4 (banked).** The L4 planned a `ctx` threaded into `compute` so a
first-order `compute` could call back into `eval`. Building showed that is backwards:
`map` *invokes* the evaluator, so it belongs **in `eval`**, where recursion already lives —
which keeps `compute` purely first-order. No `ctx` trait, no signature change, no churn to
the first-order tests. Clean boundary: **`compute` = first-order leaves; `eval` = graph +
higher-order.**

**Fork surfaced (for P3.2.1).** `map`'s `fn` is referenced by a **param-hash** (hex),
outside the `children` channel. Transitive **pinning** holds — the param is in the preimage,
so `map`'s hash depends on `fn`'s hash (I6 ✓). Transitive **inspection** (P3.2.1) must
additionally treat hash-valued params as references:
`referenced_hashes(unit) = children ∪ {hash-valued params}`. Recorded; needed at P3.2.1.

**Deferred.** Higher-order **type checking** (does `fn`'s signature fit the element type?)
is not done — `fn` is applied dynamically, `None` on mismatch; and `Type::List` carries no
element type. Both acceptable for now.

**Next inch.** **P1.5.2 — param promotion** (`Param = Frozen | Open` + a binding table;
touches the encoder), which closes Phase B.

---

## 2026-06-29 · P1.5.2a — `Param = Frozen | Open` + encoder tag · *(Phase B, inch 3a)*

**Built.** `Param` is now an enum — `Frozen { name, value } | Open { name }` — with
constructors (`frozen`/`open`) and accessors (`name`/`value`/`is_open`). The canonical
encoder tags the variant in the preimage (`PARAM_FROZEN = 0x00`, `PARAM_OPEN = 0x01`), so a
frozen and an open param of the same name hash differently. All readers now use `value()`
(which is `None` for a hole) and all constructors use `Param::frozen`.

**Scope.** Sub-leaf **(a)** of P1.5.2 — the representation + encoder change, **in isolation
and behavior-preserving**. Every param is still `Frozen`, so the system computes exactly as
before. `open_param` / `open_params` / `instantiate` (sub-leaves b, c) follow next.

**Encoder discipline.** The first encoder change since the foundations. The variant tag is
domain-separated and length-prefixed like everything else. Param-bearing unit hashes change
(the preimage gained a tag byte) — pre-release, so no migration; `ENCODING_VERSION` stays 1.

**Tests prove.** No new tests this sub-inch — the proof of safety is that **all 63 existing
tests still pass under the re-encoding**. They are all *relative* (determinism, `=`, `≠`, the
encapsulation hash-equality test, the `map` hash round-trip), so a uniform encoding change
leaves them intact. That is exactly why the encoder change was isolated first.

**Next.** **P1.5.2 b/c** — `open_param` (frozen→open, make a template), `open_params` (the
derived open interface), `instantiate` (open→frozen, specialize bottom-up and re-hash). Then
Phase B closes.

---

## 2026-06-29 · P1.5.2 b/c — open / instantiate · *(Phase B, inch 3 b/c — closes Phase B)*

**Built.** `holon-core::promote` —
- `open_param(unit, name, bind)`: turn a frozen param into a hole (creates a template);
- `open_params(unit, resolver)`: the **derived** open interface — the set of `bind` keys
  across the closure (union over children), never stored (I8);
- `instantiate(template, bindings, store)`: specialize — substitute `bind→value` throughout
  the closure, re-hash bottom-up, insert new concrete units, return the top hash. Partial
  binding leaves the rest open.

**Representation fixed (found while building).** Sub-leaf (a) shipped `Open { name }`, but
`instantiate` revealed a hole needs **two** names: the *role* `name` (`"value"`, what
`compute` looks up once filled) and a separate *bind* key (`"K"`, what `instantiate` fills
by) — otherwise two holes sharing a role name couldn't be set independently. Revised to
`Open { name, bind }` (encoder + accessor `bind()`). Nothing had constructed `Open` yet, so
the fix was clean and the 63 stayed green.

**Leaf closed → Phase B complete.** `P1.5.1 → Map → P1.5.2`. The language pillar's
composition story is whole: encapsulate flows into units, apply units higher-order (`map`),
and parameterize units into families.

**Tests prove** (5, total now ~68): `open_then_instantiate_is_identity` (open `number(5)`
as `K`, bind `K=5` ⇒ **hash equals** the original `number(5)` — open and bind are true
inverses); `family_by_hash` (`K=2` vs `K=3` ⇒ different units, eval to 2 vs 3);
`open_params_is_derived` (`MultiplyByK`'s open interface is `{"K"}`, derived);
`template_not_evaluable_until_instantiated` (template ⇒ `None`; `MultiplyBy3(7)=21` after);
`partial_instantiation_leaves_rest_open`.

**Forks pinned.**
- **Instantiate-time (B) confirmed.** Instances are first-class content-addressed units;
  eval and the cache treat them like any other unit — no param channel in either.
- **Always-recurse.** `instantiate` rebuilds every child; an unaffected subtree rebuilds to
  the **same hash** (content-addressing), so it's correct, just unoptimized. Skipping
  subtrees with no bound open param is a deferred optimization.
- **`instantiate` is coupled to `MapResolver`** (it needs `insert`). A `MutableStore` trait
  would generalize it to the on-disk store (P3.8.1); deferred.

**Next: Phase C — the effect boundary (P2).** Kinds, the state cell, derived kind +
capabilities, eager effect-firing, design/deploy modes. The pure core is done; P2 builds the
OS around it (in `holon-host`, never in `holon-core`).

---

## 2026-06-29 · Addendum — "template not evaluable" is consumer-dependent

**Correction.** The P1.5.2 b/c entry claimed a template evaluates to `None`. A test caught
that this is only true at the **primitive** level. A number with an open hole *does* eval to
`None` (the hole makes `compute` fail). But a **composite** template whose hole feeds a
**variadic** consumer (`multiply`/`add`) does **not** hard-fail — the missing operand is
simply absent, so `multiply([7]) = 7`, not `None`. Variadic firing tolerates the gap.

**Fixed.** Split the over-strong test into `primitive_template_is_not_evaluable` (a hole ⇒
`None`, then instantiates and computes) and `instantiate_composite_specializes`
(`MultiplyBy3(7)=21`, no false `None` claim). promote tests: 5 → 6; total ~69.

**Pinned.** Templates are **"evaluate only after instantiation" by contract**, not by a
hard eval-time guard. A universal guard (eval rejects any unit with open params in its
closure) would cost a closure walk per eval and is **deferred**; the cheap primitive-level
truth (an open hole ⇒ `compute` ⇒ `None`) already holds for the actual vocabulary, since
every current primitive reads the param it would open.

---

## 2026-06-29 · P2.1.1 — Kind taxonomy (Phase C opens)

**Built.** `kind.rs`: `Kind = Pure | Driver | State | Sink` + `kind_of(name) -> Option<Kind>`,
the leaf classifier that is the **trust basis** of the effect layer. New module wired into
`lib.rs` (mod + doc + re-export). 75 tests (69 → +6).

**Shape (settled in the kind-sourcing sketch).** A *closed core table*: all 23 pure leaves
(the `compute` vocabulary + higher-order `map`) → `Pure`; three effectful exemplars
(`store → State`, `timer → Driver`, `email → Sink`) classification-only, no `compute` —
core *declares* their kind without being able to *run* them (execution is host-side).

**Why a closed table, not an open registry.** The marketplace ships only *composites*
(kind derives, no declaration → no trust surface); new *leaves* (effects) arrive only as
trusted host releases ("syscalls"). So the leaf set is closed+trusted and core can own the
classification outright. An open registry would buy extensibility we don't need and smuggle
back an asserted-label surface I8 forbids.

**Fail-closed.** Unknown leaf → `None` (never `Some(Pure)`); callers treat `None` ≥ `Sink`.
`unknown_is_none` pins the one direction that would be a laundering hole.

**Pinned for later.** Two boundary invariants live in the module header, untestable until
host execution exists (P2.4.1 / P2.6.1): (1) *disjointness* — a leaf is pure-in-core xor
effect-in-host; (2) *no effect without classification* — every host-fired name classifies
non-`Pure`. Tested the in-core half now (`effectful_exemplars_are_not_pure` = the security
direction); the cross-crate containment lands as a real test with the host, not as an empty
`#[ignore]`. **Follow-on noted:** `validate` rejecting unknown leaf names (the leaf-analogue
of its existing unresolved-child rejection) — closes the hole at *import*; separate inch.

**Kind is a derived view, never a field** — nothing stored or hashed, so mislabel is
inexpressible, not merely forbidden (I8).