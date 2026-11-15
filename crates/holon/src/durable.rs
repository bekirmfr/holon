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
//!   approves or declines it (`POST /deployments/<slug>/runs/<run_id>/decide`), each decision kept
//!   on the waiting point, and once as many different people as it asks have approved, or one
//!   declines, the decision is delivered as its event, with every decision, its step and who took
//!   it ([`decide`]). No token resumes it, and a start hands none out for it.
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

use holon_core::approval::{Slot, SlotKind, Steps};
use holon_host::{Exchange, Waits, Waiting};

use crate::accounts::{Kind, Parties};
use crate::bindings::{ApprovalSettings, Bound};
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

/// Read the host key at `path`, or make one and write it there, sealed under the keyring's
/// `server` scope (owner-only); `":memory:"` keeps a fresh one in memory. Called once, at
/// startup. A file that exists but doesn't open as a host key is an error: replacing it would
/// invalidate every token handed out.
pub(crate) fn load_host_key(path: &str, keys: &crate::at_rest::Keyring) -> Result<(), String> {
    const ROLE: &str = "host key";
    let key = if path == ":memory:" {
        random_key()
    } else {
        let file = std::path::Path::new(path);
        match crate::at_rest::read(keys, file, ROLE)? {
            Some(bytes) => <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| format!("{path:?} is not a host key (32 bytes)"))?,
            None => {
                let key = random_key();
                crate::at_rest::write_private(keys, file, crate::at_rest::SERVER, ROLE, &key).map_err(|e| format!("could not write {path:?}: {e}"))?;
                key
            }
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
    /// Who started the run, signed in: a control-plane call's caller, or an action's; for a run a
    /// flow's call started, whoever started the run that called (calls.rs). None for a request on
    /// the flow's own port, where nobody signs in, and for every other trigger.
    pub(crate) started_by: Option<String>,
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
    /// An approval's steps, the canonical text (`person*2`, `agent:classifier>person:reviewer`):
    /// empty (not written) for an event.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) steps: String,
    /// The decisions taken on it so far, in the order they came.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) decisions: Vec<Decided>,
    /// What it shows those who decide it, an approval's (`shown_of`): none for an event, or for a
    /// run with nothing to show. Sealed on disk with the run's inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) shown: Option<Shown>,
}

/// What an approval shows those who decide it, as the run's log would keep it: what its flow put
/// in its `shows` (an amount, a payee), or else the run's input.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Shown {
    Shows(serde_json::Value),
    Input(serde_json::Value),
}

/// The most an approval shows, as JSON text: past it, each value is cut down (`cut_down`), and
/// past it still, only how large it is.
const SHOWN_MAX: usize = 16 * 1024;

/// Cut down, a text past this many bytes shows its head and how many more; a list or a record past
/// this many items, its first, and how many more.
const SHOWN_TEXT: usize = 1024;
const SHOWN_ITEMS: usize = 64;

/// What an approval shows of `v`, already redacted (R42): itself within `SHOWN_MAX`; else cut
/// down, so a field padded past the bound hides none beside it; else how large it was. Cut only
/// once redacted: a secret a cut runs through would leave its head in clear.
fn bounded(v: serde_json::Value) -> serde_json::Value {
    let size = v.to_string().len();
    if size <= SHOWN_MAX {
        return v;
    }
    let cut = cut_down(v);
    if cut.to_string().len() <= SHOWN_MAX {
        return cut;
    }
    format!("‹{size} bytes, more than an approval shows ({} KiB)›", SHOWN_MAX / 1024).into()
}

/// `v` with each text past `SHOWN_TEXT` bytes cut to its head, and each list and record past
/// `SHOWN_ITEMS` items to its first, each cut saying how much more there was.
fn cut_down(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value as J;
    let text = |s: String| {
        if s.len() <= SHOWN_TEXT {
            return s;
        }
        let at = (0..=SHOWN_TEXT).rev().find(|i| s.is_char_boundary(*i)).unwrap_or(0);
        format!("{}‹… {} more bytes›", &s[..at], s.len() - at)
    };
    match v {
        J::String(s) => J::String(text(s)),
        J::Array(items) => {
            let more = items.len().saturating_sub(SHOWN_ITEMS);
            let mut kept: Vec<J> = items.into_iter().take(SHOWN_ITEMS).map(cut_down).collect();
            if more > 0 {
                kept.push(J::String(format!("‹{more} more items›")));
            }
            J::Array(kept)
        }
        J::Object(fields) => {
            let more = fields.len().saturating_sub(SHOWN_ITEMS);
            let mut kept: serde_json::Map<String, J> = fields.into_iter().take(SHOWN_ITEMS).map(|(k, v)| (text(k), cut_down(v))).collect();
            if more > 0 {
                kept.insert("‹more›".into(), J::String(format!("‹{more} more fields›")));
            }
            J::Object(kept)
        }
        other => other,
    }
}

impl Shown {
    fn value(&self) -> &serde_json::Value {
        match self {
            Shown::Shows(v) | Shown::Input(v) => v,
        }
    }

    fn map(self, f: impl FnOnce(serde_json::Value) -> serde_json::Value) -> Shown {
        match self {
            Shown::Shows(v) => Shown::Shows(f(v)),
            Shown::Input(v) => Shown::Input(f(v)),
        }
    }
}

/// What each of `run`'s waiting points shows those who decide it, in order: an approval what its
/// `shows` put there, withheld whole when a secret may have made it, or else the run's input
/// (`input_of`); each redacted as the run's log entry is (the run's `tokens`, the credentials its
/// calls were handed, and `redaction`: the deployment's secrets and the keys its flow taints), and
/// past `SHOWN_MAX`, cut down (`bounded`). None for an event, and for a run with no input that
/// names nothing to show.
pub(crate) fn shown_of(
    kind: &str,
    inputs: &[Option<Value>],
    sourced: &[(Capability, Value)],
    run: &FiredRun,
    tokens: &BTreeMap<String, String>,
    redaction: Option<(Arc<Mutex<secrets::Resolved>>, Vec<String>)>,
) -> Vec<Option<Shown>> {
    if run.waiting.iter().all(|w| w.approval.is_none()) {
        return vec![None; run.waiting.len()];
    }
    // An entry as the run's would be, only for its redactor: never logged, so its kind is no kind
    // the host logs a run as.
    const UNLOGGED: &str = "shown";
    let mut entry = run_entry(UNLOGGED, started_now(), sourced, run);
    entry.request = inputs.first().cloned().flatten().map(|r| SValue::from_core(&r));
    // What `shows` holds is looked through for the keys the flow taints too, as a log line is.
    let lines: Vec<(String, Value)> = run.waiting.iter().filter_map(|w| Some((w.name.clone(), w.shows.clone()?))).collect();
    let marked = Redactor::tokens(tokens).and(Redactor::handed(&calls::handed(&run.rounds)));
    let redactor = match &redaction {
        Some((secrets, taint)) => redactor_of(&mut entry, &lines, marked, secrets, taint),
        None => marked,
    };
    let json = |v: &Value| -> serde_json::Value {
        let Some(text) = holon_core::stringify_json(v) else { return "‹it has no JSON form›".into() };
        serde_json::from_str(&text).unwrap_or_default()
    };
    run.waiting
        .iter()
        .map(|w| {
            w.approval.as_ref()?;
            let shown = match &w.shows {
                Some(_) if run.secret_fed.input(&w.path, 0) == crate::taint::Taint::Derived => Shown::Shows(crate::taint::WITHHELD.into()),
                Some(v) => Shown::Shows(json(v)),
                None => Shown::Input(json(&input_of(kind, inputs, sourced)?)),
            };
            // Redacted whole, then cut down: never the other way round.
            Some(shown.map(|mut v| {
                redactor.json(&mut v);
                bounded(v)
            }))
        })
        .collect()
}

/// A run's input, as those who decide its approvals read it: an action's argument, a request's
/// record without its headers, or the message, mail, file or feed event that started it; none for
/// a run a clock started.
fn input_of(kind: &str, inputs: &[Option<Value>], sourced: &[(Capability, Value)]) -> Option<Value> {
    if let Some(Some(input)) = inputs.first() {
        return Some(match (kind, input) {
            ("request", Value::Record(r)) => Value::Record(r.iter().filter(|(k, _)| k.as_str() != "headers").map(|(k, v)| (k.clone(), v.clone())).collect()),
            _ => input.clone(),
        });
    }
    sourced
        .iter()
        .find(|(c, _)| matches!(c, Capability::FileArrival(_) | Capability::MailArrival(_) | Capability::FeedEvent(_) | Capability::QueueMessage(_)))
        .map(|(_, v)| v.clone())
}

/// One decision on an approval: which step it took, who took it, and what they decided. The run
/// is given each as it is kept.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Decided {
    /// The step's number, from 0 as the steps are written: 0 for the short form.
    pub(crate) slot: u8,
    /// Their account's name.
    pub(crate) who: String,
    pub(crate) kind: Kind,
    /// The step's role: none for the short form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    /// When, seconds since the epoch.
    pub(crate) at: u64,
    pub(crate) approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    /// What they noted with it, any JSON (an agent's label and confidence): given to the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<serde_json::Value>,
    /// What let them (the plan's D43): the id of the binding they took it through, and the
    /// access document's version that binding was read in, which `holon access show` prints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) binding: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) access: Option<u64>,
    /// How it came, when not through their own sign-in: this machine's run file, as its OS user
    /// (`local:<user>`), which signs in as the account it is tied to (D45). The record says so,
    /// and what it is shown to and exported with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) via: Option<String>,
}

