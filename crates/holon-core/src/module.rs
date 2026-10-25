//! What a WebAssembly module must be to be a unit at all (docs/L4s/wasm-module-L4.md §3), read
//! from the binary's own sections, with no dependency: core must be able to say `Pure` about a
//! module without trusting a crate it cannot see into.
//!
//! 1. **No imports.** An empty import section is what makes the module pure as a *fact*: it can
//!    reach no clock, file, network or host memory. Anything it needs arrives as an input.
//! 2. **The ABI's exports, exactly:** one exported memory `memory`; `alloc(len: i32) -> i32`;
//!    `run(ptr: i32, len: i32) -> i64` (high 32 bits the output pointer, low 32 its length).
//!    Other exports are ignored.
//! 3. **Budgets.** `fuel` (instructions) and `memory` (bytes) are frozen params, so the same
//!    module under two budgets is two units and a result's budget is part of its identity. An
//!    open param (a template, `promote.rs`) and an absent one read as the default.
//!
//! The parser walks only what it needs: the type, import, function, memory and export sections.
//! Everything else (code, data, customs) is the engine's to validate when it loads the module.

use std::collections::BTreeMap;

use crate::{Param, Resolver, Unit, UnitBody};

/// The fuel a module gets when its params don't say: enough for real code, done in milliseconds.
pub const DEFAULT_MODULE_FUEL: u64 = 10_000_000;
/// The memory a module may grow to when its params don't say: 16 MiB.
pub const DEFAULT_MODULE_MEMORY: u64 = 16 * 1024 * 1024;

/// The frozen param names the budgets live under, and the manifest's limit names for them.
pub const FUEL_PARAM: &str = "fuel";
pub const MEMORY_PARAM: &str = "memory";
pub const FUEL_LIMIT: &str = "module_fuel";
pub const MEMORY_LIMIT: &str = "module_memory";

/// A module's budgets: how much it may compute and hold per call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleLimits {
    /// Instructions, as the engine meters them.
    pub fuel: u64,
    /// Bytes of linear memory it may grow to.
    pub memory: u64,
}

impl Default for ModuleLimits {
    fn default() -> Self {
        ModuleLimits { fuel: DEFAULT_MODULE_FUEL, memory: DEFAULT_MODULE_MEMORY }
    }
}

/// Why a byte string is not a module unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleError {
    /// Not a WebAssembly binary, or one this reader can't walk (truncated, a bad section).
    Malformed(&'static str),
    /// The import section isn't empty: `first` names the first import as `module.name`.
    Imports { count: usize, first: String },
    /// A required export is missing.
    MissingExport(&'static str),
    /// A required export has the wrong kind or signature.
    WrongSignature { name: &'static str, expected: &'static str },
    /// A budget param isn't a positive whole number.
    BadBudget { name: &'static str, value: String },
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleError::Malformed(why) => write!(f, "not a WebAssembly module: {why}"),
            ModuleError::Imports { count, first } => {
                write!(f, "a module may import nothing; this one imports {count} ({first} first)")
            }
            ModuleError::MissingExport(name) => write!(f, "the module doesn't export {name:?}"),
            ModuleError::WrongSignature { name, expected } => {
                write!(f, "the module's export {name:?} must be {expected}")
            }
            ModuleError::BadBudget { name, value } => {
                write!(f, "the module's {name} must be a positive whole number, not {value:?}")
            }
        }
    }
}

/// The budgets a module unit's params give (defaults where they don't).
pub fn module_limits(params: &[Param]) -> Result<ModuleLimits, ModuleError> {
    let read = |name: &'static str, default: u64| -> Result<u64, ModuleError> {
        match params.iter().find(|p| p.name() == name) {
            Some(Param::Frozen { value, .. }) => match value.trim().parse::<u64>() {
                Ok(n) if n > 0 => Ok(n),
                _ => Err(ModuleError::BadBudget { name, value: value.clone() }),
            },
            Some(Param::Open { .. }) | None => Ok(default),
        }
    };
    Ok(ModuleLimits { fuel: read(FUEL_PARAM, DEFAULT_MODULE_FUEL)?, memory: read(MEMORY_PARAM, DEFAULT_MODULE_MEMORY)? })
}

