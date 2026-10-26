//! Durable runs (§4.3, §4.4 of docs/PLATFORM_LAYOUT.md): a run that waits for an event, and
//! continues when it arrives, minutes or days later.
//!
//! - **Park.** A run still waiting at an `await` when it settles performs nothing; its caller gets
//!   whatever it answered meanwhile (a 202, say), and the host keeps a [`ParkedRun`]: the unit it
//!   ran (pinned: units are content-addressed, so a redeploy doesn't change it), its inputs, the
//!   state it read (by reference), its sourced values and nonce, the requests it made, and the
//!   events delivered so far. Not its tokens: a token is derived from the run id and the waiting
//!   point under the host's key ([`token_for`]), so it is the same every time it is asked for, and
//!   nothing in a run record or a log would let anyone continue the run.
//! - **Resume.** A request on a route declared `resume = <name>`, carrying a run's token (path
//!   param or query `token`), delivers itself to that run's `await` of that name: the route's own
//!   handler answers the caller, and the run is evaluated again from its record with the event.
//!   It finishes (its effects are performed then, once) or waits again. A token is only accepted
//!   by a resume route of the deployment that issued it, for the waiting point it was minted for.
//! - **Expire.** Each waiting point has a deadline (the `await`'s `expires`); past it,
//!   `{expired: true}` is delivered as its event, so the flow decides what timing out means.
//! - **Limits.** How long a deployment's runs may wait (`max_await_seconds`) and how many may
//!   wait at once (`max_waiting_runs`) are limits the host grants like any other (permissions.rs):
//!   a deploy whose `await` waits longer than granted is refused, and parking holds to the grant
//!   in force.
//! - **Visible.** `GET /deployments/<slug>/runs` lists waiting runs (never their tokens); the flow
//!   log has an entry for each part of a run, under one run id.
//!
//! A resumed run reads the state as it was when the run started (that is what makes it the same
//! run, and replayable); its state writes apply when it finishes, over whatever is there then.

use std::collections::BTreeMap;

use holon_host::{Exchange, Waits, Waiting};

use crate::*;

/// Every `await` in `unit`'s closure, as its name and how long it waits (seconds): what a deploy
/// holds to the deployment's `max_await_seconds`. One whose params don't parse never waits, and
/// isn't listed.
pub(crate) fn awaits_in(unit: &Unit, resolver: &impl Resolver) -> Vec<(String, u64)> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![unit];
    while let Some(u) = stack.pop() {
        if let UnitBody::Primitive { name } = &u.body
            && name == "await"
            && let (Some(n), Some(secs)) = (
                u.params.iter().find(|p| p.name() == "name").and_then(|p| p.value()),
                u.params.iter().find(|p| p.name() == "expires").and_then(|p| p.value()).and_then(|v| v.parse().ok()),
            )
        {
            found.push((n.to_string(), secs));
        }
        for child in child_refs(u) {
            if seen.insert(child)
                && let Some(c) = resolver.resolve(&child)
            {
                stack.push(c);
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Why `permit` can't hold `unit`'s waits, if it can't: an `await` that waits longer than the
/// deployment may.
pub(crate) fn refuses_waits(unit: &Unit, resolver: &impl Resolver, permit: &Permit) -> Option<String> {
    let max = permit.durable.max_await_seconds;
    let (name, secs) = awaits_in(unit, resolver).into_iter().find(|(_, secs)| *secs > max)?;
    Some(format!(
        "the await {name:?} waits up to {secs} s, longer than this deployment may ({max} s): ask for max_await_seconds={secs}"
    ))
}

/// The host's key for resume tokens: 32 bytes from the OS's secure generator, kept in a file of
/// its own (`--host-key`) so outstanding tokens survive a restart. Without one (the tests, an
/// in-memory run) it is made fresh for this process.
static HOST_KEY: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();

fn random_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).expect("the OS has a secure random generator");
    key
}

/// Read the host key at `path`, or make one and write it there (owner-only where the OS allows);
/// `":memory:"` keeps a fresh one in memory. Called once, at startup. A file that exists but isn't
/// a key is an error: replacing it would invalidate every token handed out.
pub(crate) fn load_host_key(path: &str) -> Result<(), String> {
    let key = if path == ":memory:" {
        random_key()
    } else {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                // One written before keys were restricted on every OS is restricted now.
                let _ = crate::sealing::restrict_to_owner(std::path::Path::new(path));
                let hex = text.trim();
                let bytes: Option<Vec<u8>> = (0..hex.len())
                    .step_by(2)
                    .map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
                    .collect();
                bytes
                    .and_then(|b| <[u8; 32]>::try_from(b).ok())
                    .ok_or(format!("{path:?} is not a host key (64 hex digits)"))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = random_key();
                let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
                crate::sealing::write_private(std::path::Path::new(path), format!("{hex}\n").as_bytes())
                    .map_err(|e| format!("could not write {path:?}: {e}"))?;
                key
            }
            Err(e) => return Err(format!("could not read {path:?}: {e}")),
        }
    };
    HOST_KEY.set(key).map_err(|_| "the host key was already loaded".to_string())
}

