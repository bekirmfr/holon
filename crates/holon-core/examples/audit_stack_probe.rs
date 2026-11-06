// AUDIT: stack-depth probe for the evaluator rewrite (item 3). Builds a composite nested `depth`
// units deep (each a one-child wrapper around an `add`) and evaluates it on a thread with a given
// stack size. Prints "OK" on success; a stack overflow aborts the process (Rust can't unwind it),
// so the caller runs one process per depth and treats a non-"OK" exit as overflow.
//
//   cargo run -p holon-core --release --example audit_stack_probe -- <depth> <stack_mib>
//
// Compares two evaluators by swapping eval.rs / primitives.rs between runs: the deepest depth
// that prints "OK" and the first that overflows, per stack size, with `map` and without.

use holon_core::{eval, Edge, Hash, MapResolver, Param, Port, Type, Unit, UnitBody, Value, BOUNDARY};

fn any_port(name: &str) -> Port {
    Port { name: name.into(), ty: Type::Any }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // `mem <iters>`: run many varied evaluations on ONE thread (item 4 — cross-call state growth).
    if args.get(1).map(|s| s == "mem").unwrap_or(false) {
        mem_probe(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(200_000));
        return;
    }
    let depth: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(64);
    let stack_mib: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2);

    // Also exercise map-in-map nesting when arg 3 is "map".
    let map_nesting = args.get(3).map(|s| s == "map").unwrap_or(false);

    let mut r = MapResolver::new();
    let top: Unit = if map_nesting {
        build_map_in_map(&mut r, depth)
    } else {
        build_nested_composite(&mut r, depth)
    };

    let stack = stack_mib * 1024 * 1024;
    let handle = std::thread::Builder::new().stack_size(stack).spawn(move || {
        let inputs = if map_nesting {
            // A list nested `depth` deep ([[[…1.0…]]]), so every map level finds a list to iterate
            // and the map-in-map recursion actually reaches `depth`.
            let mut v = Value::Number(1.0);
            for _ in 0..depth {
                v = Value::List(vec![v]);
            }
            vec![Some(v)]
        } else {
            vec![Some(Value::Number(10.0)), Some(Value::Number(5.0))]
        };
        eval(&top, &inputs, &r).into_vec()
    }).unwrap();
    let out = handle.join().unwrap();
    println!("OK depth={depth} stack_mib={stack_mib} map={map_nesting} out={out:?}");
}

/// Run `iters` evaluations cycling through many DISTINCT small units (each builds its own resolver,
/// so addresses churn and the 64-slot thread-local KEPT cache is exercised). The only cross-call
/// state the rewrite adds is that fixed-size KEPT array; if anything grew per call, working set
/// would climb across the run. The caller reads PeakWorkingSet externally.
fn mem_probe(iters: usize) {
    use holon_core::Param;
    let mut sink = 0u64;
    for i in 0..iters {
        let mut r = MapResolver::new();
        // A map over a small list whose fn is a composite chain of a few keeps-leaves (giving on),
        // its shape varied by `i` so KEPT sees many distinct units.
        let len = 1 + (i % 6);
        let mut children: Vec<Hash> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        children.push(r.insert(Unit { body: UnitBody::Primitive { name: "append".into() }, params: vec![Param::frozen("k", "0")], inputs: vec![any_port("a"), any_port("b")], outputs: vec![any_port("out")] }).unwrap());
        edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 });
        edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 1 });
        for c in 1..len {
            children.push(r.insert(Unit { body: UnitBody::Primitive { name: "append".into() }, params: vec![Param::frozen("k", format!("{i}-{c}"))], inputs: vec![any_port("a"), any_port("b")], outputs: vec![any_port("out")] }).unwrap());
            edges.push(Edge { from_child: (c - 1) as u32, from_port: 0, to_child: c as u32, to_port: 0 });
            edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: c as u32, to_port: 1 });
        }
        edges.push(Edge { from_child: (len - 1) as u32, from_port: 0, to_child: BOUNDARY, to_port: 0 });
        let fnu = Unit { body: UnitBody::Composite { children, edges }, params: vec![], inputs: vec![any_port("item")], outputs: vec![any_port("out")] };
        let f = r.insert(fnu).unwrap();
        let hexs: String = f.iter().map(|b| format!("{b:02x}")).collect();
        let map = Unit { body: UnitBody::Primitive { name: "map".into() }, params: vec![Param::frozen("fn", hexs)], inputs: vec![any_port("list")], outputs: vec![any_port("out")] };
        let out = eval(&map, &[Some(Value::List(vec![Value::Number(i as f64), Value::Number(1.0)]))], &r).into_vec();
        sink = sink.wrapping_add(out.len() as u64);
    }
    println!("mem_probe iters done, sink={sink}");
}

/// A composite nested `depth` units deep: `add` at the bottom, `depth-1` one-child wrappers above.
fn build_nested_composite(r: &mut MapResolver, depth: usize) -> Unit {
    let hadd = r.insert(Unit {
        body: UnitBody::Primitive { name: "add".into() },
        params: vec![],
        inputs: vec![any_port("a"), any_port("b")],
        outputs: vec![any_port("out")],
    }).unwrap();
    let wrap = |children: Vec<Hash>| Unit {
        body: UnitBody::Composite {
            children,
            edges: vec![
                Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                Edge { from_child: BOUNDARY, from_port: 1, to_child: 0, to_port: 1 },
                Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![any_port("a"), any_port("b")],
        outputs: vec![any_port("out")],
    };
    // depth counts units: 1 add + (depth-1) wrappers.
    let mut below = r.insert(wrap(vec![hadd])).unwrap();
    for _ in 0..depth.saturating_sub(2) {
        below = r.insert(wrap(vec![below])).unwrap();
    }
    wrap(vec![below])
}

/// `depth` levels of map-in-map: each level's function maps the next level's function over its
/// item (treated as a list). The innermost just returns its item via a const-less identity.
fn build_map_in_map(r: &mut MapResolver, depth: usize) -> Unit {
    // innermost function: identity composite (item → output).
    let hid = r.insert(Unit {
        body: UnitBody::Composite { children: vec![], edges: vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: BOUNDARY, to_port: 0 }] },
        params: vec![],
        inputs: vec![any_port("item")],
        outputs: vec![any_port("out")],
    }).unwrap();
    let hex = |h: &Hash| h.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut fnhash = hid;
    for _ in 0..depth {
        // a map unit applying the previous function.
        let map = r.insert(Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(&fnhash))],
            inputs: vec![any_port("list")],
            outputs: vec![any_port("out")],
        }).unwrap();
        // wrap the map in a composite so it can be the fn of the next map.
        fnhash = r.insert(Unit {
            body: UnitBody::Composite { children: vec![map], edges: vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 }, Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }] },
            params: vec![],
            inputs: vec![any_port("item")],
            outputs: vec![any_port("out")],
        }).unwrap();
    }
    Unit {
        body: UnitBody::Primitive { name: "map".into() },
        params: vec![Param::frozen("fn", hex(&fnhash))],
        inputs: vec![any_port("list")],
        outputs: vec![any_port("out")],
    }
}
