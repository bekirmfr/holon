// Every pure leaf over values a request or a flow can hand it, from the security pass of the
// JavaScript library's leaves: no panic, the same answer twice, every number it answers finite,
// and a count past what a leaf may make refused at once, on a thread a host's size. Each family
// value is a leaf of its own; each higher-order leaf is given functions of every arity, an
// inconsistent comparator among them. The values: numbers at the edges of JavaScript's (huge
// counts, -0, 2^53, the smallest denormal), texts with combining marks, astral characters and
// broken escapes, and lists and records, nested and not.
//
// Run: cargo test -p holon-core --test audit_leaves -- --nocapture (prints the slowest leaves);
//      HOLON_AUDIT_TRACE=1 names each evaluation before it runs, so the last one named is the one
//      that overflowed the stack (which aborts the process before any report).

use holon_core::{capabilities_of, eval, hash_unit, hex, interface_of, Edge, MapResolver, Param, Port, Type, Unit, UnitBody, Value, BOUNDARY, FAMILIES, GENESIS, PASSES_EXTRA};
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

struct Rng(u64);
impl Rng {
    fn below(&mut self, m: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % m.max(1) as u64) as usize
    }
}

fn num(n: f64) -> Value {
    Value::Number(n)
}
fn txt(s: &str) -> Value {
    Value::Text(s.into())
}
fn rec(fs: &[(&str, Value)]) -> Value {
    Value::Record(fs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}
fn nested(depth: usize) -> Value {
    (0..depth).fold(num(1.0), |v, _| Value::List(vec![v, num(2.0)]))
}

fn pool() -> Vec<Value> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let numbers: Vec<Value> = (0..1000).map(|_| num(rng.below(2000) as f64 - 1000.0)).collect();
    let mixed: Vec<Value> = (0..200).map(|i| [num(i as f64), txt(&format!("t{}", i % 7)), Value::Boolean(i % 2 == 0), Value::List(vec![]), rec(&[])][i % 5].clone()).collect();
    vec![
        num(0.0),
        num(-0.0),
        num(1.0),
        num(-1.0),
        num(2.5),
        num(-7.5),
        num(3.0),
        num(36.0),
        num(100.0),
        num(1e21),
        num(-1e21),
        num(2147483648.0),
        num(4294967296.0),
        num(9007199254740994.0),
        num(1e9),
        num(1e300),
        num(f64::MAX),
        num(5e-324),
        txt(""),
        txt("a"),
        txt("é\u{301}ﬃ"),
        txt("a,b,,c"),
        txt("😀x😀"),
        txt("  padded\t\n"),
        txt("%E0%A4%A"),
        txt("%"),
        txt("2024-02-29T12:34:56.789Z"),
        txt("-0x1F"),
        txt("1e400"),
        txt("{\"a\":[1,{\"b\":null}],\"__proto__\":2}"),
        txt(&"ab😀".repeat(3000)),
        Value::Boolean(true),
        Value::Boolean(false),
        Value::List(vec![]),
        Value::List(vec![num(3.0), num(1.0), num(2.0), num(-0.0), num(0.0)]),
        Value::List(vec![txt("b"), txt("a"), txt("b")]),
        Value::List(vec![Value::List(vec![txt("a"), num(1.0)]), Value::List(vec![txt("b"), num(2.0)])]),
        Value::List(numbers),
        Value::List(mixed),
        nested(70),
        rec(&[]),
        rec(&[("a", num(1.0)), ("b", txt("x")), ("c", Value::List(vec![num(1.0)]))]),
        rec(&[("a", rec(&[("a", rec(&[("a", num(0.0))]))]))]),
        Value::Sum { arm: 0, payload: Box::new(num(1.0)) },
        Value::Effect { sink: "store_write".into(), payload: Box::new(num(1.0)) },
    ]
}

fn port(name: &str) -> Port {
    Port { name: name.into(), ty: Type::Any }
}

/// Functions for the higher-order leaves, in `r`: the leaves of one and two inputs a flow most
/// often passes, a comparator answering booleans (not a total order), one whose answer is its
/// first input whole, and one of three inputs (an arity the caller doesn't hand).
fn functions(r: &mut MapResolver) -> Vec<String> {
    let mut leaf = |name: &str, n: usize, params: Vec<Param>| {
        let mut unit = interface_of(name).unwrap();
        unit.inputs = (0..n).map(|i| port(&format!("i{i}"))).collect();
        if !params.is_empty() {
            unit.params = params;
        }
        hex(r.insert(unit).unwrap())
    };
    let mut fns = vec![
        leaf("not", 1, vec![]),
        leaf("is_truthy", 1, vec![]),
        leaf("to_text", 1, vec![]),
        leaf("length", 1, vec![]),
        leaf("add", 2, vec![]),
        leaf("subtract", 2, vec![]),
        leaf("compare", 2, vec![Param::frozen("op", "<")]),
        leaf("pair", 2, vec![]),
        leaf("list", 3, vec![]),
    ];
    let first = Unit {
        body: UnitBody::Composite { children: vec![], edges: vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: BOUNDARY, to_port: 0 }] },
        params: vec![],
        inputs: vec![port("a"), port("b")],
        outputs: vec![port("out")],
    };
    fns.push(hex(r.insert(first).unwrap()));
    fns
}

