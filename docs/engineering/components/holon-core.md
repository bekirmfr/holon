# holon-core

The pure core: what a unit is and how it is hashed, the values that flow on its wires, the
catalog of primitives and what each computes, the evaluator, and the facts derived from a graph
before it runs. The evidence is in the evaluator's L4 notes:
[P1.4.3-evaluator-L4](../../L4s/P1.4.3-evaluator-L4.md) (the firing rule),
[lazy-choices-L4](../../L4s/lazy-choices-L4.md) (LC: which nodes run),
[interpreter-path-L4](../../L4s/interpreter-path-L4.md) (IP: the evaluator's steps and their
measurements), [child-order-canonical-L4](../../L4s/child-order-canonical-L4.md) (CO) and the
[resilience audit](../../L4s/resilience-audit-L4.md) (RA). What came before the repository's
history is in the [JOURNAL](../../JOURNAL.md), by date.

## 1. What it is

`crates/holon-core`, compiled native for the server and the command line, and to wasm32 for the
editor ([holon-wasm](holon-wasm.md)): one evaluator, one encoder. It performs no effect and has
no IO, async or network dependency; its five dependencies are `blake3`, `hmac`, `libm` (the
number leaves' transcendental functions, the same bits on every target), `sha2` and `smallvec`
(`Cargo.toml`), and the purity boundary is that dependency graph (`lib.rs`). An
effect is a value core builds and the host performs ([holon-host](holon-host.md)).

| File | Holds |
|---|---|
| `hash.rs` | `Unit`, `UnitBody`, `Param`, `Port`, `Edge`, `Type`; `encode_preimage`, `decode_preimage`, `hash_unit`, `canonical_child_order` |
| `value.rs`, `value_hash.rs`, `json.rs` | `Value`; `encode_value`, `decode_value`, `hash_value`, `state_root`, `run_id`; JSON both ways |
| `validate.rs`, `resolve.rs` | `validate`, `compatible`; the `Resolver` trait and `MapResolver` |
| `catalog.rs`, `primitives.rs`, `numbers.rs`, `lists.rs`, `records.rs`, `dates.rs`, `texts.rs`, `docs.rs`, `crypto.rs` | `GENESIS`, `interface_of`, `VARIADIC`, `PASSES_EXTRA`, `function_takes`, `FAMILIES`, `TRIGGERED`; `compute`, the `ops!` list, the number leaves, the list leaves and `sort_by`'s merge sort, the record, date and text leaves; `describe` |
| `eval.rs`, `eager.rs` | `eval`, `eval_observed`, `Observer`, `Slot`, `Answer`, `choice_next`, `Shape`, `Shapes`; the eager oracle |
| `kind.rs`, `manifest.rs`, `taint.rs`, `sql.rs`, `documents.rs` | `Capability`, `Kind`, `capabilities`; `manifest_items`, `consent_name`; secret flow; what a database leaf touches |
| `declare.rs`, `triggers.rs` | `read_server`, `match_route`, `LogLevel`, `Server::handlers`; file triggers |
| `module.rs` | `validate_module`, `module_limits`, `ModuleLimits` |
| `encapsulate.rs`, `promote.rs`, `fork.rs`, `inspect.rs`, `memo.rs`, `simulate.rs`, `advisory.rs`, `determinism.rs` | grouping into a holon, templates, re-pinning, `child_refs`, memo keys, design mode, the float rule |

## 2. How it works

### Units and their hash

A `Unit` is a body, params, input ports and output ports. The body is `Primitive { name }`,
`Composite { children, edges }` (children by 32-byte hash only, never inlined) or
`Module { bytes }` (a WebAssembly binary, [holon-module](holon-module.md)). A `Param` is
`Frozen { name, value }` or `Open { name, bind }`, a hole a template is instantiated through
(`instantiate`). An `Edge` joins `(from_child, from_port)` to `(to_child, to_port)`; `BOUNDARY`
(`u32::MAX`) as a child is the composite's own interface, a source for its inputs and a sink for
its outputs, so a holon's interface rides ordinary edges.

`encode_preimage` writes `ENCODING_VERSION` (5), a body tag (`TAG_PRIMITIVE`, `TAG_COMPOSITE`,
`TAG_MODULE`), then tagged, `u32`-LE length-prefixed fields; `hash_unit` is BLAKE3 of it. What
changes a result is in (the body, params, port names and types, edges, child hashes); what only
changes the picture is out (positions, labels). Params and edges are sorted. Children are put in
`canonical_child_order`: by hash, and same-hash children by a path label naming where each one's
value goes, from the outputs back; two left tied are a value computed twice and never used,
refused as `EncodeError::UnusedTwins` (CO §4). `decode_preimage` is the inverse and refuses
anything else (`DecodeError`), types nested past `MAX_TYPE_DEPTH` (32) included.

A `Value` is `Number(f64)`, `Boolean`, `Text`, `List`, `Record` (a `BTreeMap`, so fields are in
key order), `Sum { arm, payload }` (`none()` is arm 1 holding the empty list) or
`Effect { sink, payload }`. `encode_value` starts `[ENCODING_VERSION, 0x20]` so a value hash never
equals a unit hash; every `NaN` encodes as `CANONICAL_NAN` and `-0.0` as `0.0`. `state_root`
hashes a key → value-hash map; `run_id` hashes a deployment's hash with the root of what a run
started from. Wire types are `Type`; `compatible` is `Any` on either side, else structural
equality, recursive over `List`, `Product` and `Sum`. `validate` checks one composite's wiring: each
endpoint exists, each input port has at most one wire, no cycle, types fit; every error is
collected, for the editor.

### The catalog

`GENESIS` names the 152 primitives. Each name has its facts in one place each: its interface
(`interface_of`), its capabilities (`capabilities_of`, `kind.rs`), what it does in words
(`describe`), whether it grows inputs (`VARIADIC`) and its computation. `compute` covers 117 of them,
by an `Op` that one list (`ops!`) makes along with `Op::of`, so a name can't be in one and not the
other (RA D1). The rest are not computed here: `map`, `fold` and the other higher-order leaves
(`PASSES_EXTRA`: `filter`, `any`, the `find` family, `sort_by`, `group_by`, …) are the evaluator's; 18 build a
request the host answers (`Request::of`: `http_request`, `collection_get`, `await`, …); the
drivers and `store_read` are sourced, their value put in by the host's resolver as a constant.
`MAX_RANGE` (10,000) bounds `range`, and there is no loop or recursion, so every run ends.

### The evaluator

`eval(unit, inputs, resolver)` gives one `Option<Value>` per declared output (`EvalOutputs`, a
`SmallVec` inline for one). A primitive and a composite go through the same entry point.
`eval_observed` takes an `Observer`: told when each child is entered and left and what it
`produced`, asked to `answer` each request, and able to stand in for a node (`asserted`, how
`holon verify` gets past a secret it never logged). Core reads no clock: the host's timeline and
its rounds of outbound calls are both observers.

A composite is walked from its roots (what its outputs read, and what nothing consumes) back
through what each child needs, depth first with an explicit stack (`Prepared::run_in`). An ordinary
leaf needs every wired input; a choice only what `choice_next` says: `guard` its value only when
its condition is `true`, `coalesce` its fallback only when its first input is absent. A node only
an untaken choice consumes is never run, so it asks the host nothing (LC §2). Each slot is
`Slot::Value`, `Absent` (nothing will come) or `Pending` (an answer not given in this round); a
node needing a pending input is pending, and a choice whose deciding input is pending waits (LC
§4). Pending reads as `None` at the public edge. A variadic leaf with an absent input gives
nothing (`eval_as`), and so do a sink whose wired trigger hasn't arrived and a leaf with optional
ports whose wired one is absent (`held_when_absent`): unwired, an optional port reads as none.

What a walk needs of its unit alone is made once. `NodeKind::of` decides each child's kind (a
`Body`: `Map`, `Fold`, `Ask(Request)`, `Coalesce`, `Leaf { variadic, op }`, `Composite`, `Module`)
where its composite is prepared. A composite's `Shape` (its edges indexed by destination in
`Incoming`, its roots, whether it has a cycle) is made by `Shape::of`, or kept by a host and handed
out through `Resolver::shape`; `Shapes` finds one by the unit's address, so a lookup hashes nothing.
Inputs are lent (`Refs`), never copied to be read. A `map`'s or `fold`'s function is prepared once
(`Ready::of`) and, when it has no choice but pass-throughs (a `guard` on a `const` `true`), run
down a `Plan`: the walk's order worked out ahead, each child's inputs as slots. When the plan
meets what it wasn't built for (a condition not `true`, an input not yet in) it hands the run to
the walk on the same scratch (`run_planned`).

