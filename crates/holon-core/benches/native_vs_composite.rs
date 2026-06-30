//! native_vs_composite — does the native set *earn* its speed?
//!
//! The §13 admission gate lets a primitive into the privileged, unforkable core on a
//! *speed* ground in two cases:
//!   (a) complexity class — the composite form is a worse big-O (`head`/`length` are
//!       O(1) native but O(n) through `fold`);
//!   (b) hot & pervasive — same class, but paid in the inner loop (arithmetic, etc.).
//! Those are claims the design *asserts*. This bench *measures* them: it races each
//! speed-justified atom, through the real `eval`, against the exact composite donor the
//! design says it could have been. No pass/fail threshold — criterion prints ns and
//! ratios; a human reads whether the atom cashes its claim. An atom that does NOT pull
//! ahead is a real signal it is in the trust root on a claim it cannot back.
//!
//! It also probes one thing the source predicts: `eval` gathers a primitive's inputs by
//! CLONING them (`inputs.iter().flatten().cloned().collect()`), so an O(1) primitive over
//! a big list still pays an O(n) copy *at the evaluator*. If the "O(1)" inspectors below
//! climb with size, the culprit is that marshalling clone — a finding about the
//! evaluator, not the primitive, and a concrete optimisation (gather by reference).
//!
//! Add to crates/holon-core/Cargo.toml:
//!   [dev-dependencies]
//!   criterion = "0.5"
//!   [[bench]]
//!   name = "native_vs_composite"
//!   harness = false
//! Run:  cargo bench -p holon-core

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use holon_core::{
    eval, Edge, Hash, MapResolver, Param, Port, Type, Unit, UnitBody, Value, BOUNDARY,
};

// ── constructors (mirroring the crate's own eval-test helpers) ────────────────

fn num(n: f64) -> Value {
    Value::Number(n)
}

/// A list value `[0.0, 1.0, … , n-1]`.
fn list_of(n: usize) -> Value {
    Value::List((0..n).map(|i| num(i as f64)).collect())
}

/// A primitive with `n_in` `Any`-typed input ports and one output — the exact shape
/// used by holon-core's `eval` tests.
fn prim(name: &str, n_in: usize, out: Type) -> Unit {
    Unit {
        body: UnitBody::Primitive { name: name.into() },
        params: vec![],
        inputs: (0..n_in)
            .map(|i| Port { name: format!("i{i}"), ty: Type::Any })
            .collect(),
        output: Some(out),
    }
}

/// Lowercase 64-char hex of a 32-byte hash. The crate's own `to_hex` is `pub(crate)`,
/// so a bench (which sees only the public API) reimplements its identical two lines —
/// needed to put a unit's hash into a `fold`/`map` `fn` param.
fn to_hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// `sum` expressed as `fold(add, init=0, list)` — the universal-donor path, and exactly
/// the crate's `fold_sums_with_add` shape (bare `add` as the combiner, referenced by hash).
fn fold_sum(r: &mut MapResolver) -> Unit {
    let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
    Unit {
        body: UnitBody::Primitive { name: "fold".into() },
        params: vec![Param::frozen("fn", to_hex(&hadd))],
        inputs: vec![
            Port { name: "init".into(), ty: Type::Any },
            Port { name: "list".into(), ty: Type::list_any() },
        ],
        output: Some(Type::Any),
    }
}

/// `add` wrapped in a one-child composite — the `add2` test shape. Wiring:
/// boundary.in0 → add.0, boundary.in1 → add.1, add.out → boundary.out.
fn wrapped_add(r: &mut MapResolver) -> Unit {
    let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
    Unit {
        body: UnitBody::Composite {
            children: vec![hadd],
            edges: vec![
                Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 1 },
                Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![
            Port { name: "a".into(), ty: Type::Number },
            Port { name: "b".into(), ty: Type::Number },
        ],
        output: Some(Type::Number),
    }
}

const SIZES: &[usize] = &[1, 8, 64, 512, 4096];

// ── (a) complexity class: O(1) native inspectors + native vs fold traversal ───
fn traversal(c: &mut Criterion) {
    let mut r = MapResolver::new();
    let folded = fold_sum(&mut r);

    let head = prim("head", 1, Type::Any);
    let length = prim("length", 1, Type::Number);
    let is_empty = prim("is_empty", 1, Type::Boolean);
    let native_sum = prim("sum", 1, Type::Number);

    let mut g = c.benchmark_group("traversal");
    for &n in SIZES {
        let xs = list_of(n);
        // Inputs are built ONCE per size, outside the timed loop, so we measure `eval`
        // and not our own list construction.
        let one = [Some(xs.clone())];
        let init_list = [Some(num(0.0)), Some(xs)];

        // gate says O(1) — flat across n iff eval's input-clone does not dominate
        g.bench_with_input(BenchmarkId::new("native_head", n), &n, |b, _| {
            b.iter(|| black_box(eval(&head, &one, &r)))
        });
        g.bench_with_input(BenchmarkId::new("native_length", n), &n, |b, _| {
            b.iter(|| black_box(eval(&length, &one, &r)))
        });
        g.bench_with_input(BenchmarkId::new("native_is_empty", n), &n, |b, _| {
            b.iter(|| black_box(eval(&is_empty, &one, &r)))
        });
        // O(n) traversal: native (tight Rust loop) vs the fold composite (one full
        // `eval` recursion + resolver lookup + clone per element).
        g.bench_with_input(BenchmarkId::new("native_sum", n), &n, |b, _| {
            b.iter(|| black_box(eval(&native_sum, &one, &r)))
        });
        g.bench_with_input(BenchmarkId::new("fold_sum", n), &n, |b, _| {
            b.iter(|| black_box(eval(&folded, &init_list, &r)))
        });
    }
    g.finish();
}

// ── (b) the composite-wrapping tax: what one layer of indirection costs ───────
fn wrapping_tax(c: &mut Criterion) {
    let mut r = MapResolver::new();
    let native = prim("add", 2, Type::Number);
    let wrapped = wrapped_add(&mut r);
    let inp = [Some(num(8.0)), Some(num(9.0))];

    let mut g = c.benchmark_group("wrapping_tax");
    g.bench_function("native_add", |b| b.iter(|| black_box(eval(&native, &inp, &r))));
    g.bench_function("composite_add", |b| b.iter(|| black_box(eval(&wrapped, &inp, &r))));
    g.finish();
}

criterion_group!(benches, traversal, wrapping_tax);
criterion_main!(benches);
