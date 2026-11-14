//! A differential harness for the evaluator (`eval.rs`, `primitives.rs`), through the public API
//! only, so it compiles against any version of those two files from the kept children on
//! (`Shapes::kept`) and can compare them case by case.
//!
//! A deterministic generator builds "worlds" (a resolver holding leaves, composites nested up to
//! four levels, and `map`s and `fold`s of them, bare ones included) and, in each, cases: a unit and
//! inputs. Each case is evaluated several ways (plain `eval`, `eval_observed` under a recording
//! observer that answers requests with nothing / values / not yet, and one that also stands in for
//! nodes as `holon verify` does, `eval_one`, the same unit at a reused address next to a mutant of
//! it) and everything seen (outputs and every observer call, in order) is hashed. Two versions are
//! compared by those hashes; within one version each case is also checked against the eager
//! oracle (`eval_eager`, with `--features eager-oracle`) and against itself (plain `eval` against
//! the observed run, a repeat against the first run).
//!
//! Worlds differ by seed: odd seeds give the resolver `Shapes`, which keep each composite's
//! children too when seed % 4 == 1 (`Shapes::kept`); seed % 4 == 3 adds edges
//! `validate` refuses (ports out of range, missing children, duplicates, cycles, leaves fed at a
//! port they don't declare); seed % 4 == 1 adds leaves declaring a port more than they use, which
//! `validate` takes; seeds 0 and 2 (mod 4) hold catalog interfaces and valid edges only.
//!
//! The `audit_*` tests at the end are the divergences found, reduced, each expecting what the
//! evaluator before giving (b8048e9) gave (and, where it reads only declared ports, the eager oracle).
//!
//! Both sides, compared, in one command (from the repository root):
//!
//!     sh crates/holon-core/tests/differential.sh [BEFORE_REF]     # default BEFORE_REF: main
//!
//! By hand: `HOLON_DIFF_RECORD=<file>` writes `<case id> <hash>` per case;
//! `HOLON_DIFF_DUMP=<id,id,...|@file>` with `HOLON_DIFF_DUMP_OUT=<file>` writes those cases in
//! full; `HOLON_DIFF_WORLDS`, `HOLON_DIFF_FIRST_WORLD`, `HOLON_DIFF_CASES` size the run;
//! `HOLON_DIFF_SUMMARY=<file>` writes the coverage counts. Without `HOLON_DIFF_RECORD` the test
//! runs a small sample and fails on any self-consistency mismatch, or eager mismatch in a case of
//! catalog interfaces and valid edges (where a leaf declares an odd interface or an edge is
//! unvalidated, the walk and the oracle already differed before; those are counted apart).
//!
//!     HOLON_DIFF_WORLDS=120 HOLON_DIFF_RECORD=after.txt cargo test -p holon-core --release \
//!         --features eager-oracle --test differential -- --exact differential_record --nocapture

// The generator's nested `if`s spell out the order its random draws are made in, which every
// recorded case depends on.
#![allow(clippy::collapsible_if, clippy::manual_is_multiple_of)]

use holon_core::{
    eval, eval_observed, eval_one, hash_unit, hex, interface_of, Answer, Edge, Hash, ModuleLimits, ModuleRun, Observer, Param, Port, Resolved, Resolver, Shape,
    Shapes, Slot, Type, Unit, UnitBody, Value, BOUNDARY,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::fmt::Write as _;

// ───────────────────────────── randomness ─────────────────────────────

#[derive(Clone)]
struct Rng(u64);

impl Rng {
    fn new(a: u64, b: u64) -> Rng {
        let mut r = Rng(a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ 0x2545_F491_4F6C_DD1D);
        r.next();
        r
    }
    /// splitmix64
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }
}

/// A stable 64-bit digest of `text` (the observer's answers and stand-ins depend on it only).
fn digest(text: &str) -> u64 {
    let h = blake3::hash(text.as_bytes());
    u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap())
}

// ───────────────────────────── values ─────────────────────────────

fn num(n: f64) -> Value {
    Value::Number(n)
}
fn txt(s: &str) -> Value {
    Value::Text(s.into())
}
fn list(xs: Vec<Value>) -> Value {
    Value::List(xs)
}
fn rec(kv: &[(&str, Value)]) -> Value {
    Value::Record(kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}

const NUMBERS: &[f64] = &[0.0, -0.0, 1.0, 2.0, -3.5, 0.25, 7.0, 3.0];
const TEXTS: &[&str] = &["", "a", "b", "id", "\u{e9}\u{20ac}\u{1f600}", "a,b", "x y", "\u{fc}", "status", "{\"a\":1}"];
const KEYS: &[&str] = &["a", "b", "id", "x", "status"];

/// A value of any kind, nested at most `depth` deep.
fn value(rng: &mut Rng, depth: u32) -> Value {
    match rng.below(if depth == 0 { 6 } else { 11 }) {
        0 | 1 => num(rng.pick(NUMBERS)),
        2 => Value::Boolean(rng.chance(50)),
        3 | 4 => txt(rng.pick(TEXTS)),
        5 => {
            if rng.chance(50) {
                Value::Sum { arm: 0, payload: Box::new(num(rng.pick(NUMBERS))) }
            } else if rng.chance(90) {
                Value::Sum { arm: 1, payload: Box::new(list(vec![])) }
            } else {
                Value::Effect { sink: "egress".into(), payload: Box::new(txt("e")) }
            }
        }
        6..=8 => list((0..rng.below(4)).map(|_| value(rng, depth - 1)).collect()),
        _ => Value::Record((0..rng.below(4)).map(|_| (rng.pick(KEYS).to_string(), value(rng, depth - 1))).collect()),
    }
}

/// A value worth giving (a list, a record or a text) most of the time.
fn heavy(rng: &mut Rng) -> Value {
    match rng.below(10) {
        0..=3 => list((0..rng.below(4)).map(|_| value(rng, 1)).collect()),
        4..=6 => Value::Record((0..rng.below(4)).map(|_| (rng.pick(KEYS).to_string(), value(rng, 1))).collect()),
        7 | 8 => txt(rng.pick(TEXTS)),
        _ => value(rng, 2),
    }
}

/// An input: absent sometimes, heavy often.
fn arg(rng: &mut Rng) -> Option<Value> {
    match rng.below(100) {
        0..=9 => None,
        10..=64 => Some(heavy(rng)),
        _ => Some(value(rng, 2)),
    }
}

/// A list for a `map` or `fold`: of items of one kind mostly, sometimes not a list or absent.
fn items(rng: &mut Rng) -> Option<Value> {
    match rng.below(100) {
        0..=3 => None,
        4..=9 => Some(value(rng, 1)),
        _ => {
            let n = if rng.chance(15) { rng.below(8) } else { rng.below(4) };
            let kind = rng.below(4);
            Some(list(
                (0..n)
                    .map(|_| match kind {
                        0 => list((0..rng.below(3)).map(|_| value(rng, 0)).collect()),
                        1 => Value::Record((0..rng.below(3)).map(|_| (rng.pick(KEYS).to_string(), value(rng, 0))).collect()),
                        2 => txt(rng.pick(TEXTS)),
                        _ => value(rng, 1),
                    })
                    .collect(),
            ))
        }
    }
}

// ───────────────────────────── the resolver ─────────────────────────────

/// Units by hash, each shared so it never moves (a `Shapes` table may hold their addresses, and
/// keep them as children), with a fake module engine.
struct Res {
    units: HashMap<Hash, Arc<Unit>>,
    shapes: Option<Shapes>,
}

impl Res {
    fn insert(&mut self, unit: Unit) -> Option<Hash> {
        let h = hash_unit(&unit).ok()?;
        self.units.entry(h).or_insert_with(|| Arc::new(unit));
        Some(h)
    }
}

impl Resolver for Res {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        self.units.get(hash).map(|b| &**b)
    }
    fn shape(&self, unit: &Unit) -> Option<&Shape> {
        self.shapes.as_ref()?.get(unit)
    }
    fn resolved(&self, unit: &Unit) -> Option<&Resolved> {
        self.shapes.as_ref()?.resolved(unit)
    }
    fn run_module(&self, bytes: &[u8], _limits: &ModuleLimits, inputs: &[Option<Value>]) -> ModuleRun {
        let outputs = match bytes.first() {
            Some(0) => Err(format!("trapped on {} inputs", inputs.len())),
            _ => Ok(vec![Some(list(inputs.iter().flatten().cloned().collect())), None, Some(num(bytes.len() as f64))]),
        };
        ModuleRun { outputs, burned: 1, out_of_fuel: false }
    }
}

// ───────────────────────────── features, for the coverage counts ─────────────────────────────

const F_GUARD_PASS: u64 = 1 << 0;
const F_GUARD_COND: u64 = 1 << 1;
const F_COALESCE: u64 = 1 << 2;
const F_SELECT: u64 = 1 << 3;
const F_MULTI_OUT: u64 = 1 << 4;
const F_CHILD_PORT1: u64 = 1 << 5;
const F_OUT_FROM_INPUT: u64 = 1 << 6;
const F_TWO_OUT_SAME: u64 = 1 << 7;
const F_TWO_OUT_VIA_CHOICES: u64 = 1 << 8;
const F_SPECULATIVE: u64 = 1 << 9;
const F_REQUEST: u64 = 1 << 10;
const F_SINK_TRIGGER_WIRED: u64 = 1 << 11;
const F_SINK_TRIGGER_OPEN: u64 = 1 << 12;
const F_OPTIONAL_WIRED: u64 = 1 << 13;
const F_MODULE: u64 = 1 << 14;
const F_INVALID_EDGE: u64 = 1 << 15;
const F_MAP: u64 = 1 << 16;
const F_FOLD: u64 = 1 << 17;
const F_MAP_FN_COMPOSITE: u64 = 1 << 18;
const F_FOLD_FN_COMPOSITE: u64 = 1 << 19;
const F_MAP_IN_MAP: u64 = 1 << 20;
const F_MAP_IN_FOLD: u64 = 1 << 21;
const F_FOLD_IN_MAP: u64 = 1 << 22;
const F_FOLD_IN_FOLD: u64 = 1 << 23;
const F_BARE_FN: u64 = 1 << 24;
const F_EXTRAS: u64 = 1 << 25;
const F_CONST: u64 = 1 << 26;
const F_COMPOSITE_CHILD: u64 = 1 << 27;
const F_SINK_NO_TRIGGER: u64 = 1 << 28;
const F_SHARED_BUILT: u64 = 1 << 29;
const F_TWO_OUT_INPUT: u64 = 1 << 30;
const F_ODD_IFACE: u64 = 1 << 31;

const FLAG_NAMES: &[(u64, &str)] = &[
    (F_GUARD_PASS, "guard, pass-through (const true condition)"),
    (F_GUARD_COND, "guard, non-constant condition"),
    (F_COALESCE, "coalesce"),
    (F_SELECT, "select"),
    (F_MULTI_OUT, "multi-output composite"),
    (F_CHILD_PORT1, "reads output >= 1 of a multi-output child"),
    (F_OUT_FROM_INPUT, "output fed straight from an input"),
    (F_TWO_OUT_SAME, "two outputs read the same value directly"),
    (F_TWO_OUT_INPUT, "two outputs read the same input"),
    (F_TWO_OUT_VIA_CHOICES, "two outputs read one value through different choices"),
    (F_SHARED_BUILT, "a value read by a builder and another reader"),
    (F_SPECULATIVE, "speculative pattern (builder + coalesce fallback on the same value)"),
    (F_REQUEST, "request asking the host"),
    (F_SINK_TRIGGER_WIRED, "sink, trigger wired"),
    (F_SINK_TRIGGER_OPEN, "sink, trigger unwired"),
    (F_SINK_NO_TRIGGER, "sink without a trigger port"),
    (F_OPTIONAL_WIRED, "optional port wired (held_when_absent)"),
    (F_MODULE, "module (fake engine)"),
    (F_CONST, "const"),
    (F_COMPOSITE_CHILD, "composite nested in a composite"),
    (F_MAP, "map"),
    (F_FOLD, "fold"),
    (F_MAP_FN_COMPOSITE, "map of a composite"),
    (F_FOLD_FN_COMPOSITE, "fold of a composite"),
    (F_MAP_IN_MAP, "map in map"),
    (F_MAP_IN_FOLD, "map in fold"),
    (F_FOLD_IN_MAP, "fold in map"),
    (F_FOLD_IN_FOLD, "fold in fold"),
    (F_BARE_FN, "map/fold of a bare map/fold"),
    (F_EXTRAS, "map/fold with extras"),
    (F_INVALID_EDGE, "unvalidated edge (port out of range, missing child, duplicate)"),
    (F_ODD_IFACE, "leaf declaring a port more than it reads or produces (validate takes it)"),
];

