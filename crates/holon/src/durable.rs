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
//! - **Decide.** An `approval` is a waiting point people decide, never a token: a person signed in
//!   approves or declines it (`POST /deployments/<slug>/runs/<run_id>/decide`), each approval kept
//!   on the waiting point, and once as many different people as it asks have approved, or one
//!   declines, the decision is delivered as its event, with who decided ([`decide`]). No token
//!   resumes it, and a start hands none out for it.
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

/// Every `await` and `approval` in `unit`'s closure, as its name and how long it waits (seconds): what a deploy
/// holds to the deployment's `max_await_seconds`. One whose params don't parse never waits, and
/// isn't listed.
pub(crate) fn awaits_in(unit: &Unit, resolver: &impl Resolver) -> Vec<(String, u64)> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![unit];
    while let Some(u) = stack.pop() {
        if let UnitBody::Primitive { name } = &u.body
            && (name == "await" || name == "approval")
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
                let hex: String = holon_core::hex(key);
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
    format!("rt_{}", holon_core::hex(&mac.as_bytes()[..16]))
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
    /// The route's caller, when the run's reply forwards a streamed call (outbound.rs).
    pub(crate) forward: Option<outbound::Forwarder>,
    /// The deployment's channels, which a piped call sends its answer to (channels.rs); none
    /// outside a run.
    pub(crate) channels: Option<Arc<channels::Channels>>,
    /// The deployment the run belongs to, which a call to another flow says it is from (calls.rs);
    /// none outside a run.
    pub(crate) slug: Option<String>,
    /// The flows whose calls led to this run, the first being the one that started the chain: empty
    /// unless a flow's call started it (calls.rs).
    pub(crate) chain: Vec<String>,
    /// Its run won't be logged (an unrecorded handler's, unless it fails): no timeline is kept
    /// (docs/L4s/request-path-L4.md §4.5 a).
    pub(crate) unrecorded: bool,
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
    /// How many different people must approve it: 0 (not written) for an event.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) people: u64,
    /// The approvals it has had so far, in the order they came.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) approved: Vec<Approved>,
}

/// One person's approval of a waiting point.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Approved {
    /// Their account's name.
    pub(crate) by: String,
    /// When, seconds since the epoch.
    pub(crate) at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl StoredWaiting {
    fn new(w: &Waiting, expires_at: u64) -> StoredWaiting {
        StoredWaiting { name: w.name.clone(), expires_at, people: w.people, approved: Vec::new() }
    }

    /// What an approval delivers: `{approved, by, approvals}`, who approved and when, with
    /// `declined_by` and `reason` when `declined` says who, or `expired: true`.
    fn decision(&self, approved: bool, declined: Option<(&str, &str)>, expired: bool) -> Value {
        let text = |s: &str| Value::Text(s.to_string());
        let approvals = self
            .approved
            .iter()
            .map(|a| {
                let mut r = BTreeMap::from([("by".to_string(), text(&a.by)), ("at".to_string(), Value::Number(a.at as f64))]);
                r.extend(a.reason.as_deref().map(|why| ("reason".to_string(), text(why))));
                Value::Record(r)
            })
            .collect();
        let mut d = BTreeMap::from([
            ("approved".to_string(), Value::Boolean(approved)),
            ("by".to_string(), Value::List(self.approved.iter().map(|a| text(&a.by)).collect())),
            ("approvals".to_string(), Value::List(approvals)),
        ]);
        if let Some((by, reason)) = declined {
            d.insert("declined_by".into(), text(by));
            d.insert("reason".into(), text(reason));
        }
        if expired {
            d.insert("expired".into(), Value::Boolean(true));
        }
        Value::Record(d)
    }
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
    /// `"request"`, `"trigger"`, `"tick"` or `"file"`, and its handler's label (the route, for a
    /// request).
    pub(crate) kind: String,
    pub(crate) route: Option<String>,
    /// Its handler's level, as declared (`lazy`, `unrecorded`; empty for strict): each later part
    /// is recorded as the first was. What it waits with is kept here whatever the level.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) log: String,
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
    /// The packs its parts sent as they parked (channel, JSON), as its channels hold them
    /// (`Channels::waiting_sent`): written with it, and given back to them when the server starts,
    /// so a later part doesn't send them again after a restart, an unrecorded run's too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) sent: Vec<(String, String)>,
    /// Its inputs, sourced values, requests, events and tokens, sealed, as the deployments file
    /// keeps them (they may hold a secret, a caller's credential, a token); opened again when
    /// the server starts (`open`). Never set on a run in memory that could be opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sealed: Option<SealedPart>,
    /// Whether a resume holds it right now: claimed under the deployments lock when its token
    /// is checked (or its deadline passes), so a token resumes it once even when requests carrying
    /// it arrive together. Never persisted: a restart frees it.
    #[serde(skip)]
    pub(crate) resuming: bool,
}

