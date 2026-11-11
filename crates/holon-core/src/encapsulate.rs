//! Encapsulation — derive-then-freeze (P1.5.1).
//!
//! Turn a self-contained selection `(children, inner_edges)` into a new hashed `Unit`
//! whose interface is **derived** (types, boundary edges) and whose **choices** (which
//! inputs promote, their order = identity, their names, which outputs are exposed) come
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
use std::collections::{BTreeMap, BTreeSet};

/// A choice to promote an inner `(child, port)` to the new unit's interface, with the
/// interface port `name`. The **order** of these in [`EncapSpec::promote`] is the
/// interface input order — and therefore part of identity.
#[derive(Clone, Debug, PartialEq)]
pub struct Promote {
    pub child: u32,
    pub port: u32,
    pub name: String,
}

/// A choice to expose an inner `(child, port)` *output* at the new unit's interface, with
/// the interface port `name`. The **order** of these in [`EncapSpec::expose`] is the
/// interface output order — and therefore part of identity (P4.7: exposing several is now
/// possible, mirroring `Promote` on the input side).
///
/// Structurally identical to [`Promote`] on purpose kept as its own type, not a reuse:
/// swapping `promote`/`expose` at a call site is then a compile error, not a silent bug —
/// the same "let the compiler enforce it" discipline `eval_one`/`eval` already lean on.
///
/// Unlike `Promote`, exposing the same `(child, port)` twice under different names is not
/// rejected — there is no structural conflict the way there is on the input side (an input
/// port has at most one source, so promoting it twice would create one; an output can
/// already fan out to many consumers, so exposing it under two names is just two more
/// consumers, not a new capability). Not a missed check — there is nothing to check.
#[derive(Clone, Debug, PartialEq)]
pub struct Expose {
    pub child: u32,
    pub port: u32,
    pub name: String,
}

/// The authoring choices encapsulation cannot derive: which inputs to promote (ordered)
/// and which inner outputs to expose (ordered).
#[derive(Clone, Debug, PartialEq)]
pub struct EncapSpec {
    pub promote: Vec<Promote>,
    pub expose: Vec<Expose>,
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
    ExposeChildOutOfRange(u32),
    /// The exposed port is out of range for however many outputs the chosen child declares.
    ExposePortInvalid { child: u32, port: u32 },
    /// The derived unit failed validation (should be impossible by construction).
    Invalid(Vec<ValidationError>),
}

impl std::fmt::Display for EncapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncapError::UnresolvedChild(c) => write!(f, "child {c} couldn't be resolved"),
            EncapError::PromoteChildOutOfRange(c) => {
                write!(f, "promotion references child {c}, which doesn't exist")
            }
            EncapError::PromotePortOutOfRange { child, port } => {
                write!(f, "promotion references input port {port} of child {child}, which doesn't exist")
            }
            EncapError::PromoteAlreadyBound { child, port } => write!(
                f,
                "input port {port} of child {child} is already wired internally and can't also be promoted"
            ),
            EncapError::DuplicatePromotion { child, port } => {
                write!(f, "input port {port} of child {child} is promoted more than once")
            }
            EncapError::ExposeChildOutOfRange(c) => {
                write!(f, "the exposed output references child {c}, which doesn't exist")
            }
            EncapError::ExposePortInvalid { child, port } => {
                write!(f, "child {child} has no output at port {port}")
            }
            EncapError::Invalid(errs) => {
                let named: Vec<String> = errs.iter().map(|e| e.to_string()).collect();
                write!(f, "the derived unit failed validation: {}", named.join("; "))
            }
        }
    }
}

