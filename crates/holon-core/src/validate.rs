//! Graph validation (P1.3.1) — the gate upstream of evaluation.
//!
//! Three independent checks over a composite's wiring:
//! 1. **well-formedness** — edge endpoints exist (child or [`crate::BOUNDARY`]),
//!    ports are in range, and **each destination port has at most one incoming edge**
//!    (single source per input — the evaluator depends on this);
//! 2. **acyclicity** — the inner child→child graph is a DAG;
//! 3. **type compatibility** — `dst == any ∨ src == any ∨ src == dst` (§5).
//!
//! Errors are **collected** so an editor can surface all problems at once.
//!
//! ## The composite boundary
//! The composite's interface↔inner binding rides the ordinary edge mechanism via the
//! reserved [`crate::BOUNDARY`] endpoint: an edge `from_child == BOUNDARY` carries the
//! composite's input `from_port` inward; an edge `to_child == BOUNDARY` exposes an inner
//! output as the composite's output `to_port`. From the inner graph's view the boundary
//! is a *source* for inputs and a *sink* for outputs — so it never forms a cycle, and it
//! needs no new fields and no encoder change.

use crate::{Edge, Resolver, Type, Unit, UnitBody, BOUNDARY};
use std::collections::BTreeSet;

/// A single validation failure.
#[derive(Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// An edge endpoint references a child index that doesn't exist.
    ChildOutOfRange { edge: usize, child: u32 },
    /// A child referenced by hash couldn't be resolved.
    UnresolvedChild { child: u32 },
    /// An edge's source port isn't a valid output of its source child
    /// (single output / port 0 for now; a source with no output also lands here).
    OutputPortOutOfRange { edge: usize, child: u32, port: u32 },
    /// An edge's destination port isn't a valid input of its destination child.
    InputPortOutOfRange { edge: usize, child: u32, port: u32 },
    /// A boundary-source edge names a composite-input index that doesn't exist.
    BoundaryInputOutOfRange { edge: usize, port: u32 },
    /// A boundary-sink edge names a composite-output index that doesn't exist
    /// (single output / port 0 for now; no declared output also lands here).
    BoundaryOutputOutOfRange { edge: usize, port: u32 },
    /// Two edges feed the same destination port (ambiguous single source).
    DuplicateInputEdge { to_child: u32, to_port: u32 },
    /// The inner wiring contains a cycle.
    Cycle,
    /// An edge connects incompatible types.
    TypeMismatch { edge: usize, src: Type, dst: Type },
}

/// Wire-time type compatibility (§5) — structural equality plus the single `Any`
/// escape hatch, made recursive over `List<T>`: `List<A>` fits `List<B>` iff `A` fits
/// `B`, and `Any` matches anything at any level. No inference, no subtyping — the §5
/// rule, lifted to nested types.
pub fn compatible(src: &Type, dst: &Type) -> bool {
    match (src, dst) {
        (_, Type::Any) | (Type::Any, _) => true,
        (Type::List(a), Type::List(b)) => compatible(a, b),
        (Type::Product(a), Type::Product(b)) | (Type::Sum(a), Type::Sum(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| compatible(x, y))
        }
        _ => src == dst, // scalar equality (Number / Boolean / Text / Effect)
    }
}

