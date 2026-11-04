# holon-module

The engine behind module bodies: it runs a WebAssembly module with no imports under a fuel and
memory budget, its values crossing in core's own encoding. One implementation, compiled native for
the server and the command line and to wasm32 for the editor. The evidence is in
[wasm-module-L4](../../L4s/wasm-module-L4.md) (WM, the design) and
[js-node-L4](../../L4s/js-node-L4.md) (JS, the first module: the JavaScript engine).

## 1. What it is

`crates/holon-module/src/lib.rs`, with the js engine's tests in `tests/js_engine.rs`. It depends on
[holon-core](holon-core.md), `wasmi` 2 (default features off; `std`, `validate`, `stable`,
`auto-dispatch`), `wasmi_core` and `blake3`. Core knows a module as `UnitBody::Module { bytes }`,
admits one with `validate_module` and asks `Resolver::run_module` to run it; this crate is the
answer, kept out of core so core can say `Pure` about a module without carrying an engine.

It is used by the server (`crates/holon`: `deploy.rs`, `listeners.rs`, `scheduler.rs`,
`arrivals.rs`, `durable.rs`, `verify.rs` each wrap their resolver in
`WithEngine::new(&resolver, holon_module::shared())`), by [holon-author](holon-author.md)
(`modules.rs` rebuilds the js engine unit, `commands.rs` checks a js body with `compile_js`) and by
[holon-wasm](holon-wasm.md) (`Evaluator::eval`). `holon-host` does not use it.

## 2. How it works

- **The ABI** (WM §3). A module exports `memory`, `alloc(len: i32) -> i32` and
  `run(ptr: i32, len: i32) -> i64`. `Engine::run_measured` encodes the inputs as one `List` with
  `encode_value`, writes it at the address `alloc(len)` gives, calls `run`, and reads the answer at
  the pointer in the result's high 32 bits, of the length in its low 32. The answer decodes with
  `decode_value` as one `List`, one item per declared output: `none` (`Sum { arm: 1, … }`) is an
  absent output, `some(x)` is `x`, anything else is itself (`present`).
