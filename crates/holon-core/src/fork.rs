//! Fork / re-pin (P3.5.1) — the one operation that deliberately creates a *new* identity.
//!
//! Everything else preserves identity (a hash is fixed and trusted); fork derives. To change a
//! child is to pin a *different* hash there, which re-hashes the parent, which re-hashes its
//! parents — up to the root. [`refork`] performs that re-pin with **path-copying**: only the
//! ancestors of the changed node are re-minted; every subtree off that path keeps its hash and
//! is shared by reference (the content-addressing payoff — a one-node change costs one spine,
//! not a deep copy).
//!
//! The transform is **pure**: given a resolver it mints new units and returns them; storing
//! them (and re-publishing) is the host's job. Re-pinning is uniform with [`crate::child_refs`]
//! — the same references inspection *reads*, fork *rewrites*: a child is a hash swap; a `fn`
//! param becomes `Param::frozen("fn", to_hex(replacement))`, the inverse of the `parse_hash`
//! eval uses to read it.

use crate::hash::{hash_unit, parse_hash, to_hex};
use crate::{Hash, Param, Resolver, Unit, UnitBody};
use std::collections::BTreeMap;

/// The result of a fork: the new root's hash, and the re-pinned spine (the units the host must
/// store). Off-path subtrees are *not* here — they are shared by hash with the original.
pub struct Refork {
    /// The new root's hash (a key within `minted`).
    pub new_root: Hash,
    /// The newly minted ancestors. Keyed by hash; includes the new root.
    pub minted: BTreeMap<Hash, Unit>,
}

/// Re-pin every reference to `target` → `replacement` in the DAG below `root`, path-copying the
/// affected ancestors. `None` if `target` is not referenced anywhere in `root`'s closure
/// (nothing to fork). The `replacement` unit's own storage is the caller's concern — `refork`
/// only rewrites references to it.
pub fn refork(
    root: &Unit,
    target: &Hash,
    replacement: &Hash,
    resolver: &impl Resolver,
) -> Option<Refork> {
    let mut minted = BTreeMap::new();
    let new_root = refork_node(root, target, replacement, resolver, &mut minted)?;
    Some(Refork { new_root, minted })
}

