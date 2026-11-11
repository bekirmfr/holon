# The js node

JavaScript as one node:
`const closest = js(function closest(word, candidates) { … }, { outputs: ["best", "distance"] })`
puts a computation the vocabulary can't spell (an edit-distance table, a game's rules) in a flow,
with every guarantee a leaf has. The design is the
[js node L4](../../L4s/js-node-L4.md) (JL4), over the [module unit](../../L4s/wasm-module-L4.md);
the language is [`LANGUAGE.md`](../../LANGUAGE.md) §5.8a.
The sample is `nearest`; `inbox-report`, `guestbook`, `tally`, `chess` and others use one.

## 1. What it is

A js node is a holon whose body is JavaScript. It lowers to a **composite over one pinned engine
module and its source as a `const`**, never a new body kind: the engine is a WebAssembly module
with no imports, a JavaScript interpreter that runs the source in a fresh realm per call, with no
clock, no randomness and no I/O.

In the code:

- `crates/holon-author/src/lang/`: the TypeScript reader makes a `JsDef` of the `js()` call, its
  body as written with its types blanked (`ts.rs`, `js_holon`, `blank_types`), the lowering builds
  the composite (`lower.rs`, `js_node`, `js_source`), the printer reads it back (`print.rs`,
  `is_js_node`, `js_statement`).
- `crates/holon-author/src/modules.rs`: the engine unit under a budget (`js_engine_unit`), and a
  document's modules by engine and params (`entry_of`, `restore_modules`).
- `crates/holon-module/src/lib.rs`: the engine bytes (`JS_ENGINE`, from
  `fixtures/modules/js-engine/`), the `wasmi` runner (`Engine`, `shared`), the resolver that runs
  modules (`WithEngine`), the compile-only call (`compile_js`), the defaults (`JS_DEFAULT_FUEL`,
  `JS_DEFAULT_MEMORY`).
- `crates/holon-core/src/module.rs`: what a module must be (`validate_module`), its budgets
  (`module_limits`, `module_limits_of`, `DEFAULT_MODULE_FUEL`).
- `fixtures/modules/js-engine/`: the engine's source (Boa 0.22, `boa_engine`), its `Cargo.lock`
  and `build.sh`, and the committed `js-engine.wasm` (3,244,101 bytes), so a checkout needs no
  toolchain.

## 2. How it works

### The `js()` function and its lowering

```ts
const f = js(function f(a, b) { body }, { outputs: ["total", "weight"], fuel: "50M", memory: "16MB" })
```

The function's parameters are its input ports; its output is named after the function,
or `outputs` names two or more; `fuel` and `memory` are optional. oxc's parser reads the body as
part of the function, so nothing inside is read as Holon; its TypeScript types are blanked to
spaces before the engine sees it, so its lines and columns stay where they were, and `fmt` never
touches it. It lowers to:

```
const{value: "(a, b) => { body }"}       the source, js_source: the declared names and the body
list(a, b)                               the arguments, in declared order
engine<fuel, memory>(source, args)       the engine module under its budget, js_engine_unit
field{total}(result.value), field{weight}(result.value)    one per output (none for one output)
```

A budget is a frozen param of the engine unit, so a node's budget is part of its identity; the
source is too, verbatim. Two js nodes with the same ports, source and budget are one unit in any
flow. The text names no engine hash: the document's closure pins it, and an engine release is a
new version with nothing new to approve. A flow document lists a module by `{hash, engine,
params}`, not its 3.2 MB binary, and whoever restores it rebuilds the unit from the engine its
build carries and checks the hash (`restore_modules`).

### The engine

The module's interface is `engine(source: Text, args: List<Any>) -> (result: Record)`. Per call it
makes a fresh realm, installs the prelude (no `Date`, no `Math.random`, no timers, `fetch`,
`WebAssembly` or `Intl`; `toLocaleString` is `toString`; strict mode), compiles the source as a
function expression, applies it to the arguments and answers `{value}` or `{error: "<name>:
<message> at line L"}`. Values cross in core's own encoding (`encode_value`, `decode_value`):
numbers as `f64` (non-finite ones canonical), text as UTF-8, lists as arrays, records as plain
objects in key order, an absent value inside as `undefined`; `undefined` or `null` returned is an
absent output; a `bigint`, function, `Map`, `Set`, `Date`, class instance or `Promise` is an error
naming the kind. An `Effect` never crosses. An absent input never reaches the body: `list` fails
closed, so the node is absent, as any strict leaf.