- **Admission first.** `validate_module` (core's) runs before the engine compiles anything, so a
  module with an import never runs, whoever built the unit.
- **Budgets.** `Config::consume_fuel` meters instructions; `store.set_fuel(limits.fuel)` sets the
  call's. `Budget`, a `ResourceLimiter`, refuses `memory.grow` past `limits.memory` and remembers
  that it did. Each failure has its name (`RunError`): `NotAModule`, `Compile`, `Instantiate`,
  `OutOfFuel { fuel }`, `OutOfMemory { memory }`, `Trap`, `BadOutput`. A module that asked past its
  memory and still answered is `OutOfMemory`: it computed without what it wanted.
- **The answer is read from inside its memory.** `out_ptr..out_ptr + out_len` is taken from the
  module's memory slice, never allocated to the length the module claims, so a lying length is
  `BadOutput`, not 4 GiB of the host's.
- **A trap can name itself.** A module may export `last_panic() -> i64` (a message it kept); a
  `Trap` then carries the message, up to 4,096 bytes. This is how a module written in Rust, whose
  panic is an `unreachable`, says what went wrong.
- **One engine a process.** `shared()` builds an `Engine` on first use (`OnceLock`). It caches
  compiled modules by the BLAKE3 hash of their bytes (`compiled`, under a `Mutex`), so a module is
  compiled once per process. Every call gets a fresh `Store` and instance: nothing survives a call.
- **Through the resolver.** `WithEngine` resolves through the resolver it wraps (forwarding
  `resolve` and `shape`) and answers `run_module` with the engine, keeping the last `RunError`
  (`take_failure`). Core passes the reason on to `Observer::failed` at the node, which is how a
  run's record holds why a module gave nothing.
- **The js engine.** `JS_ENGINE` is the committed `fixtures/modules/js-engine/js-engine.wasm`
  (Boa, built by that folder's `build.sh`), a module unit
  `engine(source: Text, args: List<Any>) -> (result: Record)`. Each call makes a fresh realm with a
  prelude that removes the clock, randomness, timers, IO and locale, compiles `source`, applies it
  and answers `{ value }` or `{ error }` (JS §4, §5). `compile_js` calls it with the source alone,
  which compiles and runs nothing: `holon check`'s question. `JS_DEFAULT_FUEL` is 200M and
  `JS_DEFAULT_MEMORY` the module default, 16 MiB.

## 3. Why it is this way

- **A module is a unit body, not a primitive with a blob param** (WM §1): it is placed, hashed,
  pinned and inspected as any unit is, with no second blob kind in the store.
- **No imports, read from the bytes** (WM §3): an empty import section is what makes the module
  pure as a fact. A WASI build run under stubs the host provides is refused for the same reason
  (JS §4).
- **The engine out of core** (WM §5): core keeps its four dependencies and its purity argument.
  The default `run_module` answers "this host runs no code", so a host without an engine gives
  nothing for a module, never a wrong answer.
- **`wasmi`**: pure Rust, interpreted (no JIT, no platform code path), fuel metered natively, and
  it builds for wasm32, so the editor runs the same engine the server does. `wasmtime` was refused:
  a JIT, and not a wasm32 target, so the editor would need a second engine (WM §5).
- **Core's encoding is the ABI**: no JSON, no second format. Determinism at the float edge comes
  from the encoding, which writes every `NaN` as one bit pattern (`CANONICAL_NAN`); WM §3.4 says a
  `NaN` fails to decode, which predates that and is not what the code does (JS §5).
- **Budgets are frozen params**, so the same module under two budgets is two units, and a result's
  budget is part of its identity (WM §3.5). The manifest asks for the largest a flow places
  (`module_limits_of`: `Limit(module_fuel=…)`, `Limit(module_memory=…)`); the server's ceilings
  are a hundred times the default fuel and sixteen times the default memory
  (`MODULE_FUEL_CEILING`, `MODULE_MEMORY_CEILING`, `crates/holon/src/permissions.rs`).
- **The js default of 200M fuel** is measured (`what_a_call_costs`, JS §2): a fresh realm and a
  one-line function cost about 15M instructions, an edit-distance search over eight words 41M, a
  loop of a hundred thousand iterations 271M. The module default (10M) can't run a body at all.
- **A fresh store per call**: equal inputs give equal answers (I2), and no realm state can carry
  from one run to the next. Compilation may be cached, state may not (JS §4).

## 4. Reach and limits

- **Slow, by being an interpreter.** A js call is a WebAssembly interpreter running a JavaScript
  interpreter. Unoptimised, `wasmi` took 16 to 30 seconds for a call (the chess sample's rules) that
  takes a tenth of a second in release, past a run's 25-second budget on the debug server the e2e
  tests run; the workspace now builds the `wasmi` crates at `opt-level` 1 in debug, and the call
  takes 1.5 to 2.4 seconds (`0f7af8f`, `Cargo.toml`). Level 3 overflowed a test thread's stack.
- **The js engine is large**: 3.2 MB, 6.5 MB as hex in a preimage (JS §3). Documents carry a module
  by engine and params (`FlowDocument.modules`), not its bytes, because a flow with two budgets
  passed the control plane's 8 MiB body limit.
- **Opaque to replay.** A module is inputs in, outputs out; a replay can't show what happened
  inside one (WM §1).
- **The compiled cache is never emptied.** One entry per distinct module the process has run, kept
  for the process's life. Two runs compiling the same module at once each compile it.
- **What a call cost is measured, not recorded.** `run_measured` returns a `Cost` (fuel burned,
  memory ended with), which only the tests read; `run` drops it.
- **Every input must be present.** Core's module arm gives nothing when an input is absent, before
  the engine is asked; an absent value only reaches a module inside a list or record.
- **Only where a caller wraps its resolver.** `holon-host` doesn't depend on this crate: its
  firing paths run a module only when their caller hands them a `WithEngine`, as the server does.
  Any other resolver gives nothing for a module.
- **The bundle always carries the engine.** The editor's wasm links it, and the js engine module
  with it (`JS_ENGINE`, 3.2 MB), whether or not a flow places a module. No CI job rebuilds the js
  engine; the committed `js-engine.wasm` is trusted as built (`build.sh` says so).

## 5. What the tests hold

In `src/lib.rs`, over tiny modules written as text and assembled with `wat`:

- `echo_answers_its_inputs_as_its_outputs`: the ABI round trip, and a second run served from the
  compiled cache (one entry).
- `none_in_the_answer_is_an_absent_output_and_some_is_its_value`.
- `a_module_with_imports_never_runs`: `NotAModule(Imports)`, from the engine itself.
- `running_out_of_fuel_is_named` (`OutOfFuel { fuel: 10_000 }`, and its words),
  `growing_past_the_memory_budget_is_named`, `a_trap_is_named`,
  `an_answer_claimed_past_its_memory_is_bad_output_not_an_allocation`.
- `through_the_resolver_a_module_is_a_node_and_a_failure_is_kept`: a module inside a composite,
  absent without the engine (the default hook), and a starved one's `OutOfFuel` kept.

In `tests/js_engine.rs`, over the committed engine: `the_engine_is_a_module_unit` (core's
`validate_module` passes it, and it is under 12 MiB), `a_function_runs_on_its_arguments_and_answers_its_value`, the value
mapping (`records_cross_both_ways_in_key_order_and_undefined_fields_are_left_out`,
`absence_crosses_as_undefined_and_back`, `numbers_cross_as_f64_including_the_non_finite_ones`,
`kinds_that_cannot_cross_are_errors_naming_the_kind`), errors (`an_exception_is_an_error_naming_it`,
`a_syntax_error_is_an_error_too`), the prelude
(`the_prelude_leaves_no_clock_randomness_io_or_locale`), determinism (`two_calls_share_nothing`,
`the_same_call_answers_the_same_bytes`), `a_body_that_never_ends_runs_out_of_fuel`,
`the_source_alone_is_compiled_and_not_run`. `what_a_call_costs` is ignored: a measurement, run by
hand (`cargo test -p holon-module --test js_engine -- --ignored --no-capture`); it asserts only that
the three bodies cost more in order.

Core holds admission (`module.rs`: `an_import_is_refused_by_name`,
`the_abi_exports_are_required_with_their_signatures`, `budgets_default_and_parse`,
`a_module_inside_a_composite_is_one_child_like_any_and_pure`).

**Gaps.** No test runs the engine under wasm32: the crate's tests run native, and the editor's
module runs are covered only by the editor's tests and e2e. Nothing checks that the committed
`js-engine.wasm` is what `build.sh` builds. The cache's concurrency is untested.

## 6. History

The crate predates the repository's history (`e49e371`, 2026-10-30). It was built on 2026-10-25,
in WM §7's inches: the body and the hook in core, then this crate on `wasmi`, then the server, the
verifier and the bridge wired, the budgets in the manifest and the policy's ceilings; the js
engine (Boa 0.22) the same day (JS §4). Since, by commit:

- `0f7af8f`: the `wasmi` crates built at `opt-level` 1 in debug builds.
- `440eca0`: `WithEngine` forwards `Resolver::shape`, so a server's kept shapes reach a run that
  wraps its resolver.
