// What crosses a module's boundary, and what a call is a function of.
//
// A module is a unit like another: pure, pinned by its hash, placed as one node (docs/L4s/
// wasm-module-L4.md §8, I2 and I7). Two statements make that true of an engine that is one object
// for a whole process, with a cache of what it compiled, running code nobody here wrote:
//
// 1. A call is a function of its module and its inputs: its answer and what it cost are the same
//    on an engine just made and on one long used, whatever was called before it and in whatever
//    order.
// 2. What a module answers is data: every value comes back as it went in, and an answer holding
//    an effect gives the flow nothing.
//
// Both are held over a pool of values (numbers at their edges and past them, texts, bytes, nested
// lists and records, optionals, effects) and several modules, rather than over a case of each.

use std::collections::BTreeMap;

use holon_core::{encode_value, eval, MapResolver, ModuleLimits, Port, Type, Unit, UnitBody, Value};
use holon_module::{Cost, Engine, RunError, WithEngine, JS_ENGINE};

/// `run` answers its own input: the outputs are the inputs. `alloc` hands out a fixed address.
const ECHO: &str = r#"(module
    (memory (export "memory") 4)
    (func (export "alloc") (param i32) (result i32) i32.const 1024)
    (func (export "run") (param $ptr i32) (param $len i32) (result i64)
        (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32)) (i64.extend_i32_u (local.get $len)))))"#;

fn n(x: f64) -> Value {
    Value::Number(x)
}
fn t(s: &str) -> Value {
    Value::Text(s.into())
}
fn rec(fields: &[(&str, Value)]) -> Value {
    Value::Record(fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<BTreeMap<_, _>>())
}
fn none() -> Value {
    Value::Sum { arm: 1, payload: Box::new(Value::List(vec![])) }
}
fn effect(payload: Value) -> Value {
    Value::Effect { sink: "store_write:balance".into(), payload: Box::new(payload) }
}

/// Values a flow can hand a module, and a module can try to hand back.
fn pool() -> Vec<Value> {
    let nested = (0..60).fold(n(1.0), |v, _| Value::List(vec![v, t("x")]));
    vec![
        n(0.0),
        n(-0.0),
        n(1.5),
        n(-7.0),
        n(9007199254740993.0),
        n(f64::MAX),
        n(5e-324),
        n(f64::NAN),
        n(f64::INFINITY),
        n(f64::NEG_INFINITY),
        t(""),
        t("é\u{301}ﬃ 😀"),
        t(&"ab😀".repeat(2000)),
        Value::Boolean(true),
        Value::Boolean(false),
        Value::Bytes(vec![]),
        Value::Bytes((0..=255).collect()),
        Value::List(vec![]),
        Value::List(vec![n(1.0), t("two"), Value::Boolean(false), Value::List(vec![]), rec(&[])]),
        nested,
        rec(&[]),
        rec(&[("a", n(1.0)), ("b", rec(&[("c", Value::List(vec![n(f64::NAN)]))]))]),
        Value::Sum { arm: 0, payload: Box::new(n(3.0)) },
        Value::Sum { arm: 0, payload: Box::new(none()) },
        none(),
        effect(n(1_000_000.0)),
        Value::List(vec![n(1.0), effect(t("x"))]),
        rec(&[("rows", Value::List(vec![rec(&[("write", effect(n(1.0)))])]))]),
        Value::Sum { arm: 0, payload: Box::new(effect(n(1.0))) },
    ]
}

fn holds_effect(v: &Value) -> bool {
    match v {
        Value::Effect { .. } => true,
        Value::List(items) => items.iter().any(holds_effect),
        Value::Record(fields) => fields.values().any(holds_effect),
        Value::Sum { payload, .. } => holds_effect(payload),
        _ => false,
    }
}

/// An output as a module's answer gives it: `none` is absent, `some(x)` is `x` (the ABI, §3).
fn as_answered(v: &Value) -> Option<Value> {
    match v {
        Value::Sum { arm: 1, payload } if matches!(**payload, Value::List(ref xs) if xs.is_empty()) => None,
        Value::Sum { arm: 0, payload } => Some((**payload).clone()),
        other => Some(other.clone()),
    }
}

/// A call's answer and cost as bytes that are equal exactly when the two are: a value's encoding
/// (every NaN is one NaN there, and none equals itself otherwise), or the failure.
fn told(result: &Result<(Vec<Option<Value>>, Cost), RunError>) -> String {
    match result {
        Ok((outs, cost)) => {
            let outs: Vec<String> = outs.iter().map(|o| o.as_ref().map_or("-".into(), |v| format!("{:?}", encode_value(v).unwrap()))).collect();
            format!("{} | fuel {} memory {}", outs.join(" "), cost.fuel, cost.memory)
        }
        Err(e) => format!("failed: {e}"),
    }
}

