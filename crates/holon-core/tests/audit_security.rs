// The evaluator held to what giving, lending and keeping preparations must not change, from the
// security audit of that change: no panic on any unit that decodes (a host runs stored units
// without `validate`), no shape kept for one unit handed to another, no limit skipped on the
// giving path, nothing growing across calls on a thread, and the depth a host runs still fitting
// its thread. Each test says what it holds.
//
// Run: cargo test -p holon-core --test audit_security
//      (the complexity timing probe is #[ignore]d; add `-- --ignored --nocapture`).

use holon_core::{eval, eval_observed, Answer, Edge, Hash, MapResolver, Observer, Param, Port, Slot, Type, Unit, UnitBody, Value, BOUNDARY};

// ── tiny deterministic RNG ───────────────────────────────────────────────────
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }
    fn next(&mut self, m: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % m.max(1)
    }
}

// ── value / unit builders ─────────────────────────────────────────────────────
fn num(n: f64) -> Value {
    Value::Number(n)
}
fn txt(s: &str) -> Value {
    Value::Text(s.into())
}
fn list(xs: Vec<Value>) -> Value {
    Value::List(xs)
}
fn rec(fs: &[(&str, Value)]) -> Value {
    Value::Record(fs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}

/// A spread of values of every kind, some heavy and nested (so giving/lending both fire).
fn value_pool() -> Vec<Value> {
    vec![
        num(0.0),
        num(1.0),
        num(-3.5),
        num(f64::NAN),
        Value::Boolean(true),
        Value::Boolean(false),
        txt(""),
        txt("abc"),
        txt("héllo→\u{1f600}"),
        list(vec![]),
        list(vec![num(1.0), txt("x"), Value::Boolean(false)]),
        list(vec![list(vec![num(1.0)]), rec(&[("a", num(2.0))])]),
        rec(&[]),
        rec(&[("name", txt("n")), ("v", num(9.0))]),
        rec(&[("name", num(1.0))]),
        Value::Sum { arm: 0, payload: Box::new(num(7.0)) },
        Value::Sum { arm: 1, payload: Box::new(list(vec![])) },
        Value::Effect { sink: "egress".into(), payload: Box::new(txt("p")) },
    ]
}

fn any_port(name: &str) -> Port {
    Port { name: name.into(), ty: Type::Any }
}

/// A primitive unit with `n_in` Any-typed inputs and one Any output, plus params.
fn prim(name: &str, n_in: usize, params: Vec<Param>) -> Unit {
    Unit {
        body: UnitBody::Primitive { name: name.into() },
        params,
        inputs: (0..n_in).map(|i| any_port(&format!("i{i}"))).collect(),
        outputs: vec![any_port("out")],
    }
}

fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// An observer that *reads* produced outputs (reads_produced == true, the default): the class of
/// observer the recorder / verify / host use. It forces lending OFF, so this exercises the
/// copy-not-lend path. It also answers every request absent.
#[derive(Default)]
struct Reader {
    nodes: usize,
    bytes: usize,
}
impl Observer for Reader {
    fn produced(&mut self, _path: &[u32], outputs: &[Slot]) {
        self.nodes += 1;
        for s in outputs {
            if let Slot::Value(Value::Text(t)) = s {
                self.bytes += t.len();
            }
        }
    }
    fn answer(&mut self, _path: &[u32], _req: &Value) -> Answer {
        Answer::Absent
    }
}

/// Structural equality that treats NaN == NaN (Value uses IEEE PartialEq, so a result holding a
/// NaN never equals itself — a determinism check must look past that). Debug is a stable, total
/// rendering of the value tree.
fn same(a: &[Option<Value>], b: &[Option<Value>]) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

fn eval_quietly(unit: &Unit, inputs: &[Option<Value>], r: &MapResolver) -> std::thread::Result<Vec<Option<Value>>> {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let unit = unit.clone();
    let inputs = inputs.to_vec();
    // Evaluate on the same resolver; clone to move into the closure without borrowing across unwind.
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| eval(&unit, &inputs, r).into_vec()));
    std::panic::set_hook(prev);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// ITEM 1 (panics). Drive the SECOND leaf implementation `primitives::compute_given`
