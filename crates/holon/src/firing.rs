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
    /// The state it started from (the live state, or a resumed run's recorded one): what its log
    /// entry says it found.
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

/// Whether `live` is another state than `start`: a key added, gone, or holding another value. The
/// same answer as comparing their snapshots' hashes (`restore::snapshot_hash`), since a snapshot is
/// each key's value hash and nothing else, without hashing either (docs/L4s/request-path-L4.md,
/// C3 a; interpreter-path-L4.md, step 2). A run that only read, or wrote back what was there,
/// changed nothing.
fn changed_from(start: &State, live: &State) -> bool {
    start.len() != live.len() || start.iter().zip(live.iter()).any(|((a, _), (b, _))| a != b || start.hash_of(a) != live.hash_of(b))
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
                        keep_if_changed(cell, &a.run, &live);
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
    let holder = format!("last:{}", holon_core::hex(nonce));
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
                    keep_if_changed(cell, &a.run, &live);
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
    // Nothing read but its cells: nothing more to ask (interpreter-path-L4.md, step 2).
    if reads.is_empty() {
        return true;
    }
    let granted = permit.collections_read();
    // A collection read of one whose root is the one the run read it at is answered the same:
    // the root names its contents. Asked again only when the root moved.
    let same_root = |request: &Value| match request {
        Value::Record(q) => match q.get("collection") {
            Some(Value::Text(name)) if granted.contains(name) => {
                let key = collections::cell_key(name);
                start.hash_of(&key) == live.hash_of(&key)
            }
            _ => false,
        },
        _ => false,
    };
    let mut now: Option<collections::Readable> = None;
    let mut files = folders.map(|f| folders::Readable::new(Arc::clone(f), permit));
    reads.iter().all(|r| {
        // A database's reads are checked in its own commit transaction (databases.rs).
        if matches!(&r.request, Value::Record(q) if q.contains_key("database")) {
            return true;
        }
        if !matches!(&r.request, Value::Record(q) if q.contains_key("folder")) {
            if pinned || same_root(&r.request) {
                return true;
            }
            let now = now.get_or_insert_with(|| collections::Readable { store: store.clone(), roots: collections::roots(live), granted: granted.clone() });
            return now.answer(&r.request) == r.answer;
        }
        files.as_mut().is_none_or(|files| files.answer(&r.request, false).is_ok_and(|(answer, _)| answer == r.answer))
    })
}

