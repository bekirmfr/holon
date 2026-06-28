//! holon-core — the pure evaluator substrate.
//!
//! This crate is the canonical, deterministic core that compiles to both native
//! and WASM. It must never gain an IO / async / networking dependency: the
//! purity boundary (invariants I2 / I7) is enforced by this crate's dependency
//! graph, so "the core cannot perform an effect" stays a compile-time fact.
//!
//! Modules:
//! - [`hash`] — content hashing (I5) and, as a free consequence, transitive
//!   pinning (I6): a unit's hash is computed bottom-up over its closure, every
//!   child referenced only by its 32-byte hash.
//! - [`value`] — the runtime [`Value`] that flows on wires (P1.4.1).

mod hash;
mod value;

pub use hash::{
    encode_preimage, hash_unit, Edge, EncodeError, Hash, Param, Port, Type, Unit,
    UnitBody, ENCODING_VERSION,
};
pub use value::Value;