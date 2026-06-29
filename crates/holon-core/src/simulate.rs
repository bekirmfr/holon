//! # Design-mode simulation — running a service with its *drivers* mocked (P2.4.1 / P2.7.1)
//!
//! A **driver** leaf (`timer`, an inbound request) has no pure `compute` — it is
//! external input the host supplies. **Design mode** lets a driver-bearing graph run
//! without a host by substituting each driver with a *pure mock* (a scripted stimulus)
//! at resolve time, then running ordinary core [`eval`]. The result is pure,
//! deterministic, replayable, and memoizable like any pure evaluation.
//!
//! Sinks need no mock: under Fork B a sink purely *constructs its request payload*, so
//! `eval` runs it directly (the host performs the request later, at deploy). State is
//! the other exception — host-threaded, not mockable. So **only drivers are
//! substituted here.**
//!
//! The substitution rides the **resolver**: `eval` resolves every child by hash
//! before running it, so a [`Simulated`] resolver returns a driver's mock instead of
//! the driver primitive — at any depth, with no `eval` change and no graph surgery.
//! Core `eval` stays mode-ignorant; the resolver is the only thing that differs.
//!
//! ## The guardrail (enforced structurally)
//!
//! This module *depends on* [`crate::kind`] (to know which leaves are drivers); the
//! classifiers never depend on it. So `capabilities` / `effects_of` are computed
//! from the **real** units and never see a mock — simulating innocent buys nothing
//! at the trust boundary, and a mock is never in any content hash.

use crate::eval::eval;
use crate::hash::parse_hash;
use crate::{capabilities_of, Hash, Param, Resolver, Type, Unit, UnitBody, Value};
use std::collections::BTreeMap;

/// A pure stand-in for a **sourced** (input-side) leaf, used **only** in design-mode
/// evaluation — a constant of the leaf's output type (a scripted stimulus; custom
/// stimuli are a deferred P2.7 refinement, the default is a type-zero value).
///
/// Mock the *sourced* side — input the host would otherwise supply: drivers (`Clock`,
/// inbound request, file/secret/random read) and `store_read` (`StateRead`). Everything
/// else returns `None`:
/// - a sink / `store_write` purely constructs its request (Fork B) ⇒ `eval` runs it;
/// - a pure or unknown leaf already evaluates (or has no manifest).
pub fn mock_of(unit: &Unit) -> Option<Unit> {
    let name = match &unit.body {
        UnitBody::Primitive { name } => name,
        UnitBody::Composite { .. } => return None,
    };
    if capabilities_of(name)?.iter().any(|c| c.is_sourced()) {
        sourced_mock(unit)
    } else {
        None
    }
}

/// A sourced leaf's mock: a pure constant of its output type.
fn sourced_mock(unit: &Unit) -> Option<Unit> {
    let (cname, value): (&str, &str) = match unit.output.as_ref()? {
        Type::Number => ("number", "0"),
        Type::Boolean => ("boolean", "false"),
        Type::Text | Type::Any => ("text", ""),
        // An empty-list constant: `list` with no inputs computes to `[]`.
        Type::List => return Some(constant("list", vec![], unit.output.clone())),
        // A sourced leaf never outputs an Effect (effects are constructed by sinks, the
        // performed side, not the sourced side) — so there is nothing to mock.
        Type::Effect => return None,
    };
    Some(constant(cname, vec![Param::frozen("value", value)], unit.output.clone()))
}

fn constant(name: &str, params: Vec<Param>, output: Option<Type>) -> Unit {
    Unit { body: UnitBody::Primitive { name: name.to_string() }, params, inputs: vec![], output }
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
    use crate::{capabilities, hash_unit, Capability, Edge, MapResolver, Port, BOUNDARY};

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
            output: Some(Type::Effect),
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
    /// input → email → output. Under Fork B `email` is pure (identity on payload), so
    /// this whole graph is eval-pure.
    fn sink_graph(email_hash: Hash) -> Unit {
        Unit {
            body: UnitBody::Composite {
                children: vec![email_hash],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                ],
            },
            params: vec![],
            inputs: vec![Port { name: "msg".into(), ty: Type::Text }],
            output: Some(Type::Text),
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
    fn sink_is_eval_pure() {
        // Fork B: a sink purely *constructs its request* (representation B), so a sink-bearing
        // graph evaluates under the REAL resolver — no design mode, no mock. The settled output
        // is a self-describing `Effect`, not the bare payload.
        let mut store = MapResolver::new();
        let eh = store.insert(email()).unwrap();
        let comp = sink_graph(eh);
        let hi = Value::Text("hi".into());
        assert_eq!(
            eval(&comp, &[Some(hi.clone())], &store),
            Some(Value::Effect { sink: "email".into(), payload: Box::new(hi) })
        );
    }

    #[test]
    fn sink_graph_is_effect_bearing_and_replayable() {
        // The combination Fork A could not express: bears an egress AND is replayable.
        let mut store = MapResolver::new();
        let eh = store.insert(email()).unwrap();
        let comp = sink_graph(eh);
        let caps = capabilities(&comp, &store);
        assert!(caps.contains(Capability::NetworkEgress)); // effect-bearing (you see the send)
        assert!(caps.is_replayable()); // no state ⇒ cacheable / shareable
    }

    #[test]
    fn state_graph_is_not_replayable() {
        // The one place purity ends: a state cell forfeits replay / caching.
        let mut store = MapResolver::new();
        let sh = store.insert(prim("store_write", Some(Type::Number))).unwrap();
        let comp = pass_through(sh, Type::Number);
        assert!(!capabilities(&comp, &store).is_replayable());
    }

    #[test]
    fn driver_still_needs_a_mock() {
        // Design mode substitutes only drivers; sinks are pure, state is host-threaded.
        assert!(mock_of(&timer()).is_some()); // driver — external input, mocked
        assert!(mock_of(&email()).is_none()); // sink — pure request, no mock
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
    fn store_read_is_mockable() {
        // StateRead is sourced (the host supplies it) ⇒ design mode scripts a mock.
        assert!(mock_of(&prim("store_read", Some(Type::Number))).is_some());
    }

    #[test]
    fn store_write_has_no_mock() {
        // StateWrite is performed (output-carried, a Fork-B sink) ⇒ eval runs it; no mock.
        assert_eq!(mock_of(&prim("store_write", Some(Type::Number))), None);
    }

    #[test]
    fn state_read_graph_design_evals() {
        // A read-bearing graph runs in design mode: store_read is sourced, so it mocks to
        // a scripted value and the graph evaluates purely (just like a driver).
        let mut store = MapResolver::new();
        let rh = store.insert(prim("store_read", Some(Type::Number))).unwrap();
        let comp = pass_through(rh, Type::Number);
        assert_eq!(eval(&comp, &[], &store), None); // store_read not computable (sourced)
        assert_eq!(design_eval(&comp, &[], &store), Some(Value::Number(0.0))); // scripted
    }
}