/// Every pure leaf, once for each value of its family parameters and each function a
/// higher-order one may be given, with its frozen parameters' odd values too.
fn variants(fns: &[String]) -> Vec<(String, Unit)> {
    let mut out = Vec::new();
    for &name in GENESIS {
        let Some(base) = interface_of(name) else { continue };
        if !capabilities_of(name, &base.params).is_some_and(|c| c.is_pure()) {
            continue;
        }
        let mut units = vec![base.clone()];
        for (_, param, values) in FAMILIES.iter().filter(|(leaf, _, _)| *leaf == name) {
            units = units
                .iter()
                .flat_map(|u| values.iter().map(move |v| set(u, param, v)))
                .collect();
        }
        let odd: &[(&str, &[&str])] = match name {
            "flat" => &[("depth", &["0", "1", "3", "Infinity", "1e9", "-1", "x"])],
            "json_stringify" => &[("indent", &["", "  ", "\t", "abcdefghijklmnop"]), ("keys", &["", "[\"a\",\"b\"]", "[1]", "["])],
            "field" | "with_field" | "without_field" => &[("name", &["a", "__proto__", ""])],
            "sha256" | "hmac_sha256" => &[("encoding", &["hex", "base64", "x"])],
            _ => &[],
        };
        for (param, values) in odd {
            units = units.iter().flat_map(|u| values.iter().map(move |v| set(u, param, v))).collect();
        }
        if PASSES_EXTRA.contains(&name) {
            units = units.iter().flat_map(|u| fns.iter().map(move |f| set(u, "fn", f))).collect();
            if matches!(name, "json_parse" | "json_stringify") {
                units.push(base.clone());
            }
        }
        for u in units {
            let label = format!("{name}{}", u.params.iter().map(|p| format!(" {}={}", p.name(), short(p.value().unwrap_or("")))).collect::<String>());
            out.push((label, u));
        }
    }
    out
}

fn set(unit: &Unit, param: &str, value: &str) -> Unit {
    let mut u = unit.clone();
    u.params.retain(|p| p.name() != param);
    u.params.push(Param::frozen(param, value));
    u.params.sort_by(|a, b| a.name().cmp(b.name()));
    u
}

fn short(s: &str) -> String {
    s.chars().take(10).collect()
}

/// Every number in `v` is finite, as a value a run carries must be.
fn finite(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_finite(),
        Value::List(xs) => xs.iter().all(finite),
        Value::Record(fs) => fs.values().all(finite),
        Value::Sum { payload, .. } | Value::Effect { payload, .. } => finite(payload),
        _ => true,
    }
}

fn input_sets(n: usize, pool: &[Value], rng: &mut Rng) -> Vec<Vec<Option<Value>>> {
    let pick = |i: usize| if i == pool.len() { None } else { Some(pool[i].clone()) };
    match n {
        0 => vec![vec![]],
        1 => (0..=pool.len()).map(|i| vec![pick(i)]).collect(),
        2 => (0..=pool.len()).flat_map(|i| (0..=pool.len()).map(move |j| (i, j))).map(|(i, j)| vec![pick(i), pick(j)]).collect(),
        _ => (0..1500).map(|_| (0..n).map(|_| pick(rng.below(pool.len() + 1))).collect()).collect(),
    }
}

#[test]
fn every_pure_leaf_over_hostile_values_answers_finitely_and_the_same_twice() {
    // A host runs a flow on a 2 MiB thread (tokio's); the sweep runs on one too.
    let found = std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(sweep).unwrap().join().unwrap();
    assert!(found.is_empty(), "{} leaves misbehaved:\n{}", found.len(), found.join("\n"));
}

fn sweep() -> Vec<String> {
    let mut r = MapResolver::new();
    let fns = functions(&mut r);
    let pool = pool();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut found = Vec::new();
    let mut slowest: Vec<(Duration, String)> = Vec::new();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let variants = variants(&fns);
    let mut evals = 0usize;
    for (label, unit) in &variants {
        assert!(hash_unit(unit).is_ok(), "{label} hashes");
        let mut worst = (Duration::ZERO, String::new());
        let mut faults: BTreeMap<&str, (usize, String)> = BTreeMap::new();
        for inputs in input_sets(unit.inputs.len(), &pool, &mut rng) {
            evals += 1;
            if std::env::var_os("HOLON_AUDIT_TRACE").is_some() {
                eprintln!("{label} <- {}", inputs.iter().map(|v| v.as_ref().map(|v| short(&format!("{v:?}"))).unwrap_or("-".into())).collect::<Vec<_>>().join(", "));
            }
            let started = Instant::now();
            let first = catch_unwind(AssertUnwindSafe(|| eval(unit, &inputs, &r).into_vec()));
            let took = started.elapsed();
            let shown = || inputs.iter().map(|v| v.as_ref().map(|v| short(&format!("{v:?}"))).unwrap_or("-".into())).collect::<Vec<_>>().join(", ");
            if took > worst.0 {
                worst = (took, shown());
            }
            let fault = match first {
                Err(_) => Some("panics"),
                Ok(out) if !out.iter().flatten().all(finite) => Some("answers a number that isn't finite"),
                Ok(out) => match catch_unwind(AssertUnwindSafe(|| eval(unit, &inputs, &r).into_vec())) {
                    Ok(again) if again == out => None,
                    _ => Some("answers differently the second time"),
                },
            };
            let fault = fault.or((took > Duration::from_secs(2)).then_some("takes more than 2 seconds"));
            if let Some(f) = fault {
                faults.entry(f).or_insert((0, shown())).0 += 1;
            }
        }
        for (fault, (count, example)) in faults {
            found.push(format!("{label}: {fault} on {count} input sets, e.g. ({example})"));
        }
        slowest.push((worst.0, format!("{label} ({})", worst.1)));
    }
    std::panic::set_hook(hook);
    slowest.sort_by_key(|s| std::cmp::Reverse(s.0));
    println!("{} leaves, {evals} evaluations; the slowest:", variants.len());
    for (took, what) in slowest.iter().take(12) {
        println!("  {took:>10.2?}  {what}");
    }
    found
}
