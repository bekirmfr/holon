//! Running a unit once: the recording host, firing with a deployment's state, and turning a
//! run into a flow-log entry.

use crate::*;

/// The `Host` this endpoint runs `fire` against. **Deliberately does not perform anything
/// real** — no network call, no file write, no email. It records each `(capability, payload)`
/// `fire` hands it and returns them to the caller as a receipt instead. This is a scoped
/// decision, not an oversight: the project's own plan already names real platform hosts (real
/// email/socket/file IO behind the `Host` seam) as a still-deferred, separate piece of work,
/// not something this transport crate was ever asked to build. Swapping in a real `Host` later
/// is exactly what the `Host` trait already exists to make a clean, isolated change — nothing
/// about this endpoint's wire shape would need to move.
pub(crate) struct RecordingHost {
    pub(crate) performed: Vec<(Capability, Value)>,
}

impl Host for RecordingHost {
    fn perform(&mut self, capability: Capability, payload: &Value) {
        self.performed.push((capability, payload.clone()));
    }
}

/// Fire `root_unit` with one extra sourced value (whatever actually triggered this fire —
/// `NetworkIngress`'s body, or `Clock`'s current time) plus, if this deployment carries state,
/// every key currently in `state` sourced as `StateRead(key)` — then, after firing, apply any
/// `StateWrite(key)` that was performed back into `state`. The same three steps
/// `holon-host`'s own `StateHost::round` uses internally (snapshot → fire → apply) —
/// deliberately **not** routed through `StateHost` itself, though: `round()` only ever keeps
/// `StateWrite` from what gets performed and quietly discards everything else, which is wrong
/// here — a state-bearing deployment that also sends an egress needs that egress reported back
/// to the caller, not silently dropped. `RecordingHost` already captures everything performed;
/// this just adds the one extra step `round()` would otherwise hide.
///
/// Takes the deployment's own `Arc<Mutex<State>>` (cloned out from under the table lock, which
/// is never held here), and holds it only to copy the state and, at the end, to commit: runs of
/// one deployment proceed side by side, each committing only if what it read is unchanged and
/// running again otherwise (`fire_with_state`).
///
/// A `store_read` for a key that's never been written simply isn't in `sourced` at all, so it
/// evaluates to nothing — the same honest-absence behavior any other unsupplied sourced
/// capability already has (`fire_sourced`'s own doc). No special-casing needed for a
/// deployment's first ever fire.
/// Fire `root_unit` with whatever sourced values this fire can honestly supply — always
/// including every `StateRead(key)` this deployment's own state currently holds, plus
/// `extra_sourced`: the driver capabilities the caller can actually answer for this fire.
/// A slice, not one pair: `Clock` (the wall clock is always knowable) and `Random` (a fresh
/// draw is always makeable) are cheap and honest to supply on *every* fire regardless of what
/// specifically triggered it — a unit combining `ingress` and `random`, say, needs both
/// available at once. `SecretAccess` is deliberately never in this list: there is
/// no addressing mechanism in the wire surface today (a leaf declaring `SecretAccess` doesn't say
/// *which* secret; `secret` and `file_read` name theirs, and are answered elsewhere), so there is
/// nothing honest to supply — a real, separate design question, not
/// something to paper over with a placeholder value.
///
/// Returns every settled output slot, not just one — `fire_sourced` itself now carries
/// `eval`'s real `Vec<Option<Value>>` end to end, and this just passes it through. A flow that
/// produces a genuine data response *and* performs a genuine side effect are two separate
/// things; forcing them into one artificial `list(...)` to satisfy an old single-output
/// requirement doesn't just add unrelated wiring, it makes the settled value redundantly
/// echo the effect inside what's supposed to be the plain answer, since nothing ever removes
/// it from there. `perform_effects` already walks every non-`None` slot, so a state-write and
/// a text response left as two separate exposed outputs are both correctly handled now,
/// without combining them into anything.
/// A fired run: its outputs, the effects performed, the state it read, and when each step ran
/// (the last two for the flow log).
pub(crate) struct FiredRun {
    pub(crate) outputs: Vec<Option<Value>>,
    pub(crate) performed: Vec<(Capability, Value)>,
    pub(crate) state_before: BTreeMap<String, Value>,
    pub(crate) timeline: Vec<recorder::NodeTiming>,
    /// The outbound requests it made, round by round.
    pub(crate) rounds: Vec<Round>,
    /// Why it failed (a run budget), if it did: then nothing was performed.
    pub(crate) error: Option<String>,
    /// The run's id (hex): its deployment and the Merkle root of what it started from.
    pub(crate) run_id: String,
    /// The deployment version it ran on: the deployment's unit then (hex).
    pub(crate) unit: String,
    /// The host nonce (hex) in that input, so the id can be recomputed from the log entry.
    pub(crate) nonce: String,
    /// The grant (hash, hex) it was held to.
    pub(crate) grant: String,
    /// The waiting points it settled waiting at: then it performed nothing (durable.rs parks it).
    pub(crate) waiting: Vec<Waiting>,
    /// For a run that settled waiting: the packs that didn't wait, sent as it parks.
    pub(crate) progress: Vec<(Capability, Value)>,
    /// The state it started from (the live state, or a resumed run's recorded one).
    pub(crate) start_state: Option<State>,
    /// The collection reads it made, and what each answered.
    pub(crate) reads: Vec<collections::Read>,
    /// How many times it ran again because what it read changed before it could commit.
    pub(crate) retries: u32,
    /// The keys it claimed before calling out (claims.rs), over every attempt.
    pub(crate) claims: Vec<claims::ClaimRecord>,
    /// The nodes a secret reaches (taint.rs): what its log entry keeps none of, or withholds.
    pub(crate) secret_fed: taint::SecretFed,
    /// The secrets it was given, by name: the version and fingerprint of each (`v3 #a1b2c3d4`).
    pub(crate) secrets: BTreeMap<String, String>,
    /// The credentials its calls answered to requests that held a secret (`secrets::answered_tokens`).
    pub(crate) answered: recorder::AnswerTokens,
    /// The state it left, once committed: a snapshot's hash (`restore::save_snapshot`).
    pub(crate) state_after: Option<String>,
}

