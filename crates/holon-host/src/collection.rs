//! A collection: a keyed, ordered set of records, kept in the content store as a radix trie, so
//! that one read loads only the nodes on its key's path and one write stores only those it changes.
//!
//! - **Canonical.** The trie's shape follows from which keys exist and nothing else (every node
//!   but the root holds an item or branches), so equal contents have equal roots, however they
//!   were built. A collection is identified by its root hash, like a state by its state root.
//! - **Persistent.** A write copies the nodes on its key's path and shares every other node with
//!   the version before, so every version stays readable (a snapshot is a root) and costs only
//!   what it changed.
//! - **Ordered.** Keys are text, ordered by code point (as `String`'s order); a listing walks the
//!   trie in that order, from after a key and within a prefix, without visiting the rest.
//!
//! A node is a holon value, `{p: <label>, v?: <item's value hash>, c?: {<first char>: <child
//! hash>}}`, stored with `Store::put_value`; an item's value is stored apart, by its own hash, so
//! equal values are stored once and a node stays small whatever the items hold.

use std::collections::BTreeMap;

use holon_core::{parse_hash, Hash, Value};

use crate::{Store, StoreError};

/// Why a collection couldn't be read or written.
#[derive(Debug)]
pub enum CollectionError {
    Store(StoreError),
    /// A node or value the trie refers to isn't in the store.
    Missing(Hash),
    /// A stored node isn't one.
    Malformed(Hash),
}

impl From<StoreError> for CollectionError {
    fn from(e: StoreError) -> Self {
        CollectionError::Store(e)
    }
}

type Result<T> = std::result::Result<T, CollectionError>;

#[derive(Clone, Debug, Default, PartialEq)]
struct Node {
    /// The key characters this node adds to its parent's; empty only at the root.
    label: String,
    /// The value stored under the key ending here, by hash.
    value: Option<Hash>,
    /// The children, by the first character of their label.
    children: BTreeMap<char, Hash>,
}

fn hex(h: &Hash) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

impl Node {
    fn to_value(&self) -> Value {
        let mut fields = BTreeMap::from([("p".to_string(), Value::Text(self.label.clone()))]);
        if let Some(v) = &self.value {
            fields.insert("v".to_string(), Value::Text(hex(v)));
        }
        if !self.children.is_empty() {
            let children = self.children.iter().map(|(c, h)| (c.to_string(), Value::Text(hex(h)))).collect();
            fields.insert("c".to_string(), Value::Record(children));
        }
        Value::Record(fields)
    }

    fn from_value(v: &Value) -> Option<Node> {
        let Value::Record(fields) = v else { return None };
        let Some(Value::Text(label)) = fields.get("p") else { return None };
        let value = match fields.get("v") {
            None => None,
            Some(Value::Text(h)) => Some(parse_hash(h)?),
            Some(_) => return None,
        };
        let children = match fields.get("c") {
            None => BTreeMap::new(),
            Some(Value::Record(c)) => c
                .iter()
                .map(|(k, h)| {
                    let mut chars = k.chars();
                    let (Some(first), None, Value::Text(h)) = (chars.next(), chars.next(), h) else { return None };
                    Some((first, parse_hash(h)?))
                })
                .collect::<Option<_>>()?,
            Some(_) => return None,
        };
        Some(Node { label: label.clone(), value, children })
    }
}

fn load(store: &impl Store, h: &Hash) -> Result<Node> {
    let v = store.get_value(h)?.ok_or(CollectionError::Missing(*h))?;
    Node::from_value(&v).ok_or(CollectionError::Malformed(*h))
}

fn save(store: &mut impl Store, node: &Node) -> Result<Hash> {
    Ok(store.put_value(&node.to_value())?)
}

/// The first character of `s`, which callers only ask of a non-empty string.
fn first(s: &str) -> char {
    s.chars().next().expect("a non-empty key part")
}

/// How many leading bytes `a` and `b` share, at a character boundary.
fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices().zip(b.chars()).take_while(|((_, x), y)| x == y).map(|((i, x), _)| i + x.len_utf8()).last().unwrap_or(0)
}

/// The root of an empty collection.
pub fn empty(store: &mut impl Store) -> Result<Hash> {
    save(store, &Node::default())
}

/// The value under `key`, if there is one.
pub fn get(store: &impl Store, root: &Hash, key: &str) -> Result<Option<Value>> {
    match item_hash(store, root, key)? {
        Some(h) => Ok(Some(store.get_value(&h)?.ok_or(CollectionError::Missing(h))?)),
        None => Ok(None),
    }
}

/// The hash of the value under `key`, if there is one: what tells whether an item changed.
pub fn item_hash(store: &impl Store, root: &Hash, key: &str) -> Result<Option<Hash>> {
    let mut node = load(store, root)?;
    let mut rest = key;
    loop {
        let Some(after) = rest.strip_prefix(node.label.as_str()) else { return Ok(None) };
        if after.is_empty() {
            return Ok(node.value);
        }
        let Some(child) = node.children.get(&first(after)) else { return Ok(None) };
        node = load(store, child)?;
        rest = after;
    }
}

