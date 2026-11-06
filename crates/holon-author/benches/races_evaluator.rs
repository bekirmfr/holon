//! The evaluator: what a composite pays per node and per call, raced against what a native leaf
//! would pay, so the evaluator's own costs are seen apart from any one gap
//! (docs/engineering/themes/js-library-races/leaves.md).
//!
//! - a walked composite (`abs`, with a choice) and a planned one (`lin`, without), mapped, where a
//!   function is hot: the per-node cost of a walk and of a plan; and `lin` written in place, so
//!   the call of a function inside a function is seen apart;
//! - a list used twice, which passes through a shared `guard(true, xs)`: whether the guard copies
//!   it, once per call, once per item for a function mapped over lists, or once per item for a
//!   list a function written in place captures;
//! - folds that grow their accumulator (`filter`, `reverse`, a record built key by key): whether
//!   the fold copies it at each step (O(n²)) or grows it in place (O(n));
//! - one call of a composite of about twenty nodes, and the same function mapped: what preparing a
//!   composite costs per call, apart from running it;
//! - each leaf whose body was rewritten (`join`, `concat`, `slice`, `append`) against its body as it
//!   was, on the same inputs ("as it was");
//! - a deep copy against an `Arc`'s, for whether values should be kept behind one ("arc").

mod race;

use std::collections::BTreeMap;

use holon_core::Value;
use race::{check, function, leaf, mapped, num, numbers, race, text, v, Composite, Side};

fn abs(xs: &[&Value]) -> Option<Value> {
    let [Value::Number(x)] = xs else { return None };
    Some(Value::Number(x.abs()))
}

fn lin(xs: &[&Value]) -> Option<Value> {
    let [Value::Number(x)] = xs else { return None };
    Some(Value::Number(x * 2.0 + 1.0))
}

/// `xs.filter((x) => x > 10)`.
fn filter(xs: &[&Value]) -> Option<Value> {
    let [Value::List(items)] = xs else { return None };
    let mut out = Vec::new();
    for x in items {
        let Value::Number(n) = x else { return None };
        if *n > 10.0 {
            out.push(x.clone());
        }
    }
    Some(Value::List(out))
}

/// `xs.toReversed()`.
fn reverse(xs: &[&Value]) -> Option<Value> {
    let [Value::List(items)] = xs else { return None };
    Some(Value::List(items.iter().rev().cloned().collect()))
}

/// `Object.fromEntries(xs.map((k) => [k, k]))`, each item a text.
fn entries(xs: &[&Value]) -> Option<Value> {
    let [Value::List(items)] = xs else { return None };
    let mut out = BTreeMap::new();
    for x in items {
        let Value::Text(k) = x else { return None };
        out.insert(k.clone(), x.clone());
    }
    Some(Value::Record(out))
}

/// [`SCORE`], natively.
fn score_of(a: f64, b: f64) -> f64 {
    let s = a + b;
    let d = a - b;
    let q = s * d / 4.0;
    if q > 100.0 {
        q - 100.0
    } else if q < 0.0 {
        0.0 - q
    } else {
        q * 3.0 + 1.0
    }
}

fn score(xs: &[&Value]) -> Option<Value> {
    let [Value::Number(a), Value::Number(b)] = xs else { return None };
    Some(Value::Number(score_of(*a, *b)))
}

/// [`SCORE`] of each item and 7, natively.
fn each_score(xs: &[&Value]) -> Option<Value> {
    let [Value::List(items)] = xs else { return None };
    items.iter().map(|x| match x {
        Value::Number(a) => Some(Value::Number(score_of(*a, 7.0))),
        _ => None,
    }).collect::<Option<Vec<_>>>().map(Value::List)
}

/// `xs.at(-1)`.
fn last(xs: &[&Value]) -> Option<Value> {
    let [Value::List(items)] = xs else { return None };
    items.last().cloned()
}

/// `idx.map((i) => ys[ys.length - 1 - i])`, each index in range.
fn mirror(xs: &[&Value]) -> Option<Value> {
    let [Value::List(idx), Value::List(ys)] = xs else { return None };
    idx.iter().map(|i| match i {
        Value::Number(i) if *i >= 0.0 && (*i as usize) < ys.len() => Some(ys[ys.len() - 1 - *i as usize].clone()),
        _ => None,
    }).collect::<Option<Vec<_>>>().map(Value::List)
}

