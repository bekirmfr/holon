//! A cell's history (docs/L4s/history-L4.md §10, step 3a): the runs that changed one cell, a
//! collection, or one item of it, newest first, each with the value before and after it.
//!
//! `GET /deployments/<slug>/changes?name=<cell or collection>[&item=<key>][&before=<seq>][&limit=<n>]`.
//! A run's log line keeps the state it began with (`state_before`) and the state it left
//! (`state_after`, a snapshot). Walking the log in order, the value as the last run left it is
//! the value now; a run that left another one changed it. So a run that only read it while
//! another wrote it isn't named: its snapshot already carries the other's write. A value moved
//! between two runs, where neither did it (a restore put a state back, a deploy started afresh),
//! is a change too, named by the events between them, so each value follows from the one before.

use crate::*;

/// The log entries read per request, newest first; `before` reads further back.
const SCAN: usize = 2000;
/// The changes answered, unless `limit` says fewer (or more, up to 200).
const LIMIT: usize = 50;

/// What the history is of.
enum Target {
    Cell(String),
    /// A collection's key in the state, and one item of it.
    Item(String, String),
    /// A whole collection: each change lists the items it changed.
    Collection(String),
}

impl Target {
    fn key(&self) -> &str {
        match self {
            Target::Cell(k) | Target::Item(k, _) | Target::Collection(k) => k,
        }
    }
    /// The value it has, given the state's value for its key.
    fn pick(&self, store: &SharedStore, v: Option<Value>) -> Result<Option<Value>, String> {
        match self {
            Target::Item(_, item) => match root(v.as_ref()) {
                Some(root) => holon_host::collection::get(store, &root, item).map_err(|e| format!("{e:?}")),
                None => Ok(None),
            },
            _ => Ok(v),
        }
    }
}

/// A collection's root, as the state keeps it: its hash, as text.
fn root(v: Option<&Value>) -> Option<Hash> {
    match v? {
        Value::Text(hex) => parse_hash(hex),
        _ => None,
    }
}

pub(crate) fn handle_get_changes(state: &AppState, slug: &str, request: &Request) -> Response {
    let Some(name) = request.get_param("name").filter(|n| !n.trim().is_empty()) else {
        return err(400, "say whose history: ?name=<a cell or a collection>, with &item=<key> for one item of a collection");
    };
    let item = request.get_param("item");
    let before = request.get_param("before").and_then(|s| s.parse::<u64>().ok()).unwrap_or(u64::MAX);
    let limit = request.get_param("limit").and_then(|s| s.parse::<usize>().ok()).unwrap_or(LIMIT).clamp(1, 200);
    let (entries, older, events, redactor, now) = {
        let table = state.deployments.lock().unwrap();
        let Some(dep) = table.get(slug) else { return err(404, format!("no deployment {slug:?}")) };
        let mut entries: Vec<LogEntry> = dep.log.after(None, usize::MAX).into_iter().filter(|e| e.seq < before).collect();
        let more = entries.len() > SCAN;
        entries.drain(..entries.len().saturating_sub(SCAN));
        let live = dep.state.as_ref().map(|arc| arc.state.lock().unwrap().values()).unwrap_or_default();
        (entries, more, dep.events.entries().to_vec(), Redactor::of(&dep.secrets), live)
    };
    let collection = format!("#collection:{name}");
    let is_collection = now.contains_key(&collection) || entries.iter().any(|e| e.state_before.contains_key(&collection));
    let target = match (is_collection, item) {
        (true, Some(item)) => Target::Item(collection, item),
        (true, None) => Target::Collection(collection),
        (false, None) => Target::Cell(name.clone()),
        (false, Some(_)) => return err(400, format!("{name:?} isn't a collection of {slug:?}: an item is one of a collection's")),
    };
    let mut answer = match changes(&state.store, &target, &entries, &events, limit) {
        Ok((list, cut)) => {
            let now = match target.pick(&state.store, now.get(target.key()).cloned()) {
                Ok(v) => v,
                Err(e) => return err(500, format!("its value now can't be read: {e}")),
            };
            serde_json::json!({
                "name": name,
                "item": match &target { Target::Item(_, i) => Some(i), _ => None },
                "collection": is_collection,
                "now": if matches!(target, Target::Collection(_)) { None } else { now.as_ref().map(SValue::from_core) },
                "changes": list,
                // Older changes: before the window read, or past the limit.
                "more": older || cut,
                "scanned_from": entries.first().map(|e| e.seq),
            })
        }
        Err(e) => return err(500, format!("{name:?}'s history can't be read: {e}")),
    };
    redactor.json(&mut answer);
    Response::json(&answer)
}

