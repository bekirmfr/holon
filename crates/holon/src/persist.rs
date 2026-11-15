//! Saving and loading deployments and flows: atomic writes, and refusing to start on a file that
//! won't parse.

use crate::*;

/// A slug must be non-empty ASCII alphanumeric/`-`/`_` only — keeps the simple prefix-based
/// router in `route()` unambiguous (no `/` inside a slug could be confused with a path
/// separator) and avoids any path-traversal-shaped weirdness reaching `strip_prefix`.
/// Write the whole current deployments table to `path` as a JSON array of `DeploymentRecord`
/// (write-through — called after every successful deploy/delete, not batched or scheduled).
/// A no-op for a table kept nowhere. Failures are logged, not fatal — a person's server
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
pub(crate) fn persist_deployments(path: &Place, deployments: &Mutex<BTreeMap<String, Deployment>>) {
    static PERSIST: Mutex<()> = Mutex::new(());
    let Some(path) = path.at() else { return };
    let _persist = PERSIST.lock().unwrap();
    let records: Vec<DeploymentRecord> = deployments
        .lock()
        .unwrap()
        .iter_mut()
        .map(|(slug, dep)| {
            DeploymentRecord {
                slug: slug.clone(),
                unit: to_hex(&dep.unit),
                deployed_at_epoch_seconds: dep
                    .deployed_at
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs()),
                deployed_by: dep.deployed_by.clone(),
                uses: dep.uses.clone(),
                browsers: dep.browsers.clone(),
                agents_propose_off: dep.agents_propose_off,
                trigger_count: dep.trigger_count,
                error_count: dep.error_count,
                tallies: dep.tallies.clone(),
                status: Some(dep.life.status),
                sessions: dep.life.sessions.clone(),
                document: dep.document.clone(),
                versions: Vec::new(),
                versions_kept: dep.versions.clone(),
                grant: Some(dep.grant.clone()),
                // What a waiting run keeps that may hold a secret is sealed under its
                // deployment's keys, whose root is kept apart from this file: a backup of it
                // carries none. Sealed once, as it parked (`keep_waiting`), not again here: a save
                // under the table's lock writes what each holds (release audit D4). With what
                // each sent as it parked, which its channels hold. One that can't be sealed is
                // left out, never written in the clear.
                parked: {
                    let keys = dep.secrets.lock().unwrap().keys.clone();
                    let channels = Arc::clone(&dep.channels);
                    dep.parked
                        .values_mut()
                        .filter_map(|p| {
                            let sent = channels.waiting_sent(&p.run_id);
                            // One whose sealed part didn't open as the server started: as it was.
                            // On a server without keys (a test's), as it is.
                            let Some(keys) = keys.as_deref().filter(|_| p.sealed.is_none()) else {
                                return Some(ParkedRun { sent, ..p.clone() });
                            };
                            let part = match p.sealed_part.clone().filter(|part| part.sealed.is_some()) {
                                Some(part) => part,
                                None => match seal_waiting(slug, Some(keys), p) {
                                    Ok(part) => {
                                        p.sealed_part = Some(Arc::clone(&part));
                                        part
                                    }
                                    Err(e) => {
                                        eprintln!("warning: waiting run {} of {slug:?} not kept: it can't be sealed: {e}", p.run_id);
                                        return None;
                                    }
                                },
                            };
                            #[cfg(debug_assertions)]
                            check_sealed(slug, p, &part);
                            Some(ParkedRun { sent, ..p.sealed_for_disk(part.sealed.as_deref().unwrap_or_default()) })
                        })
                        .collect()
                },
            }
        })
        .collect();
    match serde_json::to_vec_pretty(&records) {
        Ok(bytes) => {
            // Sealed under the server's keys, owner-only: it holds the flows' documents, grants
            // and what their waiting runs need.
            if let Err(e) = crate::at_rest::write_table(path, crate::at_rest::role::DEPLOYMENTS, &bytes) {
                eprintln!("warning: failed to persist deployments to {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to serialize deployments: {e}"),
    }
}

/// A waiting run's part that may hold a secret (its inputs, sourced values, requests, tokens,
/// events and what its points show: `ParkedRun::plain_part`), sealed once under its deployment's
/// keys as it parks or parks again (`keep_waiting`), and written as it is by every save
/// (`persist_deployments`) until it changes.
pub(crate) struct SealedPart {
    /// Sealed, bound to its run; none on a server without keys (a test's).
    pub(crate) sealed: Option<String>,
    /// The BLAKE3 of what was sealed: a debug build checks at each save that the run still holds
    /// it (`check_sealed`), so a path that changed the part without sealing it again fails a
    /// test. Its nonce makes the sealed form no witness of that.
    of: [u8; 32],
    /// How long what was sealed is: what `max_waiting_bytes` counts.
    pub(crate) bytes: u64,
}

impl std::fmt::Debug for SealedPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SealedPart")
    }
}

