//! Saving and loading deployments and flows: atomic writes, and refusing to start on a file that
//! won't parse.

use crate::*;

/// A slug must be non-empty ASCII alphanumeric/`-`/`_` only — keeps the simple prefix-based
/// router in `route()` unambiguous (no `/` inside a slug could be confused with a path
/// separator) and avoids any path-traversal-shaped weirdness reaching `strip_prefix`.
/// Write the whole current deployments table to `path` as a JSON array of `DeploymentRecord`
/// (write-through — called after every successful deploy/delete, not batched or scheduled).
/// A no-op for the `":memory:"` sentinel. Failures are logged, not fatal — a person's server
/// keeps working even if the disk write failed; they just won't survive a restart until it
/// succeeds again, which is the honest degradation for something that was opt-in persistence
/// to begin with.
///
/// Takes the table's mutex rather than a locked table: the table lock is held only long enough
/// to copy each deployment's fields and clone its state `Arc`. Each state map is then locked on
/// its own, and the disk write happens with no lock held but `PERSIST`. Before, the table lock
/// was held throughout, so one deployment mid-fire (holding its state lock) or one slow disk
/// write stalled every endpoint and the clock scheduler. `PERSIST` serializes whole
/// snapshot-and-write passes, so an older snapshot can never land on disk after a newer one.
pub(crate) fn persist_deployments(path: &str, deployments: &Mutex<BTreeMap<String, Deployment>>) {
    static PERSIST: Mutex<()> = Mutex::new(());
    if path == ":memory:" {
        return;
    }
    let _persist = PERSIST.lock().unwrap();
    let snapshot: Vec<(DeploymentRecord, Option<DeploymentState>)> = deployments
        .lock()
        .unwrap()
        .iter()
        .map(|(slug, dep)| {
            let record = DeploymentRecord {
                slug: slug.clone(),
                unit: to_hex(&dep.unit),
                interval_seconds: dep.interval_seconds,
                state: None,
                deployed_at_epoch_seconds: dep
                    .deployed_at
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs()),
                trigger_count: dep.trigger_count,
                error_count: dep.error_count,
                status: Some(dep.life.status),
                sessions: dep.life.sessions.clone(),
                document: dep.document.clone(),
            };
            (record, dep.state.clone())
        })
        .collect();
    let records: Vec<DeploymentRecord> = snapshot
        .into_iter()
        .map(|(mut record, state)| {
            record.state = state.map(|arc| {
                let storage = arc.lock().unwrap();
                storage.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect()
            });
            record
        })
        .collect();
    match serde_json::to_vec_pretty(&records) {
        Ok(bytes) => {
            if let Err(e) = write_atomically(Path::new(path), &bytes) {
                eprintln!("warning: failed to persist deployments to {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to serialize deployments: {e}"),
    }
}

/// Load deployments from `path` at startup — the inverse of `persist_deployments`. Returns an
/// empty table for the `":memory:"` sentinel or a missing file (first run). A file that exists
/// but can't be read or parsed is an `Err`, and the server refuses to start: starting empty
/// would let the first deploy overwrite the file — every deployment and all its state — with
/// the empty table. Each record is re-materialized against the
/// **current** store and re-derives its `triggers`/`state`-eligibility fresh via
/// `capabilities()` — exactly what a live `/deploy` call would compute, not a cached copy that
/// could go stale. A record that can't be loaded (a bad hash, a store error, or a unit no
/// longer in the store, e.g. `--store :memory:` against a persisted file) is also an `Err`:
/// skipping it would let the next save drop it and its state for good. Persisted state is
/// restored whenever the record has some, even for a unit that no longer derives a state
/// capability, matching `/deploy`, which keeps a slug's state across a stateless version; a
/// state-capable unit with no persisted state (an old file predating this feature) starts from
/// an empty map, same as a fresh `/deploy`.
pub(crate) fn load_deployments(path: &str, store: &SharedStore) -> Result<BTreeMap<String, Deployment>, String> {
    let mut loaded = BTreeMap::new();
    if path == ":memory:" {
        return Ok(loaded);
    }

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(loaded),
        Err(e) => return Err(format!("could not read {path:?}: {e}")),
    };
    let records: Vec<DeploymentRecord> = serde_json::from_slice(&bytes)
        .map_err(|e| format!("{path:?} is not valid deployment JSON: {e}"))?;

    for record in records {
        let slug = &record.slug;
        let Some(unit_hash) = parse_hash(&record.unit) else {
            return Err(format!("deployment {slug:?} in {path:?}: {:?} is not a valid hash", record.unit));
        };
        let resolver = materialize_closure(store, unit_hash)
            .map_err(|e| format!("deployment {slug:?} in {path:?}: store error: {e:?}"))?;
        let Some(root_unit) = resolver.resolve(&unit_hash) else {
            return Err(format!(
                "deployment {slug:?} in {path:?}: unit {} is not in the store. Start with the --store it \
                 was deployed into, run with --deployments :memory:, or move {path:?} aside",
                record.unit
            ));
        };
        let caps = capabilities(root_unit, &resolver);
        let server = match declaration_of(root_unit) {
            Some("server") => Some(
                read_server(root_unit, &resolver)
                    .map_err(|e| format!("deployment {slug:?} in {path:?}: not a valid server declaration: {e:?}"))?,
            ),
            _ => None,
        };
        let triggers = trigger_labels(
            server.as_ref(),
            caps.contains(Capability::NetworkIngress),
            caps.contains(Capability::Clock) && server.is_none(),
        );
        let next_due = record.interval_seconds.filter(|_| server.is_none()).map(|_| SystemTime::now());

        let state = match record.state {
            Some(persisted) => {
                let map = persisted.into_iter().map(|(k, v)| (k, SValue::into_core(v))).collect();
                Some(Arc::new(Mutex::new(map)))
            }
            None if derives_state(caps.iter()) => Some(Arc::new(Mutex::new(BTreeMap::new()))),
            None => None,
        };
        let deployed_at = record
            .deployed_at_epoch_seconds
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap_or_else(SystemTime::now);

        println!("loaded deployment {:?} from {path:?} (triggers: {triggers:?})", record.slug);
        loaded.insert(
            record.slug,
            Deployment {
                unit: unit_hash,
                triggers,
                interval_seconds: record.interval_seconds,
                next_due,
                history: Vec::new(),
                state,
                server,
                life: {
                    let started = deployed_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                    let mut life = Lifecycle { status: record.status.unwrap_or(Status::Running), sessions: record.sessions };
                    if life.sessions.is_empty() {
                        life.sessions = Lifecycle::deployed(started).sessions;
                    }
                    life
                },
                log: Log::default(),
                document: record.document,
                trigger_count: record.trigger_count,
                error_count: record.error_count,
                deployed_at,
            },
        );
    }
    Ok(loaded)
}