/// How many times a run is tried without holding its state's lock before it runs holding it.
const OPTIMISTIC_ATTEMPTS: u32 = 4;

/// What one attempt at a run produced, before it is committed (or found stale and tried again).
struct Attempt {
    run: FiredRun,
    /// What it started from (and validates against at commit).
    start: Option<State>,
    /// It found what it read changed as it claimed it: run it again, nothing was sent.
    overtaken: bool,
}

// Everything a run is held to arrives here, so it takes them all.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fire_with_state(
    root_unit: &Unit,
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
    store: &SharedStore,
    state: Option<&Arc<claims::StateCell>>,
    extra_sourced: &[(Capability, Value)],
    permit: &Permit,
    clock: bool,
    secrets: &Vault,
    waits: &mut RunWaits,
    resumed: Option<Resumed>,
) -> FiredRun {
    let attempt = |start: Option<State>, nonce: &[u8; 16], prior: Vec<Round>, waits: &mut RunWaits, claim: Option<(&Arc<claims::StateCell>, Vec<claims::ClaimRecord>, bool)>| {
        run_attempt(root_unit, inputs, resolver, store, start, extra_sourced, permit, clock, secrets, waits, nonce, prior, claim)
    };
    let limits = if clock { &permit.clock_limits } else { &permit.request_limits };
    // A resumed run keeps the nonce it started with: it is the same run.
    let nonce = resumed.as_ref().map_or_else(new_nonce, |r| r.nonce);
    // The resources an admin set to `run` it holds from before it starts until it returns (it
    // commits, fails or waits); one held by others' runs past its claim_wait turns it away.
    let _run_locked = match RunLocked::take(waits.folders.as_deref(), permit, format!("run:{}", hex_bytes(&nonce)), std::time::Instant::now() + limits.claim_wait) {
        Ok(held) => held,
        Err(resource) => return turned_away(permit, inputs, extra_sourced, &nonce, &resource),
    };
    // A resumed run is the same run it started as: pinned to the state it first read (its own
    // deployment's cells and collections), its writes landing on the live state when it finishes,
    // its earlier parts' requests answered from its record (`base`). What it shares with other
    // deployments, files and databases, it reads as they are now, and checks at its commit as any
    // run does (§7.4).
    let (pinned, base) = match resumed {
        Some(r) => (Some(r.state_before), r.prior),
        None => (None, Vec::new()),
    };
    let Some(cell) = state else {
        return attempt(None, &nonce, base, waits, None).run;
    };
    // Optimistic: run on a copy, no lock held, so runs of one deployment proceed side by side;
    // then commit only if everything it could have read is still as it found it. Otherwise run
    // again from the state as it now is (a resumed run: from where it last waited, with fresh
    // reads): the requests it already made are answered from its own record (`sent`), not sent
    // again, and nothing it did is kept but its answer. A run about to call out first claims the
    // keys it read and may write (claims.rs): one that finds them changed as it claims them runs
    // again having sent nothing, and its commit never lands under another run's claim.
    let mut sent: Vec<Round> = Vec::new();
    let mut records = Vec::new();
    let prior = |sent: &[Round]| base.iter().chain(sent).cloned().collect::<Vec<Round>>();
    for retries in 0..OPTIMISTIC_ATTEMPTS {
        // Its resume tokens are derived from its id, which a fresh start changes; a resumed run's
        // is its first's (and an old one's tokens are only in its record).
        if pinned.is_none() {
            waits.tokens.clear();
        }
        // Copied in its own statement: a lock taken inside the call's arguments would be held
        // until the call returns, the whole run long.
        let start = match &pinned {
            Some(s) => s.clone(),
            None => cell.state.lock().unwrap().clone(),
        };
        let mut a = attempt(Some(start), &nonce, prior(&sent), waits, Some((cell, records, pinned.is_none())));
        sent.append(&mut a.run.rounds);
        a.run.rounds = sent.clone();
        if !a.overtaken {
            let written = claims::written_keys(&a.run.performed);
            if !cell.claims.wait_free(&written, &a.run.run_id, std::time::Instant::now() + limits.claim_wait) {
                eprintln!("holon: run {} commits over a key another run still holds (claim_wait passed)", a.run.run_id);
            }
            // The same for the rows and files it writes, which a run of another deployment (or of
            // another server, on a Postgres or MySQL database) may hold: it holds them itself until
            // it has committed, so none calls out over them meanwhile.
            let shared_folders = waits.folders.clone();
            let _writing = shared_folders.as_deref().and_then(|f| {
                let shared = claims::shared_written_keys(f, &a.run.performed);
                if shared.is_empty() {
                    return None;
                }
                claims::Hold::take(&f.all, shared, a.run.run_id.clone(), std::time::Instant::now() + limits.claim_wait)
                    .map_err(|key| eprintln!("holon: run {} commits over {key}, which another run still holds (claim_wait passed)", a.run.run_id))
                    .ok()
            });
            // Its files are written beside their names before any lock is taken: under the locks,
            // what is left is the check and a rename.
            let staged = folders::stage(waits.folders.as_ref(), &a.run.performed);
            let mut live = cell.state.lock().unwrap();
            // The shared resources it read or writes are locked from the check to the last write,
            // so a run of another deployment can't write them in between. One it can't lock in
            // time counts as changed: it runs again.
            match commit_locks(waits.folders.as_deref(), &a.run.reads, &a.run.performed, limits) {
                Ok(_held) if still_as_read(a.start.as_ref().expect("a stateful run's start"), &live, &a.run.reads, permit, store, waits.folders.as_ref(), pinned.is_some()) => {
                    // Its databases' writes first, each in a transaction that checks what it read
                    // there; the rest lands while those are open.
                    let landed = databases::commit(waits.folders.as_deref(), &a.run.run_id, &a.run.reads, &a.run.performed, permit, limits, true, || {
                        commit(&mut live, store, staged, &a.run.performed)
                    });
                    let done = match landed {
                        databases::Commit::Done(mut failed, db) => {
                            failed.extend(db);
                            not_all_written(&mut a.run, failed);
                            true
                        }
                        databases::Commit::Refused(why) => {
                            refused(&mut a.run, &why);
                            true
                        }
                        databases::Commit::Again(why) => {
                            eprintln!("holon: run {}: {why}: it runs again", a.run.run_id);
                            false
                        }
                    };
                    if done {
                        a.run.state_after = snapshot_after(&live, a.start.as_ref(), store, &a.run.run_id);
                        a.run.retries = retries;
                        if let Some(c) = waits.claiming.take() {
                            a.run.claims = c.finish();
                        }
                        return a.run;
                    }
                }
                Ok(_) => {}
                Err(resource) => eprintln!("holon: run {} waited past commit_wait for {resource}: it runs again", a.run.run_id),
            }
        }
        records = match waits.claiming.take() {
            Some(c) => c.finish(),
            None => a.run.claims,
        };
    }
    // Contended that long: run once more holding the lock, so it can't be overtaken again. It
    // claims nothing (it owns everything), once every claim before it is let go (a resumed run's
    // own state is pinned: no claim on it can change what it reads).
    if pinned.is_none() {
        waits.tokens.clear();
        if !cell.claims.wait_until_empty(std::time::Instant::now() + limits.claim_wait) {
            eprintln!("holon: a run runs holding the state while a key is still claimed (claim_wait passed)");
        }
    }
    // A run of another deployment may hold a row, table, file or folder it may touch: it holds
    // them all, whole, for its whole run, once those are let go, so none calls out over them
    // meanwhile.
    let shared = waits.folders.clone();
    let holder = format!("last:{}", nonce.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let _claimed = shared.as_deref().and_then(|f| {
        let keys = claims::whole_of(f, permit);
        if keys.is_empty() {
            return None;
        }
        claims::Hold::take(&f.all, keys, holder, std::time::Instant::now() + limits.claim_wait)
            .map_err(|key| eprintln!("holon: a run runs holding the state while {key} is still claimed (claim_wait passed)"))
            .ok()
    });
    let mut live = cell.state.lock().unwrap();
    // Nothing of its own deployment changes under it, but a shared folder or database can: it holds
    // every one it may read or write for its whole length too, as a claim is waited for.
    let whole_run = waits.folders.as_deref().and_then(|f| {
        let mut wanted = f.run_locks(&permit.folders_read(), &permit.folders_written());
        wanted.extend(f.all.databases.locks_on(&f.deployment, permit.databases_named()));
        f.all.locks.take(wanted, std::time::Instant::now() + limits.claim_wait).ok()
    });
    let is_pinned = pinned.is_some();
    let start = pinned.unwrap_or_else(|| live.clone());
    let mut a = attempt(Some(start), &nonce, prior(&sent), waits, None);
    sent.append(&mut a.run.rounds);
    a.run.rounds = sent;
    let staged = folders::stage(waits.folders.as_ref(), &a.run.performed);
    // Its reads are checked under the folders' locks (taken now, if it couldn't hold them all
    // along), and one changed fails it: it is the last try.
    let commit_held = match whole_run {
        Some(held) => Ok(held),
        None => commit_locks(waits.folders.as_deref(), &a.run.reads, &a.run.performed, limits),
    };
    match commit_held {
        Ok(_held) if still_as_read(a.start.as_ref().expect("a stateful run's start"), &live, &a.run.reads, permit, store, waits.folders.as_ref(), is_pinned) => {
            match databases::commit(waits.folders.as_deref(), &a.run.run_id, &a.run.reads, &a.run.performed, permit, limits, true, || {
                commit(&mut live, store, staged, &a.run.performed)
            }) {
                databases::Commit::Done(mut failed, db) => {
                    failed.extend(db);
                    not_all_written(&mut a.run, failed);
                    a.run.state_after = snapshot_after(&live, a.start.as_ref(), store, &a.run.run_id);
                }
                databases::Commit::Refused(why) => refused(&mut a.run, &why),
                databases::Commit::Again(why) => nothing_written(&mut a.run, &why),
            }
        }
        Ok(_) => nothing_written(&mut a.run, "a file it read was changed by another deployment's run as it ran"),
        Err(resource) => nothing_written(&mut a.run, &format!("its commit waited for {resource} past commit_wait_ms")),
    }
    a.run.retries = OPTIMISTIC_ATTEMPTS;
    a.run.claims = records;
    a.run
}

/// The run locks (§7.3) a run holds for its whole length: on each resource it touches that an
/// admin set to `run`, shared if it only reads it and exclusive if it may write it, in the server's
/// table (and a shared folder's file), and in each Postgres or MySQL database among them, for the
/// runs of other servers. Let go when dropped.
struct RunLocked {
    _guards: Vec<locks::Guard>,
    remote: Vec<(databases::Shared, bool)>,
    holder: String,
}

impl RunLocked {
    /// Take every one the grant's resources need, in the order of their addresses, waiting until
    /// `deadline` at most: past it, none is held, and the error names the resource it waited for.
    /// None to take: none held.
    fn take(folders: Option<&folders::Scoped>, permit: &Permit, holder: String, deadline: std::time::Instant) -> Result<Option<RunLocked>, String> {
        let Some(f) = folders else { return Ok(None) };
        let mut wanted = f.whole_run_locks(&permit.folders_handed(), &permit.folders_written());
        let databases = |tables: BTreeSet<String>| tables.into_iter().map(|t| t.split('/').next().unwrap_or(&t).to_string()).collect::<BTreeSet<String>>();
        let (in_databases, remote) = f.all.databases.whole_run_locks(&f.deployment, &databases(permit.tables_read()), &databases(permit.tables_written()));
        wanted.extend(in_databases);
        if wanted.is_empty() {
            return Ok(None);
        }
        let mut held = RunLocked { _guards: f.all.locks.take(wanted, deadline)?, remote: Vec::new(), holder };
        for (database, exclusive, label) in remote {
            while !database.hold_run(&held.holder, exclusive) {
                if std::time::Instant::now() + claims::POLL >= deadline {
                    return Err(label);
                }
                std::thread::sleep(claims::POLL);
            }
            held.remote.push((database, exclusive));
        }
        Ok(Some(held))
    }
}

impl Drop for RunLocked {
    fn drop(&mut self) {
        for (database, exclusive) in &self.remote {
            database.let_go_run(&self.holder, *exclusive);
        }
    }
}

/// A run turned away before it started: `resource`, which an admin set to `run`, was held by other
/// runs past its `claim_wait`. It did nothing, and says so as a busy claim does: a request answers
/// 503, a clock tick is skipped, both logged.
fn turned_away(permit: &Permit, inputs: &[Option<Value>], extra_sourced: &[(Capability, Value)], nonce: &[u8; 16], resource: &str) -> FiredRun {
    let mut run = FiredRun::nothing();
    run.run_id = to_hex(&run_id_of(permit.deployment, inputs, None, extra_sourced, nonce));
    run.unit = to_hex(&permit.deployment);
    run.nonce = hex_bytes(nonce);
    run.grant = permit.grant.clone();
    run.error = Some(format!("busy: {resource} is held by another run for its whole length (its run lock) past claim_wait"));
    eprintln!("holon: run {} turned away: {resource} is run-locked by another run past claim_wait", run.run_id);
    run
}

/// What a run whose writes didn't all land says: the run happened, and these didn't.
fn not_all_written(run: &mut FiredRun, failed: Vec<String>) {
    if !failed.is_empty() && run.error.is_none() {
        run.error = Some(format!("{}{}", folders::WRITE_FAILED, failed.join("; ")));
    }
}

/// What a run that couldn't commit at all says: nothing it did was written, and why.
fn nothing_written(run: &mut FiredRun, why: &str) {
    if run.error.is_none() && !run.performed.is_empty() {
        run.error = Some(format!("{}nothing it did was written: {why}", folders::WRITE_FAILED));
    }
}

/// What a run whose database commit was refused says: nothing it did was written, and why (an
/// `expect` the data didn't meet says so first: databases::UNMET).
fn refused(run: &mut FiredRun, why: &str) {
    if why.starts_with(databases::UNMET) {
        if run.error.is_none() {
            run.error = Some(format!("{why}: nothing it did was written"));
        }
    } else if let Some(why) = why.strip_prefix(databases::WRITE_FAILED) {
        if run.error.is_none() {
            run.error = Some(format!("{}nothing it did was written: {why}", databases::WRITE_FAILED));
        }
    } else {
        nothing_written(run, why);
    }
}

/// The commit locks on the shared resources a run read or writes (locks.rs), taken in order and
/// waiting `commit_wait` at most: the error names the one it waited for.
fn commit_locks(
    folders: Option<&folders::Scoped>,
    reads: &[collections::Read],
    performed: &[(Capability, Value)],
    limits: &holon_host::Limits,
) -> Result<Vec<locks::Guard>, String> {
    let Some(f) = folders else { return Ok(Vec::new()) };
    let mut wanted = f.commit_locks(reads, performed);
    wanted.extend(f.all.databases.commit_locks(&f.deployment, reads, performed));
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    f.all.locks.take(wanted, std::time::Instant::now() + limits.commit_wait)
}

/// Whether `live` still holds everything a run that started from `start` could have read: each
/// state cell it was granted (by its hash), and each collection and file read it made (asked
/// again; a file as it now is on disk). A `pinned` run (a resumed one) is held to its first
/// start's cells and collections, whatever they now are: only its file reads are checked.
#[allow(clippy::too_many_arguments)]
fn still_as_read(
    start: &State,
    live: &State,
    reads: &[collections::Read],
    permit: &Permit,
    store: &SharedStore,
    folders: Option<&Arc<folders::Scoped>>,
    pinned: bool,
) -> bool {
    let cells: BTreeSet<&String> = start.iter().chain(live.iter()).map(|(k, _)| k).collect();
    let cells_unchanged = pinned
        || cells
            .into_iter()
            .filter(|k| !k.starts_with('#') && permit.allows(&Capability::StateRead((*k).clone())))
            .all(|k| start.hash_of(k) == live.hash_of(k));
    if !cells_unchanged {
        return false;
    }
    let now = collections::Readable { store: store.clone(), roots: collections::roots(live), granted: permit.collections_read() };
    let mut files = folders.map(|f| folders::Readable::new(Arc::clone(f), permit));
    reads.iter().all(|r| {
        // A database's reads are checked in its own commit transaction (databases.rs).
        if matches!(&r.request, Value::Record(q) if q.contains_key("database")) {
            return true;
        }
        if !matches!(&r.request, Value::Record(q) if q.contains_key("folder")) {
            return pinned || now.answer(&r.request) == r.answer;
        }
        files.as_mut().is_none_or(|files| files.answer(&r.request, false).is_ok_and(|(answer, _)| answer == r.answer))
    })
}

/// The state a run left, kept as a snapshot (its hash), taken under the state's lock as it commits:
/// the cells it changed since `start` go into the content store, then the snapshot. A store that
/// can't take it leaves the run without one (it can't be restored to), and says so.
fn snapshot_after(live: &State, start: Option<&State>, store: &SharedStore, run_id: &str) -> Option<String> {
    crate::restore::save_snapshot(live, start, store)
        .map_err(|e| eprintln!("holon: run {run_id}: the state it left wasn't kept: {e}"))
        .ok()
}

/// Apply a run's writes to the live state: each state write, then each collection put and delete,
/// in the order the run made them; then each file written or deleted, in its order (`staged`:
/// their bytes are beside their names already). What didn't land, and why: a file that couldn't
/// be written (the others still are).
fn commit(live: &mut State, store: &SharedStore, staged: folders::Staged, performed: &[(Capability, Value)]) -> Vec<String> {
    for (cap, payload) in performed {
        if let Capability::StateWrite(key) = cap {
            if !is_storable(payload) {
                eprintln!("holon: refused to store an effect request in state key {key:?}");
            } else if let Err(e) = live.insert(key.clone(), payload.clone()) {
                eprintln!("holon: state key {key:?} not written (too large to encode): {e:?}");
            }
        }
    }
    let mut store = store.clone();
    for (cap, payload) in performed {
        if let Capability::CollectionWrite(name) = cap
            && let Err(e) = collections::apply(live, &mut store, name, payload)
        {
            eprintln!("holon: collection {name:?} not written: {e}");
        }
    }
    let failed = staged.land();
    for e in &failed {
        eprintln!("holon: {e}");
    }
    failed
}

/// One run of `root_unit` from `start`, nothing committed: its outputs, what it would perform
/// (refused effects already dropped), and what it read.
#[allow(clippy::too_many_arguments)]
fn run_attempt(
    root_unit: &Unit,
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
    store: &SharedStore,
    start: Option<State>,
    extra_sourced: &[(Capability, Value)],
    permit: &Permit,
    clock: bool,
    secrets: &Vault,
    waits: &mut RunWaits,
    nonce: &[u8; 16],
    prior: Vec<Round>,
    claim: Option<(&Arc<claims::StateCell>, Vec<claims::ClaimRecord>, bool)>,
) -> Attempt {
    let state_before = start.as_ref().map(State::values).unwrap_or_default();
    let run_id = run_id_of(permit.deployment, inputs, start.as_ref(), extra_sourced, nonce);
    // Its resume tokens are derived from its id (durable.rs).
    waits.run_id = Some(to_hex(&run_id));
    // Its collections, as it found them, for the reads it was granted.
    waits.reads.clear();
    waits.collections = Some(collections::Readable {
        store: store.clone(),
        roots: start.as_ref().map(collections::roots).unwrap_or_default(),
        granted: permit.collections_read(),
    });
    // Its folders, for the reads it was granted: read as it finds them.
    waits.failed = None;
    waits.files = waits.folders.as_ref().map(|f| folders::Readable::new(Arc::clone(f), permit));
    waits.databases = waits.folders.as_ref().map(|f| databases::Readable::new(Arc::clone(f), permit));

    // Only the state keys the deployment was granted are read.
    let mut sourced: Vec<(Capability, Value)> = match start.as_ref() {
        Some(storage) => storage
            .iter()
            .map(|(key, value)| (Capability::StateRead(key.clone()), value.clone()))
            .filter(|(cap, _)| permit.allows(cap))
            .collect(),
        None => Vec::new(),
    };
    sourced.extend_from_slice(extra_sourced);
    // The secrets it was granted. Not part of the run's input record (nor its id): a log never
    // holds them, and a replay stands a placeholder in.
    let mut given = BTreeMap::new();
    let vault = secrets.lock().unwrap();
    for (name, value) in vault.current.iter() {
        let cap = Capability::SecretRead(name.clone());
        if permit.allows(&cap) {
            sourced.push((cap, Value::Text(value.to_string())));
            given.extend(vault.shown.get(name).map(|shown| (name.clone(), shown.clone())));
        }
    }
    drop(vault);

    // Which nodes compute with a secret it was granted, for its log entry.
    let secret_fed = if sourced.iter().any(|(c, _)| matches!(c, Capability::SecretRead(_))) {
        taint::SecretFed::of(root_unit, resolver)
    } else {
        taint::SecretFed::default()
    };

    let mut host = RecordingHost { performed: Vec::new() };
    let mut timer = recorder::Timer::new();
    // Outbound requests go only where the grant allows, within its limits.
    let limits = if clock { &permit.clock_limits } else { &permit.request_limits };
    // What this attempt will hold before it sends anything: the keys it reads that it may write,
    // in its collections and in the folders and databases other deployments share.
    let shared = waits.folders.as_ref().map(|f| claims::SharedReads {
        folders: Arc::clone(f),
        tables_written: permit.tables_written(),
        folders_written: permit.folders_written(),
        files: folders::Readable::new(Arc::clone(f), permit),
        databases: databases::Readable::new(Arc::clone(f), permit),
    });
    // A resumed run claims only what it shares (`own` false): its own collections it reads as
    // they were when it started, which no claim on them now can change.
    waits.claiming = claim.map(|(cell, records, own)| {
        claims::Claiming::new(
            Arc::clone(cell),
            store.clone(),
            if own { permit.collections_written() } else { BTreeSet::new() },
            permit.collections_read(),
            shared,
            to_hex(&run_id),
            limits.claim_wait,
            records,
        )
    });
    // A file as a request's body is sent from a folder the deployment reads.
    let forward = waits.forward.clone();
    let (slug, run, chain) = (waits.slug.clone().unwrap_or_default(), waits.run_id.clone().unwrap_or_default(), waits.chain.clone());
    let caller = calls::Caller { store, fed: &secret_fed, permit, vault: secrets, slug: &slug, run: &run, chain: &chain };
    let sender = folders::Sender {
        outbound: &permit.outbound,
        folders: waits.folders.clone(),
        granted: permit.folders_handed(),
        forward: forward.as_ref(),
        channels: waits.channels.clone(),
        sends: permit.channels_sent(),
        calls: Some(&caller),
    };
    let outcome = run_rounds(root_unit, inputs, &sourced, resolver, &mut host, &sender, limits, &mut timer, waits, &prior);
    let overtaken = outcome.error == Some(holon_host::RunError::Overtaken);
    // A read that failed (a file too big, or not text; a folder not bound) fails the run: it
    // performs nothing.
    let failed_read = waits.failed.take();
    if failed_read.is_some() {
        host.performed.clear();
    }
    // The credentials the calls that sent a secret were answered with: kept out of the log by
    // their value, and out of what is written, as the secret is.
    let answered = secrets::answered_tokens(&outcome.rounds, secrets, &secret_fed);
    let holds_answered = |payload: &Value| answered.iter().find(|(_, v)| secrets::value_holds(payload, &|s: &str| s.contains(v.as_str()))).map(|(name, _)| name.clone());
    // An effect the grant doesn't cover is refused, whatever the graph says; so is a write
    // carrying a secret's value, or something computed from one (taint.rs), which would put it on
    // disk and in every later snapshot, and a copy from a folder the deployment doesn't read (a
    // handle is only a record: one can be made up).
    let reads_folders = permit.folders_handed();
    host.performed.retain(|(cap, payload)| {
        if !permit.allows(cap) {
            eprintln!("holon: refused {cap:?}: not granted to this deployment");
            return false;
        }
        if let Some(from) = folders::copied_from(payload).filter(|f| !reads_folders.contains(*f))
            && matches!(cap, Capability::FileWrite(_))
        {
            eprintln!("holon: refused {cap:?}: a copy from the folder {from:?}, which this deployment doesn't read");
            return false;
        }
        // A file too: another deployment that reads it logs its text, redacted only of its own
        // secrets, and a part of it read is kept as it is.
        if let Capability::StateWrite(key) | Capability::CollectionWrite(key) | Capability::DbWrite(key) | Capability::ChannelSend(key) | Capability::FileWrite(key) = cap {
            if let Some(name) = secrets::secret_in(payload, secrets) {
                eprintln!("holon: refused to store or send secret {name:?} in {key:?}");
                return false;
            }
            if secret_fed.derives_effect(cap) {
                eprintln!("holon: refused to store or send in {key:?} what is computed from a secret");
                return false;
            }
            if let Some(name) = holds_answered(payload) {
                eprintln!("holon: refused to store or send in {key:?} the credential a call answered ({name})");
                return false;
            }
        }
        // A subscription is a route's answer, read from its response, never performed.
        !matches!(cap, Capability::ChannelRead(_))
    });
    // Writes past the run's file limits: it fails before anything it did commits.
    let files_read = waits.files.as_ref().map_or(0, |f| f.files);
    let past_limit = failed_read.is_none().then(|| folders::past_write_limits(&host.performed, files_read, &permit.files)).flatten();
    // Packs past the run's channel limits: likewise.
    let past_packs = (failed_read.is_none() && past_limit.is_none()).then(|| channels::past_pack_limits(&host.performed, &permit.channels)).flatten();
    if past_limit.is_some() || past_packs.is_some() {
        host.performed.clear();
    }
    // A run that settled waiting performs nothing yet, but the packs it settled that didn't wait
    // go out as it parks, held to the same grant, secrets and limits as any run's.
    let mut progress = if outcome.waiting.is_empty() || outcome.error.is_some() { Vec::new() } else { channels::settled_sends(&outcome.outputs) };
    progress.retain(|(cap, payload)| {
        let sends = permit.allows(cap) && secrets::secret_in(payload, secrets).is_none() && !secret_fed.derives_effect(cap) && holds_answered(payload).is_none();
        if !sends {
            eprintln!("holon: refused {cap:?} as the run waits: not granted, or carrying a secret or what is computed from one");
        }
        sends
    });
    if let Some(past) = channels::past_pack_limits(&progress, &permit.channels) {
        eprintln!("holon: the packs a waiting run settled weren't sent: {past}");
        progress.clear();
    }

    let run = FiredRun {
        outputs: outcome.outputs,
        performed: host.performed,
        state_before,
        timeline: timer.timeline,
        rounds: outcome.rounds,
        error: failed_read
            .or(past_limit.map(|e| format!("{}{e}", folders::WRITE_FAILED)))
            .or(past_packs.map(|e| format!("{}{e}", channels::SEND_FAILED)))
            .or(outcome.error.map(|e| e.message())),
        run_id: to_hex(&run_id),
        unit: to_hex(&permit.deployment),
        nonce: hex_bytes(nonce),
        grant: permit.grant.clone(),
        waiting: outcome.waiting,
        progress,
        start_state: start.clone(),
        reads: std::mem::take(&mut waits.reads),
        retries: 0,
        claims: waits.claiming.as_ref().map(|c| c.records.clone()).unwrap_or_default(),
        secret_fed,
        secrets: given,
        answered: recorder::AnswerTokens(answered),
        state_after: None,
    };
    Attempt { run, start, overtaken }
}

/// 128 bits from the operating system's secure generator.
fn new_nonce() -> [u8; 16] {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).expect("the OS has a secure random generator");
    nonce
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A run's id (§4.4): its deployment, and the Merkle root of what it started from: the host
/// nonce, each input, the state (by its root), and each sourced value, each by its hash.
pub(crate) fn run_id_of(
    deployment: Hash,
    inputs: &[Option<Value>],
    state: Option<&State>,
    sourced: &[(Capability, Value)],
    nonce: &[u8; 16],
) -> Hash {
    let h = |v: &Value| hash_value(v).unwrap_or([0xff; 32]);
    let mut entries: BTreeMap<String, Hash> = BTreeMap::new();
    entries.insert("nonce".into(), h(&Value::Text(hex_bytes(nonce))));
    for (i, input) in inputs.iter().enumerate() {
        if let Some(v) = input {
            entries.insert(format!("input:{i}"), h(v));
        }
    }
    if let Some(state) = state {
        entries.insert("state".into(), state.root());
    }
    for (cap, v) in sourced {
        entries.insert(format!("sourced:{cap:?}"), h(v));
    }
    let root = state_root(entries.iter().map(|(k, v)| (k.as_str(), v))).unwrap_or([0xff; 32]);
    run_id(&deployment, &root)
}

/// `POST /fire?root=<hex>` — settle a published unit **once**, on request. Not "deploy": there
/// is no standing process left running afterward, no listener bound to a driver — this is a
/// single call-and-return, the same shape `eval`/`design_eval` already are, just via `fire`
/// (deploy mode) instead. The genuine "keep a flow alive, bound to `ingress`/`timer`" pillar is
/// real and different work, deliberately not this endpoint — see the L4.
///
/// `root` must already be resolvable from the store (staged via `/unit`, or published) — this
/// reuses `materialize_closure` exactly as the cache endpoints do, rather than accepting the
/// unit fresh in the body, since firing something implies it's a real, referenceable unit, not
/// a throwaway one.
pub(crate) fn handle_post_fire(store: &SharedStore, request: &Request) -> Response {
    let Some(root_hex) = request.get_param("root") else {
        return err(400, "missing ?root=<hex> query param");
    };
    let Some(root_hash) = parse_hash(&root_hex) else {
        return err(400, "root is not a 64-char hex hash");
    };
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let fire_body: FireRequestBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("not a valid fire request JSON: {e}")),
    };
    let resolver = match materialize_closure(store, root_hash) {
        Ok(r) => r,
        Err(e) => return e.response(),
    };
    let Some(root_unit) = resolver.resolve(&root_hash) else {
        return err(404, "no unit stored at that hash — stage or publish it first");
    };
    let inputs: Vec<Option<Value>> =
        fire_body.inputs.into_iter().map(|o| o.map(SValue::into_core)).collect();

    let mut host = RecordingHost { performed: Vec::new() };
    let outputs = fire(root_unit, &inputs, &resolver, &mut host);

    Response::json(&FireResponseBody {
        outputs: outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        performed: host
            .performed
            .into_iter()
            .map(|(capability, payload)| PerformedEntry { capability: format!("{capability:?}"), payload: SValue::from_core(&payload) })
            .collect(),
    })
}

