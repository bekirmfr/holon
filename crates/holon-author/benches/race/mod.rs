//! The races' harness: a composite written in TypeScript, compiled by the reader and run through
//! the real `eval`, against a native candidate, a Rust body timed behind one leaf's dispatch
//! through `eval` (what a leaf costs to reach), so neither side skips what a flow pays
//! (docs/engineering/themes/javascript-library.md §0, §11).
//!
//! A bench file is `harness = false` and calls these from its `main`; each race prints one
//! markdown row per size: the composite's time, the candidate's, and their ratio. A filter
//! argument (`cargo bench -p holon-author --bench races_numbers -- abs`) runs the races whose
//! name contains it.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use holon_core::{decode_preimage, eval, interface_of, MapResolver, Param, Resolver, Unit, Value};

/// A function of a TypeScript text, compiled, with the units it places.
pub struct Composite {
    pub name: String,
    pub unit: Unit,
    pub store: MapResolver,
}

impl Composite {
    /// Its output on `inputs`, or none.
    pub fn call(&self, inputs: &[Value]) -> Option<Value> {
        let inputs: Vec<Option<Value>> = inputs.iter().cloned().map(Some).collect();
        eval(&self.unit, &inputs, &self.store).into_iter().next().flatten()
    }

    /// How many leaves one call of it places, its functions' included (a `map`'s or `fold`'s
    /// function counted once).
    pub fn leaves(&self) -> usize {
        fn count(unit: &Unit, store: &MapResolver) -> usize {
            match &unit.body {
                holon_core::UnitBody::Composite { children, .. } => children.iter().filter_map(|h| store.resolve(h)).map(|u| count(u, store)).sum(),
                _ => {
                    let f = unit.params.iter().find_map(|p| match p {
                        Param::Frozen { name, value } if name == "fn" => holon_core::parse_hash(value),
                        _ => None,
                    });
                    1 + f.and_then(|h| store.resolve(&h)).map_or(0, |u| count(u, store))
                }
            }
        }
        count(&self.unit, &self.store)
    }
}

/// The function `name` of a flow's TypeScript `functions` (one or more `function` declarations,
/// with any `import { … } from "holon"` lines they need first), compiled through the reader. The
/// flow around them is the harness's: an action calls `name` on its input, so the function is
/// lowered. Panics with the reader's mistake.
pub fn function(functions: &str, name: &str, arity: usize) -> Composite {
    let (imports, body): (Vec<&str>, Vec<&str>) = functions.lines().partition(|l| l.trim_start().starts_with("import "));
    let args: Vec<String> = (0..arity).map(|k| format!("input.a{k}")).collect();
    let src = format!(
        "import {{ flow, action }} from \"holon\"\n{}\n\nflow(\"race\")\n\n{}\n\nexport const run = action((input) => {name}({}))\n",
        imports.join("\n"),
        body.join("\n"),
        args.join(", ")
    );
    let lowered = holon_author::lang::ts::compile(&src).unwrap_or_else(|e| panic!("{name} doesn't compile: {e:?}\n{src}"));
    let doc = lowered.doc;
    let mut store = MapResolver::new();
    for hex in doc.units.iter().flatten() {
        let bytes: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex")).collect();
        let unit = decode_preimage(&bytes).expect("a unit");
        store.insert(unit).expect("stored");
    }
    let hash = doc
        .names
        .iter()
        .flatten()
        .find(|(_, n)| n.as_str() == name)
        .map(|(h, _)| h.clone())
        .unwrap_or_else(|| panic!("no function {name} in the compiled flow: {:?}", doc.names));
    let unit = store.resolve(&holon_core::parse_hash(&hash).expect("a hash")).expect("resolved").clone();
    Composite { name: name.to_string(), unit, store }
}