/// A parked run's sealed part: AES-256-GCM under the secrets master key, bound to its deployment
/// and run id (hex).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct SealedPart {
    pub(crate) nonce: String,
    pub(crate) data: String,
}

/// What of a parked run is sealed on disk.
#[derive(Serialize, Deserialize, Default)]
struct Sealable {
    inputs: Vec<Option<SValue>>,
    sourced: Vec<(String, SValue)>,
    rounds: Vec<Vec<StoredExchange>>,
    #[serde(default)]
    tokens: BTreeMap<String, String>,
    events: BTreeMap<String, SValue>,
}

fn parked_context(slug: &str, run_id: &str) -> String {
    format!("holon-parked/v1\0{slug}\0{run_id}")
}

/// The capability a logged sourced value is for: what a run's trigger and the run's start supply.
pub(crate) fn capability_named(name: &str) -> Option<Capability> {
    match name {
        "Clock" => Some(Capability::Clock),
        "Random" => Some(Capability::Random),
        "NetworkIngress" => Some(Capability::NetworkIngress),
        // The new file that started it (arrivals.rs), the mail (inbox.rs), the feed's event
        // (subscriptions.rs), or the queue's message (dequeue.rs).
        _ => {
            let named = |prefix: &str| name.strip_prefix(prefix).and_then(|r| r.strip_suffix(')')).and_then(|q| serde_json::from_str::<String>(q).ok());
            named("FileArrival(").map(Capability::FileArrival).or_else(|| named("MailArrival(").map(Capability::MailArrival)).or_else(|| named("FeedEvent(").map(Capability::FeedEvent)).or_else(|| named("QueueMessage(").map(Capability::QueueMessage))
        }
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
            log: String::new(),
            inputs: inputs.iter().map(|i| i.as_ref().map(SValue::from_core)).collect(),
            sourced: sourced
                .iter()
                .filter(|(c, _)| matches!(c, Capability::Clock | Capability::Random | Capability::NetworkIngress | Capability::FileArrival(_) | Capability::MailArrival(_) | Capability::FeedEvent(_) | Capability::QueueMessage(_)))
                .map(|(c, v)| (format!("{c:?}"), SValue::from_core(v)))
                .collect(),
            state_refs,
            nonce: nonce.to_string(),
            rounds: store_rounds(rounds),
            tokens: BTreeMap::new(),
            events: waits.events.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect(),
            waiting: waiting.iter().map(|w| StoredWaiting::new(w, now + w.expires)).collect(),
            parked_at: now,
            version: String::new(),
            sent: Vec::new(),
            sealed: None,
            resuming: false,
        })
    }

    /// The waiting point `token` resumes, if it is one of this run's and it still waits there: an
    /// event's, never an approval's, which people decide.
    pub(crate) fn waiting_point_of(&self, token: &str) -> Option<&str> {
        self.waiting.iter().filter(|w| w.people == 0).map(|w| w.name.as_str()).find(|name| {
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
        // Lent to its reads as they are (`run_rounds`): read back from storage, each must be data.
        let sourced = self
            .sourced
            .iter()
            .filter_map(|(c, v)| Some((capability_named(c)?, v.clone().into_core())))
            .filter(|(_, v)| is_storable(v))
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

    /// This run as the deployments file keeps it: what may hold a secret sealed by `sealer`.
    pub(crate) fn sealed_for_disk(&self, slug: &str, sealer: &crate::sealing::Sealer) -> ParkedRun {
        if self.sealed.is_some() {
            return self.clone();
        }
        let part = Sealable {
            inputs: self.inputs.clone(),
            sourced: self.sourced.clone(),
            rounds: self.rounds.clone(),
            tokens: self.tokens.clone(),
            events: self.events.clone(),
        };
        let plain = zeroize::Zeroizing::new(serde_json::to_string(&part).unwrap_or_default());
        let (nonce, data) = sealer.seal(&parked_context(slug, &self.run_id), &plain);
        ParkedRun {
            inputs: Vec::new(),
            sourced: Vec::new(),
            rounds: Vec::new(),
            tokens: BTreeMap::new(),
            events: BTreeMap::new(),
            sealed: Some(SealedPart { nonce, data }),
            ..self.clone()
        }
    }

    /// Open what the deployments file sealed, with `sealer`; nothing to do for a run not sealed.
    pub(crate) fn open(&mut self, slug: &str, sealer: Option<&crate::sealing::Sealer>) -> Result<(), String> {
        let Some(sealed) = &self.sealed else { return Ok(()) };
        let sealer = sealer.ok_or("it was sealed under a key this server doesn't have")?;
        let plain = sealer.open(&parked_context(slug, &self.run_id), &sealed.nonce, &sealed.data)?;
        let part: Sealable = crate::wire::read_json(plain.as_bytes()).map_err(|e| format!("its sealed part doesn't read: {e}"))?;
        self.inputs = part.inputs;
        self.sourced = part.sourced;
        self.rounds = part.rounds;
        self.tokens = part.tokens;
        self.events = part.events;
        self.sealed = None;
        Ok(())
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

/// After a fresh run: keep it as a parked run if it settled waiting (and didn't fail), whatever
/// its handler's level `log`: a parked run is its own record. The names it waits on, for its log
/// entry; an error when the deployment already has as many runs waiting as it may
/// (`max_waiting_runs`). Each deadline is held to its `max_await_seconds`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn park_if_waiting(
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    store: &SharedStore,
    slug: &str,
    unit: &Hash,
    kind: &str,
    route: Option<String>,
    log: holon_core::LogLevel,
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
    if log != holon_core::LogLevel::Strict {
        record.log = log.word().to_string();
    }
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

/// A parked run held for one resume: no other resume gets it while this is alive, and dropping
/// it (the resume done, failed, or panicked) frees the run if it still waits.
pub(crate) struct Claimed<'a> {
    deployments: &'a Mutex<BTreeMap<String, Deployment>>,
    slug: String,
    run_id: String,
}

impl Claimed<'_> {
    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }
}

impl Drop for Claimed<'_> {
    fn drop(&mut self) {
        let mut table = self.deployments.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(run) = table.get_mut(&self.slug).and_then(|d| d.parked.get_mut(&self.run_id)) {
            run.resuming = false;
        }
    }
}