/// Add a run to `slug`'s flow log, in its current session: redacted of its secrets and of
/// `tokens` (the run's), in one pass, so that the log's marks are only ever its own.
pub(crate) fn log_run(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, mut entry: LogEntry, tokens: Redactor) {
    if let Some(dep) = deployments.lock().unwrap().get_mut(slug) {
        let answered = std::mem::take(&mut entry.answered);
        tokens.and(Redactor::of(&dep.secrets)).and(Redactor::answers(&answered.0, &dep.secrets)).entry(&mut entry);
        entry.session = dep.life.sessions.last().map_or(0, |s| s.number);
        // Its packs go out as it is logged: the run is over and its writes have landed, and what
        // a subscriber gets is what the log holds (channels.rs).
        let packs = dep.channels.packs_of(&entry, dep.log.next_seq());
        dep.log.push(entry);
        for (name, pack) in packs {
            dep.channels.send(&name, pack, &dep.permit.channels);
        }
    }
}

/// The timeline, with the outputs of each node that carries the text of a file the log keeps by
/// hash (read, or written past the inline size) left out: that text isn't in the log by another
/// way.
fn without_kept_files(timeline: &[recorder::NodeTiming], reads: &[collections::Read], performed: &[(Capability, Value)]) -> Vec<recorder::NodeTiming> {
    let read = reads.iter().filter(|r| r.file.as_ref().is_some_and(|f| !f.inline)).filter_map(|r| match &r.answer {
        Some(Value::Text(t)) => Some(t.as_str()),
        _ => None,
    });
    // A range's lines, as a node carries them: the list whole.
    let lines = reads.iter().filter(|r| r.file.as_ref().is_some_and(|f| !f.inline)).filter_map(|r| match &r.answer {
        Some(v @ Value::List(l)) if !l.is_empty() => serde_json::to_string(&SValue::from_core(v)).ok(),
        _ => None,
    });
    let kept: Vec<String> = read
        .chain(folders::written_texts(performed))
        .filter(|t| !t.is_empty())
        .filter_map(|t| serde_json::to_string(t).ok().map(|q| q[1..q.len() - 1].to_string()))
        .chain(lines)
        .collect();
    let mut timeline = timeline.to_vec();
    if kept.is_empty() {
        return timeline;
    }
    for node in &mut timeline {
        let carries = node.outputs.as_ref().and_then(|o| serde_json::to_string(o).ok()).is_some_and(|json| kept.iter().any(|t| json.contains(t.as_str())));
        if carries {
            node.outputs = None;
        }
    }
    timeline
}