/// The collection with `key` set to `value`; returns its root.
pub fn put(store: &mut impl Store, root: &Hash, key: &str, value: &Value) -> Result<Hash> {
    let value = store.put_value(value)?;
    let node = load(store, root)?;
    let node = put_at(store, node, key, value)?;
    save(store, &node)
}

/// `node`, with the item `rest` (which starts where `node`'s label does) set to `value`.
fn put_at(store: &mut impl Store, mut node: Node, rest: &str, value: Hash) -> Result<Node> {
    let common = common_prefix(&node.label, rest);
    if common < node.label.len() {
        // The key leaves this node's label part way: split it at the fork.
        let lower = Node { label: node.label[common..].to_string(), ..node.clone() };
        let mut fork = Node { label: node.label[..common].to_string(), value: None, children: BTreeMap::new() };
        fork.children.insert(first(&lower.label), save(store, &lower)?);
        let tail = &rest[common..];
        if tail.is_empty() {
            fork.value = Some(value);
        } else {
            let leaf = Node { label: tail.to_string(), value: Some(value), children: BTreeMap::new() };
            fork.children.insert(first(tail), save(store, &leaf)?);
        }
        return Ok(fork);
    }
    let tail = &rest[common..];
    if tail.is_empty() {
        node.value = Some(value);
        return Ok(node);
    }
    let c = first(tail);
    let child = match node.children.get(&c) {
        Some(h) => {
            let child = load(store, h)?;
            put_at(store, child, tail, value)?
        }
        None => Node { label: tail.to_string(), value: Some(value), children: BTreeMap::new() },
    };
    node.children.insert(c, save(store, &child)?);
    Ok(node)
}

/// The collection without `key`; returns its root (the same root when there was no such item).
pub fn delete(store: &mut impl Store, root: &Hash, key: &str) -> Result<Hash> {
    let node = load(store, root)?;
    match delete_at(store, node, key, true)? {
        Removed::Unchanged => Ok(*root),
        Removed::Kept(node) => save(store, &node),
        Removed::Gone => empty(store),
    }
}

enum Removed {
    Unchanged,
    Kept(Node),
    Gone,
}

fn delete_at(store: &mut impl Store, mut node: Node, rest: &str, root: bool) -> Result<Removed> {
    let Some(tail) = rest.strip_prefix(node.label.as_str()) else { return Ok(Removed::Unchanged) };
    if tail.is_empty() {
        if node.value.take().is_none() {
            return Ok(Removed::Unchanged);
        }
    } else {
        let c = first(tail);
        let Some(h) = node.children.get(&c).copied() else { return Ok(Removed::Unchanged) };
        let child = load(store, &h)?;
        match delete_at(store, child, tail, false)? {
            Removed::Unchanged => return Ok(Removed::Unchanged),
            Removed::Gone => {
                node.children.remove(&c);
            }
            Removed::Kept(child) => {
                node.children.insert(c, save(store, &child)?);
            }
        }
    }
    // Keep the trie canonical: a node that neither holds an item nor branches goes, or is merged
    // into its one child. The root stays, labelled "", whatever it holds.
    if !root && node.value.is_none() {
        match node.children.len() {
            0 => return Ok(Removed::Gone),
            1 => {
                let (_, only) = node.children.pop_first().expect("one child");
                let mut child = load(store, &only)?;
                child.label = format!("{}{}", node.label, child.label);
                return Ok(Removed::Kept(child));
            }
            _ => {}
        }
    }
    Ok(Removed::Kept(node))
}

/// Up to `limit` items in key order whose keys start with `prefix` and come after `after` (when
/// given): a page of the collection, and where the next page starts (its last key).
pub fn list(store: &impl Store, root: &Hash, prefix: &str, after: Option<&str>, limit: usize) -> Result<Vec<(String, Value)>> {
    let mut out = Vec::new();
    let node = load(store, root)?;
    walk(store, node, String::new(), prefix, after, limit, &mut out)?;
    Ok(out)
}

