//! Eager graph evaluator (P1.4.3) — the leaf that closes "identity but not evaluation".
//!
//! `eval(unit, inputs, resolver) -> Option<Value>`. A primitive dispatches to its
//! native `compute`; a composite resolves its inner DAG **by reference** (I3) in a
//! single topological pass (acyclicity from P1.3.1 means one pass reaches the fixed
//! point), injecting promoted inputs through `BOUNDARY→child` edges and reading the
//! exposed output through the `child→BOUNDARY` edge. A primitive and a composite are
//! evaluated through the **identical** entry point (I1). `None` means "not computable"
//! and **propagates**.
//!
//! ## Firing rule (the pure subset)
//! The evaluator gathers each node's *present* inputs (in port order) and lets the
//! primitive's `compute` decide — its arity/type checks realize `required/optional`
//! for the current vocabulary. Port memory (`persist`) and firing rounds (`consume`)
//! stay out of the pure core (state boundary / async, P2.4.2). An explicit per-port
//! `required/optional/default` field is deferred until a primitive needs a positional
//! optional with a default — none does yet.
//!
//! ## `Map` / `ctx`
//! `compute` is first-order here. The higher-order `Map` and the evaluation context it
//! needs (`apply(hash, inputs)`) arrive with P1.5.1; the evaluator is the natural
//! provider of that context.

use crate::{primitives, Edge, Hash, Resolver, Unit, UnitBody, Value, BOUNDARY};
use std::collections::{BTreeMap, BTreeSet};

/// Evaluate a unit against values for its input ports (index = input-port index;
/// `None` = absent). Returns the unit's single output (port 0), or `None`.
pub fn eval(unit: &Unit, inputs: &[Option<Value>], resolver: &impl Resolver) -> Option<Value> {
    match &unit.body {
        UnitBody::Primitive { name } => {
            // Gather present values in port order; compute's arity check is the firing rule.
            let present: Vec<Value> = inputs.iter().flatten().cloned().collect();
            primitives::compute(name, &present, &unit.params)
        }
        UnitBody::Composite { children, edges } => {
            eval_composite(children, edges, inputs, resolver)
        }
    }
}

fn eval_composite(
    children: &[Hash],
    edges: &[Edge],
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
) -> Option<Value> {
    let n = children.len();

    // Resolve children (by hash). Any unresolved ⇒ not evaluable.
    let mut resolved: Vec<&Unit> = Vec::with_capacity(n);
    for h in children {
        resolved.push(resolver.resolve(h)?);
    }

    // Index incoming edges: destination (to_child, to_port) → source (from_child, from_port).
    // Validation guarantees ≤1 source per destination, so this never loses an edge.
    let mut incoming: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for e in edges {
        incoming.insert((e.to_child, e.to_port), (e.from_child, e.from_port));
    }

    // One topological pass (deterministic; ties broken by ascending child index).
    let order = topo_order(n, edges)?;

    // Per-evaluation scratch, indexed by child index (fresh each call ⇒ nested and
    // repeated composites never collide).
    let mut scratch: Vec<Option<Value>> = vec![None; n];

    for ci in order {
        let child = resolved[ci];
        let k = child.inputs.len();
        let mut gathered: Vec<Option<Value>> = Vec::with_capacity(k);
        for p in 0..k {
            let val = match incoming.get(&(ci as u32, p as u32)) {
                // promoted composite input arriving through the boundary
                Some(&(BOUNDARY, fp)) => inputs.get(fp as usize).cloned().flatten(),
                // another child's (already-evaluated) output
                Some(&(fc, _fp)) => scratch[fc as usize].clone(),
                // unconnected port
                None => None,
            };
            gathered.push(val);
        }
        scratch[ci] = eval(child, &gathered, resolver);
    }

    // The exposed output is whatever feeds the output boundary (port 0).
    match incoming.get(&(BOUNDARY, 0)) {
        Some(&(fc, _fp)) => scratch[fc as usize].clone(),
        None => None,
    }
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
            output: Some(out),
        }
    }
    fn composite(inputs: Vec<Port>, out: Type, children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        Unit { body: UnitBody::Composite { children, edges }, params: vec![], inputs, output: Some(out) }
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
            eval(&add, &[Some(num(3.0)), Some(num(7.0)), Some(num(5.0))], &r),
            Some(num(15.0))
        );
    }

    #[test]
    fn eval_composite_add() {
        let mut r = MapResolver::new();
        let c = add2(&mut r);
        assert_eq!(eval(&c, &[Some(num(3.0)), Some(num(4.0))], &r), Some(num(7.0)));
    }

    #[test]
    fn eval_composite_sum() {
        // The v3 seed (minus Map): list(a,b,c) → sum → 15.
        let mut r = MapResolver::new();
        let hlist = r.insert(prim("list", 3, Type::List)).unwrap();
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
            eval(&c, &[Some(num(3.0)), Some(num(7.0)), Some(num(5.0))], &r),
            Some(num(15.0))
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
        assert_eq!(eval(&outer, &[Some(num(10.0)), Some(num(5.0))], &r), Some(num(15.0)));
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
        assert_eq!(eval(&c, &[Some(num(3.0)), None], &r), None);
    }

    #[test]
    fn parity_standalone_primitive_eq_composite() {
        // A primitive and an equivalent composite evaluate identically in isolation.
        let mut r = MapResolver::new();
        let comp_add = add2(&mut r);
        let prim_add = prim("add", 2, Type::Number);
        let inp = [Some(num(8.0)), Some(num(9.0))];
        assert_eq!(eval(&prim_add, &inp, &r), eval(&comp_add, &inp, &r));
        assert_eq!(eval(&comp_add, &inp, &r), Some(num(17.0)));
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
        assert_eq!(eval(&host_prim, &inp, &r), Some(num(14.0)));
    }

    #[test]
    fn deterministic() {
        let mut r = MapResolver::new();
        let c = add2(&mut r);
        let inp = [Some(num(3.0)), Some(num(4.0))];
        assert_eq!(eval(&c, &inp, &r), eval(&c, &inp, &r));
    }
}