/// The timeline, with the outputs of each node that may produce something computed from a secret
/// left out: redaction finds a secret's value, not what is computed from it (taint.rs).
fn without_secret_derived(mut timeline: Vec<recorder::NodeTiming>, fed: &taint::SecretFed) -> Vec<recorder::NodeTiming> {
    if !fed.is_empty() {
        for node in timeline.iter_mut().filter(|n| fed.derives(&n.path)) {
            node.outputs = None;
        }
    }
    timeline
}

/// `value` as the log keeps it, or the withheld mark when it may be computed from a secret.
fn withheld_or(derived: bool, value: &Value) -> SValue {
    if derived { SValue::from_core(&Value::Text(taint::WITHHELD.into())) } else { SValue::from_core(value) }
}

/// A run's outputs as its log entry and history keep them (`shown`: how each looks there), each
/// that may be computed from a secret withheld.
pub(crate) fn logged_outputs(run: &FiredRun, shown: impl Fn(&Value) -> Value) -> Vec<Option<SValue>> {
    run.outputs.iter().enumerate().map(|(port, o)| o.as_ref().map(|v| withheld_or(run.secret_fed.derives_output(port), &shown(v)))).collect()
}

/// What a run performed as its log entry and history keep it (`shown`: how each payload looks
/// there), each payload that may be computed from a secret withheld.
pub(crate) fn logged_performed(run: &FiredRun, shown: impl Fn(&Capability, &Value) -> Value) -> Vec<PerformedEntry> {
    run.performed
        .iter()
        .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: withheld_or(run.secret_fed.derives_effect(c), &shown(c, p)) })
        .collect()
}

