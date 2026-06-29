//! # Design-mode simulation — running a service with its effects mocked (P2.4.1 / P2.7.1)
//!
//! An effectful leaf (`timer`, `email`, …) has no pure `compute`, so the pure
//! evaluator cannot run a graph that contains one. **Design mode** makes such a
//! graph evaluable by substituting each effect leaf with a *pure mock* (P2.7.1) at
//! resolve time, then running ordinary core [`eval`]. The result is pure,
//! deterministic, replayable, and memoizable like any pure evaluation.
//!
//! The substitution rides the **resolver**: `eval` resolves every child by hash
//! before running it, so a [`Simulated`] resolver returns a leaf's mock instead of
//! the effect primitive — at any depth, with no `eval` change and no graph surgery.
//! Core `eval` stays mode-ignorant; the resolver is the only thing that differs.
//! (Deploy mode — sourcing real driver events and firing real sinks — is the
//! `holon-host` shell, a separate and deferred path.)
//!
//! ## The guardrail (enforced structurally)
//!
//! This module *depends on* [`crate::kind`] (to know which leaves are effects); the
//! classifiers never depend on it. So `capabilities` / `effects_of` are computed
//! from the **real** units and never see a mock — simulating innocent buys nothing
//! at the trust boundary, and a mock is never in any content hash.

use crate::eval::eval;
use crate::hash::parse_hash;
use crate::{kind_of, Edge, Hash, Kind, Param, Resolver, Type, Unit, UnitBody, Value, BOUNDARY};
use std::collections::BTreeMap;

/// A pure stand-in for an effect leaf, used **only** in design-mode evaluation.
///
/// - a [`Driver`](Kind::Driver) mocks to a *constant* of its output type — the
///   scripted stimulus a design run sees (custom stimuli are a deferred P2.7
///   refinement; the default is a type-zero value);
/// - a [`Sink`](Kind::Sink) mocks to a *passthrough* that echoes its input — absorb
///   the value, return a plausible result;
/// - a [`State`](Kind::State) cell has **no** pure mock: its value depends on prior
///   writes, so it is host-threaded (P2.6.1) ⇒ `None`;
/// - a [`Pure`](Kind::Pure) or unknown leaf is not an effect ⇒ `None` (nothing to
///   substitute; it already evaluates).
pub fn mock_of(unit: &Unit) -> Option<Unit> {
    let name = match &unit.body {
        UnitBody::Primitive { name } => name,
        UnitBody::Composite { .. } => return None,
    };
    match kind_of(name)? {
        Kind::Driver => driver_mock(unit),
        Kind::Sink => sink_mock(unit),
        Kind::State | Kind::Pure => None,
    }
}

/// A driver's mock: a pure constant of its output type.
fn driver_mock(unit: &Unit) -> Option<Unit> {
    let (cname, value): (&str, &str) = match unit.output.as_ref()? {
        Type::Number => ("number", "0"),
        Type::Boolean => ("boolean", "false"),
        Type::Text | Type::Any => ("text", ""),
        // An empty-list constant: `list` with no inputs computes to `[]`.
        Type::List => return Some(constant("list", vec![], unit.output.clone())),
    };
    Some(constant(cname, vec![Param::frozen("value", value)], unit.output.clone()))
}

fn constant(name: &str, params: Vec<Param>, output: Option<Type>) -> Unit {
    Unit { body: UnitBody::Primitive { name: name.to_string() }, params, inputs: vec![], output }
}

/// A sink's mock: a passthrough composite that echoes its first input to its output
/// (absorb → plausible result). Requires an input and an output to wire through.
fn sink_mock(unit: &Unit) -> Option<Unit> {
    if unit.inputs.is_empty() || unit.output.is_none() {
        return None;
    }
    Some(Unit {
        body: UnitBody::Composite {
            children: vec![],
            edges: vec![Edge { from_child: BOUNDARY, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
        },
        params: vec![],
        inputs: unit.inputs.clone(),
        output: unit.output.clone(),
    })
}

/// A design-mode resolver: wraps the real resolver and, for every effect leaf in a
/// root's closure, returns that leaf's pure [`mock_of`] instead. The mocks are
/// **owned** here (the [`Resolver`] API hands out `&Unit`), keyed by the real leaf's
/// hash, built by one closure pre-pass at construction.
pub struct Simulated<'a, R: Resolver> {
    real: &'a R,
    mocks: BTreeMap<Hash, Unit>,
}

impl<'a, R: Resolver> Simulated<'a, R> {
    /// Build a design-mode resolver over `root`'s closure.
    pub fn new(root: &Unit, real: &'a R) -> Self {
        let mut mocks = BTreeMap::new();
        collect_mocks(root, real, &mut mocks);
        Simulated { real, mocks }
    }
}

impl<R: Resolver> Resolver for Simulated<'_, R> {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        match self.mocks.get(hash) {
            Some(mock) => Some(mock),
            None => self.real.resolve(hash),
        }
    }
}

