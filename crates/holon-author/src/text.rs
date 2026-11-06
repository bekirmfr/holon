//! A flow document's own units and names put back where they are read: a saved document carries
//! the units its composites and modules are (`units`, `modules`) and the names it gave them
//! (`names`), so a document opened anywhere resolves without the workspace that made it.

use holon_core::{decode_preimage, MapResolver};

use crate::builder::FlowDocument;
use crate::composites::Names;

/// The units a document carries put into `store` (one that can't be read is skipped: a preimage
/// is re-hashed on insert, so a tampered entry can only add a unit nobody points at), and the
/// names its composites go by: `known` ones first, then the document's.
pub fn restore(doc: &FlowDocument, store: &mut MapResolver, known: &Names) -> Names {
    for hex in doc.units.iter().flatten() {
        let bytes: Option<Vec<u8>> = (hex.len() % 2 == 0).then(|| (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()).flatten();
        if let Some(unit) = bytes.and_then(|b| decode_preimage(&b).ok()) {
            let _ = store.insert(unit);
        }
    }
    // Its modules, rebuilt from the engines this build carries (one that doesn't rebuild to its
    // hash stays out, like a preimage that doesn't decode).
    if let Some(modules) = &doc.modules {
        let _ = crate::modules::restore_modules(modules, store);
    }
    let mut names = known.clone();
    for (hash, name) in doc.names.iter().flatten() {
        if !names.contains_key(hash) && !name.is_empty() && hash.get(..8) != Some(name.as_str()) {
            names.insert(hash.clone(), name.clone());
        }
    }
    names
}
