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