/// The leaves `primitives::keeps` marks, in the harness's own copy of its table.
const KEEPS: &[&str] = &[
    "append", "extend", "cons", "tail", "insert", "with_field", "without_field", "remove", "concat", "select", "head", "get", "field", "lookup", "list", "pair",
    "some", "default",
];

fn keeps(name: &str, port: usize) -> bool {
    match name {
        "append" | "extend" | "cons" | "with_field" | "pair" => port < 2,
        "insert" => port < 3,
        "select" => port == 1 || port == 2,
        "tail" | "without_field" | "remove" | "concat" | "head" | "get" | "field" | "lookup" | "some" | "default" => port == 0,
        "list" => true,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Feat {
    flags: u64,
    /// Flags inside a map's or fold's function.
    in_fn: u64,
    leaves: u32,
    leaves_in_fn: u32,
    depth: u8,
    /// Every map/fold in the closure has extras (and there is one).
    extras_every: bool,
}

impl Feat {
    fn join(&mut self, o: &Feat) {
        self.flags |= o.flags;
        self.in_fn |= o.in_fn;
        self.leaves |= o.leaves;
        self.leaves_in_fn |= o.leaves_in_fn;
    }
    fn has_hof(&self) -> bool {
        self.flags & (F_MAP | F_FOLD) != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Class {
    Builder,
    Choice,
    Reader,
    Const,
    Request,
    Sink,
    Module,
    Composite,
    Map,
    Fold,
}

#[derive(Clone)]
struct Entry {
    h: Hash,
    unit: Unit,
    n_in: usize,
    n_out: usize,
    name: String,
    class: Class,
    feat: Feat,
}

// ───────────────────────────── the leaves ─────────────────────────────

/// (pool name, leaf, params, arity for a variadic one, class)
type LeafSpec = (&'static str, &'static str, &'static [(&'static str, &'static str)], Option<usize>, Class);

const LEAVES: &[LeafSpec] = &[
    ("append", "append", &[], None, Class::Builder),
    ("extend", "extend", &[], None, Class::Builder),
    ("cons", "cons", &[], None, Class::Builder),
    ("tail", "tail", &[], None, Class::Builder),
    ("insert", "insert", &[], None, Class::Builder),
    ("with_field_a", "with_field", &[("name", "a")], None, Class::Builder),
    ("with_field_id", "with_field", &[("name", "id")], None, Class::Builder),
    ("without_field", "without_field", &[("name", "a")], None, Class::Builder),
    ("remove", "remove", &[], None, Class::Builder),
    ("concat2", "concat", &[], Some(2), Class::Builder),
    ("concat3", "concat", &[], Some(3), Class::Builder),
    ("concat1", "concat", &[], Some(1), Class::Builder),
    ("select", "select", &[], None, Class::Builder),
    ("head", "head", &[], None, Class::Builder),
    ("get", "get", &[], None, Class::Builder),
    ("field_a", "field", &[("name", "a")], None, Class::Builder),
    ("field_status", "field", &[("name", "status")], None, Class::Builder),
    ("lookup", "lookup", &[], None, Class::Builder),
    ("list2", "list", &[], Some(2), Class::Builder),
    ("list3", "list", &[], Some(3), Class::Builder),
    ("list1", "list", &[], Some(1), Class::Builder),
    ("pair", "pair", &[], None, Class::Builder),
    ("some", "some", &[], None, Class::Builder),
    ("default_n", "default", &[("type", "Number"), ("value", "7")], None, Class::Builder),
    ("default_t", "default", &[("type", "Text"), ("value", "d\u{e9}")], None, Class::Builder),
    ("guard", "guard", &[], None, Class::Choice),
    ("coalesce", "coalesce", &[], None, Class::Choice),
    ("length", "length", &[], None, Class::Reader),
    ("is_empty", "is_empty", &[], None, Class::Reader),
    ("equals", "equals", &[], None, Class::Reader),
    ("is_truthy", "is_truthy", &[], None, Class::Reader),
    ("not", "not", &[], None, Class::Reader),
    ("and", "and", &[], None, Class::Reader),
    ("or", "or", &[], None, Class::Reader),
    ("add", "add", &[], None, Class::Reader),
    ("subtract", "subtract", &[], None, Class::Reader),
    ("divide", "divide", &[], None, Class::Reader),
    ("compare", "compare", &[("op", "<")], None, Class::Reader),
    ("text_length", "text_length", &[], None, Class::Reader),
    ("join", "join", &[], None, Class::Reader),
    ("slice", "slice", &[], None, Class::Reader),
    ("split", "split", &[], None, Class::Reader),
    ("uppercase", "uppercase", &[], None, Class::Reader),
    ("to_text", "to_text", &[], None, Class::Reader),
    ("as_text", "as_text", &[], None, Class::Reader),
    ("keys", "keys", &[], None, Class::Reader),
    ("json_stringify", "json_stringify", &[], None, Class::Reader),
    ("json_parse", "json_parse", &[], None, Class::Reader),
    ("index_of", "index_of", &[], None, Class::Reader),
    ("sum", "sum", &[], None, Class::Reader),
    ("find", "find", &[("name", "a")], None, Class::Reader),
    ("none", "none", &[], None, Class::Reader),
    ("record", "record", &[], None, Class::Reader),
    ("range", "range", &[], None, Class::Reader),
    ("T", "const", &[("type", "Boolean"), ("value", "true")], None, Class::Const),
    ("F", "const", &[("type", "Boolean"), ("value", "false")], None, Class::Const),
    ("c_num", "const", &[("type", "Number"), ("value", "2")], None, Class::Const),
    ("c_negzero", "const", &[("type", "Number"), ("value", "-0")], None, Class::Const),
    ("c_text", "const", &[("type", "Text"), ("value", "\u{e9}a")], None, Class::Const),
    ("c_empty", "const", &[("type", "Text"), ("value", "")], None, Class::Const),
    ("c_key", "const", &[("type", "Text"), ("value", "a")], None, Class::Const),
    ("http_request", "http_request", &[], None, Class::Request),
    ("collection_get", "collection_get", &[], None, Class::Request),
    ("collection_list", "collection_list", &[], None, Class::Request),
    ("await", "await", &[], None, Class::Request),
    ("store_write", "store_write", &[], None, Class::Sink),
    ("collection_put", "collection_put", &[], None, Class::Sink),
    ("egress", "egress", &[], None, Class::Sink),
    ("egress_legacy", "egress", &[], None, Class::Sink),
];

fn port(name: &str) -> Port {
    Port { name: name.into(), ty: Type::Any }
}

fn leaf_unit(spec: &LeafSpec) -> Unit {
    let (pool_name, name, params, arity, _) = *spec;
    let mut unit = interface_of(name).unwrap_or_else(|| panic!("no leaf {name}"));
    if !params.is_empty() {
        unit.params = params.iter().map(|&(k, v)| Param::frozen(k, v)).collect();
    }
    if let Some(n) = arity {
        unit.inputs = (0..n).map(|i| port(&format!("i{i}"))).collect();
    }
    if pool_name == "egress_legacy" {
        unit.inputs.retain(|p| p.name != "trigger");
    }
    unit
}

fn module_unit(fails: bool, n_in: usize, n_out: usize) -> Unit {
    Unit {
        body: UnitBody::Module { bytes: if fails { vec![0, 1] } else { vec![1, 2, 3] } },
        params: vec![],
        inputs: (0..n_in).map(|i| port(&format!("m{i}"))).collect(),
        outputs: (0..n_out).map(|i| port(&format!("o{i}"))).collect(),
    }
}

// ───────────────────────────── a world ─────────────────────────────

struct World {
    seed: u64,
    res: Res,
    pool: Vec<Entry>,
    by_name: HashMap<String, usize>,
    builders: Vec<usize>,
    choices: Vec<usize>,
    readers: Vec<usize>,
    consts: Vec<usize>,
    requests: Vec<usize>,
    sinks: Vec<usize>,
    modules: Vec<usize>,
    /// Composites and map/fold units by depth (index = depth), depth 0 being bare wrappers of leaves.
    deep: Vec<Vec<usize>>,
    /// Composites by depth, for wrapping.
    fns: Vec<Vec<usize>>,
    /// Map/fold units (any depth).
    hofs: Vec<usize>,
    /// Leaves with a nonstandard declared interface (unvalidated worlds only).
    odd: Vec<usize>,
    /// Leaves declaring a port more than they read or produce, which `validate` takes (worlds
    /// with seed % 4 == 1 only, so that the other worlds' cases stay as they were).
    valid_odd: Vec<usize>,
    unvalidated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Src {
    In(u32),
    Ch(u32, u32),
    Unwired,
}

/// A composite under construction.
struct B {
    n_in: usize,
    children: Vec<usize>,
    edges: Vec<Edge>,
    outs: Vec<Src>,
}

impl B {
    fn new(n_in: usize) -> B {
        B { n_in, children: Vec::new(), edges: Vec::new(), outs: Vec::new() }
    }
    fn add(&mut self, entry: usize, srcs: &[Src]) -> u32 {
        let c = self.children.len() as u32;
        self.children.push(entry);
        for (p, s) in srcs.iter().enumerate() {
            match *s {
                Src::In(i) => self.edges.push(Edge { from_child: BOUNDARY, from_port: i, to_child: c, to_port: p as u32 }),
                Src::Ch(fc, fp) => self.edges.push(Edge { from_child: fc, from_port: fp, to_child: c, to_port: p as u32 }),
                Src::Unwired => {}
            }
        }
        c
    }
    fn out(&mut self, s: Src) {
        self.outs.push(s);
    }
}

impl World {
    fn new(seed: u64) -> World {
        let mut w = World {
            seed,
            res: Res { units: HashMap::new(), shapes: None },
            pool: Vec::new(),
            by_name: HashMap::new(),
            builders: Vec::new(),
            choices: Vec::new(),
            readers: Vec::new(),
            consts: Vec::new(),
            requests: Vec::new(),
            sinks: Vec::new(),
            modules: Vec::new(),
            deep: vec![Vec::new(); 6],
            fns: vec![Vec::new(); 6],
            hofs: Vec::new(),
            odd: Vec::new(),
            valid_odd: Vec::new(),
            unvalidated: seed % 4 == 3,
        };
        for spec in LEAVES {
            let unit = leaf_unit(spec);
            let mut feat = Feat::default();
            if let Some(i) = KEEPS.iter().position(|&k| k == spec.1) {
                feat.leaves |= 1 << i;
            }
            feat.flags |= match spec.4 {
                Class::Const => F_CONST,
                Class::Request => F_REQUEST,
                Class::Choice if spec.1 == "coalesce" => F_COALESCE,
                Class::Builder if spec.1 == "select" => F_SELECT,
                _ => 0,
            };
            let e = w.push(unit, spec.0, spec.4, feat);
            match spec.4 {
                Class::Builder => w.builders.push(e),
                Class::Choice => w.choices.push(e),
                Class::Reader => w.readers.push(e),
                Class::Const => w.consts.push(e),
                Class::Request => w.requests.push(e),
                Class::Sink => w.sinks.push(e),
                _ => {}
            }
        }
        // Leaves declaring an output more than they produce (`validate` reads the declared ones):
        // for the unvalidated worlds only.
        for name in ["guard", "coalesce", "append", "tail"] {
            let mut unit = interface_of(name).unwrap();
            unit.outputs.push(port("extra_out"));
            let class = if name == "guard" || name == "coalesce" { Class::Choice } else { Class::Builder };
            let mut feat = Feat { flags: F_INVALID_EDGE, ..Feat::default() };
            if let Some(i) = KEEPS.iter().position(|&k| k == name) {
                feat.leaves |= 1 << i;
            }
            let e = w.push(unit, &format!("{name}_2out"), class, feat);
            w.odd.push(e);
        }
        // And leaves declaring an input fewer than they read: an edge into the undeclared port is
        // one `validate` refuses, but the walk reads a choice's second port whatever it declares.
        for name in ["guard", "coalesce", "append"] {
            let mut unit = interface_of(name).unwrap();
            unit.inputs.truncate(1);
            let class = if name == "append" { Class::Builder } else { Class::Choice };
            let e = w.push(unit, &format!("{name}_1in"), class, Feat { flags: F_INVALID_EDGE, ..Feat::default() });
            w.odd.push(e);
        }
        // Validated units may declare more than a leaf uses: an output it never produces (read,
        // it is absent), an input it never reads (wired, nothing reads it there).
        if seed % 4 == 1 {
            for (name, extra_out) in [("guard", true), ("coalesce", true), ("append", true), ("tail", true), ("guard", false), ("coalesce", false), ("append", false), ("tail", false), ("select", false)] {
                let mut unit = interface_of(name).unwrap();
                if extra_out {
                    unit.outputs.push(port("extra_out"));
                } else {
                    unit.inputs.push(port("extra_in"));
                }
                let class = if name == "guard" || name == "coalesce" { Class::Choice } else { Class::Builder };
                let mut feat = Feat { flags: F_ODD_IFACE, ..Feat::default() };
                if let Some(i) = KEEPS.iter().position(|&k| k == name) {
                    feat.leaves |= 1 << i;
                }
                let e = w.push(unit, &format!("{name}_{}", if extra_out { "2out" } else { "xin" }), class, feat);
                w.valid_odd.push(e);
            }
        }
        // A child hash nothing resolves (for unvalidated units only).
        {
            let unit = Unit { body: UnitBody::Primitive { name: "ghost".into() }, params: vec![], inputs: vec![port("a"), port("b")], outputs: vec![port("out")] };
            let e = w.pool.len();
            w.pool.push(Entry { h: [0xEE; 32], n_in: 2, n_out: 1, unit, name: "ghost".into(), class: Class::Reader, feat: Feat { flags: F_INVALID_EDGE, ..Feat::default() } });
            w.by_name.insert("ghost".into(), e);
        }
        for (fails, n_in, n_out) in [(false, 2, 2), (true, 1, 1), (false, 1, 3)] {
            let e = w.push(module_unit(fails, n_in, n_out), "module", Class::Module, Feat { flags: F_MODULE, ..Feat::default() });
            w.modules.push(e);
        }
        // Bare maps and folds of leaves, and of bare maps and folds.
        let mut rng = Rng::new(seed, u64::MAX);
        for (hof, leaf, extras) in [
            ("map", "append", 1),
            ("map", "cons", 1),
            ("map", "tail", 0),
            ("map", "head", 0),
            ("map", "concat2", 1),
            ("map", "with_field_a", 1),
            ("map", "some", 0),
            ("map", "to_text", 0),
            ("map", "field_a", 0),
            ("map", "pair", 1),
            ("map", "list2", 1),
            ("map", "insert", 2),
            ("map", "get", 1),
            ("map", "length", 0),
            ("map", "extend", 1),
            ("map", "default_n", 0),
            ("fold", "append", 0),
            ("fold", "extend", 0),
            ("fold", "cons", 0),
            ("fold", "concat2", 0),
            ("fold", "insert", 1),
            ("fold", "with_field_a", 0),
            ("fold", "pair", 0),
            ("fold", "add", 0),
            ("fold", "list2", 0),
            ("fold", "select", 1),
            ("fold", "lookup", 0),
        ] {
            let f = w.by_name[leaf];
            w.wrap(hof, f, extras);
        }
        let bare: Vec<usize> = w.hofs.clone();
        for _ in 0..10 {
            let f = rng.pick(&bare);
            let hof = if rng.chance(50) { "map" } else { "fold" };
            let extras = if hof == "map" { w.pool[f].n_in.saturating_sub(1) } else { w.pool[f].n_in.saturating_sub(2) };
            w.wrap(hof, f, extras);
        }
        // Composites of each depth, and maps and folds of them.
        for depth in 1..=4u8 {
            let count = [0, 16, 14, 10, 8][depth as usize];
            let mut made = 0;
            let mut tries = 0;
            while made < count && tries < count * 10 {
                tries += 1;
                let n_in = 1 + rng.below(3);
                let style = Style::random(&mut rng, w.unvalidated);
                if let Some(e) = w.random_composite(&mut rng, depth, n_in, &style) {
                    made += 1;
                    for _ in 0..2 {
                        let hof = if rng.chance(50) { "map" } else { "fold" };
                        let n_in = w.pool[e].n_in;
                        let extras = match (hof, rng.chance(85)) {
                            ("map", true) => n_in.saturating_sub(1),
                            ("fold", true) => n_in.saturating_sub(2),
                            _ => rng.below(3),
                        };
                        w.wrap(hof, e, extras);
                    }
                }
            }
        }
        if seed % 4 == 1 {
            w.res.shapes = Some(Shapes::kept(w.res.units.values(), |h| w.res.units.get(h)));
        } else if seed % 2 == 1 {
            w.res.shapes = Some(Shapes::of(w.res.units.values().map(|b| &**b)));
        }
        w
    }

    fn push(&mut self, unit: Unit, name: &str, class: Class, feat: Feat) -> usize {
        let h = hash_unit(&unit).expect("a leaf hashes");
        self.res.insert(unit.clone());
        let e = self.pool.len();
        self.pool.push(Entry { h, n_in: unit.inputs.len(), n_out: unit.outputs.len(), unit, name: name.into(), class, feat });
        self.by_name.entry(name.into()).or_insert(e);
        e
    }

    fn leaf(&self, name: &str) -> usize {
        *self.by_name.get(name).unwrap_or_else(|| panic!("no pool leaf {name}"))
    }

    /// A map or fold of pool entry `f`, with `extras` extra inputs: in the pool, and its index.
    fn wrap(&mut self, hof: &str, f: usize, extras: usize) -> Option<usize> {
        let (unit, feat) = wrap_unit(&self.pool[f], hof, extras);
        let h = self.res.insert(unit.clone())?;
        let e = self.pool.len();
        let class = if hof == "map" { Class::Map } else { Class::Fold };
        self.pool.push(Entry { h, n_in: unit.inputs.len(), n_out: 1, unit, name: hof.into(), class, feat });
        self.hofs.push(e);
        self.deep[feat.depth as usize].push(e);
        Some(e)
    }

    /// A composite of `children` and their edges, with outputs `outs`: its unit and features.
    fn finish(&self, b: &B) -> (Unit, Feat) {
        let children: Vec<Hash> = b.children.iter().map(|&e| self.pool[e].h).collect();
        let mut edges = b.edges.clone();
        for (o, s) in b.outs.iter().enumerate() {
            match *s {
                Src::In(i) => edges.push(Edge { from_child: BOUNDARY, from_port: i, to_child: BOUNDARY, to_port: o as u32 }),
                Src::Ch(fc, fp) => edges.push(Edge { from_child: fc, from_port: fp, to_child: BOUNDARY, to_port: o as u32 }),
                Src::Unwired => {}
            }
        }
        let unit = Unit {
            body: UnitBody::Composite { children, edges: edges.clone() },
            params: vec![],
            inputs: (0..b.n_in).map(|i| port(&format!("in{i}"))).collect(),
            outputs: (0..b.outs.len()).map(|i| port(&format!("out{i}"))).collect(),
        };
        (unit, self.features(b, &edges))
    }

    fn features(&self, b: &B, edges: &[Edge]) -> Feat {
        let ents: Vec<&Entry> = b.children.iter().map(|&e| &self.pool[e]).collect();
        let n = ents.len();
        let mut f = Feat::default();
        let mut depth = 0u8;
        let mut all_extras = true;
        let mut any_hof = false;
        for e in &ents {
            f.join(&e.feat);
            if matches!(e.class, Class::Composite | Class::Map | Class::Fold) {
                depth = depth.max(e.feat.depth);
            }
            if e.class == Class::Composite {
                f.flags |= F_COMPOSITE_CHILD;
            }
            if e.feat.has_hof() {
                any_hof = true;
                all_extras &= e.feat.extras_every;
            }
        }
        f.depth = depth + 1;
        f.extras_every = any_hof && all_extras;
        // Who feeds what (last edge wins, as the evaluator reads them).
        let mut incoming: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
        let mut seen = HashSet::new();
        for e in edges {
            if !seen.insert((e.to_child, e.to_port)) {
                f.flags |= F_INVALID_EDGE;
            }
            incoming.insert((e.to_child, e.to_port), (e.from_child, e.from_port));
            if e.from_child != BOUNDARY {
                match ents.get(e.from_child as usize) {
                    None => f.flags |= F_INVALID_EDGE,
                    Some(src) if e.from_port as usize >= src.n_out => f.flags |= F_INVALID_EDGE,
                    Some(src) if e.from_port >= 1 && src.n_out >= 2 => f.flags |= F_CHILD_PORT1,
                    _ => {}
                }
            } else if e.from_port as usize >= b.n_in {
                f.flags |= F_INVALID_EDGE;
            }
            if e.to_child != BOUNDARY {
                if let Some(dst) = ents.get(e.to_child as usize) {
                    if e.to_port as usize >= dst.n_in {
                        f.flags |= F_INVALID_EDGE;
                    }
                }
            }
        }
        let mut readers: HashMap<(u32, u32), Vec<(u32, u32)>> = HashMap::new();
        for (&dst, &src) in &incoming {
            readers.entry(src).or_default().push(dst);
        }
        for v in readers.values_mut() {
            v.sort();
        }
        let is = |c: u32, name: &str| ents.get(c as usize).is_some_and(|e| e.unit_name() == name);
        for (c, e) in ents.iter().enumerate() {
            let c = c as u32;
            let name = e.unit_name();
            if name == "guard" {
                match incoming.get(&(c, 0)) {
                    Some(&(t, 0)) if t != BOUNDARY && ents.get(t as usize).is_some_and(|x| x.name == "T") => f.flags |= F_GUARD_PASS,
                    _ => f.flags |= F_GUARD_COND,
                }
            }
            if e.class == Class::Sink {
                if e.unit.inputs.last().is_some_and(|p| p.name == "trigger") {
                    f.flags |= if incoming.contains_key(&(c, e.n_in as u32 - 1)) { F_SINK_TRIGGER_WIRED } else { F_SINK_TRIGGER_OPEN };
                } else {
                    f.flags |= F_SINK_NO_TRIGGER;
                }
            }
            let optional: &[u32] = match name {
                "http_request" => &[1, 2],
                "collection_list" => &[0, 1],
                _ => &[],
            };
            if optional.iter().any(|p| incoming.contains_key(&(c, *p))) {
                f.flags |= F_OPTIONAL_WIRED;
            }
            // A value a builder reads, read elsewhere too.
            for p in 0..e.n_in as u32 {
                if keeps(name, p as usize) {
                    if let Some(src) = incoming.get(&(c, p)) {
                        if readers.get(src).is_some_and(|r| r.len() > 1) {
                            f.flags |= F_SHARED_BUILT;
                        }
                    }
                }
            }
            // The speculative pattern: builder `c`'s output, through guards' values and coalesces'
            // firsts, reaching a coalesce whose fallback is (or is read by a node reading) what
            // `c` builds on.
            if e.class == Class::Builder && e.n_out == 1 {
                let built: Vec<(u32, u32)> = (0..e.n_in as u32).filter(|&p| keeps(name, p as usize)).filter_map(|p| incoming.get(&(c, p)).copied()).collect();
                let mut at = c;
                for _ in 0..8 {
                    let Some(rs) = readers.get(&(at, 0)) else { break };
                    let [(g, q)] = rs[..] else { break };
                    if g == BOUNDARY {
                        break;
                    }
                    if is(g, "guard") && q == 1 {
                        at = g;
                    } else if is(g, "coalesce") && q == 0 {
                        if let Some(&fb) = incoming.get(&(g, 1)) {
                            let direct = built.contains(&fb);
                            let through = fb.0 != BOUNDARY
                                && readers.get(&fb).is_some_and(|r| r.len() == 1)
                                && (0..4).any(|p| incoming.get(&(fb.0, p)).is_some_and(|s| built.contains(s)));
                            if direct || through {
                                f.flags |= F_SPECULATIVE;
                            }
                        }
                        at = g;
                    } else {
                        break;
                    }
                }
            }
        }
        // Outputs.
        let outs: Vec<Option<(u32, u32)>> = (0..b.outs.len() as u32).map(|o| incoming.get(&(BOUNDARY, o)).copied()).collect();
        if b.outs.len() >= 2 {
            f.flags |= F_MULTI_OUT;
        }
        let value_of_choice = |c: u32| -> Vec<(u32, u32)> {
            let ports: &[u32] = if is(c, "guard") { &[1] } else if is(c, "coalesce") { &[0, 1] } else { &[] };
            ports.iter().filter_map(|p| incoming.get(&(c, *p)).copied()).collect()
        };
        for (i, a) in outs.iter().enumerate() {
            let Some(a) = a else { continue };
            if a.0 == BOUNDARY {
                f.flags |= F_OUT_FROM_INPUT;
            }
            for b2 in outs.iter().skip(i + 1).flatten() {
                if a == b2 {
                    f.flags |= if a.0 == BOUNDARY { F_TWO_OUT_INPUT } else { F_TWO_OUT_SAME };
                }
                if a.0 != BOUNDARY && b2.0 != BOUNDARY && a.0 != b2.0 {
                    let (va, vb) = (value_of_choice(a.0), value_of_choice(b2.0));
                    if va.iter().any(|v| vb.contains(v)) {
                        f.flags |= F_TWO_OUT_VIA_CHOICES;
                    }
                }
            }
        }
        let _ = n;
        f
    }

    /// Pool entries a composite of depth `depth` may place: anything shallower.
    fn random_composite(&mut self, rng: &mut Rng, depth: u8, n_in: usize, style: &Style) -> Option<usize> {
        let (unit, feat) = self.random_unit(rng, depth, n_in, style);
        let h = self.res.insert(unit.clone())?;
        let e = self.pool.len();
        self.pool.push(Entry { h, n_in, n_out: unit.outputs.len(), unit, name: format!("composite{depth}"), class: Class::Composite, feat });
        self.deep[feat.depth.min(5) as usize].push(e);
        self.fns[depth as usize].push(e);
        Some(e)
    }

    fn random_unit(&self, rng: &mut Rng, depth: u8, n_in: usize, style: &Style) -> (Unit, Feat) {
        let mut b = B::new(n_in);
        let t = if rng.chance(70) { Some(b.add(self.leaf("T"), &[])) } else { None };
        let n = if style.wide { 6 + rng.below(10) } else { 2 + rng.below(7) };
        let mut placed_deep = false;
        for i in 0..n {
            let last = i + 1 == n;
            let need_deep = depth > 1 && !placed_deep && (last || rng.chance(30));
            let e = if need_deep {
                let d = (depth - 1) as usize;
                if self.deep[d].is_empty() {
                    self.pick_shallow(rng, depth, style.invalid)
                } else {
                    placed_deep = true;
                    rng.pick(&self.deep[d])
                }
            } else {
                self.pick_shallow(rng, depth, style.invalid)
            };
            let entry = &self.pool[e];
            let srcs: Vec<Src> = (0..entry.n_in)
                .map(|p| {
                    if entry.unit_name() == "guard" && p == 0 {
                        if let Some(t) = t {
                            if rng.chance(style.pass) {
                                return Src::Ch(t, 0);
                            }
                        }
                    }
                    if entry.class == Class::Sink && entry.unit.inputs[p].name == "trigger" && rng.chance(40) {
                        return Src::Unwired;
                    }
                    if entry.class == Class::Request && p > 0 && rng.chance(50) {
                        return Src::Unwired;
                    }
                    self.random_src(rng, &b, n_in, style)
                })
                .collect();
            let c = b.add(e, &srcs);
            if self.pool[e].name.ends_with("_1in") {
                // What it would read at the port it doesn't declare.
                match self.random_src(rng, &b, n_in, style) {
                    Src::In(i) => b.edges.push(Edge { from_child: BOUNDARY, from_port: i, to_child: c, to_port: 1 }),
                    Src::Ch(fc, fp) if fc < c => b.edges.push(Edge { from_child: fc, from_port: fp, to_child: c, to_port: 1 }),
                    _ => {}
                }
            }
        }
        if style.invalid && rng.chance(20) {
            // An edge the validator refuses: a port out of range, a missing child, or a second edge
            // to a destination.
            let c = rng.below(b.children.len()) as u32;
            match rng.below(6) {
                5 => {
                    // An edge into a port the child doesn't declare.
                    let k = self.pool[b.children[c as usize]].n_in as u32;
                    let s = if rng.chance(50) { Src::In(rng.below(n_in) as u32) } else { self.random_src(rng, &b, n_in, style) };
                    match s {
                        Src::In(i) => b.edges.push(Edge { from_child: BOUNDARY, from_port: i, to_child: c, to_port: k + rng.below(2) as u32 }),
                        Src::Ch(fc, fp) if fc < c => b.edges.push(Edge { from_child: fc, from_port: fp, to_child: c, to_port: k + rng.below(2) as u32 }),
                        _ => {}
                    }
                }
                3 => {
                    // A child no resolver has.
                    b.add(self.leaf("ghost"), &[Src::In(0)]);
                }
                4 => {
                    // An edge back to an earlier child: a cycle when that child feeds this one.
                    if c > 0 {
                        b.edges.push(Edge { from_child: c, from_port: 0, to_child: rng.below(c as usize) as u32, to_port: 0 });
                    }
                }
                0 => {
                    let to = c + 1 + rng.below(3) as u32;
                    if (to as usize) < b.children.len() {
                        let n_out = self.pool[b.children[c as usize]].n_out as u32;
                        b.edges.push(Edge { from_child: c, from_port: n_out + rng.below(2) as u32, to_child: to, to_port: rng.below(2) as u32 });
                    }
                }
                1 => b.edges.push(Edge { from_child: b.children.len() as u32 + 2, from_port: 0, to_child: c, to_port: 0 }),
                _ => {
                    let s = self.random_src(rng, &b, n_in, style);
                    if let Src::Ch(fc, fp) = s {
                        if fc < c {
                            b.edges.push(Edge { from_child: fc, from_port: fp, to_child: c, to_port: 0 });
                        }
                    }
                }
            }
        }
        let n_out = match rng.below(100) {
            0..=54 => 1,
            55..=84 => 2,
            _ => 3,
        };
        let count = b.children.len();
        for o in 0..n_out {
            let s = match rng.below(100) {
                0..=9 => Src::In(rng.below(n_in) as u32),
                10..=24 if o > 0 => b.outs[o - 1],
                _ => {
                    let c = if rng.chance(50) { count - 1 } else { rng.below(count) };
                    let n_out_c = self.pool[b.children[c]].n_out;
                    let mut fp = rng.below(n_out_c.max(1)) as u32;
                    if style.invalid && rng.chance(4) {
                        fp = n_out_c as u32;
                    }
                    Src::Ch(c as u32, fp)
                }
            };
            b.out(s);
        }
        self.finish(&b)
    }

    fn pick_shallow(&self, rng: &mut Rng, depth: u8, invalid: bool) -> usize {
        if invalid && rng.chance(6) {
            return rng.pick(&self.odd);
        }
        if !self.valid_odd.is_empty() && rng.chance(8) {
            return rng.pick(&self.valid_odd);
        }
        loop {
            let r = rng.below(100);
            let from: &[usize] = match r {
                0..=34 => &self.builders,
                35..=46 => &self.choices[..1],
                47..=54 => &self.choices[1..],
                55..=69 => &self.readers,
                70..=73 => &self.consts,
                74..=77 => &self.requests,
                78..=81 => &self.sinks,
                82 => &self.modules,
                83..=89 => &self.deep[0],
                _ => {
                    let d = rng.below(depth as usize);
                    &self.deep[d]
                }
            };
            if !from.is_empty() {
                return rng.pick(from);
            }
        }
    }

    fn random_src(&self, rng: &mut Rng, b: &B, n_in: usize, style: &Style) -> Src {
        let i = b.children.len();
        let r = rng.below(100);
        if r < 4 {
            return Src::Unwired;
        }
        if r < style.inputs || i == 0 {
            let p = if rng.chance(style.focus0) { 0 } else { rng.below(n_in) };
            return Src::In(p as u32);
        }
        let j = i - 1 - rng.below(i.min(4));
        let n_out = self.pool[b.children[j]].n_out;
        Src::Ch(j as u32, rng.below(n_out.max(1)) as u32)
    }
}

impl Entry {
    fn unit_name(&self) -> &str {
        match &self.unit.body {
            UnitBody::Primitive { name } => name,
            _ => "",
        }
    }
}

struct Style {
    /// Out of 100, how often an input read is input 0 (a fold's accumulator, a map's item).
    focus0: u64,
    /// Out of 100, how often a guard's condition is the shared `true`.
    pass: u64,
    invalid: bool,
    /// Out of 100, how often a port reads an input rather than an earlier child.
    inputs: usize,
    /// Many children (6 to 15) rather than a few.
    wide: bool,
}

impl Style {
    fn random(rng: &mut Rng, unvalidated: bool) -> Style {
        // Now and then an accumulator-heavy one: most ports read input 0.
        let acc = rng.chance(20);
        Style {
            focus0: if acc { 95 } else { rng.pick(&[0, 40, 70, 90]) },
            pass: rng.pick(&[30, 60, 90]),
            invalid: unvalidated,
            inputs: if acc { 60 } else { rng.pick(&[25, 40, 50]) },
            wide: rng.chance(25),
        }
    }
}

fn wrap_unit(f: &Entry, hof: &str, extras: usize) -> (Unit, Feat) {
    let mut inputs = Vec::new();
    if hof == "fold" {
        inputs.push(port("init"));
    }
    inputs.push(port("list"));
    for i in 0..extras {
        inputs.push(port(&format!("extra{i}")));
    }
    let unit = Unit {
        body: UnitBody::Primitive { name: hof.into() },
        params: vec![Param::frozen("fn", hex(f.h))],
        inputs,
        outputs: vec![port("out")],
    };
    let ff = &f.feat;
    let mut feat = Feat { flags: ff.flags, in_fn: ff.flags | ff.in_fn, leaves: ff.leaves, leaves_in_fn: ff.leaves | ff.leaves_in_fn, depth: ff.depth, extras_every: false };
    let composite = f.class == Class::Composite;
    let bare = matches!(f.class, Class::Map | Class::Fold);
    if hof == "map" {
        feat.flags |= F_MAP;
        if composite {
            feat.flags |= F_MAP_FN_COMPOSITE;
        }
        if ff.flags & F_MAP != 0 {
            feat.flags |= F_MAP_IN_MAP;
        }
        if ff.flags & F_FOLD != 0 {
            feat.flags |= F_FOLD_IN_MAP;
        }
    } else {
        feat.flags |= F_FOLD;
        if composite {
            feat.flags |= F_FOLD_FN_COMPOSITE;
        }
        if ff.flags & F_MAP != 0 {
            feat.flags |= F_MAP_IN_FOLD;
        }
        if ff.flags & F_FOLD != 0 {
            feat.flags |= F_FOLD_IN_FOLD;
        }
    }
    if bare {
        feat.flags |= F_BARE_FN;
    }
    if extras > 0 {
        feat.flags |= F_EXTRAS;
    }
    feat.extras_every = extras > 0 && (!ff.has_hof() || ff.extras_every);
    (unit, feat)
}

// ───────────────────────────── templates ─────────────────────────────

/// The source a template reads its shared value from: an input, or a child built from the inputs.
fn shared_value(rng: &mut Rng, w: &World, b: &mut B, t: u32) -> (Src, bool) {
    if rng.chance(45) {
        return (Src::In(0), true);
    }
    let v = match rng.below(10) {
        0 => b.add(w.leaf("append"), &[Src::In(0), Src::In(1)]),
        1 => b.add(w.leaf("tail"), &[Src::In(0)]),
        2 => b.add(w.leaf("concat2"), &[Src::In(0), Src::In(1)]),
        3 => b.add(w.leaf("with_field_a"), &[Src::In(0), Src::In(1)]),
        4 => b.add(w.leaf("guard"), &[Src::Ch(t, 0), Src::In(0)]),
        5 => b.add(w.leaf("list2"), &[Src::In(0), Src::In(1)]),
        6 => b.add(w.leaf("extend"), &[Src::In(0), Src::In(1)]),
        7 => b.add(w.leaf("cons"), &[Src::In(1), Src::In(0)]),
        8 => b.add(w.leaf("insert"), &[Src::In(0), Src::In(2), Src::In(1)]),
        _ => b.add(w.leaf("default_t"), &[Src::In(0)]),
    };
    (Src::Ch(v, 0), false)
}

/// A builder reads a value that a `coalesce`'s fallback reads too, the builder's output reaching
/// the coalesce's first input through guards: given speculatively, put back when it builds nothing.
/// Returns the unit, its features, and (when the builder reads only inputs and constants) the
/// builder and where each of its inputs comes from, to count the cases where it builds nothing.
fn t_speculative(rng: &mut Rng, w: &World) -> (Unit, Feat, Option<(usize, Vec<Src>)>) {
    let mut b = B::new(3);
    let t = b.add(w.leaf("T"), &[]);
    let (v, direct) = shared_value(rng, w, &mut b, t);
    let builders = [
        "append", "extend", "cons", "tail", "insert", "with_field_a", "without_field", "remove", "concat2", "concat3", "select", "head", "get", "field_a", "lookup", "list2",
        "pair", "some", "default_n", "list1", "concat1",
    ];
    let l = w.leaf(rng.pick(&builders));
    let k = w.pool[l].n_in;
    let name = w.pool[l].unit_name().to_string();
    let vports: Vec<usize> = (0..k).filter(|&p| keeps(&name, p)).collect();
    let vport = if vports.is_empty() { 0 } else { rng.pick(&vports) };
    let ck = b.add(w.leaf("c_key"), &[]);
    let srcs: Vec<Src> = (0..k)
        .map(|p| {
            if p == vport {
                v
            } else {
                match rng.below(5) {
                    0 => Src::In(1),
                    1 => Src::In(2),
                    2 => Src::Ch(ck, 0),
                    3 => Src::Ch(t, 0),
                    _ => Src::In(rng.below(3) as u32),
                }
            }
        })
        .collect();
    let lc = b.add(l, &srcs);
    let mut prev = Src::Ch(lc, 0);
    for _ in 0..rng.below(3) {
        let cond = match rng.below(3) {
            0 => Src::Ch(t, 0),
            1 => Src::Ch(b.add(w.leaf("is_truthy"), &[Src::In(2)]), 0),
            _ => Src::Ch(b.add(w.leaf("F"), &[]), 0),
        };
        prev = Src::Ch(b.add(w.leaf("guard"), &[cond, prev]), 0);
    }
    let fallback = if rng.chance(60) {
        v
    } else {
        let x = match rng.below(5) {
            0 => b.add(w.leaf("guard"), &[Src::Ch(t, 0), v]),
            1 => b.add(w.leaf("tail"), &[v]),
            2 => b.add(w.leaf("to_text"), &[v]),
            3 => b.add(w.leaf("append"), &[v, Src::In(1)]),
            _ => b.add(w.leaf("head"), &[v]),
        };
        Src::Ch(x, 0)
    };
    let mut c = b.add(w.leaf("coalesce"), &[prev, fallback]);
    if rng.chance(25) {
        let fb = if rng.chance(50) { v } else { Src::In(1) };
        c = b.add(w.leaf("coalesce"), &[Src::Ch(c, 0), fb]);
    }
    b.out(Src::Ch(c, 0));
    if rng.chance(40) {
        let r = rng.pick(&["length", "is_empty", "to_text", "keys", "http_request"]);
        let rc = if r == "http_request" {
            let path = b.add(w.leaf("to_text"), &[v]);
            b.add(w.leaf(r), &[Src::Ch(path, 0), Src::Unwired, v, Src::Unwired])
        } else {
            b.add(w.leaf(r), &[v])
        };
        if rng.chance(60) {
            b.out(Src::Ch(rc, 0));
        }
    }
    if rng.chance(20) {
        b.out(v);
    }
    let (unit, feat) = w.finish(&b);
    let simple = direct && srcs.iter().all(|s| matches!(s, Src::In(_)) || *s == Src::Ch(t, 0) || *s == Src::Ch(ck, 0));
    let srcs = srcs
        .iter()
        .map(|s| match *s {
            Src::Ch(c, _) if c == t => Src::Ch(u32::MAX, 0),
            Src::Ch(c, _) if c == ck => Src::Ch(u32::MAX - 1, 0),
            other => other,
        })
        .collect();
    (unit, feat, simple.then_some((l, srcs)))
}

/// Several choices pass one heavy value on, read by builders and readers, two outputs through
/// different choices: what lending (a choice keeping no copy) must read through.
fn t_lending(rng: &mut Rng, w: &World) -> (Unit, Feat) {
    let mut b = B::new(3);
    let t = b.add(w.leaf("T"), &[]);
    let (v, _) = shared_value(rng, w, &mut b, t);
    let mut choices = Vec::new();
    for _ in 0..2 + rng.below(3) {
        let c = match rng.below(5) {
            0 => b.add(w.leaf("guard"), &[Src::Ch(t, 0), v]),
            1 => {
                let cond = b.add(w.leaf("is_truthy"), &[Src::In(2)]);
                b.add(w.leaf("guard"), &[Src::Ch(cond, 0), v])
            }
            2 => b.add(w.leaf("coalesce"), &[v, Src::In(1)]),
            3 => {
                let f = b.add(w.leaf("F"), &[]);
                let absent = b.add(w.leaf("guard"), &[Src::Ch(f, 0), Src::In(1)]);
                b.add(w.leaf("coalesce"), &[Src::Ch(absent, 0), v])
            }
            _ => b.add(w.leaf("coalesce"), &[Src::In(1), v]),
        };
        choices.push(c);
    }
    if rng.chance(40) {
        let c0 = choices[0];
        choices.push(b.add(w.leaf("guard"), &[Src::Ch(t, 0), Src::Ch(c0, 0)]));
    }
    let mut readers = Vec::new();
    for _ in 0..rng.below(4) {
        let c = Src::Ch(rng.pick(&choices), 0);
        let r = match rng.below(8) {
            0 => b.add(w.leaf("append"), &[c, Src::In(1)]),
            1 => b.add(w.leaf("length"), &[c]),
            2 => b.add(w.leaf("concat2"), &[c, Src::Ch(rng.pick(&choices), 0)]),
            3 => b.add(w.leaf("equals"), &[c, Src::Ch(rng.pick(&choices), 0)]),
            4 => b.add(w.leaf("with_field_a"), &[c, v]),
            5 => b.add(w.leaf("tail"), &[c]),
            6 => {
                let path = b.add(w.leaf("to_text"), &[c]);
                b.add(w.leaf("http_request"), &[Src::Ch(path, 0), Src::Unwired, c, Src::Unwired])
            }
            _ => b.add(w.leaf("pair"), &[c, v]),
        };
        readers.push(r);
    }
    let n_out = if rng.chance(70) { 2 + rng.below(2) } else { 1 };
    for o in 0..n_out {
        let s = match rng.below(10) {
            // Two outputs through two different choices of the one value, most of the time.
            0..=4 if o > 0 && choices.len() > 1 => {
                let prev = b.outs[o - 1];
                let others: Vec<u32> = choices.iter().copied().filter(|&c| Src::Ch(c, 0) != prev).collect();
                Src::Ch(rng.pick(&others), 0)
            }
            0..=4 => Src::Ch(rng.pick(&choices), 0),
            5 => v,
            6 if o > 0 => b.outs[o - 1],
            _ if !readers.is_empty() => Src::Ch(rng.pick(&readers), 0),
            _ => Src::Ch(rng.pick(&choices), 0),
        };
        b.out(s);
    }
    w.finish(&b)
}

/// Requests and sinks: a request built from the item, headers and bodies wired from guards that
/// are absent sometimes (held), sinks with triggers wired, open and absent.
fn t_requests(rng: &mut Rng, w: &World) -> (Unit, Feat) {
    let mut b = B::new(3);
    let t = b.add(w.leaf("T"), &[]);
    let cond = b.add(w.leaf("is_truthy"), &[Src::In(1)]);
    let x = Src::In(0);
    let path = match rng.below(3) {
        0 => x,
        1 => Src::Ch(b.add(w.leaf("to_text"), &[x]), 0),
        _ => {
            let s = b.add(w.leaf("to_text"), &[x]);
            Src::Ch(b.add(w.leaf("concat2"), &[Src::Ch(t, 0), Src::Ch(s, 0)]), 0)
        }
    };
    let maybe = |rng: &mut Rng, b: &mut B, v: Src| -> Src {
        match rng.below(4) {
            0 => Src::Unwired,
            1 => v,
            _ => Src::Ch(b.add(w.leaf("guard"), &[Src::Ch(cond, 0), v]), 0),
        }
    };
    let headers = maybe(rng, &mut b, Src::In(2));
    let body = maybe(rng, &mut b, x);
    let trigger = maybe(rng, &mut b, Src::In(1));
    let req = b.add(w.leaf("http_request"), &[path, headers, body, trigger]);
    let mut outs = vec![Src::Ch(req, 0)];
    if rng.chance(50) {
        let key = b.add(w.leaf("to_text"), &[x]);
        outs.push(Src::Ch(b.add(w.leaf("collection_get"), &[Src::Ch(key, 0)]), 0));
    }
    if rng.chance(25) {
        outs.push(Src::Ch(b.add(w.leaf("await"), &[]), 0));
    }
    if rng.chance(30) {
        let prefix = maybe(rng, &mut b, path);
        outs.push(Src::Ch(b.add(w.leaf("collection_list"), &[prefix, Src::Unwired]), 0));
    }
    // Sinks.
    for _ in 0..rng.below(3) {
        let trig = maybe(rng, &mut b, Src::In(1));
        let s = match rng.below(4) {
            0 => b.add(w.leaf("store_write"), &[x, trig]),
            1 => {
                let key = b.add(w.leaf("to_text"), &[x]);
                b.add(w.leaf("collection_put"), &[Src::Ch(key, 0), x, trig])
            }
            2 => b.add(w.leaf("egress"), &[x, trig]),
            _ => b.add(w.leaf("egress_legacy"), &[x]),
        };
        if rng.chance(70) {
            outs.push(Src::Ch(s, 0));
        }
    }
    // What reads the answer, and a builder on the item after the request read it.
    let after = match rng.below(5) {
        0 => b.add(w.leaf("coalesce"), &[Src::Ch(req, 0), x]),
        1 => b.add(w.leaf("field_status"), &[Src::Ch(req, 0)]),
        2 => b.add(w.leaf("pair"), &[Src::Ch(req, 0), x]),
        3 => b.add(w.leaf("append"), &[x, Src::Ch(req, 0)]),
        _ => b.add(w.leaf("list2"), &[x, Src::Ch(req, 0)]),
    };
    outs.push(Src::Ch(after, 0));
    let n_out = 1 + rng.below(outs.len().min(3));
    for _ in 0..n_out {
        let o = rng.pick(&outs);
        b.out(o);
    }
    w.finish(&b)
}

// ───────────────────────────── the observer ─────────────────────────────

/// Writes every call it gets, in order. Answers requests by `answers` (0: nothing, 1: values or
/// nothing, 2: values, nothing or not yet) and, when `asserts`, stands in for some nodes.
struct Rec {
    log: String,
    answers: u8,
    asserts: bool,
}

impl Rec {
    fn new(answers: u8, asserts: bool) -> Rec {
        Rec { log: String::new(), answers, asserts }
    }
}

impl Observer for Rec {
    fn enter(&mut self, path: &[u32]) {
        let _ = write!(self.log, "E{path:?} ");
    }
    fn leave(&mut self, path: &[u32]) {
        let _ = write!(self.log, "L{path:?} ");
    }
    fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
        let _ = write!(self.log, "P{path:?}={outputs:?} ");
    }
    fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
        let a = match self.answers {
            0 => Answer::Absent,
            mode => {
                let h = digest(&format!("{path:?}{request:?}"));
                match h % 6 {
                    0 => Answer::Absent,
                    1 if mode == 2 => Answer::NotYet,
                    1 => Answer::Absent,
                    2 => Answer::Value(rec(&[("status", num(200.0)), ("body", list(vec![num((h >> 8 & 3) as f64)]))])),
                    3 => Answer::Value(list(vec![txt("r"), num((h >> 12 & 7) as f64)])),
                    4 => Answer::Value(txt("answer\u{e9}")),
                    _ => Answer::Value(rec(&[("a", txt("x"))])),
                }
            }
        };
        let _ = write!(self.log, "A{path:?}{request:?}->{a:?} ");
        a
    }
    fn failed(&mut self, path: &[u32], reason: &str) {
        let _ = write!(self.log, "F{path:?}{reason} ");
    }
    fn asserted(&mut self, path: &[u32], unit: &Unit, inputs: &[Option<&Value>]) -> Option<Vec<Option<Value>>> {
        if !self.asserts {
            return None;
        }
        let fp = match &unit.body {
            UnitBody::Primitive { name } => name.clone(),
            UnitBody::Composite { children, .. } => format!("c{}", children.len()),
            UnitBody::Module { .. } => "m".into(),
        };
        let h = digest(&format!("{path:?}{fp}"));
        let n = unit.outputs.len();
        let out = if h % 4 != 0 {
            None
        } else if (h >> 8) % 6 == 0 {
            Some(vec![None; n + 1])
        } else {
            Some((0..n).map(|i| if (h >> 16) % 3 == 0 { None } else { Some(txt(&format!("as{i}:{}", inputs.iter().flatten().count()))) }).collect())
        };
        let _ = write!(self.log, "S{path:?}{fp}{inputs:?}->{out:?} ");
        out
    }
}

// ───────────────────────────── the eager oracle ─────────────────────────────

#[cfg(feature = "eager-oracle")]
fn eager(unit: &Unit, inputs: &[Option<Value>], res: &Res) -> Option<Vec<Option<Value>>> {
    Some(holon_core::eager::eval_eager(unit, inputs, res))
}

#[cfg(not(feature = "eager-oracle"))]
fn eager(_: &Unit, _: &[Option<Value>], _: &Res) -> Option<Vec<Option<Value>>> {
    None
}

// ───────────────────────────── describing a case ─────────────────────────────

fn describe(res: &Res, unit: &Unit, out: &mut String, indent: usize, seen: &mut HashSet<Hash>) {
    let pad = " ".repeat(indent);
    match &unit.body {
        UnitBody::Primitive { name } => {
            let params: Vec<String> = unit.params.iter().map(|p| format!("{}={}", p.name(), p.value().unwrap_or("?"))).collect();
            let _ = writeln!(out, "{pad}{name}({} in) {}", unit.inputs.len(), params.join(" "));
            if name == "map" || name == "fold" {
                if let Some(f) = unit.params.iter().find(|p| p.name() == "fn").and_then(|p| p.value()).and_then(holon_core::parse_hash) {
                    match res.resolve(&f) {
                        Some(fu) if seen.insert(f) => describe(res, fu, out, indent + 2, seen),
                        Some(_) => {
                            let _ = writeln!(out, "{pad}  (fn {} above)", &hex(f)[..8]);
                        }
                        None => {
                            let _ = writeln!(out, "{pad}  (fn unresolved)");
                        }
                    }
                }
            }
        }
        UnitBody::Composite { children, edges } => {
            let _ = writeln!(out, "{pad}composite in={} out={}", unit.inputs.len(), unit.outputs.len());
            for (i, c) in children.iter().enumerate() {
                let _ = writeln!(out, "{pad} [{i}] {}", &hex(c)[..8]);
                match res.resolve(c) {
                    Some(cu) => describe(res, cu, out, indent + 4, seen),
                    None => {
                        let _ = writeln!(out, "{pad}    unresolved");
                    }
                }
            }
            let es: Vec<String> = edges
                .iter()
                .map(|e| {
                    let f = if e.from_child == BOUNDARY { format!("in{}", e.from_port) } else { format!("{}.{}", e.from_child, e.from_port) };
                    let t = if e.to_child == BOUNDARY { format!("out{}", e.to_port) } else { format!("{}.{}", e.to_child, e.to_port) };
                    format!("{f}->{t}")
                })
                .collect();
            let _ = writeln!(out, "{pad} edges: {}", es.join(" "));
        }
        UnitBody::Module { bytes } => {
            let _ = writeln!(out, "{pad}module {bytes:?} {} in {} out", unit.inputs.len(), unit.outputs.len());
        }
    }
}

// ───────────────────────────── running cases ─────────────────────────────

#[derive(Default)]
struct Tally {
    cases: u64,
    evaluations: u64,
    flags: BTreeMap<&'static str, (u64, u64)>,
    leaves: BTreeMap<&'static str, (u64, u64)>,
    kinds: BTreeMap<String, u64>,
    inputs: BTreeMap<&'static str, u64>,
    eager_checked: u64,
    eager_mismatch: Vec<String>,
    /// Eager mismatches in cases with a leaf declaring an odd interface or an unvalidated edge,
    /// where the lazy walk and the oracle already differed before (b8048e9): the walk reads a
    /// choice's two ports whatever it declares, the oracle what it declares.
    eager_mismatch_odd: Vec<String>,
    self_mismatch: Vec<String>,
    spec_put_back: u64,
    spec_builds: u64,
}

impl Tally {
    fn count(&mut self, key: impl Into<String>) {
        *self.kinds.entry(key.into()).or_default() += 1;
    }

    /// Where an eager mismatch of `case` goes ([`Tally::eager_mismatch_odd`]).
    fn eager_list(&mut self, case: &Case) -> &mut Vec<String> {
        if (case.feat.flags | case.feat.in_fn) & (F_ODD_IFACE | F_INVALID_EDGE) != 0 {
            &mut self.eager_mismatch_odd
        } else {
            &mut self.eager_mismatch
        }
    }
}

fn kinds_of(v: &Option<Value>, t: &mut Tally) {
    let mut note = |k: &'static str| *t.inputs.entry(k).or_default() += 1;
    fn walk(v: &Value, note: &mut dyn FnMut(&'static str), depth: usize) {
        match v {
            Value::Number(n) if *n == 0.0 && n.is_sign_negative() => note("number -0"),
            Value::Number(_) => note("number"),
            Value::Boolean(_) => note("boolean"),
            Value::Text(s) if s.chars().any(|c| c.len_utf8() > 1) => note("text, multibyte"),
            Value::Text(s) if s.is_empty() => note("text, empty"),
            Value::Text(_) => note("text"),
            Value::List(xs) => {
                note(if xs.is_empty() { "list, empty" } else if depth > 0 { "list, nested" } else { "list" });
                xs.iter().for_each(|x| walk(x, note, depth + 1));
            }
            Value::Record(fs) => {
                note(if fs.is_empty() { "record, empty" } else if depth > 0 { "record, nested" } else { "record" });
                fs.values().for_each(|x| walk(x, note, depth + 1));
            }
            Value::Sum { arm, payload } => {
                note(if *arm == 0 { "sum, some" } else { "sum, none" });
                walk(payload, note, depth + 1);
            }
            Value::Effect { .. } => note("effect"),
            Value::Bytes(_) => note("bytes"),
        }
    }
    match v {
        None => note("absent"),
        Some(v) => walk(v, &mut note, 0),
    }
}

/// What one case is: its unit, its inputs, what it is for the tallies.
struct Case {
    id: String,
    unit: Unit,
    inputs: Vec<Option<Value>>,
    feat: Feat,
    kind: String,
    /// For the speculative template: the builder, its inputs' sources, and the role of the case.
    spec: Option<(usize, Vec<Src>, &'static str)>,
}

fn inputs_for(rng: &mut Rng, unit: &Unit) -> Vec<Option<Value>> {
    match &unit.body {
        UnitBody::Primitive { name } if name == "map" => {
            let mut v = vec![items(rng)];
            v.extend((1..unit.inputs.len()).map(|_| if rng.chance(5) { None } else { Some(heavy(rng)) }));
            v
        }
        UnitBody::Primitive { name } if name == "fold" => {
            let mut v = vec![if rng.chance(5) { None } else { Some(heavy(rng)) }, items(rng)];
            v.extend((2..unit.inputs.len()).map(|_| if rng.chance(5) { None } else { Some(heavy(rng)) }));
            v
        }
        _ => (0..unit.inputs.len()).map(|_| arg(rng)).collect(),
    }
}

/// Wraps a freshly made composite (inserted into the world) as the case's top: called directly,
/// mapped, or folded.
fn as_top(rng: &mut Rng, w: &mut World, unit: Unit, feat: Feat, label: &str) -> (Unit, Feat, String) {
    let role = rng.below(100);
    if role < 25 {
        return (unit, feat, format!("{label}, called"));
    }
    let n_in = unit.inputs.len();
    let Some(h) = w.res.insert(unit.clone()) else { return (unit, feat, format!("{label}, called")) };
    let entry = Entry { h, n_in, n_out: unit.outputs.len(), unit, name: label.into(), class: Class::Composite, feat };
    let (hof, extras) = if role < 60 { ("map", n_in.saturating_sub(1)) } else { ("fold", n_in.saturating_sub(2)) };
    let (wu, wf) = wrap_unit(&entry, hof, extras);
    if rng.chance(25) {
        // Mapped or folded inside another composite that is itself mapped or folded: the template
        // nested one level further, its list given to it.
        if let Some(hw) = w.res.insert(wu.clone()) {
            let e = w.pool.len();
            w.pool.push(Entry { h: hw, n_in: wu.inputs.len(), n_out: 1, unit: wu.clone(), name: hof.into(), class: if hof == "map" { Class::Map } else { Class::Fold }, feat: wf });
            let mut b = B::new(2);
            let t = b.add(w.leaf("T"), &[]);
            let list = b.add(w.leaf("guard"), &[Src::Ch(t, 0), Src::In(0)]);
            let srcs: Vec<Src> = (0..wu.inputs.len()).map(|p| if p == 0 { Src::Ch(list, 0) } else { Src::In(1) }).collect();
            let inner = b.add(e, &srcs);
            b.out(Src::Ch(inner, 0));
            let (cu, cf) = w.finish(&b);
            if let Some(hc) = w.res.insert(cu.clone()) {
                let ce = Entry { h: hc, n_in: 2, n_out: 1, unit: cu, name: "nest".into(), class: Class::Composite, feat: cf };
                let outer = if rng.chance(50) { "map" } else { "fold" };
                let (ou, of) = wrap_unit(&ce, outer, 0);
                return (ou, of, format!("{label}, {hof} in a composite {}", if outer == "map" { "mapped" } else { "folded" }));
            }
        }
    }
    (wu, wf, format!("{label}, {}", if hof == "map" { "mapped" } else { "folded" }))
}

fn make_case(w: &mut World, idx: usize) -> Case {
    let mut rng = Rng::new(w.seed, idx as u64);
    let id = format!("w{}c{}", w.seed, idx);
    let r = rng.below(100);
    let (unit, feat, kind, spec) = if r < 48 {
        // A unit of the world's pool.
        let d = rng.below(5);
        let e = if !w.deep[d].is_empty() && rng.chance(80) { rng.pick(&w.deep[d]) } else { rng.pick(&w.hofs) };
        let entry = &w.pool[e];
        (entry.unit.clone(), entry.feat, format!("pool {}, depth {}", entry.name, entry.feat.depth), None)
    } else if r < 62 {
        let (u, f, simple) = t_speculative(&mut rng, w);
        let (u, f, kind) = as_top(&mut rng, w, u, f, "speculative template");
        let role = if kind.ends_with("called") {
            "called"
        } else if kind.ends_with("mapped") && !kind.contains("composite") {
            "mapped"
        } else if kind.ends_with("folded") && !kind.contains("composite") {
            "folded"
        } else {
            "nested"
        };
        (u, f, kind, simple.map(|(l, s)| (l, s, role)))
    } else if r < 72 {
        let (u, f) = t_lending(&mut rng, w);
        let (u, f, kind) = as_top(&mut rng, w, u, f, "lending template");
        (u, f, kind, None)
    } else if r < 80 {
        let (u, f) = t_requests(&mut rng, w);
        let (u, f, kind) = as_top(&mut rng, w, u, f, "requests template");
        (u, f, kind, None)
    } else {
        // A composite made for this case alone.
        let depth = 1 + rng.below(4) as u8;
        let n_in = 1 + rng.below(3);
        let style = Style::random(&mut rng, w.unvalidated);
        let (u, f) = w.random_unit(&mut rng, depth, n_in, &style);
        let (u, f, kind) = as_top(&mut rng, w, u, f, &format!("fresh depth {depth}"));
        (u, f, kind, None)
    };
    let inputs = inputs_for(&mut rng, &unit);
    Case { id, unit, inputs, feat, kind, spec }
}

/// One evaluation, a panic caught and kept as what it said (the evaluator before never panicked
/// on these, so a panic is a divergence like any other).
fn guarded(f: impl FnOnce() -> Vec<Option<Value>>) -> String {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => format!("{v:?}"),
        Err(e) => format!("PANIC {}", e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default()),
    }
}

/// Every way the case is evaluated, as one text: what a version of the evaluator does with it.
fn run_case(w: &World, case: &Case, rng: &mut Rng, t: &mut Tally) -> String {
    let (unit, inputs, res) = (&case.unit, &case.inputs, &w.res);
    let mut text = String::new();
    let o1 = guarded(|| eval(unit, inputs, res).to_vec());
    let _ = writeln!(text, "eval {o1}");
    if o1.starts_with("PANIC") {
        t.self_mismatch.push(format!("{}: {o1}", case.id));
    }
    let again = guarded(|| eval(unit, inputs, res).to_vec());
    if again != o1 {
        t.self_mismatch.push(format!("{}: eval twice", case.id));
    }
    let mut r0 = Rec::new(0, false);
    let o2 = guarded(|| eval_observed(unit, inputs, res, &mut r0).to_vec());
    let _ = writeln!(text, "observed {o2}\n {}", r0.log);
    if o2 != o1 {
        t.self_mismatch.push(format!("{}: eval and eval_observed", case.id));
    }
    t.evaluations += 3;
    if let Some(e) = eager(unit, inputs, res) {
        t.eager_checked += 1;
        if format!("{e:?}") != o1 {
            t.eager_list(case).push(format!("{}: eager {e:?} lazy {o1}", case.id));
        }
    }
    if case.feat.flags & F_REQUEST != 0 || case.feat.in_fn & F_REQUEST != 0 {
        for mode in [1, 2] {
            let mut r = Rec::new(mode, false);
            let o = guarded(|| eval_observed(unit, inputs, res, &mut r).to_vec());
            let _ = writeln!(text, "answers{mode} {o}\n {}", r.log);
            t.evaluations += 1;
        }
        t.count("observer answering values");
        t.count("observer answering not-yet");
    }
    if matches!(unit.body, UnitBody::Composite { .. }) || rng.chance(30) {
        let mut r = Rec::new(2, true);
        let o = guarded(|| eval_observed(unit, inputs, res, &mut r).to_vec());
        let _ = writeln!(text, "asserted {o}\n {}", r.log);
        t.evaluations += 1;
        if r.log.contains("->Some(") {
            t.count("observer standing in for a node (asserted)");
        }
    }
    if unit.outputs.len() == 1 {
        let one = guarded(|| vec![eval_one(unit, inputs, res)]);
        let _ = writeln!(text, "one {one}");
        t.evaluations += 1;
    }
    // The unit at a place another unit lived at just before, then a mutant of it at the same
    // place: the shapes kept per thread are found by address.
    if let UnitBody::Composite { children, edges } = &unit.body {
        if rng.chance(30) && !edges.is_empty() {
            let boxed = Box::new(unit.clone());
            let ob = guarded(|| eval(&boxed, inputs, res).to_vec());
            drop(boxed);
            let mut edges = edges.clone();
            let k = rng.below(edges.len());
            if edges[k].to_child != BOUNDARY && edges[k].to_child > 0 && rng.chance(70) {
                edges[k].from_child = rng.below(edges[k].to_child as usize) as u32;
                edges[k].from_port = 0;
            } else {
                edges[k].from_child = BOUNDARY;
                edges[k].from_port = rng.below(unit.inputs.len().max(1)) as u32;
            }
            let mutant = Box::new(Unit { body: UnitBody::Composite { children: children.clone(), edges }, ..unit.clone() });
            let om = guarded(|| eval(&mutant, inputs, res).to_vec());
            let _ = writeln!(text, "boxed {ob} mutant {om}");
            if ob != o1 {
                t.self_mismatch.push(format!("{}: boxed", case.id));
            }
            if let Some(e) = eager(&mutant, inputs, res) {
                t.eager_checked += 1;
                if format!("{e:?}") != om {
                    t.eager_list(case).push(format!("{}: mutant eager {e:?} lazy {om}", case.id));
                }
            }
            t.evaluations += 2;
            t.count("unit at a reused address, then a mutant of it there");
        }
    }
    text
}

fn tally_case(case: &Case, w: &World, t: &mut Tally) {
    t.cases += 1;
    t.count(case.kind.clone());
    for &(bit, name) in FLAG_NAMES {
        let any = case.feat.flags & bit != 0;
        let in_fn = case.feat.in_fn & bit != 0;
        if any || in_fn {
            let e = t.flags.entry(name).or_default();
            e.0 += any as u64;
            e.1 += in_fn as u64;
        }
    }
    for (i, &name) in KEEPS.iter().enumerate() {
        let any = case.feat.leaves & (1 << i) != 0;
        let in_fn = case.feat.leaves_in_fn & (1 << i) != 0;
        if any || in_fn {
            let e = t.leaves.entry(name).or_default();
            e.0 += any as u64;
            e.1 += in_fn as u64;
        }
    }
    match case.feat.depth {
        0..=2 => {}
        d => t.count(format!("composite nesting depth {}", d.min(6))),
    }
    if case.feat.extras_every {
        t.count("extras at every map/fold level");
    }
    for v in &case.inputs {
        kinds_of(v, t);
        if let Some(Value::List(xs)) = v {
            for x in xs {
                kinds_of(&Some(x.clone()), t);
            }
        }
    }
    // The speculative template: does its builder build on what it is given, for each application
    // whose inputs are known (the call, each item of a map, a fold's first step)?
    if let Some((l, srcs, role)) = &case.spec {
        let apps: Vec<Vec<Option<Value>>> = match *role {
            "called" => vec![case.inputs.clone()],
            "mapped" => match case.inputs.first() {
                Some(Some(Value::List(xs))) => xs.iter().map(|x| [vec![Some(x.clone())], case.inputs[1..].to_vec()].concat()).collect(),
                _ => vec![],
            },
            "folded" => match (case.inputs.first(), case.inputs.get(1)) {
                (Some(Some(init)), Some(Some(Value::List(xs)))) if !xs.is_empty() => vec![[vec![Some(init.clone()), Some(xs[0].clone())], case.inputs[2..].to_vec()].concat()],
                _ => vec![],
            },
            _ => vec![],
        };
        for app in apps {
            let args: Vec<Option<Value>> = srcs
                .iter()
                .map(|s| match *s {
                    Src::In(i) => app.get(i as usize).cloned().flatten(),
                    Src::Ch(c, _) if c == u32::MAX => Some(Value::Boolean(true)),
                    Src::Ch(_, _) => Some(txt("a")),
                    Src::Unwired => None,
                })
                .collect();
            if guarded(|| vec![eval_one(&w.pool[*l].unit, &args, &w.res)]) != "[None]" {
                t.spec_builds += 1;
            } else {
                t.spec_put_back += 1;
            }
        }
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn dump_ids() -> Option<HashSet<String>> {
    let spec = std::env::var("HOLON_DIFF_DUMP").ok()?;
    let text = match spec.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).expect("the dump list"),
        None => spec.replace(',', "\n"),
    };
    Some(text.lines().map(|l| l.split_whitespace().next().unwrap_or("").to_string()).filter(|s| !s.is_empty()).collect())
}

#[test]
fn differential_record() {
    let record = std::env::var("HOLON_DIFF_RECORD").ok();
    let dump = dump_ids();
    let worlds = env_usize("HOLON_DIFF_WORLDS", 2);
    let first = env_usize("HOLON_DIFF_FIRST_WORLD", 0);
    let per_world = env_usize("HOLON_DIFF_CASES", if record.is_some() || dump.is_some() { 2000 } else { 150 });
    // Only the worlds a dump asks for.
    let wanted_worlds: Option<HashSet<u64>> =
        dump.as_ref().map(|ids| ids.iter().filter_map(|id| id.strip_prefix('w')?.split('c').next()?.parse().ok()).collect());
    let mut lines = String::new();
    let mut dumped = String::new();
    let mut t = Tally::default();
    for seed in first as u64..(first + worlds) as u64 {
        if wanted_worlds.as_ref().is_some_and(|ws| !ws.contains(&seed)) {
            continue;
        }
        let mut w = World::new(seed);
        for idx in 0..per_world {
            let case = make_case(&mut w, idx);
            if dump.as_ref().is_some_and(|ids| !ids.contains(&case.id)) {
                continue;
            }
            let mut rng = Rng::new(seed ^ 0xABCD, idx as u64);
            let text = run_case(&w, &case, &mut rng, &mut t);
            tally_case(&case, &w, &mut t);
            let h = blake3::hash(text.as_bytes());
            let _ = writeln!(lines, "{} {}", case.id, &h.to_hex()[..16]);
            if dump.is_some() {
                let mut d = String::new();
                describe(&w.res, &case.unit, &mut d, 2, &mut HashSet::new());
                let _ = writeln!(dumped, "==== {} ({})\ninputs {:?}\n{d}{text}", case.id, case.kind, case.inputs);
            }
        }
        eprintln!("world {seed}: {} cases so far", t.cases);
    }
    if let Some(path) = &record {
        std::fs::write(path, &lines).expect("the record");
    }
    if let Ok(path) = std::env::var("HOLON_DIFF_DUMP_OUT") {
        std::fs::write(path, &dumped).expect("the dump");
    }
    let summary = summary(&t);
    eprintln!("{summary}");
    if let Ok(path) = std::env::var("HOLON_DIFF_SUMMARY") {
        std::fs::write(path, &summary).expect("the summary");
    }
    if record.is_none() && dump.is_none() {
        assert!(t.eager_mismatch.is_empty() && t.self_mismatch.is_empty(), "{summary}");
    }
}

fn summary(t: &Tally) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "cases {} evaluations {}", t.cases, t.evaluations);
    let _ = writeln!(s, "eager checks {} mismatches {}", t.eager_checked, t.eager_mismatch.len());
    for m in t.eager_mismatch.iter().take(20) {
        let _ = writeln!(s, "  {m}");
    }
    let _ = writeln!(s, "eager mismatches where a leaf declares an odd interface or an edge is unvalidated {}", t.eager_mismatch_odd.len());
    for m in t.eager_mismatch_odd.iter().take(10) {
        let _ = writeln!(s, "  {m}");
    }
    let _ = writeln!(s, "self-consistency mismatches {}", t.self_mismatch.len());
    for m in t.self_mismatch.iter().take(20) {
        let _ = writeln!(s, "  {m}");
    }
    let _ = writeln!(s, "\nfeatures (cases with it anywhere / inside a map or fold function):");
    for &(_, name) in FLAG_NAMES {
        let (a, f) = t.flags.get(name).copied().unwrap_or_default();
        let _ = writeln!(s, "  {name}: {a} / {f}");
    }
    let _ = writeln!(s, "\nleaves `keeps` marks (cases with it anywhere / inside a map or fold function):");
    for &name in KEEPS {
        let (a, f) = t.leaves.get(name).copied().unwrap_or_default();
        let _ = writeln!(s, "  {name}: {a} / {f}");
    }
    let _ = writeln!(s, "\nspeculative template applications with known inputs: builder builds {}, builds nothing (put back) {}", t.spec_builds, t.spec_put_back);
    let _ = writeln!(s, "\ncase kinds and modes:");
    for (k, n) in &t.kinds {
        let _ = writeln!(s, "  {k}: {n}");
    }
    let _ = writeln!(s, "\ninput values (top-level inputs and the items of top-level lists):");
    for (k, n) in &t.inputs {
        let _ = writeln!(s, "  {k}: {n}");
    }
    s
}

// ───────────────────────────── reduced cases ─────────────────────────────
//
// Each is a divergence the harness found, reduced; the expected value is what b8048e9's evaluator
// and the eager oracle give.

fn edge(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
    Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
}

fn composite(n_in: usize, children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
    Unit { body: UnitBody::Composite { children, edges }, params: vec![], inputs: (0..n_in).map(|i| port(&format!("in{i}"))).collect(), outputs: vec![port("out")] }
}

// AUDIT: a choice that lends keeps no outputs, and `Prepared::output` / `Prepared::outputs` read *any*
// missing port of it (`outs.get(fp)` is None) as the value it lends, so port 1 of a one-output
// `coalesce` reads as its value instead of absent.
#[test]
fn audit_a_lending_choice_is_read_only_at_the_output_it_has() {
    let mut r = holon_core::MapResolver::new();
    let coalesce = r.insert(interface_of("coalesce").unwrap()).unwrap();
    let to_text = r.insert(interface_of("to_text").unwrap()).unwrap();
    // The composite's output wired from the coalesce's port 1, which it doesn't have.
    let out = composite(1, vec![coalesce], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 1, BOUNDARY, 0)]);
    assert_eq!(eval(&out, &[Some(txt("x"))], &r).to_vec(), vec![None], "an output read at a port the choice lacks");
    // A child wired from it.
    let read = composite(1, vec![coalesce, to_text], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 1, 1, 0), edge(1, 0, BOUNDARY, 0)]);
    assert_eq!(eval(&read, &[Some(txt("x"))], &r).to_vec(), vec![None], "a child reading a port the choice lacks");
}

