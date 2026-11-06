//! What the races don't cover, timed old against new (docs/engineering/themes/js-library-races/
//! leaves.md): one call at the top level, as the server makes it (its resolver keeps every
//! unit's shape, its observer reads what each node produced), and `map` and `fold` with leaf and
//! composite functions over numbers, texts and records, where giving can and can't apply.
//!
//! Each case prints one row, `| case | n | ns |`: the median of seven samples of as many calls as
//! fill about 20 ms, after one to warm up, as the races time. A filter argument
//! (`cargo bench -p holon-author --bench perf_audit -- fold`) runs the cases whose name has it.
//! Written against the public API alone, so the same file builds against the evaluator before
//! the change and after it, and the two binaries can be raced in turn.

mod race;

use std::hint::black_box;
use std::time::{Duration, Instant};

use holon_core::{eval, eval_observed, hash_unit, interface_of, MapResolver, Observer, Param, Port, Resolver, Shape, Shapes, Slot, Type, Unit, Value};
use race::{function, leaf, num, numbers, text, v, Composite};

/// A resolver that keeps every unit's shape, as the server's does (crates/holon/src/units.rs,
/// `Loaded::shape`), over a store nothing is added to while it is used.
struct Keeping<'a> {
    store: &'a MapResolver,
    shapes: Shapes,
}

impl<'a> Keeping<'a> {
    fn over(store: &'a MapResolver) -> Keeping<'a> {
        Keeping { store, shapes: Shapes::of(store.units()) }
    }
}

impl Resolver for Keeping<'_> {
    fn resolve(&self, hash: &holon_core::Hash) -> Option<&Unit> {
        self.store.resolve(hash)
    }
    fn shape(&self, unit: &Unit) -> Option<&Shape> {
        self.shapes.get(unit)
    }
}

/// An observer that reads what each node produced, as the host's recorder does (it keeps each
/// output down to three levels, crates/holon/src/recorder.rs `Timer`), without a clock.
#[derive(Default)]
struct Reading {
    kept: Vec<(Vec<u32>, Vec<Option<Value>>)>,
}

impl Observer for Reading {
    fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
        if path.len() <= 3 {
            self.kept.push((path.to_vec(), outputs.iter().map(|s| s.value().cloned()).collect()));
        }
    }
}