/// **Guards:** a call of a module is a function of the module and its inputs. Its answer and its
/// cost (fuel burned, memory ended with) are the same on an engine just made and on one long
/// used, whatever was called on it before and in whatever order.
///
/// **Why:** one engine serves a whole process and keeps what it compiled; the JavaScript engine
/// is a module run on it, whose own realm could carry state from a call to the next. A run is
/// checked later by running it again somewhere else (`holon verify`, a replay in the editor), on
/// an engine that has seen other calls, or none.
///
/// **If it fails:** the same run gives two answers, or fits its budget on a server long up and
/// runs out of fuel when it is checked, so `holon verify` reports a run that happened as one that
/// didn't. That was so: the first call in a process paid for translating the functions it
/// reached, 15.8M fuel against 12.1M after.
#[test]
fn a_call_is_a_function_of_its_module_and_its_inputs() {
    let echo = wat::parse_str(ECHO).unwrap();
    let limits = ModuleLimits { fuel: 2_000_000_000, memory: 64 << 20 };
    // Each call: a module, and what it is given.
    let mut calls: Vec<(String, &[u8], Vec<Option<Value>>)> = Vec::new();
    for (i, v) in pool().into_iter().enumerate() {
        calls.push((format!("echo of value {i}"), &echo, vec![Some(v), Some(n(2.0)), None]));
    }
    let scripts = [
        "(x) => x",
        "(x) => [x, typeof x]",
        // A realm that kept anything from a call to the next would count.
        "(x) => { globalThis.kept = (globalThis.kept ?? 0) + 1; return globalThis.kept }",
        "(x) => { let s = 0; for (let i = 0; i < 2000; i++) s += i; return [s, x] }",
        "(x) => ({ keys: Object.keys({ b: 1, a: 2 }), now: typeof Date.now, x })",
    ];
    let given = [n(1.5), t("é 😀"), Value::List(vec![n(1.0), t("a")]), rec(&[("a", n(f64::INFINITY))]), n(f64::NAN), effect(n(1.0))];
    for script in scripts {
        for (i, v) in given.iter().enumerate() {
            calls.push((format!("{script} of value {i}"), JS_ENGINE, vec![Some(t(script)), Some(Value::List(vec![v.clone()]))]));
        }
    }
    // On one engine in order and then backwards, and on another backwards from the start.
    let (first, second) = (Engine::new(), Engine::new());
    let in_order: Vec<String> = calls.iter().map(|(_, module, inputs)| told(&first.run_measured(module, &limits, inputs))).collect();
    let mut again = vec![String::new(); calls.len()];
    let mut elsewhere = vec![String::new(); calls.len()];
    for (i, (_, module, inputs)) in calls.iter().enumerate().rev() {
        again[i] = told(&first.run_measured(module, &limits, inputs));
        elsewhere[i] = told(&second.run_measured(module, &limits, inputs));
    }
    let mut differing = Vec::new();
    for (i, (what, _, _)) in calls.iter().enumerate() {
        if in_order[i] != again[i] || in_order[i] != elsewhere[i] {
            let short = |s: &str| s.chars().rev().take(60).collect::<String>().chars().rev().collect::<String>();
            differing.push(format!("{what}: first …{}, again …{}, elsewhere …{}", short(&in_order[i]), short(&again[i]), short(&elsewhere[i])));
        }
    }
    assert!(differing.is_empty(), "{} calls differ by when or where they were made:\n{}", differing.len(), differing.join("\n"));
    assert!(calls.len() > 50 && in_order.iter().filter(|r| !r.starts_with("failed")).count() > 40, "the calls answer");
}

/// **Guards:** what a module answers is data. Every value handed to it comes back as it went in
/// (an optional as the ABI reads one: `none` an absent output, `some(x)` `x`), and an answer
/// holding an effect anywhere gives the flow nothing, the outputs beside it included.
///
/// **Why:** a module writes its answer's bytes as it likes, in an encoding that has a form for
/// an effect, and the host performs any effect that reaches a flow's outputs. An effect is to be
/// built only by a leaf that may build it, which is how a flow's manifest names everything the
/// flow performs.
///
/// **If it fails:** a module (a unit an agent staged, a holon from elsewhere) makes a flow write
/// or call what no leaf of the flow asked for and no manifest shows, within what the flow was
/// granted; or a value is changed by crossing, and a flow computes with another than it was given.
#[test]
fn what_a_module_answers_is_data_as_it_was_given() {
    let port = |name: &str| Port { name: name.into(), ty: Type::Any };
    let echo = Unit { body: UnitBody::Module { bytes: wat::parse_str(ECHO).unwrap() }, params: vec![], inputs: vec![port("a"), port("b")], outputs: vec![port("x"), port("y")] };
    let UnitBody::Module { bytes } = &echo.body else { unreachable!() };
    let (store, engine) = (MapResolver::new(), Engine::new());
    let with = WithEngine::new(&store, &engine);
    let same = |a: &Option<Value>, b: &Option<Value>| a.as_ref().map(|v| encode_value(v).unwrap()) == b.as_ref().map(|v| encode_value(v).unwrap());
    let (mut data, mut effects) = (0, 0);
    for (i, v) in pool().into_iter().enumerate() {
        // The engine itself answers what the module wrote, an effect like another value.
        let answered = engine.run(bytes, &ModuleLimits::default(), &[Some(v.clone()), Some(n(2.0))]).unwrap();
        assert!(same(&answered[0], &as_answered(&v)) && same(&answered[1], &Some(n(2.0))), "value {i}: the engine answers what it was given");
        // As a node of a flow: data as it was given, and nothing of an answer holding an effect.
        let outs = eval(&echo, &[Some(v.clone()), Some(n(2.0))], &with);
        let expected = if holds_effect(&v) { [None, None] } else { [as_answered(&v), Some(n(2.0))] };
        assert!(same(&outs[0], &expected[0]) && same(&outs[1], &expected[1]), "value {i} ({v:.80?}): as a node it gives {outs:.160?}");
        if holds_effect(&v) {
            effects += 1;
        } else {
            data += 1;
        }
    }
    assert!(data > 20 && effects >= 4, "{data} values of data, {effects} holding an effect");
}