/// The largest budgets any module in `root`'s closure asks for, as the manifest's limits
/// (`module_fuel`, `module_memory`); empty when the closure places no module. These join the
/// limits a deploy asks the host for, so a person approving sees how much computation a flow
/// may consume per call, and the host's policy may cap them like any limit.
pub fn module_limits_of(root: &Unit, resolver: &impl Resolver) -> BTreeMap<String, u64> {
    let mut acc: Option<ModuleLimits> = None;
    collect(root, resolver, &mut acc);
    let mut out = BTreeMap::new();
    if let Some(l) = acc {
        out.insert(FUEL_LIMIT.to_string(), l.fuel);
        out.insert(MEMORY_LIMIT.to_string(), l.memory);
    }
    out
}

fn collect(unit: &Unit, resolver: &impl Resolver, acc: &mut Option<ModuleLimits>) {
    if let UnitBody::Module { .. } = &unit.body
        && let Ok(l) = module_limits(&unit.params)
    {
        let m = acc.get_or_insert(ModuleLimits { fuel: 0, memory: 0 });
        m.fuel = m.fuel.max(l.fuel);
        m.memory = m.memory.max(l.memory);
    }
    for h in crate::inspect::child_refs(unit) {
        if let Some(u) = resolver.resolve(&h) {
            collect(u, resolver, acc);
        }
    }
}

// ── the binary ──────────────────────────────────────────────────────────────────────────────

const SECTION_TYPE: u8 = 1;
const SECTION_IMPORT: u8 = 2;
const SECTION_FUNCTION: u8 = 3;
const SECTION_MEMORY: u8 = 5;
const SECTION_EXPORT: u8 = 7;

const EXPORT_FUNC: u8 = 0;
const EXPORT_MEMORY: u8 = 2;

const I32: u8 = 0x7F;
const I64: u8 = 0x7E;

/// A function type as the module declares it: parameter and result value types.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FuncType {
    params: Vec<u8>,
    results: Vec<u8>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Result<u8, ModuleError> {
        let b = *self.bytes.get(self.at).ok_or(ModuleError::Malformed("truncated"))?;
        self.at += 1;
        Ok(b)
    }

    /// An unsigned LEB128 of at most 32 bits.
    fn u32(&mut self) -> Result<u32, ModuleError> {
        let mut result: u32 = 0;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            if shift == 28 && (b & 0x70) != 0 {
                return Err(ModuleError::Malformed("a length overflows 32 bits"));
            }
            result |= u32::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift > 28 {
                return Err(ModuleError::Malformed("a length overflows 32 bits"));
            }
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ModuleError> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.bytes.len()).ok_or(ModuleError::Malformed("truncated"))?;
        let s = &self.bytes[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn name(&mut self) -> Result<String, ModuleError> {
        let len = self.u32()? as usize;
        std::str::from_utf8(self.take(len)?).map(str::to_string).map_err(|_| ModuleError::Malformed("a name isn't UTF-8"))
    }

    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }
}

/// What the reader gathers from the sections it walks.
#[derive(Default, Debug)]
struct Sections {
    types: Vec<FuncType>,
    imports: Vec<String>,
    /// Each defined function's type index, in function-index order (imports come first in that
    /// space, and there are none when the module is admitted).
    functions: Vec<u32>,
    memories: usize,
    /// Export name → (kind, index).
    exports: BTreeMap<String, (u8, u32)>,
}

