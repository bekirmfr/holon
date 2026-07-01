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
//! *declares* `egress` a [`Sink`](Kind::Sink) without being able to *run* it —
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

// ── Capabilities: the fine-grained manifest (the *primary* classification) ────
//
// Capabilities are primary; [`Kind`] is their coarse four-bucket *projection*
// (P2.3.1). Capability is finer than kind on purpose: `Clock` and `SecretAccess`
// both project to `Driver`, but a consent surface must tell "reads the clock" from
// "reads your secrets" — the distinction kind discards and capabilities keep. By
// *derivable ⟹ derived*, kind is computed from capabilities, never asserted beside
// them, so the two cannot drift.

/// A category of effect — what a leaf touches outside its inputs. **Atomic** for
/// now: the *category*, not yet the *resource* (`NetworkEgress`, not
/// `NetworkEgress("api.evil.com")`); the resource parameter is a deferred precision
/// layer. Grouped by the [`Kind`] each projects to.
///
/// `PartialOrd`/`Ord` exist for deterministic [`BTreeSet`] storage only — storage
/// order, not a danger ranking (same disclaimer as [`Kind`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Capability {
    /// Reads wall-clock / timers.
    Clock,
    /// Receives inbound data / requests.
    NetworkIngress,
    /// Reads external file state.
    FileRead,
    /// Reads a secret / credential (a security-sensitive read).
    SecretAccess,
    /// Sources entropy / nondeterminism.
    Random,
    /// Reads a value retained across firings.
    StateRead,
    /// Writes a value retained across firings.
    StateWrite,
    /// Sends outbound (mail, HTTP write).
    NetworkEgress,
    /// Writes / deletes files.
    FileWrite,
    /// Starts a process / job.
    Spawn,
}

impl Capability {
    /// The coarse [`Kind`] bucket this capability projects to — *read the world* ⇒
    /// `Driver`, *retain* ⇒ `State`, *commit irreversibly* ⇒ `Sink`. Never
    /// [`Kind::Pure`]: purity is the **empty** capability set, not a projection
    /// target.
    pub fn kind(self) -> Kind {
        use Capability::*;
        match self {
            Clock | NetworkIngress | FileRead | SecretAccess | Random => Kind::Driver,
            StateRead | StateWrite => Kind::State,
            NetworkEgress | FileWrite | Spawn => Kind::Sink,
        }
    }

    /// **Sourced** (input-side) ⇔ the host *supplies* this value to the graph (a clock
    /// tick, an inbound request, a file/secret/random read, **a state read**). The
    /// complement is **performed** (output-side): the host *executes* it (egress, file
    /// write, spawn, **state write**). Design mode mocks the sourced side; deploy
    /// performs the rest. The `State` kind splits here — `StateRead` enters at the top,
    /// `StateWrite` exits at the bottom — which is what "time in, consequences out"
    /// always meant.
    pub fn is_sourced(self) -> bool {
        use Capability::*;
        matches!(self, Clock | NetworkIngress | FileRead | SecretAccess | Random | StateRead)
    }
}

/// The capability manifest of a unit — the set it **may** touch. Same shape as
/// [`Effects`]: **∅ ⇒ pure** (a positive "touches nothing outside its inputs"), a
/// may-touch upper bound. The union over a closure (P2.3.2) is the same fold as
/// [`effects_of`], with `Capability` in place of [`Kind`].
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Capabilities(BTreeSet<Capability>);

impl Capabilities {
    /// No capabilities ⇒ pure.
    pub fn is_pure(&self) -> bool {
        self.0.is_empty()
    }
    /// Alias of [`is_pure`](Capabilities::is_pure) (the empty manifest is pure).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Does the manifest include this capability?
    pub fn contains(&self, cap: Capability) -> bool {
        self.0.contains(&cap)
    }
    /// Number of distinct capabilities (`0` ⇒ pure).
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Iterate the capabilities in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.0.iter().copied()
    }
    /// Replayable / cacheable ⇔ **no `State` capability anywhere in the closure** — the
    /// one place the design forfeits caching and replay (`§7`). This is *orthogonal* to
    /// effect-bearing: a sink-bearing but state-free manifest is effect-bearing **and**
    /// replayable — the combination the memo market (P3.3) keys on. Drivers are inputs,
    /// sinks purely construct requests (Fork B); only retained state breaks determinism
    /// across firings. (The positive form of P2.6.3's "cell-bearing ⇒ non-replayable".)
    pub fn is_replayable(&self) -> bool {
        !self.iter().any(|c| c.kind() == Kind::State)
    }
}