fn walk(
    store: &impl Store,
    node: Node,
    mut key: String,
    prefix: &str,
    after: Option<&str>,
    limit: usize,
    out: &mut Vec<(String, Value)>,
) -> Result<()> {
    key.push_str(&node.label);
    // Every key below starts with `key`: skip the whole subtree when that rules it out.
    if !(key.starts_with(prefix) || prefix.starts_with(key.as_str())) {
        return Ok(());
    }
    if let Some(after) = after
        && key.as_str() < after
        && !after.starts_with(key.as_str())
    {
        return Ok(());
    }
    if let Some(v) = node.value
        && key.starts_with(prefix)
        && after.is_none_or(|a| key.as_str() > a)
    {
        if out.len() >= limit {
            return Ok(());
        }
        out.push((key.clone(), store.get_value(&v)?.ok_or(CollectionError::Missing(v))?));
    }
    for h in node.children.values() {
        if out.len() >= limit {
            break;
        }
        walk(store, load(store, h)?, key.clone(), prefix, after, limit, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemStore;

    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }

    /// A tiny deterministic shuffle, so the tests need no random source.
    fn shuffled(mut keys: Vec<String>, seed: u64) -> Vec<String> {
        let mut s = seed;
        for i in (1..keys.len()).rev() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            keys.swap(i, (s >> 33) as usize % (i + 1));
        }
        keys
    }

    fn build(store: &mut MemStore, keys: &[String]) -> Hash {
        let mut root = empty(store).unwrap();
        for k in keys {
            root = put(store, &root, k, &txt(&format!("value of {k}"))).unwrap();
        }
        root
    }

    fn sample_keys() -> Vec<String> {
        let mut keys: Vec<String> = ["", "a", "ab", "abc", "abd", "b", "ba", "note:1", "note:10", "note:2", "nöte", "日本", "日本語"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        keys.extend((0..200).map(|i| format!("item-{i:03}")));
        keys
    }

    #[test]
    fn equal_contents_have_equal_roots_whatever_the_order() {
        let mut store = MemStore::new();
        let keys = sample_keys();
        let root = build(&mut store, &keys);
        for seed in 1..6 {
            assert_eq!(build(&mut store, &shuffled(keys.clone(), seed)), root, "insertion order {seed}");
        }
        // Built up and torn down to the same set: the same root again.
        let mut other = build(&mut store, &[keys.clone(), vec!["extra".into(), "item-0999".into(), "a-b".into()]].concat());
        for k in ["extra", "item-0999", "a-b"] {
            other = delete(&mut store, &other, k).unwrap();
        }
        assert_eq!(other, root);
        assert_eq!(delete(&mut store, &root, "absent").unwrap(), root, "deleting nothing changes nothing");
        // Emptied, it is the empty collection.
        let mut emptied = root;
        for k in shuffled(keys, 9) {
            emptied = delete(&mut store, &emptied, &k).unwrap();
        }
        assert_eq!(emptied, empty(&mut store).unwrap());
    }

    #[test]
    fn items_are_got_set_replaced_and_deleted() {
        let mut store = MemStore::new();
        let keys = sample_keys();
        let root = build(&mut store, &keys);
        for k in &keys {
            assert_eq!(get(&store, &root, k).unwrap(), Some(txt(&format!("value of {k}"))), "{k:?}");
        }
        for absent in ["abcd", "note:", "item-1000", "日", "c", "nö"] {
            assert_eq!(get(&store, &root, absent).unwrap(), None, "{absent:?}");
        }
        let replaced = put(&mut store, &root, "ab", &txt("new")).unwrap();
        assert_eq!(get(&store, &replaced, "ab").unwrap(), Some(txt("new")));
        assert_eq!(get(&store, &root, "ab").unwrap(), Some(txt("value of ab")), "the version before is untouched");
        let gone = delete(&mut store, &replaced, "ab").unwrap();
        assert_eq!(get(&store, &gone, "ab").unwrap(), None);
        assert_eq!(get(&store, &gone, "abc").unwrap(), Some(txt("value of abc")), "its neighbours stay");
        assert_eq!(item_hash(&store, &gone, "abd").unwrap(), Some(holon_core::hash_value(&txt("value of abd")).unwrap()));
    }

    #[test]
    fn listing_is_in_key_order_by_page_and_prefix() {
        let mut store = MemStore::new();
        let keys = sample_keys();
        let root = build(&mut store, &shuffled(keys.clone(), 3));
        let mut sorted = keys.clone();
        sorted.sort();
        let all: Vec<String> = list(&store, &root, "", None, usize::MAX).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(all, sorted);

        // Page by page, each starting after the last key of the one before.
        let mut paged = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = list(&store, &root, "", after.as_deref(), 7).unwrap();
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= 7);
            after = Some(page.last().unwrap().0.clone());
            paged.extend(page.into_iter().map(|(k, _)| k));
        }
        assert_eq!(paged, sorted);

        let notes: Vec<String> = list(&store, &root, "note:", None, 10).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(notes, ["note:1", "note:10", "note:2"]);
        let after_ab: Vec<String> = list(&store, &root, "a", Some("ab"), 10).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(after_ab, ["abc", "abd"]);
        assert_eq!(list(&store, &root, "zzz", None, 10).unwrap(), vec![]);
        assert_eq!(list(&store, &root, "item-1", None, 3).unwrap().len(), 3, "limit stops the walk");
    }

    #[test]
    fn a_write_stores_only_the_nodes_on_its_path() {
        let mut store = MemStore::new();
        let keys: Vec<String> = (0..5000).map(|i| format!("user-{i:05}")).collect();
        let root = build(&mut store, &keys);
        let before = store.value_count();
        let next = put(&mut store, &root, "user-02500", &txt("changed")).unwrap();
        let written = store.value_count() - before;
        assert!(written <= 8, "a write in 5000 items stored {written} new values");
        assert_eq!(get(&store, &next, "user-02500").unwrap(), Some(txt("changed")));
        assert_eq!(get(&store, &next, "user-04999").unwrap(), Some(txt("value of user-04999")));
    }
}