impl Decided {
    /// As the run is given it: `{slot, who, kind, role, at, approved, reason, note, binding,
    /// access, via}`, the role, the reason, the note, what let it and how it came when there are.
    fn value(&self) -> Value {
        let text = |s: &str| Value::Text(s.to_string());
        let mut r = BTreeMap::from([
            ("slot".to_string(), Value::Number(f64::from(self.slot))),
            ("who".to_string(), text(&self.who)),
            ("kind".to_string(), text(self.kind.name())),
            ("at".to_string(), Value::Number(self.at as f64)),
            ("approved".to_string(), Value::Boolean(self.approved)),
        ]);
        r.extend(self.role.as_deref().map(|role| ("role".to_string(), text(role))));
        r.extend(self.reason.as_deref().map(|why| ("reason".to_string(), text(why))));
        r.extend(self.note.as_ref().and_then(|n| holon_core::parse_json(&n.to_string())).map(|n| ("note".to_string(), n)));
        r.extend(self.binding.map(|id| ("binding".to_string(), Value::Number(id as f64))));
        r.extend(self.access.map(|v| ("access".to_string(), Value::Number(v as f64))));
        r.extend(self.via.as_deref().map(|via| ("via".to_string(), text(via))));
        Value::Record(r)
    }
}

/// One step of an approval as it stands: what it asks, who approved it so far, and whether it is
/// open, every stage before its own filled and itself not.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct SlotState {
    /// Its number, from 0 as the steps are written.
    pub(crate) slot: u8,
    /// Its stage's, from 0.
    pub(crate) stage: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    /// `person` or `agent`.
    pub(crate) kind: &'static str,
    /// How many different principals fill it.
    pub(crate) people: u8,
    /// Whether whoever started the run may take it (`starter: true`); listed only when they may.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) starter: bool,
    /// Who approved it, in the order they did.
    pub(crate) by: Vec<String>,
    pub(crate) open: bool,
}

impl SlotState {
    fn filled(&self) -> bool {
        self.by.len() >= usize::from(self.people)
    }

    /// As a refusal names it: `slot 1 (reviewer)`, or `slot 0` for a step without a role.
    fn named(&self) -> String {
        match &self.role {
            Some(role) => format!("slot {} ({role})", self.slot),
            None => format!("slot {}", self.slot),
        }
    }
}

impl StoredWaiting {
    fn new(w: &Waiting, expires_at: u64) -> StoredWaiting {
        StoredWaiting { name: w.name.clone(), expires_at, steps: w.approval.clone().unwrap_or_default(), decisions: Vec::new(), shown: None }
    }

    /// Whether it is an event's, which its route delivers, not an approval's.
    fn is_event(&self) -> bool {
        self.steps.is_empty()
    }

    /// Its steps as they stand, in the order written: none for an event, or for steps this host
    /// can't read (core fails closed), which nobody decides, and which run out.
    pub(crate) fn slots(&self) -> Vec<SlotState> {
        let Some(steps) = Steps::parse(&self.steps) else { return Vec::new() };
        let mut slots: Vec<SlotState> = steps
            .slots()
            .map(|(n, stage, s)| SlotState {
                slot: n as u8,
                stage: stage as u8,
                role: s.role.clone(),
                kind: s.kind.word(),
                people: s.people,
                starter: s.starter,
                by: self.approvals().filter(|d| usize::from(d.slot) == n).map(|d| d.who.clone()).collect(),
                open: false,
            })
            .collect();
        // Open: not filled, every earlier stage filled, and nobody declined.
        let declined = self.declined().is_some();
        let stage_filled = |stage: u8, slots: &[SlotState]| slots.iter().filter(|s| s.stage == stage).all(SlotState::filled);
        let open: Vec<bool> = slots.iter().map(|s| !declined && !s.filled() && (0..s.stage).all(|st| stage_filled(st, &slots))).collect();
        for (s, open) in slots.iter_mut().zip(open) {
            s.open = open;
        }
        slots
    }

    /// Its approvals so far, in the order they came.
    fn approvals(&self) -> impl Iterator<Item = &Decided> {
        self.decisions.iter().filter(|d| d.approved)
    }

    /// The decline it had, if it had one.
    fn declined(&self) -> Option<&Decided> {
        self.decisions.iter().find(|d| !d.approved)
    }

    /// Whether every step is filled, and nobody declined.
    fn approved(&self) -> bool {
        let slots = self.slots();
        self.declined().is_none() && !slots.is_empty() && slots.iter().all(SlotState::filled)
    }

    /// Whether it is decided: declined, or every step filled.
    fn decided(&self) -> bool {
        self.declined().is_some() || self.approved()
    }

    /// Takes `d`, a decision on its step `d.slot`: refused when the step isn't open, or when its
    /// principal already decided a step of it (one principal, one step, and once within it).
    /// Once it is decided (declined, or every step filled), the decision to deliver; none while
    /// it waits for more. One decided already (a run lock turned it away, firing.rs) stands, and
    /// is delivered again whoever decides next.
    fn take(&mut self, d: Decided) -> Result<Option<Value>, String> {
        if self.decided() {
            return Ok(Some(self.decision(false)));
        }
        let slots = self.slots();
        let Some(slot) = slots.get(usize::from(d.slot)) else {
            return Err(format!("{:?} has {} steps, from slot 0: there is no slot {}", self.name, slots.len(), d.slot));
        };
        // Theirs was an approval: a decline would have decided it.
        if let Some(before) = self.decisions.iter().find(|b| b.who == d.who) {
            let at = self.step_said(&slots, before.slot);
            return Err(if before.slot != d.slot {
                format!("{} already decided {at}: each step of an approval is someone else's", d.who)
            } else if !d.approved {
                format!("{} already approved {at}, and a decision stands: someone else declines it", d.who)
            } else if slot.people > 1 {
                format!("{} already approved {at}: it needs {} different people", d.who, slot.people)
            } else {
                format!("{} already approved {at}", d.who)
            });
        }
        if !slot.open {
            let at = self.step_said(&slots, d.slot);
            let waits_for: Vec<String> = slots.iter().filter(|s| s.stage < slot.stage && !s.filled()).map(SlotState::named).collect();
            return Err(match waits_for.as_slice() {
                [] => format!("{at} is filled: {} approved it", slot.by.join(" and ")),
                [one] => format!("{at} opens once {one} is filled"),
                more => format!("{at} opens once {} are filled", more.join(" and ")),
            });
        }
        self.decisions.push(d);
        Ok(self.decided().then(|| self.decision(false)))
    }

    /// The open steps `by` may fill now, by number: what the decide route would let them take
    /// without naming one. None when the run's `excluded` keeps them out, or they decided a step of
    /// it already, or it is decided; when they started the run, only a step that takes them.
    fn yours(&self, excluded: &Excluded, by: &Decider) -> Vec<u8> {
        let Some(steps) = Steps::parse(&self.steps) else { return Vec::new() };
        if excluded.excludes(by, &self.name, &steps).is_some() || self.decided() || self.decisions.iter().any(|d| d.who == by.who) {
            return Vec::new();
        }
        let fills = |n: u8| steps.slots().nth(usize::from(n)).is_some_and(|(_, _, slot)| excluded.may_fill(by, slot, "").is_ok());
        let apart = |n: u8| excluded.apart(by.who, n, &steps, &self.decisions, |_| String::new()).is_ok();
        self.slots().iter().filter(|s| s.open && fills(s.slot) && apart(s.slot)).map(|s| s.slot).collect()
    }

    /// Step `n` as a refusal names it: `slot 1 (reviewer) of "payout"`, or `"payout"` when it is
    /// the approval's one step.
    fn step_said(&self, slots: &[SlotState], n: u8) -> String {
        match slots {
            [_] => format!("{:?}", self.name),
            _ => format!("{} of {:?}", slots[usize::from(n)].named(), self.name),
        }
    }

    /// As `GET /deployments/<slug>/runs` lists it: `{name, expires_at}`, and for an approval its
    /// steps (`steps`, and `said` in words), each step as it stands (`slots`), and every
    /// decision taken; and what it shows of the run (`shows` or `input`) when `shown`: a run's
    /// values, for whoever reads its steps or decides it (R55, H6).
    fn listed(&self, shown: bool) -> serde_json::Value {
        let mut v = serde_json::json!({ "name": self.name, "expires_at": self.expires_at });
        if !self.is_event() {
            v["steps"] = self.steps.clone().into();
            v["said"] = Steps::parse(&self.steps).map_or_else(|| self.steps.clone(), |s| s.said()).into();
            v["slots"] = serde_json::to_value(self.slots()).unwrap_or_default();
            v["decisions"] = serde_json::to_value(&self.decisions).unwrap_or_default();
            match self.shown.as_ref().filter(|_| shown) {
                Some(s @ Shown::Shows(_)) => v["shows"] = s.value().clone(),
                Some(s @ Shown::Input(_)) => v["input"] = s.value().clone(),
                None => {}
            }
        }
        v
    }