/// Returns `Some(new_hash)` if this subtree referenced `target` (and records the re-minted
/// units in `minted`); `None` if the subtree was untouched (and is therefore shared as-is).
fn refork_node(
    unit: &Unit,
    target: &Hash,
    replacement: &Hash,
    resolver: &impl Resolver,
    minted: &mut BTreeMap<Hash, Unit>,
) -> Option<Hash> {
    let mut changed = false;

    // (1) Composite children.
    let new_body = match &unit.body {
        UnitBody::Composite { children, edges } => {
            let mut new_children = Vec::with_capacity(children.len());
            for ch in children {
                if ch == target {
                    changed = true;
                    new_children.push(*replacement);
                } else if let Some(u) = resolver.resolve(ch) {
                    match refork_node(u, target, replacement, resolver, minted) {
                        Some(h2) => {
                            changed = true;
                            new_children.push(h2);
                        }
                        None => new_children.push(*ch), // subtree unchanged → shared
                    }
                } else {
                    new_children.push(*ch); // unresolvable → treat as unchanged (see L4 F6)
                }
            }
            UnitBody::Composite { children: new_children, edges: edges.clone() }
        }
        UnitBody::Primitive { name } => UnitBody::Primitive { name: name.clone() },
    };

    // (2) The `fn` param (a hash-valued reference).
    let mut new_params = Vec::with_capacity(unit.params.len());
    for p in &unit.params {
        let repinned = if p.name() == "fn" {
            p.value().and_then(parse_hash).and_then(|h| {
                if &h == target {
                    Some(Param::frozen("fn", to_hex(replacement)))
                } else if let Some(u) = resolver.resolve(&h) {
                    refork_node(u, target, replacement, resolver, minted)
                        .map(|h2| Param::frozen("fn", to_hex(&h2)))
                } else {
                    None
                }
            })
        } else {
            None
        };
        match repinned {
            Some(p2) => {
                changed = true;
                new_params.push(p2);
            }
            None => new_params.push(p.clone()),
        }
    }

    if !changed {
        return None; // target not in this subtree — share it untouched
    }
    let new_unit = Unit {
        body: new_body,
        params: new_params,
        inputs: unit.inputs.clone(),
        output: unit.output.clone(),
    };
    let h = hash_unit(&new_unit).ok()?;
    minted.insert(h, new_unit);
    Some(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edge, MapResolver, Port, Type, BOUNDARY};

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
    fn map_over(fn_hash: &Hash) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", to_hex(fn_hash))],
            inputs: vec![Port { name: "list".into(), ty: Type::List }],
            output: Some(Type::List),
        }
    }
    fn children_of(u: &Unit) -> Vec<Hash> {
        match &u.body {
            UnitBody::Composite { children, .. } => children.clone(),
            _ => vec![],
        }
    }

    #[test]
    fn repins_a_direct_child() {
        let mut r = MapResolver::new();
        let a = r.insert(prim("add")).unwrap();
        let b = r.insert(prim("multiply")).unwrap();
        let a2 = r.insert(prim("subtract")).unwrap();
        let root = composite(vec![a, b]);

        let f = refork(&root, &a, &a2, &r).expect("a is in the closure");
        let new_root = &f.minted[&f.new_root];
        assert_eq!(children_of(new_root), vec![a2, b]); // a→a2, b shared
        assert_ne!(f.new_root, crate::hash_unit(&root).unwrap()); // a new identity
    }

    #[test]
    fn repins_deep_and_shares_off_path() {
        // root → [A, B], A → [C, D]; replace D → D2.
        let mut r = MapResolver::new();
        let c = r.insert(prim("add")).unwrap();
        let d = r.insert(prim("multiply")).unwrap();
        let d2 = r.insert(prim("subtract")).unwrap();
        let b = r.insert(prim("divide")).unwrap();
        let a = r.insert(composite(vec![c, d])).unwrap();
        let root = composite(vec![a, b]);

        let f = refork(&root, &d, &d2, &r).expect("d is in the closure");

        // Exactly the spine A and root were re-minted — B and C were not.
        assert_eq!(f.minted.len(), 2);
        let new_root = &f.minted[&f.new_root];
        let new_a_hash = children_of(new_root)[0];
        let new_a = &f.minted[&new_a_hash];
        assert_eq!(children_of(new_root)[1], b); // B shared untouched
        assert_eq!(children_of(new_a), vec![c, d2]); // C shared, D→D2
        assert!(!f.minted.contains_key(&b) && !f.minted.contains_key(&c)); // off-path not copied
    }

    #[test]
    fn repins_a_fn_param() {
        let mut r = MapResolver::new();
        let f0 = r.insert(prim("add")).unwrap();
        let f1 = r.insert(prim("multiply")).unwrap();
        let m = map_over(&f0);

        let result = refork(&m, &f0, &f1, &r).expect("f0 is the fn");
        let new_map = &result.minted[&result.new_root];
        assert_eq!(crate::child_refs(new_map), vec![f1]); // fn now points at f1
    }

    #[test]
    fn replace_all_occurrences() {
        // root → [X, X]: the same target in two slots, both re-pinned in one fork.
        let mut r = MapResolver::new();
        let x = r.insert(prim("add")).unwrap();
        let y = r.insert(prim("multiply")).unwrap();
        let root = composite(vec![x, x]);

        let f = refork(&root, &x, &y, &r).expect("x is in the closure");
        assert_eq!(children_of(&f.minted[&f.new_root]), vec![y, y]); // both slots
    }

    #[test]
    fn target_not_in_closure_is_none() {
        let mut r = MapResolver::new();
        let a = r.insert(prim("add")).unwrap();
        let root = composite(vec![a]);
        let absent = crate::hash_unit(&prim("not_here")).unwrap();
        let any = crate::hash_unit(&prim("anything")).unwrap();
        assert!(refork(&root, &absent, &any, &r).is_none());
    }
}