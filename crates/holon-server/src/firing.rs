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
/// available at once. `FileRead`/`SecretAccess` are deliberately never in this list: there is
/// no addressing mechanism in the wire surface today (a leaf declaring `FileRead` doesn't say
/// *which* file), so there is nothing honest to supply — a real, separate design question, not
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
    /// The state it started from (the live state, or a resumed run's recorded one).
    pub(crate) start_state: Option<State>,
    /// The collection reads it made, and what each answered.
    pub(crate) reads: Vec<collections::Read>,
    /// How many times it ran again because what it read changed before it could commit.
    pub(crate) retries: u32,
}

/// How many times a run is tried without holding its state's lock before it runs holding it.
const OPTIMISTIC_ATTEMPTS: u32 = 4;

/// What one attempt at a run produced, before it is committed (or found stale and tried again).
struct Attempt {
    run: FiredRun,
    /// What it started from (and validates against at commit).
    start: Option<State>,
}

// Everything a run is held to arrives here, so it takes them all.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fire_with_state(
    root_unit: &Unit,
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
    store: &SharedStore,
    state: Option<&Mutex<State>>,
    extra_sourced: &[(Capability, Value)],
    permit: &Permit,
    clock: bool,
    secrets: &Vault,
    waits: &mut RunWaits,
    resumed: Option<Resumed>,
) -> FiredRun {
    let attempt = |start: Option<State>, nonce: &[u8; 16], prior: Vec<Round>, waits: &mut RunWaits| {
        run_attempt(root_unit, inputs, resolver, store, start, extra_sourced, permit, clock, secrets, waits, nonce, prior)
    };
    // A resumed run is the same run it started as: pinned to the state it first read, run once,
    // its writes landing on the live state when it finishes.
    if let Some(r) = resumed {
        let a = attempt(state.map(|_| r.state_before), &r.nonce, r.prior, waits);
        if let Some(cell) = state {
            commit(&mut cell.lock().unwrap(), store, &a.run.performed);
        }
        return a.run;
    }
    let nonce = new_nonce();
    let Some(cell) = state else {
        return attempt(None, &nonce, Vec::new(), waits).run;
    };
    // Optimistic: run on a copy, no lock held, so runs of one deployment proceed side by side;
    // then commit only if everything it could have read is still as it found it. Otherwise run
    // again from the state as it now is: the requests it already made are answered from its own
    // record (`prior`), not sent again, and nothing it did is kept but its answer.
    let mut prior = Vec::new();
    for retries in 0..OPTIMISTIC_ATTEMPTS {
        waits.tokens.clear();
        // Copied in its own statement: a lock taken inside the call's arguments would be held
        // until the call returns, the whole run long.
        let start = cell.lock().unwrap().clone();
        let mut a = attempt(Some(start), &nonce, prior, waits);
        let mut live = cell.lock().unwrap();
        if still_as_read(a.start.as_ref().expect("a stateful run's start"), &live, &a.run.reads, permit, store) {
            commit(&mut live, store, &a.run.performed);
            a.run.retries = retries;
            return a.run;
        }
        drop(live);
        prior = a.run.rounds;
    }
    // Contended that long: run once more holding the lock, so it can't be overtaken again.
    waits.tokens.clear();
    let mut live = cell.lock().unwrap();
    let mut a = attempt(Some(live.clone()), &nonce, prior, waits);
    commit(&mut live, store, &a.run.performed);
    a.run.retries = OPTIMISTIC_ATTEMPTS;
    a.run
}

/// Whether `live` still holds everything a run that started from `start` could have read: each
/// state cell it was granted (by its hash), and each collection read it made (asked again).
fn still_as_read(start: &State, live: &State, reads: &[collections::Read], permit: &Permit, store: &SharedStore) -> bool {
    let cells: BTreeSet<&String> = start.iter().chain(live.iter()).map(|(k, _)| k).collect();
    let cells_unchanged = cells
        .into_iter()
        .filter(|k| !k.starts_with('#') && permit.allows(&Capability::StateRead((*k).clone())))
        .all(|k| start.hash_of(k) == live.hash_of(k));
    if !cells_unchanged {
        return false;
    }
    let now = collections::Readable { store: store.clone(), roots: collections::roots(live), granted: permit.collections_read() };
    reads.iter().all(|r| now.answer(&r.request) == r.answer)
}