/// The resume token for waiting point `name` of run `run_id` (hex): the first 128 bits of a
/// keyed BLAKE3 MAC under the host key, as `rt_<32 hex>`. The same for the same run and point, as
/// often as it is asked for; underivable without the key, from the run id or anything else a run
/// record or a log holds (§4.4).
pub(crate) fn token_for(run_id: &str, name: &str) -> String {
    let mut data = b"holon resume token v1\0".to_vec();
    data.extend_from_slice(run_id.as_bytes());
    data.push(0);
    data.extend_from_slice(name.as_bytes());
    let mac = blake3::keyed_hash(HOST_KEY.get_or_init(random_key), &data);
    format!("rt_{}", mac.as_bytes()[..16].iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Whether `a` and `b` are equal, comparing every byte: how a presented token is checked, so the
/// time it takes says nothing about how much of a guess was right.
fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// A run's waiting points and tokens: what [`holon_host::run_rounds`] asks for.
#[derive(Default, Debug)]
pub(crate) struct RunWaits {
    /// The run's id (hex), set when the run starts: its tokens are derived from it.
    pub(crate) run_id: Option<String>,
    /// Waiting point → the token the run was handed (so its log entry can be redacted; for a run
    /// parked before tokens were derived, the stored one). In memory only.
    pub(crate) tokens: BTreeMap<String, String>,
    /// Waiting point → the event delivered to it.
    pub(crate) events: BTreeMap<String, Value>,
    /// The collections this run may read, as it found them (collections.rs); none outside a run.
    pub(crate) collections: Option<collections::Readable>,
    /// The folders the deployment sees (folders.rs); none outside a run.
    pub(crate) folders: Option<Arc<folders::Scoped>>,
    /// The folders this run may read, and how much it has read (folders.rs); none outside a run.
    pub(crate) files: Option<folders::Readable>,
    /// The databases this run may read, and how much it has read (databases.rs); none outside a
    /// run.
    pub(crate) databases: Option<databases::Readable>,
    /// The collection and file reads it made, each once, with what each answered.
    pub(crate) reads: Vec<collections::Read>,
    /// Why a read failed the run (a file too big, a folder not bound…), if one did: the run's
    /// error, as it starts (`folders::READ_FAILED`, `databases::READ_FAILED`).
    pub(crate) failed: Option<String>,
    /// The keys this attempt holds (claims.rs), and what it needs to claim more; none for a run
    /// without state, a resumed part, or the attempt that runs holding the state's lock.
    pub(crate) claiming: Option<claims::Claiming>,
}

impl Waits for RunWaits {
    fn claim(&mut self) -> holon_host::Claim {
        match self.claiming.as_mut() {
            Some(c) => c.claim(&self.reads),
            None => holon_host::Claim::Held,
        }
    }
    fn read(&mut self, path: &[u32], request: &Value) -> Option<Value> {
        // A round evaluates again what an earlier one did: the same read, the same answer.
        if let Some(seen) = self.reads.iter().find(|r| r.path == path && &r.request == request) {
            return seen.answer.clone();
        }
        let is_file = matches!(request, Value::Record(r) if r.contains_key("folder"));
        let is_database = matches!(request, Value::Record(r) if r.contains_key("database"));
        let (answer, file) = if is_file {
            match self.files.as_mut().map(|f| f.answer(request, true)) {
                Some(Ok(seen)) => seen,
                Some(Err(e)) => {
                    self.failed.get_or_insert(format!("{}{e}", folders::READ_FAILED));
                    (None, None)
                }
                None => (None, None),
            }
        } else if is_database {
            match self.databases.as_mut().map(|d| d.answer(request)) {
                Some(Ok(answer)) => (answer, None),
                Some(Err(e)) => {
                    self.failed.get_or_insert(format!("{}{e}", databases::READ_FAILED));
                    (None, None)
                }
                None => (None, None),
            }
        } else {
            (self.collections.as_ref().and_then(|c| c.answer(request)), None)
        };
        self.reads.push(collections::Read { path: path.to_vec(), request: request.clone(), answer: answer.clone(), file });
        answer
    }
    fn event(&self, name: &str) -> Option<Value> {
        self.events.get(name).cloned()
    }
    fn token(&mut self, name: &str) -> String {
        let run_id = self.run_id.as_deref().expect("a run's id is set before it runs");
        let derived = token_for(run_id, name);
        self.tokens.entry(name.to_string()).or_insert(derived).clone()
    }
}

/// What a resumed run starts from instead of now: the state it first read, its nonce, and the
/// requests it already made (answered from here, not sent again).
pub(crate) struct Resumed {
    pub(crate) state_before: State,
    pub(crate) nonce: [u8; 16],
    pub(crate) prior: Vec<Round>,
}

/// Everything to run a parked run again: its unit, inputs, sourced values, waits (the new event
/// delivered) and what it started from.
pub(crate) struct Resumption {
    pub(crate) unit: Hash,
    pub(crate) inputs: Vec<Option<Value>>,
    pub(crate) sourced: Vec<(Capability, Value)>,
    pub(crate) waits: RunWaits,
    pub(crate) resumed: Resumed,
}

/// One waiting point of a parked run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct StoredWaiting {
    pub(crate) name: String,
    /// When it expires, seconds since the epoch.
    pub(crate) expires_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct StoredExchange {
    pub(crate) path: Vec<u32>,
    pub(crate) request: SValue,
    pub(crate) response: SValue,
    pub(crate) start_us: f64,
    pub(crate) end_us: f64,
}

/// A run waiting for an event. Persisted with its deployment.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct ParkedRun {
    pub(crate) run_id: String,
    /// The unit it ran (hex): the endpoint for a request, the deployment's unit otherwise.
    pub(crate) unit: String,
    /// `"request"`, `"trigger"` or `"tick"`, and the route, for a request.
    pub(crate) kind: String,
    pub(crate) route: Option<String>,
    pub(crate) inputs: Vec<Option<SValue>>,
    /// The sourced values it saw (`"Clock"`, `"Random"`, `"NetworkIngress"`).
    pub(crate) sourced: Vec<(String, SValue)>,
    /// The state it read, by reference (absent for a stateless deployment).
    pub(crate) state_refs: Option<BTreeMap<String, String>>,
    pub(crate) nonce: String,
    /// The requests it made, round by round, over all its parts so far.
    pub(crate) rounds: Vec<Vec<StoredExchange>>,
    /// Waiting point → token, for a run parked before tokens were derived (they were minted at
    /// random then): still accepted, and handed out again on resume. Empty, and not written, for
    /// every run since.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) tokens: BTreeMap<String, String>,
    /// Waiting point → the event delivered to it.
    pub(crate) events: BTreeMap<String, SValue>,
    pub(crate) waiting: Vec<StoredWaiting>,
    pub(crate) parked_at: u64,
    /// The deployment version (its unit, hex) the run started on: each part it runs later is
    /// logged as of that version, whose canvas shows it. Absent for runs parked before.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) version: String,
}

/// The capability a logged sourced value is for: what a run's trigger and the run's start supply.
pub(crate) fn capability_named(name: &str) -> Option<Capability> {
    match name {
        "Clock" => Some(Capability::Clock),
        "Random" => Some(Capability::Random),
        "NetworkIngress" => Some(Capability::NetworkIngress),
        // The new file that started it (arrivals.rs).
        _ => name.strip_prefix("FileArrival(").and_then(|r| r.strip_suffix(')')).and_then(|q| serde_json::from_str(q).ok()).map(Capability::FileArrival),
    }
}

fn store_rounds(rounds: &[Round]) -> Vec<Vec<StoredExchange>> {
    rounds
        .iter()
        .map(|r| {
            r.exchanges
                .iter()
                .map(|x| StoredExchange {
                    path: x.path.clone(),
                    request: SValue::from_core(&x.request),
                    response: SValue::from_core(&x.response),
                    start_us: x.start_us,
                    end_us: x.end_us,
                })
                .collect()
        })
        .collect()
}

impl ParkedRun {
    /// The record of a run that settled waiting. `prior` are the rounds of its earlier parts.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        run_id: &str,
        unit: &holon_core::Hash,
        kind: &str,
        route: Option<String>,
        inputs: &[Option<Value>],
        sourced: &[(Capability, Value)],
        state_before: Option<&State>,
        nonce: &str,
        rounds: &[Round],
        waits: &RunWaits,
        waiting: &[Waiting],
        now: u64,
        store: &SharedStore,
    ) -> Result<ParkedRun, String> {
        let mut values = store.clone();
        let state_refs = match state_before {
            Some(state) => Some(
                state
                    .save(&mut values)
                    .map_err(|e| format!("the run's state couldn't be stored: {e:?}"))?
                    .iter()
                    .map(|(k, h)| (k.clone(), to_hex(h)))
                    .collect(),
            ),
            None => None,
        };
        Ok(ParkedRun {
            run_id: run_id.to_string(),
            unit: to_hex(unit),
            kind: kind.to_string(),
            route,
            inputs: inputs.iter().map(|i| i.as_ref().map(SValue::from_core)).collect(),
            sourced: sourced
                .iter()
                .filter(|(c, _)| matches!(c, Capability::Clock | Capability::Random | Capability::NetworkIngress))
                .map(|(c, v)| (format!("{c:?}"), SValue::from_core(v)))
                .collect(),
            state_refs,
            nonce: nonce.to_string(),
            rounds: store_rounds(rounds),
            tokens: BTreeMap::new(),
            events: waits.events.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect(),
            waiting: waiting.iter().map(|w| StoredWaiting { name: w.name.clone(), expires_at: now + w.expires }).collect(),
            parked_at: now,
            version: String::new(),
        })
    }

    /// The waiting point `token` resumes, if it is one of this run's and it still waits there.
    pub(crate) fn waiting_point_of(&self, token: &str) -> Option<&str> {
        self.waiting.iter().map(|w| w.name.as_str()).find(|name| {
            let stored = self.tokens.get(*name).is_some_and(|t| same_token(t, token));
            stored || same_token(&token_for(&self.run_id, name), token)
        })
    }

    /// Everything to run it again: its inputs, sourced values, waits (with `event` delivered at
    /// `name`) and what it started from.
    pub(crate) fn resume(
        &self,
        name: &str,
        event: Value,
        store: &SharedStore,
    ) -> Result<Resumption, String> {
        let unit = parse_hash(&self.unit).ok_or("the run's unit isn't a hash")?;
        let inputs = self.inputs.iter().map(|i| i.clone().map(SValue::into_core)).collect();
        let sourced = self
            .sourced
            .iter()
            .filter_map(|(c, v)| Some((capability_named(c)?, v.clone().into_core())))
            .collect();
        let mut events: BTreeMap<String, Value> = self.events.iter().map(|(k, v)| (k.clone(), v.clone().into_core())).collect();
        events.insert(name.to_string(), event);
        let waits = RunWaits { run_id: Some(self.run_id.clone()), tokens: self.tokens.clone(), events, ..Default::default() };
        let refs = self
            .state_refs
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|(k, h)| Some((k, parse_hash(&h)?)))
            .collect::<Option<BTreeMap<_, _>>>()
            .ok_or("a state reference isn't a hash")?;
        let state_before = State::load(&refs, store).map_err(|e| format!("the run's state can't be loaded: {e:?}"))?;
        let nonce = (0..16)
            .map(|i| u8::from_str_radix(self.nonce.get(2 * i..2 * i + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()
            .and_then(|v| <[u8; 16]>::try_from(v).ok())
            .ok_or("the run's nonce isn't 128 bits of hex")?;
        let prior = self
            .rounds
            .iter()
            .map(|r| Round {
                exchanges: r
                    .iter()
                    .map(|x| Exchange {
                        path: x.path.clone(),
                        request: x.request.clone().into_core(),
                        response: x.response.clone().into_core(),
                        start_us: x.start_us,
                        end_us: x.end_us,
                    })
                    .collect(),
            })
            .collect();
        Ok(Resumption { unit, inputs, sourced, waits, resumed: Resumed { state_before, nonce, prior } })
    }

    /// What `GET .../runs` shows: never the tokens.
    pub(crate) fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "run_id": self.run_id,
            "kind": self.kind,
            "route": self.route,
            "parked_at": self.parked_at,
            "waiting": self.waiting,
            "delivered": self.events.keys().collect::<Vec<_>>(),
        })
    }
}