    /// What an approval delivers, from its decisions: `{approved, by, decisions}`, who approved
    /// and every decision taken, with `declined_by` and `reason` when one declined, or
    /// `expired: true` when it ran out first.
    fn decision(&self, expired: bool) -> Value {
        let text = |s: &str| Value::Text(s.to_string());
        let mut d = BTreeMap::from([
            ("approved".to_string(), Value::Boolean(!expired && self.approved())),
            ("by".to_string(), Value::List(self.approvals().map(|a| text(&a.who)).collect())),
            ("decisions".to_string(), Value::List(self.decisions.iter().map(Decided::value).collect())),
        ]);
        if let Some(no) = self.declined() {
            d.insert("declined_by".into(), text(&no.who));
            d.insert("reason".into(), text(no.reason.as_deref().unwrap_or_default()));
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
    /// Who started it, signed in (`RunWaits::started_by`): they decide none of its approvals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_by: Option<String>,
    /// Its inputs, sourced values, requests, events and tokens, sealed, as the deployments file
    /// keeps them (they may hold a secret, a caller's credential, a token); opened again when
    /// the server starts (`open`). Never set on a run in memory that could be opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sealed: Option<String>,
    /// Whether a resume holds it right now: claimed under the deployments lock when its token
    /// is checked (or its deadline passes), so a token resumes it once even when requests carrying
    /// it arrive together. Never persisted: a restart frees it.
    #[serde(skip)]
    pub(crate) resuming: bool,
    /// The waiting point it was given its event at, while the part that follows runs: saved
    /// before any of it runs, with the decision it was given (`resume_parked`), and cleared when
    /// it has. Found set as the server starts, a crash cut that part off, and what it does may
    /// have been done: it is ended, not run again (`end_cut_off_parts`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) going_on: Option<String>,
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
    /// What each waiting point shows, by its name.
    #[serde(default)]
    shown: BTreeMap<String, Shown>,
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
            started_by: waits.started_by.clone(),
            sealed: None,
            resuming: false,
            going_on: None,
        })
    }

    /// The waiting point `token` resumes, if it is one of this run's and it still waits there: an
    /// event's, never an approval's, which people decide.
    pub(crate) fn waiting_point_of(&self, token: &str) -> Option<&str> {
        self.waiting.iter().filter(|w| w.is_event()).map(|w| w.name.as_str()).find(|name| {
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
        // Who started it is who started each later part: a run a part starts (calls.rs) is theirs.
        let waits = RunWaits { run_id: Some(self.run_id.clone()), tokens: self.tokens.clone(), events, started_by: self.started_by.clone(), ..Default::default() };
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

    /// This run as the deployments file keeps it: what may hold a secret sealed under its
    /// deployment's keys (`at_rest::deployment`), bound to its run id; or why it can't be.
    pub(crate) fn sealed_for_disk(&self, slug: &str, keys: &crate::at_rest::Keyring) -> Result<ParkedRun, String> {
        if self.sealed.is_some() {
            return Ok(self.clone());
        }
        let part = Sealable {
            inputs: self.inputs.clone(),
            sourced: self.sourced.clone(),
            rounds: self.rounds.clone(),
            tokens: self.tokens.clone(),
            events: self.events.clone(),
            shown: self.waiting.iter().filter_map(|w| Some((w.name.clone(), w.shown.clone()?))).collect(),
        };
        let plain = zeroize::Zeroizing::new(serde_json::to_string(&part).unwrap_or_default());
        let sealed = keys.seal_line(&crate::at_rest::deployment(slug), &parked_context(slug, &self.run_id), plain.as_bytes())?;
        Ok(ParkedRun {
            inputs: Vec::new(),
            sourced: Vec::new(),
            rounds: Vec::new(),
            tokens: BTreeMap::new(),
            events: BTreeMap::new(),
            waiting: self.waiting.iter().map(|w| StoredWaiting { shown: None, ..w.clone() }).collect(),
            sealed: Some(sealed),
            ..self.clone()
        })
    }

    /// Open what the deployments file sealed, with `keys`; nothing to do for a run not sealed.
    pub(crate) fn open(&mut self, slug: &str, keys: Option<&crate::at_rest::Keyring>) -> Result<(), String> {
        let Some(sealed) = &self.sealed else { return Ok(()) };
        let keys = keys.ok_or("it was sealed under keys this server doesn't have")?;
        let plain = keys.open_line(&crate::at_rest::deployment(slug), &parked_context(slug, &self.run_id), sealed).map_err(|e| e.to_string())?;
        let mut part: Sealable = crate::wire::read_json(&plain).map_err(|e| format!("its sealed part doesn't read: {e}"))?;
        for w in &mut self.waiting {
            w.shown = part.shown.remove(&w.name).or(w.shown.take());
        }
        self.inputs = part.inputs;
        self.sourced = part.sourced;
        self.rounds = part.rounds;
        self.tokens = part.tokens;
        self.events = part.events;
        self.sealed = None;
        Ok(())
    }

    /// What `GET .../runs` shows: never the tokens; of each point it waits at, what it shows of the
    /// run where `shown` says so.
    pub(crate) fn summary(&self, shown: impl Fn(&StoredWaiting) -> bool) -> serde_json::Value {
        serde_json::json!({
            "run_id": self.run_id,
            "kind": self.kind,
            "route": self.route,
            "by": self.started_by,
            "parked_at": self.parked_at,
            "waiting": self.waiting.iter().map(|w| w.listed(shown(w))).collect::<Vec<_>>(),
            "delivered": self.events.keys().collect::<Vec<_>>(),
        })
    }

    /// Who decides none of its approvals, or only some steps, as this host's `rules` say: who
    /// started it and their party (a step that says so, where the host lets one), and who deployed
    /// the version it is on (of its deployment's `versions`) and theirs, when they are excluded.
    fn excluded(&self, versions: &[wire::Version], rules: &Rules) -> Excluded {
        Excluded {
            starter: self.started_by.clone(),
            deployer: rules.settings.exclude_deployer.then(|| deployer_of(versions, &self.version, self.parked_at)).flatten(),
            no_agents: !rules.settings.agents,
            no_starter_steps: !rules.settings.starter,
            parties: Arc::clone(&rules.parties),
        }
    }
}

/// Who deployed the version a run is on, of a deployment's `versions`: of those with the `unit`
/// it started on, the last deployed by the time it parked, `at` (the last of them, if none was).
fn deployer_of(versions: &[wire::Version], unit: &str, at: u64) -> Option<String> {
    let by_then = versions.iter().rfind(|v| v.unit == unit && v.deployed_at.is_none_or(|d| d <= at));
    by_then.or_else(|| versions.iter().rfind(|v| v.unit == unit))?.by.clone()
}

/// Who decides none of a run's approvals, or only some of its steps, whatever they hold (the
/// design's §8.4): who started it and their party (`Parties`), but for a step that says
/// `starter: true` on a host that lets one (`no_starter_steps` off), and an agent that started it
/// never; whoever deployed the version it is on, and their party, when this host excludes them;
/// and every agent when it lets none decide (bindings.rs `ApprovalSettings`).
#[derive(Default)]
struct Excluded {
    starter: Option<String>,
    deployer: Option<String>,
    no_agents: bool,
    no_starter_steps: bool,
    parties: Arc<Parties>,
}

impl Excluded {
    /// Whether `by` started the run.
    fn started(&self, by: &Decider) -> bool {
        self.starter.as_deref() == Some(by.who)
    }

    /// Whether `who` started the run or is one party with whoever did, in words: "dee started
    /// this run", "quill and dee, who started this run, are one party (dee owns quill)".
    fn with_starter(&self, who: &str) -> Option<String> {
        let starter = self.starter.as_deref()?;
        if starter == who {
            return Some(format!("{who} started this run"));
        }
        let how = self.parties.joined(who, starter)?;
        Some(format!("{who} and {starter}, who started this run, are one party ({how})"))
    }

    /// Whether `who` deployed the version the run is on, where the host keeps them out, or is one
    /// party with whoever did, in words.
    fn with_deployer(&self, who: &str) -> Option<String> {
        let deployer = self.deployer.as_deref()?;
        if deployer == who {
            return Some(format!("{who} deployed the version this run is on"));
        }
        let how = self.parties.joined(who, deployer)?;
        Some(format!("{who} and {deployer}, who deployed the version this run is on, are one party ({how})"))
    }

    /// Whether `slot` takes whoever started the run, or one party with them: it says so, and this
    /// host lets it (an agent that started the run is kept out by name, `excludes`).
    fn takes_starter(&self, slot: &Slot) -> bool {
        slot.starter && !self.no_starter_steps
    }

    /// Why `by` decides none of the approval `name`, with `steps`, if they don't: they started the
    /// run, an agent, or they or their party did and no step takes them; the host keeps whoever
    /// deployed the version, and their party, out; or it lets no agent decide.
    fn excludes(&self, by: &Decider, name: &str, steps: &Steps) -> Option<String> {
        if self.started(by) && by.kind == Kind::Agent {
            return Some(format!("{} started this run, and an agent never decides a run it started: someone else decides its approval {name:?}", by.who));
        }
        if let Some(who) = self.with_starter(by.who).filter(|_| !steps.slots().any(|(_, _, s)| self.takes_starter(s))) {
            return Some(format!("{who}: someone else decides its approval {name:?}"));
        }
        if let Some(who) = self.with_deployer(by.who) {
            return Some(format!("{who}: on this host, someone else decides its approval {name:?}"));
        }
        (self.no_agents && by.kind == Kind::Agent).then(|| format!("{} is an agent, and this host lets no agent decide an approval", by.who))
    }

    /// Whether `by` may fill `slot`, `named` as a refusal says it: as their kind and roles let them
    /// (`Decider::may_fill`), and, when they or their party started the run, only a step that
    /// takes them.
    fn may_fill(&self, by: &Decider, slot: &Slot, named: &str) -> Result<(), String> {
        if let Some(who) = self.with_starter(by.who).filter(|_| !self.takes_starter(slot)) {
            let party = if self.started(by) { "" } else { ", nor one party with them" };
            return Err(format!("{who}, and {named} doesn't take whoever started it{party}"));
        }
        by.may_fill(slot, named)
    }

    /// Why `by` may not take step `n` of `steps` beside `decisions` (its decisions so far), if they
    /// may not: one of them is one party with `by`, and decided another step (`named`, as a
    /// refusal says a step). An approval takes one step of each party; the starter's party fills
    /// every step that takes whoever started the run, one principal each.
    fn apart(&self, by: &str, n: u8, steps: &Steps, decisions: &[Decided], named: impl Fn(u8) -> String) -> Result<(), String> {
        let starts = |n: u8| steps.slots().nth(usize::from(n)).is_some_and(|(_, _, s)| self.takes_starter(s));
        for d in decisions.iter().filter(|d| d.who != by) {
            let Some(how) = self.parties.joined(by, &d.who) else { continue };
            if starts(n) && starts(d.slot) && self.with_starter(by).is_some() && self.with_starter(&d.who).is_some() {
                continue;
            }
            return Err(format!("{by} and {} are one party ({how}), and {} decided {}: each step of an approval is another party's", d.who, d.who, named(d.slot)));
        }
        Ok(())
    }
}

/// What decides who takes an approval's steps here, besides what each holds: this host's approval
/// settings, and who answers for each agent and service (`Parties`). Read before the deployments
/// lock is taken, as the accounts are their own.
#[derive(Default)]
pub(crate) struct Rules {
    pub(crate) settings: ApprovalSettings,
    pub(crate) parties: Arc<Parties>,
}

impl Rules {
    /// This server's, as they are now.
    pub(crate) fn of(state: &AppState) -> Rules {
        let parties = state.access.accounts().map(|a| a.parties()).unwrap_or_default();
        Rules { settings: crate::bindings::approval_settings(state), parties: Arc::new(parties) }
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
    // What its approvals show those who decide them, redacted as its log entry is.
    let mut tokens = waits.tokens.clone();
    for w in &run.waiting {
        tokens.entry(w.name.clone()).or_insert_with(|| token_for(&run.run_id, &w.name));
    }
    for (w, shown) in record.waiting.iter_mut().zip(shown_of(kind, inputs, sourced, run, &tokens, redaction_of(deployments, slug))) {
        w.shown = shown;
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

/// Parked run `run_id`, claimed while it still waits at `name`, unless another resume holds it or
/// its deployment takes no run now (paused or stopped since it was found due).
fn claim<'a>(deployments: &'a Mutex<BTreeMap<String, Deployment>>, slug: &str, run_id: &str, name: &str) -> Option<Claimed<'a>> {
    let mut table = deployments.lock().unwrap();
    let dep = table.get_mut(slug)?;
    if dep.life.status != Status::Running {
        return None;
    }
    let run = dep.parked.get_mut(run_id)?;
    if run.resuming || !run.waiting.iter().any(|w| w.name == name) {
        return None;
    }
    run.resuming = true;
    Some(Claimed { deployments, slug: slug.to_string(), run_id: run_id.to_string() })
}

/// Deliver `event` to the claimed parked run's waiting point `name`, and run it again from its
/// record. Whether its deployment takes it was asked as it was claimed, under the lock a pause
/// takes (`decide`, `claim`, and a resume route before `claim_by_token`): claimed while it ran, it
/// goes on, as a run started before a pause does, and isn't refused here, where a decision it was
/// given would be kept with nothing to give it to the run later. It finishes (its effects
/// performed now, once), waits again, or fails; the log gets an entry for this part, under the
/// same run id. The claim is let go when this returns.
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
        record.open(slug, dep.secrets.lock().unwrap().keys.as_deref())?;
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
    // Saved as going on, with the decision it was given, before any of the part runs: a crash
    // while it runs leaves it so on disk, and the server, starting again, ends it rather than
    // give it its event again and make its effects twice (`end_cut_off_parts`). Not saved, it
    // doesn't go on: it waits, and its event (a decision, which stands) can be given again.
    let going_on = |at: Option<&str>| {
        if let Some(run) = deployments.lock().unwrap().get_mut(slug).and_then(|d| d.parked.get_mut(run_id)) {
            run.going_on = at.map(String::from);
        }
    };
    going_on(Some(name));
    if let Err(e) = persist_deployments(deployments_path, deployments) {
        going_on(None);
        return Err(format!("it wasn't saved as going on, so it waits on: {e}"));
    }
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
    // What a point it waits at now shows those who decide it, as for its first part.
    let mut shown = shown_of(&record.kind, &inputs, &sourced, &run, &tokens, redaction_of(deployments, slug)).into_iter();
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
            if let Some(kept) = dep.parked.get_mut(run_id) {
                kept.going_on = None;
            }
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
                // A point it was already waiting at keeps its deadline, and the decisions it had.
                kept.waiting = run
                    .waiting
                    .iter()
                    .map(|w| {
                        let shown = shown.next().flatten();
                        match kept.waiting.iter().find(|o| o.name == w.name) {
                            Some(o) => o.clone(),
                            None => StoredWaiting { shown, ..StoredWaiting::new(w, now + w.expires.min(dep.permit.durable.max_await_seconds)) },
                        }
                    })
                    .collect();
            }
        }
    }
    let saved = persist_deployments(deployments_path, deployments);
    match (run.error, saved) {
        (Some(e), _) => Err(e),
        (None, Err(e)) => Err(format!("it went on, but {e}")),
        (None, Ok(())) => Ok(()),
    }
}

