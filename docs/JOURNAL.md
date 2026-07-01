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

---

## 2026-06-29 · P2.2.1 — Derived-kind (Effects set over the closure)

**Built.** `effects_of(unit, resolver) -> Effects` in `kind.rs` — the leaf classifier
lifted to whole graphs. 83 tests (75 → +8).

**Combine rule: a set, not a "worst" bucket.** The three effect kinds are independent
*axes* (read / retain / commit), not degrees of one quantity, so collapsing to a single
`Kind` would conceal real axes — a soft cousin of laundering. `Effects(BTreeSet<Kind>)`,
**`Pure` is the empty set** (never a member; `is_pure() == is_empty()`), a **may-touch
upper bound**. `mixed_axes_are_preserved_not_collapsed` proves `timer+email ⇒ {Driver,Sink}`.
This is the shape capability-union (P2.3.2) reuses verbatim.

**Fail-closed, self-contained.** Unknown leaf, unresolvable child, and hash-shaped param that
won't resolve each *saturate* to all axes. Over-approximating danger is sound; the only
forbidden move (under-approximating to `Pure`) is unreachable. The analysis does not assume
`validate` ran first.

**`map` laundering hole closed.** The walk follows `children ∪ hash-valued params`, so a `map`
(itself `Pure`) over an `email`-bearing `fn` reads `{Sink}` — `map_over_effectful_fn_is_effectful`.
Per §12, content-pinning the `fn` ref is exactly what sidesteps Unison's hardest open problem
("when to infer concrete effects"): the reference is always concrete at authoring.

**Refactor at the natural boundary.** `parse_hash` was private to `eval` but the hex⇄`Hash`
mapping is a `hash.rs` concern and now has two production callers (`eval`, `kind`), so it moved
there as `pub(crate)` (+ symmetric `to_hex`) — not reached for across an odd `kind→eval` edge.

**`Kind` gained `Ord`** purely for deterministic `BTreeSet` storage (disclaimed in-code as
*storage order, not a danger priority* — we explicitly do not rank the axes).

---

## 2026-06-29 · P2.3.1 — Capability model (primary classification; kind becomes a projection)

**Built.** `Capability` (10 atomic categories) + `Capability::kind()` projection,
`Capabilities` newtype (mirrors `Effects`), and `capabilities_of(name) -> Option<Capabilities>`
in `kind.rs`. 89 tests (83 → +6).

**The inversion (the decision that mattered).** Capabilities are now the **primary** leaf
classification; `kind_of` and `effects_of` are **projections** of it. Caps are fine, kind is
coarse — project fine→coarse, never the reverse — so by *derivable ⟹ derived* the coarse view
is computed from the fine one and the two cannot drift. `kind_of(name)` = single-bucket
projection of `capabilities_of`; `effects_of`'s leaf branch inserts each capability's `.kind()`.

**Why caps exist (the motivating test).** `fine_distinguishes_what_coarse_loses`: `Clock` and
`SecretAccess` **both** project to `Driver`, yet are distinct capabilities — the gap kind
discards and a consent gate cannot live without ("reads the clock" vs "reads your credentials").

**Staging α was behaviour-preserving.** The leaf branch switched `kind_of → capabilities_of`
projection; for the current vocabulary outputs are identical, so the proof is the **entire
P2.1.1 + P2.2.1 suite (14 tests) staying green** beside the 6 new ones. Source moved, behaviour
did not. Recorded caveat: `kind_of`'s single-`Kind` return is exact only for today's ≤1-axis
leaves; a multi-axis leaf makes leaf-kind a *set* and the convenience yields to `Effects` (§4).

**Set: atomic, fail-closed, ∅⇒pure** — `email→{NetworkEgress}`, `store→{StateRead,StateWrite}`,
`timer→{Clock}`; pure leaves declare the positive empty manifest; unknown ⇒ `None` ⇒ saturate.
Parameterized resources (`NetworkEgress(host)`), Role-1/Role-2 param invocation, and
"disclosure attaches to running not naming" are pinned in the P2.3.1 L4 §9 for later.

**One fix:** a tail-expression `match` over `caps.iter().next()` outlived `caps` (E0597);
bound `next()` to a local (owned `Option<Capability>`, `Copy`) so the iterator drops first.

---

## 2026-06-29 · P2.3.2 — Capability union (single walk; effects_of becomes a projection)

**Built.** `capabilities(unit, resolver) -> Capabilities` — the security manifest, union
over the closure, **computed not stored**, surfaced before import (I9). 95 tests (89 → +6).

**The consolidation (the structural win).** There is now **one** closure walk, not two.
`effects_of` was reduced to `capabilities(...).iter().map(|c| c.kind()).collect()` — the
kind-set is literally the capability-set projected through `Capability::kind()`. The old
kind-walk (`collect`/`saturate`) is deleted. So the primary/projection relationship holds all
the way down: capabilities are the single source of truth, kind is the view, one place the
closure is traversed, one place the fail-closed rule lives. Removed the drift surface rather
than adding a parallel walk (the option chosen over duplicating the fold).

**Behaviour-preserving, proven structurally.** The saturated capability set (all 10) projects
to exactly `{Driver, State, Sink}`, so `effects_of`'s fail-closed output is unchanged; the
P2.2.1 suite stays green beside the new union tests. `effects_is_the_projection_of_capabilities`
pins the relationship as a regression guard.

**Governing test (I9).** `buried_egress_surfaces_before_import`: a `NetworkEgress` three levels
down surfaces at the top — nothing exfiltrates silently. `map_fn_capabilities_surface`: a
capability routed through a `map` fn-param cannot hide either. Fail-closed: an unknown leaf in
the closure saturates to all 10 capabilities.

**Milestone: the classification half of P2 is complete.** kind (P2.1.1) + derived-kind (P2.2.1)
+ capability model (P2.3.1) + capability union (P2.3.2) — all pure core, all derived, all
surfaced, none assertable. Next the frontier crosses the purity seam into `holon-host`:
P2.6.1 (state cell) and P2.4.1 (eager firing loop, needs L4).

---

## 2026-06-29 · P2.4.1 — Design-mode firing (+ minimal P2.7.1); an eval defect fixed on the way in

**Built.** Design-mode firing, and it is **pure `holon-core`** — `mock_of(unit)`,
the `Simulated` substituting resolver, and `design_eval`. 103 tests (95 → +8: 7 in
`simulate`, 1 regression in `eval`).

**The decision that held.** An effect leaf has no pure `compute`, so a graph
containing one is unevaluable. Design mode makes it evaluable by substituting each
effect leaf with a *pure mock* (the minimal P2.7.1) — **at the resolver**, since
`eval` already resolves every child by hash before running it. So firing's design
mode is not a second evaluator and not a host shell: it is ordinary core `eval`
against a `Simulated` resolver. Core stays mode-ignorant; the resolver is the only
thing that differs. α′ (boundary-promotion) is demoted to a deploy-time sink-
observation option — `resolve` returns structure, never IO, so it can swap inputs at
any depth but cannot *fire* a sink; design mode never needs to.

**The structural consequence (the win).** Following the insight all the way kept the
buildable inch **on the pure side of the seam**. `mock_of` + the substituting
resolver + `design_eval` are all pure core; only deploy (real `drive`/`emit`) crosses
into `holon-host`, and it is deferred with real IO. The effect layer's first runnable
slice cost zero impurity.

**Mocks, by kind.** Driver ⇒ a constant of its output type (scripted stimulus;
type-zero default, custom stimuli deferred to P2.7). Sink ⇒ a passthrough that echoes
its settled input (absorb → plausible result). **State ⇒ no pure mock** — its value
depends on prior writes, so it is host-threaded ephemeral memory, which is exactly why
P2.6.1 stays its own leaf. Pure/unknown ⇒ no substitution.

**Guardrail, enforced structurally.** `simulate` depends on `kind`; the classifiers
never depend on `simulate`. So `capabilities()`/`effects_of()` are computed from the
**real** units and never see a mock — `classification_ignores_mocks` pins that a
mocked-away `timer` still surfaces `Clock`, and the mock (a different unit) is never in
any content hash. Simulating innocent buys nothing at the trust boundary.

**A real eval defect, surfaced by the sink mock.** The sink mock is a passthrough
composite — output fed directly by a promoted input (`BOUNDARY→BOUNDARY`). No prior
graph had used that shape, and `eval` had a latent gap: line 82 let a promoted input
feed a *child's* input, but the output resolver let it feed only a *child's* output,
so it indexed the scratch by `BOUNDARY` and panicked. `validate` accepts the shape
(it is the identity passthrough), so `eval` was simply non-total over a valid graph.
Fixed by adding the symmetric arm — the composite output may be fed by a promoted
input too. Not a mock artifact: any composite exposing an input unchanged as its
output would have hit it. `promoted_input_routes_straight_to_output` pins it in
`eval`'s own suite. This is the dividend of building the effect layer on the pure
core — it stresses shapes the arithmetic tests never reached.

**Governing tests.** `effect_leaf_makes_graph_evaluable_in_design` (the unevaluable
becomes evaluable under substitution); `effect_leaf_at_depth_is_substituted` (depth-
agnostic, where α′ could not reach); `sink_mock_echoes_settled_input` (the sink sees
the value `eval`'s single pass settles); `classification_ignores_mocks` (the
guardrail); `state_leaf_has_no_pure_mock` (the exception is explicit).

**Next.** Deploy-mode firing crosses the seam into `holon-host`: the `Host`/`drive`/
`emit` shell, real sink observation, and state threading (P2.6.1) — all with real IO,
all deferred until now. The pure core's effect layer is design-runnable; deploy is the
remaining half.

---

## 2026-06-29 · Fork B — sinks become pure request-constructors (reconciling code to design intent)

**Built.** The sink-model reconciliation, pure core: `email` gains a one-line pure
`compute` (identity on payload), `Capabilities` gains `is_replayable` (no `State` in
the closure), and the sink mock retires. 107 tests (103 → +1 in `primitives`, net +3
in `simulate`).

**The decision (and why it was a *reconciliation*, not a new design).** A fork hid in
the deploy crossing: is a sink an *impure leaf observed in place* (Fork A) or a *pure
request-constructor the host performs* (Fork B)? We chose **Fork B**, and the deciding
frame was the end-state, not local taste: the product is a trustless, content-addressed,
universally-memoized fabric, so **purity is the currency — the pure surface *is* the
product.** Fork A makes impurity contagious (every graph that ultimately *does*
something falls out of the cacheable/shareable surface); Fork B confines impurity to one
point, the host. Grounding confirmed the design doc already committed to B —
*"the state cell, the one explicit place purity ends"*, FCIS/Roc named as the model to
adopt. The **code had drifted**: `email` had no pure `compute` and `is_effectful` was
`!= Pure`, quietly asserting a *second* place purity ends. Fork B reconciles the
implementation back to intent that was already correct.

**The split that carries it.** `is_effectful = (self != Pure)` conflated two questions.
They are now distinct: **effect-bearing** (the capability manifest — does this *describe*
an effect the host must perform? Driver/State/Sink yes) stays exactly as-is; and
**replayable** (`is_replayable` = no `State` capability — is the output deterministic
across firings?) is new, and is the one the design doc actually ties caching to. Mapped
across the four kinds, only `State` breaks replayability — drivers are *inputs*, sinks
purely *construct requests*. The headline test, `sink_graph_is_effect_bearing_and_
replayable`: a `Timer → Email` graph reports `{NetworkEgress}` **and** `is_replayable`
at once — the combination Fork A made structurally inexpressible ("bears an effect" and
"is cacheable" were the same bit). `sink_is_eval_pure` pins the other half: `eval` now
succeeds over a sink-bearing graph under the *real* resolver, no design mode.

**The value-representation answer (solved carefully, not reflexively).** A request is the
pair *(capability, payload)*: the capability is **structural** (derived from the leaf,
already in the manifest), the payload is **ordinary data** (the `Text`/`List` the pure
interior composed). So **no `Value::Request` variant** — the "command-ness" lives in the
structural manifest, never in the `Value` that pure arithmetic shares, and the hash
preimage doesn't move (runtime `Value` was never hashed). A sink is therefore
*identity-on-payload + a structural capability* (option **A**). The richer alternative —
a first-class `Value::Effect` enabling **computed, data-driven effect fan-out** (one
`map` producing `List[Effect]`, the "welcome email per signup" pattern, Elm/Roc `Cmd`) —
is **B**, deferred to the host inch where the case is concrete. Key finding that makes
the deferral free: **A → B touches no content address** — a primitive's `compute` is
keyed by name *outside* the hashed preimage, so switching later re-hashes nobody's
graphs. We pay the `Value→Capability` layering cost only when deploy evidence justifies
the cut, not on spec.

**Retired.** The sink mock and `sink_mock_echoes_settled_input` — they were Fork A
leaking into the P2.4.1 L4 (rev. 2), which treated sinks as impure leaves needing a pure
stand-in. A pure sink needs none. `mock_of` now substitutes **only drivers**; sinks
evaluate directly, state stays host-threaded. The eval fix from that inch was correct on
its own and survives untouched.

**The payoff (the end-state argument, concrete).** The marketplace's keyed surface just
grew from "arithmetic" to "anything whose only impurity is the final send." A
`Timer → compose → Email` graph is now eval-pure, replayable, cacheable, and trustlessly
shareable — its manifest shows the egress *before import* (I9), yet computing *what* to
send is pure and memoizable. Exactly one thing — a `State` cell — pulls a graph out of
that surface, exactly as the doc always said.

**Next.** Deploy firing in `holon-host`, now genuinely thin: source driver values,
`eval` the fully-pure graph, collect *(capability, settled payload)* per sink (a pure
"pending effects" computation — inspectable before running), and *perform* them (the
only impure act). The `(state, effects)` round shape, batching, and the A→B value
upgrade all live there, with real IO.

---

## 2026-06-29 · Deploy-mode firing — the host crossing (holon-host stops being a stub)

**Built.** Deploy-mode firing in `holon-host`: a `Host` trait, `fire`, and a recording
host. The crate now depends on `holon-core`. 110 tests (107 core + **3 host**) — the
first tests outside the pure core.

**The seam is real.** Until now everything was pure `holon-core`. `holon-host` now
depends on it, and that edge — **host → core, never the reverse** — *is* the purity
boundary, enforced by Cargo, not by discipline. `holon-core` still has exactly one
dependency (`blake3`); the day it grows an IO/async crate is the day the invariant
breaks, and the crate graph will say so out loud. There is exactly **one impure
function in the whole system**: `Host::perform`.

**The design choice that shaped it: effects are output-carried, not collected from
inside the evaluator.** The alternative was an observer hook firing as each sink settles
(capture at any depth). Rejected for two non-aesthetic reasons: it would give the one
evaluator a second job and a hook every caller must reason about (`I5`/P4.2 — the
editor's `eval` and the host's must stay bit-identical), and it contradicts the doc's
geometry, *"consequences exit at the bottom."* So `fire` calls the same `eval`
everything uses, then performs the request the **output** carries. `eval` is untouched;
no hook exists. Consequence, made explicit and *correct*: the capability manifest is a
conservative **may** (surfaced at any depth, the trust story before import), and a
request **fires** only when it reaches the output — a buried request wired nowhere is
dead code that correctly does nothing. *Can egress* ≠ *will egress on this wiring*, and
keeping them distinct is right.

**Multiplicity rides the A→B axis, not a loop change.** Minimal (A) = a single terminal
sink: the settled output *is* the payload, the manifest names the egress, the host
performs once. Fan-out (B) = `Value::Effect` / `List[Effect]` at the output, performed
by iterating — taken when the per-signup case is concrete. `fire` never changes; only
the output *shape* does. (Flagged honestly: under A a multi-sink graph would perform one
payload per sink — which is *why* genuine fan-out waits for B rather than being faked.)

**The tests pin the load-bearing claims.** `bare_sink_is_performed_once_with_settled_
payload`: the egress happens **once**, with the value `eval` settled, and **only** at
the host (core performed nothing). `sink_in_a_composite_is_performed`: the same through
a layer of composition — the consequence exits at the bottom and the host picks it up.
`pure_graph_performs_nothing`: no sink in the manifest ⇒ the host is never called; the
pure surface stays inert until something actually egresses.

**Deferred, not hidden.** No driver leaves yet — inputs are handed in directly, standing
in for what `drive` will source from the world. State (the `Model` half of the round's
`(Model, Cmd)`) is host-threaded and its own leaf (`P2.6.1`). Real platform hosts (an
actual mail/file/network `Host`) wait until there is a platform to target.

**Next.** `drive` (source real driver values) and the **state cell** (`P2.6.1`) — the
`(Model, Cmd)` round — then the **A→B** value upgrade when computed fan-out is concrete.
The purity seam now has a crate on each side of it.

---

## 2026-06-29 · State cell, core half — two leaves, the sourced/performed split (P2.6.1)

**Built.** The pure `holon-core` half of the state cell: `store` splits into two leaves,
the classifier gains the input/output axis, and `mock_of` sharpens. 111 core tests
(107 → +1 `primitives`, +1 `kind`, net +2 `simulate`). Host untouched (3); workspace 114.

**The realization driving it (pinned in `P2.6.1-state-cell-L4.md`).** A cell needs almost
no new machinery: a **read** is a driver (host-supplied), a **write** is a Fork-B sink
(its output *is* the new value; the host performs it by *storing*). Two leaves, not one,
which dissolves three problems at once — no graph cycle (the feedback goes through the
host's storage *between rounds*, so `validate` is untouched), no `eval` change (the write
is output-carried), and the `(Model, Cmd)` round falls out (storage is the `Model`;
`StateWrite` is a loop-back effect). The editor (P4) will present one "Store" widget over
the pair; the mechanism stays two clean leaves.

**This inch (core only).** `"store" => [StateRead, StateWrite]` became `"store_read" =>
[StateRead]` + `"store_write" => [StateWrite]`. `store_write` got an identity `compute`
(a Fork-B sink, like `email`); `store_read` has none (sourced). The keystone is
`Capability::is_sourced()` — the **sourced** (input-side: `Clock`, `FileRead`, …,
**`StateRead`**) vs **performed** (output-side: `NetworkEgress`, …, **`StateWrite`**)
split. With it, two predicates sharpen rather than two get invented: `mock_of` mocks the
sourced side (so `StateRead` joins the drivers), and — next inch — `fire` will perform
the non-sourced side (so `StateWrite` joins the sinks). `StateRead ≠ StateWrite` was
always two capabilities; this just lets the two ends be sourced and performed.

**Tests.** `is_sourced_splits_input_from_output` pins that `State` divides across the
line. `store_read_is_mockable` / `store_write_has_no_mock` are the old single
`state_leaf_has_no_pure_mock` *split in two* — the read is supplied, the write is
constructed, which is the model getting more correct. `state_read_graph_design_evals`:
a read-bearing graph runs purely in design mode via the scripted mock, exactly like a
driver. `store_write_is_pure_identity` pins the `compute`.

**Honest scope.** Nothing here threads state. A `store_read` mocks to a type-zero
constant (always `0` in design mode); a `store_write`'s value just reaches the output.
Correct for this inch — it makes the leaves real and classified. Memory across firings is
the **host inch**.

**Next.** The host round: a single-slot `storage` (the `Model`), a read-substituting
resolver (injects the snapshot for `store_read`), `fire` performing `!is_sourced` (so
`StateWrite` stores), and the snapshot→fire→apply loop — with the counter that finally
remembers across firings.

---

## 2026-06-29 · State cell, host round — the cell remembers (P2.6.1 complete)

**Built.** The host half of the state cell: `StateHost` threads one anonymous cell across
firing rounds. 5 host tests (3 → +`state_write_is_performed`, +`counter_remembers_across_
firings`); workspace 116 (111 core + 5 host). **P2.6.1 is complete.**

**The counter remembers.** `store_read → +1 → store_write`, three rounds, storage climbs
`0 → 1 → 2 → 3`. That proves the whole P2.6.1 design — and it cost `eval` **zero** changes,
`validate` **zero** changes, and needed **no graph cycle**. The feedback that looked like a
loop (`cell → … → cell`) is the host's storage *between rounds*; the register that breaks
the combinational cycle is storage, not a `validate` exemption.

**Three pieces, all in `holon-host`.** `fire` flipped from `kind == Sink` to
`!cap.is_sourced()`, so `StateWrite` is performed alongside egress (`state_write_is_
performed` pins it; the three email/pure tests confirm backward-compatibility — egress is
non-sourced). A `StateResolver` injects the round-start snapshot at every `store_read`
leaf (it *owns* the constant, since `resolve` hands out `&Unit`). And `StateHost::round`
is Elm's `update(old) → (new, cmds)`: snapshot → fire → apply the write — reads see the
round-start value, the write lands at round-end and loops back. The borrow knot (resolver
reads storage, the round writes it) dissolves because the snapshot is a clone the resolver
owns, leaving `self` free to be the `&mut Host`, and the apply uses `mem::take`.

**The classification paid off in full.** Two orthogonal axes now decide everything the
host needs: **effect-bearing ⊥ replayable** (Fork B) and **sourced ⊥ performed** (this
arc). `State` sits on the interesting side of every line — effect-bearing, *not*
replayable, and it *straddles* sourced/performed (read sourced, write performed). The
host acts on exactly those bits: source the sourced, perform the performed, store the
`StateWrite`, never cache a `State`-bearing graph. "One fenced place purity ends" is now
a structural fact the code enforces, not a sentence in the doc.

**Honest scope.** One anonymous scalar cell. The injection masks `store_read` to a pure
constant in the resolver `fire` sees (harmless — reads are sourced, never performed;
sinks/`store_write` are never masked, so the performed manifest is intact). Keyed/multiple
cells (`StateWrite(key)`, the parameterized capability resource), non-scalar state, and
`drive` (sourcing real *external* driver values, vs state reading its own slot) remain.

**Next.** `drive` (real driver sourcing from the world), then the **A→B** value upgrade
(`Value::Effect` / `List[Effect]`) when computed fan-out — a `StateWrite` *and* an egress
in one round, or N effects — becomes concrete. `P2.6.2` (transactional) and `P2.6.3`
(fork/pin; the non-replayable half already holds) compose from this donor cell.

---

## 2026-06-29 · A→B — the effect *value*, computed fan-out (representation B complete)

**Built (two slices, 118 green = 111 core + 7 host).** Representation **B**: an effect is now a
self-describing value, `Value::Effect { sink: String, payload: Box<Value> }`, so an output can
carry *many*. The one-effect ceiling — a round could write state **or** send a mail, never both,
and "a welcome email **per** signup" couldn't be said at all — is gone.

**The realization that made it cheap: the value carries the *name*, not the capability.** I had
quoted this as forcing a layering inversion — `value.rs` learning about `Capability`, the sink's
`compute` too. Wrong. The effect value holds the sink's **name** (a `String`); the capability is
*derived* from it (`capabilities_of`), so nothing is lost and **no layer learns anything it
didn't already key on**. `value.rs` needs no `kind` import; `primitives.rs` constructs the effect
with the name it's already matching on; `fire` derives the capability from the name, so the
`Host` trait never moves. The name *is* the structural identity; the capability is a projection
(`I8` again — derived, not asserted). B cost less than A's own framing implied.