/// A round as the log keeps it: each request's parts computed from a secret withheld.
fn logged_round(round: &Round, fed: &taint::SecretFed) -> recorder::LoggedRound {
    if fed.is_empty() {
        return recorder::LoggedRound::from_host(round);
    }
    let mut round = round.clone();
    for x in &mut round.exchanges {
        taint::withhold_request(&mut x.request, &x.path, fed);
    }
    recorder::LoggedRound::from_host(&round)
}

/// The parts every log entry shares, from what a run read and produced.
pub(crate) fn run_entry(
    kind: &'static str,
    started: (u64, std::time::Instant),
    sourced: &[(Capability, Value)],
    run: &FiredRun,
) -> LogEntry {
    LogEntry {
        seq: 0,
        prev: String::new(),
        session: 0,
        at_ms: started.0,
        duration_ms: started.1.elapsed().as_secs_f64() * 1000.0,
        kind: kind.to_string(),
        route: None,
        method: None,
        url: None,
        status: None,
        request: None,
        sourced: sourced.iter().map(|(c, v)| (format!("{c:?}"), SValue::from_core(v))).collect(),
        state_before: run.state_before.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect(),
        state_after: run.state_after.clone(),
        outputs: logged_outputs(run, folders::loggable_output),
        performed: logged_performed(run, folders::loggable),
        response_body: None,
        error: run.error.clone(),
        timeline: without_secret_derived(without_kept_files(&run.timeline, &run.reads, &run.performed), &run.secret_fed),
        rounds: run.rounds.iter().map(|r| logged_round(r, &run.secret_fed)).collect(),
        run_id: run.run_id.clone(),
        unit: run.unit.clone(),
        nonce: run.nonce.clone(),
        grant: run.grant.clone(),
        parked: vec![],
        progress: run
            .progress
            .iter()
            .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: withheld_or(run.secret_fed.derives_effect(c), p) })
            .collect(),
        events: BTreeMap::new(),
        retries: run.retries,
        claims: run
            .claims
            .iter()
            .map(|c| recorder::LoggedClaim {
                key: c.key.to_string(),
                waited_ms: c.waited.as_secs_f64() * 1000.0,
                overtaken: c.overtaken,
                held_ms: c.held.map(|d| d.as_secs_f64() * 1000.0),
            })
            .collect(),
        by: None,
        secrets: run.secrets.clone(),
        answered: run.answered.clone(),
        escaped: false,
        reads: run
            .reads
            .iter()
            .map(|r| recorder::LoggedRead {
                path: r.path.clone(),
                request: SValue::from_core(&r.request),
                // A file's text past the inline size is kept by its hash, not in the entry; a range's
                // answer is its text (or the lines of it), kept with the range.
                answer: r.answer.as_ref().filter(|_| r.file.as_ref().is_none_or(|f| f.inline && f.range.is_none())).map(SValue::from_core),
                file: r.file.as_ref().map(|f| recorder::LoggedFile {
                    hash: f.hash.clone(),
                    size: f.size,
                    blob: f.blob.clone(),
                    range: f.range.as_ref().map(|g| recorder::LoggedRange {
                        start: g.start,
                        end: g.end,
                        text: g.text.clone(),
                        before: ranges::hex(&g.before),
                        after: ranges::hex(&g.after),
                        proof: g.proof.iter().map(|cv| ranges::hex(cv)).collect(),
                    }),
                }),
            })
            .collect(),
    }
}

impl FiredRun {
    /// No run at all: an unrouted request.
    pub(crate) fn nothing() -> FiredRun {
        FiredRun {
            outputs: vec![],
            performed: vec![],
            state_before: BTreeMap::new(),
            timeline: vec![],
            rounds: vec![],
            error: None,
            run_id: String::new(),
            unit: String::new(),
            nonce: String::new(),
            grant: String::new(),
            waiting: vec![],
            progress: vec![],
            start_state: None,
            reads: vec![],
            retries: 0,
            claims: vec![],
            secret_fed: taint::SecretFed::default(),
            secrets: BTreeMap::new(),
            answered: recorder::AnswerTokens::default(),
            state_after: None,
        }
    }
}

pub(crate) fn started_now() -> (u64, std::time::Instant) {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    (ms, std::time::Instant::now())
}