/// As the server starts: each waiting run saved as going on (`ParkedRun::going_on`) had a part
/// cut off by a crash or a power loss, and what that part does may have been done (a payment
/// sent, a row written). It is ended, not run again, as a request a crash cut off isn't: given its
/// event again, its effects would be made twice. Its deployment's timeline says so, and its log
/// how far it got. Whether any was ended, so the deployments file is saved without it.
pub(crate) fn end_cut_off_parts(deployments: &mut BTreeMap<String, Deployment>) -> bool {
    let mut ended = false;
    for (slug, dep) in deployments.iter_mut() {
        let cut: Vec<(String, String)> = dep.parked.values().filter_map(|p| Some((p.run_id.clone(), p.going_on.clone()?))).collect();
        for (run_id, at) in cut {
            dep.parked.remove(&run_id);
            eprintln!("holon: run {run_id} of {slug:?} was going on from {at:?} when the server stopped: it is ended, not run again");
            let what = format!("ended run {run_id}, cut off going on from {at:?} when the server stopped: what it does may have been done, so it isn't run again (its log says how far it got)");
            dep.note("lifecycle", what, None, Default::default());
            ended = true;
        }
    }
    ended
}

/// Why a run waiting in `slug`, whose status is `status`, doesn't go on now.
fn not_running(slug: &str, status: Status) -> String {
    format!("{slug:?} is {}: a run waiting in it goes on once it runs again", crate::deploy::status_word(status))
}

/// How many expired waiting runs go on at once (R34). Each goes on in a thread of its own, so one
/// slow to finish (a request its part sends, a lock it waits for) holds no other deadline; past
/// this many, the rest stay due, and go on at the next pass, a second later.
pub(crate) const EXPIRING_AT_ONCE: usize = 16;

/// The expired runs a pass of [`expire_waiting`] set going, each in its thread. Dropped, they go
/// on alone, as the clock scheduler leaves them; [`Expiring::wait`] sees them through.
#[must_use = "dropped, the expired runs go on alone: wait() sees them through"]
// The server drops them (a stop waits for their runs at its door, stopping.rs); the tests wait.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Expiring(Vec<std::thread::JoinHandle<()>>);

impl Expiring {
    /// None set going: the server is stopping.
    pub(crate) fn none() -> Expiring {
        Expiring(Vec::new())
    }

    /// Until each has gone on (finished, waited again or failed).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn wait(self) {
        for going in self.0 {
            let _ = going.join();
        }
    }
}

/// One of the [`EXPIRING_AT_ONCE`] taken, given back as its run's part is over (or panicked).
struct Going(Arc<calls::Hub>);

impl Going {
    fn take(hub: &Arc<calls::Hub>) -> Option<Going> {
        use std::sync::atomic::Ordering::SeqCst;
        hub.expiring.fetch_update(SeqCst, SeqCst, |n| (n < EXPIRING_AT_ONCE).then_some(n + 1)).ok().map(|_| Going(Arc::clone(hub)))
    }
}