/// The run waiting at `name` for `token` in `slug`, claimed for this resume, if there is one and
/// no other resume holds it: what a resume route checks before it answers.
pub(crate) fn claim_by_token<'a>(deployments: &'a Mutex<BTreeMap<String, Deployment>>, slug: &str, name: &str, token: &str) -> Option<Claimed<'a>> {
    let mut table = deployments.lock().unwrap();
    let run = table.get_mut(slug)?.parked.values_mut().find(|p| !p.resuming && p.waiting_point_of(token) == Some(name))?;
    run.resuming = true;
    Some(Claimed { deployments, slug: slug.to_string(), run_id: run.run_id.clone() })
}

/// Parked run `run_id`, claimed while it still waits at `name`, unless another resume holds it.
fn claim<'a>(deployments: &'a Mutex<BTreeMap<String, Deployment>>, slug: &str, run_id: &str, name: &str) -> Option<Claimed<'a>> {
    let mut table = deployments.lock().unwrap();
    let run = table.get_mut(slug)?.parked.get_mut(run_id)?;
    if run.resuming || !run.waiting.iter().any(|w| w.name == name) {
        return None;
    }
    run.resuming = true;
    Some(Claimed { deployments, slug: slug.to_string(), run_id: run_id.to_string() })
}

/// Deliver `event` to the claimed parked run's waiting point `name`, and run it again from its
/// record. It finishes (its effects performed now, once), waits again, or fails; the log gets an
/// entry for this part, under the same run id. The claim is let go when this returns.
pub(crate) fn resume_parked(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &Place,
    slug: &str,
    claimed: Claimed<'_>,
    name: &str,
    event: Value,
) -> Result<(), String> {
    let run_id = claimed.run_id.clone();
    let run_id = run_id.as_str();
    let (record, state, permit, secrets, folders, channels) = {
        let table = deployments.lock().unwrap();
        let dep = table.get(slug).ok_or("no such deployment")?;
        let mut record = dep.parked.get(run_id).ok_or("no such waiting run")?.clone();
        record.open(slug, dep.secrets.lock().unwrap().sealer.as_deref())?;
        (record, dep.state.clone(), Arc::clone(&dep.permit), Arc::clone(&dep.secrets), Arc::clone(&dep.folders), Arc::clone(&dep.channels))
    };
    let Resumption { unit, inputs, sourced, mut waits, resumed } = record.resume(name, event, store)?;
    // Each part is recorded as its handler declares, its grant holding `unrecorded` as for any
    // run: what it waits with is the parked run's own, whatever the level.
    let declared = holon_core::LogLevel::parse(&record.log).unwrap_or_default();
    waits.unrecorded = permit.log_level(record.route.as_deref(), declared) == holon_core::LogLevel::Unrecorded;
    waits.folders = Some(folders);
    waits.channels = Some(channels);
    waits.slug = Some(slug.to_string());
    let resolver = closure(store, unit).map_err(|e| format!("store error: {e:?}"))?;
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
    // Its tokens, whether it handed them out itself or a start handed them back for it: each
    // derived from its id, for each point it waited at or waits at now. The event that woke it
    // may carry one.
    let mut tokens = waits.tokens.clone();
    for name in waits.events.keys().chain(run.waiting.iter().map(|w| &w.name)) {
        tokens.entry(name.clone()).or_insert_with(|| token_for(run_id, name));
    }
    let redactor = Redactor::tokens(&tokens).and(Redactor::handed(&calls::handed(&run.rounds)));
    if let Some(l) = log_run(deployments, slug, entry, redactor, declared) {
        l.said(slug);
    }

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
                // A point it was already waiting at keeps its deadline, and the approvals it had.
                kept.waiting = run
                    .waiting
                    .iter()
                    .map(|w| match kept.waiting.iter().find(|o| o.name == w.name) {
                        Some(o) => o.clone(),
                        None => StoredWaiting::new(w, now + w.expires.min(dep.permit.durable.max_await_seconds)),
                    })
                    .collect();
            }
        }
    }
    persist_deployments(deployments_path, deployments);
    match run.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Deliver `{expired: true}` to every waiting point past its deadline: to an approval, with who