/// Math.abs: `0 - x` at and below zero keeps `-0` out (races_numbers.rs's pilot).
const ABS: &str = "function abs(x) {\n  return x <= 0 ? 0 - x : x\n}";

/// A function with no choice, so its walk is planned once for a whole list.
const LIN: &str = "function lin(x) {\n  return x * 2 + 1\n}";

/// [`LIN`] written in place: what `xs.map((x) => lin(x))` pays beyond it is the call of a function
/// inside a function.
const LIN_IN_PLACE: &str = "function each_lin(xs) {\n  return xs.map((x) => x * 2 + 1)\n}";

/// The last item: the list is used twice, so it passes through a shared `guard(true, xs)`.
const LAST: &str = "import { length, get } from \"holon\"\nfunction last(xs) {\n  return get(xs, length(xs) - 1)\n}";

/// Each index's item counted from the end: the function written in place reads the list it
/// captures twice for each item.
const MIRROR: &str = "import { length, get } from \"holon\"\nfunction mirror(idx, ys) {\n  return idx.map((i) => get(ys, length(ys) - 1 - i))\n}";

/// The items over ten, kept by a fold that grows its accumulator: each step spreads it.
const FILTER: &str = "function filter(xs) {\n  return xs.reduce((acc, x) => x > 10 ? [...acc, x] : acc, [])\n}";

/// The items in reverse, by a fold that puts each item in front of its accumulator.
const REVERSE: &str = "function reverse(xs) {\n  return xs.reduce((acc, x) => [x, ...acc], [])\n}";

/// A record of each text under itself, by a fold that inserts one key a step (`insert` has no
/// JavaScript form, so it is called by name).
const ENTRIES: &str = "import { insert } from \"holon\"\nfunction entries(xs) {\n  return xs.reduce((acc, x) => insert(acc, x, x), {})\n}";

/// About twenty nodes: arithmetic, three comparisons and two choices.
const SCORE: &str = "function score(a, b) {
  const s = a + b
  const d = a - b
  const q = s * d / 4
  return q > 100 ? q - 100 : q < 0 ? 0 - q : q * 3 + 1
}";

/// [`SCORE`] applied to each item of a list with 7: its preparation paid once for the list.
fn each_score_composite() -> Composite {
    let mut c = function(&format!("{SCORE}\n\nfunction each_score(xs) {{\n  return xs.map((x) => score(x, 7))\n}}"), "each_score", 1);
    c.name = "xs.map((x) => score(x, 7))".into();
    c
}

fn texts(n: usize) -> Value {
    Value::List((0..n).map(|k| text(&format!("k{k}"))).collect())
}

/// 64 lists of `n` numbers each.
fn lists_of(n: usize) -> Value {
    Value::List((0..64).map(|_| numbers(n)).collect())
}