**Slice 1 — representation swap, behavior identical.** Two sink lines flipped from identity to
`sink_effect` construction; `Type::Effect` appended (tag 6 — existing hashes unmoved); `fire`
now performs from the **output** (`perform_effects` unwraps the `Effect`) rather than the
manifest. Same 116 tests, same count, same performs — only the internal representation changed.
A clean refactor-shaped inch. Adding a `Value` variant rippled into three exhaustive matches
(`ty`, `sourced_mock`, the host's `constant_unit`) — each got an honest arm: a sourced leaf
*never* outputs an effect (`sourced_mock` → `None`); a state cell *never* holds one
(`constant_unit` → `unreachable!`, an asserted invariant, not a fabricated value).

**Slice 2 — the fan-out walk.** One additive change: `perform_effects` recurses through a
`List`. `map(email, [a,b,c])` ⇒ `List[Effect{email,·}]` ⇒ **three sends** — fan-out is just
`map` doing what it already did. And a single output carrying `[Effect{store_write,v},
Effect{email,msg}]` fires **both** a write and a send in one round, **with no multi-output
composite** — the self-describing `List[Effect]` is what lets two *different* effects exit one
output. That fork stays deferred, dissolved rather than built.

**A bonus invariant from `Type::Effect`.** validate's compat (`src==dst || Any`) absorbs `Effect`
with zero code change, and because `Effect` matches only `Effect`, **a request cannot be wired
into a sink's payload slot** — effects flow down and out, never back in. A typing fact now, not
a convention.

**What stood still:** no `eval` rewrite (an `Effect` flows like any value; `map` already
iterates), no `Host` trait change, no `validate` rewrite, **no existing content address** (`compute`
keyed by name, outside the preimage; `Type::Effect` appended last). Performing is driven by the
output (*does*); the manifest stays the disclosure (*may*) — and "fires what reaches the output"
became literally true rather than approximately so.

**Next.** `drive` — source real *external* driver values from the world (`Clock`, ingress).
It generalizes the read-substituting resolver from "state reads its own slot" to "a driver reads
the world," turning a `round` from a test-loop call into something an event can trigger.

---

## 2026-06-29 · `drive` — source the world (the Sub half; the Elm runtime closes)

**Built (host-only, 120 green = 111 core + 9 host).** Deploy can now source a driver from
outside the pure core — a `timer` (Clock) gets its value injected and the graph reacts. No core
change: classification, `is_sourced`, the driver table, `constant_unit` already existed. `drive`
was *recognition*, not construction.

**The recognition: `store_read` was a driver all along.** `is_sourced()` already grouped
`StateRead` with `Clock | NetworkIngress | …` — they share the one property that matters: the
value enters from outside. So `StateResolver` (which substitutes `StateRead`) and a driver
resolver are **one machine with a wider key**. Generalized it to `SourcedResolver`: a map
`(capability → constant)` that injects at *any* sourced leaf. `fire_sourced` is the single entry
every mode funnels through; `round` now assembles its sourced set as storage-for-`StateRead`
(internal, threaded) plus this round's `drivers` (external, the event's inputs).

