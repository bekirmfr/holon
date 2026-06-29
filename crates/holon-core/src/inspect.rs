//! Structural inspection (P3.2.1) — the I9 promise (full transitive inspectability) made
//! into an operation.
//!
//! Two functions over a unit's *reference shape*:
//!
//! - [`child_refs`] — the **single source of truth** for "what hashes does this unit
//!   *directly* reference": a composite's children (I3) and its hash-valued params (the `fn`
//!   of `map`/`fold`, I6). Every closure walk in the system — capability classification
//!   ([`crate::capabilities`]), design-mode mocking ([`crate::Simulated`]), and registry
//!   publish (`holon-host`) — routes through this one definition, so they can never disagree
//!   about what counts as a reference.
//!
//! - [`dependencies`] — the transitive closure *below* a root: every unit it reaches, so a
//!   consumer can audit exactly what they are about to depend on **before** pinning it.
//!
//! ## Why a hash-valued param is identified by *name*, not by parsing
//! A reference riding a param is the one named `fn` — the convention [`crate::eval`] uses to
//! find a combiner — **not** "any value that parses as a hash". A literal `Text` value can
//! coincidentally be 64 hex characters; reading it as a reference would be wrong in three
//! different ways (mocking a string, ascribing it phantom capabilities, or rejecting a valid
//! unit at publish). Keying on the name aligns inspection with how the evaluator actually
//! resolves references, and removes that whole false-positive class.

use crate::hash::parse_hash;
use crate::{Hash, Resolver, Unit, UnitBody};
use std::collections::BTreeMap;

/// The hashes a unit **directly** references: a composite's children, plus the value of any
/// hash-valued param (by convention, the one named `fn`). A primitive with no `fn` param
/// references nothing. This is the one definition the whole system shares.
pub fn child_refs(unit: &Unit) -> Vec<Hash> {
    let mut refs = Vec::new();
    if let UnitBody::Composite { children, .. } = &unit.body {
        refs.extend_from_slice(children);
    }
    for p in &unit.params {
        // The `fn` param is a reference; every other param is data — even one whose value
        // happens to look like a hash.
        if p.name() == "fn" {
            if let Some(h) = p.value().and_then(parse_hash) {
                refs.push(h);
            }
        }
    }
    refs
}

/// The transitive closure **below** `root`: every unit reachable through [`child_refs`],
/// keyed by hash (deduplicated — a DAG can reach the same unit by many paths) in deterministic
/// hash order. A reference that does not resolve is simply not expanded; completeness is the
/// registry's guarantee (a published unit's closure is whole by construction, P3.1.1), so
/// inspecting a published unit always yields its full structure. `root` itself is not
/// included — these are its dependencies. Terminates for free: content-addressing forbids a
/// cycle.
pub fn dependencies<'r, R: Resolver>(root: &Unit, resolver: &'r R) -> BTreeMap<Hash, &'r Unit> {
    let mut acc = BTreeMap::new();
    collect(root, resolver, &mut acc);
    acc
}

fn collect<'r, R: Resolver>(unit: &Unit, resolver: &'r R, acc: &mut BTreeMap<Hash, &'r Unit>) {
    for h in child_refs(unit) {
        if acc.contains_key(&h) {
            continue; // already reached by another path
        }
        if let Some(u) = resolver.resolve(&h) {
            acc.insert(h, u);
            collect(u, resolver, acc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edge, MapResolver, Param, Port, Type, BOUNDARY};

    fn prim(name: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params: vec![],
            inputs: vec![],
            output: Some(Type::Number),
        }
    }
    fn composite(children: Vec<Hash>) -> Unit {
        Unit {
            body: UnitBody::Composite {
                children,
                edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
            },
            params: vec![],
            inputs: vec![],
            output: Some(Type::Number),
        }
    }
    fn hex(h: &Hash) -> String {
        h.iter().map(|b| format!("{:02x}", b)).collect()
    }
    fn map_over(fn_hash: &Hash) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", hex(fn_hash))],
            inputs: vec![Port { name: "list".into(), ty: Type::List }],
            output: Some(Type::List),
        }
    }

    #[test]
    fn child_refs_of_primitive_is_empty() {
        assert!(child_refs(&prim("add")).is_empty());
    }

    #[test]
    fn child_refs_of_composite_are_its_children() {
        let mut r = MapResolver::new();
        let a = r.insert(prim("add")).unwrap();
        let b = r.insert(prim("multiply")).unwrap();
        assert_eq!(child_refs(&composite(vec![a, b])), vec![a, b]);
    }

    #[test]
    fn child_refs_follows_the_fn_param() {
        let mut r = MapResolver::new();
        let f = r.insert(prim("add")).unwrap();
        assert_eq!(child_refs(&map_over(&f)), vec![f]);
    }

    #[test]
    fn child_refs_ignores_a_hash_shaped_non_fn_param() {
        // "aaaa…" (64 chars) is valid hex, so it *parses* as a hash — but it rides a `value`
        // param, which is data, so it is correctly NOT a reference. (The false-positive the
        // name convention exists to prevent.)
        let c = Unit {
            body: UnitBody::Primitive { name: "const".into() },
            params: vec![Param::frozen("type", "Text"), Param::frozen("value", &"a".repeat(64))],
            inputs: vec![],
            output: Some(Type::Text),
        };
        assert!(child_refs(&c).is_empty());
    }

    #[test]
    fn dependencies_are_transitive() {
        // grandchild ← child ← root ⇒ dependencies(root) = {child, grandchild}
        let mut r = MapResolver::new();
        let gc = r.insert(prim("add")).unwrap();
        let child = r.insert(composite(vec![gc])).unwrap();
        let root = composite(vec![child]);
        let deps = dependencies(&root, &r);
        assert_eq!(deps.len(), 2);
        assert!(deps.contains_key(&child) && deps.contains_key(&gc));
    }

    #[test]
    fn dependencies_follow_fn_and_dedup() {
        // root holds [map(f), f]: f is reachable via the map's `fn` AND as a direct child —
        // it must appear exactly once.
        let mut r = MapResolver::new();
        let f = r.insert(prim("add")).unwrap();
        let m = r.insert(map_over(&f)).unwrap();
        let root = composite(vec![m, f]);
        let deps = dependencies(&root, &r);
        assert_eq!(deps.len(), 2); // m and f, f deduped
        assert!(deps.contains_key(&m) && deps.contains_key(&f));
    }

    #[test]
    fn dependencies_of_a_leaf_are_empty() {
        let r = MapResolver::new();
        assert!(dependencies(&prim("add"), &r).is_empty());
    }
}