fn read_sections(bytes: &[u8]) -> Result<Sections, ModuleError> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(4)? != b"\0asm" {
        return Err(ModuleError::Malformed("no \\0asm magic"));
    }
    if r.take(4)? != [1, 0, 0, 0] {
        return Err(ModuleError::Malformed("not binary version 1"));
    }
    let mut s = Sections::default();
    while !r.done() {
        let id = r.u8()?;
        let size = r.u32()? as usize;
        let payload = r.take(size)?;
        let mut p = Reader { bytes: payload, at: 0 };
        match id {
            SECTION_TYPE => {
                let n = p.u32()?;
                for _ in 0..n {
                    if p.u8()? != 0x60 {
                        return Err(ModuleError::Malformed("a type isn't a function type"));
                    }
                    let np = p.u32()? as usize;
                    let params = p.take(np)?.to_vec();
                    let nr = p.u32()? as usize;
                    let results = p.take(nr)?.to_vec();
                    s.types.push(FuncType { params, results });
                }
            }
            SECTION_IMPORT => {
                let n = p.u32()?;
                if n > 0 {
                    let module = p.name()?;
                    let field = p.name()?;
                    // The descriptor is skipped: whatever it imports, it imports.
                    s.imports.push(format!("{module}.{field}"));
                }
                // Count them all without decoding descriptors: the count is what we report.
                s.imports.extend(std::iter::repeat_n(String::new(), (n as usize).saturating_sub(s.imports.len())));
            }
            SECTION_FUNCTION => {
                let n = p.u32()?;
                for _ in 0..n {
                    s.functions.push(p.u32()?);
                }
            }
            SECTION_MEMORY => {
                s.memories = p.u32()? as usize;
            }
            SECTION_EXPORT => {
                let n = p.u32()?;
                for _ in 0..n {
                    let name = p.name()?;
                    let kind = p.u8()?;
                    let index = p.u32()?;
                    s.exports.insert(name, (kind, index));
                }
            }
            _ => {}
        }
    }
    Ok(s)
}