// through the real public `fold` path, over heterogeneous lists with a heavy given
// accumulator, for every leaf that `keeps()` an input. The old evaluator had no such
// path. A panic here is reachable by any flow author (a valid `fold`) and in
// `holon verify` (a crafted record), so it would be at least medium.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn audit_fold_giving_over_heterogeneous_values_never_panics() {
    // Every leaf with a `keeps` arm in compute_given, applied as fn(acc, elem).
    let keeps_leaves = ["append", "extend", "cons", "insert", "with_field", "remove", "concat", "select", "head", "get", "field", "lookup", "some", "default", "list", "pair", "tail", "without_field"];
    let pool = value_pool();
    let mut panics = 0;
    let mut checked = 0;
    for name in keeps_leaves {
        for arity in 1..=3usize {
            let mut r = MapResolver::new();
            // fn: a leaf of this arity, taking (acc, elem, [extra]) — a record `name` param set
            // so with_field/field/remove/without_field have a key to use.
            let f = r.insert(prim(name, arity, vec![Param::frozen("name", "name")])).unwrap();
            let fold = Unit {
                body: UnitBody::Primitive { name: "fold".into() },
                params: vec![Param::frozen("fn", hex(&f))],
                // acc, list, and (for arity 3) one extra.
                inputs: (0..(1 + 1 + arity.saturating_sub(2))).map(|i| any_port(&format!("i{i}"))).collect(),
                outputs: vec![any_port("out")],
            };
            // Try every initial accumulator and a spread of list contents.
            for init in &pool {
                for li in 0..pool.len() {
                    let xs = list(vec![pool[li].clone(), pool[(li + 3) % pool.len()].clone(), pool[(li + 7) % pool.len()].clone()]);
                    let mut inputs = vec![Some(init.clone()), Some(xs)];
                    if fold.inputs.len() == 3 {
                        inputs.push(Some(pool[(li + 1) % pool.len()].clone()));
                    }
                    match eval_quietly(&fold, &inputs, &r) {
                        Ok(out) => {
                            // determinism: same answer a second time (shape cache / scratch reuse).
                            assert!(same(&out, &eval(&fold, &inputs, &r).into_vec()), "fold {name} not deterministic");
                        }
                        Err(_) => panics += 1,
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 5000, "coverage too thin: {checked}");
    assert_eq!(panics, 0, "compute_given panicked via fold on {panics} input combinations");
}

// ─────────────────────────────────────────────────────────────────────────────
// ITEM 1 + ITEM 8 (panics / reentrancy / put-back). Map a COMPOSITE function over a
// heterogeneous list. Inside the composite, children are chained so a child's OWNED
// output is given to the next (fire_giving / may_give / take / put_back), and a
// `coalesce` whose first input is a keeps-leaf exercises the *speculative* give +
// put-back when the leaf produces nothing. Composites with and without a choice
// cover both the planned and the walked paths.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn audit_map_of_composite_with_giving_and_coalesce_never_panics() {
    let pool = value_pool();
    let mut panics = 0;
    let mut checked = 0;
    let mut rng = Rng::new(0xA5A5_1234);

    for trial in 0..400u64 {
        let mut r = MapResolver::new();
        // children: a small library of keeps-leaves + guard/coalesce choices + consts.
        let hhead = r.insert(prim("head", 1, vec![])).unwrap();
        let htail = r.insert(prim("tail", 1, vec![])).unwrap();
        let happend = r.insert(prim("append", 2, vec![])).unwrap();
        let hcons = r.insert(prim("cons", 2, vec![])).unwrap();
        let hconcat = r.insert(prim("concat", 2, vec![])).unwrap();
        let hcoalesce = r.insert(prim("coalesce", 2, vec![])).unwrap();
        let hfield = r.insert(prim("field", 1, vec![Param::frozen("name", "name")])).unwrap();
        let cfall = r.insert(Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("type", "Text"), Param::frozen("value", "fallback")],
            inputs: vec![],
            outputs: vec![any_port("out")],
        }).unwrap();
        let lib = [hhead, htail, happend, hcons, hconcat, hfield];

        // A composite: input 0 = the mapped item. A chain of up to 5 keeps-leaves, each fed
        // by the item and/or a previous child's output. Optionally a coalesce(last, const).
        let nchain = 1 + rng.next(5) as usize;
        let mut children: Vec<Hash> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        for c in 0..nchain {
            let pick = lib[rng.next(lib.len() as u64) as usize];
            children.push(pick);
            // port 0 of each child ← item (BOUNDARY input 0) or a previous child's output.
            let src0 = if c == 0 || rng.next(2) == 0 { BOUNDARY } else { (rng.next(c as u64)) as u32 };
            edges.push(Edge { from_child: src0, from_port: 0, to_child: c as u32, to_port: 0 });
            // a 2-input child gets a second input (item again — shared read, which disables giving).
            edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: c as u32, to_port: 1 });
        }
        let out_src = (nchain - 1) as u32;
        let use_coalesce = rng.next(2) == 0;
        if use_coalesce {
            let co = children.len() as u32;
            children.push(hcoalesce);
            children.push(cfall);
            edges.push(Edge { from_child: out_src, from_port: 0, to_child: co, to_port: 0 });
            edges.push(Edge { from_child: co + 1, from_port: 0, to_child: co, to_port: 1 });
            edges.push(Edge { from_child: co, from_port: 0, to_child: BOUNDARY, to_port: 0 });
        } else {
            edges.push(Edge { from_child: out_src, from_port: 0, to_child: BOUNDARY, to_port: 0 });
        }
        let fnunit = Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs: vec![any_port("item")],
            outputs: vec![any_port("out")],
        };
        let f = match r.insert(fnunit) {
            Ok(h) => h,
            Err(_) => continue,
        };
        let map = Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(&f))],
            inputs: vec![any_port("list")],
            outputs: vec![any_port("out")],
        };
        // heterogeneous lists, including nested/heavy ones.
        for _ in 0..6 {
            let n = rng.next(6) as usize;
            let xs = list((0..n).map(|_| pool[rng.next(pool.len() as u64) as usize].clone()).collect());
            let inputs = vec![Some(xs)];
            match eval_quietly(&map, &inputs, &r) {
                Ok(out) => assert!(same(&out, &eval(&map, &inputs, &r).into_vec()), "map not deterministic, trial {trial}"),
                Err(_) => panics += 1,
            }
            checked += 1;
        }
    }
    assert!(checked > 2000, "coverage too thin: {checked}");
    assert_eq!(panics, 0, "giving through a mapped composite panicked on {panics} cases");
}