### Running it

Core sees a `UnitBody::Module` and asks `Resolver::run_module`. Everywhere a flow runs, the
resolver is `holon_module::WithEngine::new(&resolver, holon_module::shared())`: a route
(`listeners.rs`), a clock, a file, an action, a resume, `holon verify`'s rerun, and design mode in
the editor through the wasm bridge (`holon-wasm`), so the same bytes run under the same budget
everywhere. `Engine::run_measured` validates the module (`validate_module`: an empty import
section, the ABI's exports), compiles it once per process (cached by the bytes' hash), and runs it
in a fresh `wasmi` `Store` with fuel metering and a memory limiter (`Budget`). Running out of
either, a trap or an answer that doesn't decode is a `RunError`; `WithEngine` keeps it
(`take_failure`), core tells the observer at the module's path, and the server's timer keeps it as
`NodeTiming::reason`, so `explain` says "closest gave nothing: out of fuel (200000000
instructions)" or the body's `TypeError`. From the flow's side the node is absent, and a route
answers it with the status it gives an absent body (`return found ?? status(502)`; 404 unsaid).

### Budgets and the manifest

A js node's defaults are its own (`JS_DEFAULT_FUEL`, 200M instructions; `JS_DEFAULT_MEMORY`, the
module default of 16 MiB), since the module default of 10M fuel can never run a body. A flow with a
module asks for `Limit(module_fuel=N)` and `Limit(module_memory=N)` at the largest any node
declares (`Requirements::with_module_limits`); the approval says "run code it carries (a js node)
for up to N instructions per call, reaching nothing outside" (`describe_item`). The host's ceilings are `MODULE_FUEL_CEILING` (1,000,000,000) and
`MODULE_MEMORY_CEILING` (256 MiB), raised under `[ceilings]`; a `[limits]` default for a module
budget is refused, since a module's budget is its own. `chess`'s `rules` asks
`{ fuel: 1000000000 }`, the ceiling.

### `check` and the editor

`holon check` compiles each body once through the engine's compile-only call (`compile_js`: the
source alone, no arguments) and says a syntax error at its line in the file with a caret
(`js_mistakes`). The engine rides in the editor's bridge bundle, so the text pane can lower a js
node anywhere a text is compiled. The flow cache shares the engine module's unit between every
closure that places it (`units.rs`, `Loaded`).

## 3. Why it is this way

- **A module, not a host engine.** A native JavaScript engine in the host would be faster to build
  and run, but "computes only" would become a promise instead of a property core reads from the
  bytes (an empty import section is `Pure`), and determinism would be per platform (JL4 §11).
- **Fresh realm per call, no clock or randomness.** Two equal calls must be equal, or replay and
  `holon verify` can't hold; a cached realm is the first place nondeterminism would hide. A flow
  that wants the time wires `now` in (JL4 §4).
- **Boa, pure Rust.** No engine built to `wasm32-unknown-unknown` with no imports was ready-made;
  Boa builds with the same toolchain as everything else. Building it taught two things: `getrandom`
  needs a custom backend (a fixed sequence), and Boa's clock read the system clock (a panic), so the
  realm gets a `FixedClock` at the epoch (JL4 §4). QuickJS stays the alternative.
- **200M fuel by default**, measured (`what_a_call_costs`): a fresh realm and a one-line function
  cost about 12M instructions, an edit distance over eight words 41M, a loop of 100,000 271M (JL4
  §2).
- **Modules by engine and params in a document**: carrying the binary, a flow with two budgets
  passed the control plane's 8 MiB body limit (JL4 §3).
- **Errors are values, then absence**: the flow sees an absent output like any failed leaf, and the
  record keeps the reason for `explain` (JL4 §5).

## 4. Reach and limits

- **Cost, measured once** (JL4 F6, a release build, one developer machine): a trivial body 6 ms
  warm (32 ms the first call, compiling the engine), the edit distance 20 ms, a loop of 100,000 82
  ms; a request to `nearest` through the deployed server took 33 to 105 ms in the log and 70 to 80
  ms at the port, against 10 to 12 ms for a request without a js node. A debug build is about
  eighty times slower. So a js node is for request-sized work; compute-bound bodies would want
  ahead-of-time compilation (F6, open).
- **A black box to replay.** `explain` and view mode show its ports and recorded values, and the
  engine's error, but don't step inside the body.