// AUDIT: the same read-through, inside a `map`'s function, where every observer lends (`Asks`): an
// observer that reads what each node produced sees a list where the map gave nothing.
#[test]
fn audit_a_lending_choice_is_read_only_at_the_output_it_has_inside_a_map() {
    let mut r = holon_core::MapResolver::new();
    let coalesce = r.insert(interface_of("coalesce").unwrap()).unwrap();
    let f = r.insert(composite(1, vec![coalesce], vec![edge(BOUNDARY, 0, 0, 0), edge(0, 1, BOUNDARY, 0)])).unwrap();
    let mut map = interface_of("map").unwrap();
    map.params = vec![Param::frozen("fn", hex(f))];
    let mut seen = Rec::new(0, false);
    let got = eval_observed(&map, &[Some(list(vec![txt("x"), txt("y")]))], &r, &mut seen).to_vec();
    assert_eq!(got, vec![None]);
}

// AUDIT: reachable through `validate` too: a `guard` unit that declares a second output passes it
// (edges are checked against the declared outputs), and plain `eval` then reads that second
// output as the guard's value, where b8048e9 read it as absent.
#[test]
fn audit_a_guard_declaring_two_outputs_passes_validate_and_its_second_reads_absent() {
    let mut r = holon_core::MapResolver::new();
    let mut guard = interface_of("guard").unwrap();
    guard.outputs.push(Port { name: "more".into(), ty: Type::Any });
    let guard = r.insert(guard).unwrap();
    let unit = Unit {
        body: UnitBody::Composite { children: vec![guard], edges: vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 1, BOUNDARY, 0)] },
        params: vec![],
        inputs: vec![Port { name: "cond".into(), ty: Type::Boolean }, Port { name: "value".into(), ty: Type::Any }],
        outputs: vec![Port { name: "out".into(), ty: Type::Any }],
    };
    assert!(holon_core::validate(&unit, &r).is_ok());
    assert_eq!(eval(&unit, &[Some(Value::Boolean(true)), Some(list(vec![num(1.0)]))], &r).to_vec(), vec![None]);
}

