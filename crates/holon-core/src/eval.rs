//! Eager graph evaluator (P1.4.3) — the leaf that closes "identity but not evaluation".
//!
//! `eval(unit, inputs, resolver) -> EvalOutputs` (`SmallVec<[Option<Value>; 1]>`, inline for the
//! common 0-1-output case — the SmallVec perf inch, 2026-07-02). A primitive dispatches to its
//! native `compute`; a composite resolves its inner DAG **by reference** (I3) in a
//! single topological pass (acyclicity from P1.3.1 means one pass reaches the fixed
//! point), injecting promoted inputs through `BOUNDARY→child` edges and reading each
//! exposed output through its own `child→BOUNDARY` edge (P4.7: a composite may expose
//! several; a primitive always exposes exactly one — multi-output is composite-only). A
//! primitive and a composite are evaluated through the **identical** entry point (I1). Each
//! output slot independently `None`s ("not computable") and **propagates** — one failed slot
//! never blanks out the others.
//!
//! ## Firing rule (the pure subset)
//! The evaluator gathers each node's *present* inputs (in port order) and lets the
//! primitive's `compute` decide — its arity/type checks realize `required/optional`
//! for the current vocabulary. Port memory (`persist`) and firing rounds (`consume`)
//! stay out of the pure core (state boundary / async, P2.4.2). An explicit per-port
//! `required/optional/default` field is deferred until a primitive needs a positional
//! optional with a default — none does yet.
//!
//! ## Higher-order `map`
//! `compute` stays purely first-order. `map` is higher-order — it resolves a referenced
//! unit and applies it per element — so it is handled **here**, where recursion already
//! lives, rather than via a callback into a first-order `compute`. Its `fn` argument is a
//! param holding the hex hash of the unit to apply.

use crate::{primitives, Edge, Hash, Param, Resolver, Unit, UnitBody, Value, BOUNDARY, VARIADIC};
use crate::hash::parse_hash;
use smallvec::{smallvec, SmallVec};
use std::collections::{BTreeMap, BTreeSet};

/// The eager evaluator's own output shape (P4.7 + the SmallVec perf inch, 2026-07-02).
/// Inline storage for 0 or 1 outputs — every genesis primitive declares exactly one, forever,
/// and a single-output composite is still the overwhelming majority even now that multi-output
/// is a real, everyday editor capability — so the common case never touches the heap. Spills to
/// the heap only for a genuine multi-output composite, behaving exactly as a plain `Vec` would.
pub type EvalOutputs = SmallVec<[Option<Value>; 1]>;

/// Evaluate a unit against values for its input ports (index = input-port index;
/// `None` = absent). Returns one slot per the unit's declared output — `unit.outputs.len()`
/// entries, always, in order; each slot independently `Some`/`None` (P4.7: multi-output is a
/// composite-only capability — every primitive declares exactly one output, so its arm always
/// returns a 1-element vec; only a composite's own boundary can expose several).
pub fn eval(unit: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver) -> EvalOutputs {
    match &unit.body {
        // `map` is higher-order — it invokes the evaluator — so it lives here, where
        // recursion lives, rather than in first-order `compute`.
        UnitBody::Primitive { name } if name == "map" => smallvec![eval_map(inputs, &unit.params, resolver)],
        // `fold` is the one iteration primitive (the universal list donor): also higher-order,
        // so it lives here too. It threads an accumulator, which `map` does not.
        UnitBody::Primitive { name } if name == "fold" => smallvec![eval_fold(inputs, &unit.params, resolver)],
        UnitBody::Primitive { name } => {
            // Gather present values in port order **by reference** — `compute` borrows them,
            // so an O(1) reader (head/length/is_empty) copies only what it returns, not the
            // whole input list. compute's arity check is still the firing rule.
            //
            // That rule alone fails *open* for a variadic leaf: dropping an absent input leaves
            // a shorter list it still accepts, so `and(authenticated, <absent>)` would yield
            // `authenticated` and `list(a, <absent>, c)` would yield `[a, c]`. A variadic leaf
            // therefore fires only when every declared input is present, exactly as a
            // fixed-arity leaf already does.
            if VARIADIC.contains(&name.as_str()) && inputs.iter().any(Option::is_none) {
                return smallvec![None];
            }
            let present: Vec<&Value> = inputs.iter().flatten().collect();
            smallvec![primitives::compute(name, &present, &unit.params)]
        }
        UnitBody::Composite { children, edges } => {
            eval_composite(children, edges, unit.outputs.len(), inputs, resolver)
        }
    }
}