Every other way a unit can be wrong reads as absent, never a panic: a child that doesn't resolve
or a cycle makes every output absent; an edge from a child that doesn't exist reads as absent
(`Source::Missing`); a node whose kind isn't its body's computes nothing (RA R3). A module asks
`Resolver::run_module`, whose default answers "this host runs no code".

### Facts read off the graph

`capabilities` unions each leaf's manifest over the closure, following children and `fn` params
(`child_refs`); an unknown leaf or a child that doesn't resolve saturates to every capability.
`Kind` (`Pure`, `Driver`, `State`, `Sink`) is a projection of the capabilities, never stored.
`manifest_items`, `consent_name` and `trigger` are the rules the server grants by and the editor
shows. `taint.rs` follows each secret through the graph (literal or derived) to the effects and
answers it reaches. `read_server` reads a flow's declaration (routes, clocks, file handlers,
actions, each handler's `LogLevel`) and fails closed on anything malformed.

## 3. Why it is this way

- **Effects at the edge, the core pure.** A sink builds its request as a value; the host performs
  it after the run settles ([sink-as-pure-request-L4](../../L4s/sink-as-pure-request-L4.md)). Core
  can't perform an effect because it has nothing to perform one with. This is what lets a run be
  replayed and checked offline.
- **The firing rule splits** (P1.4.3 §0): required inputs, optional ones and defaults are pure, so
  they are here; a port's memory and consumed tokens are not, so state is the host's.
