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
/// The non-`Pure` variants are the three ways purity ends, in increasing
/// commitment: a [`Driver`](Kind::Driver) *reads* the world, a
/// [`State`](Kind::State) cell *remembers* across firings, a [`Sink`](Kind::Sink)
/// *changes* the world irreversibly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
}