/// The median time of one call of `once`, in nanoseconds.
fn time(mut once: impl FnMut()) -> f64 {
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

fn row(case: &str, n: usize, ns: f64) {
    println!("| {case} | {n} | {ns:.1} |");
}

/// A compiled function's own unit as the store holds it (the harness keeps a copy outside it),
/// so a resolver keeping shapes by address finds the top unit's too.
fn stored(c: &Composite) -> &Unit {
    c.store.resolve(&hash_unit(&c.unit).expect("hashed")).expect("stored")
}

/// One call at the top level of `c` on `inputs`, four ways: plain `eval` and under a reading
/// observer, each with a resolver that keeps no shapes (the races', the tests') and one that
/// keeps them (the server's).
fn top_level(name: &str, c: &Composite, n: usize, inputs: &[Value]) {
    let ins: Vec<Option<Value>> = inputs.iter().cloned().map(Some).collect();
    let unit = stored(c);
    let keeping = Keeping::over(&c.store);
    let want = eval(unit, &ins, &c.store).into_vec();
    assert_eq!(eval_observed(unit, &ins, &keeping, &mut Reading::default()).into_vec(), want, "{name}");
    let case = |how: &str| format!("top {name}, {how}");
    if race::wanted(&case("")) {
        row(&case("eval, map resolver"), n, time(|| {
            black_box(eval(unit, black_box(&ins), &c.store));
        }));
        row(&case("eval, shapes kept"), n, time(|| {
            black_box(eval(unit, black_box(&ins), &keeping));
        }));
        row(&case("reading observer, map resolver"), n, time(|| {
            let mut seen = Reading::default();
            black_box(eval_observed(unit, black_box(&ins), &c.store, &mut seen));
            black_box(seen);
        }));
        row(&case("reading observer, shapes kept"), n, time(|| {
            let mut seen = Reading::default();
            black_box(eval_observed(unit, black_box(&ins), &keeping, &mut seen));
            black_box(seen);
        }));
    }
}

/// A `map` or `fold` placed bare, its function a leaf: what the reader places for `xs.map(f)`
/// when `f` is a leaf is a composite, so these are the core's own shapes (a `fold` of `add` is
/// criterion's `fold_sum`).
struct Bare {
    unit: Unit,
    store: MapResolver,
}

fn hex(h: &holon_core::Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// `name(fn: f)` with `extras` more inputs after the list (a `map` passes them after the item).
fn bare(name: &str, f: Unit, extras: usize) -> Bare {
    let mut store = MapResolver::new();
    let hf = store.insert(f).expect("stored");
    let mut unit = interface_of(name).expect("a leaf");
    unit.params.retain(|p| p.name() != "fn");
    unit.params.push(Param::frozen("fn", hex(&hf)));
    for k in 0..extras {
        unit.inputs.push(Port { name: format!("extra{k}"), ty: Type::Any });
    }
    Bare { unit, store }
}

fn run_bare(name: &str, b: &Bare, n: usize, inputs: &[Value]) {
    if !race::wanted(name) {
        return;
    }
    let ins: Vec<Option<Value>> = inputs.iter().cloned().map(Some).collect();
    assert!(eval(&b.unit, &ins, &b.store).into_vec()[0].is_some(), "{name} gives a value");
    row(name, n, time(|| {
        black_box(eval(&b.unit, black_box(&ins), &b.store));
    }));
}

fn run_composite(name: &str, c: &Composite, n: usize, inputs: &[Value]) {
    if !race::wanted(name) {
        return;
    }
    let ins: Vec<Option<Value>> = inputs.iter().cloned().map(Some).collect();
    assert!(eval(&c.unit, &ins, &c.store).into_vec()[0].is_some(), "{name} gives a value");
    row(name, n, time(|| {
        black_box(eval(&c.unit, black_box(&ins), &c.store));
    }));
}

const LIN: &str = "function lin(x) {\n  return x * 2 + 1\n}";
const ABS: &str = "function abs(x) {\n  return x <= 0 ? 0 - x : x\n}";
const SCORE: &str = "function score(a, b) {
  const s = a + b
  const d = a - b
  const q = s * d / 4
  return q > 100 ? q - 100 : q < 0 ? 0 - q : q * 3 + 1
}";
const LAST: &str = "import { length, get } from \"holon\"\nfunction last(xs) {\n  return get(xs, length(xs) - 1)\n}";

/// About a hundred nodes: four calls of [`SCORE`] and the sums of their answers.
fn big() -> String {
    format!("{SCORE}\n\nfunction big(a, b) {{\n  return score(a, b) + score(b, a) + score(a + 1, b) + score(a, b + 1)\n}}")
}

/// A handler's usual work: fields read out of a record and a record made of them.
const SHAPE: &str = "import { length } from \"holon\"
function shape(r) {
  return { a: r.x + 1, b: r.y * 2, c: `${r.name}!`, d: length(r.tags), e: r.x > r.y ? \"x\" : \"y\" }
}";

fn row_record(k: usize) -> Value {
    v(&format!("{{\"x\": {k}, \"y\": {}, \"name\": \"item {k}\", \"tags\": [\"a\", \"b\"], \"a\": {k}, \"b\": 2}}", k % 7))
}

fn rows(n: usize) -> Value {
    Value::List((0..n).map(row_record).collect())
}

fn texts(n: usize) -> Value {
    Value::List((0..n).map(|k| text(&format!("word{k:05}"))).collect())
}

fn main() {
    // 1a. One call at the top level, small to large, under the server's resolver and observer.
    let lin = function(LIN, "lin", 1);
    top_level("lin (5 leaves)", &lin, 1, &[num(3.0)]);
    let abs = function(ABS, "abs", 1);
    top_level("abs (9 leaves, a choice)", &abs, 1, &[num(-3.0)]);
    let score = function(SCORE, "score", 2);
    top_level("score (25 nodes)", &score, 1, &[num(30.0), num(1.0)]);
    let big = function(&big(), "big", 2);
    top_level("big (4 nested score)", &big, 1, &[num(30.0), num(1.0)]);
    let shape = function(SHAPE, "shape", 1);
    top_level("shape (a record read and made)", &shape, 1, &[row_record(3)]);
    let last = function(LAST, "last", 1);
    for n in [64, 4096] {
        top_level("last (list used twice)", &last, n, &[numbers(n)]);
    }

    // 1c. `map` and `fold`: leaf functions over numbers, composite functions over numbers, and
    // over texts and records, where nothing can be given (each item is lent).
    let sizes = [16, 256, 4096];
    let add = leaf("add", &[]);
    let fold_add = bare("fold", add.clone(), 0);
    let map_add = bare("map", add.clone(), 1);
    let map_upper = bare("map", leaf("uppercase", &[]), 0);
    let map_field = bare("map", leaf("field", &[("name", "x")]), 0);
    let fold_c = function("function f(xs) {\n  return xs.reduce((acc, x) => acc + x * 2, 0)\n}", "f", 1);
    let map_c = function("function f(xs) {\n  return xs.map((x) => x * 2 + 1)\n}", "f", 1);
    let map_concat = function("function f(xs) {\n  return xs.map((s) => `${s}!`)\n}", "f", 1);
    let map_len = function("import { text_length } from \"holon\"\nfunction f(xs) {\n  return xs.map((s) => text_length(s) + 1)\n}", "f", 1);
    let fold_len = function("import { text_length } from \"holon\"\nfunction f(xs) {\n  return xs.reduce((acc, s) => acc + text_length(s), 0)\n}", "f", 1);
    let map_fields = function("function f(rs) {\n  return rs.map((r) => r.a + r.b)\n}", "f", 1);
    let map_spread = function("function f(rs) {\n  return rs.map((r) => ({ ...r, c: 1 }))\n}", "f", 1);
    let fold_field = function("function f(rs) {\n  return rs.reduce((acc, r) => acc + r.a, 0)\n}", "f", 1);
    let map_shape = function(&format!("{SHAPE}\n\nfunction f(rs) {{\n  return rs.map((r) => shape(r))\n}}"), "f", 1);
    for &n in &sizes {
        run_bare("fold(add) over numbers", &fold_add, n, &[num(0.0), numbers(n)]);
        run_bare("map(add, 1) over numbers", &map_add, n, &[numbers(n), num(1.0)]);
        run_composite("fold, composite acc + x * 2, numbers", &fold_c, n, &[numbers(n)]);
        run_composite("map, composite x * 2 + 1, numbers", &map_c, n, &[numbers(n)]);
        run_bare("map(uppercase) over texts", &map_upper, n, &[texts(n)]);
        run_composite("map, composite `${s}!`, texts", &map_concat, n, &[texts(n)]);
        run_composite("map, composite text_length(s) + 1, texts", &map_len, n, &[texts(n)]);
        run_composite("fold, composite acc + text_length(s), texts", &fold_len, n, &[texts(n)]);
        run_bare("map(field x) over records", &map_field, n, &[rows(n)]);
        run_composite("map, composite r.a + r.b, records", &map_fields, n, &[rows(n)]);
        run_composite("map, composite {...r, c: 1}, records", &map_spread, n, &[rows(n)]);
        run_composite("fold, composite acc + r.a, records", &fold_field, n, &[rows(n)]);
        run_composite("map, composite shape(r), records", &map_shape, n, &[rows(n)]);
    }

    // Goal 3. What speculative giving is for: a value one builder reads and a `coalesce`'s
    // fallback reads too, in a fold that grows it.
    let spec_append = function("import { append } from \"holon\"\nfunction f(xs) {\n  return xs.reduce((acc, x) => append(acc, x) ?? acc, [])\n}", "f", 1);
    let spec_insert = function("import { insert } from \"holon\"\nfunction f(xs) {\n  return xs.reduce((acc, x) => insert(acc, x, x) ?? acc, {})\n}", "f", 1);
    let spec_filter = function("function f(xs) {\n  return xs.reduce((acc, x) => x > 10 ? [...acc, x] : acc, [])\n}", "f", 1);
    let spec_group = function(
        "import { insert } from \"holon\"\nfunction f(rs) {\n  return rs.reduce((acc, r) => r.b > 1 ? insert(acc, r.name, r.a) : acc, {})\n}",
        "f",
        1,
    );
    for &n in &sizes {
        run_composite("spec: fold append(acc, x) ?? acc", &spec_append, n, &[numbers(n)]);
        run_composite("spec: fold insert(acc, x, x) ?? acc", &spec_insert, n, &[texts(n)]);
        run_composite("spec: fold x > 10 ? [...acc, x] : acc", &spec_filter, n, &[numbers(n)]);
        run_composite("spec: fold r.b > 1 ? insert(acc, r.name, r.a) : acc", &spec_group, n, &[rows(n)]);
    }

    // Goal 4. What `unneeded` looks through: a fold's step that grows its accumulator and also
    // reads it inside a branch not taken, behind k diamonds (each a value read by two nodes that
    // meet again). The accumulator can be given to the builder only once the walk sees that the
    // branch's reader will never run, which `unneeded` decides by looking up its readers.
    for k in [1, 2, 3, 4, 5, 6] {
        let c = function(&deep(k), "f", 1);
        for n in [256, 4096] {
            run_composite(&format!("unneeded: fold, acc also read behind {k} diamonds not taken"), &c, n, &[numbers(n)]);
        }
    }
}

/// A fold whose step appends to its accumulator and reads its length inside a branch never taken
/// (`x < 0` over `0..n`), through `k` diamonds.
fn deep(k: usize) -> String {
    let mut s = String::from("import { length } from \"holon\"\nfunction step(acc, x) {\n  const n0 = length(acc)\n");
    for i in 1..=k {
        let p = i - 1;
        s += &format!("  const a{i} = n{p} + 1\n  const b{i} = n{p} * 2\n  const n{i} = a{i} + b{i}\n");
    }
    s += &format!("  const r = x < 0 ? n{k} : 0\n  return [...acc, x + r]\n}}\n\nfunction f(xs) {{\n  return xs.reduce((acc, x) => step(acc, x), [])\n}}");
    s
}