/// `function(functions, name, 1)` applied to each item of a list: `xs.map((x) => name(x))`, so a
/// scalar composite is timed where it is hot, its preparation paid once for the list, not per
/// call. Race it against `Side::NativeMapped`.
pub fn mapped(functions: &str, name: &str) -> Composite {
    let wrapper = format!("{functions}

function each_{name}(xs) {{
  return xs.map((x) => {name}(x))
}}");
    let mut c = function(&wrapper, &format!("each_{name}"), 1);
    c.name = format!("xs.map({name})");
    c
}

/// A catalog leaf by its name, its params set (`[("op", "<")]`), as the reader places it.
pub fn leaf(name: &str, params: &[(&str, &str)]) -> Unit {
    let mut unit = interface_of(name).unwrap_or_else(|| panic!("no leaf {name}"));
    for (k, v) in params {
        unit.params.retain(|p| p.name() != *k);
        unit.params.push(Param::frozen(*k, *v));
    }
    unit
}

/// A value from JSON: numbers, texts, booleans, lists and records (`null` has no value).
pub fn v(json: &str) -> Value {
    fn of(j: &serde_json::Value) -> Value {
        match j {
            serde_json::Value::Number(n) => Value::Number(n.as_f64().expect("a number")),
            serde_json::Value::String(s) => Value::Text(s.clone()),
            serde_json::Value::Bool(b) => Value::Boolean(*b),
            serde_json::Value::Array(xs) => Value::List(xs.iter().map(of).collect()),
            serde_json::Value::Object(m) => Value::Record(m.iter().map(|(k, x)| (k.clone(), of(x))).collect::<BTreeMap<_, _>>()),
            serde_json::Value::Null => panic!("null has no value"),
        }
    }
    of(&serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}")))
}

pub fn num(n: f64) -> Value {
    Value::Number(n)
}

pub fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

/// A list of `n` numbers, `0..n`.
pub fn numbers(n: usize) -> Value {
    Value::List((0..n).map(|k| Value::Number(k as f64)).collect())
}

/// A native candidate: what the leaf would compute, on its present inputs.
pub type Native = fn(&[&Value]) -> Option<Value>;

/// One contender in a race.
pub enum Side<'a> {
    /// A function written in TypeScript, through `eval`.
    Composite(&'a Composite),
    /// A leaf that exists, through `eval`.
    Leaf(&'a Unit),
    /// A candidate leaf: its body, behind one leaf's dispatch through `eval`.
    Native(Native),
    /// A candidate unary leaf applied to each item of a list input, as `map` would apply it: `map`
    /// of a cheap leaf (`is_truthy`) through `eval`, plus the body on each item.
    NativeMapped(Native),
}

impl Side<'_> {
    fn label(&self) -> String {
        match self {
            Side::Composite(c) => format!("composite {} ({} leaves)", c.name, c.leaves()),
            Side::Leaf(u) => format!("leaf {}", leaf_name(u)),
            Side::Native(_) => "native candidate".to_string(),
            Side::NativeMapped(_) => "native candidate, mapped".to_string(),
        }
    }
}

fn leaf_name(u: &Unit) -> String {
    match &u.body {
        holon_core::UnitBody::Primitive { name } => name.clone(),
        _ => "?".into(),
    }
}

/// What reaching a leaf costs: `not` on a boolean, through `eval`. A candidate's body is timed
/// behind it, so a candidate pays a dispatch as a leaf would.
struct Floor {
    unit: Unit,
    inputs: Vec<Option<Value>>,
    store: MapResolver,
    /// `map(is_truthy)`: what `map` costs to apply a leaf to each item.
    map: Unit,
}

impl Floor {
    fn new() -> Floor {
        let mut store = MapResolver::new();
        let truthy = store.insert(leaf("is_truthy", &[])).expect("stored");
        let hex: String = truthy.iter().map(|b| format!("{b:02x}")).collect();
        let map = leaf("map", &[("fn", &hex)]);
        Floor { unit: leaf("not", &[]), inputs: vec![Some(Value::Boolean(true))], store, map }
    }
}

/// Each side's output on `cases`, which must be the same (a composite that differs from what it
/// replaces is no composite for it). Panics on the first case where they differ.
pub fn check(name: &str, a: &Side, b: &Side, cases: &[Vec<Value>]) {
    for case in cases {
        let (x, y) = (output(a, case), output(b, case));
        let alike = match (&x, &y) {
            (Some(x), Some(y)) => same(x, y),
            (None, None) => true,
            _ => false,
        };
        assert!(alike, "{name}: {} gives {x:?} and {} gives {y:?} on {case:?}", a.label(), b.label());
    }
}

/// Whether two values are the same, a number by its bits: `-0` isn't `0` here, as JavaScript's
/// `Object.is` says, so a composite that loses a sign is caught.
pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.to_bits() == y.to_bits(),
        (Value::List(xs), Value::List(ys)) => xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same(x, y)),
        (Value::Record(xs), Value::Record(ys)) => xs.len() == ys.len() && xs.iter().zip(ys).all(|((k, x), (l, y))| k == l && same(x, y)),
        (Value::Sum { arm: a, payload: x }, Value::Sum { arm: b, payload: y }) => a == b && same(x, y),
        _ => a == b,
    }
}