/// Convenience for the (still overwhelming majority of) callers that only ever deal with a
/// single-output unit: the one output, or `None` if the unit doesn't declare *exactly* one —
/// zero outputs, or two-or-more, both fail closed here rather than silently taking the first of
/// several. This is what `map`/`fold` use for their `fn` argument below: a multi-output
/// combinator isn't yet a meaningful thing (would it build a list of tuples? a tuple of lists?
/// — a real, separate design question, deferred per the P4.7 L4 §10), so refusing it here is
/// honest, not a workaround.
pub fn eval_one(unit: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver) -> Option<Value> {
    let mut outs = eval(unit, inputs, resolver);
    if outs.len() == 1 {
        outs.pop().unwrap()
    } else {
        None
    }
}

fn eval_composite(
    children: &[Hash],
    edges: &[Edge],
    n_outputs: usize,
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
) -> EvalOutputs {
    let n = children.len();

    // Resolve children (by hash). Any unresolved ⇒ nothing evaluable — every declared output
    // slot is None (the vec's *length* is still the declared contract; only the values are
    // absent, mirroring how `inputs` already has a fixed length with individually-absent slots).
    let mut resolved: Vec<&Unit> = Vec::with_capacity(n);
    for h in children {
        match resolver.resolve(h) {
            Some(u) => resolved.push(u),
            None => return smallvec![None; n_outputs],
        }
    }

    // Index incoming edges: destination (to_child, to_port) → source (from_child, from_port).
    // Validation guarantees ≤1 source per destination, so this never loses an edge.
    let mut incoming: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for e in edges {
        incoming.insert((e.to_child, e.to_port), (e.from_child, e.from_port));
    }

    // One topological pass (deterministic; ties broken by ascending child index).
    let Some(order) = topo_order(n, edges) else {
        return smallvec![None; n_outputs]; // a cycle — impossible after P1.3.1, handled not panicked
    };

    // Per-evaluation scratch, indexed by child, holding EACH child's own output vector (a child
    // may itself be a multi-output composite; a primitive child's vector is always length 1).
    // Fresh each call ⇒ nested and repeated composites never collide. Inner type matches
    // `eval`'s own `EvalOutputs` exactly — `scratch[ci] = eval(child, ...)` assigns a recursive
    // call's result directly, so this must stay in lockstep with `eval`'s return type or the
    // SmallVec perf inch's whole point (no allocation through nested composite evaluation, not
    // just the outermost call) would silently stop applying past the first level.
    let mut scratch: Vec<EvalOutputs> = vec![SmallVec::new(); n];

    for ci in order {
        let child = resolved[ci];
        let k = child.inputs.len();
        let mut gathered: Vec<Option<Value>> = Vec::with_capacity(k);
        for p in 0..k {
            let val = match incoming.get(&(ci as u32, p as u32)) {
                // promoted composite input arriving through the boundary
                Some(&(BOUNDARY, fp)) => inputs.get(fp as usize).cloned().flatten(),
                // another child's (already-evaluated) output, at the specific port wired —
                // `fp` is now actually used, not discarded: a child may have more than one.
                // `get`, not indexing: an edge naming a child that does not exist (bytes that
                // never went through `validate`, e.g. straight from a peer) reads as absent.
                Some(&(fc, fp)) => output_of(&scratch, fc, fp),
                // unconnected port
                None => None,
            };
            gathered.push(val);
        }
        scratch[ci] = eval(child, &gathered, resolver);
    }

    // Each exposed output is whatever feeds that boundary-output port: a child's output at the
    // right port, or — symmetric with the child-input case above — a promoted input routed
    // straight through. (A composite whose output *is* one of its inputs is a valid,
    // validate-accepted shape; the BOUNDARY→BOUNDARY arm below is why that doesn't panic.)
    (0..n_outputs as u32)
        .map(|op| match incoming.get(&(BOUNDARY, op)) {
            Some(&(BOUNDARY, fp)) => inputs.get(fp as usize).cloned().flatten(),
            Some(&(fc, fp)) => output_of(&scratch, fc, fp),
            None => None,
        })
        .collect()
}