impl Drop for Going {
    fn drop(&mut self) {
        self.0.expiring.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Deliver `{expired: true}` to every waiting point past its deadline: to an approval, with who
/// approved it before it ran out, `approved` false. Not in a deployment that takes no run now
/// (paused or stopped): its runs wait on, and time out once it runs again. Each run goes on in a
/// thread of its own, claimed there, so this pass, and the next, wait for none of them (R34).
pub(crate) fn expire_waiting(store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: &Place, now: u64) -> Expiring {
    let mut due: Vec<(String, String, String, Value)> = Vec::new();
    for (slug, dep) in deployments.lock().unwrap().iter().filter(|(_, dep)| dep.takes_runs(store)) {
        // One going on now (resumed by a request, or by a pass before this one) is left to it.
        for p in dep.parked.values().filter(|p| !p.resuming) {
            for w in p.waiting.iter().filter(|w| w.expires_at <= now) {
                let event = if w.is_event() { expired_event() } else { w.decision(true) };
                due.push((slug.clone(), p.run_id.clone(), w.name.clone(), event));
            }
        }
    }
    let mut going = Vec::new();
    for (slug, run_id, name, event) in due {
        // As many at once as may be: the rest are due at the next pass.
        let Some(taken) = Going::take(&store.1) else { break };
        let (store, deployments, deployments_path) = (store.clone(), Arc::clone(deployments), deployments_path.clone());
        let spawned = std::thread::Builder::new().name(format!("holon-expire-{slug}")).spawn(move || {
            let _taken = taken;
            // A run a request (or another pass) is resuming right now isn't expired under it.
            let Some(claimed) = claim(&deployments, &slug, &run_id, &name) else {
                return;
            };
            if let Err(e) = resume_parked(&store, &deployments, &deployments_path, &slug, claimed, &name, event) {
                eprintln!("holon: expiring {name:?} of run {run_id} in {slug:?}: {e}");
            }
        });
        match spawned {
            Ok(thread) => going.push(thread),
            Err(e) => eprintln!("holon: expiring a waiting run: no thread for it ({e}); it is due again at the next pass"),
        }
    }
    Expiring(going)
}

/// `GET /deployments/<slug>/runs` — the runs waiting in it, without their tokens; each approval
/// point with what it shows (`shows`, or the run's `input`) and the open steps `caller` may fill
/// (`yours`, by number), as the decide route says under this host's `settings`. All of them to
/// whoever views the flow's runs; to whoever only decides, each run waiting at an approval they
/// may decide, with those points alone: what they decide, and nothing else of the flow's runs.
pub(crate) fn handle_get_runs(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, caller: &access::Caller, rules: &Rules) -> Response {
    let table = deployments.lock().unwrap();
    let Some(dep) = table.get(slug) else { return err(404, format!("no deployment {slug:?}")) };
    Response::json(&serde_json::json!({ "runs": waiting_runs(dep, slug, caller, rules) }))
}

/// `GET /runs/waiting`: every deployment's waiting runs as [`waiting_runs`] lists them to
/// `caller`, each with its deployment's slug. Someone who only decides an approval (bound at it,
/// or holding run.decide alone) may list no flow (`GET /deployments` asks flow.view), so this is
/// how the editor's Runs waiting finds what waits for them.
pub(crate) fn handle_get_waiting(deployments: &Mutex<BTreeMap<String, Deployment>>, caller: &access::Caller, rules: &Rules) -> Response {
    let table = deployments.lock().unwrap();
    let runs: Vec<serde_json::Value> = table
        .iter()
        .flat_map(|(slug, dep)| {
            waiting_runs(dep, slug, caller, rules).into_iter().map(move |mut run| {
                run["slug"] = slug.as_str().into();
                run
            })
        })
        .collect();
    Response::json(&serde_json::json!({ "runs": runs }))
}

/// The waiting runs of `dep` (deployed as `slug`) `caller` lists: each run and every point it
/// waits at to whoever views its runs; to whoever only decides one of its approvals, the runs
/// waiting at an approval they may decide, those points alone, and nothing delivered to its
/// events. Each approval says which of its steps are the caller's to take (`yours`). What a point
/// shows of its run, a run's values, is listed to whoever reads the flow's runs' steps
/// (run.steps, R55) or decides that point (H6), and to no one else.
fn waiting_runs(dep: &Deployment, slug: &str, caller: &access::Caller, rules: &Rules) -> Vec<serde_json::Value> {
    use crate::can::{Action, Resource, can};
    let views = can(caller, Action::RunView, &Resource::Flow(slug)).is_ok();
    let steps = can(caller, Action::RunSteps, &Resource::Flow(slug)).is_ok();
    let decides = |w: &StoredWaiting| !w.is_event() && can(caller, Action::RunDecide, &Resource::Approval { slug, name: Some(&w.name) }).is_ok();
    dep.parked.values().filter_map(|run| {
        let mut listed = run.summary(|w| steps || decides(w));
        let excluded = run.excluded(&dep.versions, rules);
        let points = listed["waiting"].as_array_mut()?;
        let mut kept = Vec::new();
        for (w, mut point) in run.waiting.iter().zip(std::mem::take(points)) {
            if !views && !decides(w) {
                continue;
            }
            if !w.is_event() {
                point["yours"] = yours(caller, slug, &excluded, w).into();
            }
            kept.push(point);
        }
        if kept.is_empty() {
            return None;
        }
        listed["waiting"] = kept.into();
        if !views {
            // What was delivered to its events is the flow's, not theirs.
            listed["delivered"] = serde_json::json!([]);
        }
        Some(listed)
    })
    .collect()
}

/// The open steps of `w` `caller` may fill, by number: none when `run.decide` refuses them there.
fn yours(caller: &access::Caller, slug: &str, excluded: &Excluded, w: &StoredWaiting) -> Vec<u8> {
    let Ok(by) = decider(caller, slug, &w.name) else { return Vec::new() };
    let point = crate::can::Resource::Approval { slug, name: Some(&w.name) };
    let holds = |role: Option<&str>| crate::can::holding(caller, role, &point).and_then(|h| h.from);
    w.yours(excluded, &Decider { who: &by, kind: caller.kind.unwrap_or_default(), holds: &holds, via: caller.local.as_deref() })
}

/// What a decision did: the approval still waits for more, or it is decided, `approved` or not,
/// and the run is held to be given the decision. Each with the step it took, and the steps as
/// they stand.
pub(crate) enum Outcome<'a> {
    Waits { slot: u8, slots: Vec<SlotState> },
    Decides { claimed: Claimed<'a>, slot: u8, slots: Vec<SlotState>, approved: bool, event: Value },
}

/// Who decides, as the decide route found them: their name, their kind, and the binding they
/// hold a role through where the approval is, by its name (`can::holding`; none named, a role
/// holding run.decide), with the version it was read in; none when they hold it through none.
pub(crate) struct Decider<'a> {
    pub(crate) who: &'a str,
    pub(crate) kind: Kind,
    pub(crate) holds: &'a dyn Fn(Option<&str>) -> Option<Bound>,
    /// Through this machine's run file, as its OS user (`local:<user>`): none through their own
    /// sign-in.
    pub(crate) via: Option<&'a str>,
}

impl Decider<'_> {
    /// Whether they may fill `slot`, `named` as a refusal says it: a person's step takes a person,
    /// an agent's an agent, and a step with a role someone bound to it (the plan's D37, D38).
    fn may_fill(&self, slot: &Slot, named: &str) -> Result<(), String> {
        let theirs = matches!((slot.kind, self.kind), (SlotKind::Person, Kind::Person) | (SlotKind::Agent, Kind::Agent));
        if !theirs {
            return Err(format!("{} is {}, and {named} is {}'s", self.who, self.kind.said(), if slot.kind == SlotKind::Person { "a person" } else { "an agent" }));
        }
        match &slot.role {
            Some(role) if (self.holds)(Some(role)).is_none() => Err(format!("{named} takes someone bound to {role} here, and {} isn't", self.who)),
            // Every decision names the binding that let it (D43, D45): none, no decision.
            None if (self.holds)(None).is_none() => Err(format!("{named} takes someone bound here to a role that decides it, and {} isn't", self.who)),
            _ => Ok(()),
        }
    }
}

/// A decision: approved or not, the step it names (none: the one the decider may fill), why, a
/// note, and when, seconds since the epoch.
pub(crate) struct Decision {
    pub(crate) approve: bool,
    pub(crate) slot: Option<u8>,
    pub(crate) reason: Option<String>,
    pub(crate) note: Option<serde_json::Value>,
    pub(crate) at: u64,
}

impl StoredWaiting {
    /// `by`'s decision on it: the step it names, or the one open step `by` may fill; none when
    /// the run's `excluded` keeps them out. The step taken, and the decision to deliver once it is
    /// decided; the status and why when it can't be decided. One decided already (its delivery
    /// turned away) is delivered again, whichever step is named.
    fn decide(&mut self, excluded: &Excluded, by: &Decider, decision: Decision) -> Result<(u8, Option<Value>), (u16, String)> {
        let name = &self.name;
        if self.is_event() {
            return Err((409, format!("{name:?} is an event, which its route delivers: nobody decides it")));
        }
        let Some(steps) = Steps::parse(&self.steps) else {
            return Err((409, format!("{name:?} asks for steps this host can't read ({}): nobody decides it, and it runs out", self.steps)));
        };
        if let Some(why) = excluded.excludes(by, name, &steps) {
            return Err((403, why));
        }
        let slot = if self.decided() { decision.slot.unwrap_or(0) } else { self.pick(&steps, excluded, by, decision.slot)? };
        let role = steps.slots().nth(usize::from(slot)).and_then(|(_, _, s)| s.role.clone());
        // What let them: the binding of the step's role, or of run.decide for a step without one.
        let (binding, access) = (by.holds)(role.as_deref()).map_or((None, None), |b| (Some(b.binding), Some(b.access)));
        let d = Decided { slot, who: by.who.to_string(), kind: by.kind, role, at: decision.at, approved: decision.approve, reason: decision.reason, note: decision.note, binding, access, via: by.via.map(String::from) };
        self.take(d).map(|taken| (slot, taken)).map_err(|why| (409, why))
    }

    /// The step `by` decides: the one named, when they may fill it (whether it is open, `take`
    /// says), or else the one open step they may fill. Refused when they may fill none (403), or
    /// may fill several and named none (409, the plan's D36). Whoever started the run fills only a
    /// step that takes them (`excluded`).
    fn pick(&self, steps: &Steps, excluded: &Excluded, by: &Decider, named: Option<u8>) -> Result<u8, (u16, String)> {
        let slots = self.slots();
        let fills = |s: &SlotState| -> Result<(), String> {
            let (_, _, slot) = steps.slots().nth(usize::from(s.slot)).expect("a slot of the steps");
            excluded.may_fill(by, slot, &self.step_said(&slots, s.slot))?;
            excluded.apart(by.who, s.slot, steps, &self.decisions, |n| self.step_said(&slots, n))
        };
        if let Some(n) = named {
            let s = slots.get(usize::from(n)).ok_or((409, format!("{:?} has {} steps, from slot 0: there is no slot {n}", self.name, slots.len())))?;
            return fills(s).map(|()| n).map_err(|why| (403, why));
        }
        let open: Vec<&SlotState> = slots.iter().filter(|s| s.open).collect();
        let mine: Vec<&SlotState> = open.iter().copied().filter(|s| fills(s).is_ok()).collect();
        let said = |slots: &[&SlotState]| slots.iter().map(|s| s.named()).collect::<Vec<_>>().join(" and ");
        match (mine.as_slice(), open.as_slice()) {
            ([one], _) => Ok(one.slot),
            ([], [one]) => Err((403, fills(one).unwrap_err())),
            ([], _) => Err((403, format!("{} may fill none of the open steps of {:?}: {}", by.who, self.name, open.iter().map(|s| fills(s).unwrap_err()).collect::<Vec<_>>().join("; ")))),
            (more, _) => Err((409, format!("{} may fill {} of {:?}: say which, with slot", by.who, said(more), self.name))),
        }
    }
}

