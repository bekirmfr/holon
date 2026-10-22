//! A deployment's state as a Merkle map: each key's value, and its content hash, computed once
//! when the value is written. The whole state then hashes from those (`state_root`), cheaply
//! however big the values are, and two states, or two snapshots of one, compare by root.
//!
//! Persisted by reference: [`State::save`] puts each value into the content store and returns
//! the `key → value hash` map, which is all a deployment record or a session snapshot keeps. A
//! value shared by several snapshots is stored once; [`State::load`] rebuilds a state from such
//! a map, verifying every value against its hash.

use std::collections::BTreeMap;

use holon_core::{hash_value, state_root, EncodeError, Hash, Value};

use crate::{Store, StoreError};

/// One key's value and its content hash.
#[derive(Clone, Debug, PartialEq)]
struct Cell {
    value: Value,
    hash: Hash,
}

/// A key → value map that knows each value's hash, and so its own root.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    cells: BTreeMap<String, Cell>,
}

/// Why a state couldn't be rebuilt from its references.
#[derive(Debug)]
pub enum LoadError {
    /// A referenced value isn't in the store.
    Missing { key: String, hash: Hash },
    /// The store failed, or holds bytes that don't match the hash.
    Store(StoreError),
}

impl State {
    pub fn new() -> State {
        State::default()
    }

    /// A state holding `values`, each hashed now.
    pub fn from_values(values: BTreeMap<String, Value>) -> Result<State, EncodeError> {
        let mut state = State::new();
        for (key, value) in values {
            state.insert(key, value)?;
        }
        Ok(state)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.cells.get(key).map(|c| &c.value)
    }

    /// The content hash of `key`'s value.
    pub fn hash_of(&self, key: &str) -> Option<&Hash> {
        self.cells.get(key).map(|c| &c.hash)
    }

    /// Set `key` to `value`, hashing it. Fails only for a value too large to encode (a text,
    /// list or record over the `u32` length budget), leaving the state unchanged.
    pub fn insert(&mut self, key: String, value: Value) -> Result<(), EncodeError> {
        let hash = hash_value(&value)?;
        self.cells.insert(key, Cell { value, hash });
        Ok(())
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.cells.remove(key).map(|c| c.value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.cells.iter().map(|(k, c)| (k, &c.value))
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// A plain copy of the values, e.g. what a run read, for its log entry.
    pub fn values(&self) -> BTreeMap<String, Value> {
        self.cells.iter().map(|(k, c)| (k.clone(), c.value.clone())).collect()
    }

    /// The `key → value hash` map: what persisting or snapshotting this state keeps.
    pub fn refs(&self) -> BTreeMap<String, Hash> {
        self.cells.iter().map(|(k, c)| (k.clone(), c.hash)).collect()
    }

    /// The state's Merkle root: equal for equal states, from the value hashes alone.
    pub fn root(&self) -> Hash {
        state_root(self.cells.iter().map(|(k, c)| (k.as_str(), &c.hash)))
            .expect("every key and hash was encodable when inserted")
    }

    /// Put every value into `store` (a value already there is not written again) and return
    /// the references.
    pub fn save(&self, store: &mut impl Store) -> Result<BTreeMap<String, Hash>, StoreError> {
        for cell in self.cells.values() {
            store.put_value(&cell.value)?;
        }
        Ok(self.refs())
    }

    /// The state `refs` names, each value fetched from `store` and verified against its hash.
    pub fn load(refs: &BTreeMap<String, Hash>, store: &impl Store) -> Result<State, LoadError> {
        let mut cells = BTreeMap::new();
        for (key, hash) in refs {
            let value = store
                .get_value(hash)
                .map_err(LoadError::Store)?
                .ok_or_else(|| LoadError::Missing { key: key.clone(), hash: *hash })?;
            cells.insert(key.clone(), Cell { value, hash: *hash });
        }
        Ok(State { cells })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileStore, MemStore};

    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn todos(items: &[&str]) -> Value {
        Value::List(items.iter().map(|s| txt(s)).collect())
    }

    #[test]
    fn the_root_follows_the_values_and_nothing_else() {
        let mut a = State::new();
        a.insert("todos".into(), todos(&["milk"])).unwrap();
        a.insert("count".into(), Value::Number(1.0)).unwrap();
        let b = State::from_values(BTreeMap::from([
            ("count".to_string(), Value::Number(1.0)),
            ("todos".to_string(), todos(&["milk"])),
        ]))
        .unwrap();
        assert_eq!(a.root(), b.root());
        let before = a.root();
        a.insert("todos".into(), todos(&["milk", "dog"])).unwrap();
        assert_ne!(a.root(), before);
        a.insert("todos".into(), todos(&["milk"])).unwrap();
        assert_eq!(a.root(), before, "back to the same values, back to the same root");
        assert_eq!(a.hash_of("count"), Some(&hash_value(&Value::Number(1.0)).unwrap()));
        assert_ne!(State::new().root(), a.root());
    }

    #[test]
    fn saving_and_loading_round_trips_and_shares_values() {
        let mut store = MemStore::new();
        let mut s = State::new();
        s.insert("todos".into(), todos(&["milk"])).unwrap();
        s.insert("big".into(), txt(&"x".repeat(100_000))).unwrap();
        let first = s.save(&mut store).unwrap();

        // A later state changes one key; the other's reference, and its stored value, are shared.
        s.insert("todos".into(), todos(&["milk", "dog"])).unwrap();
        let second = s.save(&mut store).unwrap();
        assert_eq!(first["big"], second["big"]);
        assert_ne!(first["todos"], second["todos"]);

        let earlier = State::load(&first, &store).unwrap();
        assert_eq!(earlier.get("todos"), Some(&todos(&["milk"])));
        assert_eq!(State::load(&second, &store).unwrap().root(), s.root());

        let mut dangling = first.clone();
        dangling.insert("gone".into(), [9u8; 32]);
        assert!(matches!(State::load(&dangling, &store), Err(LoadError::Missing { .. })));
    }

    #[test]
    fn a_file_store_keeps_values_apart_and_refuses_tampered_ones() {
        let dir = std::env::temp_dir().join(format!("holon-state-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = FileStore::new(&dir).unwrap();
        let mut s = State::new();
        s.insert("todos".into(), todos(&["milk"])).unwrap();
        let refs = s.save(&mut store).unwrap();
        let hex: String = refs["todos"].iter().map(|b| format!("{b:02x}")).collect();
        let path = dir.join("values").join(&hex);
        assert!(path.exists(), "values live under values/");
        assert_eq!(State::load(&refs, &store).unwrap(), s);

        std::fs::write(&path, holon_core::encode_value(&todos(&["beer"])).unwrap()).unwrap();
        assert!(matches!(State::load(&refs, &store), Err(LoadError::Store(StoreError::Corrupt { .. }))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
