//! holon-core — the pure evaluator substrate.
//!
//! This crate is the canonical, deterministic core that compiles to both native
//! and WASM. It must never gain an IO / async / networking dependency: the
//! purity boundary (invariants I2 / I7) is enforced by this crate's dependency
//! graph, so "the core cannot perform an effect" stays a compile-time fact.
//!
//! This module implements **content hashing** (invariant I5) and, as a free
//! consequence of hashing children by their digests, **transitive pinning**
//! (invariant I6). A unit's hash is computed bottom-up over its closure; every
//! child is referenced *only* by its 32-byte hash, never inlined, so a unit's
//! hash commits to its entire dependency closure.

mod hash;

pub use hash::{
    encode_preimage, hash_unit, Edge, EncodeError, Hash, Param, Port, Type, Unit,
    UnitBody, ENCODING_VERSION,
};
