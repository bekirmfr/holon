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
mod encapsulate;
mod eval;
mod kind;
mod promote;
mod hash;
mod resolve;
mod validate;
mod value;

pub mod primitives;

pub use determinism::is_finite_number;
pub use encapsulate::{default_spec, derive_and_freeze, EncapError, EncapSpec, Promote};
pub use promote::{instantiate, open_param, open_params, ParamError};
pub use eval::eval;
pub use hash::{
    encode_preimage, hash_unit, Edge, EncodeError, Hash, Param, Port, Type, Unit,
    UnitBody, ENCODING_VERSION,
};
pub use kind::{
    capabilities_of, effects_of, kind_of, Capabilities, Capability, Effects, Kind,
};
pub use resolve::{MapResolver, Resolver};
pub use validate::{compatible, validate, ValidationError};
pub use value::Value;

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