/// `by`'s decision on the approval `name` of the run `run_id` in `slug`, made under the
/// deployments lock, so two deciding at once decide it once (the design's §8.3), and under this
/// host's `settings`. Each decision is kept on the waiting point (`StoredWaiting::decide`); when
/// every step is filled, or one declines (with a `reason`), the run is claimed to be given the
/// decision. The status and why when it can't be decided: 409 while the deployment is paused or
/// stopped, nothing kept of it, as it takes no run then.
pub(crate) fn decide<'a>(
    deployments: &'a Mutex<BTreeMap<String, Deployment>>,
    slug: &str,
    run_id: &str,
    name: &str,
    by: &Decider,
    decision: Decision,
    rules: &Rules,
) -> Result<Outcome<'a>, (u16, String)> {
    let mut table = deployments.lock().unwrap();
    let dep = table.get_mut(slug).ok_or((404, format!("no deployment {slug:?}")))?;
    if dep.life.status != Status::Running {
        return Err((409, not_running(slug, dep.life.status)));
    }
    let run = dep.parked.get_mut(run_id).ok_or((404, format!("no run {run_id} of {slug:?} is waiting: it may have been decided, or finished")))?;
    if run.resuming {
        return Err((409, format!("run {run_id} of {slug:?} is going on right now: try again in a moment")));
    }
    let excluded = run.excluded(&dep.versions, rules);
    let w = run.waiting.iter_mut().find(|w| w.name == name).ok_or((404, format!("run {run_id} of {slug:?} isn't waiting at {name:?}")))?;
    let (slot, taken) = w.decide(&excluded, by, decision)?;
    let slots = w.slots();
    let Some(event) = taken else {
        return Ok(Outcome::Waits { slot, slots });
    };
    let approved = w.approved();
    run.resuming = true;
    Ok(Outcome::Decides { claimed: Claimed { deployments, slug: slug.to_string(), run_id: run_id.to_string() }, slot, slots, approved, event })
}

/// What `POST /deployments/<slug>/runs/<run_id>/decide` takes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideBody {
    name: String,
    approve: bool,
    #[serde(default)]
    reason: Option<String>,
    /// The step it decides, from 0 as the steps are written; none: the one open step the caller
    /// may fill.
    #[serde(default)]
    slot: Option<u8>,
    /// Any JSON, given to the run with the decision.
    #[serde(default)]
    note: Option<serde_json::Value>,
}

/// The longest reason a decision may give, in characters.
const MAX_REASON_CHARS: usize = 1000;
/// The longest note a decision may carry, in bytes of JSON.
const MAX_NOTE_BYTES: usize = 16 * 1024;

/// Who `caller` decides `slug`'s approval point `point` as: a person or an agent signed in, by
/// their name (the run file as the account it is tied to, D45); or why they can't decide (can.rs):
/// a service (a script), a viewer, one bound at another point, anyone on a server without
/// accounts, or the run file tied to no account, which holds admin and no binding. Which step they
/// may fill, the step's kind and role say (`handle_post_decide`).
pub(crate) fn decider(caller: &access::Caller, slug: &str, point: &str) -> Result<String, crate::can::Refusal> {
    crate::can::can(caller, crate::can::Action::RunDecide, &crate::can::Resource::Approval { slug, name: Some(point) })?;
    let name = caller.name.as_deref().unwrap_or_default();
    if caller.untied() {
        return Err(crate::can::Refusal(format!(
            "{name} is this machine's run file, tied to no account now (the one that set the server up is gone or disabled): an admin ties it to a person with holon access local --as NAME"
        )));
    }
    Ok(name.to_string())
}

