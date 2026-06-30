//! Encapsulation — derive-then-freeze (P1.5.1).
//!
//! Turn a self-contained selection `(children, inner_edges)` into a new hashed `Unit`
//! whose interface is **derived** (types, boundary edges) and whose **choices** (which
//! inputs promote, their order = identity, their names, which output is exposed) come
//! from an [`EncapSpec`]. `derive_and_freeze` generates exactly the `BOUNDARY` edges the
//! evaluator and validator already consume — encapsulation is their inverse.
//!
//! "Freeze" is not a step: the derived interface is baked into the hashed `Unit`, so by
//! I5 it cannot drift — re-deriving from edited children yields a *different* unit.
//!
//! Out of scope here: param promotion (P1.5.2 — `params` stays empty) and kind
//! derivation (P2.2.1). The selection must already be self-contained and 0-indexed
//! (subgraph extraction/renumbering is the editor's job).

use crate::{validate, Edge, Hash, Port, Resolver, Unit, UnitBody, ValidationError, BOUNDARY};
use std::collections::BTreeSet;

/// A choice to promote an inner `(child, port)` to the new unit's interface, with the
/// interface port `name`. The **order** of these in [`EncapSpec::promote`] is the
/// interface input order — and therefore part of identity.
#[derive(Clone, Debug, PartialEq)]
pub struct Promote {
    pub child: u32,
    pub port: u32,
    pub name: String,
}

/// The authoring choices encapsulation cannot derive: which inputs to promote (ordered)
/// and which inner output to expose.
#[derive(Clone, Debug, PartialEq)]
pub struct EncapSpec {
    pub promote: Vec<Promote>,
    pub output: (u32, u32), // (child, port); port is 0 for now (single output)
}

/// Why an encapsulation was rejected.
#[derive(Debug, PartialEq)]
pub enum EncapError {
    UnresolvedChild(u32),
    PromoteChildOutOfRange(u32),
    PromotePortOutOfRange { child: u32, port: u32 },
    /// The promoted port already has an incoming inner edge (would be a second source).
    PromoteAlreadyBound { child: u32, port: u32 },
    DuplicatePromotion { child: u32, port: u32 },
    OutputChildOutOfRange(u32),
    /// The output port isn't 0, or the child has no output.
    OutputPortInvalid { child: u32, port: u32 },
    /// `default_spec` found zero or more than one free output — the author must choose.
    AmbiguousDefaultOutput,
    /// The derived unit failed validation (should be impossible by construction).
    Invalid(Vec<ValidationError>),
}

/// Derive a unit from a selection and a spec: validate the choices, derive the interface
/// types, generate the boundary edges, assemble, and validate the result.
pub fn derive_and_freeze(
    children: Vec<Hash>,
    inner_edges: Vec<Edge>,
    spec: &EncapSpec,
    resolver: &impl Resolver,
) -> Result<Unit, EncapError> {
    let k = children.len();

    let mut resolved: Vec<&Unit> = Vec::with_capacity(k);
    for (i, h) in children.iter().enumerate() {
        match resolver.resolve(h) {
            Some(u) => resolved.push(u),
            None => return Err(EncapError::UnresolvedChild(i as u32)),
        }
    }

    let bound: BTreeSet<(u32, u32)> =
        inner_edges.iter().map(|e| (e.to_child, e.to_port)).collect();

    // Validate promotions and derive the interface input ports (types from inner ports).
    let mut inputs: Vec<Port> = Vec::with_capacity(spec.promote.len());
    let mut seen: BTreeSet<(u32, u32)> = BTreeSet::new();
    for pr in &spec.promote {
        let ci = pr.child as usize;
        if ci >= k {
            return Err(EncapError::PromoteChildOutOfRange(pr.child));
        }
        let pp = pr.port as usize;
        if pp >= resolved[ci].inputs.len() {
            return Err(EncapError::PromotePortOutOfRange { child: pr.child, port: pr.port });
        }
        if bound.contains(&(pr.child, pr.port)) {
            return Err(EncapError::PromoteAlreadyBound { child: pr.child, port: pr.port });
        }
        if !seen.insert((pr.child, pr.port)) {
            return Err(EncapError::DuplicatePromotion { child: pr.child, port: pr.port });
        }
        inputs.push(Port { name: pr.name.clone(), ty: resolved[ci].inputs[pp].ty.clone() });
    }

    // Validate the exposed output and derive its type.
    let (oc, op) = spec.output;
    if oc as usize >= k {
        return Err(EncapError::OutputChildOutOfRange(oc));
    }
    let out_ty = match (op, &resolved[oc as usize].output) {
        (0, Some(t)) => t.clone(),
        _ => return Err(EncapError::OutputPortInvalid { child: oc, port: op }),
    };

    // Generate boundary edges (the dual of how eval reads them).
    let mut edges = inner_edges;
    for (i, pr) in spec.promote.iter().enumerate() {
        edges.push(Edge { from_child: BOUNDARY, from_port: i as u32, to_child: pr.child, to_port: pr.port });
    }
    edges.push(Edge { from_child: oc, from_port: op, to_child: BOUNDARY, to_port: 0 });

    let unit = Unit {
        body: UnitBody::Composite { children, edges },
        params: vec![],
        inputs,
        output: Some(out_ty),
    };

    match validate(&unit, resolver) {
        Ok(()) => Ok(unit),
        Err(errs) => Err(EncapError::Invalid(errs)),
    }
}