/// After a fresh run: keep it as a parked run if it settled waiting (and didn't fail). The names
/// it waits on, for its log entry; an error when the deployment already has as many runs waiting
/// as it may (`max_waiting_runs`). Each deadline is held to its `max_await_seconds`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn park_if_waiting(
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    store: &SharedStore,
    slug: &str,
    unit: &Hash,
    kind: &str,
    route: Option<String>,
    inputs: &[Option<Value>],
    sourced: &[(Capability, Value)],
    run: &FiredRun,
    waits: &RunWaits,
) -> Result<Vec<String>, String> {
    if run.waiting.is_empty() || run.error.is_some() {
        return Ok(Vec::new());
    }
    let mut record = ParkedRun::new(
        &run.run_id,
        unit,
        kind,
        route,
        inputs,
        sourced,
        run.start_state.as_ref(),
        &run.nonce,
        &run.rounds,
        waits,
        &run.waiting,
        epoch_now(),
        store,
    )?;
    record.version = run.unit.clone();
    let mut table = deployments.lock().unwrap();
    let dep = table.get_mut(slug).ok_or("the deployment is gone")?;
    let limits = &dep.permit.durable;
    if dep.parked.len() as u64 >= limits.max_waiting_runs {
        return Err(format!("{slug:?} already has {} runs waiting, as many as it may (max_waiting_runs)", dep.parked.len()));
    }
    for w in &mut record.waiting {
        w.expires_at = w.expires_at.min(record.parked_at + limits.max_await_seconds);
    }
    let names = record.waiting.iter().map(|w| w.name.clone()).collect();
    dep.parked.insert(record.run_id.clone(), record);
    Ok(names)
}

