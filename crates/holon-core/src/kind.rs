//! # Kind — the four-bucket classification of what a unit *is* (P2.1.1)
//!
//! `Kind` answers "does this touch the world, and how?" with the coarsest useful
//! partition: [`Pure`](Kind::Pure), [`Driver`](Kind::Driver),
//! [`State`](Kind::State), [`Sink`](Kind::Sink). It is the entry point of the
//! effect layer (P2) and the seam where the pure core (P1) starts to acknowledge
//! that a *runtime* will eventually run.
//!
//! ## Kind is informative, not identity
//!
//! Kind is a **computed view**, never a stored or hashed field. A primitive's kind
//! follows authoritatively from its name (already in the hash); a composite's kind
//! is *derived* from its children (P2.2.1). There is no `kind` field to set, so
//! laundering an effect into a "pure" label is not *prevented* — it is
//! *inexpressible* (invariant `I8`).
//!
//! ## This table is the trust basis — and it is closed
//!
//! [`kind_of`] is core-resident for **every** leaf, effectful ones included: core
//! *declares* `email` a [`Sink`](Kind::Sink) without being able to *run* it —
//! execution lives in `holon-host`. That split is sound because the leaf set is
//! **closed and trusted**: the permissionless marketplace ships only *composites*
//! (whose kind derives, no declaration), while new *leaves* (new effects) arrive
//! only as trusted host releases ("syscalls"). So there is no untrusted-author
//! declares-a-leaf's-kind surface to defend.
//!
//! Two boundary invariants keep the core/host split honest. Neither can be
//! exercised until host execution exists (P2.4.1 / P2.6.1), so they are recorded
//! here and tested then:
//!
//! 1. **Disjointness** — a leaf is pure-computable-in-core *xor*
//!    effect-executable-in-host, never both. The names [`crate::primitives`]
//!    computes all classify [`Pure`](Kind::Pure); the names the host executes all
//!    classify non-`Pure`; the two sets do not intersect.
//! 2. **No effect without classification** — every name the host can fire as an
//!    effect classifies non-`Pure` here. An effect the host runs that this table
//!    calls `Pure` *is* the laundering hole; this containment forbids it.

/// What a primitive or composite *is*, with respect to effects.
///
use crate::hash::parse_hash;
use crate::{Resolver, Unit, UnitBody};
use std::collections::BTreeSet;

/// The non-`Pure` variants are the three ways purity ends, in increasing
/// commitment: a [`Driver`](Kind::Driver) *reads* the world, a
/// [`State`](Kind::State) cell *remembers* across firings, a [`Sink`](Kind::Sink)
/// *changes* the world irreversibly.
///
/// `PartialOrd`/`Ord` are derived only so [`Kind`] can sit in a `BTreeSet`
/// ([`Effects`]) with deterministic iteration — a content-addressed system must
/// not have order wobble. The resulting order (`Pure < Driver < State < Sink`) is
/// **storage order, not a danger priority**: we deliberately do *not* rank the
/// effect axes against each other.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    /// No effect: the output is a pure function of the inputs. The
    /// empty-capability case — touches nothing outside what it is given.
    Pure,
    /// Sources stimulus from outside the graph (a clock tick, an inbound
    /// request). Reads the world; does not change it.
    Driver,
    /// Retains a value across firings — the one fenced place purity ends. The
    /// donor primitive from which durable effect contracts are composed (P2.6).
    State,
    /// Performs an irreversible consequence on the world (send a mail, write a
    /// file). The committing edge of the system.
    Sink,
}

impl Kind {
    /// `true` for anything other than [`Pure`](Kind::Pure) — i.e. it touches the
    /// world in some way. This is the coarse gate; the capability surface (P2.3)
    /// refines *what* it touches.
    pub fn is_effectful(self) -> bool {
        self != Kind::Pure
    }
}

