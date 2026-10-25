//! The engine for module bodies (docs/L4s/wasm-module-L4.md §5): runs a WebAssembly module with
//! no imports under a fuel and memory budget, values crossing in core's own encoding.
//!
//! Core knows a module as `UnitBody::Module { bytes }` and asks `Resolver::run_module` to run
//! one; this crate is the answer, kept out of core so core's purity-by-dependency argument holds
//! and the editor's bundle carries an engine only where it places a module. One implementation,
//! compiled twice: native for the host (`holon-host`, `holon`) and wasm32 for the editor's
//! bridge (`holon-wasm`), so design mode runs the real module under the real budget.
//!
//! ## The ABI (§3)
//!
//! - The module exports `memory`, `alloc(len: i32) -> i32` and `run(ptr: i32, len: i32) -> i64`.
//! - The engine encodes the inputs as one `List` with `encode_value`, writes it at `alloc(len)`,
//!   calls `run`, and reads the answer at the pointer in the result's high 32 bits, of the
//!   length in its low 32 bits. The answer decodes as one `List` of outputs, one per declared
//!   port; an optional's `none` (`Sum { arm: 1, Unit }`) is an absent output, `some(x)` is `x`.
//! - No JSON, no second format: the one encoding that defines a value's identity defines the ABI,
//!   and a module written in Rust can use `holon-core` itself on the other side.
//!
//! ## Totality and determinism
//!
//! Fuel is metered by `wasmi` (`Config::consume_fuel`); memory growth is answered by a limiter
//! that refuses past the budget. Running out of either, a trap, an answer that doesn't decode:
//! each is a [`RunError`] and, through the hook, an absent output. A module with imports never
//! runs: `validate_module` is asked first, so the engine itself enforces the admission rule. A
//! fresh `Store` per call keeps calls independent; compiled modules are cached by their bytes'
//! hash, which is a cost, not a state.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use holon_core::{decode_value, encode_value, validate_module, Hash, ModuleError, ModuleLimits, Resolver, Unit, Value};
use wasmi_core::LimiterError;
use wasmi::{Config, Engine as Wasmi, Linker, Module, ResourceLimiter, Store, TrapCode};

/// Holon's JavaScript engine module (docs/L4s/js-node-L4.md §4), as built and committed in
/// `fixtures/modules/js-engine/`: a module unit with no imports whose interface is
/// `engine(source: Text, args: List<Any>) -> (result: Record)`. The `js` statement places it.
pub const JS_ENGINE: &[u8] = include_bytes!("../../../fixtures/modules/js-engine/js-engine.wasm");

/// A `js` node's fuel when its text names none. Measured (tests/js_engine.rs, `what_a_call_costs`):
/// a fresh realm and a compiled one-line function cost about 15M instructions under the
/// interpreter, an edit-distance search over eight words 41M, a loop of a hundred thousand
/// iterations 271M. So the module default (10M) can never run a body, and 200M runs real
/// request-sized work with room; a heavier body says `fuel 500M`, up to the host's ceiling.
pub const JS_DEFAULT_FUEL: u64 = 200_000_000;
/// A `js` node's memory when its text names none: the module default (16 MiB). A realm ends
/// at about 2 MiB.
pub const JS_DEFAULT_MEMORY: u64 = holon_core::DEFAULT_MODULE_MEMORY;

/// Why a module didn't answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    /// Not admitted: imports, a missing export, a bad budget, not WebAssembly.
    NotAModule(ModuleError),
    /// The engine refused the binary (its own validation, past the sections core reads).
    Compile(String),
    /// It couldn't be instantiated (a start function trapped, say).
    Instantiate(String),
    /// It ran past its fuel.
    OutOfFuel { fuel: u64 },
    /// It asked for memory past its budget.
    OutOfMemory { memory: u64 },
    /// It trapped: an unreachable, a bounds violation, a division by zero…
    Trap(String),
    /// It answered bytes that aren't a list of values in core's encoding.
    BadOutput(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NotAModule(e) => write!(f, "{e}"),
            RunError::Compile(e) => write!(f, "the engine refused the module: {e}"),
            RunError::Instantiate(e) => write!(f, "the module couldn't start: {e}"),
            RunError::OutOfFuel { fuel } => write!(f, "out of fuel ({fuel} instructions)"),
            RunError::OutOfMemory { memory } => write!(f, "out of memory ({memory} bytes)"),
            RunError::Trap(e) => write!(f, "the module trapped: {e}"),
            RunError::BadOutput(e) => write!(f, "the module's answer isn't a list of values: {e}"),
        }
    }
}