/// `run`'s part sealed under `keys` (none: only measured).
fn seal_waiting(slug: &str, keys: Option<&crate::at_rest::Keyring>, run: &ParkedRun) -> Result<Arc<SealedPart>, String> {
    let plain = run.plain_part();
    let sealed = match keys {
        Some(keys) => Some(keys.seal_line(&crate::at_rest::deployment(slug), &crate::durable::parked_context(slug, &run.run_id), plain.as_bytes())?),
        None => None,
    };
    Ok(Arc::new(SealedPart { sealed, of: *blake3::hash(plain.as_bytes()).as_bytes(), bytes: plain.len() as u64 }))
}

/// The bytes `run` takes of its deployment's `max_waiting_bytes`: its part's, sealed or not.
fn waiting_bytes(run: &ParkedRun) -> u64 {
    match (&run.sealed_part, &run.sealed) {
        (Some(part), _) => part.bytes,
        (None, Some(sealed)) => sealed.len() as u64,
        (None, None) => run.plain_part().len() as u64,
    }
}

/// Keep `run` waiting in `dep`, in place of what it was: its part sealed once, here, and every
/// waiting run's within the bytes `dep`'s may hold in all (`max_waiting_bytes`); or `dep` left as
/// it was, and why. Every path that parks a run, or changes what a waiting run keeps sealed,
/// comes through here (durable.rs).
pub(crate) fn keep_waiting(slug: &str, dep: &mut Deployment, mut run: ParkedRun) -> Result<(), String> {
    let keys = dep.secrets.lock().unwrap().keys.clone();
    let part = seal_waiting(slug, keys.as_deref(), &run)?;
    let others: u64 = dep.parked.values().filter(|p| p.run_id != run.run_id).map(waiting_bytes).sum();
    let most = dep.permit.durable.max_waiting_bytes;
    if others.saturating_add(part.bytes) > most {
        return Err(format!("{slug:?}'s waiting runs would hold more than the {most} bytes they may (max_waiting_bytes)"));
    }
    run.sealed_part = Some(part);
    dep.parked.insert(run.run_id.clone(), run);
    Ok(())
}

/// That `run` still holds what `part` sealed: a path that changed its sealed part without keeping
/// it through `keep_waiting` would have a save write it as it was.
#[cfg(debug_assertions)]
fn check_sealed(slug: &str, run: &ParkedRun, part: &SealedPart) {
    let now = blake3::hash(run.plain_part().as_bytes());
    assert!(
        now.as_bytes() == &part.of,
        "waiting run {} of {slug:?} changed what it keeps sealed without persist::keep_waiting: a save would write it as it was",
        run.run_id
    );
}

/// Each waiting run of `dep` that opened as the server started, its part sealed again
/// (`keep_waiting`'s, with no cap to hold to: it was within it), for a save to write as it is.
pub(crate) fn seal_opened(slug: &str, dep: &mut Deployment) {
    let keys = dep.secrets.lock().unwrap().keys.clone();
    for run in dep.parked.values_mut().filter(|p| p.sealed.is_none()) {
        match seal_waiting(slug, keys.as_deref(), run) {
            Ok(part) => run.sealed_part = Some(part),
            Err(e) => eprintln!("holon: waiting run {} of {slug:?} can't be sealed: {e}", run.run_id),
        }
    }
}

/// Where deployment `slug`'s state is kept for the deployments file at `path`: a file beside its
/// run log's folder holding the hash of its current snapshot (docs/L4s/request-path-L4.md, C3 b),
/// written as each change lands. None for a table kept nowhere.
pub(crate) fn state_head_for(path: &Place, slug: &str) -> Option<PathBuf> {
    path.beside("state").map(|dir| dir.join(slug).into())
}