/// Persist the flows table — same write-through discipline and `":memory:"` opt-out as
/// `persist_deployments`, but simpler: a `FlowRecord` is already its own on-disk shape (no
/// store cross-references, no re-derivation needed on the way back in).
pub(crate) fn persist_flows(path: &str, flows: &BTreeMap<String, FlowRecord>) {
    if path == ":memory:" {
        return;
    }
    let records: Vec<&FlowRecord> = flows.values().collect();
    match serde_json::to_vec_pretty(&records) {
        Ok(bytes) => {
            if let Err(e) = write_atomically(Path::new(path), &bytes) {
                eprintln!("warning: failed to persist flows to {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to serialize flows: {e}"),
    }
}

/// Load flows at startup — the inverse of `persist_flows`. A missing file (first run) or
/// `":memory:"` yields an empty table; an unreadable or unparseable file is an `Err` and the
/// server refuses to start, for the same reason as `load_deployments`: the next save would
/// overwrite every flow with the empty table. A record whose name fails `is_valid_slug` is an
/// `Err` too, for the same reason.
pub(crate) fn load_flows(path: &str) -> Result<BTreeMap<String, FlowRecord>, String> {
    if path == ":memory:" {
        return Ok(BTreeMap::new());
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(format!("could not read {path:?}: {e}")),
    };
    let records: Vec<FlowRecord> =
        serde_json::from_slice(&bytes).map_err(|e| format!("{path:?} is not valid flows JSON: {e}"))?;
    let mut loaded = BTreeMap::new();
    for record in records {
        if !is_valid_slug(&record.name) {
            // Skipping it would let the next save drop it; refuse instead, like a parse error.
            return Err(format!("{path:?} has a flow with an invalid name: {:?}", record.name));
        }
        loaded.insert(record.name.clone(), record);
    }
    Ok(loaded)
}