/// Child `fc`'s output at port `fp`, or `None` when either index is out of range.
fn output_of(scratch: &[EvalOutputs], fc: u32, fp: u32) -> Option<Value> {
    scratch.get(fc as usize)?.get(fp as usize).cloned().flatten()
}

/// Kahn's algorithm over in-range child→child edges, emitting a deterministic order
/// (ascending child index among ready nodes). Boundary edges are excluded (an endpoint
/// `== BOUNDARY` is not `< n`). Returns `None` on a cycle — impossible after P1.3.1, but
/// handled rather than panicking.
fn topo_order(n: usize, edges: &[Edge]) -> Option<Vec<usize>> {
    let mut indeg = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        let (f, t) = (e.from_child as usize, e.to_child as usize);
        if f < n && t < n {
            adj[f].push(t);
            indeg[t] += 1;
        }
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(&node) = ready.iter().next() {
        ready.remove(&node);
        order.push(node);
        for &m in &adj[node] {
            indeg[m] -= 1;
            if indeg[m] == 0 {
                ready.insert(m);
            }
        }
    }
    if order.len() == n {
        Some(order)
    } else {
        None
    }
}

/// Higher-order `map`: input 0 is a list; the `fn` param holds the hex hash of the unit
/// to apply to each element. Yields a list of results, or `None` if the list/param is
/// malformed or any application is `None` (None-propagation). The `fn` reference rides a
/// param, so `map`'s hash transitively depends on `fn`'s hash (I6 holds via the preimage);
/// children-based *inspection* (P3.2.1) must additionally treat hash-valued params as
/// references.
fn eval_map(inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver) -> Option<Value> {
    let list = match inputs.first() {
        Some(Some(Value::List(xs))) => xs,
        _ => return None,
    };
    let fn_hex = params.iter().find(|p| p.name() == "fn").and_then(|p| p.value())?;
    let fn_hash = parse_hash(fn_hex)?;
    let unit = resolver.resolve(&fn_hash)?;

    let mut out = Vec::with_capacity(list.len());
    for elem in list {
        out.push(eval_one(unit, &[Some(elem.clone())], resolver)?); // any None ⇒ whole map None
    }
    Some(Value::List(out))
}

