//! holon-core — the pure evaluator substrate.
//!
//! This crate is the canonical, deterministic core that compiles to both native
//! and WASM. It must never gain an IO / async / networking dependency: the
//! purity boundary (invariants I2 / I7) is enforced by this crate's dependency
//! graph, so "the core cannot perform an effect" stays a compile-time fact.
//!
//! Modules:
//! - [`hash`] — content hashing (I5) and transitive pinning (I6).
//! - [`value`] — the runtime [`Value`] that flows on wires (P1.4.1).
//! - [`resolve`] — turn a child hash into its [`Unit`] (shared by validation and
//!   evaluation).
//! - [`validate`] — graph validation: well-formedness, acyclicity, wire-time type
//!   compatibility (P1.3.1).
//! - [`primitives`] — the pure first-order primitive vocabulary + native compute
//!   (P1.4.2).
//! - [`eval`] — the eager graph evaluator (P1.4.3).
//! - [`determinism`] — float discipline + the determinism harness (P1.4.4).
//! - [`encapsulate`] — derive-then-freeze a selection into a hashed `Unit` (P1.5.1).
//! - [`promote`] — open params into holes and instantiate templates (P1.5.2).
//! - [`kind`] — effect classification: a leaf's capability manifest (P2.3.1, the
//!   primary classification) and its Pure/Driver/State/Sink kind projection (P2.1.1),
//!   derived over a closure (P2.2.1); the trust basis of the effect layer.

mod determinism;
mod advisory;
mod catalog;
mod declare;
mod encapsulate;
mod eval;
mod fork;
mod kind;
mod memo;
mod promote;
mod hash;
mod inspect;
mod json;
mod resolve;
mod simulate;
mod validate;
mod value;
mod value_hash;

pub mod primitives;

pub use determinism::is_finite_number;
pub use encapsulate::{default_spec, derive_and_freeze, EncapError, EncapSpec, Expose, Promote};
pub use promote::{instantiate, open_param, open_params, ParamError};
pub use eval::{eval, eval_observed, eval_one, NoObserver, Observer};
pub use fork::{refork, Refork};
pub use advisory::{advise, Advisory};
pub use catalog::{genesis_units, interface_of, GENESIS, VARIADIC};
pub use inspect::{adoption_counts, child_refs, dependencies};
pub use memo::{entry_key, memo_key, verify_entry, CacheEntry};
pub use value_hash::{encode_value, hash_value};
pub use hash::{
    canonical_child_order,
    decode_preimage, encode_preimage, hash_unit, parse_hash, DecodeError, Edge, EncodeError, Hash,
    Param, Port, Type, Unit, UnitBody, ENCODING_VERSION, MAX_TYPE_DEPTH,
};
pub use kind::{
    capabilities, capabilities_of, effects_of, kind_of, Capabilities, Capability,
    Effects, Kind,
};
pub use resolve::{MapResolver, Resolver};
pub use simulate::{design_eval, mock_of, Simulated};
pub use validate::{compatible, validate, ValidationError};
pub use value::Value;
pub use json::{parse as parse_json, stringify as stringify_json};
pub use declare::{
    declaration_of, match_route, parse_path, read_endpoint, read_server, DeclError, EndpointError, Route, Segment,
    Server, METHODS,
};

/// Reserved edge endpoint denoting a composite's **boundary** (its own interface)
/// rather than one of its children.
///
/// The composite's interface↔inner binding rides the ordinary edge mechanism — no
/// new fields, no encoder change:
/// - an edge with `from_child == BOUNDARY` carries the composite's input `from_port`
///   *inward* to a child (the boundary acts as a **source**);
/// - an edge with `to_child == BOUNDARY` exposes a child's output as the composite's
///   output `to_port` (the boundary acts as a **sink**).
///
/// Because the boundary is a source for inputs and a sink for outputs, it can never
/// participate in an internal cycle, and a graph traversal that only follows
/// child→child edges (e.g. acyclicity) naturally ignores it. A composite cannot have
/// `u32::MAX` children, so the sentinel never collides with a real index.
pub const BOUNDARY: u32 = u32::MAX;