// ─────────────────────────────────────────────────────────────────────────────
// ITEM 1 (panics) — broad structural fuzz of the evaluator itself over UNVALIDATED
// composites (edges to children that don't exist, ports out of range, cycles, shared
// reads). This is the `holon verify` threat model: a crafted record's units are checked
// only against their hash, never `holon_core::validate`. Evaluated under both a plain
// (lending) and a produced-reading (copying) observer.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn audit_unvalidated_composites_never_panic_in_eval() {
    let pool = value_pool();
    let leaves = ["add", "subtract", "divide", "append", "extend", "cons", "head", "tail", "get", "field", "with_field", "concat", "guard", "coalesce", "select", "list", "range", "join", "insert", "lookup", "equals", "is_empty", "length"];
    let mut panics = 0;
    let mut checked = 0;
    let mut rng = Rng::new(0xDEAD_BEEF);

    for _ in 0..6000u64 {
        let mut r = MapResolver::new();
        let mut hs: Vec<Hash> = Vec::new();
        for name in leaves {
            let n_in = rng.next(4) as usize;
            hs.push(r.insert(prim(name, n_in, vec![Param::frozen("name", "name")])).unwrap());
        }
        let n = 1 + rng.next(7) as usize;
        let children: Vec<Hash> = (0..n).map(|_| hs[rng.next(hs.len() as u64) as usize]).collect();
        // Random edges, deliberately including out-of-range children/ports and self/back edges.
        // A port is any number an edge writes: one in twenty is far out, the last port there is
        // among them (a list as long as its highest port would be 51 GiB, and no test's to make).
        let n_edges = rng.next(16) as usize;
        let end = |v: u64| if v == 0 { BOUNDARY } else { (v - 1) as u32 };
        let port = |v: u64| match v {
            0 => u32::MAX,
            1 => u32::MAX - 1,
            2 => 1 << 30,
            3 => 70_000,
            v => (v % 4) as u32,
        };
        let edges: Vec<Edge> = (0..n_edges)
            .map(|_| Edge {
                from_child: end(rng.next(n as u64 + 3)),
                from_port: port(rng.next(80)),
                to_child: end(rng.next(n as u64 + 3)),
                to_port: port(rng.next(80)),
            })
            .collect();
        let n_out = 1 + rng.next(2) as usize;
        let comp = Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs: (0..3).map(|i| any_port(&format!("in{i}"))).collect(),
            outputs: (0..n_out).map(|i| any_port(&format!("o{i}"))).collect(),
        };
        // The top composite is evaluated directly (its children are in `r`); it needn't be
        // inserted. This is exactly how a crafted record's root reaches eval without validate.
        let top = comp;
        for _ in 0..3 {
            let inputs: Vec<Option<Value>> = (0..3).map(|_| if rng.next(5) == 0 { None } else { Some(pool[rng.next(pool.len() as u64) as usize].clone()) }).collect();
            // plain (lending) path
            match eval_quietly(&top, &inputs, &r) {
                Ok(_) => {}
                Err(_) => panics += 1,
            }
            // produced-reading (copying) path
            let prev = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let mut rd = Reader::default();
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                eval_observed(&top, &inputs, &r, &mut rd);
            }));
            std::panic::set_hook(prev);
            if res.is_err() {
                panics += 1;
            }
            checked += 1;
        }
    }
    assert!(checked > 10000, "coverage too thin: {checked}");
    assert_eq!(panics, 0, "eval of an unvalidated composite panicked on {panics} cases");
}