/// `fold(init, list)` of `f`.
fn fold_of(f: Hash) -> Unit {
    let mut fold = interface_of("fold").unwrap();
    fold.params = vec![Param::frozen("fn", hex(f))];
    fold
}

/// A `guard` that declares only its condition: an edge into its value port is one `validate`
/// refuses (`InputPortOutOfRange`), but the walk still reads it, as b8048e9 did.
fn guard_of_one_input(r: &mut holon_core::MapResolver) -> Hash {
    let mut guard = interface_of("guard").unwrap();
    guard.inputs.truncate(1);
    r.insert(guard).unwrap()
}

// AUDIT: `Prepared::may_give` looks a choice's port up in `Gives` at `starts[ci] + p`, but `Gives`
// holds only the ports the child declares; a `guard` declaring one input, fed at port 1, indexes
// past its own entries: here past the end, a panic (b8048e9 gave the accumulator).
#[test]
fn audit_a_guard_fed_at_a_port_it_does_not_declare_does_not_panic_in_a_fold() {
    let mut r = holon_core::MapResolver::new();
    let t = r.insert(Unit { params: vec![Param::frozen("type", "Boolean"), Param::frozen("value", "true")], ..interface_of("const").unwrap() }).unwrap();
    let g = guard_of_one_input(&mut r);
    // f(acc, x) = guard(true, acc), the guard the last child.
    let f = r.insert(composite(2, vec![t, g], vec![edge(0, 0, 1, 0), edge(BOUNDARY, 0, 1, 1), edge(1, 0, BOUNDARY, 0)])).unwrap();
    let got = std::panic::catch_unwind(|| eval(&fold_of(f), &[Some(list(vec![num(1.0)])), Some(list(vec![num(2.0)]))], &r).to_vec());
    assert_eq!(got.ok(), Some(vec![Some(list(vec![num(1.0)]))]));
}