- **Child order is presentation** (CO §2): `eval` orders by edges, so the order children are listed
  in can't change a value and is kept out of identity. The first canonical order searched
  orderings, capped at 8!; a route with nine `{…}` parts hit the cap, and path labels replaced the
  search (CO §4). A new body tag is a compatible extension; a change to how a tag encodes bumps
  `ENCODING_VERSION` ([wasm-module-L4 §2](../../L4s/wasm-module-L4.md)).
- **Lazy at its choices** (LC §1): the eager evaluator ran every child, so
  `go ? await api.get(…) : "skipped"` sent its request on every run. Values don't change (every
  leaf is pure and total); what is asked of the host does. `select` is not lazy: it is absent when
  either side is, and skipping one would change that.
- **Pending is not absent** (LC §4): a `coalesce` treating "not yet" as absent would send its
  fallback request before learning it wasn't needed.
- **Made once, not per run or per item** ([Performance](../themes/performance.md#the-evaluator-steps-1-to-6)):
  a function prepared once (`4313f0a`, `crunch` 888 → 495 µs a run at 100 items,
  [request-path-L4 §4.5](../../L4s/request-path-L4.md)); then, in instructions a run under
  callgrind (IP §4), shapes kept (`440eca0`, `hello` −8.3%), inputs lent (`ece0790`, `crunch`
  −55.7%, `hello` −34.2%), kinds decided once (`7c31de1`, `crunch` −11.0%), the plan and opcodes
  (`abc4b11`, `crunch` −15.1%).
- **The plan stands behind the walk.** It is an optimisation, not a second meaning: a case it
  wasn't built for goes to the walk, which answers as it always did (RA R3).
- **What was tried and left** ([Performance](../themes/performance.md#tried-and-left)): composites
  evaluated by reference made every primitive about 24 ns slower for a 5–10% gain, and was
  reverted; shared values inside `Value` (IP step 4) were deferred, copying being 5–7% of `crunch`
  after step 3 for a change touching about 850 uses.

## 4. Reach and limits

- **A flow is interpreted.** The walk runs the graph as data, general code on general values.
  Compute-heavy flows answer 3.2×, 11× and 14× slower than C# at 100, 1,000 and 5,000 items (p50
  at one connection, `abc4b11`, IP §4 step 5). What is left is allocating, freeing and copying
  values and firing each node ([Performance](../themes/performance.md#limits)).
- **`Value` deep-copies on clone.** `with_field` copies its record to add a field.
- **Only a `map`'s or `fold`'s function runs down a plan.** A route's handler, and any function
  with a real choice, is walked. A composite's children are resolved once where the resolver keeps
  them (the server's does, whatever a run reads: a sourced leaf asks `Resolver::sourced` for its
  value); design mode, which hands a run a mock for a hash, keeps none.
- **Laziness stops at a holon's boundary.** A holon's inputs are evaluated before it is entered, so
  a choice inside it can't spare its caller's request (LC §3). `map` applies every item even after
  one is absent, so a round's requests go out together.
- **Recursion per nested composite.** The server refuses closures nested past 64 units
  (`MAX_CLOSURE_DEPTH`, `crates/holon/src/units.rs`); a test holds 64 levels within 1 MiB of stack.
- **`Shapes` is keyed by address**, sound only while the units it was built over don't move and
  outlive it; the host keeps the two together.
- **One number type** (`f64`), **no bytes value**, and an open `Record` type (any fields). A `Sum`
  value's `ty()` is `Sum([Any, Any])` whatever its true arity, honest only while `some` and `none`
  are the only constructors (`value.rs`).
- **`validate` checks one level**: a composite's own wiring, its children by their interfaces. A
  host validates every unit of a closure (holon's `units.rs`, `load_closure`).
- **The SQL scan refuses what it doesn't understand** (`sql.rs`), on purpose: a statement outside its
  small part of SQL can't be deployed until the scan grows.
- **A state key named `<unknown>`** reads as the saturation marker (`SATURATED_STATE_KEY`); nothing
  refuses it yet (`kind.rs`).

## 5. What the tests hold

About 340 `#[test]` functions in `src/`, plus the golden records and oracles in other crates
([Testing](../themes/testing.md)).

- **Identity.** `param_order_is_cosmetic`, `edge_authoring_order_is_cosmetic`,
  `child_order_is_cosmetic`, `port_name_is_part_of_contract`, `encode_decode_is_byte_exact`,
  `decode_rejects_malformed_bytes`, `identical_children_that_feed_nothing_are_refused`
  (`hash.rs`); `the_encoding_is_pinned` (a value's exact bytes and hash),
  `both_zeros_and_every_nan_hash_alike`, `a_value_hash_never_equals_a_unit_hash` (`value_hash.rs`).
- **The catalog.** `genesis_has_95_leaves`; `the_two_catalogs_agree` (every name has an interface
  and capabilities); `every_genesis_primitive_is_computed_somewhere_and_every_opcode_is_one`.
- **The evaluator against its oracles.** `lazy_and_eager_agree_on_generated_graphs` (8,000 runs
  against `eager::eval_eager`); `a_composite_run_down_its_plan_does_what_the_walk_does` and
  `a_pass_through_found_not_true_hands_the_run_to_the_walk` (3,000 random graphs, the same events
  in the same order); `a_plan_out_of_order_hands_over_and_a_kind_that_isnt_its_bodys_computes_nothing`.
- **Choices, absence, depth.** `a_request_under_a_false_guard_is_never_asked`,
  `select_is_not_a_choice_and_asks_both_sides`, `pending_crosses_a_holon_boundary_and_a_map`,
  `variadic_leaf_with_an_absent_input_fails_closed`, `edge_from_a_missing_child_reads_as_absent_not_a_panic`,
  `a_composite_nested_as_deep_as_a_host_runs_fits_half_a_threads_stack`; `unknown_leaf_saturates`,
  `map_over_effectful_fn_is_effectful` (`kind.rs`).
- **Elsewhere.** `the_evaluator_gives_what_its_golden_record_holds` (`holon-author/src/golden.rs`:
  every node's outputs for every sample and `crunch`, against `tests/golden/eval.txt`);
  `keeping_each_composites_shape_changes_nothing_the_evaluator_gives`;
  `the_compute_heavy_flows_functions_all_run_down_a_plan` (D5); `every_samples_routes_give_what_the_eager_evaluator_gives`
  (through the `eager-oracle` feature); the wasm parity checks ([holon-wasm](holon-wasm.md)).

**Gaps.** No test in core pins a unit's hash as a literal: the hash tests compare hashes with each
other, and an encoding change is caught by the server's golden record, which holds each run's
`unit` and `run_id` (`crates/holon/tests/golden/server.txt`). The plan is checked on graphs drawn
from a small pool of leaves. Nothing tests that a moved unit misses its shape. The benchmark
(`benches/native_vs_composite.rs`, criterion) runs by hand; nothing in CI fails on a slower core.

## 6. History

Before the repository's history (the [JOURNAL](../../JOURNAL.md)): validation, values, the
primitives, the eager evaluator and the determinism harness (2026-06-29, P1.3.1–P1.4.4);
encapsulation, `map`, templates; kinds and capabilities; sinks as pure requests (Fork B); child
order canonical, the interface catalog, inputs gathered by reference (2026-06-30); several outputs,
`ENCODING_VERSION` 4 → 5 (P4.7, 2026-07-01); path labels for child order (2026-10-24); lazy choices
and the module body (2026-10-25). Then, by commit:

- `2c15b96`, `14c7163`: the secret-flow analysis moved into core (`taint.rs`); where each secret
  is sent, and answering a caller with one, are permissions.
- `f06e7a1`: `LogLevel` and `may_record_lightly`. `b17cbbb`: hex from a table.
- `7eb8e22`: the golden records, before the evaluator was touched.
- `4313f0a`, `e916977`, `440eca0`: a function prepared once; the edge index and `has_cycle`;
  shapes kept across runs.
- `ece0790`: inputs lent. `7c31de1`, `abc4b11`: `NodeKind`; the `Plan` and leaves by `Op`.
- `f47c854`: hashing into a kept buffer (`hash_value`, `state_root`).
- `dc21f76`: R3, fail closed. `cf1f961`: one list for opcodes (D1). `9969f8b`:
  `Shapes::footprint` for the flow cache's count; `would_plan` for the D5 tripwire.