/// Validate a unit's graph. A primitive always passes. A composite is checked for
/// well-formedness, acyclicity, and type compatibility — including its boundary edges.
pub fn validate(unit: &Unit, resolver: &impl Resolver) -> Result<(), Vec<ValidationError>> {
    let (children, edges) = match &unit.body {
        UnitBody::Primitive { .. } => return Ok(()),
        UnitBody::Composite { children, edges } => (children, edges),
    };

    let mut errors = Vec::new();
    let n = children.len();

    // Resolve each child once (interface lookups need it).
    let resolved: Vec<Option<&Unit>> = children.iter().map(|h| resolver.resolve(h)).collect();
    for (i, r) in resolved.iter().enumerate() {
        if r.is_none() {
            errors.push(ValidationError::UnresolvedChild { child: i as u32 });
        }
    }

    // Track destination ports so a second source to the same port is rejected.
    let mut seen_dst: BTreeSet<(u32, u32)> = BTreeSet::new();

    for (ei, e) in edges.iter().enumerate() {
        if !seen_dst.insert((e.to_child, e.to_port)) {
            errors.push(ValidationError::DuplicateInputEdge {
                to_child: e.to_child,
                to_port: e.to_port,
            });
            continue;
        }

        // ---- source type (a real child's output, or a composite input via BOUNDARY) ----
        let src_ty = if e.from_child == BOUNDARY {
            match unit.inputs.get(e.from_port as usize) {
                Some(p) => p.ty.clone(),
                None => {
                    errors.push(ValidationError::BoundaryInputOutOfRange { edge: ei, port: e.from_port });
                    continue;
                }
            }
        } else {
            let fc = e.from_child as usize;
            if fc >= n {
                errors.push(ValidationError::ChildOutOfRange { edge: ei, child: e.from_child });
                continue;
            }
            match resolved[fc] {
                None => continue, // unresolved child already reported
                Some(src) => match (e.from_port, &src.output) {
                    (0, Some(t)) => t.clone(),
                    _ => {
                        errors.push(ValidationError::OutputPortOutOfRange {
                            edge: ei,
                            child: e.from_child,
                            port: e.from_port,
                        });
                        continue;
                    }
                },
            }
        };

        // ---- destination type (a real child's input, or the composite output via BOUNDARY) ----
        let dst_ty = if e.to_child == BOUNDARY {
            match (e.to_port, &unit.output) {
                (0, Some(t)) => t.clone(),
                _ => {
                    errors.push(ValidationError::BoundaryOutputOutOfRange { edge: ei, port: e.to_port });
                    continue;
                }
            }
        } else {
            let tc = e.to_child as usize;
            if tc >= n {
                errors.push(ValidationError::ChildOutOfRange { edge: ei, child: e.to_child });
                continue;
            }
            match resolved[tc] {
                None => continue,
                Some(dst) => match dst.inputs.get(e.to_port as usize) {
                    Some(p) => p.ty.clone(),
                    None => {
                        errors.push(ValidationError::InputPortOutOfRange {
                            edge: ei,
                            child: e.to_child,
                            port: e.to_port,
                        });
                        continue;
                    }
                },
            }
        };

        if !compatible(&src_ty, &dst_ty) {
            errors.push(ValidationError::TypeMismatch { edge: ei, src: src_ty, dst: dst_ty });
        }
    }

    if has_cycle(n, edges) {
        errors.push(ValidationError::Cycle);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Kahn's algorithm over the in-range **child→child** edges. Boundary edges (an
/// endpoint == [`BOUNDARY`], which is not `< n`) are excluded, so the composite
/// boundary can never form a false cycle. The boolean result is order-independent.
fn has_cycle(n: usize, edges: &[Edge]) -> bool {
    let mut indeg = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        let (f, t) = (e.from_child as usize, e.to_child as usize);
        if f < n && t < n {
            adj[f].push(t);
            indeg[t] += 1;
        }
    }
    let mut stack: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut removed = 0usize;
    while let Some(node) = stack.pop() {
        removed += 1;
        for &m in &adj[node] {
            indeg[m] -= 1;
            if indeg[m] == 0 {
                stack.push(m);
            }
        }
    }
    removed != n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edge, Hash, MapResolver, Port, Unit, UnitBody};

    fn num_binop(name: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: vec![
                Port { name: "a".into(), ty: Type::Number },
                Port { name: "b".into(), ty: Type::Number },
            ],
            output: Some(Type::Number),
        }
    }
    fn text_source() -> Unit {
        // A Text-typed wiring stub (validate never evaluates it), so a bare `const` suffices —
        // no `value`/`type` params needed for type-checking.
        Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![],
            inputs: vec![],
            output: Some(Type::Text),
        }
    }
    fn composite(children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        composite_io(vec![Port { name: "a".into(), ty: Type::Number }], Some(Type::Number), children, edges)
    }
    fn composite_io(inputs: Vec<Port>, output: Option<Type>, children: Vec<Hash>, edges: Vec<Edge>) -> Unit {
        Unit { body: UnitBody::Composite { children, edges }, params: vec![], inputs, output }
    }
    fn edge(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }

    // ── inner-wiring checks ──────────────────────────────────────────────
    #[test]
    fn compatibility_rule() {
        use Type::*;
        assert!(compatible(&Number, &Number));
        assert!(compatible(&Any, &Number));
        assert!(compatible(&Number, &Any));
        assert!(!compatible(&Number, &Text));
        assert!(!compatible(&Text, &Number));
    }

    #[test]
    fn compatible_is_structural() {
        use Type::*;
        let list = |t: Type| Type::List(Box::new(t));
        // List<Any> bridges any element type (constructed lists stay permissive).
        assert!(compatible(&Type::list_any(), &list(Number)));
        assert!(compatible(&list(Number), &Type::list_any()));
        // Two precisely-declared list types are checked element-wise.
        assert!(compatible(&list(Number), &list(Number)));
        assert!(!compatible(&list(Text), &list(Number)));
        // …and recursively.
        assert!(compatible(&list(list(Number)), &list(list(Any))));
        assert!(!compatible(&list(list(Text)), &list(list(Number))));
        // A list is not a scalar.
        assert!(!compatible(&list(Number), &Number));
        // Product: arity + position, Any wildcard at each slot; a Product is not a List.
        let pair = |a: Type, b: Type| Type::Product(vec![a, b]);
        assert!(compatible(&pair(Number, Text), &pair(Number, Any)));
        assert!(!compatible(&pair(Number, Text), &pair(Number, Number)));
        assert!(!compatible(&pair(Number, Text), &Type::Product(vec![Number]))); // arity
        assert!(!compatible(&pair(Number, Text), &list(Number))); // Product ≠ List

        // Sum: the same arity-+-position rule as Product, but a Sum is not a Product, even at
        // identical shape — a tagged choice and a tuple are different things (P1.3.2 inch 3).
        let option_of = |t: Type| Type::Sum(vec![t, Type::Product(vec![])]); // T or Unit
        assert!(compatible(&option_of(Number), &option_of(Any)));
        assert!(!compatible(&option_of(Number), &option_of(Text)));
        assert!(!compatible(&Type::Sum(vec![Number, Text]), &Type::Sum(vec![Number]))); // arity
        assert!(!compatible(&Type::Product(vec![Number, Text]), &Type::Sum(vec![Number, Text])));
    }

    #[test]
    fn primitive_always_valid() {
        let r = MapResolver::new();
        assert_eq!(validate(&num_binop("add"), &r), Ok(()));
    }

    #[test]
    fn valid_composite_passes() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let c = composite(vec![h0, h1], vec![edge(0, 0, 1, 0)]);
        assert_eq!(validate(&c, &r), Ok(()));
    }

    #[test]
    fn out_of_range_child_rejected() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let c = composite(vec![h0, h1], vec![edge(0, 0, 5, 0)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::ChildOutOfRange { edge: 0, child: 5 }));
    }

    #[test]
    fn cycle_rejected() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let c = composite(vec![h0, h1], vec![edge(0, 0, 1, 0), edge(1, 0, 0, 0)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::Cycle));
    }

    #[test]
    fn type_mismatch_rejected() {
        let mut r = MapResolver::new();
        let ht = r.insert(text_source()).unwrap();
        let hn = r.insert(num_binop("add")).unwrap();
        let c = composite(vec![ht, hn], vec![edge(0, 0, 1, 0)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::TypeMismatch { edge: 0, src: Type::Text, dst: Type::Number }));
    }

    #[test]
    fn output_port_out_of_range_rejected() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let c = composite(vec![h0, h1], vec![edge(0, 1, 1, 0)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.iter().any(|e| matches!(e, ValidationError::OutputPortOutOfRange { .. })));
    }

    #[test]
    fn unresolved_child_rejected() {
        let r = MapResolver::new();
        let c = composite(vec![[0u8; 32]], vec![]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::UnresolvedChild { child: 0 }));
    }

    #[test]
    fn collects_multiple_errors() {
        let mut r = MapResolver::new();
        let h0 = r.insert(num_binop("add")).unwrap();
        let h1 = r.insert(num_binop("multiply")).unwrap();
        let h2 = r.insert(text_source()).unwrap();
        let c = composite(vec![h0, h1, h2], vec![edge(0, 0, 1, 0), edge(1, 0, 0, 0), edge(2, 0, 0, 1)]);
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::Cycle));
        assert!(errs.iter().any(|e| matches!(e, ValidationError::TypeMismatch { .. })));
        assert!(errs.len() >= 2);
    }

    // ── boundary checks ──────────────────────────────────────────────────
    #[test]
    fn boundary_wiring_valid() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        // input a → add.a ; add.out → output
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Some(Type::Number),
            vec![hadd],
            vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, BOUNDARY, 0)],
        );
        assert_eq!(validate(&c, &r), Ok(()));
    }

    #[test]
    fn boundary_input_type_mismatch() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Text }], // Text feeding a Number port
            Some(Type::Number),
            vec![hadd],
            vec![edge(BOUNDARY, 0, 0, 0)],
        );
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::TypeMismatch { edge: 0, src: Type::Text, dst: Type::Number }));
    }

    #[test]
    fn boundary_input_out_of_range() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Some(Type::Number),
            vec![hadd],
            vec![edge(BOUNDARY, 5, 0, 0)], // composite has only input 0
        );
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::BoundaryInputOutOfRange { edge: 0, port: 5 }));
    }

    #[test]
    fn boundary_output_out_of_range() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Some(Type::Number),
            vec![hadd],
            vec![edge(0, 0, BOUNDARY, 3)], // only output 0 exists
        );
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::BoundaryOutputOutOfRange { edge: 0, port: 3 }));
    }

    #[test]
    fn duplicate_input_edge_rejected() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let hmul = r.insert(num_binop("multiply")).unwrap();
        // two sources feed child 1, port 0
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Some(Type::Number),
            vec![hadd, hmul],
            vec![edge(0, 0, 1, 0), edge(BOUNDARY, 0, 1, 0)],
        );
        let errs = validate(&c, &r).unwrap_err();
        assert!(errs.contains(&ValidationError::DuplicateInputEdge { to_child: 1, to_port: 0 }));
    }

    #[test]
    fn boundary_does_not_create_false_cycle() {
        let mut r = MapResolver::new();
        let hadd = r.insert(num_binop("add")).unwrap();
        let hmul = r.insert(num_binop("multiply")).unwrap();
        // BOUNDARY → add → mul → BOUNDARY : a normal input→output flow, acyclic
        let c = composite_io(
            vec![Port { name: "a".into(), ty: Type::Number }],
            Some(Type::Number),
            vec![hadd, hmul],
            vec![edge(BOUNDARY, 0, 0, 0), edge(0, 0, 1, 0), edge(1, 0, BOUNDARY, 0)],
        );
        assert_eq!(validate(&c, &r), Ok(()));
    }
}