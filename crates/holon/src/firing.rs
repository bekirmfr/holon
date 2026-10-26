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
    let attempt = |start: Option<State>, nonce: &[u8; 16], prior: Vec<Round>, waits: &mut RunWaits, claim: Option<(&Arc<claims::StateCell>, Vec<claims::ClaimRecord>)>| {
        run_attempt(root_unit, inputs, resolver, store, start, extra_sourced, permit, clock, secrets, waits, nonce, prior, claim)
    };
    let limits = if clock { &permit.clock_limits } else { &permit.request_limits };
    // A resumed run is the same run it started as: pinned to the state it first read, run once,
    // its writes landing on the live state when it finishes.
    if let Some(r) = resumed {
        let a = attempt(state.map(|_| r.state_before), &r.nonce, r.prior, waits, None);
        let mut run = a.run;
        if let Some(cell) = state {
            let failed = commit(&mut cell.state.lock().unwrap(), store, waits.folders.as_deref(), &run.performed);
            not_all_written(&mut run, failed);
        }
        return run;
    }
    let nonce = new_nonce();
    let Some(cell) = state else {
        return attempt(None, &nonce, Vec::new(), waits, None).run;
    };
    // Optimistic: run on a copy, no lock held, so runs of one deployment proceed side by side;
    // then commit only if everything it could have read is still as it found it. Otherwise run
    // again from the state as it now is: the requests it already made are answered from its own
    // record (`prior`), not sent again, and nothing it did is kept but its answer. A run about to
    // call out first claims the keys it read and may write (claims.rs): one that finds them changed
    // as it claims them runs again having sent nothing, and its commit never lands under another
    // run's claim.
    let mut prior = Vec::new();
    let mut records = Vec::new();
    for retries in 0..OPTIMISTIC_ATTEMPTS {
        waits.tokens.clear();
        // Copied in its own statement: a lock taken inside the call's arguments would be held
        // until the call returns, the whole run long.
        let start = cell.state.lock().unwrap().clone();
        let mut a = attempt(Some(start), &nonce, prior, waits, Some((cell, records)));
        if !a.overtaken {
            let written = claims::written_keys(&a.run.performed);
            if !cell.claims.wait_free(&written, &a.run.run_id, std::time::Instant::now() + limits.claim_wait) {
                eprintln!("holon: run {} commits over a key another run still holds (claim_wait passed)", a.run.run_id);
            }
            let mut live = cell.state.lock().unwrap();
            if still_as_read(a.start.as_ref().expect("a stateful run's start"), &live, &a.run.reads, permit, store, waits.folders.as_ref()) {
                let failed = commit(&mut live, store, waits.folders.as_deref(), &a.run.performed);
                not_all_written(&mut a.run, failed);
                a.run.retries = retries;
                if let Some(c) = waits.claiming.take() {
                    a.run.claims = c.finish();
                }
                return a.run;
            }
        }
        records = match waits.claiming.take() {
            Some(c) => c.finish(),
            None => a.run.claims,
        };
        prior = a.run.rounds;
    }
    // Contended that long: run once more holding the lock, so it can't be overtaken again. It
    // claims nothing (it owns everything), once every claim before it is let go.
    waits.tokens.clear();
    if !cell.claims.wait_until_empty(std::time::Instant::now() + limits.claim_wait) {
        eprintln!("holon: a run runs holding the state while a key is still claimed (claim_wait passed)");
    }
    let mut live = cell.state.lock().unwrap();
    let mut a = attempt(Some(live.clone()), &nonce, prior, waits, None);
    let failed = commit(&mut live, store, waits.folders.as_deref(), &a.run.performed);
    not_all_written(&mut a.run, failed);
    a.run.retries = OPTIMISTIC_ATTEMPTS;
    a.run.claims = records;
    a.run
}

/// What a run whose writes didn't all land says: the run happened, and these didn't.
fn not_all_written(run: &mut FiredRun, failed: Vec<String>) {
    if !failed.is_empty() && run.error.is_none() {
        run.error = Some(format!("{}{}", folders::WRITE_FAILED, failed.join("; ")));
    }
}