/// Classify a **primitive leaf**'s capability manifest by name — the *primary* leaf
/// classification (P2.3.1), from which [`kind_of`] and [`effects_of`] are projected.
///
/// - pure leaf ⇒ `Some(∅)` — the positive "touches nothing outside its inputs";
/// - effectful exemplar ⇒ `Some({…})`;
/// - unknown leaf ⇒ `None` — **fail closed**: a caller treats it at least as
///   conservatively as the full capability set, never as ∅.
///
/// Closed core table: the marketplace ships only composites (which *derive*); new
/// effect leaves are trusted host releases that add a row here. Core *declares* the
/// manifest; `holon-host` *honours* it at execution.
pub fn capabilities_of(name: &str) -> Option<Capabilities> {
    use Capability::*;
    let caps: &[Capability] = match name {
        // Pure leaves: the first-order vocabulary `compute` implements, plus
        // higher-order `map` (effect-free). A positive empty manifest.
        "const"
        | "add" | "multiply" | "subtract" | "divide"
        | "compare" | "and" | "or" | "not" | "select"
        | "concat" | "uppercase" | "lowercase" | "text_length" | "slice" | "index_of"
        | "list" | "get" | "append" | "length" | "sum" | "join"
        | "cons" | "head" | "tail" | "is_empty"
        | "pair"
        | "some" | "none"
        | "fold" | "map" => &[],

        // Effectful exemplars (classification only; execution is host-side). A state
        // cell is two leaves: `store_read` sources its slot (input-side), `store_write`
        // commits to it (output-side) — see the P2.6.1 L4.
        "store_read" => &[StateRead],
        "store_write" => &[StateWrite],
        "clock" => &[Clock],
        "random" => &[Random],
        "egress" => &[NetworkEgress],
        // Request/response (P2.5.1): `ingress` is the driver (a request arrives), `egress` carries
        // the reply out. §13 merges the old `email`/`respond` into this one `egress` sink — both
        // were NetworkEgress, and the host dispatches on the *capability*, not the sink name, so the
        // destination (mailbox, connection token, …) belongs in the payload, not in a distinct
        // primitive. The correlation token rides as ordinary data; isolation is purity's
        // (independent pure paths can't cross), so the token is only a boundary label.
        "ingress" => &[NetworkIngress],

        // Unknown leaf ⇒ fail closed. The engine does not vouch for it.
        _ => return None,
    };
    Some(Capabilities(caps.iter().copied().collect()))
}