/// Walk a root's closure (children ∪ hash-valued params) and pre-build a mock for
/// every effect leaf, keyed by the leaf's hash. Termination is free: content-
/// addressing forbids a cycle.
fn collect_mocks(unit: &Unit, real: &impl Resolver, mocks: &mut BTreeMap<Hash, Unit>) {
    if let UnitBody::Composite { children, .. } = &unit.body {
        for child_hash in children {
            if let Some(child) = real.resolve(child_hash) {
                if let Some(mock) = mock_of(child) {
                    mocks.insert(*child_hash, mock);
                }
                collect_mocks(child, real, mocks);
            }
        }
    }
    for p in &unit.params {
        if let Some(v) = p.value() {
            if let Some(h) = parse_hash(v) {
                if let Some(referenced) = real.resolve(&h) {
                    if let Some(mock) = mock_of(referenced) {
                        mocks.insert(h, mock);
                    }
                    collect_mocks(referenced, real, mocks);
                }
            }
        }
    }
}

/// Evaluate a unit in **design mode**: effect leaves are substituted by their pure
/// mocks and the whole closure runs under core [`eval`] — pure, deterministic,
/// replayable, and memoizable. The "firing loop" in design mode is just `eval` over
/// the substituting resolver.
pub fn design_eval(root: &Unit, inputs: &[Option<Value>], real: &impl Resolver) -> Option<Value> {
    let sim = Simulated::new(root, real);
    match mock_of(root) {
        Some(mock) => eval(&mock, inputs, &sim),
        None => eval(root, inputs, &sim),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capabilities, hash_unit, Capability, MapResolver, Port};

    fn timer() -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "timer".into() },
            params: vec![],
            inputs: vec![],
            output: Some(Type::Number),
        }
    }
    fn email() -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "email".into() },
            params: vec![],
            inputs: vec![Port { name: "msg".into(), ty: Type::Text }],
            output: Some(Type::Text),
        }
    }
    fn prim(name: &str, output: Option<Type>) -> Unit {
        Unit { body: UnitBody::Primitive { name: name.into() }, params: vec![], inputs: vec![], output }
    }
    fn pass_through(child: Hash, out: Type) -> Unit {
        Unit {
            body: UnitBody::Composite {
                children: vec![child],
                edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![],
            output: Some(out),
        }
    }

    #[test]
    fn driver_leaf_mocks_to_constant() {
        // A bare timer (Driver) is not evaluable; design mode substitutes a constant.
        let store = MapResolver::new();
        let t = timer();
        assert_eq!(eval(&t, &[], &store), None);
        assert_eq!(design_eval(&t, &[], &store), Some(Value::Number(0.0)));
    }

    #[test]
    fn effect_leaf_makes_graph_evaluable_in_design() {
        let mut store = MapResolver::new();
        let th = store.insert(timer()).unwrap();
        let comp = pass_through(th, Type::Number);
        assert_eq!(eval(&comp, &[], &store), None); // timer not computable
        assert_eq!(design_eval(&comp, &[], &store), Some(Value::Number(0.0)));
        // deterministic
        assert_eq!(design_eval(&comp, &[], &store), design_eval(&comp, &[], &store));
    }

    #[test]
    fn effect_leaf_at_depth_is_substituted() {
        let mut store = MapResolver::new();
        let th = store.insert(timer()).unwrap();
        let inner = store.insert(pass_through(th, Type::Number)).unwrap();
        let mid = pass_through(inner, Type::Number);
        assert_eq!(design_eval(&mid, &[], &store), Some(Value::Number(0.0))); // depth-agnostic
    }

    #[test]
    fn sink_mock_echoes_settled_input() {
        // A sink (email) mocks to a passthrough: it absorbs its input and returns it,
        // seeing the settled value exactly as eval's single pass delivers it.
        let mut store = MapResolver::new();
        let eh = store.insert(email()).unwrap();
        let comp = Unit {
            body: UnitBody::Composite {
                children: vec![eh],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 }, // input → email
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // email → output
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "msg".into(), ty: Type::Text }],
            output: Some(Type::Text),
        };
        let hi = Value::Text("hi".into());
        assert_eq!(eval(&comp, &[Some(hi.clone())], &store), None); // email not computable
        assert_eq!(design_eval(&comp, &[Some(hi.clone())], &store), Some(hi));
    }

    #[test]
    fn classification_ignores_mocks() {
        // The guardrail: capabilities() reads the REAL resolver, so a Driver leaf still
        // surfaces Clock even though design mode would mock it away — and the mock is a
        // different unit than the leaf, so it is never part of the graph's identity.
        let mut store = MapResolver::new();
        let th = store.insert(timer()).unwrap();
        let comp = pass_through(th, Type::Number);

        let caps = capabilities(&comp, &store);
        assert!(caps.contains(Capability::Clock)); // real classification, unmocked

        let mock = mock_of(&timer()).unwrap();
        assert_ne!(hash_unit(&mock).unwrap(), hash_unit(&timer()).unwrap());
    }

    #[test]
    fn pure_leaf_has_no_mock() {
        assert_eq!(mock_of(&prim("add", Some(Type::Number))), None);
    }

    #[test]
    fn state_leaf_has_no_pure_mock() {
        // State is the exception — host-threaded, no standalone pure mock.
        assert_eq!(mock_of(&prim("store", Some(Type::Number))), None);
    }
}