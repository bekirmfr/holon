//! A deployment's collections (holon-host's `collection`): what a run reads, and what it writes.
//!
//! - **Where they live.** A collection's root hash is a cell of the deployment's state under a
//!   reserved key, `#collection:<name>` (core refuses state keys starting with `#`, so no flow can
//!   touch one). The trie's nodes and items are in the content store. So a collection is
//!   persisted, snapshotted with each session, part of a run's id (the state root), and in the
//!   log's state before, exactly like the rest of the state, at the cost of one small cell.
//! - **Reads** are answered on the spot from the collection as the run found it, only for
//!   collections the deployment was granted, and each read is kept (with its node's path): the log
//!   shows it, replay answers from it, and a run is checked against it before it commits.
//! - **Writes** (puts and deletes) are effects, applied to the live collection when the run
//!   commits; a value carrying an effect or a secret is refused, as for a state write.

use holon_host::collection;

use crate::*;

/// The reserved state key holding collection `name`'s root.
pub(crate) fn cell_key(name: &str) -> String {
    format!("#collection:{name}")
}

/// The roots of the collections in `state`, by name.
pub(crate) fn roots(state: &State) -> BTreeMap<String, Hash> {
    state
        .iter()
        .filter_map(|(k, v)| {
            let name = k.strip_prefix("#collection:")?;
            let Value::Text(hex) = v else { return None };
            Some((name.to_string(), parse_hash(hex)?))
        })
        .collect()
}

/// The collections one run reads: their roots as it found them, the store, and which it may read.
pub(crate) struct Readable {
    pub(crate) store: SharedStore,
    pub(crate) roots: BTreeMap<String, Hash>,
    pub(crate) granted: BTreeSet<String>,
}

impl std::fmt::Debug for Readable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Readable").field("roots", &self.roots).field("granted", &self.granted).finish()
    }
}

/// One read a run made, by the node that made it, and what it got.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Read {
    pub(crate) path: Vec<u32>,
    pub(crate) request: Value,
    pub(crate) answer: Option<Value>,
}

fn text<'v>(r: &'v BTreeMap<String, Value>, k: &str) -> Option<&'v str> {
    match r.get(k) {
        Some(Value::Text(t)) => Some(t),
        _ => None,
    }
}

impl Readable {
    /// The answer to `request` (`{collection, get}` or `{collection, list}`): absent for a missing
    /// item, an ungranted collection or a malformed request; an empty record for a listing of a
    /// collection that has no items yet.
    pub(crate) fn answer(&self, request: &Value) -> Option<Value> {
        let Value::Record(r) = request else { return None };
        let name = text(r, "collection")?;
        if !self.granted.contains(name) {
            eprintln!("holon-server: refused to read collection {name:?}: not granted to this deployment");
            return None;
        }
        if let Some(key) = text(r, "get") {
            let root = self.roots.get(name)?;
            return collection::get(&self.store, root, key).unwrap_or_else(|e| {
                eprintln!("holon-server: collection {name:?} can't be read: {e:?}");
                None
            });
        }
        let Some(Value::Record(list)) = r.get("list") else { return None };
        let Some(root) = self.roots.get(name) else { return Some(Value::Record(BTreeMap::new())) };
        let limit = match list.get("limit") {
            Some(Value::Number(n)) => *n as usize,
            _ => return None,
        };
        match collection::list(&self.store, root, text(list, "prefix").unwrap_or(""), text(list, "after"), limit) {
            Ok(items) => Some(Value::Record(items.into_iter().collect())),
            Err(e) => {
                eprintln!("holon-server: collection {name:?} can't be listed: {e:?}");
                None
            }
        }
    }
}

/// Apply a performed `CollectionWrite(name)` (a put's `{key, value}`, or a delete's key) to
/// `state`'s collection `name`, storing the changed nodes. Refuses (and reports) a value that
/// isn't storable data.
pub(crate) fn apply(state: &mut State, store: &mut SharedStore, name: &str, payload: &Value) -> Result<(), String> {
    let cell = cell_key(name);
    let root = match state.get(&cell) {
        Some(Value::Text(hex)) => parse_hash(hex).ok_or("its root isn't a hash")?,
        _ => collection::empty(store).map_err(|e| format!("{e:?}"))?,
    };
    let next = match payload {
        Value::Record(item) => {
            let (Some(Value::Text(key)), Some(value)) = (item.get("key"), item.get("value")) else {
                return Err("a put needs a key and a value".into());
            };
            if !is_storable(value) {
                return Err(format!("refused to store an effect request under {key:?}"));
            }
            collection::put(store, &root, key, value)
        }
        Value::Text(key) => collection::delete(store, &root, key),
        _ => return Err("not a put or a delete".into()),
    }
    .map_err(|e| format!("{e:?}"))?;
    state.insert(cell, Value::Text(to_hex(&next))).map_err(|e| format!("{e:?}"))
}

/// `GET /deployments/<slug>/collections/<name>`: a collection's items, for the editor's Flow state
/// pane. `?key=` gets one item; otherwise a page in key order (`prefix`, `after`, `limit`: 50 by
/// default, at most 1000) with `next`, the key to continue after when there may be more. `?root=`
/// reads the collection as it was at that root (as a logged run found it), and is accepted only
/// when it is the live root or one this deployment's log shows: a view of its own history, not a
/// way to read anything stored. Values are redacted like the rest of the control plane.
pub(crate) fn handle_get_collection(state: &AppState, slug: &str, name: &str, request: &Request) -> Response {
    let cell = cell_key(name);
    let asked = request.get_param("root");
    let (root, redactor) = {
        let table = state.deployments.lock().unwrap();
        let Some(dep) = table.get(slug) else { return err(404, format!("no deployment {slug:?}")) };
        let live = dep.state.as_ref().and_then(|s| match s.lock().unwrap().get(&cell) {
            Some(Value::Text(hex)) => parse_hash(hex),
            _ => None,
        });
        let root = match &asked {
            None => match live {
                Some(r) => r,
                None => return err(404, format!("{slug:?} has no collection {name:?} yet")),
            },
            Some(hex) => {
                let Some(r) = parse_hash(hex) else { return err(400, "root is not a 64-char hex hash") };
                if live != Some(r) && !dep.log.shows(&cell, hex) {
                    return err(404, format!("{name:?} was never at that root in {slug:?}'s log"));
                }
                r
            }
        };
        (root, Redactor::of(&dep.secrets))
    };
    let item = |(k, v): (String, Value)| serde_json::json!([k, SValue::from_core(&v)]);
    let mut reply = if let Some(key) = request.get_param("key") {
        match collection::get(&state.store, &root, &key) {
            Ok(v) => serde_json::json!({ "root": to_hex(&root), "items": v.map(|v| item((key, v))).into_iter().collect::<Vec<_>>() }),
            Err(e) => return err(500, format!("{e:?}")),
        }
    } else {
        let limit = request.get_param("limit").and_then(|l| l.parse().ok()).unwrap_or(50usize).clamp(1, 1000);
        let prefix = request.get_param("prefix").unwrap_or_default();
        let after = request.get_param("after");
        match collection::list(&state.store, &root, &prefix, after.as_deref(), limit) {
            Ok(items) => {
                let next = (items.len() == limit).then(|| items.last().map(|(k, _)| k.clone())).flatten();
                serde_json::json!({ "root": to_hex(&root), "items": items.into_iter().map(item).collect::<Vec<_>>(), "next": next })
            }
            Err(e) => return err(500, format!("{e:?}")),
        }
    };
    redactor.json(&mut reply);
    Response::json(&reply)
}