/// Higher-order `fold` — the **one** iteration primitive, and the universal list donor
/// (`sum`/`map`/`filter`/`length`/… are all composites over it). Input 0 is the initial
/// accumulator, input 1 is a list; the `fn` param holds the hex hash of the combining unit,
/// applied left-to-right as `fn(acc, elem)`. Yields the final accumulator; `fold(init, [])`
/// = `init`. Iteration is bounded by the list length — there is no unbounded looping
/// construct anywhere in the vocabulary, so **every composite is total** (the property the
/// trustless cache leans on). Any application returning `None` propagates. Like `map`, the
/// `fn` reference rides a param, so `fold`'s hash transitively depends on it (I6), and
/// inspection (P3.2.1) must treat hash-valued params as references.
fn eval_fold(inputs: &[Option<Value>], params: &[Param], resolver: &impl Resolver) -> Option<Value> {
    let init = match inputs.first() {
        Some(Some(v)) => v.clone(),
        _ => return None,
    };
    let list = match inputs.get(1) {
        Some(Some(Value::List(xs))) => xs,
        _ => return None,
    };
    let fn_hex = params.iter().find(|p| p.name() == "fn").and_then(|p| p.value())?;
    let fn_hash = parse_hash(fn_hex)?;
    let unit = resolver.resolve(&fn_hash)?;

    let mut acc = init;
    for elem in list {
        // fn(acc, elem) → new acc; any None ⇒ the whole fold is None.
        acc = eval_one(unit, &[Some(acc), Some(elem.clone())], resolver)?;
    }
    Some(acc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MapResolver, Port, Type};

    fn num(n: f64) -> Value { Value::Number(n) }
    fn nport(name: &str) -> Port { Port { name: name.into(), ty: Type::Number } }
    fn edge(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }
    /// A primitive with `n_in` input ports (typed `Any`, irrelevant at eval) and an output.
    fn prim(name: &str, n_in: usize, out: Type) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: (0..n_in).map(|i| Port { name: format!("i{i}"), ty: Type::Any }).collect(),
            outputs: vec![Port { name: "out".into(), ty: out }],
        }
    }
    fn composite(inputs: Vec<Port>, out: Type, children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs,
            outputs: vec![Port { name: "out".into(), ty: out }],
        }
    }
    /// A composite exposing several outputs — `composite()`'s multi-output sibling, named
    /// `out0`, `out1`, … in declaration order.
    fn multi_composite(inputs: Vec<Port>, outs: Vec<Type>, children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs,
            outputs: outs
                .into_iter()
                .enumerate()
                .map(|(i, ty)| Port { name: format!("out{i}"), ty })
                .collect(),
        }
    }
    /// input a → add.0 ; input b → add.1 ; add.out → output. Returns the composite.
    fn add2(r: &mut MapResolver) -> Unit {
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        composite(
            vec![nport("a"), nport("b")],
            Type::Number,
            vec![hadd],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)],
        )
    }

    #[test]
    fn eval_primitive_add() {
        let r = MapResolver::new();
        let add = prim("add", 3, Type::Number);
        assert_eq!(
            eval(&add, &[Some(num(3.0)), Some(num(7.0)), Some(num(5.0))], &r).into_vec(),
            vec![Some(num(15.0))]
        );
    }

    /// Regression: absent inputs used to be dropped before `compute`, so a variadic leaf fired
    /// on whatever was left — `and(true, <absent>)` was `true`, an auth check failing open.
    #[test]
    fn variadic_leaf_with_an_absent_input_fails_closed() {
        let r = MapResolver::new();
        let t = Value::Boolean(true);
        let x = Value::Text("x".into());
        for (name, present) in [
            ("and", t.clone()),
            ("or", Value::Boolean(false)),
            ("add", num(1.0)),
            ("multiply", num(2.0)),
            ("concat", x.clone()),
            ("list", x.clone()),
        ] {
            let leaf = prim(name, 2, Type::Any);
            assert_eq!(eval(&leaf, &[Some(present.clone()), None], &r).into_vec(), vec![None], "{name}(v, <absent>)");
            assert_eq!(eval(&leaf, &[None, Some(present)], &r).into_vec(), vec![None], "{name}(<absent>, v)");
            assert_eq!(eval(&leaf, &[None, None], &r).into_vec(), vec![None], "{name}(<absent>, <absent>)");
        }
        // Inside a composite, an unwired declared port is absent too.
        let mut r = MapResolver::new();
        let hand = r.insert(prim("and", 2, Type::Boolean)).unwrap();
        let c = composite(vec![], Type::Boolean, vec![hand], vec![edge(0, 0, BOUNDARY, 0)]);
        assert_eq!(eval(&c, &[], &r).into_vec(), vec![None]);
        // All present still fires, and a zero-input variadic leaf has nothing absent.
        assert_eq!(eval(&prim("and", 2, Type::Boolean), &[Some(t.clone()), Some(t.clone())], &r).into_vec(), vec![Some(t)]);
        assert_eq!(eval(&prim("list", 0, Type::Any), &[], &r).into_vec(), vec![Some(Value::List(vec![]))]);
    }

    /// Regression: an edge whose `from_child` names no child indexed `scratch` out of bounds.
    /// `validate` rejects such a unit, but `eval` is reached without it (the wasm bridge's
    /// `Evaluator::eval`, `verify_entry` on a peer's cache entry), so it must not panic.
    #[test]
    fn edge_from_a_missing_child_reads_as_absent_not_a_panic() {
        let mut r = MapResolver::new();
        let hnot = r.insert(prim("not", 1, Type::Boolean)).unwrap();
        let c = composite(
            vec![],
            Type::Any,
            vec![hnot],
            vec![edge(5, 0, 0, 0), edge(0, 0, BOUNDARY, 0)], // child 5 does not exist
        );
        assert_eq!(eval(&c, &[], &r).into_vec(), vec![None]);
        // An exposed output fed straight from a missing child, and from a missing port.
        let c = composite(vec![], Type::Any, vec![hnot], vec![edge(9, 0, BOUNDARY, 0)]);
        assert_eq!(eval(&c, &[], &r).into_vec(), vec![None]);
        let c = composite(vec![], Type::Any, vec![hnot], vec![edge(0, 3, BOUNDARY, 0)]);
        assert_eq!(eval(&c, &[], &r).into_vec(), vec![None]);
    }

    #[test]
    fn eval_composite_add() {
        let mut r = MapResolver::new();
        let c = add2(&mut r);
        assert_eq!(eval(&c, &[Some(num(3.0)), Some(num(4.0))], &r).into_vec(), vec![Some(num(7.0))]);
    }

    #[test]
    fn promoted_input_routes_straight_to_output() {
        // A composite whose output *is* its input (an identity passthrough): no
        // children, a single BOUNDARY→BOUNDARY edge. Valid per validate; eval must
        // return the input rather than index the scratch by BOUNDARY and panic.
        let r = MapResolver::new();
        let c = composite(vec![nport("x")], Type::Number, vec![], vec![edge(BOUNDARY, 0, BOUNDARY, 0)]);
        assert_eq!(eval(&c, &[Some(num(42.0))], &r).into_vec(), vec![Some(num(42.0))]);
    }

    #[test]
    fn eval_composite_sum() {
        // The v3 seed (minus Map): list(a,b,c) → sum → 15.
        let mut r = MapResolver::new();
        let hlist = r.insert(prim("list", 3, Type::list_any())).unwrap();
        let hsum = r.insert(prim("sum", 1, Type::Number)).unwrap();
        let c = composite(
            vec![nport("a"), nport("b"), nport("c")],
            Type::Number,
            vec![hlist, hsum],
            vec![
                edge(BOUNDARY, 0, 0, 0),
                edge(BOUNDARY, 1, 0, 1),
                edge(BOUNDARY, 2, 0, 2),
                edge(0, 0, 1, 0),       // list → sum
                edge(1, 0, BOUNDARY, 0), // sum → output
            ],
        );
        assert_eq!(
            eval(&c, &[Some(num(3.0)), Some(num(7.0)), Some(num(5.0))], &r).into_vec(),
            vec![Some(num(15.0))]
        );
    }

    #[test]
    fn eval_nested_composite() {
        let mut r = MapResolver::new();
        let inner = add2(&mut r);
        let hinner = r.insert(inner).unwrap();
        let outer = composite(
            vec![nport("x"), nport("y")],
            Type::Number,
            vec![hinner],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)],
        );
        assert_eq!(eval(&outer, &[Some(num(10.0)), Some(num(5.0))], &r).into_vec(), vec![Some(num(15.0))]);
    }

    #[test]
    fn none_propagates() {
        // subtract is binary; one operand absent ⇒ compute None ⇒ output None.
        let mut r = MapResolver::new();
        let hsub = r.insert(prim("subtract", 2, Type::Number)).unwrap();
        let c = composite(
            vec![nport("a"), nport("b")],
            Type::Number,
            vec![hsub],
            vec![edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), edge(0, 0, BOUNDARY, 0)],
        );
        assert_eq!(eval(&c, &[Some(num(3.0)), None], &r).into_vec(), vec![None]);
    }

    // hex-encode a hash for a `map` fn param (the inverse of hash::parse_hash).
    fn hex(h: &Hash) -> String {
        h.iter().map(|b| format!("{:02x}", b)).collect()
    }
    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn list(xs: Vec<Value>) -> Value {
        Value::List(xs)
    }
    // Double(x) = x * 2, as a composite: BOUNDARY.x → mul.0 ; const(2) → mul.1 ; mul → out.
    fn double(r: &mut MapResolver) -> Hash {
        let two = Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("type", "Number"), Param::frozen("value", "2")],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        };
        let htwo = r.insert(two).unwrap();
        let hmul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let unit = composite(
            vec![nport("x")],
            Type::Number,
            vec![hmul, htwo], // child0 = mul, child1 = const 2
            vec![edge(BOUNDARY, 0, 0, 0), edge(1, 0, 0, 1), edge(0, 0, BOUNDARY, 0)],
        );
        r.insert(unit).unwrap()
    }
    fn map_unit(fn_hash: &Hash) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(fn_hash))],
            inputs: vec![Port { name: "list".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        }
    }

    #[test]
    fn map_doubles_list() {
        // The v3 seed: list(3,7,5) → Map(Double) → [6, 14, 10].
        let mut r = MapResolver::new();
        let hdouble = double(&mut r);
        let m = map_unit(&hdouble);
        let input = list(vec![num(3.0), num(7.0), num(5.0)]);
        assert_eq!(
            eval(&m, &[Some(input)], &r).into_vec(),
            vec![Some(list(vec![num(6.0), num(14.0), num(10.0)]))]
        );
    }

    #[test]
    fn map_none_propagates() {
        // Double applied to a Text element fails ⇒ the whole map is None.
        let mut r = MapResolver::new();
        let hdouble = double(&mut r);
        let m = map_unit(&hdouble);
        let input = list(vec![num(3.0), txt("x")]);
        assert_eq!(eval(&m, &[Some(input)], &r).into_vec(), vec![None]);
    }

    fn fold_unit(fn_hash: &Hash) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "fold".into() },
            params: vec![Param::frozen("fn", hex(fn_hash))],
            inputs: vec![
                Port { name: "init".into(), ty: Type::Any },
                Port { name: "list".into(), ty: Type::list_any() },
            ],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        }
    }

    #[test]
    fn fold_sums_with_add() {
        // sum expressed as a fold (the universal donor): fold(add, 0, [3,7,5]) = 15.
        // `add` is the combiner — add(acc, elem) = acc + elem.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let f = fold_unit(&hadd);
        let xs = list(vec![num(3.0), num(7.0), num(5.0)]);
        assert_eq!(eval(&f, &[Some(num(0.0)), Some(xs)], &r).into_vec(), vec![Some(num(15.0))]);
    }

    #[test]
    fn fold_on_empty_is_init() {
        // No element ⇒ the accumulator is the seed, untouched. Total.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let f = fold_unit(&hadd);
        assert_eq!(eval(&f, &[Some(num(42.0)), Some(list(vec![]))], &r).into_vec(), vec![Some(num(42.0))]);
    }

    #[test]
    fn fold_none_propagates() {
        // add(acc, "x") fails ⇒ the whole fold is None.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let f = fold_unit(&hadd);
        let xs = list(vec![num(3.0), txt("x")]);
        assert_eq!(eval(&f, &[Some(num(0.0)), Some(xs)], &r).into_vec(), vec![None]);
    }

    #[test]
    fn parity_standalone_primitive_eq_composite() {
        // A primitive and an equivalent composite evaluate identically in isolation.
        let mut r = MapResolver::new();
        let comp_add = add2(&mut r);
        let prim_add = prim("add", 2, Type::Number);
        let inp = [Some(num(8.0)), Some(num(9.0))];
        assert_eq!(eval(&prim_add, &inp, &r), eval(&comp_add, &inp, &r));
        assert_eq!(eval(&comp_add, &inp, &r).into_vec(), vec![Some(num(17.0))]);
    }

    #[test]
    fn substitution_in_host_is_invisible() {
        // P1.2.1 / I1: swap a host's child between a *primitive* add and a
        // *composite-wrapping-add* (genuinely different units — different hashes) and the
        // host's output is identical. This is the substitutability claim, not just
        // standalone equality. Host computes (x + y) * z via the swappable add-like child.
        let mut r = MapResolver::new();
        let h_prim = r.insert(prim("add", 2, Type::Number)).unwrap();
        let comp_add = add2(&mut r);
        let h_comp = r.insert(comp_add).unwrap();
        let h_mul = r.insert(prim("multiply", 2, Type::Number)).unwrap();

        // child0 is the swappable "add-like" unit; child1 is multiply.
        let host = |child0: Hash| {
            composite(
                vec![nport("x"), nport("y"), nport("z")],
                Type::Number,
                vec![child0, h_mul],
                vec![
                    edge(BOUNDARY, 0, 0, 0), // x → add-like.0
                    edge(BOUNDARY, 1, 0, 1), // y → add-like.1
                    edge(0, 0, 1, 0),        // add-like.out → mul.0
                    edge(BOUNDARY, 2, 1, 1), // z → mul.1
                    edge(1, 0, BOUNDARY, 0), // mul.out → output
                ],
            )
        };

        assert_ne!(h_prim, h_comp, "the two children must be genuinely different units");

        let host_prim = host(h_prim);
        let host_comp = host(h_comp);
        let inp = [Some(num(3.0)), Some(num(4.0)), Some(num(2.0))]; // (3 + 4) * 2 = 14
        assert_eq!(eval(&host_prim, &inp, &r), eval(&host_comp, &inp, &r));
        assert_eq!(eval(&host_prim, &inp, &r).into_vec(), vec![Some(num(14.0))]);
    }

    #[test]
    fn deterministic() {
        let mut r = MapResolver::new();
        let c = add2(&mut r);
        let inp = [Some(num(3.0)), Some(num(4.0))];
        assert_eq!(eval(&c, &inp, &r), eval(&c, &inp, &r));
    }

    // ── P4.7: native multi-output (composite-only) ──────────────────────────

    #[test]
    fn eval_composite_multi_output() {
        // Three independent ops fed the same two inputs, all three exposed — proving a
        // multi-output composite genuinely carries several distinct values, not one value
        // dressed up in a longer vec.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let hmul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let hsub = r.insert(prim("subtract", 2, Type::Number)).unwrap();
        let c = multi_composite(
            vec![nport("a"), nport("b")],
            vec![Type::Number, Type::Number, Type::Number],
            vec![hadd, hmul, hsub],
            vec![
                edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), // a, b → add
                edge(BOUNDARY, 0, 1, 0), edge(BOUNDARY, 1, 1, 1), // a, b → multiply
                edge(BOUNDARY, 0, 2, 0), edge(BOUNDARY, 1, 2, 1), // a, b → subtract
                edge(0, 0, BOUNDARY, 0), // add    → out0
                edge(1, 0, BOUNDARY, 1), // multiply → out1
                edge(2, 0, BOUNDARY, 2), // subtract → out2
            ],
        );
        assert_eq!(
            eval(&c, &[Some(num(3.0)), Some(num(4.0))], &r).into_vec(),
            vec![Some(num(7.0)), Some(num(12.0)), Some(num(-1.0))]
        );
    }

    #[test]
    fn multi_output_partial_success() {
        // Two exposed outputs from two different children — one succeeds, one fails (divide by
        // zero) — independently. This is the actual reason eval's return shape is per-slot
        // (EvalOutputs, one Option per declared output) rather than Option<Vec<Value>>: a
        // genuinely computed value is never discarded just because a sibling output failed.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let hdiv = r.insert(prim("divide", 2, Type::Number)).unwrap();
        let c = multi_composite(
            vec![nport("a"), nport("b")],
            vec![Type::Number, Type::Number],
            vec![hadd, hdiv],
            vec![
                edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1), // a, b → add
                edge(BOUNDARY, 0, 1, 0), edge(BOUNDARY, 1, 1, 1), // a, b → divide
                edge(0, 0, BOUNDARY, 0), // add    → out0
                edge(1, 0, BOUNDARY, 1), // divide → out1
            ],
        );
        assert_eq!(
            eval(&c, &[Some(num(3.0)), Some(num(0.0))], &r).into_vec(),
            vec![Some(num(3.0)), None] // add(3,0)=3 succeeds; divide(3,0) fails — independently
        );
    }

    #[test]
    fn eval_one_requires_exactly_one_output() {
        // The convenience map/fold's fn uses: fails closed for a multi-output unit (never
        // silently takes the first of several and discards the rest), and matches eval(...)[0]
        // exactly for a genuine single-output unit.
        let mut r = MapResolver::new();
        let hadd = r.insert(prim("add", 2, Type::Number)).unwrap();
        let hmul = r.insert(prim("multiply", 2, Type::Number)).unwrap();
        let multi = multi_composite(
            vec![nport("a"), nport("b")],
            vec![Type::Number, Type::Number],
            vec![hadd, hmul],
            vec![
                edge(BOUNDARY, 0, 0, 0), edge(BOUNDARY, 1, 0, 1),
                edge(BOUNDARY, 0, 1, 0), edge(BOUNDARY, 1, 1, 1),
                edge(0, 0, BOUNDARY, 0),
                edge(1, 0, BOUNDARY, 1),
            ],
        );
        assert_eq!(eval_one(&multi, &[Some(num(3.0)), Some(num(4.0))], &r), None);

        let single = prim("add", 2, Type::Number);
        assert_eq!(eval_one(&single, &[Some(num(3.0)), Some(num(4.0))], &r), Some(num(7.0)));
    }
}