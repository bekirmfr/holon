//! The JavaScript engine module, as built and committed in `fixtures/modules/js-engine/`, run
//! through the engine crate: it is a module unit (no imports, the ABI's exports), it answers
//! `{ value }` or `{ error }`, values cross as docs/L4s/js-node-L4.md §5 says, and the prelude
//! removes what §4 says it removes. Each row and each removal is a test here.

use std::collections::BTreeMap;

use holon_core::{validate_module, ModuleLimits, Value};
use holon_module::{Engine, RunError};

const ENGINE: &[u8] = include_bytes!("../../../fixtures/modules/js-engine/js-engine.wasm");

fn n(x: f64) -> Value {
    Value::Number(x)
}
fn t(s: &str) -> Value {
    Value::Text(s.into())
}
fn none() -> Value {
    Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) }
}

/// Run `source` on `args`: the result record the engine answers.
fn call(source: &str, args: Vec<Value>) -> BTreeMap<String, Value> {
    call_with(source, args, &ModuleLimits { fuel: 2_000_000_000, memory: 64 << 20 }).unwrap()
}

fn call_with(source: &str, args: Vec<Value>, limits: &ModuleLimits) -> Result<BTreeMap<String, Value>, RunError> {
    let outs = Engine::new().run(ENGINE, limits, &[Some(t(source)), Some(Value::List(args))])?;
    match outs.into_iter().next().flatten() {
        Some(Value::Record(r)) => Ok(r),
        other => panic!("the engine answers one record, not {other:?}"),
    }
}

fn value(source: &str, args: Vec<Value>) -> Option<Value> {
    let r = call(source, args);
    assert!(!r.contains_key("error"), "{r:?}");
    r.get("value").cloned()
}

fn error(source: &str, args: Vec<Value>) -> String {
    let r = call(source, args);
    match r.get("error") {
        Some(Value::Text(e)) => e.clone(),
        other => panic!("expected an error, got {other:?} in {r:?}"),
    }
}

#[test]
fn the_engine_is_a_module_unit() {
    assert_eq!(validate_module(ENGINE), Ok(()), "no imports, the ABI's exports");
    assert!(ENGINE.len() < 12 << 20, "{} bytes: the editor fetches this by hash", ENGINE.len());
}

#[test]
fn a_function_runs_on_its_arguments_and_answers_its_value() {
    assert_eq!(value("(a, b) => a + b", vec![n(2.0), n(3.0)]), Some(n(5.0)));
    assert_eq!(value("(s) => s.toUpperCase() + '!'", vec![t("hi")]), Some(t("HI!")));
    assert_eq!(value("() => true", vec![]), Some(Value::Boolean(true)));
    let xs = Value::List(vec![n(1.0), n(2.0), n(3.0)]);
    assert_eq!(value("(xs) => xs.reduce((a, b) => a + b, 0)", vec![xs.clone()]), Some(n(6.0)));
    assert_eq!(value("(xs) => xs.map((x) => x * 2)", vec![xs]), Some(Value::List(vec![n(2.0), n(4.0), n(6.0)])));
}

#[test]
fn records_cross_both_ways_in_key_order_and_undefined_fields_are_left_out() {
    let input = Value::Record(BTreeMap::from([("b".to_string(), n(2.0)), ("a".to_string(), t("x"))]));
    let out = value("(r) => ({ total: r.b + 1, name: r.a, gone: undefined, nested: { k: [r.b] } })", vec![input]).unwrap();
    let Value::Record(fields) = out else { panic!("{out:?}") };
    assert_eq!(fields.keys().cloned().collect::<Vec<_>>(), ["name", "nested", "total"], "key order is the record's");
    assert_eq!(fields["total"], n(3.0));
    assert_eq!(fields["nested"], Value::Record(BTreeMap::from([("k".to_string(), Value::List(vec![n(2.0)]))])));
}

#[test]
fn absence_crosses_as_undefined_and_back() {
    // `none` in a list arrives as undefined; undefined returned is an absent value.
    assert_eq!(value("(xs) => xs[1] === undefined", vec![Value::List(vec![n(1.0), none(), n(3.0)])]), Some(Value::Boolean(true)));
    assert_eq!(value("() => undefined", vec![]), None);
    assert_eq!(value("() => null", vec![]), None);
    // Inside a list, a hole is `none`.
    assert_eq!(value("() => [1, undefined, 3]", vec![]), Some(Value::List(vec![n(1.0), none(), n(3.0)])));
}

#[test]
fn numbers_cross_as_f64_including_the_non_finite_ones() {
    assert_eq!(value("(a, b) => a / b", vec![n(1.0), n(0.0)]), Some(n(f64::INFINITY)));
    match value("() => 0 / 0", vec![]) {
        Some(Value::Number(x)) => assert!(x.is_nan()),
        other => panic!("{other:?}"),
    }
    assert_eq!(value("() => 2 ** 53 + 2", vec![]), Some(n(9007199254740994.0)));
}

#[test]
fn an_exception_is_an_error_naming_it() {
    let e = error("(xs) => xs.reduce((a, b) => a + b)", vec![n(1.0)]);
    assert!(e.starts_with("TypeError"), "{e}");
    let e = error("() => { throw new Error('nope') }", vec![]);
    assert!(e.contains("nope"), "{e}");
    let e = error("(a, b) => { oops }", vec![]);
    assert!(e.contains("oops") || e.starts_with("ReferenceError"), "{e}");
}

#[test]
fn a_syntax_error_is_an_error_too() {
    let e = error("(a => {", vec![]);
    assert!(e.starts_with("SyntaxError"), "{e}");
    let e = error("42", vec![]);
    assert_eq!(e, "the source isn't a function");
}