impl std::error::Error for EncapError {}

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

    // Validate each exposed output and derive its type — the dual of the promote loop
    // above. No `seen`/duplicate check here on purpose: see Expose's own doc comment for
    // why exposing the same (child, port) twice isn't a structural conflict the way
    // promoting the same input twice is.
    let mut outputs: Vec<Port> = Vec::with_capacity(spec.expose.len());
    for ex in &spec.expose {
        let ci = ex.child as usize;
        if ci >= k {
            return Err(EncapError::ExposeChildOutOfRange(ex.child));
        }
        let ty = match resolved[ci].outputs.get(ex.port as usize) {
            Some(p) => p.ty.clone(),
            None => return Err(EncapError::ExposePortInvalid { child: ex.child, port: ex.port }),
        };
        outputs.push(Port { name: ex.name.clone(), ty });
    }

    // Generate boundary edges (the dual of how eval reads them).
    let mut edges = inner_edges;
    for (i, pr) in spec.promote.iter().enumerate() {
        edges.push(Edge { from_child: BOUNDARY, from_port: i as u32, to_child: pr.child, to_port: pr.port });
    }
    for (i, ex) in spec.expose.iter().enumerate() {
        edges.push(Edge { from_child: ex.child, from_port: ex.port, to_child: BOUNDARY, to_port: i as u32 });
    }

    let unit = Unit {
        body: UnitBody::Composite { children, edges },
        params: vec![],
        inputs,
        outputs,
    };

    match validate(&unit, resolver) {
        Ok(()) => Ok(unit),
        Err(errs) => Err(EncapError::Invalid(errs)),
    }
}

/// The canonical proposal: promote every unbound inner input, and expose every free inner
/// output — both walked in `(child, port)` order, both named after their inner port, both
/// with zero ambiguity check. Symmetric with the input side on purpose: an input with no
/// inner source becomes a boundary input; an output with no inner destination becomes a
/// boundary output. Neither side asks "is there exactly one" — a closed composite with
/// zero free inputs already promotes nothing, and this now does the same for outputs
/// (zero free outputs ⇒ `expose: vec![]`, a composite whose effects are discarded — an
/// honest fact, not a gap, the same reasoning `eval`'s own P4.7 design settled).
///
/// `default_spec` cannot itself decide *which* of several free outputs an author actually
/// wants surfaced as "the" result of a graph — that's a real choice, deliberately left to
/// a later, separately-scoped editor inch (letting a person pick, rather than always
/// getting everything free). This function only ever proposes the maximal, fully
/// automatic answer: whatever's free, all of it.
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
    for (ci, child) in resolved.iter().enumerate().take(k) {
        for (p, port) in child.inputs.iter().enumerate() {
            // A trigger left open stays open: the sink runs with the unit it is grouped into
            // (`open_trigger`), and the group doesn't grow a port per sink.
            if !bound.contains(&(ci as u32, p as u32)) && port.name != crate::TRIGGER {
                promote.push(Promote {
                    child: ci as u32,
                    port: p as u32,
                    name: port.name.clone(),
                });
            }
        }
    }

    // The dual of `bound` above: which (child, port) OUTPUTS already feed some inner
    // destination. Tracked by the full pair, not just `from_child` — a child with two
    // outputs, one consumed and one free, must still surface the free one.
    // An effect is still free where what reads it doesn't take it in (`takes_in`): a trigger
    // (`after saved`) only waits for it, and a test that it arrived (`same_value`) tells of it
    // without holding it, so the effect still leaves the boundary and is performed.
    let mut passes = BTreeMap::new();
    let mut consumed: BTreeSet<(u32, u32)> = BTreeSet::new();
    for e in inner_edges {
        let effect = resolved.get(e.from_child as usize).and_then(|u| u.outputs.get(e.from_port as usize)).is_some_and(|p| p.ty == crate::Type::Effect);
        if !effect || takes_in(&resolved, inner_edges, e, &mut passes) {
            consumed.insert((e.from_child, e.from_port));
        }
    }
    // A call tied to a trigger is made when the trigger arrives, whatever reads its answer: once
    // made it can't be taken back (`CALLS`). Its answer leaves the boundary too, so the call is
    // made even when what reads it is a branch not taken. One left untied is made when needed.
    let tied_call = |ci: usize| {
        let child = resolved[ci];
        matches!(&child.body, UnitBody::Primitive { name } if crate::CALLS.contains(&name.as_str()))
            && inner_edges.iter().any(|e| e.to_child as usize == ci && child.inputs.get(e.to_port as usize).is_some_and(|p| p.name == crate::TRIGGER))
    };

    let mut expose = Vec::new();
    for (ci, child) in resolved.iter().enumerate().take(k) {
        for (p, port) in child.outputs.iter().enumerate() {
            if !consumed.contains(&(ci as u32, p as u32)) || tied_call(ci) {
                expose.push(Expose {
                    child: ci as u32,
                    port: p as u32,
                    name: port.name.clone(),
                });
            }
        }
    }

    Ok(EncapSpec { promote, expose })
}

