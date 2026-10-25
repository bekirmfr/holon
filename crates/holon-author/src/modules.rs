//! Module units as a flow document carries them (docs/L4s/js-node-L4.md §3): by **engine and
//! params**, not by preimage. A module's preimage is its whole binary (the js engine's is 3.2 MB,
//! 6.5 MB as hex), and a budget is a param of the module unit, so each distinct budget would be
//! another copy in every saved flow, deploy body and deployment entry. Instead a document names
//! the engine (`"js"`) and the params, and whoever restores the document rebuilds the unit from
//! the engine bytes it already has (`holon_module::JS_ENGINE`, in every build that lowers text)
//! and checks the hash it was told. A module nobody here knows stays a preimage in `units`.

use holon_core::{hash_unit, MapResolver, Port, Type, Unit, UnitBody};
use serde::{Deserialize, Serialize};

use crate::to_hex;
use crate::wire::WParam;

/// The JavaScript engine module's name in a document.
pub const JS_ENGINE: &str = "js";

/// A module unit in a document: its hash (what the composites placing it reference), the engine it
/// is, and its params (the budgets), from which the unit is rebuilt.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModuleEntry {
    pub hash: String,
    pub engine: String,
    pub params: Vec<WParam>,
}

/// The engine `name`'s unit under `params`: its bytes, its interface, the params as given. None
/// for an engine this build doesn't carry.
pub fn engine_unit(name: &str, params: &[WParam]) -> Option<Unit> {
    if name != JS_ENGINE {
        return None;
    }
    Some(Unit {
        body: UnitBody::Module { bytes: holon_module::JS_ENGINE.to_vec() },
        params: params.iter().cloned().map(WParam::into_core).collect(),
        inputs: vec![Port { name: "source".into(), ty: Type::Text }, Port { name: "args".into(), ty: Type::list_any() }],
        outputs: vec![Port { name: "result".into(), ty: Type::Record }],
    })
}

/// The js engine under `fuel` and `memory` (a js node's defaults when not given).
pub fn js_engine_unit(fuel: Option<u64>, memory: Option<u64>) -> Unit {
    use holon_core::{FUEL_PARAM, MEMORY_PARAM};
    use holon_module::{JS_DEFAULT_FUEL, JS_DEFAULT_MEMORY};
    let params = [
        WParam::frozen(FUEL_PARAM, &fuel.unwrap_or(JS_DEFAULT_FUEL).to_string()),
        WParam::frozen(MEMORY_PARAM, &memory.unwrap_or(JS_DEFAULT_MEMORY).to_string()),
    ];
    engine_unit(JS_ENGINE, &params).expect("the js engine is in every build")
}

/// `unit` as a document entry, when it is a module of an engine this build knows (its bytes are
/// that engine's); `hash` is its hash as the caller has it.
pub fn entry_of(unit: &Unit, hash: &str) -> Option<ModuleEntry> {
    let UnitBody::Module { bytes } = &unit.body else { return None };
    if bytes.as_slice() != holon_module::JS_ENGINE {
        return None;
    }
    Some(ModuleEntry { hash: hash.to_string(), engine: JS_ENGINE.into(), params: unit.params.iter().map(WParam::from_core).collect() })
}

/// The entries' units rebuilt into `store`, each checked against the hash the document gave:
/// an entry whose hash doesn't come out (another engine build, a tampered entry) is left out and
/// named. Answers what couldn't be restored, or nothing.
pub fn restore_modules(entries: &[ModuleEntry], store: &mut MapResolver) -> Option<String> {
    let mut failed = Vec::new();
    for e in entries {
        let Some(unit) = engine_unit(&e.engine, &e.params) else {
            failed.push(format!("no engine \"{}\" in this build", e.engine));
            continue;
        };
        match hash_unit(&unit) {
            Ok(h) if to_hex(&h) == e.hash => {
                let _ = store.insert(unit);
            }
            Ok(h) => failed.push(format!("the module {} rebuilds as {}: another {} engine", &e.hash[..12.min(e.hash.len())], &to_hex(&h)[..12], e.engine)),
            Err(err) => failed.push(format!("the module {} doesn't hash: {err:?}", &e.hash[..12.min(e.hash.len())])),
        }
    }
    (!failed.is_empty()).then(|| failed.join("; "))
}

/// Whether `p` is a module's param as a document writes one (used by nothing but tests today).
#[cfg(test)]
pub(crate) fn is_budget(p: &holon_core::Param) -> bool {
    matches!(p.name(), holon_core::FUEL_PARAM | holon_core::MEMORY_PARAM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_core::Resolver;

    #[test]
    fn an_entry_rebuilds_the_unit_it_came_from_and_only_that() {
        let unit = js_engine_unit(Some(50_000_000), None);
        assert!(unit.params.iter().all(is_budget));
        let hash = to_hex(&hash_unit(&unit).unwrap());
        let entry = entry_of(&unit, &hash).unwrap();
        assert_eq!((entry.engine.as_str(), entry.params.len()), (JS_ENGINE, 2));
        let mut store = MapResolver::new();
        assert_eq!(restore_modules(&[entry.clone()], &mut store), None);
        assert_eq!(store.resolve(&hash_unit(&unit).unwrap()).map(|u| u.params.clone()), Some(unit.params.clone()));
        // Another budget under the same hash: refused, named; an unknown engine: named.
        let mut wrong = entry.clone();
        wrong.params[0] = WParam::frozen(holon_core::FUEL_PARAM, "1");
        let why = restore_modules(&[wrong, ModuleEntry { hash: "ab".repeat(32), engine: "lua".into(), params: vec![] }], &mut MapResolver::new()).unwrap();
        assert!(why.contains("another js engine") && why.contains("no engine \"lua\""), "{why}");
        // A module whose bytes aren't an engine this build knows is no entry.
        let other = Unit { body: UnitBody::Module { bytes: b"\0asm\x01\0\0\0".to_vec() }, ..unit.clone() };
        assert!(entry_of(&other, &hash).is_none());
    }
}