/// Classify a **primitive leaf** by its name.
///
/// Returns `None` for a name this engine does not implement — a **fail-closed**
/// signal. A caller must treat an unclassified leaf at least as conservatively as
/// a [`Sink`](Kind::Sink), never as [`Pure`](Kind::Pure); a `Pure` answer is a
/// *positive* claim that the leaf touches nothing outside its inputs, and the
/// engine only makes that claim for leaves it actually owns.
///
/// This classifies leaves only. A composite's kind is *derived* from its closure
/// (P2.2.1), not looked up here.
pub fn kind_of(name: &str) -> Option<Kind> {
    use Kind::*;
    Some(match name {
        // --- Pure leaves: the first-order vocabulary `compute` implements, plus
        // --- higher-order `map` (effect-free; dispatched inside `eval`). Every
        // --- name here is a positive "touches nothing outside its inputs".
        "number" | "boolean" | "text" | "true" | "false"
        | "add" | "multiply" | "subtract" | "divide"
        | "compare" | "and" | "or" | "not" | "select"
        | "concat" | "uppercase"
        | "list" | "get" | "append" | "length" | "sum" | "join"
        | "map" => Pure,

        // --- Effectful exemplars: classification only. These have no `compute`;
        // --- their execution is registered in `holon-host` against these names
        // --- when the firing loop and state cell land (P2.4.1 / P2.6.1). They
        // --- anchor the three non-pure kinds so the taxonomy is exercised now.
        "store" => State,
        "timer" => Driver,
        "email" => Sink,

        // Unknown leaf ⇒ fail closed. The engine does not vouch for it.
        _ => return None,
    })
}

/// The set of effect axes a unit **may** touch — its kind lifted from a single
/// bucket to the honest *set*, because a composite can drive, hold state, *and*
/// sink at once; collapsing that to one "worst" bucket would conceal real axes (a
/// soft cousin of laundering).
///
/// **Purity is the empty set.** [`Kind::Pure`] is never a member — it is the
/// *absence* of effect — so [`is_pure`](Effects::is_pure) is exactly "no axes".
/// This mirrors the capability model (a pure unit declares ∅), and is the shape
/// capability-union (P2.3.2) will reuse.
///
/// The set is a **may-touch upper bound** — a permission/assumption surface, not a
/// record of what ran — so an unclassified or unresolvable reference *saturates*
/// to all three axes. Over-approximating danger is the sound direction;
/// under-approximating to `Pure` would be the laundering hole.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Effects(BTreeSet<Kind>);

impl Effects {
    /// No effect axes ⇒ pure. The empty set *is* purity.
    pub fn is_pure(&self) -> bool {
        self.0.is_empty()
    }
    /// Alias of [`is_pure`](Effects::is_pure) (the empty set is pure).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Does the may-touch set include this axis?
    pub fn contains(&self, kind: Kind) -> bool {
        self.0.contains(&kind)
    }
    /// Number of distinct effect axes (`0` ⇒ pure).
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Iterate the axes in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = Kind> + '_ {
        self.0.iter().copied()
    }
}

/// Derive the effect set of a unit by folding [`kind_of`] over its closure
/// (P2.2.1): a primitive contributes its own kind; a composite contributes the
/// union over its children; **and any hash-valued param is followed as a
/// reference**, so a `map` over an effectful function cannot read as pure.
///
/// Fail-closed throughout — an unknown leaf, an unresolvable child, or a
/// hash-shaped param that does not resolve each *saturate* to every axis. The
/// analysis never assumes innocence it cannot verify, and does not depend on
/// validation having run first.
///
/// Termination is free: content-addressing forbids a cycle (a hash cannot contain
/// itself), so the closure is a finite DAG.
pub fn effects_of(unit: &Unit, resolver: &impl Resolver) -> Effects {
    let mut acc = BTreeSet::new();
    collect(unit, resolver, &mut acc);
    Effects(acc)
}

/// Insert the three ways purity ends — the saturated, maximally-suspicious set.
fn saturate(acc: &mut BTreeSet<Kind>) {
    acc.insert(Kind::Driver);
    acc.insert(Kind::State);
    acc.insert(Kind::Sink);
}