/// Apply a run's writes to the live state: each state write, then each collection put and delete,
/// in the order the run made them.
fn commit(live: &mut State, store: &SharedStore, performed: &[(Capability, Value)]) {
    for (cap, payload) in performed {
        if let Capability::StateWrite(key) = cap {
            if !is_storable(payload) {
                eprintln!("holon-server: refused to store an effect request in state key {key:?}");
            } else if let Err(e) = live.insert(key.clone(), payload.clone()) {
                eprintln!("holon-server: state key {key:?} not written (too large to encode): {e:?}");
            }
        }
    }
    let mut store = store.clone();
    for (cap, payload) in performed {
        if let Capability::CollectionWrite(name) = cap
            && let Err(e) = collections::apply(live, &mut store, name, payload)
        {
            eprintln!("holon-server: collection {name:?} not written: {e}");
        }
    }
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

    let mut host = RecordingHost { performed: Vec::new() };
    let mut timer = recorder::Timer::new();
    // Outbound requests go only where the grant allows, within its limits.
    let limits = if clock { &permit.clock_limits } else { &permit.request_limits };
    let outcome = run_rounds(root_unit, inputs, &sourced, resolver, &mut host, &permit.outbound, limits, &mut timer, waits, &prior);
    // An effect the grant doesn't cover is refused, whatever the graph says; so is a write
    // carrying a secret's value, which would put it on disk and in every later snapshot.
    host.performed.retain(|(cap, payload)| {
        if !permit.allows(cap) {
            eprintln!("holon-server: refused {cap:?}: not granted to this deployment");
            return false;
        }
        if let Capability::StateWrite(key) | Capability::CollectionWrite(key) = cap
            && let Some(name) = secrets::secret_in(payload, secrets)
        {
            eprintln!("holon-server: refused to store secret {name:?} in {key:?}");
            return false;
        }
        true
    });

    let run = FiredRun {
        outputs: outcome.outputs,
        performed: host.performed,
        state_before,
        timeline: timer.timeline,
        rounds: outcome.rounds,
        error: outcome.error.map(|e| e.message()),
        run_id: to_hex(&run_id),
        unit: to_hex(&permit.deployment),
        nonce: hex_bytes(nonce),
        grant: permit.grant.clone(),
        waiting: outcome.waiting,
        start_state: start.clone(),
        reads: std::mem::take(&mut waits.reads),
        retries: 0,
    };
    Attempt { run, start }
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
        Err(e) => return err(500, format!("store error while materializing closure: {e:?}")),
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
        dep.log.push(entry);
    }
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
        outputs: run.outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        performed: run
            .performed
            .iter()
            .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: SValue::from_core(p) })
            .collect(),
        response_body: None,
        error: run.error.clone(),
        timeline: run.timeline.clone(),
        rounds: run.rounds.iter().map(recorder::LoggedRound::from_host).collect(),
        run_id: run.run_id.clone(),
        unit: run.unit.clone(),
        nonce: run.nonce.clone(),
        grant: run.grant.clone(),
        parked: vec![],
        events: BTreeMap::new(),
        retries: run.retries,
        reads: run
            .reads
            .iter()
            .map(|r| recorder::LoggedRead {
                path: r.path.clone(),
                request: SValue::from_core(&r.request),
                answer: r.answer.as_ref().map(SValue::from_core),
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
            start_state: None,
            reads: vec![],
            retries: 0,
        }
    }
}

pub(crate) fn started_now() -> (u64, std::time::Instant) {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    (ms, std::time::Instant::now())
}