fn main() {
    // (a) A walked composite, mapped.
    let abs_m = mapped(ABS, "abs");
    let lists: Vec<Vec<Value>> = vec![vec![v("[]")], vec![v("[-3.5, -0.0, 0, 2, -1e308, 5e-324]")]];
    check("abs, mapped", &Side::Composite(&abs_m), &Side::NativeMapped(abs), &lists);
    race("abs, mapped", &Side::Composite(&abs_m), &Side::NativeMapped(abs), &[1, 64, 4096], |n| vec![numbers(n)]);

    // (b) A planned composite, mapped.
    let lin_m = mapped(LIN, "lin");
    check("lin, mapped", &Side::Composite(&lin_m), &Side::NativeMapped(lin), &lists);
    race("lin, mapped", &Side::Composite(&lin_m), &Side::NativeMapped(lin), &[1, 64, 4096], |n| vec![numbers(n)]);
    // The same function written in place: the difference is the call of `lin` inside the map's
    // function, a composite inside a composite.
    let lin_p = function(LIN_IN_PLACE, "each_lin", 1);
    check("lin in place, mapped", &Side::Composite(&lin_p), &Side::NativeMapped(lin), &lists);
    race("lin in place, mapped", &Side::Composite(&lin_p), &Side::NativeMapped(lin), &[1, 64, 4096], |n| vec![numbers(n)]);

    // A list used twice passes through a shared `guard(true, xs)`: one call, then mapped over 64
    // lists (each item lent to the function), then a captured list read twice per item.
    let last_c = function(LAST, "last", 1);
    let cases: Vec<Vec<Value>> = vec![vec![v("[]")], vec![v("[1]")], vec![v("[1, 2, 3]")], vec![v("[[1], [2]]")]];
    check("last, list used twice", &Side::Composite(&last_c), &Side::Native(last), &cases);
    race("last, list used twice", &Side::Composite(&last_c), &Side::Native(last), &[64, 512, 4096], |n| vec![numbers(n)]);
    let last_m = mapped(LAST, "last");
    race("last, mapped over 64 lists of n", &Side::Composite(&last_m), &Side::NativeMapped(last), &[64, 512, 4096], |n| vec![lists_of(n)]);
    let mirror_c = function(MIRROR, "mirror", 2);
    let cases: Vec<Vec<Value>> = vec![vec![v("[]"), v("[]")], vec![v("[0]"), v("[5]")], vec![v("[0, 2, 1]"), v("[1, 2, 3]")]];
    check("mirror, captured list read twice per item", &Side::Composite(&mirror_c), &Side::Native(mirror), &cases);
    race("mirror, captured list read twice per item", &Side::Composite(&mirror_c), &Side::Native(mirror), &[64, 512, 4096], |n| vec![numbers(n), numbers(n)]);

    // (c) A fold that keeps some items, spreading its accumulator into a new list each step.
    let filter_c = function(FILTER, "filter", 1);
    let cases: Vec<Vec<Value>> = vec![vec![v("[]")], vec![v("[11]")], vec![v("[10]")], vec![v("[3, 12, 10, 40, -0.0, 11]")]];
    check("filter, by fold", &Side::Composite(&filter_c), &Side::Native(filter), &cases);
    race("filter, by fold", &Side::Composite(&filter_c), &Side::Native(filter), &[64, 512, 4096], |n| vec![numbers(n)]);

    // (d) A fold that prepends: `[x, ...acc]` is `extend([x], acc)`.
    let reverse_c = function(REVERSE, "reverse", 1);
    let cases: Vec<Vec<Value>> = vec![vec![v("[]")], vec![v("[1]")], vec![v("[1, 2, 3, -0.0]")]];
    check("reverse, by fold", &Side::Composite(&reverse_c), &Side::Native(reverse), &cases);
    race("reverse, by fold", &Side::Composite(&reverse_c), &Side::Native(reverse), &[64, 512, 4096], |n| vec![numbers(n)]);

    // (e) A fold that builds a record, a key a step.
    let entries_c = function(ENTRIES, "entries", 1);
    let cases: Vec<Vec<Value>> = vec![vec![v("[]")], vec![v("[\"a\"]")], vec![v("[\"b\", \"a\", \"b\", \"\"]")]];
    check("record, by fold", &Side::Composite(&entries_c), &Side::Native(entries), &cases);
    race("record, by fold", &Side::Composite(&entries_c), &Side::Native(entries), &[64, 512, 4096], |n| vec![texts(n)]);

    // (f) One call of a composite of about twenty nodes, then the same function mapped: the
    // difference per call is its preparation.
    let score_c = function(SCORE, "score", 2);
    let pairs: Vec<Vec<Value>> = [(1.0, 2.0), (30.0, 1.0), (40.0, 2.0), (-0.0, 0.0), (5.0, -50.0)].iter().map(|&(a, b)| vec![num(a), num(b)]).collect();
    check("score, one call", &Side::Composite(&score_c), &Side::Native(score), &pairs);
    race("score, one call", &Side::Composite(&score_c), &Side::Native(score), &[1], |_| vec![num(30.0), num(1.0)]);
    let score_m = each_score_composite();
    check("score, mapped", &Side::Composite(&score_m), &Side::Native(each_score), &lists);
    race("score, mapped", &Side::Composite(&score_m), &Side::Native(each_score), &[1, 64], |n| vec![Value::List((0..n).map(|k| num(k as f64 * 3.0 - 40.0)).collect())]);

    // The leaves changed, each against its body as it was, both behind a leaf's dispatch.
    let words = |n: usize| Value::List((0..n).map(|k| text(&format!("word{k:05}"))).collect());
    let long = |n: usize| text(&"abcdefghij".repeat(n / 10));
    let join = leaf("join", &[]);
    race("join, against as it was", &Side::Leaf(&join), &Side::Native(join_as_it_was), &[64, 4096], |n| vec![words(n), text(", ")]);
    let concat = leaf("concat", &[]);
    race("concat of two texts of n bytes, against as it was", &Side::Leaf(&concat), &Side::Native(concat_as_it_was), &[64, 10_000], |n| vec![long(n), long(n)]);
    let slice = leaf("slice", &[]);
    race("slice of half a text of n bytes, against as it was", &Side::Leaf(&slice), &Side::Native(slice_as_it_was), &[64, 10_000], |n| vec![long(n), num((n / 4) as f64), num((n / 2) as f64)]);
    let append = leaf("append", &[]);
    race("append to a list lent, against as it was", &Side::Leaf(&append), &Side::Native(append_as_it_was), &[64, 4096], |n| vec![numbers(n), num(1.0)]);

    arc_estimate();
}