// AUDIT: the same misread, not at the end: the guard's port 1 reads the next child's port-0 entry
// (an `append` of the accumulator, with nothing to wait on), so the guard takes the accumulator
// and the `append` finds it gone (b8048e9 appended to it).
#[test]
fn audit_a_guard_fed_at_a_port_it_does_not_declare_takes_nothing_another_reads() {
    let mut r = holon_core::MapResolver::new();
    let t = r.insert(Unit { params: vec![Param::frozen("type", "Boolean"), Param::frozen("value", "true")], ..interface_of("const").unwrap() }).unwrap();
    let g = guard_of_one_input(&mut r);
    let append = r.insert(interface_of("append").unwrap()).unwrap();
    // f(acc, x) = append(acc, x), with guard(true, acc) placed beside it, read by nothing.
    let f = r
        .insert(composite(2, vec![t, g, append], vec![edge(0, 0, 1, 0), edge(BOUNDARY, 0, 1, 1), edge(BOUNDARY, 0, 2, 0), edge(BOUNDARY, 1, 2, 1), edge(2, 0, BOUNDARY, 0)]))
        .unwrap();
    let got = std::panic::catch_unwind(|| eval(&fold_of(f), &[Some(list(vec![num(1.0)])), Some(list(vec![num(2.0)]))], &r).to_vec());
    assert_eq!(got.ok(), Some(vec![Some(list(vec![num(1.0), num(2.0)]))]));
}