/// The canonical proposal: promote every unbound inner input in `(child, port)` order
/// (named after its inner port), and expose the unique child whose output is unconsumed.
/// `AmbiguousDefaultOutput` when that child is not unique — the author must choose.
pub fn default_spec(
    children: &[Hash],
    inner_edges: &[Edge],
    resolver: &impl Resolver,
) -> Result<EncapSpec, EncapError> {
    let k = children.len();

    let mut resolved: Vec<&Unit> = Vec::with_capacity(k);
    for (i, h) in children.iter().enumerate() {
        match resolver.resolve(h) {
            Some(u) => resolved.push(u),
            None => return Err(EncapError::UnresolvedChild(i as u32)),
        }
    }

    let bound: BTreeSet<(u32, u32)> =
        inner_edges.iter().map(|e| (e.to_child, e.to_port)).collect();

    let mut promote = Vec::new();
    for ci in 0..k {
        for p in 0..resolved[ci].inputs.len() {
            if !bound.contains(&(ci as u32, p as u32)) {
                promote.push(Promote {
                    child: ci as u32,
                    port: p as u32,
                    name: resolved[ci].inputs[p].name.clone(),
                });
            }
        }
    }

    let consumed: BTreeSet<u32> = inner_edges.iter().map(|e| e.from_child).collect();
    let mut free: Vec<u32> = (0..k as u32)
        .filter(|c| !consumed.contains(c) && resolved[*c as usize].output.is_some())
        .collect();
    if free.len() != 1 {
        return Err(EncapError::AmbiguousDefaultOutput);
    }
    let output = (free.pop().unwrap(), 0);

    Ok(EncapSpec { promote, output })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{eval, hash_unit, MapResolver, Type, Value};

    fn binop(name: &str, p0: &str, p1: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: vec![
                Port { name: p0.into(), ty: Type::Number },
                Port { name: p1.into(), ty: Type::Number },
            ],
            output: Some(Type::Number),
        }
    }
    fn e(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }
    fn num(n: f64) -> Value { Value::Number(n) }
    fn port(name: &str) -> Port { Port { name: name.into(), ty: Type::Number } }

    #[test]
    fn encapsulates_single_unit_to_expected_hash() {
        // Wrapping a bare `add` (both inputs unbound) yields exactly the hand-built add2.
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let spec = default_spec(&[hadd], &[], &r).unwrap();
        let derived = derive_and_freeze(vec![hadd], vec![], &spec, &r).unwrap();

        let manual = Unit {
            body: UnitBody::Composite {
                children: vec![hadd],
                edges: vec![e(BOUNDARY, 0, 0, 0), e(BOUNDARY, 1, 0, 1), e(0, 0, BOUNDARY, 0)],
            },
            params: vec![],
            inputs: vec![port("a"), port("b")],
            output: Some(Type::Number),
        };
        // Edge insertion order differs, but the encoder normalizes edges ⇒ equal hashes.
        assert_eq!(hash_unit(&derived).unwrap(), hash_unit(&manual).unwrap());
    }

    #[test]
    fn encapsulated_evaluates_correctly() {
        // selection: add(a,b) → mul.0 ; mul.1 unbound. Encapsulated = (a + b) * q.
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let hmul = r.insert(binop("multiply", "p", "q")).unwrap();
        let children = vec![hadd, hmul];
        let inner = vec![e(0, 0, 1, 0)]; // add.out → mul.0

        let spec = default_spec(&children, &inner, &r).unwrap();
        // default promotes add.a, add.b, mul.q (mul.p is bound); exposes mul.
        assert_eq!(spec.promote.len(), 3);
        assert_eq!(spec.output, (1, 0));

        let derived = derive_and_freeze(children, inner, &spec, &r).unwrap();
        // (2 + 3) * 4 = 20
        assert_eq!(
            eval(&derived, &[Some(num(2.0)), Some(num(3.0)), Some(num(4.0))], &r),
            Some(num(20.0))
        );
    }

    #[test]
    fn default_spec_promotes_unbound_canonically() {
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let hmul = r.insert(binop("multiply", "p", "q")).unwrap();
        let spec = default_spec(&[hadd, hmul], &[e(0, 0, 1, 0)], &r).unwrap();
        let promoted: Vec<(u32, u32, &str)> =
            spec.promote.iter().map(|p| (p.child, p.port, p.name.as_str())).collect();
        assert_eq!(promoted, vec![(0, 0, "a"), (0, 1, "b"), (1, 1, "q")]);
        assert_eq!(spec.output, (1, 0));
    }

    #[test]
    fn order_is_identity() {
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let spec_ab = EncapSpec {
            promote: vec![
                Promote { child: 0, port: 0, name: "a".into() },
                Promote { child: 0, port: 1, name: "b".into() },
            ],
            output: (0, 0),
        };
        let spec_ba = EncapSpec {
            promote: vec![
                Promote { child: 0, port: 1, name: "b".into() },
                Promote { child: 0, port: 0, name: "a".into() },
            ],
            output: (0, 0),
        };
        let d_ab = derive_and_freeze(vec![hadd], vec![], &spec_ab, &r).unwrap();
        let d_ba = derive_and_freeze(vec![hadd], vec![], &spec_ba, &r).unwrap();
        assert_ne!(hash_unit(&d_ab).unwrap(), hash_unit(&d_ba).unwrap());
    }

    #[test]
    fn rejects_promoting_bound_port() {
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let hmul = r.insert(binop("multiply", "p", "q")).unwrap();
        let spec = EncapSpec {
            promote: vec![Promote { child: 1, port: 0, name: "p".into() }], // mul.0 is bound
            output: (1, 0),
        };
        let err = derive_and_freeze(vec![hadd, hmul], vec![e(0, 0, 1, 0)], &spec, &r).unwrap_err();
        assert_eq!(err, EncapError::PromoteAlreadyBound { child: 1, port: 0 });
    }

    #[test]
    fn rejects_invalid_output() {
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let spec = EncapSpec { promote: vec![], output: (0, 3) }; // port 3 invalid
        let err = derive_and_freeze(vec![hadd], vec![], &spec, &r).unwrap_err();
        assert_eq!(err, EncapError::OutputPortInvalid { child: 0, port: 3 });
    }

    #[test]
    fn ambiguous_default_output() {
        // Two independent (unconsumed) children ⇒ no unique free output.
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let err = default_spec(&[hadd, hadd], &[], &r).unwrap_err();
        assert_eq!(err, EncapError::AmbiguousDefaultOutput);
    }

    #[test]
    fn derived_unit_validates() {
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let spec = default_spec(&[hadd], &[], &r).unwrap();
        let derived = derive_and_freeze(vec![hadd], vec![], &spec, &r).unwrap();
        assert!(validate(&derived, &r).is_ok());
    }
}