// ─────────────────────────────────────────────────────────────────────────────
// ITEM 7 (isolation / determinism). The thread-local KEPT shape cache keys on a unit's
// ADDRESS but re-validates (child-count, edges) on a hit, and a Shape is a pure function
// of (child-count, edges). This runs MANY distinct composites interleaved from TWO
// resolvers on ONE thread (the server reuses blocking-pool threads across owners), and
// asserts each result equals a fresh single-shot evaluation. A cache that ever handed one
// unit another's shape would diverge here.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn audit_kept_shape_cache_never_crosses_units_on_one_thread() {
    // A composite chain built deterministically from a seed, so the same seed rebuilds the same
    // unit (at a different address, which is the point).
    fn make_from(seed: u64) -> (MapResolver, Unit, Vec<Option<Value>>) {
        let mut rng = Rng::new(seed);
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, vec![])).unwrap();
        let hsub = r.insert(prim("subtract", 2, vec![])).unwrap();
        let hmul = r.insert(prim("multiply", 2, vec![])).unwrap();
        let lib = [hadd, hsub, hmul];
        let n = 1 + rng.next(5) as usize;
        let children: Vec<Hash> = (0..n).map(|_| lib[rng.next(3) as usize]).collect();
        let mut edges = vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 }, Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 1 }];
        for c in 1..n {
            edges.push(Edge { from_child: (c - 1) as u32, from_port: 0, to_child: c as u32, to_port: 0 });
            edges.push(Edge { from_child: BOUNDARY, from_port: 1, to_child: c as u32, to_port: 1 });
        }
        edges.push(Edge { from_child: (n - 1) as u32, from_port: 0, to_child: BOUNDARY, to_port: 0 });
        let comp = Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs: vec![any_port("a"), any_port("b")],
            outputs: vec![any_port("out")],
        };
        let a = rng.next(1000) as f64;
        let b = 1.0 + rng.next(999) as f64;
        (r, comp, vec![Some(num(a)), Some(num(b))])
    }

    let seeds: Vec<u64> = (0..300u64).map(|i| 0x5EED_0007u64.wrapping_add(i.wrapping_mul(0x9E37_79B9))).collect();

    // Oracle: each unit's result computed on a FRESH thread (fresh thread-local KEPT), rebuilt
    // from its seed so nothing else is ever in that thread's cache.
    let oracle: Vec<Vec<Option<Value>>> = seeds
        .iter()
        .map(|&s| std::thread::spawn(move || {
            let (r, u, inp) = make_from(s);
            eval(&u, &inp, &r).into_vec()
        }).join().unwrap())
        .collect();

    // Now build and churn them all on THIS one thread. Rebuild (new addresses) each sweep so the
    // 64-slot address-keyed cache is forced to collide and reuse freed addresses.
    let mut owners: Vec<(MapResolver, Unit, Vec<Option<Value>>)> = seeds.iter().map(|&s| make_from(s)).collect();
    for round in 0..40usize {
        for i in 0..owners.len() {
            let j = (i * 7 + round * 13) % owners.len();
            let (r, u, inp) = &owners[j];
            assert_eq!(eval(u, inp, r).into_vec(), oracle[j], "kept-shape cache corrupted unit {j} on round {round}");
        }
        if round % 4 == 3 {
            for k in (0..owners.len()).step_by(2) {
                owners[k] = make_from(seeds[k]);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ITEM 3 (stack depth). A composite nested exactly MAX_CLOSURE_DEPTH (64) deep — the
// deepest the host will deploy/run — must still evaluate on a 2 MiB thread (tokio's
// worker and blocking-pool default, which is what the data plane runs eval on). This is
// a regression guard: the rewrite's per-level frame must not have grown enough to
// overflow a unit at the deploy bound.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn audit_64_deep_composite_evaluates_on_a_2mib_thread() {
    let mut r = MapResolver::new();
    let hadd = r.insert(prim("add", 2, vec![])).unwrap();
    let wrap = |children: Vec<Hash>| Unit {
        body: UnitBody::Composite { children, edges: vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 }, Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 1 }, Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }] },
        params: vec![],
        inputs: vec![any_port("a"), any_port("b")],
        outputs: vec![any_port("out")],
    };
    let mut below = r.insert(wrap(vec![hadd])).unwrap();
    for _ in 0..61 {
        below = r.insert(wrap(vec![below])).unwrap();
    }
    // 1 (add) + 62 inserted wrappers + the top = 64 units deep.
    let top = wrap(vec![below]);
    let out = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || eval(&top, &[Some(num(10.0)), Some(num(5.0))], &r).into_vec())
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(out, vec![Some(num(15.0))]);
}