/// Load deployments from `path` at startup — the inverse of `persist_deployments`. Returns an
/// empty table for one kept nowhere or a missing file (first run). A file that exists
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
    path: &Place,
    store: &SharedStore,
    policy: &Policy,
) -> Result<BTreeMap<String, Deployment>, String> {
    let mut loaded = BTreeMap::new();
    let Some(file) = path.at() else { return Ok(loaded) };

    let Some(bytes) = crate::at_rest::read_sealed(file, crate::at_rest::role::DEPLOYMENTS)? else { return Ok(loaded) };
    let records: Vec<DeploymentRecord> = crate::wire::read_json(&bytes)
        .map_err(|e| format!("{file:?} is not valid deployment JSON: {e}"))?;

    for record in records {
        let slug = &record.slug;
        let Some(unit_hash) = parse_hash(&record.unit) else {
            return Err(format!("deployment {slug:?} in {file:?}: {:?} is not a valid hash", record.unit));
        };
        let resolver = closure(store, unit_hash)
            .map_err(|e| format!("deployment {slug:?} in {file:?}: store error: {e:?}"))?;
        let Some(root_unit) = resolver.resolve(&unit_hash) else {
            return Err(format!(
                "deployment {slug:?} in {file:?}: unit {} is not in the store. Start with the --store it \
                 was deployed into, run with --deployments :memory:, or move {file:?} aside",
                record.unit
            ));
        };
        // A deployment from before every flow was a server (one run by an interval or by hand,
        // with no handler) isn't loaded: nothing would start it now (Holon is in development,
        // so it isn't carried forward).
        let server = match read_server(root_unit, &resolver) {
            Ok(s) => s,
            Err(holon_core::DeclError::NotAServer | holon_core::DeclError::NoHandlers) => {
                eprintln!("holon: deployment {slug:?} isn't loaded: it has no route, clock, file handler or action, so nothing would start a run. Deploy it again with one.");
                continue;
            }
            Err(e) => return Err(format!("deployment {slug:?} in {file:?}: not a valid server declaration: {e:?}")),
        };
        let caps = holon_core::deployed_capabilities(root_unit, &resolver);
        let arrivals = Arc::new(arrivals::Watch::open(path, slug, &caps, holon_core::file_triggers(root_unit, &resolver)));
        let triggers = trigger_labels(&server);
        let clock_due = vec![SystemTime::now(); server.clocks.len()];
        let server = Some(server);

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

        // Its state as its head names it (C3 b): no head, none kept yet. No older form is read
        // forward (Holon is in development: a data folder from before heads loads empty).
        let head = state_head_for(path, slug);
        let state = match head.as_deref().and_then(|h| std::fs::read_to_string(h).ok()) {
            Some(hex) => {
                let refs = crate::restore::snapshot_refs(hex.trim(), store)
                    .map_err(|e| format!("deployment {slug:?} in {file:?}: its state head names no snapshot the store has: {e}"))?;
                let state = State::load(&refs, store)
                    .map_err(|e| format!("deployment {slug:?} in {file:?}: its state can't be loaded from the store: {e:?}"))?;
                Some(Arc::new(claims::StateCell::kept_at(state, head)))
            }
            None if derives_state(caps.iter()) => Some(Arc::new(claims::StateCell::kept_at(State::new(), head))),
            None => None,
        };
        let sessions = record.sessions;
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
            state_at_deploy: None,
            first_seq: None,
            document: record.document.clone(),
        };
        let values = store.clone();
        let versions = crate::deploy::complete_versions(kept, current, &values)
            .map_err(|e| format!("deployment {slug:?} in {file:?}: {e}"))?;

        println!("loaded deployment {:?} from {file:?} (triggers: {triggers:?})", record.slug);
        let mut log = Log::for_deployment(path, &record.slug);
        // Not started on a log or events it can't read: it would number its runs again from 0,
        // and cut what it holds (recorder.rs, `Log::refused`).
        if let Some(why) = log.refused() {
            return Err(format!("deployment {slug:?}'s run log can't be opened: {why}"));
        }
        // Writing what it lets go of to the backup from its first run, as a new deployment's does
        // (deploy.rs), not only from the housekeeping's first look; and before its retention,
        // which cuts back a file past its age at once: set after, the aged runs a restart found
        // were let go of with no backup written (release audit R46).
        log.set_backup(policy.storage.backup.clone().map(|dir| (dir.into(), false)));
        log.retain(permit.log.keep_runs as usize, permit.log.max_age_ms());
        if !log.breaks().is_empty() {
            let runs: Vec<String> = log.breaks().iter().map(u64::to_string).collect();
            eprintln!(
                "holon: {:?}'s run log was changed outside holon: run {} doesn't follow the run before it",
                record.slug,
                runs.join(", run ")
            );
        }
        let channels = Arc::new(channels::Channels::from_log(&log, &permit.channels));
        for p in &record.parked {
            channels.keep_waiting_sent(&p.run_id, p.sent.clone());
        }
        let events = crate::events::Events::for_deployment(path, &record.slug);
        if let Some(why) = events.refused() {
            return Err(format!("deployment {slug:?}'s events can't be opened: {why}"));
        }
        if !events.breaks().is_empty() {
            eprintln!("holon: {:?}'s events were changed outside holon: event {:?} doesn't follow the one before it", record.slug, events.breaks());
        }
        loaded.insert(
            record.slug,
            Deployment {
                events,
                channels,
                sockets: Default::default(),
                unit: unit_hash,
                triggers,
                deployed_by: record.deployed_by.clone(),
                uses: record.uses.clone(),
                browsers: record.browsers.clone(),
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
                log: Arc::new(LogCell::new(log)),
                document: record.document,
                versions,
                trigger_count: record.trigger_count,
                error_count: record.error_count,
                tallies: record.tallies,
                tallies_changed: false,
                deployed_at,
                agents_propose_off: record.agents_propose_off,
            },
        );
    }
    Ok(loaded)
}