/// The coarse [`Kind`] of a **primitive leaf**, *projected* from its capability
/// manifest ([`capabilities_of`]) — the single source of leaf classification:
///
/// - unknown leaf ⇒ `None` (fail-closed);
/// - pure leaf (∅ manifest) ⇒ `Some(Pure)`;
/// - effectful leaf ⇒ the [`Kind`] its capabilities project to.
///
/// A single-bucket **convenience**, exact for the current ≤1-axis vocabulary. The
/// general truth — a leaf may touch several axes — is the *set* [`effects_of`]
/// returns; when a multi-axis leaf first exists, this convenience yields to it.
pub fn kind_of(name: &str) -> Option<Kind> {
    let caps = capabilities_of(name)?;
    let first = caps.iter().next();
    Some(match first {
        None => Kind::Pure,
        Some(cap) => cap.kind(),
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

/// Derive the **capability manifest** of a unit by folding [`capabilities_of`]
/// over its closure (P2.3.2) — the fine-grained, security-relevant union of
/// everything the unit *may* touch, **computed not stored**, surfaced *before* a
/// unit is run or imported (I9). This is the single closure walk; [`effects_of`] is
/// its projection.
///
/// A primitive contributes its own manifest; a composite the union over its
/// children; **and any hash-valued param is followed as a reference**, so a `map`
/// over an effectful `fn` cannot hide its capabilities.
///
/// Fail-closed throughout — an unknown leaf, an unresolvable child, or a
/// hash-shaped param that does not resolve each *saturate* to the full set. The
/// analysis never assumes innocence it cannot verify, and does not depend on
/// validation having run first. Termination is free: content-addressing forbids a
/// cycle (a hash cannot contain itself), so the closure is a finite DAG.
pub fn capabilities(unit: &Unit, resolver: &impl Resolver) -> Capabilities {
    let mut acc = BTreeSet::new();
    collect_caps(unit, resolver, &mut acc);
    Capabilities(acc)
}

/// Derive the kind-set of a unit — the coarse **projection** of its
/// [`capabilities`]. There is one source of truth and one closure walk (the
/// capability union); kind is each capability's [`Kind`](Capability::kind) lifted
/// to the set. (Saturated capabilities project to the saturated kind-set
/// `{Driver, State, Sink}`, so fail-closed behaviour is preserved.)
pub fn effects_of(unit: &Unit, resolver: &impl Resolver) -> Effects {
    let mut acc = BTreeSet::new();
    for cap in capabilities(unit, resolver).iter() {
        acc.insert(cap.kind());
    }
    Effects(acc)
}

/// Insert every capability — the saturated, maximally-suspicious manifest.
fn saturate_caps(acc: &mut BTreeSet<Capability>) {
    use Capability::*;
    for c in [
        Clock, NetworkIngress, FileRead, SecretAccess, Random, StateRead,
        StateWrite, NetworkEgress, FileWrite, Spawn,
    ] {
        acc.insert(c);
    }
}

fn collect_caps(unit: &Unit, resolver: &impl Resolver, acc: &mut BTreeSet<Capability>) {
    // (1) This unit's own leaf contribution, if it is a primitive: its capability
    //     manifest. A pure leaf's ∅ contributes nothing; an unknown leaf fails closed.
    if let UnitBody::Primitive { name } = &unit.body {
        match capabilities_of(name) {
            Some(caps) => {
                for c in caps.iter() {
                    acc.insert(c);
                }
            }
            None => saturate_caps(acc), // unknown leaf ⇒ fail closed
        }
    }

    // (2) Every reference — a composite's children and the `fn` param — via the shared
    //     `child_refs` (the one definition of what a unit references). A capability routed
    //     through a reference cannot hide; an unresolvable reference fails closed rather than
    //     being read as inert data.
    for h in crate::inspect::child_refs(unit) {
        match resolver.resolve(&h) {
            Some(u) => collect_caps(u, resolver, acc),
            None => saturate_caps(acc),
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
            outputs: vec![],
        }
    }

    /// A composite over the given children. Edges are irrelevant to effect
    /// derivation (it walks children + params, never wiring), so they stay empty.
    fn composite(children: Vec<Hash>) -> Unit {
        Unit {
            body: UnitBody::Composite { children, edges: vec![] },
            params: vec![],
            inputs: vec![],
            outputs: vec![],
        }
    }

    /// A `map` primitive whose `fn` param references `target`.
    fn map_over(target: &Hash) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "map".to_string() },
            params: vec![Param::frozen("fn", to_hex(target))],
            inputs: vec![],
            outputs: vec![],
        }
    }

    /// Every leaf `compute` implements, plus `map`, classifies `Pure`. (Listed
    /// explicitly so a drift between this list and the table is a test failure.)
    #[test]
    fn pure_leaves_are_pure() {
        const PURE: &[&str] = &[
            "const", "add", "multiply",
            "subtract", "divide", "compare", "and", "or", "not", "select",
            "concat", "uppercase", "lowercase", "text_length", "slice", "index_of",
            "list", "get", "append", "length", "sum",
            "join", "cons", "head", "tail", "is_empty", "pair", "some", "none", "fold", "map",
        ];
        for name in PURE {
            assert_eq!(kind_of(name), Some(Kind::Pure), "{name} should be Pure");
        }
    }

    #[test]
    fn effectful_exemplars_classify() {
        assert_eq!(kind_of("store_read"), Some(Kind::State));
        assert_eq!(kind_of("store_write"), Some(Kind::State));
        assert_eq!(kind_of("clock"), Some(Kind::Driver));
        assert_eq!(kind_of("random"), Some(Kind::Driver));
        assert_eq!(kind_of("egress"), Some(Kind::Sink));
    }

    /// The security-relevant direction: every effectful leaf classifies non-`Pure`.
    /// (The cross-crate half — that the host never executes a `Pure`-classified
    /// name — lands with host execution, P2.4.1 / P2.6.1.)
    #[test]
    fn effectful_exemplars_are_not_pure() {
        for name in ["store_read", "store_write", "clock", "egress"] {
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
        let n = store.insert(prim("const")).unwrap();
        let m = store.insert(prim("multiply")).unwrap();
        let comp = composite(vec![n, m]);
        let fx = effects_of(&comp, &store);
        assert!(fx.is_pure());
        assert_eq!(fx.len(), 0);
    }

    #[test]
    fn sink_leaf_at_depth_surfaces() {
        // An `egress` (Sink) buried three levels down must surface at the top —
        // and the pure sibling must add nothing.
        let mut store = MapResolver::new();
        let egress = store.insert(prim("egress")).unwrap();
        let inner = store.insert(composite(vec![egress])).unwrap();
        let mid = store.insert(composite(vec![inner])).unwrap();
        let pure = store.insert(prim("const")).unwrap();
        let top = composite(vec![mid, pure]);
        let fx = effects_of(&top, &store);
        assert!(!fx.is_pure());
        assert!(fx.contains(Kind::Sink));
        assert_eq!(fx.len(), 1);
    }

    #[test]
    fn mixed_axes_are_preserved_not_collapsed() {
        // clock (Driver) + egress (Sink) ⇒ {Driver, Sink}, not one "worst" bucket.
        let mut store = MapResolver::new();
        let clock = store.insert(prim("clock")).unwrap();
        let egress = store.insert(prim("egress")).unwrap();
        let comp = composite(vec![clock, egress]);
        let fx = effects_of(&comp, &store);
        assert!(fx.contains(Kind::Driver));
        assert!(fx.contains(Kind::Sink));
        assert!(!fx.contains(Kind::State));
        assert_eq!(fx.len(), 2);
    }

    #[test]
    fn map_over_effectful_fn_is_effectful() {
        // The laundering test: `map` is itself Pure, but its `fn` references an
        // egress-bearing unit — following the hash-valued param is what keeps the
        // whole thing from reading as pure.
        let mut store = MapResolver::new();
        let egress = store.insert(prim("egress")).unwrap();
        let map = map_over(&egress);
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

    // ── P2.3.1: the capability manifest (primary classification) ────────────

    #[test]
    fn pure_leaf_declares_empty_manifest() {
        // A positive empty manifest — Some(∅), not None — "touches nothing".
        let caps = capabilities_of("add").unwrap();
        assert!(caps.is_pure());
        assert_eq!(caps.len(), 0);
    }

    #[test]
    fn effectful_leaves_declare_their_caps() {
        let egress = capabilities_of("egress").unwrap();
        assert!(egress.contains(Capability::NetworkEgress));
        assert_eq!(egress.len(), 1);

        let store_read = capabilities_of("store_read").unwrap();
        assert!(store_read.contains(Capability::StateRead));
        assert_eq!(store_read.len(), 1);

        let store_write = capabilities_of("store_write").unwrap();
        assert!(store_write.contains(Capability::StateWrite));
        assert_eq!(store_write.len(), 1);

        let clock = capabilities_of("clock").unwrap();
        assert!(clock.contains(Capability::Clock));
        assert_eq!(clock.len(), 1);

        // Request/response (P2.5.1): the in/out pair. `ingress` (a request arrives) is a driver —
        // effect-bearing yet *replayable* (no State); `egress` carries the reply (checked above,
        // since the merged sink is the same leaf). The correlating token is plain data, never a
        // capability.
        let ingress = capabilities_of("ingress").unwrap();
        assert!(ingress.contains(Capability::NetworkIngress));
        assert!(ingress.is_replayable()); // a driver, not state
    }

    #[test]
    fn is_sourced_splits_input_from_output() {
        use Capability::*;
        // sourced (input-side): drivers and the state *read*
        for c in [Clock, NetworkIngress, FileRead, SecretAccess, Random, StateRead] {
            assert!(c.is_sourced(), "{c:?} should be sourced");
        }
        // performed (output-side): sinks and the state *write*
        for c in [NetworkEgress, FileWrite, Spawn, StateWrite] {
            assert!(!c.is_sourced(), "{c:?} should be performed");
        }
    }

    #[test]
    fn capability_projects_to_expected_kind() {
        assert_eq!(Capability::NetworkEgress.kind(), Kind::Sink);
        assert_eq!(Capability::Clock.kind(), Kind::Driver);
        assert_eq!(Capability::StateRead.kind(), Kind::State);
        assert_eq!(Capability::StateWrite.kind(), Kind::State);
    }

    #[test]
    fn fine_distinguishes_what_coarse_loses() {
        // The motivating case: Clock and SecretAccess both project to Driver, yet
        // are distinct capabilities — kind is lossy where capabilities are not.
        assert_eq!(Capability::Clock.kind(), Kind::Driver);
        assert_eq!(Capability::SecretAccess.kind(), Kind::Driver);
        assert_ne!(Capability::Clock, Capability::SecretAccess);
    }

    #[test]
    fn capability_kind_is_never_pure() {
        use Capability::*;
        for cap in [
            Clock, NetworkIngress, FileRead, SecretAccess, Random, StateRead,
            StateWrite, NetworkEgress, FileWrite, Spawn,
        ] {
            assert_ne!(cap.kind(), Kind::Pure, "{cap:?} must project to an effect");
        }
    }

    #[test]
    fn unknown_leaf_has_no_manifest() {
        assert_eq!(capabilities_of("frobnicate"), None);
        assert_eq!(capabilities_of(""), None);
    }

    // ── P2.3.2: capability union, surfaced before import ────────────────────

    #[test]
    fn buried_egress_surfaces_before_import() {
        // The governing I9 test: a `NetworkEgress` buried three levels down
        // surfaces at the top — nothing exfiltrates silently.
        let mut store = MapResolver::new();
        let egress = store.insert(prim("egress")).unwrap();
        let inner = store.insert(composite(vec![egress])).unwrap();
        let mid = store.insert(composite(vec![inner])).unwrap();
        let pure = store.insert(prim("const")).unwrap();
        let top = composite(vec![mid, pure]);
        let caps = capabilities(&top, &store);
        assert!(caps.contains(Capability::NetworkEgress));
        assert_eq!(caps.len(), 1);
    }

    #[test]
    fn pure_closure_has_empty_manifest() {
        let mut store = MapResolver::new();
        let n = store.insert(prim("const")).unwrap();
        let m = store.insert(prim("multiply")).unwrap();
        let caps = capabilities(&composite(vec![n, m]), &store);
        assert!(caps.is_pure());
    }

    #[test]
    fn mixed_caps_union_over_closure() {
        let mut store = MapResolver::new();
        let clock = store.insert(prim("clock")).unwrap();
        let egress = store.insert(prim("egress")).unwrap();
        let store_read = store.insert(prim("store_read")).unwrap();
        let store_write = store.insert(prim("store_write")).unwrap();
        let comp = composite(vec![clock, egress, store_read, store_write]);
        let caps = capabilities(&comp, &store);
        assert!(caps.contains(Capability::Clock));
        assert!(caps.contains(Capability::NetworkEgress));
        assert!(caps.contains(Capability::StateRead));
        assert!(caps.contains(Capability::StateWrite));
        assert_eq!(caps.len(), 4);
    }

    #[test]
    fn map_fn_capabilities_surface() {
        // A capability routed through a `map` fn-param cannot hide.
        let mut store = MapResolver::new();
        let egress = store.insert(prim("egress")).unwrap();
        let caps = capabilities(&map_over(&egress), &store);
        assert!(caps.contains(Capability::NetworkEgress));
    }

    #[test]
    fn unknown_in_closure_saturates_caps() {
        let mut store = MapResolver::new();
        let unknown = store.insert(prim("frobnicate")).unwrap();
        let caps = capabilities(&composite(vec![unknown]), &store);
        assert_eq!(caps.len(), 10); // every capability — fail closed
    }

    #[test]
    fn effects_is_the_projection_of_capabilities() {
        // The structural invariant: effects_of is exactly capabilities projected
        // through Capability::kind — one source of truth, one walk.
        let mut store = MapResolver::new();
        let clock = store.insert(prim("clock")).unwrap();
        let egress = store.insert(prim("egress")).unwrap();
        let comp = composite(vec![clock, egress]);
        let projected: std::collections::BTreeSet<Kind> =
            capabilities(&comp, &store).iter().map(|c| c.kind()).collect();
        let fx = effects_of(&comp, &store);
        assert!(fx.contains(Kind::Driver) && fx.contains(Kind::Sink));
        assert_eq!(fx.iter().collect::<std::collections::BTreeSet<_>>(), projected);
    }
}