/// Whether `bytes` is a module unit: a WebAssembly binary with no imports and the ABI's exports.
pub fn validate_module(bytes: &[u8]) -> Result<(), ModuleError> {
    let s = read_sections(bytes)?;
    if !s.imports.is_empty() {
        return Err(ModuleError::Imports { count: s.imports.len(), first: s.imports[0].clone() });
    }
    match s.exports.get("memory") {
        None => return Err(ModuleError::MissingExport("memory")),
        Some((EXPORT_MEMORY, i)) if (*i as usize) < s.memories => {}
        Some(_) => return Err(ModuleError::WrongSignature { name: "memory", expected: "an exported linear memory" }),
    }
    let func = |name: &'static str, expected: &'static str, params: &[u8], results: &[u8]| -> Result<(), ModuleError> {
        let Some((kind, index)) = s.exports.get(name) else { return Err(ModuleError::MissingExport(name)) };
        let ty = (*kind == EXPORT_FUNC)
            .then(|| s.functions.get(*index as usize))
            .flatten()
            .and_then(|t| s.types.get(*t as usize))
            .ok_or(ModuleError::WrongSignature { name, expected })?;
        if ty.params != params || ty.results != results {
            return Err(ModuleError::WrongSignature { name, expected });
        }
        Ok(())
    };
    func("alloc", "a function (i32) -> i32", &[I32], &[I32])?;
    func("run", "a function (i32, i32) -> i64", &[I32, I32], &[I64])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest module that is a unit: the ABI's exports and nothing else.
    pub(crate) const MINIMAL: &str = r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) i32.const 0)
        (func (export "run") (param i32 i32) (result i64) i64.const 0))"#;

    #[test]
    fn the_minimal_module_is_a_unit() {
        assert_eq!(validate_module(&wat::parse_str(MINIMAL).unwrap()), Ok(()));
    }

    #[test]
    fn an_import_is_refused_by_name() {
        let with_import = r#"(module
            (import "env" "clock" (func (result i64)))
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 0)
            (func (export "run") (param i32 i32) (result i64) i64.const 0))"#;
        assert_eq!(
            validate_module(&wat::parse_str(with_import).unwrap()),
            Err(ModuleError::Imports { count: 1, first: "env.clock".into() })
        );
    }

    #[test]
    fn the_abi_exports_are_required_with_their_signatures() {
        let no_run = r#"(module (memory (export "memory") 1) (func (export "alloc") (param i32) (result i32) i32.const 0))"#;
        assert_eq!(validate_module(&wat::parse_str(no_run).unwrap()), Err(ModuleError::MissingExport("run")));
        let wrong_alloc = r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i64) (result i32) i32.const 0)
            (func (export "run") (param i32 i32) (result i64) i64.const 0))"#;
        assert!(matches!(validate_module(&wat::parse_str(wrong_alloc).unwrap()), Err(ModuleError::WrongSignature { name: "alloc", .. })));
        let no_memory = r#"(module
            (func (export "alloc") (param i32) (result i32) i32.const 0)
            (func (export "run") (param i32 i32) (result i64) i64.const 0))"#;
        assert_eq!(validate_module(&wat::parse_str(no_memory).unwrap()), Err(ModuleError::MissingExport("memory")));
    }

    #[test]
    fn bytes_that_are_not_wasm_are_malformed() {
        assert!(matches!(validate_module(b"hello"), Err(ModuleError::Malformed(_))));
        assert!(matches!(validate_module(b"\0asm\x02\0\0\0"), Err(ModuleError::Malformed(_))));
        let mut truncated = wat::parse_str(MINIMAL).unwrap();
        truncated.truncate(20);
        assert!(matches!(validate_module(&truncated), Err(ModuleError::Malformed(_))));
    }

    use crate::{decode_preimage, encode_preimage, eval, hash_unit, validate, Edge, Hash, MapResolver, Port, Type, Value, BOUNDARY};

    fn module_unit(wat_text: &str, params: Vec<Param>) -> Unit {
        Unit {
            body: UnitBody::Module { bytes: wat::parse_str(wat_text).unwrap() },
            params,
            inputs: vec![Port { name: "a".into(), ty: Type::Number }, Port { name: "b".into(), ty: Type::Number }],
            outputs: vec![Port { name: "sum".into(), ty: Type::Number }, Port { name: "both".into(), ty: Type::list_any() }],
        }
    }

    #[test]
    fn a_module_unit_round_trips_through_its_preimage_and_validates() {
        let unit = module_unit(MINIMAL, vec![Param::frozen("fuel", "1000")]);
        let bytes = encode_preimage(&unit).unwrap();
        assert_eq!(decode_preimage(&bytes).unwrap(), unit);
        assert_eq!(encode_preimage(&decode_preimage(&bytes).unwrap()).unwrap(), bytes);
        let store = MapResolver::new();
        assert_eq!(validate(&unit, &store), Ok(()));
        // The bytes are the identity: one more byte of custom section, another unit.
        let mut other = unit.clone();
        if let UnitBody::Module { bytes } = &mut other.body {
            bytes.extend_from_slice(&[0, 1, 0]); // an empty custom section (id 0, size 1, name len 0)
        }
        assert_ne!(hash_unit(&unit).unwrap(), hash_unit(&other).unwrap());
        // And so is the budget.
        let mut budget = unit.clone();
        budget.params = vec![Param::frozen("fuel", "2000")];
        assert_ne!(hash_unit(&unit).unwrap(), hash_unit(&budget).unwrap());
    }

    #[test]
    fn a_module_that_is_not_one_fails_validation_with_the_reason() {
        let with_import = r#"(module (import "env" "now" (func (result i64))) (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) i32.const 0)
            (func (export "run") (param i32 i32) (result i64) i64.const 0))"#;
        let unit = module_unit(with_import, vec![Param::frozen("memory", "0")]);
        let errors = validate(&unit, &MapResolver::new()).unwrap_err();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].to_string().contains("env.now"), "{}", errors[0]);
        assert!(errors[1].to_string().contains("memory"), "{}", errors[1]);
    }

    /// A resolver with an "engine" that adds its two numbers and lists them: what a real engine
    /// does, without one, so the evaluator's side (strictness, budgets, output distribution) is
    /// tested in core.
    struct Adder(MapResolver);
    impl Resolver for Adder {
        fn resolve(&self, hash: &Hash) -> Option<&Unit> {
            self.0.resolve(hash)
        }
        fn run_module(&self, bytes: &[u8], limits: &ModuleLimits, inputs: &[Option<Value>]) -> Option<Vec<Option<Value>>> {
            assert!(validate_module(bytes).is_ok());
            if limits.fuel < 10 {
                return None; // "out of fuel"
            }
            let (Some(Value::Number(a)), Some(Value::Number(b))) = (&inputs[0], &inputs[1]) else { return None };
            Some(vec![Some(Value::Number(a + b)), Some(Value::List(vec![Value::Number(*a), Value::Number(*b)]))])
        }
    }

    #[test]
    fn eval_runs_a_module_through_the_resolver_strictly_and_per_output() {
        let unit = module_unit(MINIMAL, vec![]);
        let adder = Adder(MapResolver::new());
        let two = |a: f64, b: f64| vec![Some(Value::Number(a)), Some(Value::Number(b))];
        let outs = eval(&unit, &two(2.0, 3.0), &adder);
        assert_eq!(outs.as_slice(), &[Some(Value::Number(5.0)), Some(Value::List(vec![Value::Number(2.0), Value::Number(3.0)]))]);
        // Strict: an absent input makes every output absent, and the engine isn't asked.
        assert_eq!(eval(&unit, &[Some(Value::Number(2.0)), None], &adder).as_slice(), &[None, None]);
        // A budget run out is absence, one slot per declared output.
        let starved = Unit { params: vec![Param::frozen("fuel", "1")], ..unit.clone() };
        assert_eq!(eval(&starved, &two(2.0, 3.0), &adder).as_slice(), &[None, None]);
        // A resolver without an engine makes a module absent: the default hook.
        assert_eq!(eval(&unit, &two(2.0, 3.0), &MapResolver::new()).as_slice(), &[None, None]);
    }

    #[test]
    fn a_module_inside_a_composite_is_one_child_like_any_and_pure() {
        let mut store = Adder(MapResolver::new());
        let module = module_unit(MINIMAL, vec![Param::frozen("fuel", "5000"), Param::frozen("memory", "65536")]);
        let h = store.0.insert(module).unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![h],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 1 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "x".into(), ty: Type::Number }, Port { name: "y".into(), ty: Type::Number }],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        assert_eq!(validate(&comp, &store.0), Ok(()));
        assert!(crate::capabilities(&comp, &store.0).is_pure(), "a module asks for nothing");
        assert_eq!(eval(&comp, &[Some(Value::Number(1.0)), Some(Value::Number(2.0))], &store).as_slice(), &[Some(Value::Number(3.0))]);
        let limits = module_limits_of(&comp, &store.0);
        assert_eq!(limits.get(FUEL_LIMIT), Some(&5000));
        assert_eq!(limits.get(MEMORY_LIMIT), Some(&65536));
        assert!(module_limits_of(&store.0.resolve(&h).unwrap().clone(), &store.0).contains_key(FUEL_LIMIT));
        assert!(module_limits_of(&Unit { body: UnitBody::Primitive { name: "add".into() }, params: vec![], inputs: vec![], outputs: vec![] }, &store.0).is_empty());
    }

    #[test]
    fn budgets_default_and_parse() {
        assert_eq!(module_limits(&[]).unwrap(), ModuleLimits::default());
        let given = [Param::frozen("fuel", "50000000"), Param::frozen("memory", "1048576")];
        assert_eq!(module_limits(&given).unwrap(), ModuleLimits { fuel: 50_000_000, memory: 1_048_576 });
        let open = [Param::Open { name: "fuel".into(), bind: "F".into() }];
        assert_eq!(module_limits(&open).unwrap().fuel, DEFAULT_MODULE_FUEL);
        assert!(matches!(module_limits(&[Param::frozen("fuel", "0")]), Err(ModuleError::BadBudget { name: "fuel", .. })));
        assert!(matches!(module_limits(&[Param::frozen("memory", "lots")]), Err(ModuleError::BadBudget { name: "memory", .. })));
    }
}