// AUDIT: `Prepared::gives_of` counts a value's readers over the ports each child declares, but the
// walk reads a choice's second port whatever it declares; a `coalesce` declaring one input, its
// fallback the accumulator, isn't counted, so the `cons` beside it is given the accumulator and the
// fallback then reads it gone (b8048e9, and the same function called directly, give it).
#[test]
fn audit_a_choice_reading_a_port_it_does_not_declare_is_counted_as_a_reader() {
    let mut r = holon_core::MapResolver::new();
    let mut coalesce = interface_of("coalesce").unwrap();
    coalesce.inputs.truncate(1);
    let coalesce = r.insert(coalesce).unwrap();
    let cons = r.insert(interface_of("cons").unwrap()).unwrap();
    let to_text = r.insert(interface_of("to_text").unwrap()).unwrap();
    // f(acc, x) = cons(acc, coalesce(nothing, acc)); the `to_text` after the coalesce, read by
    // nothing, keeps the coalesce's misread `Gives` entry (see the two tests above) a port that
    // gives nothing, so only the uncounted reader is left.
    let f = composite(
        2,
        vec![cons, coalesce, to_text],
        vec![edge(BOUNDARY, 0, 0, 0), edge(1, 0, 0, 1), edge(BOUNDARY, 0, 1, 1), edge(BOUNDARY, 1, 2, 0), edge(0, 0, BOUNDARY, 0)],
    );
    let acc = list(vec![num(1.0)]);
    let want = Some(list(vec![acc.clone(), num(1.0)]));
    assert_eq!(eval(&f, &[Some(acc.clone()), Some(num(2.0))], &r).to_vec(), vec![want.clone()], "called directly");
    let f = r.insert(f).unwrap();
    let got = std::panic::catch_unwind(|| eval(&fold_of(f), &[Some(acc.clone()), Some(list(vec![num(2.0)]))], &r).to_vec());
    assert_eq!(got.ok(), Some(vec![want]), "as a fold's function");
}