**The two boundaries stayed in their two layers.** `StateRead` and a driver differ for real —
internal/threaded/write-paired vs external/sampled/unpaired — but that difference never lived in
the resolver; it lives in the round. The resolver answers one uniform question ("sourced? inject
the constant"); the round assembles values from storage vs event and commits *only* the
`StateWrite` back. Unification removed an accidental narrowness without conflating a real
distinction — reads injected by the resolver, writes committed by the round, no overlap.

**Per-capability keying — the principled default, three ways.** One value per sourced *kind*
(one anonymous clock, like the one anonymous cell): (1) it makes the round a *consistent
snapshot* — every leaf reading `Clock` sees the same instant, so a smeared round is
unrepresentable, not merely discouraged; (2) it stages identically to the state cell, so keyed
drivers lift with the same mechanism keyed cells will; (3) it's what the *atomic* capability
model can express — per-leaf keying needs parameterized capabilities (`Clock(id)`), the same
extension `StateWrite(key)` needs, so choosing it now would drag that work forward out of order.

**Deploy's honest asymmetry.** `an_undriven_driver_yields_nothing`: with no `Clock` supplied, the
driver has no compute, the graph evaluates to `None`, nothing performs — no silent default. Design
mode answers "what *would* happen" (it mocks a type-default for preview); deploy answers "what
*does* happen," and the honest answer to a clock-dependent graph with no time given is nothing,
not zero.

**The Elm runtime is structurally complete, on a pure core:**

| Elm | Holon |
|---|---|
| `Model`  | the host's storage (threaded `StateRead`/`StateWrite`) |
| `update` | the graph (pure `eval`) |
| `Cmd`    | sinks → `Value::Effect`, performed by the host |
| `Sub`    | drivers → sourced values, injected by the host |

And `round` is no longer a test convenience: once a driver value arrives **from an event**, that
event *is* the round trigger — `round` is the event loop's body. The pure interior reacts; only
the host touches a clock or a socket.

**Deferred (unchanged):** *real* world-reading (`SystemTime::now`, an actual socket) is a platform
host — the same seam as real `email`. Keyed/multiple drivers, multi-sourced leaves, and event
scheduling / quiescence (`P2.4.2`) wait on real need.

**Next.** The effect *contracts* that compose from the donor cell: `P2.6.2` transactional
read-then-write, or `P2.5.x` correlation / idempotency. The runtime shape is done; what remains is
making individual effects well-behaved.

---

## 2026-06-29 · P2.6.3 — the cell's identity, dissolved by host-storage (complete)

**One demonstration test, no production code (121 green = 111 core + 10 host).** P2.6.3 —
"a cell's value is runtime state, not identity; cell-bearing ⇒ non-replayable; fork is
copy-on-write; pin" — turns out to be **entirely a corollary of P2.6.1's decision to put storage
in the host, not the unit.** Each sub-part:

- **value-not-hashed** — not merely tested but **unrepresentable to violate.** `hash_unit` takes a
  `Unit`; the runtime value lives in `StateHost.storage`; there is no input you could hand the
  hasher that carries it. (`Value` appears in `hash.rs` only in a comment.) The architecture is
  the proof; a test would be vacuous.
- **cell-bearing ⇒ non-replayable** — already proven by `state_graph_is_not_replayable`, and
  `is_replayable`'s own doc names it as exactly P2.6.3's positive form. Landed with Fork B.
- **fork = copy-on-write** — the one sub-part with observable behavior, and it needed no new
  mechanism: a fork is a new `StateHost` over a *clone* of the current storage. `forking_a_cells_
  state_is_copy_on_write` forks a counter at 6, runs the original to 8 and the fork to 7, and
  watches them diverge. Identity shared (one `&counter`), state per-host.
- **pin** — the value-not-hashed split once more: pinning a cell-bearing unit pins its *structure*
  (hash); state stays runtime. Drifts to **P3**, where pinning *configured-stateful* units is an
  actual marketplace concern.

**The shape of the result.** This is the "dissolved more than built" pattern in its purest form:
a single load-bearing decision (host-threaded storage) paid four debts at once, and P2.6.3's job
was to *recognize* that, add the one test fork actually has, and book pin to P3. Naming an inch
"already true by construction" honestly beats padding it with vacuous tests.

**Housekeeping.** Extracted `counter_graph()` — the canonical one-cell counter — shared now by
the state and fork tests.

**Next.** `P2.6.2` — transactional read-then-write. I expect "transactional" to want a careful
look first at what concurrency even *means* in a single-process eager host (the round is already
atomic: snapshot → fire → apply, no interleaving) before any code; the contract may be largely
present, or may need a precise modeled adversary to be worth stating.

---

## 2026-06-29 · P2.6.2 — transactional read-then-write, dissolved (the round is the transaction)

**One demonstration test, no production code (122 green = 111 core + 11 host).** Grounding
falsified the leaf's own premise. It called transactional read-then-write "the isolation
guarantee that *cannot* be composed from separate read + write units" — but in a single-process
eager host it **can**, and the round already provides the isolation.

**Why there's no adversary.** `round` is `&mut self`, so rounds are *serialized by the borrow
checker* — none overlap. The body is snapshot (`clone`) → `fire_sourced` (pure `eval`, nothing
committed) → apply (`StateWrite → storage`); nothing observes intermediate state. So a round is
atomic by construction: the write that lands is computed from the value read, with no gap an
interleaving writer could exploit — because there is no other writer and no mid-round observation.
The interleaved read-modify-write the leaf imagined simply cannot occur.

**Conditional updates compose.** `compare → Bool` and `select(cond, a, b)` already exist, so a
read-modify-write is an ordinary graph: `store_read → compare(N, cap) → select(·, N+1, N) →
store_write`. `saturating_counter_is_one_atomic_conditional_update` runs it five times and watches
`0 → 1 → 2 → 2 → 2`: the saturation at 2 *is* the proof the conditional branched on a consistent
snapshot. No transaction primitive — the composition is the transaction.

**The deferral, stated honestly.** The genuine transactional adversary — a *concurrent* writer, or
*shared external* state two processes both touch — only appears with real multi-process hosting.
And when it does, "transactional" there will **not** be a Holon core primitive: it will be
whatever the backing store offers (a DB transaction, a CAS on a key), carried through the `Host`
seam — the same deferral as real `email`/socket. P2.6.2's real future home is *inside a platform
host*, not the round or the core.

**Three in a row — no longer a coincidence.** P2.6.1 (storage in the host), then P2.6.3 (identity)
and P2.6.2 (transaction) each dissolved into it rather than building atop it. That single design
choice — *the cell's value is host runtime state, threaded between atomic rounds, never in the
unit* — turned out unusually load-bearing: value-not-hashed, non-replayable, fork-CoW, pin, and
now read-then-write isolation are all corollaries of it. The lesson is the project's own thesis
turned inward: pin the right boundary once and the "features" downstream become theorems.

**Next.** `P2.5.1` correlation token — and it is *genuinely different in kind*. It touches the
**value model** and request identity, not state, so it will not dissolve into host-storage the way
these three did. It is the design-forward fork (does every value carry routing identity, or is the
token explicit?) and wants its own L4.

---

## 2026-06-29 · P2.5.1 — correlation as a boundary label (isolation is purity's theorem)

**Two caps, one sink-compute, two tests (124 green = 112 core + 12 host).** The leaf asked for a
correlation mechanism — machinery to keep request A's reply from reaching caller B. Grounding
found that machinery is a **trap**: we already have it, as a theorem of purity, and building it
again would be the bad kind of duplication.

**Why non-crossing is free.** For two replies to cross, request A's data must bleed into request
B's path — i.e. *shared mutable state*. But the interior is pure: independent requests flow down
independent data paths, `eval` has no ambient channel, no global. The only place two requests can
mix is a *state cell*, a merge the user deliberately writes. So "two concurrent requests, none
cross" is the **same purity** that gives replay and memoization, seen sideways. The demo
(`concurrent_requests_keep_their_tokens_no_crossing`) is `map(respond, [[1,"alpha"],[2,"beta"]])`
→ each reply keeps its own token, and it *looks* like a `map` test because that **is** the point:
correlation is purity preserving independence, not special machinery.

**So the token isn't the isolation — it's a boundary label.** It is only what survives the
round-trip so the *host* knows which open connection to write the reply on. That settled the
explicit-vs-implicit fork from two directions at once: explicit (the token is ordinary data in the
`[token, reply]` payload — no `Value` change, since `Effect`'s payload is any `Value`) is not just
cleaner but the *only shape consistent with non-crossing being free* — an implicit ambient token
would itself be the shared channel two requests could cross on. Keeping the token as data keeps
the theorem true. (Same `I8` move yet again: derive/route by data, don't store identity in the
value model.)

**The surface, and a noticed corollary.** `request → [NetworkIngress]`, `respond →
[NetworkEgress]` make the in/out pair real; `respond` constructs an `Effect` like any Fork-B sink;
`request` is a sourced driver (no compute). The classification test pins something nice: a
`request → respond` closure is **effect-bearing yet replayable** — it bears egress but has no
`State`, so it stays cacheable. (Egress doesn't forfeit replay; only `State` does.)

**Deferred, honestly.** Real routing (token → an actual socket) is a **platform host**, same seam
as real `email`. Client-flavor *await* (fire a request, suspend, resume when the response arrives)
is a graph state-machine across rounds — async-adjacent (`P2.4.2`). And sourcing a request *batch*
as an ingress driver hits `constant_unit`'s non-scalar limit, so the demo models the batch as an
input — the same non-scalar deferral state already carries; the correlation point is identical.

**Next.** `P2.5.2` idempotency (a key on a sink so the external system dedupes a double-fire —
"engine carries the key, user composes the policy"), then `P2.7.2` mode-aware firing (consolidate
design/deploy under one host surface). Two inches from a closed P2.

---

## 2026-06-29 · P2.5.2 — idempotency = determinism + a data key + external dedup

**One demonstration test, no core change (125 green = 112 core + 13 host).** Like correlation, the
key is plain data in the `[key, body]` payload and `Host::perform` already carries it — but the
*reason it works* is the opposite of correlation's, and that's the point worth recording.

**Purity is the source of the problem and the fix.** Correlation's isolation was free because
purity prevents crossing. Idempotency is about a **double-fire**, and that is not an interior
property — it's caused by re-running the graph (a retry, a resubmit, a replay). Purity makes
re-running *expected*: a replayable graph re-evaluated reconstructs the same `email`/`respond`
again, by design. So determinism (P1.4.4 — proven: identical `(unit, inputs)` ⇒ identical output)
turns "the same effect fires twice" from a bug into a **dedup-able event** — the re-fire rebuilds
the *identical key*, and an external system that remembers keys recognizes it. The demo
(`an_idempotency_key_dedupes_a_refire`) fires the same intent (`"order-42"`) twice; `DedupeHost`
performs once, because the second fire's key matched the first.

**Where the guarantee lives — two halves, neither a new mechanism.** The engine guarantees the key
is *stable* — but that's just determinism, already proven. The user guarantees the key *tracks
intent* — by deriving it from intent data with pure primitives. That responsibility has teeth: a
key derived from a `Clock` driver or a `State` counter would **vary** across re-fires and defeat
the dedup. So an idempotency-key subgraph must be **replayable** (`is_replayable` already expresses
exactly this) — a future guardrail could enforce it; for now it's the user's discipline, noted.
The dedup itself is the external system's (modeled by `DedupeHost`; real → platform host).

**The P2.5 pattern, stated once.** Both effect contracts dissolve to the same shape: *a label
rides as ordinary data; the engine carries it without interpreting; the boundary acts on it; the
real boundary defers to a platform host.* Correlation's label is "which connection" (isolation is
purity's); idempotency's label is "which intent" (stability is determinism's). Neither needed a
`Value` change, an `eval` change, or a `Host` change — the value model stayed clean, exactly as
Fork B and A→B set it up to.

**Next.** `P2.7.2` — mode-aware firing, the **last leaf in P2.** Pure consolidation: `design_eval`
(core) and `fire`/`fire_sourced` (host) both exist; P2.7.2 is one host surface that picks between
them, with a test that the *same* graph mocks an effect in design and fires it for real in deploy,
the core staying mode-blind throughout. Then P2 closes.

---

## 2026-06-29 · P2.7.2 — mode-aware firing, and **P2 closes**

**One host surface, two modes (126 green = 112 core + 14 host).** `Mode{Design, Deploy}` + `run`
name a choice the host already had: `design_eval` (core — mocks drivers, the whole graph goes
pure, performs nothing) vs `fire_sourced` (host — real drivers, effects performed). The only real
change is that `fire`/`fire_sourced` now *return* the settled output, so design mode can hand back
a **preview** — the same `timer → email` graph returns `Effect{email, 0}` with the driver mocked
and **nothing sent**, then performs `(NetworkEgress, 42)` for real in deploy. The core never sees a
`Mode`: design substitutes mocks at resolve time, deploy supplies real values, both through the
**one** evaluator (`I2`/`I5` intact).

---

## ▣▣▣ MILESTONE — P2 (The OS / effect boundary) is COMPLETE ▣▣▣

Twelve leaves, every one built or **honestly dissolved**. What stands:

- **Classification** (`P2.1`–`P2.3`): `Kind{Pure,Driver,State,Sink}` derived not asserted;
  10 atomic `Capability`s; the manifest unions up the closure and surfaces before import; fail
  closed on the unknown. Two orthogonal axes — **effect-bearing ⊥ replayable**, **sourced ⊥
  performed** — decide everything the host needs.
- **Firing** (`P2.4`, `P2.7`): one evaluator, two modes. Design = `eval` over a substituting
  resolver (mocks); deploy = the host shell. Effects **output-carried, self-describing**
  (`Value::Effect{sink-name, payload}`) — `map` fans out to N, heterogeneous effects ride one
  output, no multi-output composite. The only impurity is `Host::perform`.
- **State** (`P2.6`): one host-threaded cell. `P2.6.1` built it; `P2.6.2` (transactional) and
  `P2.6.3` (identity/replay/fork/pin) **dissolved into it** — the round *is* the transaction,
  value-not-hashed is unrepresentable-to-violate, fork is CoW, non-replayable is tested.
- **Effect contracts** (`P2.5`): correlation and idempotency, both **a label riding as data**;
  the engine carries it, the boundary acts on it. Correlation's non-crossing is purity's theorem;
  idempotency's key-stability is determinism's. Neither touched `Value`.

**The Elm runtime is closed on a pure core:** `Model` = host storage, `update` = the graph,
`Cmd` = sinks→effects, `Sub` = drivers→sourced values — and the entire effectful surface reduces
to one impure function.

**The lesson of the phase.** Pin the right boundary once and the downstream "features" become
theorems. P2.6.1's host-storage decision dissolved three leaves; purity dissolved correlation's
isolation; determinism dissolved idempotency's stability. Of twelve leaves, the genuinely *new*
code was small and the recurring move was recognition — the system's own thesis (derive, don't
assert; keep the core pure) turned back on its own roadmap.

**Honestly deferred (not gaps — decisions):** real platform hosts (real `email`/clock/socket,
token→socket routing, external dedup — all behind the one `Host` seam); keyed cells/drivers and
non-scalar sourcing (parameterized capabilities + list `constant_unit`); async/await + quiescence
(`P2.4.2` — eager makes it unnecessary now); the replayable-key guardrail (`is_replayable` on an
idempotency key).

---

**Next — P3, The Marketplace.** The trustless memo cache where `is_replayable` stops being a
predicate and becomes **the product**: pure + content-pinned ⟹ universal, shareable memoization.
This is the project's actual thesis and a far larger design arc than any P2 leaf — it will want
real grounding and its own L4 sequence, not an inch. The spine so far: P1 (language) ⊳ P2 (OS)
both closed; P3 is the third and final spine pillar.

---

## 2026-06-29 · The genesis primitive set & the admission gate (design inch, no code)

A design-only inch, written into `COMPOSABLE_VM_DESIGN.md` §13 — pinning the atom set
as the trust root before P3 makes it permanent.

**The frame that did the work.** The primitives are the *leaves of the hash DAG*
(confirmed in `hash.rs`: composites encode children by hash, primitives by name). So
the atom set is the entire surface of *trusted semantics* — everything above it is
verifiable by re-hashing, nothing above it must be trusted. That reframed "how many
primitives" into "how small can the privileged, unforkable centre be," which is the
permissionless thesis itself.

**The gate (decidable, not taste).** Native iff a composite couldn't match it:
worse complexity class, or hot-and-pervasive (finer than memo grain), or impossible
(an effect = a capability), or genuinely inexpressible (the constructive test: a `for`
loop *builds* as `fold`, so it's surface syntax, not an atom; iteration itself does
*not* build, so `fold` is native; `while` is refused — it would forfeit totality).
Everything failing the gate is a blessed stdlib composite that, by I1, *feels* native.
Convenience is served one tier out, in the open, for free — and the gate is what stops
the native set ballooning over the years.

**The set: 27 frozen atoms** — 15 language core (incl. the two control atoms `select`
+ `fold`, and `cons`/`head`/`tail`/`is_empty`), 6 text (Text kept *opaque*; casing
native via locale-independent Unicode default mapping — a determinism/I2 call, since
Turkish-i-style locale tailoring would poison every downstream hash), 6 effects
(`email`/`respond` merged into `egress`); `spawn` deferred, append-only.

**Two donors, two payoffs.** `fold` (language) and the state cell (OS) generate almost
everything above them. Control flow = `select` + `fold` only ⇒ **everything is total by
construction** ⇒ the memo thesis is airtight (no verifier ever asked to confirm a result
that might not halt). And the demotions (subtract's inverse, the list lib, typed
constants, email/respond) are the one breaking move — re-hashing — hence genesis-timed;
appends stay free forever.

**Status:** no code, no tests touched (126 green unchanged). This is the trust-root
characterization + admission gate; the *act* of freezing the canonical set, and the
totality decision it rests on, are the natural first preconditions when we ground P3.

---

## 2026-06-29 · P3.8.1 — content-addressed store (local CAS); P3 begins

**The first leaf of the Marketplace pillar (136 green = 117 core + 19 host).** Where
units live, keyed by their 32-byte hash. Chose the additive path (prove the machinery on
the current primitive set; defer the breaking genesis reconciliation to just before the
registry), so this inch moves nothing's hash.

**The one genuinely-new piece is in core: `decode_preimage`,** the exact inverse of
`encode_preimage`. The preimage was already length-prefixed, tagged, and counted, so the
decoder is mechanical — but identity-critical, so every read is bounds-checked and counts
from the (untrusted) stream are never pre-allocated (a blob that lies about a length hits
`UnexpectedEof`, not an allocation). Five tests pin the property: `encode∘decode` is
byte-exact, the hash survives the round-trip, malformed bytes are rejected with a reason.
Added `Type::from_tag` as the mirror of `Type::tag` (one append-only table, both directions).

**Two boundaries did the load-bearing work on the host side.** (1) **`Store` is not
`Resolver`** — the store returns *owned* units for persistence; `Resolver` hands out
borrows for the eval hot path. So evaluation never reaches for disk, and IO stays cleanly
on the host side of the purity boundary. (2) **Verify-on-read is pure content-addressing**
— the stored bytes *are* the hash preimage, so `get` decodes, re-hashes the decoded unit,
and refuses it unless it equals the key it was filed under (`I5` made operational: a blob
is trusted only because it hashes to its own name). This needed no `blake3` in the host —
the crate-dependency boundary holds, `holon-core` still at one dependency.

`MemStore` and `FileStore` share the exact encode/key/verify/decode path, differing only
in the byte backend — which is what makes the deferred networked CAS (`P3.8.2`) a third
backend, not a rewrite. The leaf's test stands: `store(unit)→hash`; `fetch(hash)→` the
identical unit; a tampered blob is rejected.

**The loop the plan predicted is visible here:** P3.3 *forced* content-addressing at the
very start; the store is the first place that pin is *consumed* — units addressed by the
hash P1.6 already computes. Exposure, not invention.

**Next — `P3.1.1`, the registry:** a `name@version` alias resolving to a hash, publish
(unit→store + alias→hash), and the immutability rule (a published alias resolves to one
hash forever). It's the first place a hash becomes a *permanent public commitment* — so
the genesis reconciliation (§13) is the thing to settle in the same neighbourhood, before
the first publish fragments the trust root.

---

## 2026-06-29 · Genesis reconciliation R1a — list-structure irreducibles (cons/head/tail/is_empty)

**The reconciliation to §13 is an arc, not an inch** — most of it (effect renames, the
five-constants→`const` merge, demoting the list library to `fold`-composites) re-hashes
existing units, and the demotions need a stdlib mechanism first. Wrote
`P3-genesis-reconciliation-L4.md` to sequence it into six independently-green inches
(R1–R6): additive first (no hash moves), breaking-but-localized renames next, the heavy
demotions last behind an open fork. **The fork (decide at R6):** full §13 (build the stdlib,
demote, minimal trust root) vs trust-root-freeze (add the irreducibles, clean names, keep the
convenience derivables native, let the admission *gate* stop future bloat). Both give the
registry a stable trust root; deferred because R1–R5 are required either way.

**R1a (additive, 137 green = 118c + 19h):** added `cons`/`head`/`tail`/`is_empty` to `compute`.
O(1) build/inspect — the irreducibles §13 was missing; every demoted op (sum/map/filter…)
will be a `fold`-composite over these. `head`/`tail` of `[]` ⇒ `None`, reusing the existing
`get`-out-of-bounds convention (partial-via-`None`; it halts, just yields no value, so
totality is intact); `is_empty` is total. No existing hash moved.

**Re-confirmed the two-table rule:** a pure primitive lands in `compute` (primitives.rs) **and**
the pure allowlist in `capabilities_of` (kind.rs); the enumerated `pure_leaves_are_pure` test
was extended in place so any drift between table and list fails loudly. No `eval`/`simulate`/
`validate` change — pure leaves flow through `compute`, aren't sourced, carry no capability.

**Next — R1b · `fold`:** the one iteration primitive and the universal list donor. Higher-order
(reads a `fn`-hash param, threads an accumulator over a list), so it's the first reconciliation
inch to reach into `eval.rs`, beside `map`.

---

## 2026-06-29 · Genesis reconciliation R1b — `fold` (closes R1: pure irreducibles)

**The keystone primitive (140 green = 121c + 19h).** `fold` is the second higher-order
primitive, so it lives in `eval.rs` beside `map` (it needs the resolver to look up the
combiner by hash — `compute` can't). Shape committed: `fn` is a param (the combiner's hash,
structural), `init` + `list` are inputs (runtime), combiner applied left-to-right as
`fn(acc, elem)` — the conventional left fold. `fold(init, []) = init` (total); any `None`
propagates (like `map`). The test shows the payoff directly: `fold(add, 0, [3,7,5]) = 15` is
`sum` as a fold — exactly how every demoted list op (R6) will be rebuilt.

**Control flow is now provably two primitives — `select` (choice) + `fold` (iteration) —
and nothing in the vocabulary can express unbounded looping** (`fold` is bounded by its
list; `while` was refused at the gate). The language is total *by construction*, visible in
code, not just the design doc. That totality is what the P3 trustless cache leans on: a
verifier is never asked to confirm a result that might not halt.

Same two-table discipline: `fold` registered pure (higher-order, effect-free) in both
`capabilities_of` and the enumerated `pure_leaves_are_pure` test. Additive — no hash moved.

**R1 (pure irreducibles) complete.** Next — **R2 · text irreducibles** (`text_length`,
`slice`, `index_of`, `lowercase`), `Text` stays opaque; the one real call is casing =
locale-independent Unicode default mapping (an `I2` determinism decision — locale tailoring
would poison every downstream hash).

---

## 2026-06-29 · Genesis reconciliation R2 — text irreducibles (Text opaque)

**Four native text ops (141 green = 122c + 19h), additive.** `Text` stays opaque (not a list
of char-codes — that would regress UTF-8 to O(n) and launder text through `List`). Added the
ops that must touch the encoding: `lowercase` (mirror of `uppercase`), `text_length`, `slice`,
`index_of`. Three decisions baked in:

- **Char-based throughout** (Unicode scalar values): `text_length` counts chars, `slice` indexes
  in chars, `index_of` re-counts chars before a byte-level `find`. The three never disagree; a
  multibyte char is never split. (Tested with `"ab\u{00e7}"` = 3 chars / 4 bytes.)
- **Casing is locale-free by construction.** Rust's `to_lower/uppercase` are Unicode *default*
  case conversion, not locale-tailored — so the same text hashes the same on every machine. This
  is the `I2` call (locale tailoring, e.g. Turkish dotted/dotless i, would poison every
  downstream hash and break the trustless-sharing thesis).
- **`index_of` returns a -1 sentinel**, not `None`: "not found" is a normal, total answer, not a
  computation failure — it must not propagate as `None` and collapse the graph. `slice` is total
  too (start past end ⇒ empty).

The rest of the text surface (`contains`/`starts_with`/`split`/`replace`/`trim`/`join`) composes
from these four + `compare`/`fold` — stdlib, not native. Two-table rule held (allowlist + test).

**Next — R3 · `random` driver** (sourced like `clock`; capability `Random` already defined),
the last additive inch. Then the trust root starts moving: R4 (effect renames) + R5 (const merge).

---

## 2026-06-29 · Genesis reconciliation R3 — `random` driver (closes the additive run)

**The smallest inch, and the point is its smallness (142 green = 123c + 19h).** Adding a new
driver to the language was **one line of real code**: a single `capabilities_of` row,
`random → [Random]`. Everything else is generic. No `compute` arm — a driver has no pure
computation (entropy is host-sourced at deploy), so `random` falls through `_ => None`. No
`simulate.rs` change — `sourced_mock` keys on a leaf's *output type*, not its name, so `random`
(output Number) mocks to 0 in design mode through the exact path `timer` uses. Classification,
sourcing, mocking, design preview — all worked the moment the capability was declared. `I8`
(derived-not-asserted) + the two-axis model paying off: the engine learned a new way to touch
the world by being told one fact about it.

**The additive run (R1–R3) is complete; not one existing hash has moved.** The native set now
holds every irreducible §13 specifies — pure kit, text kit, drivers. The trust root has only
grown, never shifted.

**Next — the breaking inches.** R4 (`timer→clock`, `request→ingress`, `email`+`respond`→`egress`)
and R5 (five constants → `const`) change the dispatch names ⇒ the preimage ⇒ hashes. Each is a
multi-file lockstep rename (compute/sink_effect names, `capabilities_of` rows AND the enumerated
tests, `simulate.rs` helpers, host tests). Discipline: survey *every* occurrence first, move them
together. Approach R4 by grepping the workspace for each old name before touching anything.

---

## 2026-06-29 · Genesis reconciliation R4a — timer→clock, request→ingress (first breaking inch)

**The first inch where hashes deliberately move (142 green = 123c + 19h, count unchanged — a
rename adds no tests).** Two clean 1:1 renames to §13 names. `clock` and `ingress` now hash
differently than `timer`/`request` did; no test asserts a golden hash, so the suite holds.

**Survey-first paid off immediately.** Grepping every occurrence first revealed `request` is two
things: the *primitive* (→ `ingress`) and the *word* (Fork-B "the request a sink constructs",
correct terminology that had to stay). A naive find-replace would have corrupted the vocabulary —
so the rename was scoped to quoted strings for the primitive, leaving the word (still 8× in
primitives.rs, correctly). `timer` was unambiguous (every occurrence the primitive), so it cleaned
all the way to helper-fn and variable names.

**Why R4 split.** `clock`/`ingress` are 1:1: one table arm in, one out, nothing collides. R4b
(`email`+`respond`→`egress`) is a *merge* — after renaming, `capabilities_of` and `compute` would
each hold two identical `egress` arms (duplicate patterns, a compile error). R4b's real work is
deduplicating those collapsed arms + merging the two sink tests + rewriting the request/response
comment block left intact here. Hence its own inch.

**Next — R4b · the egress merge.** Survey `email`/`respond`, collapse to one native `egress` sink
(both already NetworkEgress; the host dispatches on capability, not sink name, so the merge is
clean host-side), dedup the arms, rewrite the P2.5.1 comment.

---

## 2026-06-29 · Genesis reconciliation R4b — email+respond→egress merge (completes R4)

**The structural rename: two sink primitives collapse into one (142 green = 123c + 19h, count
unchanged — the two sink tests were renamed, not removed).** `email`/`respond` → a single native
`egress`. Safe because of the capability/name split built in P2: **the host dispatches on the
*capability* (NetworkEgress), never the sink name.** Both were always NetworkEgress; the name only
fed `capabilities_of` to *derive* it. So unifying to `egress` changes nothing the host does — what
distinguished "send mail" from "send a reply" now lives where §13 says it belongs: in the payload
(a mailbox, or the `[token, reply]` pair the correlation test still checks). The reply lost nothing
— its token was already plain data, non-crossing already purity's theorem.

**Three site-types needed care beyond a blanket sed** (the checklist for R5):
1. *Collapsing match arms* — `capabilities_of` and `compute` each ended with two identical `egress`
   arms (duplicate patterns = compile error); deduped by hand to one. A duplicate `let egress` cap
   check (email + respond → same leaf) also dropped.
2. *Explanatory comments* — three comments name `email`/`respond` on purpose (documenting the
   merge); they must survive the rename, so primitives.rs was edited by hand, not sed'd.
3. *Embedded identifiers* — `email_hash`, `map_fans_out_one_email_per_element`: a trailing `_`
   defeats `\bemail\b`, so a substring grep caught them after the word-boundary pass.

**R4 complete: all effects carry §13 names** (clock, ingress, egress). **Next — R5 · const merge**:
number/boolean/text/true/false → one `const` reading value+type from params. Same species as R4b
(N primitives → 1), so the same three-site checklist applies. Then the R6 fork (demotions).

---

## 2026-06-29 · Genesis reconciliation R5 — const merge (COMPLETES the arc R1–R5)

**Five constants → one data-driven `const` (142 green = 123c + 19h, count unchanged).** The most
*spread-out* inch — eight files — because constants are minted in more places than any effect:
eval/determinism/promote/validate fixtures, the design-mode mock (`sourced_mock`), and the host's
state-cell injector (`constant_unit`/`scalar`) all build scalar units. But the change is the
cleanest consolidation: `number`/`boolean`/`text` (+ the niladic `true`/`false`) fold into one
`const` reading `value` + `type` params.

**Key decision held: `const` *declares* its type, never infers it.** `const(Number,"true")` ⇒ None
(no coercion); `const(Boolean,"true")` ⇒ the bool; a literal `"true"` string ⇒ `const(Text,"true")`
— three unambiguous units, three distinct hashes, exactly what content-addressing needs (I5). Test
pins it: value-not-parsing-for-type ⇒ None, and a type with no literal form (List) ⇒ None.

**Disposed of a latent inconsistency:** `true`/`false` were never real primitives — allowlist +
purity-test entries with no `compute` arm, a "pure leaf" that couldn't produce a value. Gone now (a
boolean literal is just `const(Boolean,…)`). Trust root is smaller *and* more honest.

**RECONCILIATION ARC R1–R5 COMPLETE.** The native set matches §13 on every axis: pure irreducibles
(cons/head/tail/is_empty + fold), text (lowercase/text_length/slice/index_of, Text opaque), drivers
(clock/random/ingress, store_read), effects (one egress + store_write), constants (one const). Only
R6 remains — and R6 is a genuine design *fork* (demote to stdlib vs trust-root-freeze), not a
mechanical inch. To be discussed, not just executed.

---

## 2026-06-29 · Genesis reconciliation R6 — DECISION: freeze the trust root (Branch B), defer demotion

**No code (the native set is already at 142 green); this inch is a decision pinned in writing.**
The R6 fork was: shrink to the theoretical-minimal trust root by demoting convenience leaves to a
blessed stdlib (Branch A), *vs* freeze the as-built set and let the admission gate bar future bloat
(Branch B). **Chose B as the interim state, deferring A to its own pillar.**

Reasoning: Branch A's payoff — the smallest surface for an auditor to verify — only accrues *once
there is an auditor*, i.e. the networked trustless cache (deferred P3 tier). No auditor today, and A
additionally needs the blessed-stdlib distribution mechanism invented first (a pillar's worth of
design, not a cleanup). So the minimal-trust-root principle is acknowledged and deferred, revisited
when stdlib mechanism + auditing use-case both exist. The gate stays fully active as
gatekeeper-for-the-future: the set is grandfathered, not derived, but it never grows by habit again.

**Pinned in COMPOSABLE_VM_DESIGN.md §13a** — the frozen genesis manifest, 34 leaves (the entire
trusted-semantics surface, append-only hereafter): 16 pure language core, 6 text (Text opaque), 6
list-convenience (demotion-deferred — the concrete leaves a future pillar moves to stdlib), 3
drivers, 2 state, 1 sink. Totality (select+fold, no while) unchanged by the larger freeze.

**GENESIS RECONCILIATION COMPLETE (R1–R6).** The trust root is frozen in the §13 shape, reached one
green inch at a time, breaking hashes only where designed. This was the hygiene prerequisite before
the registry's first publish (the first permanent public commitment of a hash).

**Next — back to P3 local-first**, now unblocked: registry (P3.1.1) → inspection (P3.2.1) → local
memo (P3.3.1, the canonical-Value encoder is the new piece) → fork/re-pin (P3.5.1, needs L4) →
advisory (P3.6.1). Networked tier (shared cache P3.3.2, reputation, identity) still deferred.

---

## 2026-06-29 · P3.1.1 — the registry (the first permanent publish boundary)

**First P3 inch after the reconciliation; the marketplace's trust model is first anchored here
(147 green = 123c + 24h, +5 host registry tests).** `Registry<S: Store>` over the P3.8.1 store, in
holon-host. `publish(root)` has exactly one load-bearing job: **closure-completeness (I6 at the
boundary)** — walk the unit's children *and* its `fn` param, and **fail closed** (`DanglingReference`)
on any reference the store can't resolve. A dangling hash would be fetchable yet un-evaluable,
poisoning the trust root the instant it's committed. Verify-first-then-commit ⇒ a refused publish
leaves nothing extra. The store's verify-on-read means every resolved reference is the right bytes,
so the closure check integrity-checks for free.

**The two tests that matter are the dangling cases:** an unstored child, and a `map` whose `fn` hash
was never stored — the I6 subtlety a children-only walk silently misses, hence its own test.

**Judgment call — identifying a hash-valued param.** The mock-walker's lenient heuristic ("follow
any value that parses as a hash") is *wrong* for publishing: a literal Text const whose value is 64
hex chars would parse-as-hash, fail to resolve, and wrongly reject a valid unit. So the registry keys
on the param **name `fn`** — the exact convention `eval` already uses to find the combiner. Not an
arbitrary special-case; aligning to the evaluator's reference convention. Limitation noted in code:
when a 2nd hash-valued param appears, this wants a shared `child_refs(unit)` helper in core that eval,
mock-walker, and registry all consult (single source of truth). Future refactor, not needed yet.

**One core change:** `parse_hash` widened `pub(crate)`→`pub` so the host can follow a `fn` hash —
legitimate public API, symmetric with `hash_unit`, widening is safe.

**Scope honest:** hash-only (no names — hash *is* identity); published *record* in memory (units are
persisted by the store; persisting the record is a short follow-on); no networking.

**Next — P3.2.1 inspection:** surface a unit's full transitive structure (same closure shape) so a
consumer audits what they're about to depend on *before* pinning — I9 full-inspectability made concrete.

---

## 2026-06-29 · P3.2.1 — inspection + the child_refs unification

**More consolidation than new surface (154 green = 130c + 24h, +7 core inspect tests).** New
core module `inspect.rs`: `dependencies(root, resolver)` — the transitive closure below a root,
deduped and hash-ordered, so a consumer audits exactly what they'd depend on before pinning (I9
made an operation). Flat reachable-set, not a tree: each composite already lists its children, so
flat + bodies *is* the structure, and flat is what an audit ("is every node pure?") wants.

**The substantive work: dissolving three divergent closure-walks into one `child_refs`.**
`collect_caps` (kind), `collect_mocks` (simulate), and registry `verify_closure` (host) each had
their own answer to "what does a unit reference." Two used the **parse-heuristic** (follow any
value that parses as a hash) — a latent fail-closed-wrong bug: a `const(Text, value="<64 hex>")`
would make `collect_caps` saturate and report a *pure text constant as maximally capable* (a
security-relevant misclassification), and `collect_mocks` would try to mock a string. No unit
triggers it today, so nothing was red — but it was real. `child_refs` keys on the param **name
`fn`** (eval's convention), removing the whole class; pinned by `child_refs_ignores_a_hash_shaped_
non_fn_param` (value = 64 valid hex chars, correctly ignored).

All four walks — capability classification, mocking, publish, inspection — now consult one
definition; they can't drift, and a 2nd hash-valued param is added in exactly one place. The
"align to the natural boundary" principle doing concrete work: the boundary was "what does a unit
reference," it had three contradictory impls, dissolving them was the fix. (Also removed two
now-unused `parse_hash` imports; updated the P3.1.1 registry doc from "future refactor" to done.)

**Next — P3.3.1 local memo** (the cache thesis made real): a pure unit's hash + inputs ⇒ output
forever, so cache and re-serve without re-eval. New piece: a **canonical `Value` encoder** (the
cache key needs inputs in a stable hashable form; Value derives only PartialEq). Real design call
— what makes two inputs "the same" for caching — so it gets an L4 first.

---

## 2026-06-29 · P3.3.1 — local memoization (the cache thesis, made real)

**The moat turns from a property into a mechanism (162 green = 135c + 27h, +5 core encoder, +3 host
memo).** Cache key = `BLAKE3(unit_hash ‖ canonical_encode(inputs))` — a content hash of (unit,
inputs), derived identically on any machine. The resolver never enters the key: a pure unit's hash
already captures its whole transitive closure (I6), so (unit_hash, inputs) fully determines the
output. Same derivation will key the shared cache (P3.3.2) and let any party verify by re-hashing.

**FCIS split:** the pure, decision-heavy half — canonical `Value` encoder + `memo_key` — in core
(`memo.rs`); the cache itself a `Memo` state cell in host. The thesis made literal: the trustless
cache is a state cell keyed by a content hash (the OS's universal donor storing the language's pure
results).

**The f64 canonicalization (the one real decision, an I2 matter):** NaN (any bit pattern) → one
fixed encoding; both signed zeros → +0.0; else IEEE bits LE. Both collapses sound because eval
treats the collapsed values identically (IEEE ops/comparisons are NaN-payload-agnostic; -0.0==+0.0).
NaN/inf can't enter as literals (const rejects non-finite) but ARE reachable computed (1e308² → inf,
inf-inf → NaN), so the encoder must handle them. Tagged + u32-LE length-prefixed (mirrors preimage
encoder) ⇒ collision-free across types. Tests pin: 2 different NaN patterns → same key; ±0.0 → same;
4 types distinct; order + presence matter.

**Soundness guard — only is_pure units cached** (∅ caps), NOT is_replayable. A driver's output
isn't a function of inputs (varies per call); a sink *performs* — caching would skip the effect.
The `impure_unit_is_not_cached` test uses `egress` deliberately: it evaluates (constructs an Effect)
but is never cached, so the effect can't be silently skipped. Impure ⇒ falls through to plain eval.

**Honest overhead (correctness-first):** per-call purity walk + per-call re-hash. A later inch
threads the hash through rather than recomputing. **Local-first P3 nearly complete.**

**Next — P3.5.1 fork/re-pin** (needs L4) then P3.6.1 advisory. The shared trustless cache (P3.3.2,
the actual differentiator) stays deferred, but its key-derivation half now exists and is tested.

---

## 2026-06-29 · P3.5.1 — fork / re-pin (the sanctioned creation of a new identity)

**The marketplace's generative act (169 green = 140c + 29h, +5 core fork, +2 host).** Everything
prior *preserves* identity; fork *derives* a new one. Core `fork.rs`: `refork(root, target,
replacement, resolver) -> Option<Refork>` re-pins every reference to `target` → `replacement`,
**path-copying** — only the changed node's ancestors are re-minted; every off-path subtree keeps
its hash and is shared. The `repins_deep_and_shares_off_path` test proves it concretely:
root→[A,B], A→[C,D], replace D ⇒ minted.len()==2 (A, root), B and C asserted NOT in minted. A
one-node change costs one spine, not a deep copy — content-addressing's payoff made visible.

**Re-pinning is uniform with `child_refs`** (P3.2.1 paying off again): a child is a hash swap; a
`fn` param becomes `Param::frozen("fn", to_hex(replacement))` — the inverse of the `parse_hash`
eval uses to read it. The same references inspection reads, fork rewrites. Pure transform (FCIS:
mints units, returns them; host stores). Provenance: NONE in the unit — content-addressing makes
the fork independent; any derives-from link is external, deferred.

**Host `fork` inherits the publish invariant for free:** refork → store the minted spine →
`Registry::publish(new_root)`. The forked closure is complete by construction (minted ∪ shared-
already-stored ∪ replacement), so the existing `DanglingReference` gate covers it with zero new
checking. The one new glue: `materialize` (walk the closure from store into a MapResolver, since
refork wants a Resolver) — the store↔resolver duality's 3rd appearance (registry, inspection,
fork); a Store→Resolver adapter is an increasingly-earned future tidy.

**Compile fix (cargo caught):** `to_hex` was `#[cfg(test)]`-gated (only test callers until now);
fork is the first *library* code to *write* a `fn` param, so ungated it. `parse_hash` (read) and
`to_hex` (write) now both library-available — matching the read/write pair child_refs/refork form.

**Next — P3.6.1 advisory** (last local-first inch): surface derived advisory signals before
depending (capability summary, purity, closure size) — all already computable (capabilities,
is_pure, dependencies). Exposure, not mechanism. Then local-first P3 is complete.

---

## 2026-06-29 · P3.6.1 — advisory signals (COMPLETES local-first P3)

**The lightest inch — pure exposure, no new mechanism (174 green = 145c + 29h, +5 core).** Core
`advise(unit, resolver) -> Advisory` bundles the four signals a consumer wants before depending:
`capabilities` (what it can touch — saturated, fail-closed), `is_pure` (cacheable), `is_replayable`
(no state), `dependency_count` (supply-chain weight). All already computable; advisory just packages
them.

**"Advisory" is load-bearing: facts, not gates.** Nothing forbids anything — a unit touching the
network or pulling 50 deps isn't blocked; the consumer reads and decides. Fits the marketplace
philosophy (make everything inspectable, let participants choose). Signals are derived (I8, never
asserted), so they can't lie — `capabilities` is saturated from the real closure.

**No host wrapper this inch:** a `Registry::advise(hash)` wants the same store→resolver materialize
that fork uses. That duality has now appeared 4× (registry, inspection, fork, advisory) — the right
move is a `Store`→`Resolver` adapter as its own small refactor, after which host `advise(hash)` is a
2-line convenience. Core `advise` over a Resolver is the genuine capability.

**LOCAL-FIRST P3 MARKETPLACE COMPLETE.** publish (P3.1.1) · inspect (P3.2.1) · memoize (P3.3.1) ·
fork (P3.5.1) · advise (P3.6.1) — the full set of local-first operations, all green, all built
almost entirely from invariants that predate them ("exposure, not invention"). The store (P3.8.1)
underlies them.

**Deferred (networked tier):** the shared trustless cache (P3.3.2 — the actual differentiator;
key-derivation half built in P3.3.1), reputation-by-adoption (P3.4.1), author-identity (P3.7.1),
networked CAS (P3.8.2). Foundations (content-addressed keys, closure verification, derived signals)
all in place + tested. **Accumulated refactor note:** the `Store`→`Resolver` adapter (would clean
fork's materialize + enable host advise(hash) cheaply).

---

## 2026-06-29 · `Store`→`Resolver` ownership fork — examined, SET ASIDE (no code)

Pressure-tested my own repeated note ("adapter refactor … appeared 4×, earned") before acting on
it. It was wrong three ways, now corrected in PLAN + captured in `P3-store-resolver-fork-L4.md`:

1. **Not an adapter — impossible as named.** `Resolver::resolve` returns a borrow (`&Unit`);
   `Store::get` yields owned units (decoded fresh, verify-on-read). No `&self`-lived `&Unit` to
   lend; `RefCell` can't return a reference past its guard. A store→resolver *must* eagerly
   materialize into an owned `MapResolver` — which is fork's `materialize`. An adapter renames the
   glue, not dissolves it.
2. **Not 4×.** The materialize glue is in *one* place (fork). `verify_closure` walks the store
   checking presence; `dependencies`/`advise` are pure-core over a Resolver with no host wrapper.
   Rule of *one*, not three — extract the helper when a 2nd real caller lands, not before.
3. **Not debt — a perf optimization.** The only real change would be the `Resolver` trait →
   `Option<Cow<'_, Unit>>` (the only form that keeps the in-memory path zero-copy; `Option<Unit>`
   clones the hot eval path, `Rc` changes MapResolver's repr for nothing extra). Bounded blast
   radius: 2 impls, ~14 sites (half already-clone test sites), 1 real return-type change
   (`dependencies` → owned map, which *drops* the `'r` param). And it buys **only** lazy
   `eval`-against-store (select/fold pruning skips unvisited units) — a saving that's currently
   *unrealizable* (eval runs against in-memory MapResolvers; the Store is publish/registry).

**Trigger recorded** (all three): a deploy-from-store eval path exists; programs large/branchy
enough that pruning skips a meaningful fraction; eager `materialize` *profiles* as the bottleneck.
Decision rule when the deploy tier lands: whole-closure op (advise/inspect a hash) → cheap eager
materialize helper; lazy eval of a big branchy stored program → the `Cow` change. Until then:
nothing. Next genuine step is the networked tier (P3.3.2 shared trustless cache).

---

## 2026-06-29 · P3.3.2 — the shared trustless cache (the differentiator)

**The thesis made operational (183 green = 151c + 32h, +6 core +3 host).** P3.3.1 made memoization
local; P3.3.2 makes a result one party computed *checkable by anyone*. Core (extends `memo.rs`,
reusing the private `encode_value`): `CacheEntry {unit_hash, inputs, output}` (self-describing so a
recipient who didn't compute it can verify it); `entry_key` (= `memo_key` of the fields — cheap
structural integrity, the "which computation" check); `verify_entry` (the **trustless oracle**:
resolve + is_pure + re-eval + **canonical** output compare). Host: `SharedCache` (untrusted store;
`put` dumb-inserts under the derived key, `get` is the fast/trust-the-source path, `get_verified`
re-executes and refuses a liar).

**Why it's the thesis: three invariants converge in `verify_entry`.** Content-addressing ⇒ the
claim names one immutable computation (can't be swapped). Determinism (I2) ⇒ re-execution is
*decisive* (one correct answer). Totality (no `while`; control = select+fold) ⇒ the oracle always
*terminates* (a verifier that could hang is no verifier). A memo is checkable exactly because the VM
is pure, total, content-pinned.

**Grounded NaN finding (would have been a silent hole):** `eval` can reach `inf`
(multiply/add don't guard overflow) and thus `NaN` (`divide`/`subtract` of inf). `Value` is only
`PartialEq` (NaN≠NaN), so a naive `==` fails an honest NaN result *on the same machine*, and NaN bit
patterns aren't portable across machines. So `verify_entry` compares **canonically** via
`encode_value` — the same fold `memo_key` applies to inputs — making verification machine-
independent. The `nan_output_verifies_canonically` test (inf-inf via a pure `subtract`, inputs carry
the inf so no composite needed) is the proof.

**Cheap/expensive split mirrors the Store:** `entry_key` cheap structural (re-hash analogue,
always-on, earns its keep at the network boundary P3.8.2); `verify_entry` expensive semantic
(re-execute, opt-in). **Honesty in the API:** `get` (trust source) vs `get_verified` (trust no one);
the liar test stores a wrong output under the right key, confirms it's stored (cache untrusted),
then watches the trustless read reject it.

**What this does NOT do (deferred, each its own pillar):** skip compute on a stranger's word — needs
reputation (P3.4.1), proofs, or quorum. All three now have this substrate to stand on. Next genuine
step: P3.4.1 reputation-by-adoption, or P3.8.2 networked CAS (the transport; cheap entry_key check
ready for it).

---

## 2026-06-29 · P3.4.1 — reputation-by-adoption (the navigation layer)

**Adoption = inbound citations in the published DAG (191 green = 156c + 35h, +5 core +3 host).**
Core `adoption_counts(corpus)` — pure tally over `child_refs`: for each referenced hash, how many
*distinct* corpus units cite it (a unit citing U twice counts once). Host `Registry::adoption()` —
the `verify_closure` walk collecting instead of checking: gathers the published universe (every unit
in any published root's closure), then calls the core tally.

**Load-bearing: derived, not asserted (I8).** Adoption is recomputed from the actual DAG every call,
NEVER a stored counter. So it can't be inflated except by publishing real, inspectable referring
units. The thesis extends once more: content-addressing made memoization *trustless* (P3.3.2); now
it makes reputation *unfakeable-by-assertion*. Same move — you can't lie because the truth is in the
content. Why it's trust and not mere popularity: adopting U = pinning its exact hash = committing to
its frozen, fully-inspected behavior forever (I5+I9); a far stronger signal than a mutable-world star.

**Framing correction (recorded in the L4):** I'd earlier called P3.4.1 "result-trust" (skip compute
on a stranger's word). Wrong: a result is *objectively* verifiable (re-exec settles it; determinism
makes the correct V the unique honest attractor), but counting endorsements needs distinct parties =
Sybil-resistance = identity. So result-trust is **re-scoped onto P3.7.1**. What's buildable now,
identity-free, is *unit* adoption (a unit's quality is NOT objectively verifiable, so an aggregate
signal genuinely informs) — and it's the more useful half anyway.

**TWO DEFERRALS, documented in PLAN with triggers:**
1. **Sybil-resistance (recursive/eigenvector weighting, PageRank-style).** Flat citation count is
   inflatable by junk referrers (cheap but inspectable). Fix weights a citation by the citer's own
   adoption (junk ≈0 ⇒ contributes ≈0). Heavier (global iteration). **Trigger:** corpus large enough
   that flat counts are noisy, OR Sybil inflation actually observed.
2. **Result-trust ("skip compute on a stranger's word").** Needs identity (P3.7.1) to count distinct
   endorsers, or stake. The determinism-attractor groundwork is laid; the mechanism waits on
   identity. **Trigger:** P3.7.1 identity lands.

**Decisions confirmed:** (A) scope = unit-adoption-by-citation, result-trust → P3.7.1; (B) ordering =
adoption-first (identity later hardens + extends). Next: P3.7.1 author-identity or P3.8.2 networked CAS.

---

## 2026-06-29 · P3.7.1 author-identity — DROPPED (no code; decision)

**Caught over-engineering before building it.** I had identity/crypto/attestation in the plan as a
"keystone" and wrote an elaborate L4 (Ed25519, signed attestations over hashes, Signer/verify). The
push that killed it: *why do we need any of this — this is an open free market.* Right. The
keystone holds up nothing; the arches were already standing on **verifiability**.

**The principle:** trust here is from *verifiability, not identity.* Content-addressing + purity +
totality means you never trust anyone — inspect a unit (I9), verify a result by re-running it
(P3.3.2; totality ⇒ the check terminates, determinism ⇒ it's decisive). Identity is what you reach
for *when verification fails or is too costly*; in a system where verification always works, reaching
for it **hedges against the foundation** and smuggles back the gatekeepers an open market exists to
avoid. Trust/identity/reputation/gatekeeping are the machinery of systems that can't let you check
for yourself.

**The three motivations dissolve identity-free:**
- Sybil-resistance of adoption → **recursive weighting** (citation weighted by citer's own adoption;
  junk ≈0) — a content-graph property, no identity. And adoption is an advisory hint, not a gate.
- Result-trust → results are **objectively verifiable**; a wrong result is caught regardless of who
  signed. The only real (narrow) problem is *expensive*-to-verify computations → **validity proofs**
  (zk) or **re-verification/spot-checking** — identity-free. Sign-and-burn is one mechanism, neither
  necessary nor most aligned, and only bites in a mature market that doesn't exist.
- Authorship/reputation → the unit speaks for itself (fully inspectable); credit is a social nicety,
  quality-by-reputation a crutch for the unverifiable (a thin sliver here).

**Net:** the open free market needs **zero identity** — publish without a gatekeeper, inspect
anything, verify any result, fork, discover by adoption, all present and identity-free. Missing only
**transport** (P3.8.2), which also needs no identity (fetch by hash + verify by re-hash; fetch a
result + verify by re-execution; the cheap `entry_key` check is already waiting).

**Preserved (if ever revisited):** identity must be optional, external, an attestation *over* hashes,
never in the preimage; core stays identity-free and trustless. (The one correct constraint from the
killed design; archived in `P3.7.1-identity-L4.md`.)

**Corrected the two P3.4.1 deferrals** to their identity-free resolutions (PLAN header). **Next:
P3.8.2 networked CAS / transport** — the literal "open market": content + verifiable results flowing
between strangers, nobody's identity required.

---

## 2026-06-29 · P3.8.2 — networked transport (the architecture already secured it)

**The finding, not the build, was the work (194 green = 156c + 38h, +3 host).** Grounding "networked
CAS" showed content-addressing **already** secured the network boundary. A stranger hands you data
claiming it's X; trustless transport = check the claim against content, never the stranger:
- **unit** (bytes claimed = hash H): `decode_verified(key, bytes)` — decode, re-hash, reject if ≠.
  **Already built**, used by every `Store::get`; a networked Store reuses it verbatim.
- **cache result** (output for K): `verify_entry` re-executes — **already built** (`get_verified`).
- **cache key** (entry claimed at K): `entry_key(entry) == K` — primitive built (P3.3.2), but the
  *ingest using it* was the one gap.

**Built the one gap: `SharedCache::ingest(claimed_key, entry)`** + `CacheIngestError`. The cache's
analogue of the Store's verify-on-read: cheap structural check (does the entry hash to the key it's
offered under?), rejecting a peer lying about *which computation* an entry is for. `put` derives the
key (can't model a claimed key); `ingest` is how an entry from *elsewhere* (a peer, a cache dump)
enters. Two tiers, same as everywhere: cheap admission on ingest, expensive trust on read
(`get_verified`). The third test proves it — a right-key/wrong-output entry passes ingest, caught by
get_verified. Exposure of a check the foundation already provided (`entry_key` finally earns the keep
P3.3.2 promised it), not new machinery.

**Deferred — and NOT a gap, correct sequencing:** the wire itself (TCP/HTTP/libp2p, async runtime,
peer discovery, framing). Its **security is already handled** by the above, so it's *plumbing* — a
different kind of work (large dep surface, async I/O, untestable in this crate's pure-unit-test
discipline), belonging to a deployment/infra phase behind whatever transport a deployment picks. Also
deferred (YAGNI): a `Remote`/`Peer` trait (speculative until a 2nd transport), a networked `Store`
impl (when built, a `Store` whose `get` fetches bytes + calls existing `decode_verified` — no new
trust logic).

**MILESTONE — networked-tier LOGIC complete.** The whole open market stands, trustlessly: publish
without a gatekeeper, inspect anything, verify any result, fork/build on anything, discover by
adoption, exchange units + results with strangers — all identity-free, all secured by
content-addressing, never by trusting anyone. What remains is plumbing (the wire) + the gated R6
demotion pillar + (parallel) the P4 editor. The architecture was designed so transport is nearly
trivial; this inch confirmed it and closed the last asymmetry.

---

## 2026-06-29 · P4.2a — the wasm bridge + CI (the editor's foundation, enforced)

**P4 begins; second build target; bit-identical-core invariant now CI-enforced (195 native green
[156c + 38h + 1 wasm] + 1 wasm-target parity).** New crate `holon-wasm` (cdylib + rlib), depends on
`holon-core` ONLY (never host — the purity boundary extended to wasm, Cargo-enforced). Exposes
bytes→bytes ops via `wasm-bindgen`: `hash_unit_bytes` (decode_preimage → hash_unit) and
`canonicalize_preimage`. CI (`.github/workflows/ci.yml`): native `cargo test --workspace` + `cargo
build --target wasm32` + `wasm-pack test --node` (the parity guardrail).

**Proven:** (1) the pure core **compiles to wasm32 with ZERO changes** — blake3 + core, clean. The
purity discipline (FCIS, no I/O in core) held all project = the core is portable for free; a pure
core has no platform. (2) **content-addressing is bit-identical native vs wasm** — `hash_parity`
runs on both targets (native `#[test]` + wasm `#[wasm_bindgen_test]`), each asserting bridge ==
direct core hash; BLAKE3 byte-determinism + u32-only preimage encoder (no usize-width/float hazard)
⇒ native-core hash == wasm-core hash. The invariant the whole editor rests on is now a CI gate, not
a hope.

**Decomposition:** P4.2 split into **P4.2a (this — build + hash parity; dodges the Value fork by
bridging only bytes→bytes)** and **P4.2b (next — the `eval` bridge: the JS `Value` boundary +
NaN/±0.0 canonicalization + eval parity).** P4.2b's one fork: how `Value` crosses JS↔WASM — lean is
a `holon-wasm`-local serde type converting to/from `core::Value`, keeping `holon-core` on its single
`blake3` dep (no serde derive in core).

**Structure decided (prior turns):** monorepo / one Cargo workspace; `holon-core` the shared pure
root both host and wasm compile from; dependency rule extended (host→core, wasm→core, never sideways
/ up); frontend `editor/` (TS) a same-repo client of the wasm pkg; CI the continuous proof the two
core-compilations stay identical. **Deferred (triggered):** extracting the target-agnostic
marketplace layer (Store/Registry/Memo/SharedCache) out of host into its own wasm-capable crate —
only when the editor needs in-browser marketplace ops, not preemptively.

---

## 2026-06-29 · P4.2b — the eval bridge (the editor can now run programs)

**Eval crosses to JS, bit-identical native vs wasm; the bridge is functionally complete (197
native green [156c + 38h + 3w] + 4 wasm-target).** Added to `holon-wasm`:
- **`WValue`** — a bridge-local serde mirror of `core::Value` (core stays blake3-only). Adjacently
  tagged: every value is `{ type, value }` in JS — uniform, recursion-friendly. Lossless
  `from_core`/`into_core`.
- **`Evaluator`** (stateful) — `insert(preimage) → hash` builds a `MapResolver`; `eval(preimage,
  inputs) → output`. Wraps the SAME core `MapResolver` + `eval` — no second evaluator. Stateful so a
  live editor doesn't re-ship the closure each keystroke; `null` output = "not ready", not an error.

**Two decisions, both grounded:**
1. **serde-wasm-bindgen, not JSON.** Straight out of P3.3.2's NaN work: eval can yield `NaN`/`Inf`
   (overflow), JSON can't carry them (→ error/null, silent corruption), but JS numbers are IEEE-754
   f64 and carry them intact. The same non-finite concern that forced canonical compare forced the
   serialization choice — the foundation's quirks propagate consistently.
2. **NaN parity by `is_nan()`, not bits.** Which payload bits a NaN carries is target-dependent and
   meaningless; "it's some NaN" IS the canonical view — the cache's equivalence, applied across the
   native/wasm seam.

**Proven:** `check_eval_parity` runs on both targets (finite arithmetic pinned exact, a composite
through the resolver, the inf−inf NaN case) → eval bit-identical native vs wasm. And
`evaluator_evals_through_js_boundary` (wasm) proves the real path end-to-end: JS array of `WValue`
→ `Evaluator::eval` → core eval → JS output → 5. serde-wasm-bindgen handshake works first try.

**Bridge status:** hashing (P4.2a) + evaluation (P4.2b) both cross to JS, both bit-identical, both
CI-guarded. The core needed ZERO changes for any of it — the purity discipline is the whole reason.
**Next: the surface** — P4.1 canvas (place units, wire ports, wire-time type check), evaluating live
through this `Evaluator`. Needs a frontend-stack decision (canvas vs SVG, framework, state model)
before code — a genuinely different kind of work from the pure-Rust loop.

---

## 2026-06-29 · P4.2c — the construction bridge (the editor can build units, not just eval them)

**Grounding the eval path caught a prerequisite I'd glossed:** the `Evaluator` takes a *preimage*,
and the only producer of preimages was `encode_preimage` in core. For the editor to place a `const`
node or wire a composite it must get those bytes — and the two ways are (a) reimplement the canonical
encoder in TypeScript (a SECOND encoder, free to drift — the exact thing this architecture refuses)
or (b) a construction path in the bridge. So this inch is (b), and it came *before* the frontend
scaffold to stay in the testable Rust loop one more step. (198 native green [156c + 38h + 4w] + 6
wasm-target.)

Added to `holon-wasm`:
- **`WUnit`** (+ `WBody`/`WParam`/`WPort`/`WEdge`/`WType`) — a serde mirror of `core::Unit`, the way
  `WValue` mirrors `Value`. A node is a plain JS object: `{ body: { kind:"Primitive", name }, params,
  inputs, output }`. Children cross as hex hashes, types as bare strings (`"Number"`).
- **`build_preimage(WUnit) → bytes`** — the editor's "compile this node to canonical bytes", through
  the ONE core encoder. **`load_unit(bytes) → WUnit`** — the inverse, for loading a unit's structure
  onto the canvas and (later) descending into a composite to inspect it (I9).
- **Hashes now cross as lowercase hex strings** (`hash_unit_bytes`→`hash_unit_hex`; `Evaluator.insert`
  returns hex). `Uint8Array` can't be a JS map key or `===`-compared; a 64-char hex string can, and
  the editor uses hashes as node identity constantly. Small revision to just-shipped P4.2a/b, made
  before any consumer locked in bytes.

**Decision watched, then cleared:** `WBody`/`WParam` use serde *internally-tagged* (`tag="kind"`) for
nicer JS shape. That's the historically finicky case with serde-wasm-bindgen; the adjacently-tagged
fallback (`content="value"`) was ready. `construction_through_js_boundary` passed on wasm ⇒ 0.6.5
handles it, no fallback needed.

**Proven:** `wunit_roundtrip` (native+wasm) — `WUnit` mirrors `Unit` losslessly, same value AND same
canonical bytes (so same hash). `construction_through_js_boundary` (wasm) — the FULL editor path:
build a `const(7)` and a composite from plain JS objects, through the encoder, insert, wire to
boundary, eval ⇒ 7. That is exactly what P4.1's canvas walks.

**Bridge status:** hash + eval + **construct** all cross to JS, through the one encoder / one
evaluator, bit-identical native vs wasm, CI-guarded. The frontend scaffold is now unblocked — and
small, because every hard part it stands on is proven on both targets.

---

## 2026-06-29 · P4.1.0 — the editor's first heartbeat (the toolchain carries a real eval)

**Green by a rendered number, not a test.** Stood up `editor/` (Vite + React + TypeScript strict +
`@xyflow/react`), built `holon-wasm` into it via `wasm-pack build --target web`, and a hardcoded
`const(7) → boundary` graph evaluates **to 7 in the browser** — built from plain TS objects, through
the one core encoder (`build_preimage`), inserted, wired, and evaluated through the one evaluator
(`Evaluator`). The `const` hash (`3ec55bcc…`) rendered too: content-addressing ran in-browser and
produced the same address the native host would. The whole chain — Rust core → wasm-pack → npm →
Vite → browser → eval → back — works. This retires the only P4 risk I could not test natively: does
the bridge actually load and run in a browser.

Scaffold (11 committed files: 10 hand-written + `package-lock.json`; `wasm/` and `node_modules/`
gitignored — the bytes always come from `crates/holon-wasm` so the editor's copy can never drift):
- `src/holon.ts` — the **typed boundary**. Its `WUnit`/`WValue`/`WParam`/… are discriminated unions
  mirroring the Rust serde types exactly; the editor codes against a real contract the compiler
  checks. (The reason we chose TypeScript, made concrete — wasm-pack's `.d.ts` types the JsValue
  crossings as `any`, and this file makes them honest.)
- `src/App.tsx` — walks the exact path P4.1's canvas will (build → insert → eval → render), with a
  hardcoded graph instead of a draggable one. React Flow draws the same graph beside the result so a
  failure in either half is visually separable.
- `--target web` + relative import + `init()`: dev needs no wasm plugin (it fetches its own `.wasm`,
  served by Vite because `editor/wasm/` sits inside the project root). Production `vite build` may
  later want `vite-plugin-wasm` — deferred.

**This is the heartbeat, deliberately NOT the canvas.** No palette, no custom nodes, no wiring UI —
those are P4.1 proper, each its own inch. Next: real nodes from a palette, draggable, with
`isValidConnection` enforcing the wire-time type check (I1), projecting to a composite and evaluating
live through this same path.

---

## 2026-06-30 · child-order canonicalization — identity = structure, not presentation

**Built.** `encode_preimage` now normalizes a composite's *child order*, closing the last place
presentation leaked into identity. `hash.rs` gains `canonical_composite` (+ helpers `factorial`,
`enumerate_orderings`, `permute`, `apply_child_order`, `order_key`) and `EncodeError::TooSymmetric`.
Children are ordered by hash; equal-hash (interchangeable) children are ordered by minimizing the
`children ++ remapped-sorted-edges` serialization over within-group permutations — presentation-
invariant by construction; edges are remapped to the new indices (`BOUNDARY` fixed) and sorted.
Bounded by `MAX_CHILD_SYMMETRY` (8!), failing closed beyond it.

**Leaf closed.** A core-identity refinement, taken *before* the editor mints composites in volume
(*settle identity before you mint* — the genesis-reconciliation precedent), on the still-empty corpus.

**Grounded first.** The linchpin is in `eval_composite`: it topo-orders by *edges* and uses a child's
index only to resolve which `Unit` a slot holds, so reordering children (edges remapped) changes no
value — child order is inert exactly like edge order, which the encoder already sorted. Encoding it
into identity was therefore a bug.

**Tests prove** (+2 core → 158c · 38h · 4w = 200 native; 6 wasm-target): `child_order_is_cosmetic`
(same wiring, opposite child order ⇒ equal hash); `duplicate_children_canonicalize` (two same-hash
children in different roles ⇒ one deterministic, presentation-invariant hash). Amended:
`decode_round_trips_a_composite_with_params` (authors canonical children — decode∘encode is identity
only on canonical input now); `fork_repins_and_republishes` (children asserted set-wise, since the
store round-trips through canonical order).

**Forks pinned.**
- Canonicalize in `encode_preimage` (the one boundary every caller inherits), not a separate caller-
  invoked pass — consistent with edge-order normalization, impossible to forget.
- `ENCODING_VERSION` stays **1**: the change only *restricts* the encoder to canonical forms (output
  set ⊆ the old range; primitives + already-canonical composites byte-identical), so no cross-version
  collision is introduced and no existing hash needlessly churns.
- **Deferred (with trigger):** wiring-based refinement (WL coloring) to shrink duplicate-hash groups
  and lift the symmetry cap — trigger: a real composite hits `MAX_CHILD_SYMMETRY`.
- **Noted, not done:** `MapResolver` returns units as-inserted (so possibly non-canonical child
  order) while their hash is canonical — eval-inert, so it stands; only the on-disk `Store`
  round-trips through canonical order.

---

## 2026-06-30 · primitive-interface catalog — core's third per-name axis

**Built.** `catalog::interface_of(name) -> Option<Unit>` (new `holon-core/src/catalog.rs`) — the
authoritative typed interface (named input ports, output type, default params) of each §13a genesis
leaf. The third per-name axis beside `compute` (behaviour) and `capabilities_of` (capability), same
fail-closed contract (unknown ⇒ None). `GENESIS` enumerates the 34 names; `genesis_units()` is the
list the editor's palette will derive from.

**Leaf closed.** The foundation P4.1's palette stands on: core states its atoms' interfaces in one
place, so the editor can *derive* the palette rather than hand-writing a TypeScript copy that would
drift from core (an I1 parity break).

**Grounded first.** `compute` defines behaviour and implies arity (`let [a,b] = inputs`) but states no
types or port names; those lived only in §13a prose and were re-hand-written in every `prim()` test
helper. The catalog is §13a as code, names matched exactly to `compute`/`eval`.

**Calls pinned.** Variadic leaves (`add`, `and`, `concat`, `list`…) get canonical arity 2 (variadic
editing is an editor concern); polymorphic leaves use `Any` (the wiring check already treats it as
compatible); `map`/`fold` carry their function as an open `fn` param, matching the evaluator; sourced
leaves get honest types (`clock`/`random` Number, `ingress`/`store_read` Any); `interface_of` returns
a default-configured unit (`const` at Number 0, `compare` at `op="eq"`) so the palette drops a usable,
valid node.

**Tests prove** (+5 core → 163c · 38h · 4w = 205 native; 6 wasm-target): `interface_of_unknown_is_none`
(fail-closed); `genesis_has_34_leaves` (the §13a count, a tripwire); `every_genesis_name_resolves`
(each name resolves and its body name matches — no enumeration/match drift); `the_two_catalogs_agree`
(interface and capability axes cover the same atom set — neither can drift from the other);
`catalogued_units_evaluate` (interfaces proven against real behaviour by *running* them: const→0,
add(2,3)→5, not(true)→false).

**Scope held (KISS).** Core-only this inch — the wasm `primitive_catalog()` bridge and the palette UI
go next, where they're tested by use. Migrating the `prim()` test-helper duplication to derive from
the catalog is a noted follow-up, not done here.

---

## 2026-06-30 · primitive_catalog() bridge — the palette's source crosses to JS

**Built.** `primitive_catalog() -> JsValue` in `holon-wasm` — core's genesis interface catalog
(`genesis_units`) as an array of `WUnit`, the single source the editor's palette is *derived* from
(never a hand-written TS copy that could drift, I1). Returned whole in one crossing; the editor builds
its palette list and its name→interface map from it.

**Leaf closed.** The bridge half of P4.1's palette: core's authoritative interfaces are now reachable
from JS, proven to cross intact, before any UI depends on them.

**Grounded first.** `load_unit` already factors the core→JS mirror (`WUnit::from_core`), so
`primitive_catalog` reuses that exact function over the catalog — a *second* converter would be a
parallel mechanism free to drift from the first, the precise thing the one-encoder/one-evaluator
discipline forbids. One whole-catalog call, not a per-name `primitive_interface`: the palette needs
all 34 at once to render; N crossings would tempt the editor to cache its own copy (drift again); a
single-name need is just a filter over the array already held.

**Tests prove** (+1 native → 5 wasm-native = 206 native; +2 wasm-target → 8): `primitive_catalog_native`
and `primitive_catalog_wasm` run the shared `check_primitive_catalog` body on both targets (34 entries;
`add` = (a:Number, b:Number) → Number; `map` keeps its open `fn` param — all surviving the `Unit`→
`WUnit` mirror); `primitive_catalog_crosses_js_boundary` proves the `Vec<WUnit>` actually serializes
across the boundary — the new risk a single-`WUnit` `load_unit` test didn't cover. Interface
*correctness* stays owned by core's catalog tests; these only prove the crossing.

**Owned in the doing.** First wired `check_primitive_catalog` to the native test only — a dead-code
warning on the wasm build, where `native_tests` is cfg'd out and nothing else called it. Fixed by
adding the `_wasm` parity test, restoring the "one body, run on both targets" pattern every other
`check_` fn follows. core unchanged throughout.

---

## 2026-06-30 · palette + custom nodes (P4.1, first visual inch) — the editor places real atoms

**Built.** The authoring canvas's first surface: a **palette** built from `primitive_catalog()` (not a
hand-written list) and a custom React Flow node that renders a primitive's typed ports straight from
the catalog interface; **drag a primitive onto the blueprint canvas** to place a typed node. New
`editor/src/{theme,PrimitiveNode,Palette}.tsx` + `styles.css`; `holon.ts` gains `primitiveCatalog()`;
`App.tsx` becomes the editor (palette + canvas + drag-drop, replacing the heartbeat).

**Leaf closed.** The placement half of P4.1: core's authoritative interfaces now appear as draggable,
typed blocks. Wiring, projection/eval, and param editing are each their own next inch.

**The load-bearing rule, made structural (I1).** A node's handles come ONLY from what the catalog
returned (`data.inputs`/`data.output`), never hand-asserted — the editor never says “add has two
number ports,” it reads them from core. This is why the bridge inch came first; confirmed visually
(add → two amber Number inputs, amber out; select → cyan Boolean + two slate Any; egress → slate Any
in, coral Effect out; fold → its open `fn` param + Any/List inputs).

**State model.** A canvas node is a placed instance whose `id`/`position` are **view-only**, excluded
from identity (I5); handle ids are the port names — the references the wiring inch turns into `Edge`s.

**Design — type as color.** The editor's one bold, grounded choice: a wire type *is* a color (six
types, six hues), the whole palette derived from core's type system, chrome kept near-monochrome
blueprint-ink. This also makes the type check legible next inch — a legal wire is two ports of one hue
(or `Any`, the neutral). All in IBM Plex Mono; an instrument that edits code-like atoms speaks in mono.

**Proven, honestly.** No React test runner in `editor/`, so the visual claims (ports match core) are
**browser-confirmed** — the green here is a rendered canvas, not a count. The *provable* path
(project → eval through the one `Evaluator`) returns in a later inch. Verified what's verifiable:
strict `tsc` and `tsc -b` clean. Also removed a pre-existing `noEmit` in `tsconfig.node.json` that
broke `npm run build`. core/host/wasm unchanged — 206 native, 8 wasm-target.

---

### P4.1 · wiring — type-checked connections (DONE; the type system is now enforced, and visible)

**The inch.** Connect placed nodes, with the §5 type rule enforced *as you wire*. A wire is legal iff
the upstream output type and the downstream input type are `compatible` — and that rule is **core's**,
reached across the bridge, never a TS copy.

**Where the rule lives (I1).** `validate::compatible` (`dst==Any || src==Any || src==dst`) already
existed; it is now exposed as `compatible(src, dst) -> bool` in `holon-wasm` — deserialize two bare
type strings to `WType`, map to core `Type` via the one `into_core`, delegate to
`holon_core::compatible`. The pull to inline a one-line TS copy was real *because* the rule is trivial
today — which is exactly the trap: a copy diverges silently the moment the rule grows, and the editor
would start allowing wires `validate` rejects. The editor must never be a second authority on type
semantics — the same discipline that derives the palette from the catalog.

**The canvas check.** `isValidConnection` resolves the source node's output type and the target node's
named input type from the catalog-built node data, and returns core's `compatible`; missing either type
⇒ refused (fail closed). One-source-per-input (core's `duplicate_input_edge_rejected`) is guarded at
wire-time as bookkeeping over the editor's own edges — core's `validate` stays the real gate at
projection. Cycle/self-loop rejection is deliberately *not* here; that's projection's job, not built
ahead of its trigger. A connected wire is colored by its source port's hue — the wire carries a type.

**Proven.** `compatible` parity on both targets + a JS-boundary crossing — 163c + 38h + 6w = **207
native** (cargo-green). **10 wasm-target** — compiled, and the crossing is confirmed *live* by the
canvas itself returning correct answers across the real JS boundary (the stronger proof). Behavior
browser-confirmed: matching connects, mismatched refused, double-feed refused, `Any` accepts either
way. Strict `tsc`/`tsc -b` clean. core/host unchanged in count beyond the one wasm-native test.

---

## 2026-06-30 · eval — gather primitive inputs by reference (a measured perf fix; gate untouched)

**The defect, grounded.** The `native_vs_composite` bench (the standing guard for §13's
speed-justified atoms) showed the "O(1)" inspectors `head`/`length`/`is_empty` climbing
linearly with list size — O(n) *as evaluated*. Reading the firing rule found the cause in one
line of `eval`: `let present: Vec<Value> = inputs.iter().flatten().cloned().collect();` — the
evaluator **deep-cloned a primitive's whole input list** before `compute` ever looked at it.
`head` then read one element of the copy. The gate's "head is O(1)" was true of the *primitive*
and false of *evaluated head* — a defect in the evaluator, not the gate.

**Built.** `eval` now gathers present inputs **by reference** (`Vec<&Value>`, no `.cloned()`),
and `primitives::compute` borrows them (`&[&Value]`). Reading arms are unchanged — they ride
deref coercion (`&&Value → &Value`). The compiler forced an explicit `(*x).clone()` at exactly
the five arms that *retain* an input — `cons`, `select`, `append`, `list`, `sink_effect` — so
"clone only what you keep" is enforced structurally, not by discipline. No cache, no
special-case, no control around the symptom; no IO added (I2 intact); the §13 gate is untouched.

**Behaviour identical.** All **163 core** tests green unchanged (the test module's direct
`compute(...)` call sites were rewrapped to `&[&Value]`; assertions untouched). 38 host, 6
wasm-native, 10 wasm-target unchanged — **207 native** total.

**Measured (release; re-run of the same bench).** The O(1)-vs-O(n) split the design promised is
now visible: `head`/`length`/`is_empty` flat at the ~40 ns floor across n = 1…4096 (was climbing
to ~20 µs at 4096 — −99.8%). `native_sum` **−91%** at n=4096 (21 µs → 1.8 µs): it stops copying
the list then summing, and just sums the borrow — it now scales as real O(n) work, not as a copy.
`fold_sum` unmoved (it is intercepted before the firing-rule arm, so it never paid this clone).

**A prediction corrected, honestly.** I predicted `wrapping_add` would "barely move" (reasoning:
two scalar inputs, nothing to deep-copy). It dropped ~15–20% (native_add 47→41 ns, composite_add
432→348 ns). I under-weighted that `Value` is a fat enum, so eliminating even two scalar clones
*plus* allocating a `Vec<&Value>` instead of an owning `Vec<Value>` saves measurably at the
~40 ns scale, and the composite's inner `add` benefits from the same arm. The wrapping *ratio*
held (9.2× → 8.5×), so the tax that justifies keeping arithmetic native is intact — but "barely
move" was wrong; "drops somewhat" (the original findings' call) was the better prediction.

**Deliberately deferred (its own inch).** `eval_composite`'s scratch/boundary gather still
clones (`scratch[fc].clone()`, `inputs.get(fp).cloned()`), so an inspector *inside a composite*
still pays O(n) until that is addressed. Passing references through the scratch fights the borrow
checker (borrow earlier slots while assigning later ones) — a real, separate inch with its own L4
and its own before/after numbers, not braided in here.

**Next.** Decide whether the `eval_composite` by-reference inch is worth taking, or return to the
P4 projection frontier.

---

## 2026-06-30 · eval_composite by reference — tried and reverted (a dead end, kept for the record)

**What we tried.** Push the firing-rule inch's by-reference gather *into* `eval_composite`, so a
composed inspector (`head`/`sum` inside a composite) is O(1), not just a bare one — and as a bonus
stop `map`/`fold` cloning each list element. Shape: keep the public `eval(&[Option<Value>])`, add a
private `eval_ref(&[Option<&Value>])` worker that composite gather, `map`, and `fold` recurse
through; the composite scratch is borrowed, not cloned.

**It was correct** (163/38/6 + 10 wasm-target green). The borrow-checker fear was unfounded: `eval`
returns an owned `Value`, so the scratch borrow that feeds a child is released before the next write.

**But the bench said no.** The first cut regressed *every* primitive eval by ~24 ns (≈40 → 64 ns,
flat across sizes): the `eval` adapter allocated a `Vec<Option<&Value>>` on every call, on top of
the primitive arm's own `Vec<&Value>`. A fast-path in `eval` (bare primitives gather straight from
owned inputs, skipping the adapter) restored the floor — but that patch was the tell. The
eval/eval_ref split *created* the allocation, and the fast-path was a control bolted on to route
around it: a hedge against the owned public `eval`, not a fix to a real defect.

**And the win never justified the cost.** The stated goal — composed inspectors O(1) — stayed
unmeasured (no composite-wrapped bench case existed), and memoization blunts it anyway: the O(n)
clone is paid once, then served from cache. The one cleanly-attributable gain — `map`/`fold`
skipping the per-element clone — was ~5–10%. Not enough to earn a split in the evaluator (the most
invariant-critical file) plus a symptom-quieting fast-path.

**Reverted.** Kept the firing-rule inch (bare inspectors O(1), `native_sum` −91%); evaluator left
simple. Two missteps along the way, both caught by measurement and owned: a wrong `wrapping_add`
prediction (it dropped ~18%, not "barely"), and the adapter regression above.

**Lesson.** A new control that routes around a problem the refactor itself introduced is the signal
to stop, not to patch. Measure the actual goal before committing; an unmeasured speculative win does
not earn structural complexity.

---

## 2026-06-30 · projection — canvas → composite (the provable path) · *(P4.1)*

**Built.** `Evaluator::project` (holon-wasm) = core's `default_spec` ∘ `derive_and_freeze` over the
evaluator's own workspace resolver, returning the composite `WUnit`; a JS-free `project_core` heart so
it tests on both targets. Editor (`App.tsx`): an on-demand **project** button marshals the wired
canvas — children by index (drop view-only `id`/`position`), wires → core `Edge`s by index (handle
*name* → port *index*, from the catalog interface each node already carries) — calls `project`, and
shows the composite's content hash (or a surfaced `EncapError`). `holon.ts` needed no change: it
already exports `Evaluator`, `buildPreimage`, and `WEdge`/`WUnit`, and `insert` returns the hash.

**The one decision.** The editor marshals *structure only*; core derives everything that is identity
or interface — which inputs promote, the exposed output, the `BOUNDARY` edges, the types — and
validates and freezes. The editor re-decides none of it (one decider). `default_spec`/
`derive_and_freeze` already existed and were tested in core (P1.5.1); the bridge method is exactly
those two calls, nothing reimplemented in TS.

**Leaf closed.** The cosmetic/structural seam the design doc names is now operational: a visually-built
graph has a real content address. The editor mints units; identity is presentation-invariant.

**Proven.** +1 native (`project_native`) + 2 wasm-target (`project_wasm` parity;
`project_crosses_js_boundary` — the full JS round-trip equals the JS-free `project_core`, same
canonical bytes, and evals to 5) → **163c + 38h + 7w = 208 native; 12 wasm-target**. core/host
unchanged (bridge only). The canvas behaviour is **browser-confirmed** (no React runner): a lone `add`
→ a hash with 2 inputs; **move it → same hash** (I5, cosmetic); a second unwired `add` → an
**ambiguous error**, surfaced not guessed; wire the two → a **new hash**, 3 inputs.

**Forks / decisions pinned.**
- **Combined, not split.** `project` exposes only the canonical proposal then freeze. The *split* —
  expose `default_spec` + `derive_and_freeze` separately so the UI can reorder/rename promoted inputs
  and choose the exposed output — is earned by the encapsulation-UX inch (**P4.3**; recorded in the
  PLAN `P4.3.1` leaf). Until then an ambiguous boundary is a **surfaced error, never auto-resolved**
  (no heuristic control over a systemic choice).
- **On-demand, not live.** A button; live-on-change is a render nicety for later.
- **`from_port` hard-zero.** Every genesis primitive has one output (the `"out"` handle); multi-output
  composites are a deferred fork, and that is the single line that learns output-handle-name → index
  when they arrive.
- **One persistent `Evaluator` per canvas** (`useMemo`): re-inserting a node is idempotent (same unit
  ⇒ same hash), and the projected composite stays inserted — exactly what the live-eval inch needs.

**Next inch.** Live eval through the one `Evaluator` (run the projected composite — already inserted —
whole-graph; per-node values deferred), then param editing.

---

## 2026-06-30 · param editing — frozen values configurable; const's type re-types the node · *(P4.1)*

**Built (editor only).** `PrimitiveNode.tsx`: frozen param values become editable text fields, and
`const`'s `type` becomes a selector over **Number / Boolean / Text**. Changing a value writes back
through `updateNodeData`; changing a const's type also updates the node's **declared output** (so the
handle re-colours and `compatible` re-judges what it may wire into) and **drops wires from that output
whose target no longer accepts the new type** — fail-closed, via `setEdges`. The node owns all of this
through `useReactFlow` (`updateNodeData` / `setEdges` / `getNode`); `App.tsx`, `holon.ts`, the bridge,
and core are untouched.

**The decision that matters — declaring a type is not deciding types.** Core never derives a const's
output from its `type` param anywhere; the catalog *sets* `output: Number` for the default and trusts
it. That "output == type param" consistency is a **construction-time contract** whoever builds the
const upholds — the catalog for the default, the editor when you change the type. The editor is
fulfilling its constructor role, exactly as the catalog does, not usurping authority: `compatible`
stays the sole judge of what connects.

**Grounded in core, not assumed.** A `const` can be exactly the three types its `compute` parses from
a text value — Number / Boolean / Text (`primitives.rs`; `constants_from_params` pins
`const(type="List") ⇒ None`). List / Effect / Any have *no literal form*, so the selector offers
exactly three, the real boundary — which is *why* a string const refuses to wire into a List input.

**List literals are a constructor, not a new primitive (decision).** A list constant is `list(…)` /
`cons(item, list)` — `list`'s `compute` is genuinely variadic. `const` excludes List by the *same*
decision that gives it a text literal form (R5 collapsed all scalars into one `const`). Adding a
"series literal" would re-smuggle the inference `const` refuses (parsing structure from `"[3,7,5]"`)
and duplicate `list` (a KISS violation, two ways to build one thing). So the editor exposes the
constructor, not a literal. *Deferred follow-up:* ergonomic **variadic-arity editing** of
`list`/`add`/`and`/`concat` (grow/shrink element ports), gated on core marking *which* leaves are
variadic — a small fourth catalog fact, the same I1 discipline that put `interface_of` in core so the
palette derives rather than guesses. `list` today is canonical arity 2; `cons` chains cover longer
lists until then.

**Values stay plain text (KISS).** Core parses a frozen value; a bad entry just fails soft at eval.
Type-aware inputs (a number spinner, an `op` dropdown) would need a per-param value-domain table in
TS — core knowledge the catalog doesn't expose, so duplicating it would invite drift. Only `type`
graduates to a selector, because it is both a *closed core enum* and *interface-affecting*.

**Proven.** Editor-only ⇒ browser-confirmed (no React runner): a value edit moves the projected hash
(core's `param_value_changes_hash`, now through the canvas); `Number → Text` re-colours the output and
flips wire compatibility; a stale wire drops when its source type changes. Leans entirely on tested
core; **208 native / 12 wasm-target unchanged** (no Rust touched). `tsc`/`tsc -b` clean.

**Reorder (banked).** Param editing was pulled **ahead of live eval** (the projection block's NEXT had
eval first). Why: eval-before-params can't show a *varying* result — every placed `const` carries the
same catalog default, so the only way to change an output would be swapping a primitive. Params-first
makes the small **closed-graph** eval fully expressive (set two consts, run `(2)+(3) ⇒ 5`, edit one,
the result moves) *and* cleanly separates a node's **config** (frozen params — identity) from a
composite's **interface inputs** (supplied at eval — not identity). The heavier "boundary-input
fields" inch then lands in its natural spot (running an open composite through its interface), not
forced early just to make eval demonstrable. A roadmap sharpening, recorded like the others.

**Scope held.** Edits *frozen* values + the const type selector. Param **promotion** (open params →
composite interface, `open_param`/`instantiate`) is encapsulation-UX, untouched.

**Next inch.** Live eval through the one `Evaluator` — now expressive: a closed graph (consts feeding
pure ops) projects and runs, and editing a const moves the value. Then variadic-arity editing and
boundary-input fields for open composites.

---

## 2026-06-30 · live eval — closed graphs run through the one Evaluator · *(P4.1)*

**Built (editor only).** `onProject` (`App.tsx`) now reuses the composite preimage it already builds
for the hash and, when the composite is **closed** (no promoted inputs), runs it through the *same
persistent* `Evaluator`: `ev.eval(pre, [])`. The panel shows `= <value>` under the hash for a closed
graph, `not ready` when eval returns null, or `N inputs — supply to run` for an open composite. A
small `formatValue` renders the five `WValue` shapes (a sink as `effect(sink)`). No bridge / `holon.ts`
/ core change; no wasm rebuild — `eval` has been there since P4.2b.

**The provable path is now closed end-to-end.** The canvas mints a content-addressed unit *and*
computes its value — both through the one core, no second evaluator and no mock (this is core's pure
`eval`). Project gives identity; eval gives meaning; they run off the same inserted workspace.

**Why the inch was small — everything hard was already proven.** `eval` was bit-identical native vs
wasm and JS-boundary-tested in P4.2b (`eval_parity`, `evaluator_evals_through_js_boundary`,
`construction_through_js_boundary`); `project` already left the children inserted; param editing made
consts settable so a closed graph computes a value that *moves*. So this is the heartbeat's
`build → eval → render`, now on the real projected composite — nothing new to test.

**Honest edges (it is the pure evaluator, not design mode).**
- **Closed graphs only** — zero promoted inputs. An open composite shows `supply to run`; the
  boundary-input-fields inch fills that seam.
- A **sink** graph (`egress`/`store_write`) evaluates to the `Effect` it *constructs* — shown
  `effect(egress)` — it does not fire; firing is the host's deploy mode.
- A **driver** graph (`clock`/`random`/`ingress`/`store_read`) returns `null` = not ready (drivers are
  sourced, unsupplied here); running them needs design-mode mocking (`design_eval`), a later concern.
- A bad const value or divide-by-zero ⇒ `null` = not ready, fail-soft.

**On "live."** On-demand, folded into the project button — re-evaluating per keystroke would mean
auto-projecting on every change, reintroducing the cosmetic-move noise we avoided. "Live" = through
the live stateful `Evaluator`, not per-keystroke; auto-refresh is a later nicety.

**Proven.** Editor-only ⇒ browser-confirmed: `const 2 → add ← const 3` projects to a hash and `= 5`;
edit a const to 10 → re-project → new hash and `= 13` (the value tracks config — the params-first
payoff); a lone `add` shows `2 inputs — supply to run`; `const → egress` shows `effect(egress)`.
Leans entirely on tested `eval`; **208 native / 12 wasm-target unchanged** (no Rust). `tsc`/`tsc -b`
clean.

**Next inch.** Boundary-input fields — supply an open composite's promoted inputs at eval time, to run
reusable units through their interface. Then variadic-arity editing of `list`/`add`/… (gated on core
marking which leaves are variadic — the `interface_of` discipline).

---

## 2026-06-30 · P1.3.2 inch 1 — the composed-type-expression language (`List<T>`) · *(identity)*

**Built (core).** `Type` is now **recursive** — `List(Box<Type>)` — so it is `Clone`, not `Copy`.
The encoder/decoder recurse (a `List` tag is followed by its element type); `ENCODING_VERSION → 2`;
and a **fail-closed decode depth bound** (`MAX_TYPE_DEPTH = 32`, `DecodeError::TypeTooDeep`) closes the
one new attack surface a recursive decoder opens over untrusted bytes. `compatible` is lifted to
`(&Type, &Type)`: **structural equality + the `Any` wildcard, recursive** (`List<A>` fits `List<B>` iff
`A` fits `B`) — §5's rule nested, no inference, no subtyping. The catalog maps every bare list to
`List<Any>` (`Type::list_any()`) — **behaviour-identical**. `derive_and_freeze` now clones inner port
types, so **bottom-up promotion carries composed types faithfully** (I4). The wasm bridge stays flat
(`List<T> → WType::List → List<Any>`) — lossless now, since core only holds `List<Any>`; the recursive
`WType` is the editor inch.

**Scope — `List<T>` only (a grounded reconsideration of the L4).** The L4 had said land `List`/
`Product`/`Sum` types together to avoid a second version bump. Grounding the real code dissolved that:
the de-Copy is paid by `List` *alone*, and a second `ENCODING_VERSION` bump is **free** while the corpus
is empty. So landing `Product`/`Sum` types speculatively bought nothing and would be machinery ahead of
use (no constructor can produce one yet). They fold into their value-constructor inches instead.

**Identity — "settle before you mint" held.** Nothing persisted, so the `v1→v2` bump moved every hash
for free; no genesis-hash manifest needed re-pinning (none existed — a hash-assertion test would have
gone red). The proof of safety: **all 208 prior tests pass unchanged under the re-encoding** —
`List<Any>` is behaviour-identical to the old bare `List`. The new capability moved identity without
moving meaning.

**The de-Copy ripple.** ~20 mechanical sites across the workspace — almost all `Type::List →
Type::list_any()` in test fixtures, plus two `E0507` clones the compiler pinpointed (`p.output.clone()`
in a hash collision test; `unit.output.clone()` in `promote::open_param`). All compiler-guided; closed
in two rounds.

**Tests prove.** **212 native** (167 core — **+4**: `list_type_round_trips`,
`bare_list_encodes_as_list_any`, `deep_type_is_rejected`, `compatible_is_structural`; 38 host; 7
wasm-native) / **12 wasm-target**, all green. `wasm-pack build` + `tsc -b` clean.

**Next.** The value-constructor inches — `pair`/tuple (`Product`, rides the `List` value) then
`Sum`/`Option` (the tagged value kind) — each a free re-bump; or the editor inch (recursive `WType` +
type-as-colour + boundary-input fields). The composable type *language* is present; the constructors
light up the remaining generators.

---

## 2026-07-01 · P1.3.2 inch 2 — `Product` (the positional tuple) + the `pair` constructor · *(types compose)*

**Built (core).** `Type` gains a second recursive constructor — `Product(Vec<Type>)` (tag 7): a
`u32`-count-prefixed list of element types, decoded under the same `MAX_TYPE_DEPTH` bound and with **no
pre-allocation from the untrusted count** (a lying count hits `UnexpectedEof`). `compatible` gets the
product arm — **arity + position, `Any` at each slot, recursive** — and a `Product` is never a `List`.
`Encoder::ty` turned fallible (`Result<(), EncodeError>`) so a product wider than the `u32` budget is
**refused, not silently truncated** — the `FieldTooLong` discipline the child/edge counts already use.
`ENCODING_VERSION → 3`.

**`pair` — a new genesis leaf (34 → 35).** The binary product constructor: pure (∅ manifest), declares
`(a: Any, b: Any) -> Product([Any, Any])`. Its compute **rides the list value** — `pair(a,b)` builds the
same `Value::List` a 2-list does — so there is **no new `Value` variant, no memo encoding change, no
`value.rs` change at all**. Only the declared *type* tells a pair from a list, and the recursive checker
keeps them apart. The payoff: **`Dictionary<K,V>` = `List<Product([K, V])>`** — the composable shape this
fork set out to reach, with zero new runtime machinery.

**Type-mostly, by design.** The whole capability lives in the type language (declare / encode / check /
promote) plus one constructor; the value layer is untouched. That is the line the L4 drew — `Product`
rides `List`; `Sum` (next inch) is the one that earns a genuinely new value kind (the runtime tag).

**Inch-1 follow-on, folded in.** `sum` and `join` tightened from `List<Any>` to **`List<Number>` /
`List<Text>`** — the first place `List<T>`'s precision bites: a typed `List<Text>` is now refused by `sum`
at wire time (core `validate`), while a constructed `List<Any>` still bridges in (the `Any` wildcard), so
every existing `list → sum` graph keeps working.

**Editor stays coarse.** The flat bridge maps `Product → WType::List` (forced by exhaustiveness, and
honest — a product is list-shaped), so an editor-placed `pair` behaves like `list` until the
recursive-`WType` editor inch. Element types are precise in core's `validate` now, in the editor then —
the same core-precise / editor-coarse staging as `sum`/`join`.

**Ripple.** Two compiler-named edits: `simulate.rs` gained `Type::Product(_) => return None` (no sourced
leaf outputs a product — unreachable, fail-closed like the `Effect` arm), and the two wasm catalog-count
assertions moved 34 → 35. The `v2→v3` bump moved every hash again, free (nothing persisted, no manifest;
parity tests compare values to each other, not fixed hex).

**Tests prove.** **216 native** — 171 core (**+4** since inch 1: `sum_and_join_are_element_typed`,
`product_type_round_trips`, `pair_declares_a_product`, `pair_builds_a_pair`; `compatible_is_structural`
extended), 38 host, 7 wasm-native — / **12 wasm-target**, all green.

**Next.** `Sum`/`Option` — the one inch in this fork that adds a genuinely new **value kind** (a runtime
tag): it touches `value.rs` and the memo value-encoding, unlike `pair`. Then the editor inch (recursive
`WType` + type-as-colour + boundary-input fields).

---

## 2026-07-01 · P1.3.2 inch 3 — `Sum` (`Option`) + a real host bug it caught · *(the value-constructor half of the fork is complete)*

**Built (core).** `Value` gains a genuinely new shape — `Sum { arm: usize, payload: Box<Value> }`.
Unlike `Product` (rides `Value::List`, every field always present), a sum value carries only its
**taken** arm; the untaken alternatives are erased — that's what a tagged union *is*. First P1.3.2
generator that couldn't be a reuse of an existing value kind. `Type::Sum(Vec<Type>)` (tag 8) shares
`Product`'s count-prefixed recursive encode/decode exactly; `compatible` gets the matching arm — and a
`Sum` is never compatible with a same-shaped `Product` (a tagged choice and a tuple are different
things, not just different tags). `ENCODING_VERSION → 4`, free again. `memo.rs` gets `TAG_SUM`, with
the arm index mixed into the encoding specifically so the same payload at a different arm never
collides — proven directly (`sum_arm_changes_the_key`), the property the cache actually depends on.

**`some`/`none` join genesis (35 → 37).** `Option<T> = Sum([T, Unit])`; `some` is always arm 0, `none`
always arm 1 — hard-coded by which primitive you call, not a generic `inject(tag, value)` (`compute()`
has no way to know a declared arity; deferred until something other than `Option` asks for it).

**`Value::ty()`'s `Sum` arm is an honest coarsening, not a sound one.** `Sum(vec![Any, Any])` is true
*today* only because `some`/`none` are the only constructors that exist — arity 2 is the only arity
reachable. Documented with an explicit trigger for when a generic injector would break this, rather
than engineered around now. `Value::ty()` has zero callers outside its own tests — latent, not live.

**The wasm bridge needed two different decisions, not one copied pattern.** `WValue` was already a
**lossless** mirror of `Value` (adjacently-tagged via serde), so `Sum` crossed for free, shaped like
`Effect`. `WType` is genuinely flat, and `Product`'s precedent — map to `WType::List`, sound because a
product value really is list-shaped — does **not** carry over: a `Sum` value is `WValue::Sum`, not
list-shaped, so that mapping would have lied to the editor about what it's about to receive. `WType`
gained a real new bare-tag variant, `WType::Sum`, reconstructing via `into_core` the same way `List`
does — the one concrete shape that exists, not a guess at some other arity.

**Caught along the way, not invented: a real latent bug in `holon-host`.** `constant_unit` (which
reconstructs a held state value as a literal, for the `store_read` boundary) turned out to have a gap
this inch's type checker exposed. `store_read`/`store_write` are `Any`-typed, so a `Sum` can genuinely
reach a state cell — not hypothetical. The only niladic `Sum` constructor is `none()`, so silently
defaulting to it for an arbitrary stored `Sum` would have **silently flipped a held `Some` into a
`None`** — a wrong-arm bug, not a lossy one, exactly what the codebase's own `Effect`-arm comment
already refuses ("a bug to fix, not a value to fabricate"). Fixed by making `constant_unit` fallible
and routing the failure through the **same seam** an unsupplied capability already uses ("evaluates to
nothing") — no new control invented, reuse of the one already there. Proven end to end
(`sum_in_state_fails_honestly_not_fabricated`): no panic, no fabricated `none()`, just an honest empty
read. (Worth knowing, not fixed here: `List`'s existing state handling has the same root limitation —
a non-empty stored list silently degrades to `[]` — lower-stakes than a flipped arm, already flagged as
deferred, out of scope this inch.)

**Ripple.** `simulate.rs`'s `sourced_mock` gained the same fail-closed arm `Product` got —
`Type::Sum(_) => None` — no sourced leaf outputs a `Sum` today, unreachable in practice.

**Tests prove.** **176 core** (+5 since inch 2: `sum_type_round_trips`,
`some_and_none_declare_an_option`, `some_and_none_tag_their_arm`, `sum_arm_changes_the_key`,
`sum_arm_is_part_of_identity`) / **39 host** (+1: `sum_in_state_fails_honestly_not_fabricated`) /
**7 wasm-native** / **12 wasm-target**, all green.

**The value-constructor half of P1.3.2 is complete.** `Dictionary<K,V> = List<Product([K,V])>` and
`Option<T> = Sum([T,Unit])` are both real, checkable, constructible, and now storable types.

**Next.** The editor inch — recursive `WType` (so `Product`/`Sum` stop bridging coarse) + the
boundary-input fields that started this whole detour. Scoped in `P1.3.2-parametric-types-L4.md` §8.

---

## 2026-07-01 · P1.3.2 inch 4a — the recursive `WType` bridge · *(precise on both sides, at last)*

**Built.** `WType` now mirrors `core::Type` exactly — `List`/`Product`/`Sum` carry their real element
type(s) across the bridge instead of flattening to a bare tag. `sum`/`join`'s input now crosses as
`List<Number>`/`List<Text>`, `pair`'s output as `Product<Any,Any>`, `Option` as `Sum<Any,Product<>>` —
precise on both sides for the first time since P1.3.2 started.

**Wire shape, no new serde attribute.** `WType`'s variants were all nullary before, so serde's own
default (external tagging) already rendered every scalar as a bare string. Adding data to
`List`/`Product`/`Sum` keeps every scalar exactly as it was and turns just those three into
single-key objects (`{"List": "Number"}`, `{"Product": [...]}`, `{"Sum": [...]}`) for free — no
Rust-side config. Considered matching `WValue`'s adjacent-tagging convention for consistency;
rejected it, since that would wrap every scalar in an object too, breaking every currently-working
scalar comparison across the editor for no real gain.

**`into_core` is depth-bounded now** — `MAX_TYPE_DEPTH`, made `pub` and reused verbatim from core's
own decoder, not duplicated. `compatible()`/`build_preimage()` are public `#[wasm_bindgen]`
functions reachable by any JS on the page, and `WType` can now nest, so this became a genuine
untrusted-input boundary the moment it stopped being flat — the same class of concern that
justified the decoder's own bound at inch 1.

**Two real, pre-existing latent bugs surfaced while grounding, neither introduced this inch.**
`holon.ts`'s `WType` union had silently drifted — it never gained `"Sum"` when inch 3 added the
bare tag, so `some`/`none`'s output type was already unsound TypeScript and rendered with an
`undefined` (invisible) swatch. And `PrimitiveNode.tsx`'s `CONST_TYPES`/`setConstType` were typed
as the full `WType` when their true domain was always just the three literal scalar forms — it
only ever type-checked because the old flat `WType` happened to be a subtype of `string`; recursive
`WType` correctly ended that coincidence. Both fixed at the root, not patched with a cast.

**`theme.ts` gained `typeKind`/`typeColor`/`typeLabel`** — the one place the editor walks a composed
type to draw it (hue, bracketed label), never to judge it; `compatible()` stays the only
wire-legality decision, asked of core every time. `Product`/`Sum` earned real hues (rose, blue)
instead of an invisible or miscolored fallback.

**Also, while verifying in-browser:** `deleteKeyCode` extended to `["Backspace", "Delete"]`
(`App.tsx`) — React Flow already deletes a selected node or edge and cleans up any wires attached
to it by default, on Backspace; `Delete` is the key most people reach for first and wasn't wired.
One line, nothing hand-rolled. Confirmed live: editing a `const`'s text field with Backspace stays
scoped to the input and does not delete the node.

**Compile-fix loop, recorded honestly.** Guessed once that marking `MAX_TYPE_DEPTH` `pub` inside
`hash.rs` would make it reachable as `holon_core::MAX_TYPE_DEPTH`, based on how `compatible()` was
already used elsewhere — without reading `holon-core`'s `lib.rs`. Wrong: `hash` is a private
module; everything it exposes crosses through one curated, item-by-item `pub use` list at the crate
root, and the constant simply hadn't been added to it. Fixed once the real file was read, not
guessed again. Separately, `JsError::new` can't be constructed outside the wasm32 target (a
wasm-bindgen FFI stub with no native implementation), so the one test that deliberately drives
`into_core`'s failure path moved to wasm-only — the property it proves is unchanged, only where
`JsError` itself can exist. Every other loop (`tsc`'s module-resolution error, `wasm-pack`'s
missing-`Cargo.toml` error, `cd editor` from inside `editor/`) turned out to be a wrong shell or
wrong directory, not a code defect — confirmed, not just asserted, by `npx tsc -b` returning clean
in both PowerShell and WSL once pointed at the right place.

**Tests prove.** **176 core / 39 host / 8 wasm-native / 14 wasm-target**, all green. `tsc -b` clean.
**Browser-confirmed:** 37 genesis atoms; `pair` renders rose with a `product<any,any>` label;
`some`/`none` render blue (previously invisible) with `sum<any,product<>>`; `sum`/`join`'s list
inputs show `list<number>`/`list<text>` on hover; wiring, deletion, and connected-edge cleanup all
behave correctly live.

**P1.3.2's value-constructor work (inches 1–3) plus the editor's recursive `WType` (4a) are both
complete.** Only boundary-input fields (4b) remain — the capability that started this whole
detour, now able to render precise composed types instead of flat ones.

---

## 2026-07-01 · P1.3.2 inch 4b — boundary-input fields · *(P1.3.2 complete)*

**Built.** An open composite's promoted inputs can now be filled and evaluated through its
interface — the capability that started the whole parametric-types detour, landing on top of the
precise composed types 4a made possible. `App.tsx` only; no Rust or wasm change.

**Mechanism.** Core's `eval` already accepts `WValue | null` per input slot (`null` = absent) —
confirmed directly in the wasm bridge source before writing any TS, not assumed. Scalar promoted
inputs (`Number`/`Boolean`/`Text`) get a real, type-native control — a number input, a checkbox, a
text input — not a generic text field needing string parsing. Deliberately different from `const`'s
own value editing: `const`'s value is baked into a frozen, hashed `Unit`, so core has to parse it
from a string at eval time; a boundary input is never hashed — a pure, ephemeral eval-time argument
— so routing it through core's parser would be pointless indirection. The browser's own type
coercion is unambiguous and sufficient. Non-scalar inputs get an honest, disabled "wire in"
placeholder, matching the exact precedent `const`'s own three-literal-form boundary already set.

**Zero-value defaults, reused not invented.** Fields start pre-filled with each type's zero value
(`0`/`false`/`""`) — the same convention already established twice elsewhere (`const`'s own
default, core's `sourced_mock`). A freshly-projected open composite is immediately evaluable, not
stuck on "supply to run." Filling a field re-evaluates live — safe to be reactive, unlike
projecting itself, because a boundary value is never part of the composite's hash. The old
closed/open branch in the projection panel is gone; a closed composite is just the zero-input case
of one unified path now.

**A real, non-obvious system fact surfaced and documented, not special-cased.** A `null` input slot
does not mean "eval blocks." Core's `eval` drops absent slots *before* `compute` ever runs
(`inputs.iter().flatten()` — "compute's arity check is the firing rule," per `eval.rs`'s own
comment), uniformly for every primitive. Whether that reads as "not ready" depends entirely on the
target primitive's own arity: `add` destructures exactly 2 values, so a dropped slot fails its own
arity match; `list` has no arity requirement at all, so it happily builds from however many values
are present — including zero, `[]`. Both correct. Discovered by checking the real `eval.rs` after a
browser screenshot showed `list` with both inputs unwired evaluating to `[]` instead of the "not
ready" that had been predicted — grounding the surprise instead of "fixing" it mattered here: the
design was right, the prediction was the only thing wrong.

**Two real gaps closed, one an honest miss.** `formatValue` never gained a `Sum` case when core
added `Value::Sum` — silent runtime `undefined`, not a compile error, since this project's
tsconfig doesn't enable `noImplicitReturns`. Fixed, with the same `never`-exhaustiveness guard
`theme.ts` already uses. Separately: `holon.ts` has two type definitions mirroring Rust enums that
both gained `Sum` at inch 3 — `WType` and `WValue`. Inch 4a fixed `WType`'s gap; `WValue`'s was
never checked, and adding `formatValue`'s `Sum` case assumed the type already supported it instead
of verifying the file — the same "reasoning from assumption" mistake the gap itself came from.
Fixed once the compiler caught it; swept every other file afterward to confirm `formatValue` was
the only exhaustive switch over `WValue.type` in the codebase.

**Browser-confirmed, not just compiled.** `add` with both inputs unwired shows two fields
pre-filled `0`/`0`, `= 0` immediately; editing one live-updates the result without re-clicking
project; a mixed graph (one input wired to a `const`, one left as a field) combines both correctly;
`list` with both non-scalar inputs unwired evaluates to `[]` exactly as `eval.rs`'s design
predicts; a lone `none()` renders `= sum#1([])` — live proof the `WValue` fix works, not just
compiles.

**Tests.** `tsc -b` clean (confirmed via `npx` in both PowerShell and WSL). No Rust/wasm rebuild
needed for this inch's mechanism.

**P1.3.2 is complete.** The composable type language (1), `Product`+`pair` (2), `Sum`+`some`/`none`
(3), the editor's recursive `WType` bridge (4a), and boundary-input fields (4b). `Dictionary<K,V>`,
`Option<T>`, and every composed type in between are real, checkable, constructible, storable, and
now runnable through the editor's own interface.

---

## 2026-07-01 · P4.1 — variadic-arity editing · *(the last piece of the authoring-canvas arc)*

**Built.** A placed `add`/`multiply`/`and`/`or`/`concat`/`list` node can grow past the catalog's
declared default of 2 ports (a `+`/`−` control in `PrimitiveNode.tsx`), so the editor can actually
build the N-ary graphs these leaves were always capable of running. This was the one item still
open from the whole authoring-canvas arc, deferred at param editing specifically so core could mark
*which* leaves this applies to before the editor guessed.

**The core question, checked before anything else: does `add`'s compute actually accept more than
two inputs, or does the catalog's "arity 2" just look fixed?** Read `primitives.rs` directly rather
than assume: `add`/`multiply`/`and`/`or` all call shared `fold_numbers`/`fold_bools` helpers over
`inputs: &[&Value]` with **no arity check at all** — already correct for any count, including zero
(the fold's identity element). `concat` loops the same way. `subtract`/`divide`/`compare` — sharing
`add`'s own catalog interface line — use `bin_numbers`, a strict `let [a, b] = inputs else return
None`, on purpose: subtraction and division aren't associative, so core gives "variadic subtract" no
meaning. This is exactly why the plan only ever named `add`/`and`/`concat`/`list`, never the whole
arithmetic group. **Net result: zero changes to any compute function** — the entire inch is telling
the editor which six names are safe to grow.

**The 4th catalog fact, not a hand-maintained TS list.** `catalog::VARIADIC` + the wasm bridge's
`variadic_primitives()` — a name-keyed classification, exactly parallel to `capabilities_of`, not
baked into `Unit`'s own shape (a `Unit`'s declared `inputs` is free-form data; nothing in
`validate`/`hash_unit` checks it against some "canonical" arity). Deliberately *not* hand-listed in
TypeScript: that would be the exact "second decider that can silently drift" mistake made twice
already this session with `Sum` — if core's classification of a leaf ever changed, a hand-maintained
list wouldn't know.

**Growing copies the last port's type** (every variadic leaf is uniform-typed); naming continues the
catalog's own `a, b, c, …` convention. Shrinking never goes below the catalog's own declared default
(derived per-node at drop time, not a hardcoded floor) and drops any wire that fed the removed port —
the same fail-closed cleanup `setConstType` already does when a type change orphans a wire.

**Tests prove the classification, not just the plumbing:** `variadic_leaves_actually_tolerate_more_
than_two_inputs` calls `compute` directly with 3 inputs for every variadic name (not trusting the
label) and confirms `subtract` correctly refuses. `variadic_names_are_genesis_leaves` guards against
a typo silently offering to grow a primitive that doesn't exist.

**One real miss, caught by the compiler, not skipped past.** Added `is_variadic(name) -> bool` as a
natural-seeming companion to `VARIADIC` — then found nothing anywhere actually called it (the bridge
iterates `VARIADIC` directly; only my own test used the wrapper). Removed it rather than force a use
into existence just to justify keeping it — a small, honest example of "build the least the
architecture needs" catching its own author.

**Tests:** 178 core (+2: `variadic_names_are_genesis_leaves`,
`variadic_leaves_actually_tolerate_more_than_two_inputs`) / 39 host / 9 wasm-native (+1:
`variadic_primitives_native`) / 16 wasm-target (+1: `variadic_primitives_wasm`), all green.
Browser-confirmed: `+`/`−` render on `add` and not on `subtract`; growing `add` to three inputs,
wiring all three, and projecting yields their actual sum; shrinking re-disables `−` exactly at 2.

**The authoring-canvas arc (P4.1) is now genuinely foundation-complete** — place · wire · project ·
configure · run, boundary-input fields, and variadic-arity editing, all landed and browser-proven.

---

## 2026-07-01 · P4.1 — error-surfacing: structured errors, highlighted nodes · *(scoped as its own inch, deliberately not folded into multi-output)*

**Context.** Raised while reflecting on native multi-output support: instead of
`AmbiguousDefaultOutput` as a flat error banner, could the editor show errors *on* the units and
highlight them? Scoped and built as its own inch, separate from multi-output itself, since nearly
every error it improves already exists today and has nothing to do with output count.

**The real finding, before writing anything:** `AmbiguousDefaultOutput` was likely to *dissolve*
once native multi-output lands — "2 free outputs" stops being an error and becomes "expose both,"
the same way inputs already work with no ambiguity check at all. Building bespoke UI for that one
error specifically would have aimed at a problem about to disappear. But checking `validate.rs`'s
real `ValidationError` enum showed every variant except `Cycle` already carries a `child`/`edge`
index — the *entire* structural error surface, almost none of which is output-count-related. The
actual gap was much bigger and much older than the one error that prompted the question.

**The gap was the bridge, not core.** `project()`'s failure path collapsed the whole `EncapError`
through `{e:?}` (Rust's `Debug` format) into a single string before it ever reached JS — core's
`child`/`port` data was computed, real, and thrown away at the boundary. The exact same pattern as
the `_fp` discard in `eval.rs` found earlier this session: the right answer already existed, nothing
downstream was reading it.

**`AmbiguousDefaultOutput` enriched at the source.** `default_spec` already computes `free: Vec<u32>`
— the exact candidate list — checks its length, and then discarded the vector on the error path.
Changed to `AmbiguousDefaultOutput(Vec<u32>)`, carrying data that was already being computed; zero
new traversal. As a genuine side effect, "zero free outputs" and "two or more" are now distinguishable
for the first time — they were silently the same bare variant before (new test:
`no_free_output_is_distinguishable_from_ambiguous`). `Cycle` stays untouched, on purpose: tracing
*which* edges form a cycle is real new algorithmic work, not "stop discarding what's already there" —
named honestly as this inch's one real scope boundary, not silently skipped.

**`Display`/`Error` given to `ValidationError` and `EncapError` in core** — a genuine core-level
improvement (any consumer benefits, not just the bridge), and it means the bridge's `message` field
is `.to_string()`, not a second, separately-worded copy of what an error says. One decider for
wording, same principle as everywhere else.

**The bridge crosses structure now, not text.** `WValidationError`/`WEncapError` mirror types (the
same tagged-union convention already used for `WValue`/`WType`), each carrying the structured fields
*and* core's own message. `TypeMismatch`'s `src`/`dst` cross as real `WType`s (reusing the already-
built `WType::from_core`), so the editor renders them with its own `typeLabel` instead of Rust's
`Debug` output. `project()`'s error channel changed from `Result<JsValue, JsError>` to
`Result<JsValue, JsValue>` — `JsError` is inherently a flatten-to-text type, the wrong tool once
there's real structure to carry. Malformed *input* (a bad hex hash, edges that don't even
deserialize) stays a plain string on purpose — a different, shouldn't-happen failure mode, not part
of the structured family.

**Two real mistakes made and caught mid-inch, not smoothed over:**
- Edited a `Jun 30`-dated upload of `encapsulate.rs` without checking it was fully current against
  everything else fixed this session — it predated the `Type`-loses-`Copy` change from the very
  start of the parametric-types work, and handing it back overwrote a file that had been correctly
  compiling for days. Caught by the compiler immediately; fixed; the actual process change adopted
  going forward — diff or spot-check an uploaded file against known session-wide changes before
  treating it as ground truth, not just the one fragment relevant to the task at hand.
- Moved the new `WValidationError`/`WEncapError` types out of `impl Evaluator` (Rust doesn't allow
  defining enums inside an `impl` block) by anchoring the edit on `impl Evaluator {` itself — which
  landed the block *between* that line and the `#[wasm_bindgen]` attribute sitting directly above
  it. Since attributes attach to the next real item regardless of doc comments in between, this
  silently stole the attribute onto the new enum, breaking wasm-bindgen's macro expansion for the
  actual `impl Evaluator` block. Root-caused precisely (not patched around) by checking actual
  attribute-to-item adjacency — a check equal brace-counts can't provide, since they say nothing
  about whether an attribute paired with the right item.

**The editor resolves node identity from data it already has — no new core surface needed for
this.** `TypeMismatch` only carries an `edge` index; the editor resolves it back to real endpoints
using the *same* `wedges` array it already builds locally before calling `project`, the same
"local data, no re-derivation" principle used earlier for boundary-field provenance.
`AmbiguousDefaultOutput`'s candidates map directly to node indices — the fix this whole inch exists
for. Highlighting is purely a rendering decoration (`decoratedNodes` in `App.tsx`) that never
touches the real `nodes` state `onProject`/`onDrop` read and write — flagging a node as errored has
zero effect on hashing, the same cosmetic/structural line I5 already draws for position. Wrapped in
its own inner `try`/`catch`: if index-extraction ever hits an unexpected runtime shape (TypeScript's
types don't validate what actually crosses the JS boundary), the error message still displays
correctly — a nicety is never allowed to break the thing that actually matters.

**Tests:** 179 core (+1: `no_free_output_is_distinguishable_from_ambiguous`) / 39 host / 11
wasm-native (+2: `encap_error_structure_native`, `validation_error_structure_native`) / 17
wasm-target (+2: the wasm-target twins), all green. `tsc -b` clean. Browser-confirmed against a real
`AmbiguousDefaultOutput` case: the banner shows core's own wording verbatim ("2 children have a free
output ([1, 2]) — choose which to expose"), and exactly the two implicated nodes — not the consumed
one — show the coral outline.

**Deliberately out of scope, named rather than silently deferred:** native multi-output itself
(the reflection that started this) — Product-as-bundle vs. real multiple output ports is still an
open, undecided fork; `Cycle`'s path-tracing (needs new algorithmic work, not just exposing existing
data); and the split `default_spec`/`derive_and_freeze` bridge surface that would let the UI actually
*choose* which output to expose (P4.3's job, not this one's — this inch only makes the existing
combined `project()` call's failures legible, it doesn't add a way to resolve them interactively).

---

## 2026-07-01 · P4.7 — native multi-output, core through editor · *(leaf closed, sub-leaves a–e)*

**Context.** Raised during the P4.1 error-surfacing inch as a deliberately-deferred fork:
`AmbiguousDefaultOutput` was a symptom of composites having exactly one exposable output, no
matter how many free children a graph had. Confirmed with the user before building: multi-output
is exclusively a composite capability — `compute()`/`primitives.rs` never change, every genesis
primitive stays single-output forever. A composite's `outputs` becomes a `Vec<Port>`, symmetric
with `inputs`, and `eval` returns `Vec<Option<Value>>` — one slot per declared output, each
independently present or absent, not `Option<Vec<Value>>` (an unresolved child fails one slot,
not the whole result).

**Built.**
- **(a) Foundation** — `hash.rs`: `Unit.output: Option<Type>` → `Unit.outputs: Vec<Port>`, the
  same count-prefixed per-port encoding already used for `inputs`; `ENCODING_VERSION` 4→5.
  `eval.rs`: `eval` returns `Vec<Option<Value>>`; new `eval_one` convenience wrapper fails closed
  (returns `None`) if a unit doesn't declare exactly one output — `map`/`fold`'s function argument
  must be single-output, so they use it. `eval_composite`'s scratch became
  `Vec<Vec<Option<Value>>>` (per child, holding all of that child's outputs); the boundary-output
  gather loop walks `0..n_outputs` instead of assuming one slot.
- **(b) Ripple, minimal-fix scope** — `validate.rs` and `encapsulate.rs`'s output-port checks
  changed from a hardcoded `(0, Some(t))` match to a natural `.get(port)` lookup against the new
  `Vec`. Deliberately *not* the full generalization: `EncapSpec.output: (u32, u32)` — the
  author's choice of *which single* output to expose — is untouched; letting an author expose
  *several* outputs at once (`EncapSpec.expose: Vec<Expose>`, `default_spec` proposing all free
  outputs) is its own later, separately-gated pass, not bundled in here.
- **(c) Local + shared memoization** — `memo.rs`'s `CacheEntry.output: Value` →
  `outputs: Vec<Option<Value>>`, mirroring `eval`'s own shape; `verify_entry` compares the whole
  slot vector canonically (order + presence matter, reusing the same `SLOT_ABSENT`/`SLOT_PRESENT`
  encoding `memo_key` already had for inputs, factored into a shared `encode_slots` helper).
  `holon-host`'s `Memo` and `SharedCache::get_verified` got the identical treatment — both
  genuinely widened to `Vec<Option<Value>>`, not narrowed, because both make an explicit
  "behaviourally identical to `eval`" promise that a multi-output-blind narrowing would break.
- **(d) Host firing loop + wasm bridge** — `holon-host::fire` and the wasm bridge's
  `Evaluator::eval` went the *other* way: both use the new `eval_one` internally but keep their
  own existing `Option<Value>` / single-`WValue` public contracts unchanged. Both are genuinely
  about a *service* graph settling to one request the host or editor inspects — not the
  multi-output data-composite case this leaf exists for — and neither can currently be handed a
  real multi-output unit anyway (nothing yet lets the editor build one). `WUnit.output` →
  `WUnit.outputs: Vec<WPort>`, same conversion pattern already used for `inputs`.
- **(e) Editor rendering + wiring** — `holon.ts`'s `WUnit.output?: WType | null` →
  `outputs?: WPort[]`. `PrimitiveNode.tsx` renders one `<Handle>` per entry in `outputs`, keyed
  by the port's own name, the same `.map()` already used for `inputs` — not a hardcoded single
  `"out"` handle. `App.tsx`: the one line that actually mattered was `onProject`'s edge-building
  loop, which hardcoded `from_port: 0` with a comment reading "single output, the 'out' handle" —
  now resolves `from_port` from the wire's `sourceHandle`, exactly the way `to_port` already
  resolves from `targetHandle`. `isValidConnection`/`onConnect` resolve the source port the same
  way, not just "the" output. `Palette.tsx`'s swatch reads `outputs[0]` (safe: a catalog entry is
  always a genesis primitive, permanently single-output, so there's no ambiguity about which one
  to preview). `theme.ts` and `styles.css` needed nothing — `outputColor` already took a bare
  `WType`, never the renamed container, and the output column's CSS was already a generic
  flex-column-with-gap, the same layout the input side already uses for variadic ports.

**Leaf closed.** All five sub-leaves, (a) through (e).

**Tests prove** (+4 net: 179→183 core, 39 host unchanged, 11 wasm-native unchanged):
`eval_composite_multi_output` (three independent outputs off the same two inputs);
`multi_output_partial_success` (one output succeeds, a sibling divide-by-zero fails,
independently); `eval_one_requires_exactly_one_output` (fails closed, doesn't silently pick
one); `multi_output_entry_verifies_every_slot_independently` (a cache entry with a partial
Some/None claim verifies honestly, and a lie on *either* slot — not just the first — is caught).
Full workspace: `cargo test --workspace` — 233/233 (183 core + 39 host + 11 wasm-native), zero
failures. Editor sub-leaf (e) is TypeScript-only, no Rust ripple — same 233 stand unchanged;
verify with `tsc -b` once `Palette.tsx`'s sibling files are in place.

**Forks pinned.**
- Multi-output is composite-only, permanently — confirmed with the user, not assumed.
- `eval` returns `Vec<Option<Value>>`, never `Option<Vec<Value>>` — per-output independent
  failure, matching how a real multi-output composite (e.g. MACD) actually fails: one line can
  go stale while its sibling is still computable.
- `eval_one` fails closed on a non-single-output unit rather than silently taking the first slot
  — a caller that needs exactly one value should say so and be refused if that's not what it got.
- `AmbiguousDefaultOutput` is expected to *dissolve* once `default_spec` is generalized —
  "2 free outputs" stops being an error and becomes "expose both." Not touched here; still gated
  on the deferred sub-leaf (b) follow-on below.

**Named, not silently deferred — two open items, neither built here.**

1. **The benchmark finding.** `cargo bench -p holon-core` after (a)–(d) showed broad
   regressions, including on operations with nothing to do with multi-output:
   `native_head`/`native_length`/`native_is_empty` (trivial, single-output list primitives)
   regressed ~45–50% *flat across every list size tested* — a fixed per-call cost, not a
   size-dependent one. `wrapping_tax` confirms it: `native_add` (bare primitive call) regressed
   33%; `composite_add` (already paying hashmap lookups and edge traversal) regressed only 7% —
   the same fixed cost is a larger fraction of a small baseline than a large one.

   Root cause, found by reading the code, not guessing: `eval_one` calls `eval` — which now
   always heap-allocates a `Vec<Option<Value>>`, even for a single-output primitive that used to
   return a stack-only `Option<Value>` with no allocation — then pops one element and discards
   the rest. `eval_map`/`eval_fold` call `eval_one` once *per list element*, so a 4096-element
   `fold_sum` now performs 4096 allocate-then-discard `Vec`s that never existed before.

   Standard fix: a small-vector type (`SmallVec<[Option<Value>; 1]>` in place of
   `Vec<Option<Value>>`) — inline storage for the common 0–1-output case, spilling to the heap
   only when a unit genuinely has more than one output. Deliberately not built: it touches
   `eval`'s core return type, rippling through every file this leaf already touched, plus adds a
   new dependency to a crate that currently has none. Absolute numbers stay tiny throughout
   (58 ns for a bare primitive call, 273 µs for a 4096-element fold) — nothing here is
   user-perceptibly slow. Deferred pending an actual multi-output composite existing to measure
   against, following the same "surface it, name it, don't silently drop it" discipline the
   `eval_composite` scratch-clone deferral already set as precedent.

2. **`EncapSpec.expose: Vec<Expose>`.** Sub-leaf (b)'s full scope — letting an author choose
   *several* outputs to expose at once, generalizing today's single-choice `EncapSpec.output:
   (u32, u32)` — was deliberately not built in this leaf. Consequence, stated plainly: the editor
   (sub-leaf e) now correctly *renders* and *wires* however many outputs a node declares, but
   nothing will ever show more than one *yet* — `derive_and_freeze` still only ever produces a
   single-output composite, so there is currently no path, anywhere in the editor's own
   drag-wire-project workflow, that can construct a unit with more than one output. Sub-leaf (e)
   is forward-ready plumbing, not a reachable feature, until this lands.

**Next inch.** Neither of the two items above is scheduled yet — both are named forks awaiting a
decision, not a queued next step. Whatever comes next on the overall plan is independent of P4.7,
which is now fully closed.

---

## 2026-07-01 · P4.3 — output exposure: from automatic-and-fixed to automatic-and-editable · *(forks 1-3, browser-confirmed; the leaf's actual scope — subgraph selection/grouping — still untouched)*

**Context.** P4.7's own deferred-forks list named this directly: `EncapSpec.expose: Vec<Expose>`
— letting an author choose several outputs, generalizing the single-choice `EncapSpec.output` —
was explicitly not built there. Separately, `P4.3.1`'s own leaf definition already anticipated
the bridge-split half of this work almost exactly: "expose `default_spec` + `derive_and_freeze`
as two calls, so the UI can... choose the exposed output." Three forks, scoped and confirmed one
at a time rather than built together.

**Built.**
- **Fork 1 (core capability).** `Expose { child, port, name }` — structurally identical to
  `Promote`, kept as its own type so swapping `promote`/`expose` at a call site is a compile
  error, not a silent bug. `EncapSpec.output: (u32, u32)` → `expose: Vec<Expose>`.
  `derive_and_freeze` generalized to loop over `spec.expose` symmetrically with the existing
  `spec.promote` loop — N output ports, N boundary edges, `to_port = i`. `default_spec` stays
  behaviorally unchanged (still exactly one required free output) — only its internal
  construction changed mechanically.
- **Fork 2 (`default_spec` generalizes).** `default_spec`'s output-side scan changed from
  per-*child* tracking (`consumed: BTreeSet<u32>`, hardcoded port 0) to per-`(child, port)`
  tracking, symmetric with the promote loop — propose every free output automatically, zero
  ambiguity check, matching how the input side already promotes every unbound input. This
  wasn't a separate patch: the correct symmetric loop simply has no room for the old bug, which
  would have mattered the moment a multi-output child (fork 1's own capability) got fed back in.
  `EncapError::AmbiguousDefaultOutput` removed entirely as dead code, not left inert — no case
  produces it anymore. Consequence that isn't optional: the wasm bridge's `Evaluator::eval()`
  had used `eval_one` (narrowed to a single value) on the reasoning that the editor couldn't yet
  build multi-output composites — fork 2 removes that precondition immediately (any two
  unconsumed primitives now project to two outputs automatically), so `eval()` was widened to
  cross the genuine `Vec<Option<WValue>>` array. Shipping the `default_spec` change alone would
  have been a regression, not a gap: the old `AmbiguousDefaultOutput` gave a clear, structured,
  node-highlighting error for this exact case; without the `eval()` fix it would have silently
  shown "not ready," indistinguishable from a broken graph. `App.tsx`'s results panel updated to
  the plural shape, labeled via `boundaryLabels` — reused directly, not reimplemented, since it
  turned out to already be fully generic (provenance + occurrence-counting, never actually
  input-specific).
- **Fork 3 (the editable seam).** Two new `Evaluator` methods, named after the exact core
  functions they wrap: `propose_encapsulation` (wraps `default_spec` alone) and
  `freeze_encapsulation` (wraps `derive_and_freeze` alone). `project()` itself untouched — still
  the single-call combined path for anyone who never intervenes. New `WPromote`/`WExpose`/
  `WEncapSpec` bridge types mirroring their core counterparts. `App.tsx`: `onProject` now calls
  propose then freeze explicitly, storing `children`/`wedges`/`spec` so exclusion can re-freeze
  without re-deriving from the (possibly since-changed) canvas. New `excludeOutput` handler — a
  genuine re-freeze (new spec → new hash → new preimage → re-eval), not a display filter, because
  unlike a boundary value, which output is exposed changes what the composite's interface
  actually declares. Deliberately narrow: exclude only, no rename, no reorder, no re-include
  without re-projecting (already a single click) — matching the promoted-input side's own
  current lack of rename/reorder, so outputs don't get more editing power than inputs have.

**Leaf closed.** Forks 1-3 of the output-exposure work only. **`P4.3.1`'s actual scope — select a
*subgraph* (today `project` always takes the whole canvas, never a partial selection), group it
into a placed, collapsed node, expand a placed composite back to inspect its children — remains
completely unbuilt.** The bridge split this arc delivered was a prerequisite `P4.3.1`'s own notes
already called out, earned early — not the leaf itself. Worth naming plainly rather than let
"P4.3" quietly stand in for the bigger, separate feature.

**Tests prove** (183→185 core: net +1 from fork 1's `multi_output_composite_exposes_several_
independently`, then -2 dead scenarios removed / +3 new for fork 2's generalization; 39 host
unchanged; 11 wasm-native unchanged; 17→18 wasm, fork 3's own test): `default_spec_exposes_
every_free_output` and `default_spec_with_no_free_output_exposes_nothing` prove the ambiguity
check is genuinely gone in both directions, not just the "two free" case. `default_spec_tracks_
free_outputs_per_port_not_per_child` proves the exact bug the symmetric generalization fixes: a
child with one consumed and one free output still surfaces the free one. `propose_and_freeze_
encapsulation_crosses_js_boundary` (wasm-only) proves the full seam end to end — propose two
free outputs, edit the proposal down to one, freeze it, confirm one real output that still
evaluates correctly, and confirm `project()` called directly still produces the full two-output
composite, unaffected. **Browser-confirmed directly, not just test-confirmed:** two independent
`add`/`select` nodes project to two results under one hash; excluding one drops to a single
result under a genuinely different hash — the re-freeze, not a filter.

**Forks pinned.**
- Multi-output exposure is opt-out (exclude from an automatic maximal proposal), not opt-in —
  matching how input promotion already works, not a new interaction model.
- Excluding an output is identity-affecting (a real re-freeze), never a client-side display
  filter over an already-frozen result.
- `project()` stays the single-call default path permanently, unless deliberately changed later
  — the split methods are an addition beside it, not a replacement.

**Named, not silently found — three real gaps caught by grounding rather than assumed fixed:**
1. `#[cfg(all(test, target_arch = "wasm32"))]` gates the entire `wasm_tests` module (plus three
   hand-written `#[wasm_bindgen_test]` functions) out of every `cargo test --workspace` run —
   meaning that whole class of tests had never actually been compiled this session until
   `wasm-pack test` was finally run successfully from the repo root. Three genuine type
   mismatches from the original P4.7 pass were found and fixed there, previously undetected.
2. `holon.ts`'s `WEncapError` type and `App.tsx`'s `encapErrorNodeIndices` still referenced
   `OutputChildOutOfRange`/`OutputPortInvalid`/`AmbiguousDefaultOutput` — variants renamed or
   removed on the Rust side across the two prior inches, never updated on the TypeScript side.
   A real `switch` with real (now-stale) case labels, not just an unused type.
3. `Expose` (and, separately, `EncapSpec`'s own re-export) had never been added to
   `holon-core`'s curated `pub use` list in `lib.rs` — the same class of gap as `eval_one`
   earlier in the P4.7 arc, caught this time before it reached the wasm bridge instead of after.

**Deferred, named:** reorder/rename of promoted inputs or exposed outputs; re-including an
excluded output without re-projecting; and — the largest, most honest one — subgraph selection,
grouping a selection into a placed node, and expand-to-inspect, which is `P4.3.1`'s actual scope
and hasn't been started.

**Next inch.** Whatever's next is independent of this arc, which is closed at forks 1-3. The
`SmallVec` perf item from P4.7 remains separately deferred, pending an actual multi-output
composite existing in the wild to benchmark against — which, as of this inch, one now does.

---

## 2026-07-01 · P4.3.1 — subgraph selection + grouping into a placed node · *(the leaf's own remaining scope; expand-to-inspect is the one piece still open)*

**Context.** The prior P4.3 entry named this explicitly as "still entirely unbuilt, and this is
the leaf's actual scope" — everything the bridge split and the exclude seam (forks 1-3) were
prerequisites *for*, not instances of. Two P4 leaves were checked against their own logical
order first (not just the plan's suggested sequence): P4.4.1's "descend a composite to its
leaves" and P4.5.1's "publish a unit" both genuinely need a placed composite node to exist
before they mean anything — this was the actual next leaf, not just the listed one.

**Built.**
- **Selection needed zero new code.** React Flow's own multi-select (shift-click, drag-box) was
  already live and already driving delete (`nodes[i].selected`, already flowing through
  existing state) — this is its first use for anything else.
- **The load-bearing finding: `propose_encapsulation`/`freeze_encapsulation` needed no changes
  at all.** Feeding them only the selected nodes as children and only the edges where *both*
  endpoints are selected as inner wiring already derives the correct boundary — an edge with
  exactly one endpoint outside the selection is, from the selected subgraph's own perspective,
  indistinguishable from an ordinary free input or output. The same derivation that already
  handles the whole canvas handles an arbitrary subset for free. This whole leaf turned out to
  be an editor-side problem, not a new backend capability.
- **`unitFor`** — a shared helper resolving any placed node's own hash: rebuild fresh from its
  catalog interface for a primitive (I3 — the same primitive always yields the same hash, so
  "rebuild" and "reference" were always the same operation), or read the hash directly for an
  already-frozen composite. Used by both `onProject` and the new `onGroup`, replacing what had
  been duplicated inline in `onProject` alone.
- **`dedupePortNames`** — disambiguates colliding default-derived port names *before* they
  become a new composite node's actual handle ids, not just its display labels the way
  `boundaryLabels` already handles for the results panel. This matters specifically here:
  wiring resolves handles by name (`findIndex`), so a genuine collision on a *new* node's own
  interface — two different selected `add` nodes both proposing a `b` — would silently miswire
  the second port to the first's handle, a real bug, not just an ambiguous-looking label.
- **`onGroup`** — builds the sub-composite from the selection, freezes it via the same
  `ev.project` `onProject` already uses, then rewires every edge that crossed the selection
  boundary to the new node's correct handle. The rewiring target is resolved by reading the
  frozen composite's own boundary edges — the identical "don't guess, read what core already
  computed" discipline `source`/`resultSource` already follow in `onProject` — not by any new
  inference. Edges fully inside the selection are dropped (now baked into the composite's hash,
  no longer a canvas wire); edges untouched by the selection are left exactly as they were.
- **`PlacedBody`** — a new discriminated field on `PrimitiveData`: `{ kind: "Primitive" }` or
  `{ kind: "Composite"; hash }`. A genuine structural difference, not cosmetic — a primitive is
  rebuilt fresh every project, a composite is frozen once and referenced forever after, the
  same relationship core's own composites already have with their children. Confirmed, not
  assumed: `PrimitiveNode.tsx`'s existing render logic needed zero changes beyond the type
  itself — `setConstType` only ever fires for a `params` entry named "type" that composites
  never have (`params: []`), and the variadic +/− buttons only render when `variadic` is true,
  which composites never are.
- A "group" button beside "project," disabled below two selected nodes — grouping a single node
  would be pure overhead, wrapping one primitive in a composite for no reason.

**Leaf status.** P4.3.1's actual scope — select a subgraph, group it into a placed node — is
now browser-confirmed done. **Expand-to-inspect is the one piece still open**, its own
separate, later inch once grouping itself was proven, the same staged discipline the
encapsulation work (types → `derive_and_freeze` → `default_spec`, each its own gated step)
already used throughout.

**Tests prove — honestly, this time meaning manual, not automated.** There is still no
frontend test infrastructure (confirmed when the question came up directly, mid-session) — this
whole inch is verified by browser confirmation only, four scenarios: (1) two fully-unwired,
identically-shaped `add` nodes grouped together, proving `dedupePortNames` — the result shows
`a`, `b`, `a_2`, `b_2`, not a silent collision; (2) a partially-wired pair grouped, proving the
internal wire disappears while the still-free ports become the new node's own handles; (3) an
*external* wire connected to a to-be-grouped node's port *before* grouping, confirmed landing on
the correct handle of the new node afterward — the one scenario that actually proves the
boundary-rewiring logic, not just that grouping produces something that renders; (4) `project`
run with a grouped node present, confirming `unitFor`'s reference path end to end. All four
passed.

**Forks pinned.**
- Grouping uses `default_spec`'s full automatic proposal directly — no exclude-review step
  extended to it. That stays a whole-canvas-projection feature (fork 3); grouping doesn't
  inherit it just because the underlying core calls are shared.
- No rename or custom naming for a grouped node — labeled by its own hash prefix, matching how
  nothing in this arc lets a person rename a promoted input or exposed output either.
- Port-name collisions are resolved by suffixing at construction time (the actual handle id),
  never left as a display-only ambiguity the way `boundaryLabels`' occurrence-counting handles
  the results panel.

**Named, not silently accepted — the accumulating gap.** This is the second consecutive
substantial editor inch (fork 3's exclude UI, now this) verified by browser confirmation alone,
with no automated coverage backing either one. That's a real, growing risk as the editor's own
surface area increases, not a one-time cost — worth treating as its own inch before too much
more gets built on top of it un-tested, not an indefinitely-deferred nicety.

**Deferred.** Expand-to-inspect — taking a placed composite node and reconstructing its
children back onto the canvas, the dual of grouping. Needs `decode_preimage`/`load_unit`-style
access to a composite's own internals and edge-reconstruction logic; deliberately not sketched
here, its own scoping pass when it's actually next.

**Next inch.** Whatever's next is independent of this one. The two most directly-related
candidates: expand-to-inspect (closes P4.3.1 completely), or P4.4.1 (transitive inspection UI),
whose own precondition — a placed composite node existing at all — this inch just satisfied.

---

## 2026-07-01 · P4.3.1 — expand-to-inspect, the leaf's last piece · *(P4.3.1 is now fully closed: select, group, and expand all browser-confirmed)*

**Context — a course-correction worth recording plainly.** The original plan was to wire the
bridge's existing `load_unit` (preimage → `WUnit`) into the editor as expand's primitive.
Tracing the actual data flow first caught that this was wrong: a grouped composite's preimage
is computed once inside `onGroup` and discarded immediately after `ev.insert()`; only its hash
survives, on the placed node itself. `load_unit` needs bytes nothing in the editor still has by
the time anyone clicks "expand." What was actually needed was simpler, not more complex —
`Evaluator.resolver` already implements `Resolver::resolve(&hash) -> Option<&Unit>` directly,
so the composite is already sitting in memory, reachable by the hash alone.

**Built — `get_unit` (the bridge primitive, its own step).**
- New `Evaluator.get_unit(hash: &str) -> Result<JsValue, JsError>`: parses the hex hash, calls
  `self.resolver.resolve(&h)` directly, converts to `WUnit`. No preimage anywhere in the call.
  `Resolver` added to the bridge's import list — it wasn't in scope before, needed just to call
  `.resolve()`.
- `holon.ts` needed zero changes to make this callable — `get_unit` is a method on the already-
  exported `Evaluator` class (unlike `load_unit`, a standalone function, which *would* have
  needed an explicit wrapper). `WUnit` was already fully defined. The entire scope of "wiring
  get_unit" turned out to live on the Rust side alone.
- New wasm-only test, `get_unit_crosses_js_boundary`: round-trips both a primitive and a
  composite hash correctly, and fails as a plain string (no structured error family a "not
  found" belongs to) for a hash never inserted in this session. `cargo test --workspace`:
  185/39/11 unchanged. `wasm-pack test`: 18 → 19 (+1, this test).

**Built — `onExpand` (the editor half, the actual dual of `onGroup`).**
- Reads a composite node's own `body.hash`, calls `get_unit`, pulls `body.children`/`body.edges`
  straight from the result — the same structure `derive_and_freeze` produced at grouping time.
- Reconstructs each child fresh: a primitive regains full editability, checked against the same
  `variadicPrimitives()` set `onDrop` already uses, with its arity floor read from the *catalog's*
  declared default (`byName`), not the child's own current — possibly grown-then-frozen — input
  count. A nested composite child (the case where a previously-grouped node got grouped again)
  stays a placed, collapsed reference — expand goes **one level only**, deliberately; deep,
  recursive, annotated descent is `P4.4.1`'s separate job, not this leaf's.
- Rewires every canvas edge that pointed at the old composite node to the correct new child's
  handle, resolved by reading the composite's own boundary edges — the identical "don't guess,
  read what core already computed" discipline `onGroup`'s own rewiring already used, walked in
  the opposite direction.
- Rebuilds the composite's internal wiring as real new canvas edges, through `addEdge` (the same
  utility `onConnect` already uses) rather than bare objects — so reconstructed wires get real
  ids and the same type-colored styling every other wire on the canvas has.
- No renaming or dedup needed here the way grouping needed `boundaryLabels`, and it's worth being
  precise about why not, not just asserting it: grouping *combines* several children's ports into
  one new interface, where collisions are possible; expanding *splits* one already-frozen
  interface whose names were already unique when it was built. Nothing to disambiguate.
- New "expand" button beside "group," enabled only for exactly one selected composite node —
  mirrors "group"'s own count-based guard rather than inventing a new per-node interaction
  `PrimitiveNode.tsx` doesn't otherwise have.

**Leaf status: `P4.3.1` is fully closed.** Select (free, React Flow's own), group, and expand
are all built and browser-confirmed. Two boundaries worth stating plainly, not silently:
expand's one-level restriction (a scope decision, not a gap — `P4.4.1` owns deep descent), and
that this is a **structural** round-trip, not a positional one — original node ids and canvas
positions were never kept anywhere, so expanding a group gives back the same children and wiring,
freshly laid out, not the exact prior canvas state. "Choose the exposed output" is still
exclude-only, not full reorder/rename — a real, smaller, separately-named limit from fork 3, not
part of what this leaf itself ever promised.

**Tests prove — still honestly manual, not automated.** No frontend test infrastructure exists.
Four scenarios, all confirmed: (1) a wired pair grouped then immediately expanded, internal wire
restored; (2) an *external* wire connected before grouping, confirmed landing on the correct
child's handle after expanding — the actual proof of the rewiring logic, mirroring grouping's
own most load-bearing scenario; (3) a composite grouped again with a third node, then the
*outer* one expanded, confirming the inner composite comes back still collapsed — the one-level
rule genuinely holding, not just asserted; (4) a variadic primitive grown to 3 inputs before
grouping, then expanded, confirming its floor reverts to the catalog's declared 2, not the
frozen 3 — the catalog-derived-floor decision genuinely holding.

**Named, not silently accepted — the risk has now compounded three times.** This is the third
consecutive substantial editor inch (fork 3's exclude UI, grouping, now expand) verified by
browser confirmation alone, with zero automated coverage across any of them. Two inches ago this
was flagged as worth deciding deliberately; it wasn't decided, and a third inch landed on top of
it anyway. This isn't a new finding so much as the same one, now with more weight behind it —
worth treating as a real blocker to more editor work, not a recurring footnote.

**Deferred.** `P4.4.1` (transitive inspection UI) is now genuinely ready to start on one front —
its own precondition, a placed composite node existing at all, is satisfied, and `get_unit` is
already the shared foundation both this leaf and `P4.4.1` need, built and tested once. It still
separately needs `kind_of`/`capabilities_of` bridge exposure — neither exists anywhere in the
wasm bridge yet, a real, additional piece of new work `P4.4.1` doesn't get for free from this
inch, named when the two leaves were first compared and still true now.

**Next inch.** `P4.4.1` is the most directly-related, genuinely-unblocked candidate on the
mechanical front. The un-tested-editor-surface risk, named three times now, is the other real
candidate — whichever gets picked, it shouldn't be silently carried into a fourth inch.

---

## 2026-07-01 · P4.4.1 — transitive inspection UI · *(the leaf's own scope is now genuinely met, browser-confirmed; the un-tested-editor-surface risk was raised and knowingly deferred, not silently carried)*

**Context.** Grounding first caught a real finding worth recording, not just a detail: the
leaf's own "annotated with kind + capabilities at every level" doesn't need any new recursive
bridge capability at all. `capabilities`/`effects_of` (both already built, tested core
functions — nothing new on that front) already recurse internally over whatever unit they're
given, so calling either once per hash encountered while walking a tree in TypeScript — using
`get_unit`, already built for `P4.3.1`'s own expand — gives exactly that subtree's own manifest
at that level. The recursion the UI needed was repeated use of things that already existed, not
a new "walk everything and annotate" bridge method.

**Built — bridge.**
- New `WCapability`/`WKind` types: plain, dataless mirrors of `core::Capability`/`core::Kind`,
  crossing as bare strings — the same convention `WType`'s own nullary variants already use.
  Neither existed in the bridge before this; `holon-core` deliberately carries no serde derive
  on either, same reasoning that's held for every other core type all session.
- New `Evaluator.capabilities(hash)` / `effects_of(hash)`: each resolves the hash and calls the
  matching core function directly. `effects_of` exposed as its own method rather than left for
  the editor to derive from `capabilities` — `Capability::kind()`'s fixed projection table stays
  core's own, never a second, TypeScript-side copy that could drift from it.
- `get_unit`'s own "parse hash, resolve" logic factored into a shared `resolve_hash` helper once
  three methods needed the identical pattern — the same discipline already applied to
  `parse_children_and_edges` in the encapsulation-split arc.
- New test, `capabilities_and_effects_cross_js_boundary`: a pure primitive declares nothing; an
  effectful one declares its own capability and kind; a capability buried two levels deep still
  surfaces at the top through the JS boundary — mirroring core's own
  `buried_egress_surfaces_before_import`, now proven crossing too; an unresolved hash fails as a
  plain string, matching `get_unit`'s own convention. `wasm-pack test`: 19 → 20 (+1). `cargo test
  --workspace`: 185/39/11 unchanged.

**Built — editor.** New `InspectPanel.tsx`: a genuinely **read-only** recursive tree, confirmed
— not just asserted — to never touch `nodes`/`edges` state anywhere in its own code. This is the
real, substantive distinction from `P4.3.1`'s expand named a few inches back: expand mutates the
canvas (places children back as editable nodes); this only ever peeks. A collapsible row per
unit (name, kind badge, capability list), children indented beneath, deeper levels collapsed by
default. New "inspect" button and `onInspect` handler in `App.tsx`, resolving the selected
node's hash via the same `unitFor` `onProject`/`onGroup` already share (a fresh primitive is
inserted if it hasn't been yet; an already-frozen composite is read straight off its own
`body.hash`) — nothing new to resolve identity, just handing an existing resolution to a new,
passive consumer.

**Leaf status: `P4.4.1`'s own stated scope is met.** Its test criterion — "inspect a 3-level
composite; every level to the primitives shows correct kind/caps; nothing stops one level
short" — is browser-confirmed: a capability buried inside a twice-grouped composite surfaced
correctly at every level on descent, the load-bearing scenario, mirroring exactly what made the
earlier boundary-rewiring checks the real proof rather than the button existing. One deliberate
scope boundary, not a gap: this only works on canvas-placed nodes today — the registry (`P4.5.1`)
doesn't exist yet to browse anything else. Nothing about `get_unit`/`capabilities`/`effects_of`
is canvas-specific, so registry-browsing inherits this for free later, with zero rework to the
inspection logic itself, whenever that leaf lands.

**Named, addressed, not silently repeated — the testing gap.** This is the fourth consecutive
editor inch (fork 3, grouping, expand, now inspection) verified by browser confirmation alone.
Flagged after the second, flagged again after the third with an explicit "shouldn't be silently
carried into a fourth" — and this time it *was* actually put on the table as a real fork
(address the debt vs. continue feature work) before this inch started, and continuing was a
deliberate choice, not a default. Worth recording as the different, more honest shape this time:
not a fourth unaddressed repetition, a knowingly-deferred one. It doesn't shrink the gap, and it
shouldn't get a fifth free pass without the same conversation happening again.

**Next inch.** `P4.5.1` (registry client) is the next genuinely-unblocked candidate on the
mechanical front — all its stated preconditions are already met, and it's the leaf whose
"import as pin" / "publish" naturally extends this inch's inspect-before-import framing from the
design doc's own trust model. The testing gap remains the other standing candidate, now on its
fourth deferral.

---

## 2026-07-02 · Frontend test infrastructure — the testing debt, closed · *(named across fork 3, grouping, expand, and inspection; addressed here, not deferred a fifth time)*

**Context.** Named after fork 3, named again after grouping, named a third time with an explicit
"shouldn't get a fifth free pass" after inspection. When the choice between addressing it and
continuing feature work was actually put on the table, `P4.4.1` was picked — deliberately, with
the debt explicitly still owed. This inch pays it.

**Setup.**
- `vitest ^3.2.6` — not the latest (`4.1.9`, which requires Vite `6`/`7`/`8`), checked directly
  against the npm registry rather than assumed. Forcing an unrelated Vite major-version bump as
  a side effect of adding tests would have been a bigger, riskier change than asked for; `3.2.6`
  is the newest release still compatible with this project's existing Vite `5.x`.
- `jsdom ^29.1.1`, `vite.config.ts` switched to `vitest/config`'s `defineConfig` (re-exports
  Vite's own, typed with the `test` field included — one config, not a second, parallel
  `vitest.config.ts` that could drift from it).

**A real course-correction, not a clean build from the start.** The original plan was the
simplest one: add `export` to four pure functions already living in `App.tsx`
(`boundaryLabels`, `isWEncapError`, `validationErrorNodeIndices`, `encapErrorNodeIndices`) and
test them in place. Tracing the actual import graph before writing the test file caught a real
problem: `App.tsx` has a top-level *value* import of `Evaluator` from `holon.ts`, which itself
imports the real wasm package (`../wasm/holon_wasm.js`, built for `--target web`, browser-
oriented). Importing anything from `App.tsx` — even four unrelated pure functions — would
evaluate that whole chain. Rather than gamble on whether that happens to work under a test
runner, or paper over it with a `vi.mock` that could silently go stale the moment `holon.ts`
gains a new real export some other file starts using, the four functions were extracted into a
new module, `diagnostics.ts` — genuinely the more correct move, not just a workaround, since
none of them ever needed anything from `holon.ts` except types. Only type-only imports remain,
which TypeScript erases entirely, so the new module has zero runtime dependency on `holon.ts` or
the wasm package. `theme.ts`'s functions needed no such extraction — already exported, already
free of any risky import.

**First batch: 25 tests, two files.** `theme.test.ts` (8 tests) — `typeKind`/`typeColor`/
`outputColor`/`typeLabel`, including that every declared kind gets a genuinely distinct color,
not just a plausible one. `diagnostics.test.ts` (17 tests) — the centerpiece is
`encapErrorNodeIndices`, deliberately exercised against its *current* variant names
(`ExposeChildOutOfRange`/`ExposePortInvalid`, no `AmbiguousDefaultOutput` at all): this is
exactly the check that would have caught the stale-`WEncapError`-names bug immediately earlier
this session, instead of by manual review.

**A real compile error, caught and fixed honestly, not just silenced.** `npx tsc -b` failed on
`encapErrorNodeIndices`'s own test: a `cases` array typed as `WEncapError[]` (the full
discriminated union, including `Invalid`, which has no `child` field) couldn't support
`err.child` even though every literal actually in the array genuinely has one — TypeScript
doesn't narrow a declared array type from its contents. Fixed via
`Array<Extract<WEncapError, { child: number }>>`, which narrows to exactly the variants that
structurally carry `child` while staying checked against the real union — a misspelled `kind` or
a missing field still fails to compile, preserving the actual protective point of typing the
array explicitly, not just making the error go away.

**Environment cleanup: a real, quiet risk, named and resolved, not assumed harmless.**
`vite.config.js` and `vite.config.d.ts` turned out to be stale compiled artifacts sitting beside
the real `vite.config.ts` (the `.d.ts` in particular is unambiguous — it only exists as `tsc`
build output). Left in place, Vite's config resolution could have silently preferred the stale
`.js` over the `.ts` carrying the new `test` field, producing a confusing failure that looked
unrelated to its actual cause. Both confirmed deleted.

**Verified, not just asserted.** `npm test`: 25/25. `npx tsc -b`: clean. `npm run dev`: starts
and serves normally. Re-ran the full suite again after the `Extract` fix specifically to confirm
a type-only change hadn't altered runtime behavior — it hadn't (`Extract` is compile-time only),
still 25/25.

**One live finding, named rather than silently fixed or silently ignored: `npm audit`'s
esbuild/vite advisory.** `esbuild <=0.24.2` (bundled via `vite <=6.4.2`) — a moderate/high
finding whose actual exposure is narrow: a malicious website, visited in a browser while the dev
server happens to be running, can send it requests and read the response — dev-server-only,
never shipped in a production build, and only reachable if the dev server is exposed beyond
localhost (not this project's default). The available fix isn't a patch — `esbuild`'s fix ships
bundled inside `vite@8.1.2`, three majors up, so taking it would mean reversing the exact
version constraint just chosen above to avoid an unrelated major bump. Deliberately left as-is,
with two explicit triggers for revisiting named rather than left implicit: immediately, if the
dev server is ever run with `--host` (exposed beyond localhost); or opportunistically, whenever
this project's own Vite upgrade happens for unrelated reasons, taking the `esbuild` fix along
with it rather than going looking for it separately.

**No `PLAN.md` change.** This inch isn't tied to any `P1`-`P4` leaf — a cross-cutting session
finding resolved, not a leaf closed.

**Next inch.** Per the order already agreed: the `SmallVec` perf item from `P4.7` next (its own
stated trigger — an actual multi-output composite existing to benchmark against — has been
satisfied since the `P4.3` forks landed), then the `P4.5.1`/`P4.6.1` host+registry transport
scoping pass.

---

## 2026-07-02 · SmallVec perf inch — `eval`'s hot-path allocation, closed · *(the P4.7 benchmark finding, deferred twice, addressed here)*

**Context.** Named at `P4.7`'s close: widening `eval` to `Vec<Option<Value>>` regressed every
primitive eval ~45–50% (flat across list sizes — a fixed per-call cost), root-caused to
`eval_one` — which `eval_map`/`eval_fold` call once per list element — always heap-allocating
through `eval` and discarding all but one slot. Deferred pending an actual multi-output
composite existing to benchmark against; that trigger was satisfied once the `P4.3` forks made
multi-output an everyday, automatic editor construct.

**One nuance surfaced and named before building, not glossed over:** `map`/`fold`'s `fn`
argument is required to be single-output (`eval_one` fails closed otherwise), so the specific
hot loop this regression concentrates in can never itself contain a multi-output unit, no matter
how common they get elsewhere. The trigger being satisfied means "is multi-output a real, used
capability worth having paid this tax for" — not "does the hot loop now see multi-output data."
Worth being exact about, since it's a different justification than a first pass implied.

**The fix.** `eval`'s return shape is now a named type, `EvalOutputs = SmallVec<[Option<Value>;
1]>` — inline stack storage for 0 or 1 outputs (every genesis primitive, forever; still the
overwhelming majority even now), spilling to the heap only for a genuine multi-output composite.
`eval_composite`'s own internal `scratch` (previously `Vec<Vec<Option<Value>>>`) had to become
`Vec<EvalOutputs>` too, not just `eval`'s own signature — `scratch[ci] = eval(child, ...)` stores
each recursive call's result directly, so skipping the scratch would have meant the fix only
reached the outermost call, not nested composites — the exact "route around it instead of fixing
it" shape the `eval_composite`-by-reference precedent (2026-06-30) warns against repeating.

**Real, but narrow, production ripple — corrected from an earlier, rougher estimate.** `eval_one`
needed zero logic changes (`.len()`/`.pop()` both exist on `SmallVec`). The wasm bridge's
`Evaluator::eval` needed zero changes at all — its existing `.iter().map(...).collect()` already
builds a fresh `Vec<Option<WValue>>`, and `SmallVec` supports `.iter()` identically to `Vec`.
`memo.rs`'s `verify_entry` needed zero changes — `canonical_slots_bytes` takes a slice
(`&[Option<Value>]`), and `SmallVec` derefs to one automatically. `holon-host`'s `fire` needed
zero changes — already on `eval_one` since `P4.7` sub-leaf (d). The one real production site:
`holon-host`'s `Memo::eval`, which calls the widened `eval` three times and returns
`Vec<Option<Value>>` from its own (deliberately still-`Vec`, since it's a stored, long-lived
cache, not a hot loop) signature — `.into_vec()` at each of the three call sites.

**Wide, but purely mechanical, test-suite ripple.** `SmallVec` implements `PartialEq` only
against another `SmallVec`, not against `Vec` directly (confirmed against the crate's own
source, not assumed) — every existing test comparing `eval(...)`'s result directly against a
`vec![...]` literal needed `.into_vec()` appended at the actual-value side, keeping every
expected-value literal exactly as `vec![...]`, unchanged. Real work — a few dozen call sites
across `eval.rs`, `catalog.rs`, `determinism.rs`, `encapsulate.rs`, `promote.rs`, `simulate.rs`,
and the wasm bridge's own test module — but zero design risk, each fix identical. Comparisons of
`eval(...)` against another `eval(...)` call (not a `vec![...]` literal) needed no change, since
`SmallVec == SmallVec` already works. Benchmark call sites (`native_vs_composite.rs`) needed no
change either — `black_box` takes any `T` by value, no comparison ever happens there.

**A real methodological finding, worth naming rather than quietly working around:** the local
copy of `holon-host/src/lib.rs` used for this pass turned out to be a stale, pre-`P4.7` snapshot
(still showing `fire` calling the widened `eval` directly with `?`, when it's actually been on
`eval_one` since `P4.7` sub-leaf (d) closed). Caught by grounding the real current file via
search before editing, rather than trusting a stale local baseline — editing or delivering a
full file from the wrong starting point would have risked silently reverting whatever this
file's actual current state is. Delivered as a targeted patch instead of a full file for that
one function, not out of caution theater but because the risk was real and specific.

**The second dependency.** `smallvec = "1"` — `holon-core`'s first new dependency since
`blake3`, and the honest price of this fix: real, not glossed over. `PLAN.md`'s own "exactly one
dependency" claim updated to match, rather than left stale.

**Verification — actually run, not deferred.** One real hiccup on first application, worth
naming rather than glossing over: `cargo test --workspace` initially failed with `unresolved
import smallvec` — the `Cargo.toml` dependency line from this patch hadn't been applied yet,
only the code changes had. Once added (`smallvec = "1"`), everything compiled and ran clean.

`cargo test --workspace`: **255/255 green** — 185 core + 39 host + 11 wasm-native.
`wasm-pack test --node crates/holon-wasm`: **20/20 green**. Confirms the `.into_vec()`
conversions were lossless — nothing behaves differently, only the allocation pattern changed.

`cargo bench -p holon-core` — the actual proof this inch exists for, and the numbers back it
up, with two anomalies named honestly rather than rounded away:

- **`fold_sum` — the actual hot loop this inch targets** (`eval_map`/`eval_fold` calling
  `eval_one` once per element) — improved 17–35% across every list size, largest at the
  biggest size (4096: −22.5%). This is the fix working exactly where it was built to: the
  allocate-then-discard `Vec` per element is gone.
- `native_head`/`native_is_empty`/`native_add` all improved 15–29%, consistent with the P4.7
  regression being undone. `composite_add` showed no significant change — expected, not a gap:
  that path is dominated by hashmap lookups and topological sort, costs this fix never touched,
  so the allocation savings are a small fraction of a much larger baseline.
- **Anomaly 1, likely pre-existing, not introduced by this fix:** `native_length/1`'s absolute
  time (~197 ns) sits roughly 4× every other list size (~47–53 ns for 8 through 4096) — odd for
  a supposedly-flat O(1) operation, and n=1 should be the *fastest* case, not the slowest. But
  its own reported *change* column shows near-zero movement ("within noise threshold"), meaning
  this gap was already there before this fix, not caused by it. Most likely a benchmark
  warm-up artifact specific to the first sample in the run, not a real defect.
- **Anomaly 2, flagged but genuinely unconfirmed — not settled, not smoothed over:**
  `native_sum/512` regressed (+17.7% to +30.5%, criterion's own "Performance has regressed"
  flag), sandwiched between `/64` and `/4096`, both of which improved sharply (−22 to −32%). A
  single size regressing between two neighbors that both improved doesn't fit a structural
  explanation — it has the shape of a one-off measurement (background load, a GC-adjacent
  pause on that run) rather than something this fix caused. This is a plausible read, not a
  verified one: it was never re-run in isolation to confirm it reproduces. If it matters,
  `cargo bench -p holon-core -- native_sum` targeting just that benchmark would settle it either
  way — genuinely open until then, not resolved by this entry.

**Net assessment:** the fix does what it was built for (the hot loop it targets improved
substantially, confirmed by measurement, not assumed), nothing broke (255+20 tests green), and
the two anomalies both look like measurement noise rather than defects from this change — but
one of them is a plausible read, not a confirmed one, and is named as such rather than folded
into a clean "all good" story it hasn't fully earned yet.

**Next.** Decide whether the `native_sum/512` anomaly is worth a targeted re-run before moving
on, or set it aside as noise and proceed straight to the `P4.5.1`/`P4.6.1` host+registry
transport scoping pass, or return to the `P4` projection frontier.