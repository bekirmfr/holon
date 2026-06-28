//! Resolving a child hash to its unit definition.
//!
//! Composites reference children **only by 32-byte hash** (I3 / I5). Both
//! validation and evaluation need to turn a hash back into the unit it identifies.
//! Resolution is a pure lookup — never an effect — so it lives in the pure core.
//!
//! *(Surfaced while building P1.3.1: validation needs a child's interface to
//! type-check an edge, so the `Resolver` — originally scoped to the evaluator,
//! P1.4.3 — is introduced here, one leaf earlier. The evaluator reuses it
//! unchanged.)*

use crate::{hash_unit, EncodeError, Hash, Unit};
use std::collections::BTreeMap;

/// Turns a content hash into the unit it identifies.
pub trait Resolver {
    fn resolve(&self, hash: &Hash) -> Option<&Unit>;
}

/// In-memory resolver (Phase A): a deterministic map, populated as units are built.
/// The on-disk content-addressed store (P3.8.1) will be another `Resolver` behind
/// this identical trait — the validator and evaluator never change.
#[derive(Default, Debug)]
pub struct MapResolver {
    units: BTreeMap<Hash, Unit>,
}

impl MapResolver {
    pub fn new() -> Self {
        MapResolver { units: BTreeMap::new() }
    }

    /// Hash a unit, store it under its hash, and return the hash. A composite's
    /// children must already be inserted (so their hashes resolve) before it is
    /// validated or evaluated.
    pub fn insert(&mut self, unit: Unit) -> Result<Hash, EncodeError> {
        let h = hash_unit(&unit)?;
        self.units.insert(h, unit);
        Ok(h)
    }
}

impl Resolver for MapResolver {
    fn resolve(&self, hash: &Hash) -> Option<&Unit> {
        self.units.get(hash)
    }
}