// ───────────────────────────── hand-written edge cases ─────────────────────────────

/// A `map` of `f` with `extras` extra inputs.
fn map_of(f: Hash, extras: usize) -> Unit {
    let mut inputs = vec![port("list")];
    inputs.extend((0..extras).map(|i| port(&format!("extra{i}"))));
    Unit { body: UnitBody::Primitive { name: "map".into() }, params: vec![Param::frozen("fn", hex(f))], inputs, outputs: vec![port("out")] }
}

type EdgeCase = (&'static str, Unit, Vec<Option<Value>>, Vec<Option<Value>>);

/// Cases where giving, putting back and lending decide what each node reads: inside `fold` and
/// `map` functions, with wrong-typed and absent values, `-0`, multibyte text and nested values.
/// Each expects what b8048e9 and the eager oracle give, compared by `Debug` (so `-0` counts).
fn edge_cases() -> (holon_core::MapResolver, Vec<EdgeCase>) {
    let mut r = holon_core::MapResolver::new();
    let t = r.insert(Unit { params: vec![Param::frozen("type", "Boolean"), Param::frozen("value", "true")], ..interface_of("const").unwrap() }).unwrap();
    let append = r.insert(interface_of("append").unwrap()).unwrap();
    let coalesce = r.insert(interface_of("coalesce").unwrap()).unwrap();
    let guard = r.insert(interface_of("guard").unwrap()).unwrap();
    let pair = r.insert(interface_of("pair").unwrap()).unwrap();
    let concat = r.insert(interface_of("concat").unwrap()).unwrap();
    // f(acc, x) = coalesce(append(acc, x), acc): the speculative pattern, its builder handed a
    // value it can't build on (put back) or one it can.
    let spec = r
        .insert(composite(2, vec![append, coalesce], vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, 1, 0), edge(BOUNDARY, 0, 1, 1), edge(1, 0, BOUNDARY, 0)]))
        .unwrap();
    // f(acc, x) = pair(guard(true, acc), acc): the guard lends the accumulator the pair builds on.
    let lend = r.insert(composite(2, vec![t, guard, pair], vec![edge(0, 0, 1, 0), edge(BOUNDARY, 0, 1, 1), edge(1, 0, 2, 0), edge(BOUNDARY, 0, 2, 1), edge(2, 0, BOUNDARY, 0)])).unwrap();
    // f(a, b) = concat(a, b), a text builder given its first input.
    let cat = r.insert(composite(2, vec![concat], vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)])).unwrap();
    let mb = "\u{e9}\u{20ac}\u{1f600}";
    // Two outputs reading one input through different choices, called directly (plain `eval` lends).
    let mut two = composite(
        1,
        vec![t, guard, coalesce],
        vec![edge(0, 0, 1, 0), edge(BOUNDARY, 0, 1, 1), edge(BOUNDARY, 0, 2, 0), edge(BOUNDARY, 0, 2, 1), edge(1, 0, BOUNDARY, 0), edge(2, 0, BOUNDARY, 1)],
    );
    two.outputs.push(port("out2"));
    let nested = rec(&[("a", rec(&[("b", list(vec![num(-0.0), txt(mb)]))]))]);
    let none = Value::Sum { arm: 1, payload: Box::new(list(vec![])) };
    let lent = list(vec![txt(mb)]);
    let lent = list(vec![lent.clone(), lent]);
    let cases = vec![
        ("put back: text accumulator", fold_of(spec), vec![Some(txt(mb)), Some(list(vec![num(1.0), num(2.0)]))], vec![Some(txt(mb))]),
        ("built: -0 kept", fold_of(spec), vec![Some(list(vec![num(-0.0)])), Some(list(vec![num(-0.0), num(2.0)]))], vec![Some(list(vec![num(-0.0), num(-0.0), num(2.0)]))]),
        ("absent accumulator", fold_of(spec), vec![None, Some(list(vec![num(1.0)]))], vec![None]),
        ("empty list", fold_of(spec), vec![Some(list(vec![num(-0.0)])), Some(list(vec![]))], vec![Some(list(vec![num(-0.0)]))]),
        (
            "map, put back per item",
            map_of(spec, 1),
            vec![Some(list(vec![list(vec![num(1.0)]), txt(mb), num(-0.0), list(vec![]), nested.clone(), none.clone()])), Some(nested.clone())],
            vec![Some(list(vec![list(vec![num(1.0), nested.clone()]), txt(mb), num(-0.0), list(vec![nested.clone()]), nested.clone(), none]))],
        ),
        ("lent, then built on", fold_of(lend), vec![Some(list(vec![txt(mb)])), Some(list(vec![num(1.0), num(2.0)]))], vec![Some(list(vec![lent.clone(), lent]))]),
        ("text built in place", fold_of(cat), vec![Some(txt(mb)), Some(list(vec![txt("a"), txt(""), txt(mb)]))], vec![Some(txt(&format!("{mb}a{mb}")))]),
        ("text, a number in the list", fold_of(cat), vec![Some(txt("a")), Some(list(vec![txt("b"), num(1.0), txt("c")]))], vec![None]),
        ("two outputs, two choices", two, vec![Some(nested.clone())], vec![Some(nested.clone()), Some(nested)]),
    ];
    (r, cases)
}

#[test]
fn hand_written_edge_cases() {
    let (r, cases) = edge_cases();
    let mut wrong = Vec::new();
    for (name, unit, inputs, want) in &cases {
        let want = &format!("{want:?}");
        let plain = guarded(|| eval(unit, inputs, &r).to_vec());
        let mut seen = Rec::new(2, false);
        let observed = guarded(|| eval_observed(unit, inputs, &r, &mut seen).to_vec());
        let one = if unit.outputs.len() == 1 { Some(guarded(|| vec![eval_one(unit, inputs, &r)])) } else { None };
        if plain != *want || observed != *want || one.as_ref().is_some_and(|o| o != want) {
            wrong.push(format!("{name}: plain {plain} observed {observed} one {one:?}, want {want}"));
        }
        #[cfg(feature = "eager-oracle")]
        {
            let eager = format!("{:?}", holon_core::eager::eval_eager(unit, inputs, &r));
            if eager != *want {
                wrong.push(format!("{name}: eager {eager}, want {want}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