/// After a stop that wasn't one (a crash, a power loss: `stopping::stopped_cleanly`), each
/// deployment whose state, as its head names it, isn't the one the last run in its log left says so
/// on its timeline: a `lifecycle` event naming the state the server loaded (`Event::loaded`). A run
/// lands its state, and its head, before its line is written (C3 b), so a run the stop cut off
/// between the two may have left it, and `holon verify` takes the run after, begun from that state,
/// as one it can't check where it began, not as a state changed outside holon (verify.rs). Not
/// where a deploy, a restore or a stop and run again came after that run, which put a state in
/// place their events say; nor where no run here left a state to compare, or an event says so
/// already (a second crash before a run).
pub(crate) fn note_cut_off_runs(path: &Place, deployments: &mut BTreeMap<String, Deployment>) {
    for (slug, dep) in deployments.iter_mut() {
        let Some(loaded) = state_head_for(path, slug).and_then(|head| std::fs::read_to_string(head).ok()) else { continue };
        let loaded = loaded.trim().to_string();
        // The run verify takes a run after it to have begun from: the last that ran and didn't fail.
        let last = dep.log.with(|log| log.newest().find(|e| !e.run_id.is_empty() && e.error.is_none()).map(|e| (e.seq, e.state_after.clone())));
        let Some((seq, Some(left))) = last else { continue };
        let said = dep.events.entries().iter().any(|ev| ev.run_seq > seq && (crate::verify::moves_state(ev) || ev.loaded.as_ref() == Some(&loaded)));
        if left == loaded || said {
            continue;
        }
        eprintln!("holon: {slug:?}'s state isn't the one its last run (run {seq}) left: the server stopped without finishing, and may have cut a run off between its commit and its line");
        let what = format!(
            "started again after a crash or a power loss, from a state the last run here (run {seq}) didn't leave: a run cut off between its commit and its line may have left it"
        );
        dep.note("lifecycle", what, None, crate::events::Detail { loaded: Some(loaded), ..Default::default() });
    }
}

/// Persist the flows table — same write-through discipline and opt-out (kept nowhere) as
/// `persist_deployments`, but simpler: a `FlowRecord` is already its own on-disk shape (no
/// store cross-references, no re-derivation needed on the way back in).
pub(crate) fn persist_flows(path: &Place, flows: &BTreeMap<String, FlowRecord>) {
    let Some(path) = path.at() else { return };
    let records: Vec<&FlowRecord> = flows.values().collect();
    match serde_json::to_vec_pretty(&records) {
        Ok(bytes) => {
            if let Err(e) = crate::at_rest::write_table(path, crate::at_rest::role::FLOWS, &bytes) {
                eprintln!("warning: failed to persist flows to {path:?}: {e}");
            }
        }
        Err(e) => eprintln!("warning: failed to serialize flows: {e}"),
    }
}

/// Load flows at startup — the inverse of `persist_flows`. A missing file (first run) or
/// one kept nowhere yields an empty table; an unreadable or unparseable file is an `Err` and the
/// server refuses to start, for the same reason as `load_deployments`: the next save would
/// overwrite every flow with the empty table. A record whose name fails `is_valid_slug` is an
/// `Err` too, for the same reason.
pub(crate) fn load_flows(path: &Place) -> Result<BTreeMap<String, FlowRecord>, String> {
    let Some(path) = path.at() else { return Ok(BTreeMap::new()) };
    let Some(bytes) = crate::at_rest::read_sealed(path, crate::at_rest::role::FLOWS)? else { return Ok(BTreeMap::new()) };
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
