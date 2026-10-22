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
/// Locks `state` itself, for the whole round, if this deployment is stateful — the caller
/// hands in the deployment's own `Arc<Mutex<...>>` dereferenced to `&Mutex<...>`, already
/// cloned out from under the deployments-table lock, which is never held here. That's what
/// keeps two triggers of two *different* deployments from ever contending, while two triggers
/// of the *same* deployment correctly serialize instead of racing.
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
    /// The host nonce (hex) in that input, so the id can be recomputed from the log entry.
    pub(crate) nonce: String,
    /// The grant (hash, hex) it was held to.
    pub(crate) grant: String,
}

pub(crate) fn fire_with_state(
    root_unit: &Unit,
    inputs: &[Option<Value>],
    resolver: &impl Resolver,
    state: Option<&Mutex<State>>,
    extra_sourced: &[(Capability, Value)],
    permit: &Permit,
    clock: bool,
) -> FiredRun {
    let mut guard = state.map(|m| m.lock().unwrap());
    let state_before = guard.as_deref().map(State::values).unwrap_or_default();
    let nonce = new_nonce();
    let run_id = run_id_of(permit.deployment, inputs, guard.as_deref(), extra_sourced, &nonce);

    // Only the state keys the deployment was granted are read.
    let mut sourced: Vec<(Capability, Value)> = match guard.as_deref() {
        Some(storage) => storage
            .iter()
            .map(|(key, value)| (Capability::StateRead(key.clone()), value.clone()))
            .filter(|(cap, _)| permit.allows(cap))
            .collect(),
        None => Vec::new(),
    };
    sourced.extend_from_slice(extra_sourced);

    let mut host = RecordingHost { performed: Vec::new() };
    let mut timer = recorder::Timer::new();
    // Outbound requests go only where the grant allows, within its limits.
    let limits = if clock { &permit.clock_limits } else { &permit.request_limits };
    let outcome = run_rounds(root_unit, inputs, &sourced, resolver, &mut host, &permit.outbound, limits, &mut timer);
    let outputs = outcome.outputs;
    // An effect the grant doesn't cover is refused, whatever the graph says.
    host.performed.retain(|(cap, _)| {
        let allowed = permit.allows(cap);
        if !allowed {
            eprintln!("holon-server: refused {cap:?}: not granted to this deployment");
        }
        allowed
    });

    if let Some(storage) = guard.as_deref_mut() {
        for (cap, payload) in &host.performed {
            if let Capability::StateWrite(key) = cap {
                if !is_storable(payload) {
                    eprintln!("holon-server: refused to store an effect request in state key {key:?}");
                } else if let Err(e) = storage.insert(key.clone(), payload.clone()) {
                    eprintln!("holon-server: state key {key:?} not written (too large to encode): {e:?}");
                }
            }
        }
    }

    FiredRun {
        outputs,
        performed: host.performed,
        state_before,
        timeline: timer.timeline,
        rounds: outcome.rounds,
        error: outcome.error.map(|e| e.message()),
        run_id: to_hex(&run_id),
        nonce: hex_bytes(&nonce),
        grant: permit.grant.clone(),
    }
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
        kind,
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
        nonce: run.nonce.clone(),
        grant: run.grant.clone(),
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
            nonce: String::new(),
            grant: String::new(),
        }
    }
}

pub(crate) fn started_now() -> (u64, std::time::Instant) {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    (ms, std::time::Instant::now())
}