/// approved it before it ran out, `approved` false.
pub(crate) fn expire_waiting(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &Place, now: u64) {
    let mut due: Vec<(String, String, String, Value)> = Vec::new();
    for (slug, dep) in deployments.lock().unwrap().iter() {
        for p in dep.parked.values() {
            for w in p.waiting.iter().filter(|w| w.expires_at <= now) {
                let event = if w.people == 0 { expired_event() } else { w.decision(false, None, true) };
                due.push((slug.clone(), p.run_id.clone(), w.name.clone(), event));
            }
        }
    }
    for (slug, run_id, name, event) in due {
        // A run a request is resuming right now isn't expired under it.
        let Some(claimed) = claim(deployments, &slug, &run_id, &name) else {
            continue;
        };
        if let Err(e) = resume_parked(store, deployments, deployments_path, &slug, claimed, &name, event) {
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

/// What a person's decision did: the approval still waits for more people, or it is decided, and
/// the run is held to be given the decision.
pub(crate) enum Decided<'a> {
    Waits { people: u64, approved_by: Vec<String> },
    Decides { claimed: Claimed<'a>, people: u64, approved_by: Vec<String>, event: Value },
}

/// `by`'s decision on the approval `name` of the run `run_id` in `slug`, made under the
/// deployments lock, so two people deciding at once decide it once. An approval counts each person
/// once; when as many as it asks have approved, or `by` declines (with a `reason`), the run is
/// claimed to be given the decision. The status and why when it can't be decided.
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide<'a>(
    deployments: &'a Mutex<BTreeMap<String, Deployment>>,
    slug: &str,
    run_id: &str,
    name: &str,
    by: &str,
    approve: bool,
    reason: Option<String>,
    now: u64,
) -> Result<Decided<'a>, (u16, String)> {
    let mut table = deployments.lock().unwrap();
    let dep = table.get_mut(slug).ok_or((404, format!("no deployment {slug:?}")))?;
    let run = dep.parked.get_mut(run_id).ok_or((404, format!("no run {run_id} of {slug:?} is waiting: it may have been decided, or finished")))?;
    if run.resuming {
        return Err((409, format!("run {run_id} of {slug:?} is going on right now: try again in a moment")));
    }
    let w = run.waiting.iter_mut().find(|w| w.name == name).ok_or((404, format!("run {run_id} of {slug:?} isn't waiting at {name:?}")))?;
    if w.people == 0 {
        return Err((409, format!("{name:?} is an event, which its route delivers: nobody decides it")));
    }
    let people = w.people;
    if approve {
        if w.approved.iter().any(|a| a.by == by) {
            return Err((409, format!("{by} already approved {name:?}: it needs {people} different people")));
        }
        w.approved.push(Approved { by: by.to_string(), at: now, reason: reason.clone() });
    }
    let approved_by: Vec<String> = w.approved.iter().map(|a| a.by.clone()).collect();
    let event = if !approve {
        w.decision(false, Some((by, reason.as_deref().unwrap_or_default())), false)
    } else if (w.approved.len() as u64) < people {
        return Ok(Decided::Waits { people, approved_by });
    } else {
        w.decision(true, None, false)
    };
    run.resuming = true;
    Ok(Decided::Decides { claimed: Claimed { deployments, slug: slug.to_string(), run_id: run_id.to_string() }, people, approved_by, event })
}

/// What `POST /deployments/<slug>/runs/<run_id>/decide` takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideBody {
    name: String,
    approve: bool,
    #[serde(default)]
    reason: Option<String>,
}