/// Whether edge `e` takes in what its source may hold, an effect: into an input that isn't a
/// trigger, of a child with an output that can hold it (not a number, a Boolean, a text or bytes)
/// and gives it on (`gives_on`). A `guard` read only by a `same_value`, as a `try`'s test of a
/// value under a condition is, doesn't: what the test reads ends there.
fn takes_in(resolved: &[&Unit], edges: &[Edge], e: &Edge, gives: &mut BTreeMap<(u32, u32), bool>) -> bool {
    let plain = |t: &crate::Type| matches!(t, crate::Type::Number | crate::Type::Boolean | crate::Type::Text | crate::Type::Bytes);
    let Some(reader) = resolved.get(e.to_child as usize) else { return false };
    reader.inputs.get(e.to_port as usize).is_some_and(|p| p.name != crate::TRIGGER)
        && (0..reader.outputs.len()).any(|q| !plain(&reader.outputs[q].ty) && gives_on(resolved, edges, e.to_child, q as u32, gives))
}

/// Whether output `port` of `child`, holding an effect, gives it on to where it is performed: it
/// is an effect itself (free, or given on in turn), nothing reads it (it leaves the boundary), or
/// a reader takes it in (`takes_in`).
fn gives_on(resolved: &[&Unit], edges: &[Edge], child: u32, port: u32, gives: &mut BTreeMap<(u32, u32), bool>) -> bool {
    if let Some(&known) = gives.get(&(child, port)) {
        return known;
    }
    // Wiring is acyclic (`validate`); a cycle met before it is checked gives nothing on.
    gives.insert((child, port), false);
    let effect = resolved.get(child as usize).and_then(|u| u.outputs.get(port as usize)).is_some_and(|p| p.ty == crate::Type::Effect);
    let readers: Vec<&Edge> = edges.iter().filter(|e| e.from_child == child && e.from_port == port).collect();
    let on = effect || readers.is_empty() || readers.into_iter().any(|e| takes_in(resolved, edges, e, gives));
    gives.insert((child, port), on);
    on
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
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
        }
    }
    fn e(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }
    fn num(n: f64) -> Value { Value::Number(n) }
    fn port(name: &str) -> Port { Port { name: name.into(), ty: Type::Number } }

    /// A trigger is never promoted (the lowering ties it), and an effect that only a trigger reads
    /// (`after saved`) is still exposed, so the host performs it.
    #[test]
    fn a_trigger_is_not_promoted_and_an_effect_into_one_is_still_exposed() {
        let mut r = MapResolver::new();
        let put = r.insert(Unit { params: vec![crate::Param::frozen("collection", "c")], ..crate::interface_of("collection_put").unwrap() }).unwrap();
        let write = r.insert(Unit { params: vec![crate::Param::frozen("key", "k")], ..crate::interface_of("store_write").unwrap() }).unwrap();
        // put.out → write.trigger
        let edges = vec![e(0, 0, 1, 1)];
        let spec = default_spec(&[put, write], &edges, &r).unwrap();
        let promoted: Vec<&str> = spec.promote.iter().map(|p| p.name.as_str()).collect();
        assert!(!promoted.contains(&crate::TRIGGER), "{promoted:?}");
        let exposed: Vec<(u32, u32)> = spec.expose.iter().map(|x| (x.child, x.port)).collect();
        assert_eq!(exposed, vec![(0, 0), (1, 0)], "both effects leave the boundary");
    }

    /// A call tied to a trigger is made when the trigger arrives, so its answer leaves the
    /// boundary though a branch reads it; one left untied is made only when it is needed.
    #[test]
    fn a_tied_call_is_exposed_though_its_answer_is_read() {
        let mut r = MapResolver::new();
        let go = r.insert(Unit { params: vec![crate::Param::frozen("value", "true"), crate::Param::frozen("type", "Boolean")], ..crate::interface_of("const").unwrap() }).unwrap();
        let call = crate::interface_of("http_request").unwrap();
        let trigger = call.inputs.iter().position(|p| p.name == crate::TRIGGER).unwrap() as u32;
        let call = r.insert(Unit { params: vec![crate::Param::frozen("method", "GET"), crate::Param::frozen("host", "a.example")], ..call }).unwrap();
        let guard = r.insert(crate::interface_of("guard").unwrap()).unwrap();
        // call.response → guard.value, read only when guard's cond is true.
        let read = e(1, 0, 2, 1);
        let exposed = |edges: &[Edge]| default_spec(&[go, call, guard], edges, &r).unwrap().expose.iter().map(|x| (x.child, x.port)).collect::<Vec<_>>();
        assert_eq!(exposed(&[e(0, 0, 1, trigger), read]), vec![(1, 0), (2, 0)], "tied: made when reached");
        assert_eq!(exposed(&[read]), vec![(0, 0), (2, 0)], "untied: made when the branch needs it");
    }

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
            outputs: vec![Port { name: "out".into(), ty: Type::Number }],
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
        assert_eq!(spec.expose, vec![Expose { child: 1, port: 0, name: "out".into() }]);

        let derived = derive_and_freeze(children, inner, &spec, &r).unwrap();
        // (2 + 3) * 4 = 20
        assert_eq!(
            eval(&derived, &[Some(num(2.0)), Some(num(3.0)), Some(num(4.0))], &r).into_vec(),
            vec![Some(num(20.0))]
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
        assert_eq!(spec.expose, vec![Expose { child: 1, port: 0, name: "out".into() }]);
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
            expose: vec![Expose { child: 0, port: 0, name: "out".into() }],
        };
        let spec_ba = EncapSpec {
            promote: vec![
                Promote { child: 0, port: 1, name: "b".into() },
                Promote { child: 0, port: 0, name: "a".into() },
            ],
            expose: vec![Expose { child: 0, port: 0, name: "out".into() }],
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
            expose: vec![Expose { child: 1, port: 0, name: "out".into() }],
        };
        let err = derive_and_freeze(vec![hadd, hmul], vec![e(0, 0, 1, 0)], &spec, &r).unwrap_err();
        assert_eq!(err, EncapError::PromoteAlreadyBound { child: 1, port: 0 });
    }

    #[test]
    fn rejects_invalid_expose() {
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let spec = EncapSpec {
            promote: vec![],
            expose: vec![Expose { child: 0, port: 3, name: "out".into() }], // port 3 invalid
        };
        let err = derive_and_freeze(vec![hadd], vec![], &spec, &r).unwrap_err();
        assert_eq!(err, EncapError::ExposePortInvalid { child: 0, port: 3 });
    }

    #[test]
    fn default_spec_exposes_every_free_output() {
        // Two independent (unconsumed) children ⇒ both are now exposed automatically — no
        // error, no ambiguity. Symmetric with how the input side already promotes every
        // unbound input with zero ambiguity check.
        let mut r = MapResolver::new();
        let hadd1 = r.insert(binop("add", "a", "b")).unwrap();
        let hadd2 = r.insert(binop("add", "a", "b")).unwrap();
        let spec = default_spec(&[hadd1, hadd2], &[], &r).unwrap();
        let exposed: Vec<(u32, u32, &str)> =
            spec.expose.iter().map(|x| (x.child, x.port, x.name.as_str())).collect();
        assert_eq!(exposed, vec![(0, 0, "out"), (1, 0, "out")]);
    }

    #[test]
    fn default_spec_with_no_free_output_exposes_nothing() {
        // A child with no output at all ⇒ expose: vec![] — a valid, ordinary proposal
        // (a composite whose effects are discarded, an honest fact per eval's own P4.7
        // design, not an error to reject).
        let mut r = MapResolver::new();
        let no_output = Unit {
            body: UnitBody::Primitive { name: "store_write".into() },
            params: vec![],
            inputs: vec![Port { name: "value".into(), ty: Type::Any }],
            outputs: vec![],
        };
        let h = r.insert(no_output).unwrap();
        let spec = default_spec(&[h], &[], &r).unwrap();
        assert_eq!(spec.expose, vec![]);
    }

    #[test]
    fn default_spec_tracks_free_outputs_per_port_not_per_child() {
        // A child with TWO outputs, one consumed internally (feeds another child) and one
        // free, proves the exposure scan tracks (child, port), not just child — the exact
        // bug the old per-child `consumed: BTreeSet<u32>` would have hit the moment a
        // multi-output child existed. This generalization fixes it as a consequence of
        // the correct symmetric loop, not as a separate patch.
        let mut r = MapResolver::new();
        let dual = Unit {
            body: UnitBody::Primitive { name: "dual".into() },
            params: vec![],
            inputs: vec![],
            outputs: vec![
                Port { name: "x".into(), ty: Type::Number },
                Port { name: "y".into(), ty: Type::Number },
            ],
        };
        let hdual = r.insert(dual).unwrap();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let children = vec![hdual, hadd];
        let inner = vec![e(0, 0, 1, 0)]; // dual.x (port 0) → add.a; dual.y (port 1) stays free

        let spec = default_spec(&children, &inner, &r).unwrap();
        let exposed: Vec<(u32, u32, &str)> =
            spec.expose.iter().map(|x| (x.child, x.port, x.name.as_str())).collect();
        // dual.y is free and exposed despite dual.x being consumed on the SAME child;
        // add's own output is free too.
        assert_eq!(exposed, vec![(0, 1, "y"), (1, 0, "out")]);
    }

    /// A derived unit that fails validation says why, each error by name.
    #[test]
    fn an_invalid_derived_unit_names_its_errors() {
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let hlen = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "text_length".into() },
                params: vec![],
                inputs: vec![Port { name: "text".into(), ty: Type::Text }],
                outputs: vec![port("length")],
            })
            .unwrap();
        let spec = default_spec(&[hadd, hlen], &[e(0, 0, 1, 0)], &r).unwrap();
        let err = derive_and_freeze(vec![hadd, hlen], vec![e(0, 0, 1, 0)], &spec, &r).unwrap_err();
        assert_eq!(err.to_string(), "the derived unit failed validation: edge 0 connects incompatible types: Number does not fit Text");
    }

    #[test]
    fn derived_unit_validates() {
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let spec = default_spec(&[hadd], &[], &r).unwrap();
        let derived = derive_and_freeze(vec![hadd], vec![], &spec, &r).unwrap();
        assert!(validate(&derived, &r).is_ok());
    }

    #[test]
    fn multi_output_composite_exposes_several_independently() {
        // add(a,b) and divide(a,b) as two independent children — neither wired to the
        // other — both exposed as separate composite outputs via a hand-built EncapSpec.
        // The actual point of this inch: `default_spec` still only ever proposes one
        // (deliberately unchanged, see its own doc comment), but `derive_and_freeze`
        // itself now accepts any number of exposed outputs — proven end to end here
        // (derived types, boundary wiring, eval), not just type-checked. Mirrors eval.rs's
        // own `multi_output_partial_success`, but through this real, public API surface,
        // not eval's internal one.
        let mut r = MapResolver::new();
        let hadd = r.insert(binop("add", "a", "b")).unwrap();
        let hdiv = r.insert(binop("divide", "a", "b")).unwrap();
        let children = vec![hadd, hdiv];

        let spec = EncapSpec {
            promote: vec![
                Promote { child: 0, port: 0, name: "add_a".into() },
                Promote { child: 0, port: 1, name: "add_b".into() },
                Promote { child: 1, port: 0, name: "div_a".into() },
                Promote { child: 1, port: 1, name: "div_b".into() },
            ],
            expose: vec![
                Expose { child: 0, port: 0, name: "sum".into() },
                Expose { child: 1, port: 0, name: "quotient".into() },
            ],
        };
        let derived = derive_and_freeze(children, vec![], &spec, &r).unwrap();

        // Interface derived correctly: two named, typed output ports, in expose order.
        assert_eq!(
            derived.outputs,
            vec![
                Port { name: "sum".into(), ty: Type::Number },
                Port { name: "quotient".into(), ty: Type::Number },
            ]
        );

        // Honest success: 3+4=7, 12/4=3 — both slots present.
        assert_eq!(
            eval(&derived, &[Some(num(3.0)), Some(num(4.0)), Some(num(12.0)), Some(num(4.0))], &r).into_vec(),
            vec![Some(num(7.0)), Some(num(3.0))]
        );

        // Partial success: divide-by-zero fails ONLY the quotient slot; sum is
        // unaffected — the whole point of eval's per-slot EvalOutputs over Option<Vec<Value>>
        // (P4.7's original foundation), now proven through a unit this module actually
        // built, not just eval's own internal fixtures.
        assert_eq!(
            eval(&derived, &[Some(num(3.0)), Some(num(4.0)), Some(num(12.0)), Some(num(0.0))], &r).into_vec(),
            vec![Some(num(7.0)), None]
        );
    }
}