/// `POST /deployments/<slug>/runs/<run_id>/decide` — a principal approves, or declines with a
/// reason, a step of the approval `name` the run waits at (`slot`, or the one open step they may
/// fill), with a `note` if they like. A step with a role takes someone bound to that role where the
/// approval is (`can::holding`); a person's step a person, an agent's an agent. Answered
/// `{name, slot, slots, outcome}`, `slots` each step as it stands, `outcome` `waiting` while it
/// waits for more, else `approved` or `declined`, the run having been given the decision
/// (`run_error` when its part then failed).
pub(crate) fn handle_post_decide(state: &AppState, slug: &str, run_id: &str, caller: &access::Caller, request: &Request) -> Response {
    // The body names the point, so it is read first: a malformed one is 400, as on any route.
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body: DecideBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("a decision is {{\"name\": \"payout\", \"approve\": true, \"reason\": \"…\", \"slot\": 1, \"note\": {{…}}}}: {e}")),
    };
    let by = match decider(caller, slug, &body.name) {
        Ok(by) => by,
        Err(refusal) => return refusal.response(),
    };
    let kind = caller.kind.unwrap_or_default();
    let reason = body.reason.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    if reason.as_ref().is_some_and(|r| r.chars().count() > MAX_REASON_CHARS) {
        return err(400, format!("a reason is at most {MAX_REASON_CHARS} characters"));
    }
    if !body.approve && reason.is_none() {
        return err(400, "a decline says why: give a reason");
    }
    if body.note.as_ref().is_some_and(|n| n.to_string().len() > MAX_NOTE_BYTES) {
        return err(400, format!("a note is at most {} KiB of JSON", MAX_NOTE_BYTES / 1024));
    }
    // A step with a role takes someone bound to it where the approval is, and keeps the binding.
    let point = crate::can::Resource::Approval { slug, name: Some(&body.name) };
    let holds = |role: Option<&str>| crate::can::holding(caller, role, &point).and_then(|h| h.from);
    let decider = Decider { who: &by, kind, holds: &holds, via: caller.local.as_deref() };
    let (deployments, path) = (&state.deployments, &state.deployments_path);
    // In the activity log as what it was, by whom, at which step.
    let recorded = |state: &AppState, slot: u8| state.activity.record(if body.approve { "approved" } else { "declined" }, &format!("{slug} run {run_id} at {:?}, slot {slot}", body.name), Some(&by));
    let reply = |slot: u8, slots: &[SlotState], outcome: &str| serde_json::json!({ "name": body.name, "slot": slot, "slots": slots, "outcome": outcome });
    let decision = Decision { approve: body.approve, slot: body.slot, reason, note: body.note, at: epoch_now() };
    match decide(deployments, slug, run_id, &body.name, &decider, decision, &Rules::of(state)) {
        Err((code, why)) => err(code, why),
        Ok(Outcome::Waits { slot, slots }) => {
            recorded(state, slot);
            if let Err(e) = persist_deployments(path, deployments) {
                return err(500, format!("your decision is taken, but {e}: after a restart it isn't"));
            }
            Response::json(&reply(slot, &slots, "waiting"))
        }
        Ok(Outcome::Decides { claimed, slot, slots, approved, event }) => {
            recorded(state, slot);
            let mut answer = reply(slot, &slots, if approved { "approved" } else { "declined" });
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

    /// What an approval that names nothing shows, by what started its run: an action's argument as
    /// it is (a field called headers too), a request without its headers, the file, mail, feed
    /// event or queue message a source delivered (not what else it read), and nothing for a clock.
    #[test]
    fn an_approval_naming_nothing_shows_what_started_its_run() {
        let v = |json: &str| holon_core::parse_json(json).unwrap();
        let clock = (Capability::Clock, Value::Number(5.0));
        // Each row: the run's kind, its input, what it sourced, and what the approval shows.
        type Row<'a> = (&'a str, Option<Value>, Vec<(Capability, Value)>, Option<&'a str>);
        let rows: Vec<Row> = vec![
            ("trigger", Some(v(r#"{"amount":30,"headers":{"x":"1"}}"#)), vec![], Some(r#"{"amount":30,"headers":{"x":"1"}}"#)),
            ("request", Some(v(r#"{"method":"POST","path":"/pay","headers":{"authorization":"Bearer t"},"body":"{}"}"#)), vec![], Some(r#"{"body":"{}","method":"POST","path":"/pay"}"#)),
            ("file", None, vec![clock.clone(), (Capability::FileArrival("in".into()), v(r#"{"name":"a.csv"}"#))], Some(r#"{"name":"a.csv"}"#)),
            ("mail", None, vec![(Capability::MailArrival("ops".into()), v(r#"{"subject":"pay"}"#))], Some(r#"{"subject":"pay"}"#)),
            ("feed", None, vec![(Capability::FeedEvent("prices".into()), v(r#"{"price":3}"#))], Some(r#"{"price":3}"#)),
            ("queue", None, vec![(Capability::QueueMessage("jobs".into()), v(r#""go""#))], Some(r#""go""#)),
            ("tick", None, vec![clock.clone()], None),
        ];
        for (kind, input, sourced, shown) in rows {
            let got = input_of(kind, &[input], &sourced).map(|v| holon_core::stringify_json(&v).unwrap());
            assert_eq!(got.as_deref(), shown, "{kind}");
        }
    }

    /// **T5** (docs/L4s/approval-steps-plan.md §5): what a run waiting at an approval for two
    /// people is given, from the decisions taken on it, each row its decisions in turn, which of
    /// them decides it (none: it runs out), and the value. Every decision is in `decisions`, with
    /// its step, kind, when, note and what let it; a decline is the decision, and stands.
    #[test]
    fn an_approval_gives_its_run_every_decision_taken() {
        let point = || StoredWaiting { name: "payout".into(), expires_at: 100, steps: "person*2".into(), decisions: Vec::new(), shown: None };
        let took = |who: &str, approved: bool, reason: Option<&str>, at: u64| Decided {
            slot: 0,
            who: who.into(),
            kind: Kind::Person,
            role: None,
            at,
            approved,
            reason: reason.map(Into::into),
            note: None,
            binding: None,
            access: None,
            via: None,
        };
        let noted = |d: Decided| Decided { note: Some(serde_json::json!({ "checked": ["invoice", "po"], "score": 0.93 })), ..d };
        let through = |d: Decided, binding: u64, access: u64| Decided { binding: Some(binding), access: Some(access), ..d };
        let by_run_file = |d: Decided| Decided { via: Some("local:bma".into()), ..d };
        let ada = serde_json::json!({ "slot": 0, "who": "ada", "kind": "person", "at": 5, "approved": true });
        let rows = [
            (
                "approved by two people, one with a note through this machine's run file, each through what let them",
                vec![
                    through(took("ada", true, None, 5), 3, 7),
                    by_run_file(through(noted(took("bob", true, Some("the invoice matches"), 9)), 4, 7)),
                ],
                Some(1),
                serde_json::json!({
                    "approved": true,
                    "by": ["ada", "bob"],
                    "decisions": [{ "slot": 0, "who": "ada", "kind": "person", "at": 5, "approved": true, "binding": 3, "access": 7 }, {
                        "slot": 0, "who": "bob", "kind": "person", "at": 9, "approved": true, "reason": "the invoice matches",
                        "note": { "checked": ["invoice", "po"], "score": 0.93 }, "binding": 4, "access": 7, "via": "local:bma",
                    }],
                }),
            ),
            (
                "declined by one",
                vec![took("bob", false, Some("not this vendor"), 7)],
                Some(0),
                serde_json::json!({
                    "approved": false,
                    "by": [],
                    "decisions": [{ "slot": 0, "who": "bob", "kind": "person", "at": 7, "approved": false, "reason": "not this vendor" }],
                    "declined_by": "bob",
                    "reason": "not this vendor",
                }),
            ),
            (
                "approved by one, then declined by another",
                vec![took("ada", true, None, 5), took("bob", false, Some("wrong account"), 8)],
                Some(1),
                serde_json::json!({
                    "approved": false,
                    "by": ["ada"],
                    "decisions": [ada, { "slot": 0, "who": "bob", "kind": "person", "at": 8, "approved": false, "reason": "wrong account" }],
                    "declined_by": "bob",
                    "reason": "wrong account",
                }),
            ),
            (
                "declined, a run lock turning it away, then decided again by two: the decline stands",
                vec![took("bob", false, Some("not this vendor"), 7), took("ada", true, None, 5), took("cy", true, None, 9)],
                Some(0),
                serde_json::json!({
                    "approved": false,
                    "by": [],
                    "decisions": [{ "slot": 0, "who": "bob", "kind": "person", "at": 7, "approved": false, "reason": "not this vendor" }],
                    "declined_by": "bob",
                    "reason": "not this vendor",
                }),
            ),
            (
                "approved by one, then run out",
                vec![took("ada", true, None, 5)],
                None,
                serde_json::json!({ "approved": false, "by": ["ada"], "decisions": [ada], "expired": true }),
            ),
            ("run out undecided", vec![], None, serde_json::json!({ "approved": false, "by": [], "decisions": [], "expired": true })),
        ];
        for (what, decisions, decides_at, want) in rows {
            let mut w = point();
            let mut given = None;
            for (i, d) in decisions.into_iter().enumerate() {
                given = w.take(d).unwrap();
                assert_eq!(given.is_some(), decides_at.is_some_and(|at| i >= at), "{what}: decision {i}");
            }
            let given = given.unwrap_or_else(|| w.decision(true));
            let given: serde_json::Value = serde_json::from_str(&holon_core::stringify_json(&given).unwrap()).unwrap();
            assert_eq!(given, want, "{what}");
        }

        // One person approves once, and it waits on for another.
        let mut w = point();
        assert_eq!(w.take(took("ada", true, None, 5)), Ok(None));
        assert_eq!(w.take(took("ada", true, None, 6)), Err("ada already approved \"payout\": it needs 2 different people".into()));
        assert_eq!(w.decisions.len(), 1, "the refused approval isn't kept");
    }

    /// **T6** (docs/L4s/approval-steps-plan.md §7): who decides which step of an approval, over a
    /// cast and steps of each shape (a chain, any order, a step of two, people alone), and the
    /// parties among them (the audit's R18: one party a step, the starter's kept out but where a
    /// step takes them). Each row is decisions in turn, each naming its step or not, and taking a
    /// step (and whether that decides it) or refused with its status and sentence; then what the
    /// run is given: its decision, or, undecided, the expiry with the approvals so far
    /// (`decisions` is T5's).
    #[test]
    fn each_step_is_decided_in_its_order_by_its_kind_and_role() {
        use serde_json::json;
        // Who's who: their kind, and the roles they are bound to where the approval is. The admin
        // holds everything, and no role by name; sam starts the runs that have a starter. quill is
        // sam's agent, and ana made pip's token: each is one party with them.
        let cast: &[(&str, Kind, &[&str])] = &[
            ("ana", Kind::Person, &["reviewer"]),
            ("ben", Kind::Person, &["reviewer"]),
            ("cy", Kind::Person, &["approver"]),
            ("dee", Kind::Person, &["reviewer", "approver"]),
            ("pip", Kind::Agent, &["classifier"]),
            ("quill", Kind::Agent, &["classifier", "drafter"]),
            ("root", Kind::Person, &[]),
            ("sam", Kind::Person, &["reviewer"]),
            ("scout", Kind::Agent, &["classifier", "drafter"]),
        ];
        let parties = Arc::new(Parties::of(&[("pip", &[("ana", "ana made pip's token \"pip\"")]), ("quill", &[("sam", "sam owns quill")]), ("scout", &[])]));
        let chain = "agent:classifier>person:reviewer>person:approver";
        let (ok, no) = (|slot: u8, decides: bool| Ok((slot, decides)), |code: u16, why: &'static str| Err((code, why)));
        // (who, the step named, approves), then what it does.
        type Step = (&'static str, Option<u8>, bool, Result<(u8, bool), (u16, &'static str)>);
        // What it shows, the steps, who started the run, whether the host lets a step take them, the
        // decisions, and what the run is given.
        type Row = (&'static str, &'static str, Option<&'static str>, bool, Vec<Step>, serde_json::Value);
        let rows: Vec<Row> = vec![
            (
                "a chain, decided in its order by each step's kind and role",
                chain,
                Some("sam"),
                true,
                vec![
                    ("ana", None, true, no(403, "ana is a person, and slot 0 (classifier) of \"payout\" is an agent's")),
                    ("scout", None, true, ok(0, false)),
                    ("cy", None, true, no(403, "slot 1 (reviewer) of \"payout\" takes someone bound to reviewer here, and cy isn't")),
                    ("cy", Some(2), true, no(409, "slot 2 (approver) of \"payout\" opens once slot 1 (reviewer) is filled")),
                    ("cy", Some(7), true, no(409, "\"payout\" has 3 steps, from slot 0: there is no slot 7")),
                    ("sam", None, true, no(403, "sam started this run: someone else decides its approval \"payout\"")),
                    ("root", None, true, no(403, "slot 1 (reviewer) of \"payout\" takes someone bound to reviewer here, and root isn't")),
                    ("ana", None, true, ok(1, false)),
                    ("ben", Some(1), true, no(409, "slot 1 (reviewer) of \"payout\" is filled: ana approved it")),
                    ("cy", None, true, ok(2, true)),
                ],
                json!({ "approved": true, "by": ["scout", "ana", "cy"] }),
            ),
            (
                "one principal, one step; a decline ends it",
                chain,
                None,
                true,
                vec![
                    ("scout", None, true, ok(0, false)),
                    ("dee", None, true, ok(1, false)),
                    ("dee", Some(2), true, no(409, "dee already decided slot 1 (reviewer) of \"payout\": each step of an approval is someone else's")),
                    ("cy", None, false, ok(2, true)),
                    ("ana", Some(1), true, ok(1, true)),
                ],
                json!({ "approved": false, "by": ["scout", "dee"], "declined_by": "cy", "reason": "not this vendor" }),
            ),
            (
                "any order: two steps open at once",
                "person:reviewer+person:approver",
                None,
                true,
                vec![
                    ("dee", None, true, no(409, "dee may fill slot 0 (reviewer) and slot 1 (approver) of \"payout\": say which, with slot")),
                    ("scout", None, true, no(403, "scout may fill none of the open steps of \"payout\": scout is an agent, and slot 0 (reviewer) of \"payout\" is a person's; scout is an agent, and slot 1 (approver) of \"payout\" is a person's")),
                    ("cy", None, true, ok(1, false)),
                    ("dee", Some(0), true, ok(0, true)),
                ],
                json!({ "approved": true, "by": ["cy", "dee"] }),
            ),
            (
                "a step of two different people, then run out",
                "person:reviewer*2>person:approver",
                None,
                true,
                vec![
                    ("ana", None, true, ok(0, false)),
                    ("ana", Some(0), true, no(409, "ana already approved slot 0 (reviewer) of \"payout\": it needs 2 different people")),
                    ("ana", Some(0), false, no(409, "ana already approved slot 0 (reviewer) of \"payout\", and a decision stands: someone else declines it")),
                    ("cy", None, true, no(403, "slot 0 (reviewer) of \"payout\" takes someone bound to reviewer here, and cy isn't")),
                    ("ben", None, true, ok(0, false)),
                    ("scout", None, true, no(403, "scout is an agent, and slot 1 (approver) of \"payout\" is a person's")),
                ],
                json!({ "approved": false, "by": ["ana", "ben"], "expired": true }),
            ),
            (
                "people alone: anyone run.decide lets in, a person",
                "person*2",
                Some("sam"),
                true,
                vec![
                    ("root", None, true, ok(0, false)),
                    ("scout", None, true, no(403, "scout is an agent, and \"payout\" is a person's")),
                    ("root", None, true, no(409, "root already approved \"payout\": it needs 2 different people")),
                    ("ana", None, false, ok(0, true)),
                ],
                json!({ "approved": false, "by": ["root"], "declined_by": "ana", "reason": "not this vendor" }),
            ),
            (
                "an output review: whoever started it takes the step that says so, and only that one",
                "person:reviewer>person:approver^",
                Some("dee"),
                true,
                vec![
                    ("dee", Some(0), true, no(403, "dee started this run, and slot 0 (reviewer) of \"payout\" doesn't take whoever started it")),
                    ("dee", None, true, no(403, "dee started this run, and slot 0 (reviewer) of \"payout\" doesn't take whoever started it")),
                    ("ana", None, true, ok(0, false)),
                    ("dee", None, true, ok(1, true)),
                ],
                json!({ "approved": true, "by": ["ana", "dee"] }),
            ),
            (
                "a host that lets no step take whoever started the run",
                "person:reviewer>person:approver^",
                Some("dee"),
                false,
                vec![("ana", None, true, ok(0, false)), ("dee", None, true, no(403, "dee started this run: someone else decides its approval \"payout\"")), ("cy", None, true, ok(1, true))],
                json!({ "approved": true, "by": ["ana", "cy"] }),
            ),
            (
                "an agent never decides a run it started, whatever the steps say",
                "agent:classifier>person:reviewer^",
                Some("scout"),
                true,
                vec![("scout", None, true, no(403, "scout started this run, and an agent never decides a run it started: someone else decides its approval \"payout\""))],
                json!({ "approved": false, "by": [], "expired": true }),
            ),
            (
                "one party, one step: an agent and whoever made its token",
                chain,
                None,
                true,
                vec![
                    ("pip", None, true, ok(0, false)),
                    ("ana", None, true, no(403, "ana and pip are one party (ana made pip's token \"pip\"), and pip decided slot 0 (classifier) of \"payout\": each step of an approval is another party's")),
                    ("ana", Some(1), true, no(403, "ana and pip are one party (ana made pip's token \"pip\"), and pip decided slot 0 (classifier) of \"payout\": each step of an approval is another party's")),
                    ("ben", None, true, ok(1, false)),
                    ("cy", None, true, ok(2, true)),
                ],
                json!({ "approved": true, "by": ["pip", "ben", "cy"] }),
            ),
            (
                "whoever started the run keeps their party out",
                chain,
                Some("sam"),
                true,
                vec![
                    ("quill", None, true, no(403, "quill and sam, who started this run, are one party (sam owns quill): someone else decides its approval \"payout\"")),
                    ("scout", None, true, ok(0, false)),
                    ("ana", None, true, ok(1, false)),
                    ("cy", None, true, ok(2, true)),
                ],
                json!({ "approved": true, "by": ["scout", "ana", "cy"] }),
            ),
            (
                "a step that doesn't take the starter's party, and one that does",
                "agent:classifier>person:reviewer^",
                Some("sam"),
                true,
                vec![
                    ("quill", Some(0), true, no(403, "quill and sam, who started this run, are one party (sam owns quill), and slot 0 (classifier) of \"payout\" doesn't take whoever started it, nor one party with them")),
                    ("scout", None, true, ok(0, false)),
                    ("sam", None, true, ok(1, true)),
                ],
                json!({ "approved": true, "by": ["scout", "sam"] }),
            ),
            (
                "the starter's party fills each step that takes them, one principal each: their agent drafts, they read it",
                "agent:drafter^>person:reviewer^",
                Some("sam"),
                true,
                vec![("quill", None, true, ok(0, false)), ("sam", None, true, ok(1, true))],
                json!({ "approved": true, "by": ["quill", "sam"] }),
            ),
            (
                "but not another's party: sam's agent drafted a run dee started",
                "agent:drafter^>person:reviewer^",
                Some("dee"),
                true,
                vec![
                    ("quill", None, true, ok(0, false)),
                    ("sam", None, true, no(403, "sam and quill are one party (sam owns quill), and quill decided slot 0 (drafter) of \"payout\": each step of an approval is another party's")),
                    ("dee", None, true, ok(1, true)),
                ],
                json!({ "approved": true, "by": ["quill", "dee"] }),
            ),
            (
                "an agent's step may take the starter's party, and never an agent that started it",
                "agent:drafter^>person:reviewer",
                Some("quill"),
                true,
                vec![
                    ("quill", None, true, no(403, "quill started this run, and an agent never decides a run it started: someone else decides its approval \"payout\"")),
                    ("sam", Some(1), true, no(403, "sam and quill, who started this run, are one party (sam owns quill), and slot 1 (reviewer) of \"payout\" doesn't take whoever started it, nor one party with them")),
                    ("scout", None, true, ok(0, false)),
                    ("ana", None, true, ok(1, true)),
                ],
                json!({ "approved": true, "by": ["scout", "ana"] }),
            ),
            ("an event", "", None, true, vec![("ana", None, true, no(409, "\"payout\" is an event, which its route delivers: nobody decides it"))], json!({ "approved": false, "by": [], "expired": true })),
            (
                "steps this host can't read",
                "person*9",
                None,
                true,
                vec![("ana", None, true, no(409, "\"payout\" asks for steps this host can't read (person*9): nobody decides it, and it runs out"))],
                json!({ "approved": false, "by": [], "expired": true }),
            ),
        ];
        for (what, steps, starter, starter_steps, decisions, want) in rows {
            let mut w = StoredWaiting { name: "payout".into(), expires_at: 100, steps: steps.into(), decisions: Vec::new(), shown: None };
            let mut decided = false;
            for (i, (who, slot, approve, then)) in decisions.into_iter().enumerate() {
                let (_, kind, roles) = cast.iter().find(|(name, ..)| *name == who).unwrap();
                let holds = |role: Option<&str>| role.is_none_or(|r| roles.contains(&r)).then_some(Bound { binding: 1, access: 1 });
                let by = Decider { who, kind: *kind, holds: &holds, via: None };
                let decision = Decision { approve, slot, reason: (!approve).then(|| "not this vendor".into()), note: None, at: 10 + i as u64 };
                let excluded = Excluded { starter: starter.map(String::from), no_starter_steps: !starter_steps, parties: Arc::clone(&parties), ..Default::default() };
                let got = w.decide(&excluded, &by, decision);
                let got = got.as_ref().map(|(slot, taken)| (*slot, taken.is_some())).map_err(|(code, why)| (*code, why.as_str()));
                assert_eq!(got, then, "{what}: decision {i}, {who}");
                decided |= got.is_ok_and(|(_, decides)| decides);
            }
            let given = w.decision(!decided);
            let mut given: serde_json::Value = serde_json::from_str(&holon_core::stringify_json(&given).unwrap()).unwrap();
            given.as_object_mut().unwrap().remove("decisions");
            assert_eq!(given, want, "{what}");
        }

        // What each may take now says so too (`yours`): pip took the classifier's step, so ana,
        // one party with pip, takes none; ben the reviewer's.
        let mut w = StoredWaiting { name: "payout".into(), expires_at: 100, steps: chain.into(), decisions: Vec::new(), shown: None };
        let excluded = Excluded { parties: Arc::clone(&parties), ..Default::default() };
        let decider = |who: &'static str| {
            let (_, kind, roles) = *cast.iter().find(|(name, ..)| *name == who).unwrap();
            (who, kind, roles)
        };
        let yours = |w: &StoredWaiting, who: &'static str| {
            let (who, kind, roles) = decider(who);
            let holds = |role: Option<&str>| role.is_none_or(|r| roles.contains(&r)).then_some(Bound { binding: 1, access: 1 });
            w.yours(&excluded, &Decider { who, kind, holds: &holds, via: None })
        };
        let (who, kind, roles) = decider("pip");
        let holds = |role: Option<&str>| role.is_none_or(|r| roles.contains(&r)).then_some(Bound { binding: 1, access: 1 });
        w.decide(&excluded, &Decider { who, kind, holds: &holds, via: None }, Decision { approve: true, slot: None, reason: None, note: None, at: 1 }).unwrap();
        assert_eq!((yours(&w, "ana"), yours(&w, "ben")), (vec![], vec![1]));

        // Where the host keeps whoever deployed the version out, their party too.
        let excluded = Excluded { deployer: Some("sam".into()), parties: Arc::clone(&parties), ..Default::default() };
        let (who, kind, roles) = decider("quill");
        let holds = |role: Option<&str>| role.is_none_or(|r| roles.contains(&r)).then_some(Bound { binding: 1, access: 1 });
        let mut w = StoredWaiting { name: "payout".into(), expires_at: 100, steps: chain.into(), decisions: Vec::new(), shown: None };
        let refused = w.decide(&excluded, &Decider { who, kind, holds: &holds, via: None }, Decision { approve: true, slot: None, reason: None, note: None, at: 1 });
        assert_eq!(refused, Err((403, "quill and sam, who deployed the version this run is on, are one party (sam owns quill): on this host, someone else decides its approval \"payout\"".into())));
    }

    /// Who deployed the version a run is on, which `exclude_deployer` keeps from its approvals:
    /// the last to deploy its unit by the time it parked, a restore of it included, never who
    /// deployed another unit since.
    #[test]
    fn the_deployer_of_a_run_is_whoever_deployed_its_unit() {
        let version = |number: u32, unit: &str, at: u64, by: &str| -> wire::Version {
            serde_json::from_value(serde_json::json!({ "number": number, "unit": unit, "deployed_at": at, "by": by })).unwrap()
        };
        let versions = [version(1, "aa", 100, "ada"), version(2, "bb", 200, "cy"), version(3, "aa", 300, "bob")];
        for (unit, at, want) in [
            ("aa", 150, Some("ada")),
            ("aa", 250, Some("ada")),
            ("aa", 350, Some("bob")),
            ("bb", 250, Some("cy")),
            ("aa", 50, Some("bob")),
            ("cc", 250, None),
            ("", 250, None),
        ] {
            assert_eq!(deployer_of(&versions, unit, at).as_deref(), want, "a run on {unit:?} parked at {at}");
        }
    }
}