fn join_as_it_was(xs: &[&Value]) -> Option<Value> {
    let [Value::List(items), Value::Text(sep)] = xs else { return None };
    let mut parts = Vec::new();
    for v in items {
        let Value::Text(s) = v else { return None };
        parts.push(s.to_string());
    }
    Some(Value::Text(parts.join(sep)))
}

fn concat_as_it_was(xs: &[&Value]) -> Option<Value> {
    let mut s = String::new();
    for v in xs {
        let Value::Text(t) = v else { return None };
        s.push_str(t);
    }
    Some(Value::Text(s))
}

fn slice_as_it_was(xs: &[&Value]) -> Option<Value> {
    let [Value::Text(s), Value::Number(start), Value::Number(len)] = xs else { return None };
    Some(Value::Text(s.chars().skip(*start as usize).take(*len as usize).collect()))
}

fn append_as_it_was(xs: &[&Value]) -> Option<Value> {
    let [Value::List(l), item] = xs else { return None };
    let mut out = l.to_vec();
    out.push((*item).clone());
    Some(Value::List(out))
}

/// What sharing lists, texts and records behind an `Arc` would save where a value is copied: a
/// deep clone of a list of 64 records and of a 10 KB text, against an `Arc` clone (a count
/// bumped). Printed as the races print.
fn arc_estimate() {
    use std::hint::black_box;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    if !race::wanted("arc") {
        return;
    }
    fn per_call(mut f: impl FnMut()) -> Duration {
        let mut calls = 1u32;
        loop {
            let start = Instant::now();
            for _ in 0..calls {
                f();
            }
            if start.elapsed() >= Duration::from_millis(50) {
                break;
            }
            calls *= 2;
        }
        let mut runs: Vec<Duration> = (0..7)
            .map(|_| {
                let start = Instant::now();
                for _ in 0..calls {
                    f();
                }
                start.elapsed() / calls
            })
            .collect();
        runs.sort();
        runs[3]
    }
    let record = |k: usize| v(&format!("{{\"id\": {k}, \"name\": \"item {k}\", \"tags\": [\"a\", \"b\"], \"price\": 9.5}}"));
    let records = Value::List((0..64).map(record).collect());
    let ten_kb = text(&"abcdefghij".repeat(1024));
    println!("### a copy, deep or shared\n\n| value | deep clone | Arc clone | ratio |\n|---|---:|---:|---:|");
    for (name, value) in [("64 records of 4 fields", records), ("a 10 KB text", ten_kb)] {
        let shared = Arc::new(value.clone());
        let deep = per_call(|| drop(black_box(black_box(&value).clone())));
        let arc = per_call(|| drop(black_box(Arc::clone(black_box(&shared)))));
        println!("| {name} | {deep:.2?} | {arc:.2?} | {:.0} |", deep.as_secs_f64() / arc.as_secs_f64());
    }
    println!();
}