impl std::error::Error for RunError {}

/// What a call cost: the fuel it burned, and the linear memory it ended with (bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cost {
    pub fuel: u64,
    pub memory: u64,
}

/// The memory budget, as `wasmi` asks it on every `memory.grow`: refuses past the limit, and
/// remembers that it did so the failure that follows can be named.
struct Budget {
    bytes: usize,
    refused: bool,
}

impl ResourceLimiter for Budget {
    fn memory_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> Result<bool, LimiterError> {
        let ok = desired <= self.bytes;
        if !ok {
            self.refused = true;
        }
        Ok(ok)
    }
    fn table_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> Result<bool, LimiterError> {
        // A table holds references, not bytes; the same count as a generous cap.
        Ok(desired <= 1 << 20)
    }
    fn instances(&self) -> usize {
        1
    }
    fn tables(&self) -> usize {
        4
    }
    fn memories(&self) -> usize {
        1
    }
}

/// The engine: one `wasmi` engine with fuel metering on, and the modules it has compiled.
/// `Send + Sync`, so one serves a whole server ([`shared`]).
pub struct Engine {
    wasmi: Wasmi,
    compiled: Mutex<BTreeMap<Hash, Module>>,
}

/// The one engine a process needs: built on first use, kept for its life. A compiled module is
/// then compiled once per process, whichever run asks for it.
pub fn shared() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(Engine::new)
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        let mut config = Config::default();
        config.consume_fuel(true);
        Engine { wasmi: Wasmi::new(&config), compiled: Mutex::new(BTreeMap::new()) }
    }

    /// Run `bytes` on `inputs` under `limits`: one output per element of the list it answers.
    pub fn run(&self, bytes: &[u8], limits: &ModuleLimits, inputs: &[Option<Value>]) -> Result<Vec<Option<Value>>, RunError> {
        self.run_measured(bytes, limits, inputs).map(|(outs, _)| outs)
    }

    /// [`run`](Self::run), and what the call cost: the fuel it burned and the memory it ended
    /// with, in bytes. What a run records beside a module's outputs, and how a budget is chosen.
    pub fn run_measured(&self, bytes: &[u8], limits: &ModuleLimits, inputs: &[Option<Value>]) -> Result<(Vec<Option<Value>>, Cost), RunError> {
        validate_module(bytes).map_err(RunError::NotAModule)?;
        let module = self.compile(bytes)?;

        let budget = Budget { bytes: usize::try_from(limits.memory).unwrap_or(usize::MAX), refused: false };
        let mut store = Store::new(&self.wasmi, budget);
        store.limiter(|b| b);
        store.set_fuel(limits.fuel).map_err(|e| RunError::Compile(e.to_string()))?;

        let linker = Linker::<Budget>::new(&self.wasmi);
        let instance = linker.instantiate_and_start(&mut store, &module).map_err(|e| self.failed(e, &store, limits, RunError::Instantiate))?;
        let memory = instance.get_memory(&store, "memory").ok_or(RunError::NotAModule(ModuleError::MissingExport("memory")))?;
        let alloc = instance.get_typed_func::<i32, i32>(&store, "alloc").map_err(|e| RunError::Compile(e.to_string()))?;
        let run = instance.get_typed_func::<(i32, i32), i64>(&store, "run").map_err(|e| RunError::Compile(e.to_string()))?;

        // Inputs: one list, every input present (eval is strict; an engine asked with a hole
        // passes it on as `none`, which a module may read as it likes).
        let items: Vec<Value> = inputs.iter().map(|v| v.clone().unwrap_or(none())).collect();
        let input = encode_value(&Value::List(items)).map_err(|e| RunError::BadOutput(format!("inputs: {e:?}")))?;
        let len = i32::try_from(input.len()).map_err(|_| RunError::BadOutput("inputs too long".into()))?;

        let ptr = alloc.call(&mut store, len).map_err(|e| self.failed(e, &store, limits, RunError::Trap))?;
        memory.write(&mut store, ptr as u32 as usize, &input).map_err(|e| RunError::Trap(format!("alloc gave an address outside memory: {e}")))?;
        let packed = match run.call(&mut store, (ptr, len)) {
            Ok(p) => p,
            Err(e) => {
                let mut failure = self.failed(e, &store, limits, RunError::Trap);
                // A module may export `last_panic() -> i64` (address and length of a message it
                // kept before trapping): then the trap is named by it. Rust's `panic = "abort"`
                // is an `unreachable`, so this is how a module written in Rust says what went wrong.
                if let (RunError::Trap(text), Ok(last)) = (&failure, instance.get_typed_func::<(), i64>(&store, "last_panic"))
                    && let Ok(packed) = last.call(&mut store, ())
                {
                    let (p, n) = (((packed as u64) >> 32) as u32 as usize, (packed as u64 & 0xffff_ffff) as u32 as usize);
                    let mut msg = vec![0u8; n.min(4096)];
                    if n > 0 && memory.read(&store, p, &mut msg).is_ok() {
                        failure = RunError::Trap(format!("{text}: {}", String::from_utf8_lossy(&msg)));
                    }
                }
                return Err(failure);
            }
        };

        let (out_ptr, out_len) = (((packed as u64) >> 32) as u32 as usize, (packed as u64 & 0xffff_ffff) as u32 as usize);
        let mut out = vec![0u8; out_len];
        memory.read(&store, out_ptr, &mut out).map_err(|e| RunError::BadOutput(format!("the answer lies outside memory: {e}")))?;
        if store.data().refused {
            // It asked past its budget and still answered: whatever it answered was computed
            // without the memory it wanted, so the budget is the failure to name.
            return Err(RunError::OutOfMemory { memory: limits.memory });
        }
        let cost = Cost { fuel: limits.fuel.saturating_sub(store.get_fuel().unwrap_or(0)), memory: memory.size(&store) * 65_536 };
        match decode_value(&out).map_err(|e| RunError::BadOutput(format!("{e:?}")))? {
            Value::List(items) => Ok((items.into_iter().map(present).collect(), cost)),
            other => Err(RunError::BadOutput(format!("a {:?}, not a list", other.ty()))),
        }
    }

    fn compile(&self, bytes: &[u8]) -> Result<Module, RunError> {
        let key: Hash = *blake3::hash(bytes).as_bytes();
        if let Some(m) = self.compiled.lock().ok().and_then(|c| c.get(&key).cloned()) {
            return Ok(m);
        }
        let module = Module::new(&self.wasmi, bytes).map_err(|e| RunError::Compile(e.to_string()))?;
        if let Ok(mut c) = self.compiled.lock() {
            c.insert(key, module.clone());
        }
        Ok(module)
    }

    /// Name a failed call: fuel and memory by their budgets, anything else as `other` says.
    fn failed(&self, e: wasmi::Error, store: &Store<Budget>, limits: &ModuleLimits, other: fn(String) -> RunError) -> RunError {
        if e.as_trap_code() == Some(TrapCode::OutOfFuel) {
            RunError::OutOfFuel { fuel: limits.fuel }
        } else if store.data().refused {
            RunError::OutOfMemory { memory: limits.memory }
        } else {
            other(e.to_string())
        }
    }
}