/// A side's output on `inputs`.
pub fn output(side: &Side, inputs: &[Value]) -> Option<Value> {
    match side {
        Side::Composite(c) => c.call(inputs),
        Side::Leaf(u) => {
            let ins: Vec<Option<Value>> = inputs.iter().cloned().map(Some).collect();
            eval(u, &ins, &MapResolver::new()).into_iter().next().flatten()
        }
        Side::Native(f) => f(&inputs.iter().collect::<Vec<_>>()),
        Side::NativeMapped(f) => match inputs {
            [Value::List(xs)] => xs.iter().map(|x| f(&[x])).collect::<Option<Vec<_>>>().map(Value::List),
            _ => None,
        },
    }
}

/// Whether the bench's filter argument lets `name` run.
pub fn wanted(name: &str) -> bool {
    match std::env::args().skip(1).find(|a| !a.starts_with('-')) {
        Some(filter) => name.contains(&filter),
        None => true,
    }
}

/// Race `a` against `b` on the inputs `make(n)` gives for each of `sizes` (one size, `[1]`, for a
/// scalar), printing a markdown row per size: each side's median time per call and `a`'s time
/// over `b`'s. Inputs are made before timing.
pub fn race(name: &str, a: &Side, b: &Side, sizes: &[usize], make: impl Fn(usize) -> Vec<Value>) {
    if !wanted(name) {
        return;
    }
    let floor = Floor::new();
    println!("\n### {name}\n\n| n | {} | {} | ratio |\n|---:|---:|---:|---:|", a.label(), b.label());
    for &n in sizes {
        let inputs = make(n);
        let (ta, tb) = (time(a, &inputs, &floor), time(b, &inputs, &floor));
        println!("| {n} | {} | {} | {:.2} |", show(ta), show(tb), ta / tb);
    }
}

fn show(ns: f64) -> String {
    if ns >= 1_000_000.0 {
        format!("{:.2} ms", ns / 1_000_000.0)
    } else if ns >= 1_000.0 {
        format!("{:.2} µs", ns / 1_000.0)
    } else {
        format!("{ns:.0} ns")
    }
}

/// A side's median time per call on `inputs`, in nanoseconds: seven samples of as many calls as
/// fill about 20 ms, after one such sample to warm up.
fn time(side: &Side, inputs: &[Value], floor: &Floor) -> f64 {
    let ins: Vec<Option<Value>> = inputs.iter().cloned().map(Some).collect();
    let refs: Vec<&Value> = inputs.iter().collect();
    let empty = MapResolver::new();
    let once = || match side {
        Side::Composite(c) => {
            black_box(eval(&c.unit, black_box(&ins), &c.store));
        }
        Side::Leaf(u) => {
            black_box(eval(u, black_box(&ins), &empty));
        }
        Side::Native(f) => {
            black_box(eval(&floor.unit, black_box(&floor.inputs), &floor.store));
            black_box(f(black_box(&refs)));
        }
        Side::NativeMapped(f) => {
            black_box(eval(&floor.map, black_box(&ins), &floor.store));
            if let [Value::List(xs)] = inputs {
                for x in xs {
                    black_box(f(black_box(&[x])));
                }
            }
        }
    };
    // How many calls fill the sample.
    let mut calls = 1u64;
    loop {
        let start = Instant::now();
        for _ in 0..calls {
            once();
        }
        if start.elapsed() >= Duration::from_millis(20) || calls >= 1 << 30 {
            break;
        }
        calls *= 2;
    }
    let mut samples: Vec<f64> = (0..7)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..calls {
                once();
            }
            start.elapsed().as_nanos() as f64 / calls as f64
        })
        .collect();
    samples.sort_by(|x, y| x.partial_cmp(y).unwrap());
    samples[3]
}