- **Synchronous and closed.** No `async`, no `fetch`, no state across calls, no imports; a body sees
  only its parameters (a name from the flow around it is a `ReferenceError`, JL4 F4).
- **Some values can't cross**: `bigint`, `Map`, `Set`, `Date`, functions; a record's key order is
  lost on the way out.
- **The bridge bundle carries the engine**: 6 MB, from 1.9 (JL4 §7).
- **Owed** (JL4 §10): the editor's ⓘ for a module node, and the language server marking a body's
  syntax error as `check` does.
- **The engine build is not checked in CI.** No workflow in `.github/workflows/` builds
  `fixtures/modules/js-engine/`: the committed binary is trusted as built, and `build.sh` says a
  change to the engine is built by hand and committed with it.
- **One engine, one language.** Lua, Python and others would each be another engine module under
  the same contract; none is built (JL4 §8).

## 5. What the tests hold

In `crates/holon-module/tests/js_engine.rs`, the engine as a module:
`the_engine_is_a_module_unit` (its import section empty, as core validates),
`a_function_runs_on_its_arguments_and_answers_its_value`,
`records_cross_both_ways_in_key_order_and_undefined_fields_are_left_out`,
`absence_crosses_as_undefined_and_back`, `numbers_cross_as_f64_including_the_non_finite_ones`,
`an_exception_is_an_error_naming_it`, `a_syntax_error_is_an_error_too`,
`kinds_that_cannot_cross_are_errors_naming_the_kind`,
`the_prelude_leaves_no_clock_randomness_io_or_locale` (each removal),
`a_body_that_never_ends_runs_out_of_fuel`, `the_source_alone_is_compiled_and_not_run`; in
`tests/boundary.rs`, `a_call_is_a_function_of_its_module_and_its_inputs` (two calls share nothing,
and the same call answers and costs the same whenever and wherever it is made) and
`what_a_module_answers_is_data_as_it_was_given`; and `what_a_call_costs` (ignored, by hand).

In `holon-module`'s `lib.rs`, the runner: `a_module_with_imports_never_runs`,
`running_out_of_fuel_is_named`, `growing_past_the_memory_budget_is_named`,
`an_answer_claimed_past_its_memory_is_bad_output_not_an_allocation`, `a_trap_is_named`,
`through_the_resolver_a_module_is_a_node_and_a_failure_is_kept`. In `holon-core`'s `module.rs`,
`a_module_that_is_not_one_fails_validation_with_the_reason`,
`the_observer_hears_why_a_module_gave_nothing`.

In `holon-author`: `a_js_node_prints_back_verbatim_lowers_to_the_same_units_and_runs`,
`a_js_node_s_mistakes_are_named_at_their_place` (`print.rs`);
`a_js_bodys_types_are_blanked_in_place` (`ts.rs`); `a_js_body_is_never_touched` (`format.rs`);
`fmt_leaves_a_hash_inside_a_js_body_alone`,
`check_compiles_each_js_body_and_says_a_mistake_at_its_line` (`commands.rs`);
`explain_says_why_a_js_node_gave_nothing` (`agent.rs`). In the server's `permissions.rs`,
`a_module_s_budgets_are_limits_with_ceilings_and_no_defaults`.

`editor/src/samples.test.ts` builds every sample with a js node; the end-to-end sweep
(`sweep.e2e.ts`) deploys each and sends it a request, and `inbox-report reads a file from a bound
folder…` runs one and verifies its runs.

**Gaps.** No test runs `nearest`'s route end to end; nothing in CI rebuilds the engine and
compares it with the committed binary; the cost measurement asserts only an order.

## 6. History

| Release or commit | Change |
|---|---|
| rc.17 | A module is a unit (`UnitBody::Module`), underneath only |
| rc.18 | Modules run everywhere a flow runs; JavaScript as one node, on Boa; the `nearest` sample |
| rc.19 | A js node that gives nothing says why; `holon check` compiles each body; documents carry a module by engine and params; the cost measured |
| rc.20 | The `tally` sample; the cost measurement prints wall time, cold and warm |
| rc.22 | Respelled `holon f(a) -> b js { … }`, the lowering and every hash unchanged; a regex literal and a `#` in a body read right; the `guestbook` sample |
| rc.31 | `chess`: the game's rules as a js node at the fuel ceiling |
| `6d062fe`, `9969f8b` | The flow cache shares units, so the engine is held once however many flows place it (C1); the caches count memory (S1) |