/// The changes to `target` among `entries` (oldest first), newest first, at most `limit`, and
/// whether there were more.
fn changes(store: &SharedStore, target: &Target, entries: &[LogEntry], events: &[crate::events::Event], limit: usize) -> Result<(Vec<serde_json::Value>, bool), String> {
    let at_start = |e: &LogEntry| -> Result<Option<Value>, String> { target.pick(store, e.state_before.get(target.key()).cloned().map(SValue::into_core)) };
    let Some(first) = entries.first() else { return Ok((Vec::new(), false)) };
    let mut current = at_start(first)?;
    let mut found = Vec::new();
    let mut prev: Option<u64> = None;
    for e in entries {
        // Something other than a run between the last one and this: the state it began with is
        // the value then, if that moved it.
        if let Some(p) = prev {
            // Only these move a state: a grant, a secret or a binding never does.
            let moves_state = |ev: &&crate::events::Event| matches!(ev.kind.as_str(), "restore" | "deploy" | "lifecycle");
            let between: Vec<&crate::events::Event> = events.iter().filter(|ev| ev.run_seq > p && ev.run_seq <= e.seq).filter(moves_state).collect();
            if !between.is_empty() {
                let then = at_start(e)?;
                if then != current {
                    let mut change = serde_json::json!({
                        "between": [p, e.seq],
                        "events": between.iter().map(|ev| serde_json::json!({ "kind": ev.kind, "what": ev.what, "by": ev.by, "at_ms": ev.at_ms })).collect::<Vec<_>>(),
                        "at_ms": between.last().map(|ev| ev.at_ms),
                    });
                    values(store, target, &mut change, current.as_ref(), then.as_ref())?;
                    found.push(change);
                    current = then;
                }
            }
        }
        prev = Some(e.seq);
        let Some(snapshot) = &e.state_after else { continue };
        let refs = crate::restore::snapshot_refs(snapshot, store)?;
        let after = refs.get(target.key()).map(|h| store.get_value(h).map_err(|e| format!("{e:?}"))).transpose()?.flatten();
        let after = target.pick(store, after)?;
        if after != current {
            let mut change = serde_json::json!({
                "seq": e.seq,
                "at_ms": e.at_ms,
                "kind": e.kind,
                "route": e.route,
                "method": e.method,
                "url": e.url,
                "status": e.status,
                "failed": e.error.is_some(),
                "unit": e.unit,
                "by": e.by,
            });
            values(store, target, &mut change, current.as_ref(), after.as_ref())?;
            found.push(change);
            current = after;
        }
    }
    let cut = found.len() > limit;
    found.reverse();
    found.truncate(limit);
    Ok((found, cut))
}

/// A change's values: before and after, or for a whole collection, the items it changed.
fn values(store: &SharedStore, target: &Target, change: &mut serde_json::Value, before: Option<&Value>, after: Option<&Value>) -> Result<(), String> {
    match target {
        Target::Collection(_) => {
            let diff = collection_change(store, after, before)?;
            change["items"] = serde_json::Value::Object(
                diff["items"].as_object().into_iter().flatten().map(|(k, v)| (k.clone(), serde_json::json!({ "before": v["then"], "after": v["now"] }))).collect(),
            );
            change["more_items"] = diff["more"].clone();
        }
        _ => {
            change["before"] = serde_json::json!(before.map(SValue::from_core));
            change["after"] = serde_json::json!(after.map(SValue::from_core));
        }
    }
    Ok(())
}