/// The longest reason a decision may give, in characters.
const MAX_REASON_CHARS: usize = 1000;

/// Who `caller` decides as: a person signed in, by their name (the OS user's local admin by the
/// name it runs as); or why they can't decide. Not an agent, a service (a script), a viewer, or
/// anyone on a server without accounts.
pub(crate) fn decider(caller: &access::Caller) -> Result<String, String> {
    if caller.role == Some(access::Role::Viewer) {
        return Err("a viewer only reads: an editor or an admin decides an approval".into());
    }
    let Some(name) = caller.name.as_deref() else {
        return Err("deciding an approval needs a person signed in to Holon, and this server has no accounts".into());
    };
    match caller.kind {
        Some(accounts::Kind::Agent) => Err(format!("{name} is an agent: deciding an approval is a person's")),
        Some(accounts::Kind::Service) => Err(format!("{name} is a service, a script, not a person: sign in to decide an approval")),
        // A person sending `X-Holon-Role: agent` (access.rs).
        _ if caller.role == Some(access::Role::Agent) => Err("an agent may not decide an approval: deciding is a person's".into()),
        _ => Ok(name.strip_prefix("local:").unwrap_or(name).to_string()),
    }
}

/// `POST /deployments/<slug>/runs/<run_id>/decide` — a person approves, or declines with a reason,
/// the approval `name` the run waits at. Answered `{name, people, approved_by, outcome}`,
/// `outcome` `waiting` while it waits for more people, else `approved` or `declined`, the run
/// having been given the decision (`run_error` when its part then failed).
pub(crate) fn handle_post_decide(state: &AppState, slug: &str, run_id: &str, caller: &access::Caller, request: &Request) -> Response {
    let by = match decider(caller) {
        Ok(by) => by,
        Err(e) => return err(403, e),
    };
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body: DecideBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("a decision is {{\"name\": \"payout\", \"approve\": true, \"reason\": \"…\"}}: {e}")),
    };
    let reason = body.reason.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    if reason.as_ref().is_some_and(|r| r.chars().count() > MAX_REASON_CHARS) {
        return err(400, format!("a reason is at most {MAX_REASON_CHARS} characters"));
    }
    if !body.approve && reason.is_none() {
        return err(400, "a decline says why: give a reason");
    }
    let (deployments, path) = (&state.deployments, &state.deployments_path);
    // In the activity log as what it was, by whom.
    let recorded = |state: &AppState| state.activity.record(if body.approve { "approved" } else { "declined" }, &format!("{slug} run {run_id} at {:?}", body.name), Some(&by));
    let reply = |people: u64, approved_by: &[String], outcome: &str| serde_json::json!({ "name": body.name, "people": people, "approved_by": approved_by, "outcome": outcome });
    match decide(deployments, slug, run_id, &body.name, &by, body.approve, reason, epoch_now()) {
        Err((code, why)) => err(code, why),
        Ok(Decided::Waits { people, approved_by }) => {
            recorded(state);
            persist_deployments(path, deployments);
            Response::json(&reply(people, &approved_by, "waiting"))
        }
        Ok(Decided::Decides { claimed, people, approved_by, event }) => {
            recorded(state);
            let mut answer = reply(people, &approved_by, if body.approve { "approved" } else { "declined" });
            if let Err(e) = resume_parked(&state.store, deployments, path, slug, claimed, &body.name, event) {
                answer["run_error"] = e.into();
            }
            Response::json(&answer)
        }
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