/// `none()`, as core spells it: the optional's second arm, holding the unit.
fn none() -> Value {
    Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) }
}

/// An answered output: `none` is absent, `some(x)` is `x`, anything else is itself.
fn present(v: Value) -> Option<Value> {
    match v {
        Value::Sum { arm: 1, payload } if matches!(*payload, Value::List(ref xs) if xs.is_empty()) => None,
        Value::Sum { arm: 0, payload } => Some(*payload),
        other => Some(other),
    }
}

/// A resolver with an engine: resolves through `real`, runs modules through `engine`, and keeps
/// the last failure so a host can record why a module gave nothing (`explain` prints it).
pub struct WithEngine<'a, R: Resolver> {
    real: &'a R,
    engine: &'a Engine,
    last: RefCell<Option<RunError>>,
}

impl<'a, R: Resolver> WithEngine<'a, R> {
    pub fn new(real: &'a R, engine: &'a Engine) -> Self {
        WithEngine { real, engine, last: RefCell::new(None) }
    }

    /// Why the last module that gave nothing gave nothing, taken (the next call starts clean).
    pub fn take_failure(&self) -> Option<RunError> {
        self.last.borrow_mut().take()
    }
}

impl<R: Resolver> Resolver for WithEngine<'_, R> {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        self.real.resolve(hash)
    }

    fn run_module(&self, bytes: &[u8], limits: &ModuleLimits, inputs: &[Option<Value>]) -> Option<Vec<Option<Value>>> {
        match self.engine.run(bytes, limits, inputs) {
            Ok(outs) => Some(outs),
            Err(e) => {
                *self.last.borrow_mut() = Some(e);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_core::{eval, Edge, MapResolver, Param, Port, Type, UnitBody, BOUNDARY};

    fn n(x: f64) -> Value {
        Value::Number(x)
    }

    /// `run` answers its own input: the outputs are the inputs. `alloc` hands out a fixed
    /// address; the module never grows memory.
    const ECHO: &str = r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) i32.const 1024)
        (func (export "run") (param $ptr i32) (param $len i32) (result i64)
            (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32)) (i64.extend_i32_u (local.get $len)))))"#;

    #[test]
    fn echo_answers_its_inputs_as_its_outputs() {
        let engine = Engine::new();
        let bytes = wat::parse_str(ECHO).unwrap();
        let inputs = [Some(n(2.0)), Some(Value::Text("two".into())), Some(Value::List(vec![n(1.0)]))];
        let outs = engine.run(&bytes, &ModuleLimits::default(), &inputs).unwrap();
        assert_eq!(outs, inputs.to_vec());
        // Compiled once: the second run finds the module in the cache and answers the same.
        assert_eq!(engine.run(&bytes, &ModuleLimits::default(), &inputs).unwrap(), inputs.to_vec());
        assert_eq!(engine.compiled.lock().unwrap().len(), 1);
    }

    #[test]
    fn none_in_the_answer_is_an_absent_output_and_some_is_its_value() {
        let engine = Engine::new();
        let bytes = wat::parse_str(ECHO).unwrap();
        let some_three = Value::Sum { arm: 0, payload: Box::new(n(3.0)) };
        let outs = engine.run(&bytes, &ModuleLimits::default(), &[Some(none()), Some(some_three), Some(n(1.0))]).unwrap();
        assert_eq!(outs, vec![None, Some(n(3.0)), Some(n(1.0))]);
    }

    #[test]
    fn a_module_with_imports_never_runs() {
        let with_import = r#"(module (import "env" "now" (func (result i64))) (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 0)
            (func (export "run") (param i32 i32) (result i64) i64.const 0))"#;
        let err = Engine::new().run(&wat::parse_str(with_import).unwrap(), &ModuleLimits::default(), &[]).unwrap_err();
        assert!(matches!(err, RunError::NotAModule(ModuleError::Imports { .. })), "{err}");
    }

    #[test]
    fn running_out_of_fuel_is_named() {
        let forever = r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 1024)
            (func (export "run") (param i32 i32) (result i64) (loop (br 0)) (i64.const 0)))"#;
        let limits = ModuleLimits { fuel: 10_000, memory: 65536 };
        let err = Engine::new().run(&wat::parse_str(forever).unwrap(), &limits, &[Some(n(1.0))]).unwrap_err();
        assert_eq!(err, RunError::OutOfFuel { fuel: 10_000 });
        assert_eq!(err.to_string(), "out of fuel (10000 instructions)");
    }

    #[test]
    fn growing_past_the_memory_budget_is_named() {
        // Asks for a thousand more pages (64 MiB) under a 1 MiB budget: refused, so `memory.grow`
        // answers -1 and the module answers nothing useful; the budget is what's reported.
        let greedy = r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 1024)
            (func (export "run") (param i32 i32) (result i64) (drop (memory.grow (i32.const 1000))) (i64.const 0)))"#;
        let limits = ModuleLimits { fuel: 1_000_000, memory: 1 << 20 };
        let err = Engine::new().run(&wat::parse_str(greedy).unwrap(), &limits, &[Some(n(1.0))]).unwrap_err();
        assert_eq!(err, RunError::OutOfMemory { memory: 1 << 20 });
        // Under a budget that allows it, the same module grows and answers (nothing decodable:
        // an empty answer at address 0 is not a list of values). Growing costs fuel in proportion
        // to the bytes zeroed, so the roomy budget is roomy in fuel too.
        let roomy = ModuleLimits { fuel: 1_000_000_000, memory: 128 << 20 };
        let roomy_result = Engine::new().run(&wat::parse_str(greedy).unwrap(), &roomy, &[Some(n(1.0))]);
        assert!(matches!(roomy_result, Err(RunError::BadOutput(_))), "{roomy_result:?}");
    }

    #[test]
    fn a_trap_is_named() {
        let trapping = r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 1024)
            (func (export "run") (param i32 i32) (result i64) unreachable))"#;
        let err = Engine::new().run(&wat::parse_str(trapping).unwrap(), &ModuleLimits::default(), &[]).unwrap_err();
        assert!(matches!(err, RunError::Trap(_)), "{err}");
    }

    #[test]
    fn through_the_resolver_a_module_is_a_node_and_a_failure_is_kept() {
        let mut store = MapResolver::new();
        let echo = Unit {
            body: UnitBody::Module { bytes: wat::parse_str(ECHO).unwrap() },
            params: vec![],
            inputs: vec![Port { name: "a".into(), ty: Type::Any }, Port { name: "b".into(), ty: Type::Any }],
            outputs: vec![Port { name: "x".into(), ty: Type::Any }, Port { name: "y".into(), ty: Type::Any }],
        };
        let h = store.insert(echo).unwrap();
        let swap = Unit {
            body: UnitBody::Composite {
                children: vec![h],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 1 },
                    Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                    Edge { from_child: 0, from_port: 1, to_child: BOUNDARY, to_port: 1 },
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "p".into(), ty: Type::Any }, Port { name: "q".into(), ty: Type::Any }],
            outputs: vec![Port { name: "o1".into(), ty: Type::Any }, Port { name: "o2".into(), ty: Type::Any }],
        };
        let engine = Engine::new();
        let with = WithEngine::new(&store, &engine);
        assert_eq!(eval(&swap, &[Some(n(1.0)), Some(n(2.0))], &with).as_slice(), &[Some(n(2.0)), Some(n(1.0))]);
        assert!(with.take_failure().is_none());
        // Without the engine, the same flow is absent: the default hook.
        assert_eq!(eval(&swap, &[Some(n(1.0)), Some(n(2.0))], &store).as_slice(), &[None, None]);
        // A starved module gives nothing, and says why to whoever asks.
        let starved = Unit { params: vec![Param::frozen("fuel", "1")], ..store.resolve(&h).unwrap().clone() };
        assert_eq!(eval(&starved, &[Some(n(1.0)), Some(n(2.0))], &with).as_slice(), &[None, None]);
        assert_eq!(with.take_failure(), Some(RunError::OutOfFuel { fuel: 1 }));
        assert!(with.take_failure().is_none());
    }
}
