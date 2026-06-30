//! Advisory signals (P3.6.1) — derived, **advisory-only** facts a consumer reads before
//! depending on a unit. Not gates: nothing here forbids anything. The consumer decides what to
//! do with "this touches the network" or "this pulls in 47 transitive dependencies". Pure
//! exposure — every signal is already computable from pieces that exist; this just bundles them
//! into one consumer-facing summary.
//!
//! - **capabilities** — the saturated capability set (I8: derived from the closure, never
//!   asserted). The honest answer to "what can this touch?"
//! - **is_pure** — ∅ capabilities ⇒ a pure function of its inputs (cacheable/memoizable).
//! - **is_replayable** — no State capability ⇒ replayable (its drivers aside, deterministic).
//! - **dependency_count** — the transitive closure size: how much depending on this pulls in.

use crate::{capabilities, dependencies, Capabilities, Resolver, Unit};

/// A bundle of derived signals about a unit and its whole closure. Advisory only.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Advisory {
    /// Everything the unit can touch (saturated, fail-closed on unknown leaves).
    pub capabilities: Capabilities,
    /// ∅ capabilities ⇒ pure (a function of its inputs alone).
    pub is_pure: bool,
    /// No State capability ⇒ replayable.
    pub is_replayable: bool,
    /// Transitive closure size — the supply-chain weight a consumer takes on.
    pub dependency_count: usize,
}

/// Derive the advisory signals for `unit` over `resolver`. Pure: reads the closure, asserts
/// nothing, forbids nothing.
pub fn advise(unit: &Unit, resolver: &impl Resolver) -> Advisory {
    let caps = capabilities(unit, resolver);
    Advisory {
        is_pure: caps.is_pure(),
        is_replayable: caps.is_replayable(),
        capabilities: caps,
        dependency_count: dependencies(unit, resolver).len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capability, Edge, MapResolver, Type, UnitBody, BOUNDARY};

    fn prim(name: &str, out: Type) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: vec![],
            output: Some(out),
        }
    }

    #[test]
    fn a_pure_leaf_advises_pure_and_dependency_free() {
        let r = MapResolver::new();
        let a = advise(&prim("add", Type::Number), &r);
        assert!(a.is_pure && a.is_replayable);
        assert!(a.capabilities.is_pure()); // ∅
        assert_eq!(a.dependency_count, 0);
    }

    #[test]
    fn a_sink_advises_impure_with_its_capability() {
        let r = MapResolver::new();
        let a = advise(&prim("egress", Type::Effect), &r);
        assert!(!a.is_pure);
        assert!(a.capabilities.contains(Capability::NetworkEgress));
    }

    #[test]
    fn a_driver_is_impure_but_replayable() {
        let r = MapResolver::new();
        let a = advise(&prim("clock", Type::Number), &r);
        assert!(!a.is_pure); // it has a capability (Clock)
        assert!(a.is_replayable); // ...but it is a driver, not State
    }

    #[test]
    fn a_state_unit_is_not_replayable() {
        let r = MapResolver::new();
        let a = advise(&prim("store_write", Type::Effect), &r);
        assert!(!a.is_replayable); // a State capability ⇒ not replayable
    }

    #[test]
    fn dependency_count_is_the_closure_size() {
        let mut r = MapResolver::new();
        let c = r.insert(prim("add", Type::Number)).unwrap();
        let root = Unit {
            body: UnitBody::Composite {
                children: vec![c],
                edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![],
            output: Some(Type::Number),
        };
        assert_eq!(advise(&root, &r).dependency_count, 1); // one transitive child
    }
}