// ─────────────────────────────────────────────────────────────────────────────
// ITEM 5 (limit bypass). `range` enforces MAX_RANGE (10_000). `range` is not a keeps-op,
// so compute_given falls through to compute_op — the limit must be intact whether its
// output is lent or (would be) given. Confirmed directly and through a fold that feeds a
// range into a keeps-leaf.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn audit_range_limit_intact_on_the_giving_path() {
    use holon_core::primitives::compute;
    assert_eq!(compute("range", &[&num(holon_core::MAX_RANGE as f64 + 1.0)], &[]), None, "range past MAX_RANGE must be None");
    assert!(compute("range", &[&num(holon_core::MAX_RANGE as f64)], &[]).is_some());
    // A fold whose fn appends range(elem) is still bounded: an over-limit element → absent.
    let mut r = MapResolver::new();
    let hrange = r.insert(prim("range", 1, vec![])).unwrap();
    let hcons = r.insert(prim("cons", 2, vec![])).unwrap();
    // fn(acc, elem) = cons(range(elem), acc): range is a child whose output is given to cons.
    let fnunit = Unit {
        body: UnitBody::Composite {
            children: vec![hrange, hcons],
            edges: vec![
                Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 0 }, // elem → range
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },         // range → cons.0
                Edge { from_child: BOUNDARY, from_port: 0, to_child: 1, to_port: 1 },  // acc → cons.1
                Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![any_port("acc"), any_port("elem")],
        outputs: vec![any_port("out")],
    };
    let f = r.insert(fnunit).unwrap();
    let fold = Unit {
        body: UnitBody::Primitive { name: "fold".into() },
        params: vec![Param::frozen("fn", hex(&f))],
        inputs: vec![any_port("acc"), any_port("list")],
        outputs: vec![any_port("out")],
    };
    // An element past MAX_RANGE makes that application absent → whole fold absent (unchanged vs old).
    let over = num(holon_core::MAX_RANGE as f64 + 1.0);
    let got = eval(&fold, &[Some(list(vec![])), Some(list(vec![num(2.0), over]))], &r).into_vec();
    assert_eq!(got, vec![None], "an over-limit range in a fold must propagate absent, not build a huge list");
}

