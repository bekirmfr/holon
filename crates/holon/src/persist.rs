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
pub(crate) fn persist_deployments(path: &str, deployments: &Mutex<BTreeMap<String, Deployment>>, store: &SharedStore) {
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
                state_refs: None,
                state: None,
                deployed_at_epoch_seconds: dep
                    .deployed_at
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs()),
                deployed_by: dep.deployed_by.clone(),
                uses: dep.uses.clone(),
                browsers: dep.browsers.clone(),
                trigger_count: dep.trigger_count,
                error_count: dep.error_count,
                status: Some(dep.life.status),
                sessions: dep.life.sessions.clone(),
                document: dep.document.clone(),
                versions: Vec::new(),
                versions_kept: dep.versions.clone(),
                grant: Some(dep.grant.clone()),
                // What a waiting run keeps that may hold a secret is sealed under the secrets
                // master key, which is kept apart from this file: a backup of it carries none.
                parked: {
                    let sealer = dep.secrets.lock().unwrap().sealer.clone();
                    dep.parked.values().map(|p| sealer.as_deref().map_or_else(|| p.clone(), |s| p.sealed_for_disk(slug, s))).collect()
                },
            };
            (record, dep.state.clone())
        })
        .collect();
    // Each state's values go into the content store (one already there isn't written again);
    // the record keeps their references.
    let mut values = store.clone();
    let mut records: Vec<DeploymentRecord> = Vec::new();
    for (mut record, state) in snapshot {
        if let Some(arc) = state {
            match arc.state.lock().unwrap().save(&mut values) {
                Ok(refs) => record.state_refs = Some(refs.iter().map(|(k, h)| (k.clone(), to_hex(h))).collect()),
                Err(e) => {
                    // Writing the record without its state would lose the state at the next
                    // load, so keep the previous file and say why.
                    eprintln!("warning: deployments not persisted: {:?}'s state couldn't be stored: {e:?}", record.slug);
                    return;
                }
            }
        }
        records.push(record);
    }
    match serde_json::to_vec_pretty(&records) {
        Ok(bytes) => {
            // Owner-only, as the secrets and the flow log are: it holds the flows' documents,
            // grants and what their waiting runs need.
            if let Err(e) = crate::sealing::write_private(Path::new(path), &bytes) {
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
pub(crate) fn load_deployments(
    path: &str,
    store: &SharedStore,
    policy: &Policy,
) -> Result<BTreeMap<String, Deployment>, String> {
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
        let server = match declaration_of(root_unit) {
            Some("server") => Some(
                read_server(root_unit, &resolver)
                    .map_err(|e| format!("deployment {slug:?} in {path:?}: not a valid server declaration: {e:?}"))?,
            ),
            _ => None,
        };
        // As its deploy derived it: on a schedule, it runs on the clock.
        let caps = capabilities(root_unit, &resolver).deployed(record.interval_seconds.is_some() && server.is_none());
        let arrivals = Arc::new(arrivals::Watch::open(path, slug, &caps, holon_core::file_triggers(root_unit, &resolver)));
        let mut triggers = trigger_labels(
            server.as_ref(),
            caps.contains(Capability::NetworkIngress),
            caps.contains(Capability::Clock) && server.is_none(),
        );
        triggers.extend(arrivals.labels().filter(|_| server.is_none()));
        let next_due = record.interval_seconds.filter(|_| server.is_none()).map(|_| SystemTime::now());
        let clock_due = server.as_ref().map(|s| vec![SystemTime::now(); s.clocks.len()]).unwrap_or_default();

        // The grant, re-checked against what the unit needs now and the policy in force. A record
        // from before grants is granted what it was acknowledged for at its deploy.
        let mut grant = match record.grant {
            Some(grant) => grant,
            None => Grant::for_manifest(&Manifest::new(caps.iter(), Requirements::default()), epoch_now(), "migrated"),
        };
        let mut attention = grant.attention(&grant.manifest_of(caps.iter()), policy);
        if let Some(why) = &attention {
            eprintln!("holon: deployment {slug:?} is paused: {why}");
        }
        let permit = Arc::new(Permit::new(unit_hash, &grant, policy));

        let mut values = store.clone();
        let state = match (&record.state_refs, record.state) {
            (Some(refs), _) => {
                let refs = refs
                    .iter()
                    .map(|(k, h)| parse_hash(h).map(|h| (k.clone(), h)))
                    .collect::<Option<BTreeMap<String, Hash>>>()
                    .ok_or_else(|| format!("deployment {slug:?} in {path:?}: a state reference is not a hash"))?;
                let state = State::load(&refs, &values)
                    .map_err(|e| format!("deployment {slug:?} in {path:?}: its state can't be loaded from the store: {e:?}"))?;
                Some(Arc::new(claims::StateCell::new(state)))
            }
            // A file from before state was kept by reference: move its values into the store.
            (None, Some(inline)) => Some(Arc::new(claims::StateCell::new(migrate_inline(slug, path, inline, &mut values)?))),
            (None, None) if derives_state(caps.iter()) => Some(Arc::new(claims::StateCell::new(State::new()))),
            (None, None) => None,
        };
        let mut sessions = record.sessions;
        for session in &mut sessions {
            if let Some(inline) = session.state.take() {
                let refs = migrate_inline(slug, path, inline, &mut values)?.save(&mut values).map_err(|e| {
                    format!("deployment {slug:?} in {path:?}: session {}'s snapshot can't be stored: {e:?}", session.number)
                })?;
                session.state_refs = Some(refs.iter().map(|(k, h)| (k.clone(), to_hex(h))).collect());
            }
        }
        let deployed_at = record
            .deployed_at_epoch_seconds
            .map(|secs| UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap_or_else(SystemTime::now);
        // A file from before rc.30 keeps earlier versions' documents only: number them, move
        // their documents into the store, and add the current version.
        let kept = if record.versions_kept.is_empty() { record.versions } else { record.versions_kept };
        let current = crate::wire::Version {
            number: 0,
            unit: record.unit.clone(),
            deployed_at: record.deployed_at_epoch_seconds,
            by: record.deployed_by.clone(),
            restored_from: None,
            request: None,
            document_ref: None,
            document: record.document.clone(),
        };
        let versions = crate::deploy::complete_versions(kept, current, &values)
            .map_err(|e| format!("deployment {slug:?} in {path:?}: {e}"))?;

        println!("loaded deployment {:?} from {path:?} (triggers: {triggers:?})", record.slug);
        let log = Log::for_deployment(path, &record.slug);
        if !log.breaks().is_empty() {
            let runs: Vec<String> = log.breaks().iter().map(u64::to_string).collect();
            eprintln!(
                "holon: {:?}'s run log was changed outside holon: run {} doesn't follow the run before it",
                record.slug,
                runs.join(", run ")
            );
        }
        let channels = Arc::new(channels::Channels::from_log(&log, &permit.channels));
        loaded.insert(
            record.slug,
            Deployment {
                channels,
                unit: unit_hash,
                triggers,
                interval_seconds: record.interval_seconds,
                deployed_by: record.deployed_by.clone(),
                uses: record.uses.clone(),
                browsers: record.browsers.clone(),
                next_due,
                clock_due,
                history: Vec::new(),
                state,
                server,
                life: {
                    let started = deployed_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                    let mut life = Lifecycle { status: record.status.unwrap_or(Status::Running), sessions };
                    if life.sessions.is_empty() {
                        life.sessions = Lifecycle::deployed(started).sessions;
                    }
                    // Not covered by its grant: it doesn't run until it is again. Covered again
                    // (the policy changed while it was down), it runs if the grant paused it.
                    if attention.is_some() && life.status == Status::Running {
                        life.status = Status::Paused;
                        grant.paused = true;
                    } else if attention.is_none() && grant.paused && life.status == Status::Paused {
                        life.status = Status::Running;
                        grant.paused = false;
                    }
                    life
                },
                grant,
                permit,
                attention: attention.take(),
                listen_error: None,
                secrets: Vault::default(),
                folders: Arc::new(folders::Scoped::memory()),
                arrivals,
                parked: record.parked.into_iter().map(|p| (p.run_id.clone(), p)).collect(),
                log,
                document: record.document,
                versions,
                trigger_count: record.trigger_count,
                error_count: record.error_count,
                deployed_at,
            },
        );
    }
    Ok(loaded)
}

/// A state written inline (files from before references), as a `State` whose values are now in
/// the store.
fn migrate_inline(
    slug: &str,
    path: &str,
    inline: BTreeMap<String, SValue>,
    store: &mut SharedStore,
) -> Result<State, String> {
    let state = State::from_values(inline.into_iter().map(|(k, v)| (k, v.into_core())).collect())
        .map_err(|e| format!("deployment {slug:?} in {path:?}: a state value can't be encoded: {e:?}"))?;
    state.save(store).map_err(|e| format!("deployment {slug:?} in {path:?}: its state can't be stored: {e:?}"))?;
    Ok(state)
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