fn collect(unit: &Unit, resolver: &impl Resolver, acc: &mut BTreeSet<Kind>) {
    // (1) This unit's own leaf contribution, if it is a primitive.
    if let UnitBody::Primitive { name } = &unit.body {
        match kind_of(name) {
            Some(Kind::Pure) => {}            // contributes no axis
            Some(k) => {
                acc.insert(k);                // Driver / State / Sink
            }
            None => saturate(acc),            // unknown leaf ⇒ fail closed
        }
    }

    // (2) Children of a composite, recursed.
    if let UnitBody::Composite { children, .. } = &unit.body {
        for child in children {
            match resolver.resolve(child) {
                Some(u) => collect(u, resolver, acc),
                None => saturate(acc),        // unresolvable child ⇒ fail closed
            }
        }
    }

    // (3) Hash-valued params (e.g. `map`'s `fn`) are references, not data — follow
    //     them so an effect routed through a param cannot hide. An `Open` hole has
    //     no value and contributes nothing (it is filled at instantiate time). A
    //     value shaped exactly like a 64-char hash is treated as a reference; if it
    //     cannot be resolved, we fail closed rather than read it as inert data.
    for p in &unit.params {
        if let Some(v) = p.value() {
            if let Some(h) = parse_hash(v) {
                match resolver.resolve(&h) {
                    Some(u) => collect(u, resolver, acc),
                    None => saturate(acc),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::to_hex;
    use crate::{Hash, MapResolver, Param, Unit, UnitBody};

    fn prim(name: &str) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.to_string() },
            params: vec![],
            inputs: vec![],
            output: None,
        }
    }

    /// A composite over the given children. Edges are irrelevant to effect
    /// derivation (it walks children + params, never wiring), so they stay empty.
    fn composite(children: Vec<Hash>) -> Unit {
        Unit {
            body: UnitBody::Composite { children, edges: vec![] },
            params: vec![],
            inputs: vec![],
            output: None,
        }
    }

    /// A `map` primitive whose `fn` param references `target`.
    fn map_over(target: &Hash) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "map".to_string() },
            params: vec![Param::frozen("fn", to_hex(target))],
            inputs: vec![],
            output: None,
        }
    }

    /// Every leaf `compute` implements, plus `map`, classifies `Pure`. (Listed
    /// explicitly so a drift between this list and the table is a test failure.)
    #[test]
    fn pure_leaves_are_pure() {
        const PURE: &[&str] = &[
            "number", "boolean", "text", "true", "false", "add", "multiply",
            "subtract", "divide", "compare", "and", "or", "not", "select",
            "concat", "uppercase", "list", "get", "append", "length", "sum",
            "join", "map",
        ];
        for name in PURE {
            assert_eq!(kind_of(name), Some(Kind::Pure), "{name} should be Pure");
        }
    }

    #[test]
    fn effectful_exemplars_classify() {
        assert_eq!(kind_of("store"), Some(Kind::State));
        assert_eq!(kind_of("timer"), Some(Kind::Driver));
        assert_eq!(kind_of("email"), Some(Kind::Sink));
    }

    /// The security-relevant direction: every effectful leaf classifies non-`Pure`.
    /// (The cross-crate half — that the host never executes a `Pure`-classified
    /// name — lands with host execution, P2.4.1 / P2.6.1.)
    #[test]
    fn effectful_exemplars_are_not_pure() {
        for name in ["store", "timer", "email"] {
            assert!(kind_of(name).unwrap().is_effectful(), "{name} must be effectful");
        }
    }

    /// `map` is higher-order but carries no effect — it must read as `Pure`.
    #[test]
    fn map_is_pure() {
        assert_eq!(kind_of("map"), Some(Kind::Pure));
        assert!(!kind_of("map").unwrap().is_effectful());
    }

    /// An unknown leaf fails closed: `None`, never `Some(Pure)`.
    #[test]
    fn unknown_is_none() {
        assert_eq!(kind_of("xyzzy"), None);
        assert_eq!(kind_of(""), None);
        assert_ne!(kind_of("definitely_not_a_primitive"), Some(Kind::Pure));
    }

    #[test]
    fn pure_is_the_only_non_effectful_kind() {
        assert!(!Kind::Pure.is_effectful());
        assert!(Kind::Driver.is_effectful());
        assert!(Kind::State.is_effectful());
        assert!(Kind::Sink.is_effectful());
    }

    // ── effects_of: the derived (composite) kind ────────────────────────────

    #[test]
    fn pure_only_closure_is_pure() {
        let mut store = MapResolver::new();
        let n = store.insert(prim("number")).unwrap();
        let m = store.insert(prim("multiply")).unwrap();
        let comp = composite(vec![n, m]);
        let fx = effects_of(&comp, &store);
        assert!(fx.is_pure());
        assert_eq!(fx.len(), 0);
    }

    #[test]
    fn sink_leaf_at_depth_surfaces() {
        // An `email` (Sink) buried three levels down must surface at the top —
        // and the pure sibling must add nothing.
        let mut store = MapResolver::new();
        let email = store.insert(prim("email")).unwrap();
        let inner = store.insert(composite(vec![email])).unwrap();
        let mid = store.insert(composite(vec![inner])).unwrap();
        let pure = store.insert(prim("number")).unwrap();
        let top = composite(vec![mid, pure]);
        let fx = effects_of(&top, &store);
        assert!(!fx.is_pure());
        assert!(fx.contains(Kind::Sink));
        assert_eq!(fx.len(), 1);
    }

    #[test]
    fn mixed_axes_are_preserved_not_collapsed() {
        // timer (Driver) + email (Sink) ⇒ {Driver, Sink}, not one "worst" bucket.
        let mut store = MapResolver::new();
        let timer = store.insert(prim("timer")).unwrap();
        let email = store.insert(prim("email")).unwrap();
        let comp = composite(vec![timer, email]);
        let fx = effects_of(&comp, &store);
        assert!(fx.contains(Kind::Driver));
        assert!(fx.contains(Kind::Sink));
        assert!(!fx.contains(Kind::State));
        assert_eq!(fx.len(), 2);
    }

    #[test]
    fn map_over_effectful_fn_is_effectful() {
        // The laundering test: `map` is itself Pure, but its `fn` references an
        // email-bearing unit — following the hash-valued param is what keeps the
        // whole thing from reading as pure.
        let mut store = MapResolver::new();
        let email = store.insert(prim("email")).unwrap();
        let map = map_over(&email);
        let fx = effects_of(&map, &store);
        assert!(!fx.is_pure());
        assert!(fx.contains(Kind::Sink));
    }

    #[test]
    fn map_over_pure_fn_is_pure() {
        let mut store = MapResolver::new();
        let dbl = store.insert(prim("multiply")).unwrap();
        let map = map_over(&dbl);
        assert!(effects_of(&map, &store).is_pure());
    }

    #[test]
    fn unknown_leaf_saturates() {
        let mut store = MapResolver::new();
        let unknown = store.insert(prim("frobnicate")).unwrap();
        let comp = composite(vec![unknown]);
        let fx = effects_of(&comp, &store);
        assert!(fx.contains(Kind::Driver));
        assert!(fx.contains(Kind::State));
        assert!(fx.contains(Kind::Sink));
        assert_eq!(fx.len(), 3);
    }

    #[test]
    fn unresolvable_child_saturates() {
        // A child hash absent from the resolver fails closed, never silently pure.
        let store = MapResolver::new();
        let missing: Hash = [0u8; 32];
        let comp = composite(vec![missing]);
        let fx = effects_of(&comp, &store);
        assert_eq!(fx.len(), 3);
        assert!(!fx.is_pure());
    }

    #[test]
    fn pure_is_never_a_set_member() {
        // The empty-set invariant: purity is ∅, and `Pure` is never inserted.
        let store = MapResolver::new();
        let fx = effects_of(&prim("add"), &store);
        assert!(fx.is_pure());
        assert!(!fx.contains(Kind::Pure));
        assert_eq!(fx.iter().count(), 0);
    }
}