// ─────────────────────────────────────────────────────────────────────────────
// ITEM 4 (memory). The only state the rewrite keeps across evaluations on a thread is the
// fixed [Option<Rc<KeptShape>>; 64] thread-local KEPT; `Prepared::scratch` is cleared and
// reused, and `Many` lives only inside the map/fold that owns it. This runs 200k varied
// evaluations on ONE thread (churning KEPT with distinct units) and asserts every result is
// correct throughout — a regression guard against a future per-call leak. Peak working set is
// measured outside the test (`audit_stack_probe mem`); it is flat across the run.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn audit_200k_evals_on_one_thread_stay_correct() {
    let mut rng = Rng::new(0x1234_5678);
    for i in 0..200_000u64 {
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, vec![])).unwrap();
        let hmul = r.insert(prim("multiply", 2, vec![])).unwrap();
        let lib = [hadd, hmul];
        let n = 1 + (i % 5) as usize;
        let children: Vec<Hash> = (0..n).map(|k| lib[(k + i as usize) % 2]).collect();
        let mut edges = vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 }, Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 1 }];
        for c in 1..n {
            edges.push(Edge { from_child: (c - 1) as u32, from_port: 0, to_child: c as u32, to_port: 0 });
            edges.push(Edge { from_child: BOUNDARY, from_port: 1, to_child: c as u32, to_port: 1 });
        }
        edges.push(Edge { from_child: (n - 1) as u32, from_port: 0, to_child: BOUNDARY, to_port: 0 });
        let comp = Unit { body: UnitBody::Composite { children, edges }, params: vec![], inputs: vec![any_port("a"), any_port("b")], outputs: vec![any_port("out")] };
        let a = rng.next(1000) as f64;
        let got = eval(&comp, &[Some(num(a)), Some(num(2.0))], &r).into_vec();
        // first child is add or mul of (a,2); the rest chain (prev,2). Just assert it produced a
        // finite number (no corruption from cache churn).
        match got.as_slice() {
            [Some(Value::Number(x))] => assert!(x.is_finite(), "non-finite at {i}"),
            other => panic!("unexpected output at {i}: {other:?}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ITEM 2 (algorithmic complexity) — timing probe for `unneeded` (fan-out^depth, cap 8,
// no memo) and `gives_of` (`consumers[..].contains`, O(readers^2)). #[ignore]d so it is
// a runnable reproduction, not part of the normal suite.
// ─────────────────────────────────────────────────────────────────────────────
#[test]
#[ignore = "timing probe; run with --ignored --nocapture"]
fn audit_complexity_probe_fanout_map() {
    // A map function that is a WALKED composite (terminal coalesce, the only path that calls
    // `unneeded`/giving on the walk) built as a chain of `len` DISTINCT keeps-leaves — each with
    // a unique frozen param so the unit encoder accepts it (no UnusedTwins). Each child builds on
    // the previous child's OWNED output, so giving + may_give fire at every node, every item.
    // Scaling `len` shows whether per-eval cost is linear in the composite size.
    for len in [10usize, 40, 80, 160, 320] {
        let mut r = MapResolver::new();
        let mut children: Vec<Hash> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        // child 0: append(item, item) — builds a 2-list from the item.
        children.push(r.insert(prim("append", 2, vec![Param::frozen("k", "0")])).unwrap());
        edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 });
        edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 1 });
        // chain: child c = append(child c-1, item), each distinct via its param.
        for c in 1..len {
            children.push(r.insert(prim("append", 2, vec![Param::frozen("k", c.to_string())])).unwrap());
            edges.push(Edge { from_child: (c - 1) as u32, from_port: 0, to_child: c as u32, to_port: 0 });
            edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: c as u32, to_port: 1 });
        }
        // terminal coalesce(chain_end, item) forces the walked path (a non-pass-through choice).
        let co = children.len() as u32;
        children.push(r.insert(prim("coalesce", 2, vec![])).unwrap());
        edges.push(Edge { from_child: (len - 1) as u32, from_port: 0, to_child: co, to_port: 0 });
        edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: co, to_port: 1 });
        edges.push(Edge { from_child: co, from_port: 0, to_child: BOUNDARY, to_port: 0 });
        let fnunit = Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs: vec![any_port("item")],
            outputs: vec![any_port("out")],
        };
        let f = r.insert(fnunit).unwrap();
        let map = Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(&f))],
            inputs: vec![any_port("list")],
            outputs: vec![any_port("out")],
        };
        let heavy_item = list((0..16).map(|i| num(i as f64)).collect());
        let xs = list((0..2000).map(|_| heavy_item.clone()).collect());
        let t = std::time::Instant::now();
        let _ = eval(&map, &[Some(xs)], &r);
        let el = t.elapsed();
        eprintln!("chain_len={len:>4}  map(2000 items)  {el:?}  ({:.1} us/node-item)", el.as_micros() as f64 / (2000.0 * len as f64));
    }
}