/// Whether `live` still holds everything a run that started from `start` could have read: each
/// state cell it was granted (by its hash), and each collection and file read it made (asked
/// again; a file as it now is on disk).
fn still_as_read(
    start: &State,
    live: &State,
    reads: &[collections::Read],
    permit: &Permit,
    store: &SharedStore,
    folders: Option<&Arc<folders::Scoped>>,
) -> bool {
    let cells: BTreeSet<&String> = start.iter().chain(live.iter()).map(|(k, _)| k).collect();
    let cells_unchanged = cells
        .into_iter()
        .filter(|k| !k.starts_with('#') && permit.allows(&Capability::StateRead((*k).clone())))
        .all(|k| start.hash_of(k) == live.hash_of(k));
    if !cells_unchanged {
        return false;
    }
    let now = collections::Readable { store: store.clone(), roots: collections::roots(live), granted: permit.collections_read() };
    let mut files = folders.map(|f| folders::Readable::new(Arc::clone(f), permit));
    reads.iter().all(|r| {
        if !matches!(&r.request, Value::Record(q) if q.contains_key("folder")) {
            return now.answer(&r.request) == r.answer;
        }
        files.as_mut().is_none_or(|files| files.answer(&r.request, false).is_ok_and(|(answer, _)| answer == r.answer))
    })
}

/// Apply a run's writes to the live state: each state write, then each collection put and delete,
/// in the order the run made them; then each file written or deleted, in its order. What didn't
/// land, and why: a file that couldn't be written (the others still are).
fn commit(live: &mut State, store: &SharedStore, folders: Option<&folders::Scoped>, performed: &[(Capability, Value)]) -> Vec<String> {
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
    let mut failed = Vec::new();
    for (cap, payload) in performed {
        if let Capability::FileWrite(name) = cap {
            let done = folders.ok_or_else(|| "no folders on this server".to_string()).and_then(|f| folders::apply(f, name, payload));
            if let Err(e) = done {
                eprintln!("holon: {e}");
                failed.push(e);
            }
        }
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
    claim: Option<(&Arc<claims::StateCell>, Vec<claims::ClaimRecord>)>,
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
    for (name, value) in secrets.lock().unwrap().current.iter() {
        let cap = Capability::SecretRead(name.clone());
        if permit.allows(&cap) {
            sourced.push((cap, Value::Text(value.to_string())));
        }
    }

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
    // What this attempt will hold before it sends anything: the keys it reads that it may write.
    waits.claiming = claim.map(|(cell, records)| {
        claims::Claiming::new(
            Arc::clone(cell),
            store.clone(),
            permit.collections_written(),
            permit.collections_read(),
            to_hex(&run_id),
            limits.claim_wait,
            records,
        )
    });
    // A file as a request's body is sent from a folder the deployment reads.
    let forward = waits.forward.clone();
    let sender = folders::Sender {
        outbound: &permit.outbound,
        folders: waits.folders.clone(),
        granted: permit.folders_handed(),
        forward: forward.as_ref(),
        channels: waits.channels.clone(),
        sends: permit.channels_sent(),
    };
    let outcome = run_rounds(root_unit, inputs, &sourced, resolver, &mut host, &sender, limits, &mut timer, waits, &prior);
    let overtaken = outcome.error == Some(holon_host::RunError::Overtaken);
    // A read that failed (a file too big, or not text; a folder not bound) fails the run: it
    // performs nothing.
    let failed_read = waits.failed.take();
    if failed_read.is_some() {
        host.performed.clear();
    }
    // An effect the grant doesn't cover is refused, whatever the graph says; so is a write
    // carrying a secret's value, which would put it on disk and in every later snapshot, and a copy
    // from a folder the deployment doesn't read (a handle is only a record: one can be made up).
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
        if let Capability::StateWrite(key) | Capability::CollectionWrite(key) | Capability::ChannelSend(key) = cap
            && let Some(name) = secrets::secret_in(payload, secrets)
        {
            eprintln!("holon: refused to store or send secret {name:?} in {key:?}");
            return false;
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
        let sends = permit.allows(cap) && secrets::secret_in(payload, secrets).is_none();
        if !sends {
            eprintln!("holon: refused {cap:?} as the run waits: not granted, or carrying a secret");
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
            .map(|e| format!("{}{e}", folders::READ_FAILED))
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

/// Add a run to `slug`'s flow log, in its current session.
pub(crate) fn log_run(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, mut entry: LogEntry) {
    if let Some(dep) = deployments.lock().unwrap().get_mut(slug) {
        Redactor::of(&dep.secrets).entry(&mut entry);
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
        outputs: run.outputs.iter().map(|o| o.as_ref().map(|v| SValue::from_core(&folders::loggable_output(v)))).collect(),
        performed: run
            .performed
            .iter()
            .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: SValue::from_core(&folders::loggable(c, p)) })
            .collect(),
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
            .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: SValue::from_core(p) })
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
        }
    }
}

pub(crate) fn started_now() -> (u64, std::time::Instant) {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    (ms, std::time::Instant::now())
}