/// The run waiting at `name` for `token` in `slug`, if there is one: what a resume route checks
/// before it answers.
pub(crate) fn find_by_token(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, name: &str, token: &str) -> Option<String> {
    let table = deployments.lock().unwrap();
    table.get(slug)?.parked.values().find(|p| p.waiting_point_of(token) == Some(name)).map(|p| p.run_id.clone())
}

/// Deliver `event` to parked run `run_id`'s waiting point `name`, and run it again from its
/// record. It finishes (its effects performed now, once), waits again, or fails; the log gets an
/// entry for this part, under the same run id.
pub(crate) fn resume_parked(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: &str,
    run_id: &str,
    name: &str,
    event: Value,
) -> Result<(), String> {
    let (record, state, permit, secrets, folders) = {
        let table = deployments.lock().unwrap();
        let dep = table.get(slug).ok_or("no such deployment")?;
        let record = dep.parked.get(run_id).ok_or("no such waiting run")?.clone();
        (record, dep.state.clone(), Arc::clone(&dep.permit), Arc::clone(&dep.secrets), Arc::clone(&dep.folders))
    };
    let Resumption { unit, inputs, sourced, mut waits, resumed } = record.resume(name, event, store)?;
    waits.folders = Some(folders);
    let resolver = materialize_closure(store, unit).map_err(|e| format!("store error: {e:?}"))?;
    let root = resolver.resolve(&unit).ok_or("the run's unit no longer resolves")?;
    let started = started_now();
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root, &inputs, &resolver, store, state.as_ref(), &sourced, &permit, false, &secrets, &mut waits, Some(resumed));

    let mut entry = run_entry("resume", started, &sourced, &run);
    if !record.version.is_empty() {
        entry.unit = record.version.clone();
    }
    entry.route = record.route.clone();
    entry.request = inputs.first().cloned().flatten().map(|r| SValue::from_core(&r));
    entry.events = waits.events.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect();
    entry.parked = run.waiting.iter().map(|w| w.name.clone()).collect();
    Redactor::tokens(&waits.tokens).entry(&mut entry);
    log_run(deployments, slug, entry);

    {
        let mut table = deployments.lock().unwrap();
        if let Some(dep) = table.get_mut(slug) {
            // One turned away by a run lock (firing.rs) didn't run: it waits on, for its event
            // to be delivered again.
            let turned_away = run.error.as_deref().is_some_and(|e| e.starts_with("busy:"));
            if turned_away {
                eprintln!("holon: run {run_id} in {slug:?} waits on: its event can be delivered again");
            } else if run.waiting.is_empty() || run.error.is_some() {
                dep.parked.remove(run_id);
            } else if let Some(kept) = dep.parked.get_mut(run_id) {
                // Still waiting: what it learned is kept; a point it was already waiting at keeps
                // its deadline.
                let now = epoch_now();
                kept.rounds.extend(store_rounds(&run.rounds));
                kept.events = waits.events.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect();
                kept.waiting = run
                    .waiting
                    .iter()
                    .map(|w| StoredWaiting {
                        name: w.name.clone(),
                        expires_at: kept
                            .waiting
                            .iter()
                            .find(|o| o.name == w.name)
                            .map_or(now + w.expires.min(dep.permit.durable.max_await_seconds), |o| o.expires_at),
                    })
                    .collect();
            }
        }
    }
    persist_deployments(deployments_path, deployments, store);
    match run.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Deliver `{expired: true}` to every waiting point past its deadline.