#[test]
fn kinds_that_cannot_cross_are_errors_naming_the_kind() {
    for (src, kind) in [
        ("() => new Map()", "Map"),
        ("() => new Set()", "Set"),
        ("() => () => 1", "function"),
        ("() => 10n", "bigint"),
        ("() => Symbol('s')", "symbol"),
        ("() => /re/", "RegExp"),
        ("() => Promise.resolve(1)", "Promise"),
        ("async () => 1", "Promise"),
    ] {
        let e = error(src, vec![]);
        assert!(e.contains(kind), "{src}: {e}");
    }
}

#[test]
fn the_prelude_leaves_no_clock_randomness_io_or_locale() {
    for (src, missing) in [
        ("() => Date.now()", "Date"),
        ("() => new Date()", "Date"),
        ("() => Math.random()", "random"),
        ("() => setTimeout(() => 1, 0)", "setTimeout"),
        ("() => fetch('https://example.com')", "fetch"),
        ("() => Intl.NumberFormat", "Intl"),
        ("() => new WebAssembly.Module(new Uint8Array())", "WebAssembly"),
    ] {
        let e = error(src, vec![]);
        assert!(e.contains(missing) || e.starts_with("ReferenceError") || e.starts_with("TypeError"), "{src}: {e}");
    }
    // What stays: JSON, Math (less random), the standard library.
    assert_eq!(value("() => JSON.stringify({ a: [1, Math.max(2, 3)] })", vec![]), Some(t(r#"{"a":[1,3]}"#)));
    assert_eq!(value("() => (123.456).toLocaleString()", vec![]), Some(t("123.456")), "toLocaleString is toString");
}

#[test]
fn two_calls_share_nothing() {
    assert_eq!(value("() => { globalThis.counter = (globalThis.counter ?? 0) + 1; return globalThis.counter }", vec![]), Some(n(1.0)));
    assert_eq!(value("() => { globalThis.counter = (globalThis.counter ?? 0) + 1; return globalThis.counter }", vec![]), Some(n(1.0)));
}

#[test]
fn a_body_that_never_ends_runs_out_of_fuel() {
    let starved = ModuleLimits { fuel: 20_000_000, memory: 64 << 20 };
    assert_eq!(call_with("() => { while (true) {} }", vec![], &starved).unwrap_err(), RunError::OutOfFuel { fuel: 20_000_000 });
}

#[test]
fn the_same_call_answers_the_same_bytes() {
    let a = Engine::new().run(ENGINE, &ModuleLimits { fuel: 2_000_000_000, memory: 64 << 20 }, &[Some(t("(x) => ({ x, keys: Object.keys({ b: 1, a: 2 }) })")), Some(Value::List(vec![n(1.5)]))]).unwrap();
    let b = Engine::new().run(ENGINE, &ModuleLimits { fuel: 2_000_000_000, memory: 64 << 20 }, &[Some(t("(x) => ({ x, keys: Object.keys({ b: 1, a: 2 }) })")), Some(Value::List(vec![n(1.5)]))]).unwrap();
    assert_eq!(a, b);
}

/// What a call costs, printed: how the `js` defaults were chosen. Half a minute under the debug
/// interpreter, so it runs on request: `cargo test -p holon-module --test js_engine -- --ignored --no-capture`.
#[test]
#[ignore = "a measurement, not a check: run it to revisit the js defaults"]
fn what_a_call_costs() {
    let big = ModuleLimits { fuel: 4_000_000_000, memory: 256 << 20 };
    let engine = Engine::new();
    let measure = |label: &str, src: &str, args: Vec<Value>| {
        let (_, cost) = engine.run_measured(ENGINE, &big, &[Some(t(src)), Some(Value::List(args))]).unwrap();
        println!("{label}: fuel {} memory {} bytes", cost.fuel, cost.memory);
        cost
    };
    let trivial = measure("trivial", "(s) => s.toUpperCase() + '!'", vec![t("hi")]);
    let words: Vec<Value> = ["kitten", "sitting", "mitten", "written", "bitten", "fitting", "knitting", "smitten"].iter().map(|w| t(w)).collect();
    let heavy = measure(
        "levenshtein over 8 words",
        "(word, candidates) => { const dist = (a, b) => { const d = Array.from({ length: a.length + 1 }, (_, i) => [i]); for (let j = 1; j <= b.length; j++) d[0][j] = j; for (let i = 1; i <= a.length; i++) for (let j = 1; j <= b.length; j++) d[i][j] = Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1)); return d[a.length][b.length]; }; let best = { match: undefined, distance: Infinity }; for (const c of candidates) { const k = dist(word, c); if (k < best.distance) best = { match: c, distance: k }; } return best; }",
        vec![t("kittne"), Value::List(words)],
    );
    let loop_1e5 = measure("a loop of 100k", "() => { let s = 0; for (let i = 0; i < 100000; i++) s += i; return s }", vec![]);
    assert!(trivial.fuel > 0 && heavy.fuel > trivial.fuel && loop_1e5.fuel > heavy.fuel);
}

/// A call with the source alone compiles it and runs nothing: `check`'s question.
#[test]
fn the_source_alone_is_compiled_and_not_run() {
    assert_eq!(holon_module::compile_js("(a) => { return a + 1 }"), Ok(()));
    assert_eq!(holon_module::compile_js("(a) => { globalThis.ran = 1; throw new Error('never') }"), Ok(()), "compiled, not run");
    let e = holon_module::compile_js("(a) => {\n  const x = ;\n  return x\n}").unwrap_err();
    assert_eq!(e, "SyntaxError: unexpected token ';', primary expression at line 2, col 13 (native)");
    assert_eq!(holon_module::compile_js("42").unwrap_err(), "the source isn't a function");
}