/// The state a run left, kept as a snapshot (its hash), taken under the state's lock as it commits:
/// the cells it changed since `start` go into the content store, then the snapshot. A store that
/// can't take it leaves the run without one (it can't be restored to), and says so.
/// Write the deployment's state head when `run` changed its state (C3 b): its snapshot is in the
/// store already, and the cell's lock is held, so heads land in commit order. A run that changed
/// nothing writes nothing, so a read costs no fsync.
fn keep_if_changed(cell: &claims::StateCell, run: &FiredRun, live: &State) {
    if let (Some(start), Some(snapshot)) = (&run.start_state, &run.state_after)
        && changed_from(start, live)
    {
        cell.write_head(snapshot);
    }
}

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
            match live.insert(key.clone(), payload.clone()) {
                Ok(()) => {}
                Err(InsertError::NotData) => eprintln!("holon: refused to store an effect request in state key {key:?}"),
                Err(InsertError::Unencodable(e)) => eprintln!("holon: state key {key:?} not written: {e}"),
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
    let run_id = run_id_of(permit.deployment, inputs, start.as_ref(), extra_sourced, nonce);
    // Its resume tokens are derived from its id (durable.rs).
    // Hex once: its resume tokens, its claims and its entry all name it so.
    let run_hex = to_hex(&run_id);
    waits.run_id = Some(run_hex.clone());
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

    // The secrets it was granted. Not part of the run's input record (nor its id): a log never
    // holds them, and a replay stands a placeholder in.
    let mut given = BTreeMap::new();
    let mut granted = Vec::new();
    let vault = secrets.lock().unwrap();
    for (name, value) in vault.current.iter() {
        let cap = Capability::SecretRead(name.clone());
        if permit.allows(&cap) {
            granted.push((cap, Value::Text(value.to_string())));
            given.extend(vault.shown.get(name).map(|shown| (name.clone(), shown.clone())));
        }
    }
    drop(vault);
    // Only the state keys the deployment was granted are read, each lent as the state holds it.
    let sourced: Vec<(Capability, &Value)> = start
        .iter()
        .flat_map(State::iter)
        .map(|(key, value)| (Capability::StateRead(key.clone()), value))
        .filter(|(cap, _)| permit.allows(cap))
        .chain(extra_sourced.iter().chain(&granted).map(|(cap, value)| (cap.clone(), value)))
        .collect();

    // Which nodes compute with a secret it was granted, for its log entry.
    let secret_fed = if sourced.iter().any(|(c, _)| matches!(c, Capability::SecretRead(_))) {
        taint::SecretFed::of(root_unit, resolver)
    } else {
        taint::SecretFed::default()
    };
    // And where each may be sent: a request holding one's value goes only there.
    let secret_sends = (!secret_fed.is_empty()).then(|| folders::SecretSends { vault: secrets, sent: holon_core::sent_secrets(root_unit, resolver) });

    let mut host = RecordingHost { performed: Vec::new() };
    let mut timer = if waits.unrecorded { recorder::Timer::off() } else { recorder::Timer::new() };
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
            run_hex.clone(),
            limits.claim_wait,
            records,
        )
    });
    // A file as a request's body is sent from a folder the deployment reads.
    let forward = waits.forward.clone();
    let (slug, run, chain) = (waits.slug.clone().unwrap_or_default(), waits.run_id.clone().unwrap_or_default(), waits.chain.clone());
    let caller = calls::Caller { store, fed: &secret_fed, permit, vault: secrets, slug: &slug, run: &run, chain: &chain, unrecorded: waits.unrecorded, #[cfg(test)] fixed: fixed::peek() };
    let sender = folders::Sender {
        outbound: &permit.outbound,
        folders: waits.folders.clone(),
        granted: permit.folders_handed(),
        forward: forward.as_ref(),
        channels: waits.channels.clone(),
        sends: permit.channels_sent(),
        calls: Some(&caller),
        secret_sends,
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
        timeline: timer.timeline,
        rounds: outcome.rounds,
        error: failed_read
            .or(past_limit.map(|e| format!("{}{e}", folders::WRITE_FAILED)))
            .or(past_packs.map(|e| format!("{}{e}", channels::SEND_FAILED)))
            .or(outcome.error.map(|e| e.message())),
        run_id: run_hex,
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
    #[cfg(test)]
    if let Some(n) = fixed::next() {
        nonce[..8].copy_from_slice(&n.to_le_bytes());
        return nonce;
    }
    getrandom::fill(&mut nonce).expect("the OS has a secure random generator");
    nonce
}

/// The clock a run is sourced with, in seconds since the epoch.
pub(crate) fn clock_seconds() -> u64 {
    #[cfg(test)]
    if let Some(n) = fixed::next() {
        return 1_800_000_000 + n;
    }
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// The random bytes a run is sourced with: 32 of them, from which `random` reads its number and
/// `pgp_encrypt` a message's randomness (host-capabilities L4 §5.1.11).
pub(crate) fn random_bytes() -> Value {
    #[cfg(test)]
    if let Some(n) = fixed::next() {
        return Value::Bytes(blake3::hash(&n.to_le_bytes()).as_bytes().to_vec());
    }
    Value::Bytes(rand::random::<[u8; 32]>().to_vec())
}

/// In a test, the sources made fixed: a counter, from which each run's clock, random bytes and
/// nonce follow (the server's golden runs, tests.rs). Unset, they are the system's.
#[cfg(test)]
pub(crate) mod fixed {
    use std::cell::Cell;

    thread_local! {
        static NEXT: Cell<Option<u64>> = const { Cell::new(None) };
    }

    /// Fix this thread's sources from `n` on; none, back to the system's.
    pub(crate) fn from(n: Option<u64>) {
        NEXT.with(|c| c.set(n));
    }

    /// This thread's next one, not taken: what a run's calls start from on their own threads.
    pub(crate) fn peek() -> Option<u64> {
        NEXT.with(|c| c.get())
    }

    pub(super) fn next() -> Option<u64> {
        NEXT.with(|c| {
            let n = c.get()?;
            c.set(Some(n + 1));
            Some(n)
        })
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    holon_core::hex(bytes)
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
/// resolves it through `closure` exactly as the cache endpoints do, rather than accepting the
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
    let resolver = match closure(store, root_hash) {
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
///
/// The table is held only to take what the entry needs; the entry is redacted with no lock held,
/// then numbered, chained and written under the deployment's own log lock, so a run of one
/// deployment never waits on another's log (docs/L4s/request-path-L4.md, C2).
///
/// How the run is recorded is its handler's level (`declared`, C5), as its grant holds it
/// (`Permit::log_level`): a strict run returns once its entry is on disk, sharing the sync with
/// the runs around it (C2 e); a lazy one is synced within `SYNC_EVERY`; an unrecorded one is
/// counted in its handler's tally and leaves no entry, failed or not (resilience-audit-L4.md,
/// §2.8, which reversed §8 Q10). Answers the seq the entry got (none for an unrecorded run that left none),
/// and whether it is known kept.
///
/// Its packs go out once its record is on disk (channels.rs): a strict run's, with those of every
/// run before it, once its sync is done; a lazy one's with the next sync (`sync_logs`).
pub(crate) fn log_run(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, entry: LogEntry, tokens: Redactor, declared: holon_core::LogLevel) -> Option<Logged> {
    let Some(Pushed { log, seq, level, channels, permit }) = push_run(deployments, slug, entry, tokens, declared)? else {
        return Some(Logged { seq: None, kept: Ok(()) });
    };
    let kept = match level {
        holon_core::LogLevel::Strict => log.sync_through(seq),
        holon_core::LogLevel::Lazy | holon_core::LogLevel::Unrecorded => return Some(Logged { seq: Some(seq), kept: Ok(()) }),
    };
    if kept.is_ok() {
        channels.release(seq + 1, &permit.channels);
    }
    Some(Logged { seq: Some(seq), kept })
}

/// What counting an unrecorded run takes: its route (none: no route took it), when it ran, how
/// long it took, its answer's status and its error, if any: it failed with an error, or a 5xx.
pub(crate) struct Counted<'a> {
    pub(crate) route: Option<&'a str>,
    pub(crate) at_ms: u64,
    pub(crate) duration_ms: f64,
    pub(crate) status: u16,
    pub(crate) error: Option<&'a str>,
}

impl Counted<'_> {
    fn failed(&self) -> bool {
        self.error.is_some() || self.status >= 500
    }
}

/// [`log_run`], the entry (and the tokens it is redacted of) made only if it is logged: a run its
/// grant has unrecorded is counted under the table's lock, as `log_run` counts it, and nothing more
/// is made of it (docs/L4s/interpreter-path-L4.md, the per-request bookkeeping), unless it `sends`
/// packs, which go out live ([`send_unlogged`]). Otherwise `log_run`, which reads the level again.
#[allow(clippy::too_many_arguments)]
pub(crate) fn log_or_count(
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    slug: &str,
    counted: Counted,
    entry: impl FnOnce() -> LogEntry,
    tokens: impl FnOnce() -> Redactor,
    declared: holon_core::LogLevel,
    sends: bool,
) -> Option<Logged> {
    let sending = {
        let mut table = deployments.lock().unwrap();
        let dep = table.get_mut(slug)?;
        if !count_if_unrecorded(dep, &counted, declared).1 {
            None
        } else if sends {
            Some((Arc::clone(&dep.channels), Arc::clone(&dep.secrets)))
        } else {
            return Some(Logged { seq: None, kept: Ok(()) });
        }
    };
    if let Some((channels, secrets)) = sending {
        send_unlogged(&channels, &secrets, entry(), tokens());
        return Some(Logged { seq: None, kept: Ok(()) });
    }
    log_run(deployments, slug, entry(), tokens(), declared)
}

/// An unrecorded run's packs (channels.rs): what it sent, or for a part that settled waiting what
/// it settled, redacted as a logged run's are, but sent live, with no id, and never kept. A pack's
/// id is its run's log seq and the kept packs are rebuilt from the log, which doesn't hold the
/// run: a subscriber there now gets them, and one that reconnects, or comes after a restart,
/// doesn't.
fn send_unlogged(channels: &channels::Channels, secrets: &Arc<Mutex<secrets::Resolved>>, mut entry: LogEntry, tokens: Redactor) {
    if entry.performed.iter().chain(&entry.progress).any(|p| p.capability.starts_with("ChannelSend(")) {
        let answered = std::mem::take(&mut entry.answered);
        tokens.and(Redactor::of(secrets)).and(Redactor::answers(&answered.0, secrets)).entry(&mut entry);
    }
    // Through `packs_of` all the same: a parked run's earlier parts' sends aren't sent again, and
    // a finished run lets go of them.
    for (name, pack) in channels.packs_of(&entry, 0) {
        channels.send_live(&name, pack.json);
    }
}

/// [`count_if_unrecorded`] for a test, the handler declared unrecorded: whether it was counted only.
#[cfg(test)]
pub(crate) fn count_for_tests(dep: &mut Deployment, counted: &Counted) -> bool {
    count_if_unrecorded(dep, counted, holon_core::LogLevel::Unrecorded).1
}

/// The level `dep`'s grant logs a run at now, and, for an unrecorded run, the run counted in its
/// route's tally, a failure with its status and its error (redacted of the deployment's secrets, cut
/// short); with whether that is all there is to do, which for an unrecorded run it always is: its
/// handler was declared so, and its runs are never logged, failed or not (resilience-audit-L4.md,
/// §2.8). The one rule `push_run` and `log_or_count` both keep (D2). Called with the table's lock
/// held.
fn count_if_unrecorded(dep: &mut Deployment, counted: &Counted, declared: holon_core::LogLevel) -> (holon_core::LogLevel, bool) {
    let level = dep.permit.log_level(counted.route, declared);
    if level != holon_core::LogLevel::Unrecorded {
        return (level, false);
    }
    let failure = counted.failed().then(|| wire::Failure {
        at_ms: counted.at_ms,
        status: counted.status.max(if counted.error.is_some() { 500 } else { 0 }),
        error: counted.error.map(|e| {
            const MOST: usize = 300;
            let e = Redactor::of(&dep.secrets).text(e);
            match e.char_indices().nth(MOST) {
                Some((cut, _)) => format!("{}…", &e[..cut]),
                None => e,
            }
        }),
    });
    dep.tallies.entry(counted.route.unwrap_or_default().to_string()).or_default().add(counted.at_ms, counted.duration_ms, failure);
    dep.tallies_changed = true;
    (level, true)
}

/// A run as `log_run` logged it: its entry's seq (none: unrecorded, so only counted), and, for
/// a strict run, whether the entry is on disk (an error says why not).
#[derive(Debug)]
pub(crate) struct Logged {
    pub(crate) seq: Option<u64>,
    pub(crate) kept: Result<(), String>,
}

impl Logged {
    /// Said on the console when a strict run's record isn't known kept: for a run that answers
    /// nobody (a clock's, a file's, a resumed run's).
    pub(crate) fn said(self, slug: &str) {
        if let Err(e) = &self.kept {
            eprintln!("holon: {slug:?}: a run's record may not be on disk: {e}");
        }
    }
}

/// A run's entry as `push_run` wrote it: the log it went to, its seq, its level, and the channels
/// its packs are held in until it is on disk, with the deployment's limits for them.
struct Pushed {
    log: Arc<recorder::LogCell>,
    seq: u64,
    level: holon_core::LogLevel,
    channels: Arc<channels::Channels>,
    permit: Arc<Permit>,
}

/// `log_run`'s write: the run's level as its grant holds it, and, unless it is an unrecorded run
/// (counted, its packs sent live, and nothing written: `Some(None)`), the entry pushed under the
/// deployment's log lock, its packs held. None: no such deployment.
fn push_run(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, mut entry: LogEntry, tokens: Redactor, declared: holon_core::LogLevel) -> Option<Option<Pushed>> {
    let (log, secrets, channels, permit, session, level, only_counted) = {
        let mut table = deployments.lock().unwrap();
        let dep = table.get_mut(slug)?;
        let counted = Counted { route: entry.route.as_deref(), at_ms: entry.at_ms, duration_ms: entry.duration_ms, status: entry.status.unwrap_or(200), error: entry.error.as_deref() };
        let (level, only_counted) = count_if_unrecorded(dep, &counted, declared);
        let session = dep.life.sessions.last().map_or(0, |s| s.number);
        (Arc::clone(&dep.log), Arc::clone(&dep.secrets), Arc::clone(&dep.channels), Arc::clone(&dep.permit), session, level, only_counted)
    };
    if only_counted {
        send_unlogged(&channels, &secrets, entry, tokens);
        return Some(None);
    }
    let answered = std::mem::take(&mut entry.answered);
    let redactor = tokens.and(Redactor::of(&secrets)).and(Redactor::answers(&answered.0, &secrets));
    entry.session = session;
    // Its line made before the lock is taken, so runs of one deployment don't wait while another
    // run's is: under the lock only its seq and the line before's hash go in.
    let made = redacted_line(&redactor, &mut entry);
    let seq = log.with(|log| {
        // Its packs are held in the log's order, to go out once its record is on disk: the run is
        // over and its writes have landed, and what a subscriber gets is what the log holds, after
        // a power loss too, so no pack's id is given twice (channels.rs).
        let packs = channels.packs_of(&entry, log.next_seq());
        let seq = log.push_made(entry, made);
        channels.hold(seq, packs);
        seq
    });
    Some(Some(Pushed { log, seq, level, channels, permit }))
}

/// `entry` redacted by `redactor`, and its line. With nothing to mark, redacting can only escape a
/// look-alike of a mark (text with a `‹`), an entry's records being in key order already
/// (`SValue::from_core`): a line without one is the line redaction would make, so it is made once
/// and looked at, not walked (docs/L4s/interpreter-path-L4.md, the per-request bookkeeping).
fn redacted_line(redactor: &Redactor, entry: &mut LogEntry) -> recorder::Made {
    if redactor.is_empty() {
        let made = recorder::Made::of(entry);
        if !made.may_hold_a_mark() {
            // In a debug build (every test), checked against redacting first: the line and the
            // entry the same, or what the shortcut assumes stopped holding (D3).
            #[cfg(debug_assertions)]
            {
                let mut walked = entry.clone();
                redactor.entry(&mut walked);
                assert_eq!(made.line(1, ""), recorder::Made::of(&walked).line(1, ""), "the shortcut's line isn't the redacted one");
                assert_eq!(serde_json::to_value(&walked).ok(), serde_json::to_value(&*entry).ok(), "the shortcut's entry isn't the redacted one");
            }
            return made;
        }
    }
    redactor.entry(entry);
    recorder::Made::of(entry)
}

/// The timeline, with the outputs of each node that carries the text or bytes of a file the log
/// keeps by hash (read, or written past the inline size) left out: they aren't in the log by
/// another way.
fn without_kept_files(mut timeline: Vec<recorder::NodeTiming>, reads: &[collections::Read], performed: &[(Capability, Value)]) -> Vec<recorder::NodeTiming> {
    let read = reads.iter().filter(|r| r.file.as_ref().is_some_and(|f| !f.inline)).filter_map(|r| match &r.answer {
        Some(Value::Text(t)) => Some(t.as_str()),
        _ => None,
    });
    // A range's lines, as a node carries them: the list whole.
    let lines = reads.iter().filter(|r| r.file.as_ref().is_some_and(|f| !f.inline)).filter_map(|r| match &r.answer {
        Some(v @ Value::List(l)) if !l.is_empty() => serde_json::to_string(&SValue::from_core(v)).ok(),
        _ => None,
    });
    // A file read or written as bytes, as a node carries them: their base64, whole.
    let bytes = reads
        .iter()
        .filter(|r| r.file.as_ref().is_some_and(|f| !f.inline))
        .filter_map(|r| match &r.answer {
            Some(Value::Bytes(b)) if !b.is_empty() => Some(b.as_slice()),
            _ => None,
        })
        .chain(folders::written_bytes(performed))
        .filter_map(|b| holon_core::bytes_text(b, "base64"));
    let kept: Vec<String> = read
        .chain(folders::written_texts(performed))
        .filter(|t| !t.is_empty())
        .filter_map(|t| serde_json::to_string(t).ok().map(|q| q[1..q.len() - 1].to_string()))
        .chain(lines)
        .chain(bytes)
        .collect();
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

/// [`withheld_or`], the value made only when it is shown.
fn withheld_or_shown(derived: bool, shown: impl FnOnce() -> Value) -> SValue {
    if derived { withheld_or(true, &Value::Boolean(false)) } else { SValue::from_core(&shown()) }
}

/// A run's outputs as its log entry and history keep them (`shown`: how each looks there), each
/// that may be computed from a secret withheld.
pub(crate) fn logged_outputs(run: &FiredRun, shown: impl Fn(&Value) -> Value) -> Vec<Option<SValue>> {
    run.outputs.iter().enumerate().map(|(port, o)| o.as_ref().map(|v| withheld_or_shown(run.secret_fed.derives_output(port), || shown(v)))).collect()
}

/// What a run performed as its log entry and history keep it (`shown`: how each payload looks
/// there), each payload that may be computed from a secret withheld.
pub(crate) fn logged_performed(run: &FiredRun, shown: impl Fn(&Capability, &Value) -> Value) -> Vec<PerformedEntry> {
    run.performed
        .iter()
        .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: withheld_or_shown(run.secret_fed.derives_effect(c), || shown(c, p)) })
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
    entry_with(kind, started, sourced, run, run.timeline.clone())
}

/// [`run_entry`], its timeline moved out of `run` instead of copied: for a run nothing reads after
/// its entry is made (docs/L4s/interpreter-path-L4.md, the per-request bookkeeping).
pub(crate) fn run_entry_taking(
    kind: &'static str,
    started: (u64, std::time::Instant),
    sourced: &[(Capability, Value)],
    run: &mut FiredRun,
) -> LogEntry {
    let timeline = std::mem::take(&mut run.timeline);
    entry_with(kind, started, sourced, run, timeline)
}

/// A run's entry, with `timeline` as the run's timeline.
fn entry_with(
    kind: &'static str,
    started: (u64, std::time::Instant),
    sourced: &[(Capability, Value)],
    run: &FiredRun,
    timeline: Vec<recorder::NodeTiming>,
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
        state_before: run.start_state.iter().flat_map(State::iter).map(|(k, v)| (k.clone(), SValue::from_core(v))).collect(),
        state_after: run.state_after.clone(),
        outputs: logged_outputs(run, folders::loggable_output),
        performed: logged_performed(run, folders::loggable),
        response_body: None,
        error: run.error.clone(),
        timeline: without_secret_derived(without_kept_files(timeline, &run.reads, &run.performed), &run.secret_fed),
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

/// The commit's checks made cheaper in step 2 (docs/L4s/interpreter-path-L4.md): each gives what
/// the work it replaces gave.
#[cfg(test)]
mod commit_checks {
    use super::*;

    fn state_of(pairs: &[(&str, f64)]) -> State {
        State::from_values(pairs.iter().map(|(k, v)| (k.to_string(), Value::Number(*v))).collect()).unwrap()
    }

    /// `changed_from` says what comparing the two states' snapshot hashes says.
    #[test]
    fn a_state_changed_exactly_when_its_snapshot_hash_moved() {
        let states = [
            state_of(&[]),
            state_of(&[("a", 1.0)]),
            state_of(&[("a", 2.0)]),
            state_of(&[("b", 1.0)]),
            state_of(&[("a", 1.0), ("b", 1.0)]),
            state_of(&[("a", 1.0), ("b", 2.0)]),
            state_of(&[("a", 2.0), ("b", 1.0)]),
        ];
        for x in &states {
            for y in &states {
                let by_hash = crate::restore::snapshot_hash(x).unwrap() != crate::restore::snapshot_hash(y).unwrap();
                assert_eq!(changed_from(x, y), by_hash, "{:?} → {:?}", x.values(), y.values());
            }
        }
    }

    /// A collection read whose collection's root is still the one it was read at isn't asked again,
    /// and comes out as asking again would have; one whose root moved is asked again, and holds
    /// only when its answer is still the same.
    #[test]
    fn a_collection_read_is_asked_again_only_when_its_root_moved() {
        let store = SharedStore::new(Box::new(MemStore::new()));
        let mut writer = store.clone();
        let put = |key: &str, value: &str| Value::Record(BTreeMap::from([("key".to_string(), Value::Text(key.into())), ("value".to_string(), Value::Text(value.into()))]));
        let mut start = State::new();
        collections::apply(&mut start, &mut writer, "notes", &put("a", "one")).unwrap();
        let grant: permissions::Grant = serde_json::from_value(serde_json::json!({
            "manifest": "", "items": ["CollectionRead(\"notes\")"], "requirements": {}, "granted_at": 0, "by": "deploy",
        }))
        .unwrap();
        let permit = Permit::new([0; 32], &grant, &Policy::parse("").unwrap());
        let request = Value::Record(BTreeMap::from([("collection".to_string(), Value::Text("notes".into())), ("get".to_string(), Value::Text("a".into()))]));
        let read = |answer: &str| collections::Read { path: vec![0], request: request.clone(), answer: Some(Value::Text(answer.into())), file: None };
        let asked = |live: &State, r: &collections::Read| {
            let now = collections::Readable { store: store.clone(), roots: collections::roots(live), granted: permit.collections_read() };
            now.answer(&r.request) == r.answer
        };
        let check = |live: &State, r: collections::Read| {
            let held = still_as_read(&start, live, std::slice::from_ref(&r), &permit, &store, None, false);
            assert_eq!(held, asked(live, &r), "the same as asking again");
            held
        };

        // The same root: held, as asking again holds it.
        assert!(check(&start.clone(), read("one")));
        // Another item changed: the root moved, it is asked again, and its answer still holds.
        let mut other = start.clone();
        collections::apply(&mut other, &mut writer, "notes", &put("b", "two")).unwrap();
        assert!(check(&other, read("one")));
        // Its item changed: asked again, and found changed.
        let mut changed = start.clone();
        collections::apply(&mut changed, &mut writer, "notes", &put("a", "uno")).unwrap();
        assert!(!check(&changed, read("one")));
        // Nothing read: held.
        assert!(still_as_read(&start, &changed, &[], &permit, &store, None, false));
    }

    /// A line made without walking the entry (`redacted_line`, nothing to mark and no `‹` in it) is
    /// the line redacting it first makes, and the entry is the one redaction leaves; with a `‹`
    /// anywhere, or something to mark, it is redacted.
    #[test]
    fn a_line_with_nothing_to_redact_is_the_redacted_line() {
        let text = |s: &str| Value::Text(s.into());
        let request = |body: &str| {
            Value::Record(BTreeMap::from([
                ("method".to_string(), text("GET")),
                ("headers".to_string(), Value::Record(BTreeMap::from([("x-b".to_string(), text("2")), ("x-a".to_string(), text("1"))]))),
                ("body".to_string(), text(body)),
            ]))
        };
        let entry = |body: &str| {
            let mut e = run_entry("request", (0, std::time::Instant::now()), &[(Capability::Clock, Value::Number(1.0))], &FiredRun::nothing());
            e.request = Some(SValue::from_core(&request(body)));
            e.response_body = Some(body.to_string());
            e.duration_ms = 0.0;
            e
        };
        let tokens = |pairs: &[(&str, &str)]| Redactor::tokens(&pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
        for body in ["plain", "with ‹ in it", "‹token:paid›", "‹‹secret:x›", "a {\"‹k\": 1}", ""] {
            for redactor in [tokens(&[]), tokens(&[("paid", "tok-1")])] {
                let (mut fast, mut walked) = (entry(body), entry(body));
                let line = redacted_line(&redactor, &mut fast);
                redactor.entry(&mut walked);
                let line_walked = recorder::Made::of(&walked);
                assert_eq!(line.line(1, ""), line_walked.line(1, ""), "body {body:?}");
                // The entry the log keeps in memory too: as redaction leaves it.
                assert_eq!(serde_json::to_value(&fast).unwrap(), serde_json::to_value(&walked).unwrap(), "body {body:?}");
            }
        }
    }

    /// A store that fails every read while `failing` is set.
    struct Flaky {
        inner: MemStore,
        failing: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Store for Flaky {
        fn put(&mut self, unit: &Unit) -> Result<Hash, StoreError> {
            self.inner.put(unit)
        }
        fn get(&self, key: &Hash) -> Result<Option<Unit>, StoreError> {
            self.inner.get(key)
        }
        fn put_value(&mut self, value: &Value) -> Result<Hash, StoreError> {
            self.inner.put_value(value)
        }
        fn get_value(&self, key: &Hash) -> Result<Option<Value>, StoreError> {
            if self.failing.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(StoreError::Io(std::io::Error::other("the disk is away")));
            }
            self.inner.get_value(key)
        }
    }

    /// The store failing as a run commits: a run whose collection didn't move commits on what it
    /// read, where asking again would have failed and sent it round again for nothing; a run whose
    /// collection moved is asked again as before, and a read that fails then counts as changed,
    /// so it runs again, whether or not its item changed. Once the store is back, a moved root
    /// whose item is unchanged holds.
    #[test]
    fn a_store_failing_at_the_commit_check_fails_only_a_run_whose_collection_moved() {
        let failing = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // In memory: the store keeps no values beside it, so a failing read fails.
        let store = SharedStore::new(Box::new(Flaky { inner: MemStore::new(), failing: Arc::clone(&failing) }));
        let mut writer = store.clone();
        let put = |key: &str, value: &str| Value::Record(BTreeMap::from([("key".to_string(), Value::Text(key.into())), ("value".to_string(), Value::Text(value.into()))]));
        let mut start = State::new();
        collections::apply(&mut start, &mut writer, "notes", &put("a", "one")).unwrap();
        let mut moved = start.clone();
        collections::apply(&mut moved, &mut writer, "notes", &put("b", "two")).unwrap();
        let mut changed = start.clone();
        collections::apply(&mut changed, &mut writer, "notes", &put("a", "uno")).unwrap();
        let grant: permissions::Grant = serde_json::from_value(serde_json::json!({
            "manifest": "", "items": ["CollectionRead(\"notes\")"], "requirements": {}, "granted_at": 0, "by": "deploy",
        }))
        .unwrap();
        let permit = Permit::new([0; 32], &grant, &Policy::parse("").unwrap());
        let request = Value::Record(BTreeMap::from([("collection".to_string(), Value::Text("notes".into())), ("get".to_string(), Value::Text("a".into()))]));
        // What the run read, at its start, while the store answered.
        let answer = collections::Readable { store: store.clone(), roots: collections::roots(&start), granted: permit.collections_read() }.answer(&request);
        assert_eq!(answer, Some(Value::Text("one".into())));
        let reads = [collections::Read { path: vec![0], request: request.clone(), answer, file: None }];
        let held = |live: &State| still_as_read(&start, live, &reads, &permit, &store, None, false);

        failing.store(true, std::sync::atomic::Ordering::SeqCst);
        let asked_again = collections::Readable { store: store.clone(), roots: collections::roots(&start), granted: permit.collections_read() }.answer(&request);
        assert_eq!(asked_again, None, "the store fails: asking again finds nothing");
        assert!(held(&start.clone()), "its collection didn't move: it commits on what it read");
        assert!(!held(&moved), "its collection moved: asked again, the failed read counts as changed");
        assert!(!held(&changed), "its item changed: it runs again");

        failing.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(held(&moved), "the store back: the moved root still holds its item");
        assert!(!held(&changed));
    }
}