pub(crate) fn expire_waiting(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &str, now: u64) {
    let mut due: Vec<(String, String, String)> = Vec::new();
    for (slug, dep) in deployments.lock().unwrap().iter() {
        for p in dep.parked.values() {
            for w in p.waiting.iter().filter(|w| w.expires_at <= now) {
                due.push((slug.clone(), p.run_id.clone(), w.name.clone()));
            }
        }
    }
    for (slug, run_id, name) in due {
        if let Err(e) = resume_parked(store, deployments, deployments_path, &slug, &run_id, &name, expired_event()) {
            eprintln!("holon: expiring {name:?} of run {run_id} in {slug:?}: {e}");
        }
    }
}

/// `GET /deployments/<slug>/runs` — the runs waiting in it, without their tokens.
pub(crate) fn handle_get_runs(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> Response {
    match deployments.lock().unwrap().get(slug) {
        Some(dep) => Response::json(&serde_json::json!({ "runs": dep.parked.values().map(ParkedRun::summary).collect::<Vec<_>>() })),
        None => err(404, format!("no deployment {slug:?}")),
    }
}

/// The event an expired waiting point is given.
pub(crate) fn expired_event() -> Value {
    Value::Record(BTreeMap::from([("expired".to_string(), Value::Boolean(true))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runs_tokens_are_derived_stable_and_its_own() {
        let waits = |id: &str| RunWaits { run_id: Some(id.into()), ..Default::default() };
        let mut w = waits("aa");
        let a = w.token("paid");
        assert_eq!(w.token("paid"), a, "the same waiting point, the same token");
        assert_eq!(waits("aa").token("paid"), a, "asked for again (a resumed run): the same token");
        assert_ne!(w.token("approved"), a);
        assert!(a.starts_with("rt_") && a.len() == 3 + 32, "{a}");
        assert_ne!(waits("ab").token("paid"), a, "another run, another token");
        assert!(same_token(&a, &token_for("aa", "paid")) && !same_token(&a, &token_for("aa", "paid2")));
    }
}
