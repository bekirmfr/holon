//! Rounds: a run that makes outbound requests (§4.1 of docs/PLATFORM_LAYOUT.md).
//!
//! Evaluation stays pure and total. A run is a sequence of evaluations of the same flow: in each,
//! an `http_request` whose request is ready asks for its answer (core's `Observer::answer`); the
//! ones with no answer yet are collected, and between rounds the host performs them, through a
//! [`Performer`], and evaluates again with their answers. A round that asks for nothing new is
//! the last: its outputs are the run's, and only its effects are performed. Earlier rounds'
//! outputs and effects were computed with answers missing and are discarded.
//!
//! The run is bounded by its [`Limits`]: an outbound-request budget (which also bounds the
//! rounds, since every round after the first needs a request), a wall-clock budget, checked
//! between rounds, and a budget of steps each round may take (`max_steps`: every node evaluated,
//! every item a function is applied to, `Observer::step`), which bounds what a round computes
//! whatever its graph, a budget of fuel its js and module calls burn in all (`max_fuel`,
//! `Observer::take_fuel`), and a budget of the memory it comes to hold (`max_memory`: the bytes
//! its thread made since the round began and keeps, `holon_core::memory`, looked at every step).
//! Past any, the run fails and nothing is performed. A single request that breaks a limit does
//! not fail the run: its node gets an error response and the flow decides.
//!
//! Every exchange (the node, its request, its response, and when) is recorded per round, so a run
//! can be replayed offline by answering from the record instead of the network.
//!
//! **Durable runs** (§4.3): an `await` node is a waiting point, answered from the events the run
//! has been delivered ([`Waits`]). One still unanswered when the run settles means the run is
//! waiting: it performs no effects, and the host parks it ([`RunOutcome::waiting`]). Resuming
//! it is running it again from the start with the same inputs, its earlier requests answered from
//! its record (`prior`), and the new event delivered.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use holon_core::{
    eval_observed, hash_value, memory, node_path, Answer, Budgeted as Budget, Capability, Hash, Observer, Resolver, Slot, Unit, Value, DEFAULT_MAX_FUEL, DEFAULT_MAX_MEMORY, DEFAULT_MAX_STEPS,
    MAX_FUEL_CEILING, MAX_MEMORY_CEILING, MAX_STEPS_CEILING,
};

use crate::{perform_effects, Host, SourcedResolver};

/// The numbers a run is held to (§4.2; the host grants them, §4.2.1). Defaults follow what the
/// systems a flow talks to already expect.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    /// The whole run, every round and request included: read as a round steps, so a round that
    /// only computes stops at it as one that sends requests does.
    pub run_budget: Duration,
    /// Outbound requests per run, over all rounds.
    pub max_requests: usize,
    /// Requests in flight at once to one host, within a round.
    pub per_host_concurrency: usize,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
    pub max_header_bytes: usize,
    pub max_redirects: u32,
    /// How long a run may wait, in all, for keys other runs hold (claims, `Waits::claim`).
    pub claim_wait: Duration,
    /// How long a run's commit may wait for the commit locks on the shared resources it read or
    /// writes, which other runs' commits hold for milliseconds.
    pub commit_wait: Duration,
    /// Steps each round may take (`Observer::step`): each node, each item a function is applied
    /// to, and a step for each 32 items or 128 bytes a leaf reads or makes. Counted, so the same
    /// round takes the same number wherever it runs, and a round past it fails the run however
    /// fast the machine.
    pub max_steps: u64,
    /// Fuel (instructions) each round's js and module calls may burn in all
    /// (`Observer::take_fuel`): each call runs on its own budget or what is left of this, the
    /// less, so the same round burns the same wherever it runs, and a round that wants more fails
    /// the run.
    pub max_fuel: u64,
    /// Bytes each round may come to hold (`holon_core::memory::held`, checked at every step):
    /// what it made and kept, its values and what is recorded of them. Not counted the same on
    /// every build, so a round past it fails the run where it is, and a check that runs it again
    /// can't tell how it would have ended.
    pub max_memory: u64,
}

impl Limits {
    /// For a run someone is waiting on (a request on a server's port): it must answer before
    /// its caller gives up.
    pub fn request_run() -> Limits {
        Limits { run_budget: Duration::from_secs(25), ..Limits::clock_run() }
    }

    /// For a run nobody is waiting on (a clock tick, a trigger).
    pub fn clock_run() -> Limits {
        Limits {
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            run_budget: Duration::from_secs(60),
            max_requests: 50,
            per_host_concurrency: 6,
            max_request_bytes: 10 * 1024 * 1024,
            max_response_bytes: 10 * 1024 * 1024,
            max_header_bytes: 64 * 1024,
            max_redirects: 10,
            claim_wait: Duration::from_secs(10),
            commit_wait: Duration::from_secs(2),
            max_steps: DEFAULT_MAX_STEPS,
            max_fuel: DEFAULT_MAX_FUEL,
            max_memory: DEFAULT_MAX_MEMORY,
        }
    }

    /// What no grant can exceed.
    pub fn ceilings() -> Limits {
        Limits {
            connect_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(300),
            run_budget: Duration::from_secs(900),
            max_requests: 1000,
            per_host_concurrency: 32,
            max_request_bytes: 100 * 1024 * 1024,
            max_response_bytes: 100 * 1024 * 1024,
            max_header_bytes: 256 * 1024,
            max_redirects: 20,
            claim_wait: Duration::from_secs(60),
            commit_wait: Duration::from_secs(30),
            max_steps: MAX_STEPS_CEILING,
            max_fuel: MAX_FUEL_CEILING,
            max_memory: MAX_MEMORY_CEILING,
        }
    }
}

/// Performs one outbound request. `request` is the record core built (`{method, url, headers,
/// body}`); the result is the node's response: `{status, headers, body}`, or, when the request
/// broke a rule or a limit or failed, `{status: 0, error: "..."}`. Never panics on a bad request:
/// every outcome is a response. `&self` and `Sync`, so a round's requests can go out at once.
pub trait Performer: Sync {
    fn perform(&self, request: &Value, limits: &Limits) -> Value;

    /// The same, told the path of the node that asks (in the run's root unit), for a performer
    /// that answers by what the node was fed. It answers as `perform` does unless it says otherwise.
    fn perform_at(&self, _path: &[u32], request: &Value, limits: &Limits) -> Value {
        self.perform(request, limits)
    }
}

/// One request a run made, and what came back.
#[derive(Clone, Debug, PartialEq)]
pub struct Exchange {
    /// The `http_request` node's path in the run's root unit.
    pub path: Vec<u32>,
    pub request: Value,
    pub response: Value,
    /// When it was sent and answered, in microseconds since the run began.
    pub start_us: f64,
    pub end_us: f64,
    /// What the request was told apart by as it was sent ([`Keys`]), with the values that try was
    /// given: kept with the exchange, so a later try or part asks by it whatever became of those
    /// values since (a secret set again, a reference read again, a version let go of). None for
    /// one kept without it: its key is then taken from its request as it stands.
    pub key: Option<Hash>,
}

/// What tells a run's requests apart, so a later round, try or part of the run is answered from
/// the record for a request it already sent, and not sent again (R113). The whole request, as
/// [`Whole`] has it, but for values a run is given that may change between its tries or parts
/// without making it another request: a secret's, written as its name ([`Keys::key`]). Two
/// requests that differ only in such a value have one key, and nothing else of the value is in it.
pub trait Keys: Sync {
    fn key(&self, request: &Value) -> Hash;
}

/// Requests told apart whole: what a run given no secret asks by.
pub struct Whole;

impl Keys for Whole {
    fn key(&self, request: &Value) -> Hash {
        request_key(request)
    }
}

/// The requests performed after one round, to answer the next.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Round {
    pub exchanges: Vec<Exchange>,
}

/// Why a run failed before it could settle.
#[derive(Clone, Debug, PartialEq)]
pub enum RunError {
    /// It needed more outbound requests than its limit.
    RequestBudget { max: usize },
    /// It ran past its wall-clock budget.
    TimeBudget { budget: Duration },
    /// What it read changed before it could claim it: the host runs it again from the new state,
    /// nothing having been sent (`Waits::claim`).
    Overtaken,
    /// A key it read is held by another run for longer than it may wait.
    Busy { key: String },
    /// A round took more steps than its limit (`max_steps`).
    StepBudget { max: u64 },
    /// A round's js and module calls wanted more fuel than its limit (`max_fuel`).
    FuelBudget { max: u64 },
    /// A round came to hold more memory than its limit (`max_memory`).
    MemoryBudget { max: u64 },
    /// The server came to hold more than it may in all (`--max-memory`,
    /// [`memory::set_process_cap`](holon_core::memory::set_process_cap)) as a round ran: every
    /// run's together, so the run fails, as for its own budget, and the server goes on.
    ServerMemory { cap: u64 },
    /// A try after the first would send a call its earlier tries' record doesn't answer, from a
    /// node one of them called out from (its path, item numbers out: [`node_path`]): held back,
    /// not sent a second time with other contents, and the run fails, keeping nothing (R115).
    HeldBack { node: Vec<u32> },
}

impl RunError {
    pub fn message(&self) -> String {
        match self {
            RunError::RequestBudget { max } => format!("the run needed more than {max} outbound requests"),
            RunError::TimeBudget { budget } => format!("the run took longer than {budget:?}"),
            RunError::Overtaken => "what the run read changed before it could call out; it ran again".to_string(),
            RunError::Busy { key } => format!("busy: {key} is held by another run"),
            RunError::StepBudget { max } => format!("{STEP_BUDGET}{max} steps in one round (max_steps)"),
            RunError::FuelBudget { max } => format!("{STEP_BUDGET}{max} instructions of js and module code in one round (max_fuel)"),
            RunError::MemoryBudget { max } => format!("{STEP_BUDGET}{max} bytes of memory in one round (max_memory)"),
            RunError::ServerMemory { cap } => {
                format!("{STEP_BUDGET}its share of the server's memory: the server held more than the {cap} bytes it may (--max-memory)")
            }
            RunError::HeldBack { node } => format!(
                "{HELD_BACK}: the run ran again because something it read changed while it ran, and this try would call out from step {node:?} with a request no earlier try sent, where an earlier try had already called out; a call isn't sent twice with other contents, so the run fails and keeps nothing. A step that calls out for each item of a list is one step: a list that changed between the tries, even by growing at its end, fails the run too"
            ),
        }
    }
}

/// What the error of a run whose round took more steps, more of its modules' fuel or more memory
/// than its limit starts with, or that ran as the server held more memory than it may.
pub const STEP_BUDGET: &str = "the run took more than ";

/// What the error of a run that held back a call in a try after its first starts with
/// ([`RunError::HeldBack`]).
pub const HELD_BACK: &str = "a call held back";

/// What a run produced: its outputs (all absent if it failed), the requests each round made,
/// why it failed, if it did, and the waiting points it is waiting at, if any. Effects were
/// performed only if it neither failed nor waits.
#[derive(Debug)]
pub struct RunOutcome {
    pub outputs: Vec<Option<Value>>,
    pub rounds: Vec<Round>,
    pub error: Option<RunError>,
    pub waiting: Vec<Waiting>,
}

/// A waiting point a run is waiting at: an `await` with no event yet, or an `approval` with no
/// decision.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub name: String,
    /// How long it waits, in seconds.
    pub expires: u64,
    /// An approval's steps, the canonical text (`person*2`, `holon_core::approval::Steps`): who
    /// decides it. None for an event, which a token delivers.
    pub approval: Option<String>,
    /// What an approval shows those who decide it, when its `shows` is wired: as the run computed
    /// it, for the host to keep out what a secret made.
    pub shows: Option<Value>,
    /// The node that waits there, for what fed its `shows` (`holon_core::taint`).
    pub path: Vec<u32>,
}

/// A host's answer when a run is about to send its first outbound requests (`Waits::claim`,
/// `docs/L4s/key-claims-L4.md`): the keys it read that it may write are now held for it until it
/// commits, or what it read changed under it and it must run again (nothing having been sent), or
/// a key is held by another run for longer than it may wait.
#[derive(Clone, Debug, PartialEq)]
pub enum Claim {
    Held,
    Overtaken,
    Busy { key: String },
}

/// What a durable run has been delivered, and its tokens.
pub trait Waits {
    /// The event delivered to waiting point `name`, if one has been.
    fn event(&self, name: &str) -> Option<Value>;
    /// The event delivered to the waiting point `at` as the run reaches it: by default, the one
    /// delivered to its name (`event`). A host that holds what was delivered to what waits there
    /// (`holon verify`: an approval's decisions to the steps it asks for) sees the point whole.
    fn event_at(&mut self, at: &Waiting) -> Option<Value> {
        self.event(&at.name)
    }
    /// The token that resumes waiting point `name`: the same every time this run asks.
    fn token(&mut self, name: &str) -> String;
    /// The answer to a collection read (`{collection, get}` or `{collection, list}`), from the
    /// collection as the run found it, or a folder read (`{folder, read}` or `{folder, list}`),
    /// from the folder as the run found it, or a database query (`{database, query, params, one}`);
    /// absent for a missing item or file, or with none of these
    /// (the default). A host keeps what it answered, to check the run against at commit and replay.
    fn read(&mut self, _path: &[u32], _request: &Value) -> Option<Value> {
        None
    }
    /// Called before a round's requests are sent: the host claims, for this run, every key it has
    /// read that it may write, re-checking those reads against the live state first. The default
    /// holds nothing and checks nothing (a run with no state, or a host without claims).
    fn claim(&mut self) -> Claim {
        Claim::Held
    }
}

/// No events and no tokens (an empty token): a run that can't be resumed.
pub struct NoWaits;
impl Waits for NoWaits {
    fn event(&self, _: &str) -> Option<Value> {
        None
    }
    fn token(&mut self, _: &str) -> String {
        String::new()
    }
}

/// Run `root` on `inputs`, in rounds, sourcing `sourced` (clock, state, …) as `fire_sourced`
/// does, each value lent from wherever the caller holds it (a run's state) and handed to a read
/// as it is: data (`is_storable`), as a state's cells and the drivers the host makes are, and as
/// the caller checks one it reads back from storage. Outbound requests are performed with
/// `performer` within `limits`, and the final round's effects with `host`, unless the run waits. `observer` sees every round's steps (after a
/// `round(n)` call) as in a single evaluation. `waits` answers waiting points and tokens;
/// `prior` holds the rounds of an earlier part of the same run (a resumed one), and of its earlier
/// tries, whose requests are answered from it, not sent again: each asked by its node and the
/// request as it was sent, or else its [`Keys`] key, the one it was sent with where it kept one.
/// `tried` are the rounds among `prior` that are this part's earlier tries (none for a first
/// try): a call they don't answer, asked from a node one of them sent a call from (its path, item
/// numbers out, [`node_path`]), is held back, and the run fails, nothing more sent
/// ([`RunError::HeldBack`]). A node they never called out from sends as any does.
/// The outcome's rounds are the new ones, each exchange with the key it was sent with.
#[allow(clippy::too_many_arguments)]
pub fn run_rounds(
    root: &Unit,
    inputs: &[Option<Value>],
    sourced: &[(Capability, &Value)],
    real: &impl Resolver,
    host: &mut impl Host,
    performer: &impl Performer,
    limits: &Limits,
    observer: &mut impl Observer,
    waits: &mut impl Waits,
    prior: &[Round],
    tried: &[Round],
    keys: &dyn Keys,
) -> RunOutcome {
    let started = Instant::now();
    // The run's time runs out at its budget's end, inside a round as between them: a round that
    // computes and sends nothing reads the clock as it steps ([`Asking::late`]).
    let deadline = started.checked_add(limits.run_budget);
    let resolver = SourcedResolver::lent(sourced.iter().map(|(cap, value)| (cap, *value)), real);
    // Each exchange answers its request as it was sent, byte for byte, and by its key: the same
    // request but for its secrets' values. A text that was a secret's value when it was sent and
    // is an older one now makes another key, but not another request (R113). Two maps: a
    // request's own hash is never asked among keys, nor a key among requests.
    let mut answers: BTreeMap<(Vec<u32>, Hash), Value> = BTreeMap::new();
    let mut as_sent: BTreeMap<(Vec<u32>, Hash), Value> = BTreeMap::new();
    for x in prior.iter().flat_map(|r| &r.exchanges) {
        answers.insert((x.path.clone(), x.key.unwrap_or_else(|| keys.key(&x.request))), x.response.clone());
        as_sent.insert((x.path.clone(), request_key(&x.request)), x.response.clone());
    }
    // The nodes an earlier try of this part called out from: one asked again with a request
    // the record doesn't answer would be a second call, with other contents (R115).
    let called: std::collections::BTreeSet<Vec<u32>> = tried.iter().flat_map(|r| &r.exchanges).map(|x| node_path(root, real, &x.path)).collect();
    let mut rounds: Vec<Round> = Vec::new();
    let mut sent = 0usize;
    let fail = |rounds: Vec<Round>, error: RunError| RunOutcome {
        outputs: vec![None; root.outputs.len()],
        rounds,
        error: Some(error),
        waiting: Vec::new(),
    };
    loop {
        observer.round(rounds.len());
        let mut asking = Asking {
            inner: &mut *observer,
            answers: &answers,
            as_sent: &as_sent,
            keys,
            pending: Vec::new(),
            waits: &mut *waits,
            waiting: Vec::new(),
            budget: Budget::new(limits.max_steps).with_fuel(limits.max_fuel).with_memory(limits.max_memory),
            deadline,
            late: false,
            clock_in: CLOCK_EVERY,
        };
        // The round's thread is held to its budget, and the process to its cap, by a leaf that
        // makes many values in a step as by the steps ([`memory::room`]).
        let hold = memory::Hold::new(asking.budget.at_most);
        let outputs = eval_observed(root, inputs, &resolver, &mut asking).into_vec();
        let stopped = memory::stopped();
        drop(hold);
        if asking.late {
            return fail(rounds, RunError::TimeBudget { budget: limits.run_budget });
        }
        if asking.budget.exhausted {
            return fail(rounds, RunError::StepBudget { max: limits.max_steps });
        }
        if asking.budget.out_of_fuel {
            return fail(rounds, RunError::FuelBudget { max: limits.max_fuel });
        }
        if stopped == Some(memory::Over::Process) {
            return fail(rounds, RunError::ServerMemory { cap: memory::process_cap() });
        }
        // A leaf stopped for the round's own memory as the round's last node: no step came after
        // it to say so.
        if asking.budget.out_of_memory || stopped == Some(memory::Over::Round) {
            return fail(rounds, RunError::MemoryBudget { max: limits.max_memory });
        }
        let (pending, waiting) = (asking.pending, asking.waiting);
        if pending.is_empty() {
            // A run still waiting performs nothing yet: what it does is decided when it finishes.
            if waiting.is_empty() {
                for output in outputs.iter().flatten() {
                    perform_effects(output, host);
                }
            }
            return RunOutcome { outputs, rounds, error: None, waiting };
        }
        if !called.is_empty()
            && let Some(node) = pending.iter().map(|(path, _)| node_path(root, real, path)).find(|node| called.contains(node))
        {
            return fail(rounds, RunError::HeldBack { node });
        }
        sent += pending.len();
        if sent > limits.max_requests {
            return fail(rounds, RunError::RequestBudget { max: limits.max_requests });
        }
        let Some(left) = limits.run_budget.checked_sub(started.elapsed()).filter(|d| !d.is_zero()) else {
            return fail(rounds, RunError::TimeBudget { budget: limits.run_budget });
        };
        // Before anything is sent, the keys this run read and may write are its until it
        // commits; a read that changed meanwhile ends the attempt with nothing sent.
        match waits.claim() {
            Claim::Held => {}
            Claim::Overtaken => return fail(rounds, RunError::Overtaken),
            Claim::Busy { key } => return fail(rounds, RunError::Busy { key }),
        }
        // No request may outlast the run.
        let this_round = Limits { request_timeout: limits.request_timeout.min(left), ..limits.clone() };
        let mut exchanges = perform_all(pending, performer, &this_round, started);
        // Each keyed as it was asked, with the values this try was given, and kept so.
        for ex in &mut exchanges {
            let key = keys.key(&ex.request);
            ex.key = Some(key);
            answers.insert((ex.path.clone(), key), ex.response.clone());
            as_sent.insert((ex.path.clone(), request_key(&ex.request)), ex.response.clone());
        }
        rounds.push(Round { exchanges });
        if started.elapsed() > limits.run_budget {
            return fail(rounds, RunError::TimeBudget { budget: limits.run_budget });
        }
    }
}

/// Answers a run's requests from a record of its rounds instead of the network: how a logged run
/// is replayed. A request the record doesn't have is absent: nothing more will come.
pub struct Recorded {
    answers: BTreeMap<(Vec<u32>, Hash), Value>,
}

impl Recorded {
    pub fn new(rounds: &[Round]) -> Recorded {
        let answers = rounds
            .iter()
            .flat_map(|r| &r.exchanges)
            .map(|ex| ((ex.path.clone(), request_key(&ex.request)), ex.response.clone()))
            .collect();
        Recorded { answers }
    }
}

impl Observer for Recorded {
    fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
        self.answers.get(&(path.to_vec(), request_key(request))).cloned().into()
    }
}

/// A request's identity within a run: its content hash. A request too large to encode can't be
/// answered from a record, so it gets a key no record has.
fn request_key(request: &Value) -> Hash {
    hash_value(request).unwrap_or([0xff; 32])
}

/// The observer a round runs under: answers from earlier rounds, collects what's still
/// unanswered, answers waiting points and tokens from `waits`, and passes timing through to the
/// caller's observer.
struct Asking<'a, O: Observer, W: Waits> {
    inner: &'a mut O,
    answers: &'a BTreeMap<(Vec<u32>, Hash), Value>,
    /// The same answers, each by its request as it was sent.
    as_sent: &'a BTreeMap<(Vec<u32>, Hash), Value>,
    /// What a request is asked by among `answers`.
    keys: &'a dyn Keys,
    pending: Vec<(Vec<u32>, Value)>,
    waits: &'a mut W,
    waiting: Vec<Waiting>,
    /// The round's steps, its modules' fuel and the memory it holds, counted afresh each round.
    budget: Budget,
    /// When the run's time is up ([`Limits::run_budget`] from its start): none when it never is.
    deadline: Option<Instant>,
    /// Whether a step found the run's time up: that step, and every one after it, was refused,
    /// and the run fails for its time.
    late: bool,
    /// Steps until the clock is read again ([`CLOCK_EVERY`]).
    clock_in: u32,
}

/// The steps a round takes one at a time between readings of the clock. Each is a node of the
/// round (a leaf's work is steps of its own, taken a batch at a time, and the clock is read at
/// each batch, [`Asking::take_steps`]), so the time past a run's budget is a few of these at
/// most: a hundred microseconds or so, against a reading of the clock costing some tens of
/// nanoseconds.
const CLOCK_EVERY: u32 = 1024;

impl<O: Observer, W: Waits> Asking<'_, O, W> {
    /// Whether the run's time is up: read from the clock, and once it is, for good. A round
    /// that only computes reads it as it steps, so `run_budget` bounds it as it bounds a round
    /// that sends requests, whatever its steps ([`Limits::max_steps`] is a count, the same
    /// wherever the round runs; this is the time it takes here).
    fn late(&mut self) -> bool {
        if !self.late && self.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            self.late = true;
        }
        self.late
    }
}

/// A text field of a request record.
fn text_field<'v>(request: &'v Value, key: &str) -> Option<&'v str> {
    match request {
        Value::Record(r) => match r.get(key) {
            Some(Value::Text(t)) => Some(t),
            _ => None,
        },
        _ => None,
    }
}

impl<O: Observer, W: Waits> Observer for Asking<'_, O, W> {
    fn step(&mut self) -> bool {
        self.clock_in = match self.clock_in.checked_sub(1) {
            Some(left) if !self.late => left,
            _ if self.late() => return false,
            _ => {
                // With the clock, the process's memory: past its cap, the budget refuses this
                // step and every one after it (`Budgeted` reads `memory::stopped`).
                memory::room();
                CLOCK_EVERY
            }
        };
        self.budget.step()
    }
    fn take_steps(&mut self, want: u32) -> u32 {
        if want > 0 && self.late() {
            return 0;
        }
        memory::room();
        self.budget.take_steps(want)
    }
    fn give_back(&mut self, steps: u32) {
        self.budget.give_back(steps);
    }
    fn take_fuel(&mut self, want: u64) -> u64 {
        self.budget.take_fuel(want)
    }
    fn give_back_fuel(&mut self, unburned: u64, short: bool) {
        self.budget.give_back_fuel(unburned, short);
    }
    fn held_at_most(&self) -> i64 {
        self.budget.held_at_most()
    }
    fn enter(&mut self, path: &[u32]) {
        self.inner.enter(path);
    }
    fn leave(&mut self, path: &[u32]) {
        self.inner.leave(path);
    }
    fn produced(&mut self, path: &[u32], outputs: &[Slot]) {
        self.inner.produced(path, outputs);
    }
    fn failed(&mut self, path: &[u32], reason: &str) {
        self.inner.failed(path, reason);
    }
    fn began_absent(&mut self) {
        self.inner.began_absent();
    }
    fn asserted(&mut self, path: &[u32], unit: &Unit, inputs: &[Option<&Value>]) -> Option<Vec<Option<Value>>> {
        self.inner.asserted(path, unit, inputs)
    }
    fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
        if let Some(name) = text_field(request, "await") {
            let number = |key: &str| match request {
                Value::Record(r) => match r.get(key) {
                    Some(Value::Number(n)) => Some(*n as u64),
                    _ => None,
                },
                _ => None,
            };
            let expires = number("expires").unwrap_or(holon_core::MAX_AWAIT_SECONDS);
            let shows = match request {
                Value::Record(r) => r.get("shows").cloned(),
                _ => None,
            };
            let at = Waiting { name: name.to_string(), expires, approval: text_field(request, "steps").map(str::to_string), shows, path: path.to_vec() };
            let event = self.waits.event_at(&at);
            if event.is_none() && !self.waiting.iter().any(|w| w.name == name) {
                self.waiting.push(at);
            }
            // An event not yet delivered is not yet: what waits on it waits too.
            return event.map_or(Answer::NotYet, Answer::Value);
        }
        if let Some(name) = text_field(request, "resume_token") {
            return Answer::Value(Value::Text(self.waits.token(name)));
        }
        // A collection, folder or database read: answered at once, from what the run found.
        if ["collection", "folder", "database"].iter().any(|k| text_field(request, k).is_some()) {
            return self.waits.read(path, request).into();
        }
        // The request it sent, byte for byte; else the same but for its secrets' values.
        let answer = self.as_sent.get(&(path.to_vec(), request_key(request))).or_else(|| self.answers.get(&(path.to_vec(), self.keys.key(request))));
        if let Some(answer) = answer {
            return Answer::Value(answer.clone());
        }
        if !self.pending.iter().any(|(p, r)| p == path && r == request) {
            self.pending.push((path.to_vec(), request.clone()));
        }
        // Performed before the next round, and answered in it.
        Answer::NotYet
    }
}

/// Perform a round's requests: hosts in parallel, at most `per_host_concurrency` at once to each.
/// A call to another flow's action (`{flow, action, input}`) is grouped by the flow, and made one
/// at a time, in the order of the nodes that ask: it changes things there, so two calls to one
/// flow don't race.
fn perform_all(
    pending: Vec<(Vec<u32>, Value)>,
    performer: &impl Performer,
    limits: &Limits,
    started: Instant,
) -> Vec<Exchange> {
    let mut by_host: BTreeMap<String, Vec<(Vec<u32>, Value)>> = BTreeMap::new();
    for (path, request) in pending {
        by_host.entry(host_of(&request)).or_default().push((path, request));
    }
    let us = |t: Instant| t.duration_since(started).as_secs_f64() * 1e6;
    let mut exchanges: Vec<Exchange> = std::thread::scope(|scope| {
        let per_host: Vec<_> = by_host
            .into_iter()
            .map(|(host, mut requests)| {
                let one_at_a_time = host.starts_with(FLOW_GROUP);
                if one_at_a_time {
                    requests.sort_by(|a, b| a.0.cmp(&b.0));
                }
                scope.spawn(move || {
                    let mut done = Vec::new();
                    let width = if one_at_a_time { 1 } else { limits.per_host_concurrency.max(1) };
                    for batch in requests.chunks(width) {
                        std::thread::scope(|inner| {
                            let handles: Vec<_> = batch
                                .iter()
                                .map(|(path, request)| {
                                    inner.spawn(move || {
                                        let t0 = Instant::now();
                                        let response = performer.perform_at(path, request, limits);
                                        Exchange {
                                            path: path.clone(),
                                            request: request.clone(),
                                            response,
                                            start_us: us(t0),
                                            end_us: us(Instant::now()),
                                            key: None,
                                        }
                                    })
                                })
                                .collect();
                            done.extend(handles.into_iter().map(|h| h.join().expect("a performer panicked")));
                        });
                    }
                    done
                })
            })
            .collect();
        per_host.into_iter().flat_map(|h| h.join().expect("a performer panicked")).collect()
    });
    // A stable order for the record, whatever order the threads finished in.
    exchanges.sort_by(|a, b| a.path.cmp(&b.path));
    exchanges
}

/// What a group of calls to one flow's actions is called, beside the hosts the others are grouped by.
const FLOW_GROUP: &str = "flow:";

/// The `host[:port]` of a request's URL, for grouping; `flow:<name>` for a call to a flow.
fn host_of(request: &Value) -> String {
    if let Value::Record(r) = request
        && let (Some(Value::Text(flow)), Some(Value::Text(_))) = (r.get("flow"), r.get("action"))
    {
        return format!("{FLOW_GROUP}{flow}");
    }
    let url = match request {
        Value::Record(r) => match r.get("url") {
            Some(Value::Text(u)) => u.as_str(),
            _ => "",
        },
        _ => "",
    };
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_core::{Edge, MapResolver, Param, Port, Type, UnitBody, BOUNDARY};
    use std::sync::Mutex;

    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn record(pairs: &[(&str, Value)]) -> Value {
        Value::Record(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }
    fn leaf(name: &str, params: Vec<Param>, n_in: usize, out: Type) -> Unit {
        Unit {
            body: UnitBody::Primitive { name: name.into() },
            params,
            inputs: (0..n_in).map(|i| Port { name: format!("i{i}"), ty: Type::Any }).collect(),
            outputs: vec![Port { name: "out".into(), ty: out }],
        }
    }
    fn e(fc: u32, fp: u32, tc: u32, tp: u32) -> Edge {
        Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp }
    }

    /// Answers every request with `{status: 200, body: <the path it was sent to>}`, and
    /// remembers what it was asked.
    #[derive(Default)]
    struct Echo(Mutex<Vec<String>>);
    impl Performer for Echo {
        fn perform(&self, request: &Value, _: &Limits) -> Value {
            let Value::Record(r) = request else { panic!() };
            let Value::Text(url) = &r["url"] else { panic!() };
            self.0.lock().unwrap().push(url.clone());
            let path = url.trim_start_matches("https://api.example.com");
            record(&[("status", Value::Number(200.0)), ("body", txt(path))])
        }
    }

    #[derive(Default)]
    struct Performed(Vec<(Capability, Value)>);
    impl Host for Performed {
        fn perform(&mut self, capability: Capability, payload: &Value) {
            self.0.push((capability, payload.clone()));
        }
    }

    /// `/first` is requested; its answer's body (`/first`) plus `/next` is the path of a second
    /// request, which depends on the first; the second's body goes to `egress`.
    fn chained(r: &mut MapResolver) -> Unit {
        let request = || {
            leaf("http_request", vec![Param::frozen("method", "GET"), Param::frozen("host", "api.example.com")], 3, Type::Record)
        };
        let constant = |v: &str| leaf("const", vec![Param::frozen("value", v), Param::frozen("type", "Text")], 0, Type::Text);
        let body = leaf("field", vec![Param::frozen("name", "body")], 1, Type::Any);
        let children = vec![
            r.insert(constant("/first")).unwrap(), // 0
            r.insert(request()).unwrap(),          // 1
            r.insert(body.clone()).unwrap(),       // 2
            r.insert(constant("/next")).unwrap(),  // 3
            r.insert(leaf("concat", vec![], 2, Type::Text)).unwrap(), // 4
            r.insert(request()).unwrap(),          // 5
            r.insert(body).unwrap(),               // 6
            r.insert(leaf("egress", vec![], 1, Type::Effect)).unwrap(), // 7
        ];
        Unit {
            body: UnitBody::Composite {
                children,
                edges: vec![
                    e(0, 0, 1, 0),
                    e(1, 0, 2, 0),
                    e(2, 0, 4, 0),
                    e(3, 0, 4, 1),
                    e(4, 0, 5, 0),
                    e(5, 0, 6, 0),
                    e(6, 0, 7, 0),
                    e(7, 0, BOUNDARY, 0),
                ],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        }
    }

    #[test]
    fn dependent_requests_take_a_round_each_and_only_the_last_round_performs() {
        let mut r = MapResolver::new();
        let flow = chained(&mut r);
        let (echo, mut host) = (Echo::default(), Performed::default());
        let out = run_rounds(&flow, &[], &[], &r, &mut host, &echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
        assert_eq!(out.error, None);
        assert_eq!(*echo.0.lock().unwrap(), vec!["https://api.example.com/first", "https://api.example.com/first/next"]);
        assert_eq!(out.rounds.len(), 2, "one request per round: the second needed the first's answer");
        assert_eq!(host.0, vec![(Capability::NetworkEgress, txt("/first/next"))], "performed once, from the last round");
        assert!(matches!(&out.outputs[0], Some(Value::Effect { .. })));

        // Replaying from the record answers the same, offline.
        let mut recorded = Recorded::new(&out.rounds);
        let replayed = eval_observed(&flow, &[], &r, &mut recorded).into_vec();
        assert_eq!(replayed, out.outputs);
    }

    /// `resume_token → egress` and `await → egress`: the first run hands out a token, waits, and
    /// performs nothing; resuming it with the event (and the first part's record) finishes it,
    /// and the token is the same both times.
    #[test]
    fn a_run_waits_performing_nothing_and_finishes_when_its_event_arrives() {
        #[derive(Default)]
        struct Delivered {
            events: BTreeMap<String, Value>,
            minted: usize,
        }
        impl Waits for Delivered {
            fn event(&self, name: &str) -> Option<Value> {
                self.events.get(name).cloned()
            }
            fn token(&mut self, name: &str) -> String {
                self.minted += 1;
                format!("tok-{name}")
            }
        }
        let mut r = MapResolver::new();
        let wait = leaf("await", vec![Param::frozen("name", "paid"), Param::frozen("expires", "60")], 0, Type::Any);
        let token = leaf("resume_token", vec![Param::frozen("await", "paid")], 0, Type::Text);
        let egress = leaf("egress", vec![], 1, Type::Effect);
        let flow = Unit {
            body: UnitBody::Composite {
                children: vec![r.insert(token).unwrap(), r.insert(egress.clone()).unwrap(), r.insert(wait).unwrap(), r.insert(egress).unwrap()],
                edges: vec![e(0, 0, 1, 0), e(2, 0, 3, 0), e(1, 0, BOUNDARY, 0), e(3, 0, BOUNDARY, 1)],
            },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "link".into(), ty: Type::Effect }, Port { name: "done".into(), ty: Type::Effect }],
        };
        let (echo, mut host) = (Echo::default(), Performed::default());
        let mut waits = Delivered::default();
        let first = run_rounds(&flow, &[], &[], &r, &mut host, &echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut waits, &[], &[], &Whole);
        assert_eq!(first.waiting.iter().map(|w| (w.name.as_str(), w.expires, w.approval.as_deref(), w.shows.as_ref())).collect::<Vec<_>>(), vec![("paid", 60, None, None)]);
        assert!(host.0.is_empty(), "a waiting run performs nothing");
        assert!(matches!(&first.outputs[0], Some(Value::Effect { payload, .. }) if **payload == txt("tok-paid")), "its interim output is there");

        waits.events.insert("paid".into(), txt("ok"));
        let resumed = run_rounds(&flow, &[], &[], &r, &mut host, &echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut waits, &first.rounds, &[], &Whole);
        assert!(resumed.waiting.is_empty());
        assert_eq!(host.0, vec![(Capability::NetworkEgress, txt("tok-paid")), (Capability::NetworkEgress, txt("ok"))]);
    }

    /// Guards: a request a run sent before it waited, asked again as the run goes on after the
    /// secret the request carries was set to another value, is answered from the run's record and
    /// not sent a second time (R113); the same holds for a run tried again, whose earlier tries'
    /// rounds are its `prior` too. Two requests that differ only in the secret's value have one
    /// key; any other difference makes another. One asked again the same byte for byte is
    /// answered too, whatever its key now says.
    /// Why: a resumed run is evaluated again from its start, and its earlier requests answered
    /// from its record by their key. Keyed by the whole request, the one built with the new value
    /// matched nothing, and the authorization it stood for went out twice. Keyed only by name, a
    /// text that was the secret's value and is an older one now made the same request another.
    /// If it fails: `run_rounds` asks `prior` by the whole request and not by the key each
    /// exchange was sent with (`Exchange::key`), or `Asking::answer` doesn't ask by `keys`; or,
    /// at the same bytes sent again, it asks only by the key, not by the request as it was sent.
    #[test]
    fn a_request_asked_again_with_another_value_of_its_secret_is_answered_from_the_record() {
        /// Keys that write the one secret's value, as this try was given it, as its name.
        struct ByName(&'static str, &'static str);
        fn named(v: &Value, value: &str, name: &str) -> Value {
            match v {
                Value::Text(t) => Value::Text(t.replace(value, &format!("\u{e000}{name}"))),
                Value::Record(r) => Value::Record(r.iter().map(|(k, v)| (k.clone(), named(v, value, name))).collect()),
                Value::List(l) => Value::List(l.iter().map(|v| named(v, value, name)).collect()),
                other => other.clone(),
            }
        }
        impl Keys for ByName {
            fn key(&self, request: &Value) -> Hash {
                request_key(&named(request, self.1, self.0))
            }
        }
        #[derive(Default)]
        struct Delivered(BTreeMap<String, Value>);
        impl Waits for Delivered {
            fn event(&self, name: &str) -> Option<Value> {
                self.0.get(name).cloned()
            }
            fn token(&mut self, name: &str) -> String {
                format!("tok-{name}")
            }
        }
        let (first_value, second_value) = ("sk_test_first_value", "sk_test_second_val");
        let mut r = MapResolver::new();
        let constant = leaf("const", vec![Param::frozen("value", "/pay/"), Param::frozen("type", "Text")], 0, Type::Text);
        let secret = leaf("secret", vec![Param::frozen("name", "k")], 0, Type::Text);
        let request = leaf("http_request", vec![Param::frozen("method", "POST"), Param::frozen("host", "api.example.com")], 3, Type::Record);
        let body = leaf("field", vec![Param::frozen("name", "body")], 1, Type::Any);
        let wait = leaf("await", vec![Param::frozen("name", "paid"), Param::frozen("expires", "60")], 0, Type::Any);
        let egress = leaf("egress", vec![], 1, Type::Effect);
        let children = vec![
            r.insert(constant).unwrap(),                                 // 0
            r.insert(secret).unwrap(),                                   // 1
            r.insert(leaf("concat", vec![], 2, Type::Text)).unwrap(),    // 2
            r.insert(request).unwrap(),                                  // 3
            r.insert(body).unwrap(),                                     // 4
            r.insert(egress.clone()).unwrap(),                           // 5
            r.insert(wait).unwrap(),                                     // 6
            r.insert(egress).unwrap(),                                   // 7
        ];
        let edges = vec![e(0, 0, 2, 0), e(1, 0, 2, 1), e(2, 0, 3, 0), e(3, 0, 4, 0), e(4, 0, 5, 0), e(5, 0, BOUNDARY, 0), e(6, 0, 7, 0), e(7, 0, BOUNDARY, 1)];
        let flow = Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs: vec![],
            outputs: vec![Port { name: "paid".into(), ty: Type::Effect }, Port { name: "done".into(), ty: Type::Effect }],
        };
        let given = |value: &'static str| (Capability::SecretRead("k".into()), txt(value));
        let limits = Limits::request_run();

        // Sent once, with the first value, and kept with the key it was sent by.
        let (echo, mut host, mut waits) = (Echo::default(), Performed::default(), Delivered::default());
        let first_given = given(first_value);
        let sourced = [(first_given.0.clone(), &first_given.1)];
        let first = run_rounds(&flow, &[], &sourced, &r, &mut host, &echo, &limits, &mut holon_core::NoObserver, &mut waits, &[], &[], &ByName("k", first_value));
        assert_eq!(first.waiting.len(), 1, "it waits");
        assert_eq!(*echo.0.lock().unwrap(), vec![format!("https://api.example.com/pay/{first_value}")]);
        assert!(first.rounds.iter().flat_map(|r| &r.exchanges).all(|x| x.key.is_some()), "each exchange keeps the key it was sent by");

        // Gone on with the secret set to another value: answered from the record, not sent again.
        waits.0.insert("paid".into(), txt("ok"));
        let second_given = given(second_value);
        let sourced = [(second_given.0.clone(), &second_given.1)];
        let resumed = run_rounds(&flow, &[], &sourced, &r, &mut host, &echo, &limits, &mut holon_core::NoObserver, &mut waits, &first.rounds, &[], &ByName("k", second_value));
        assert!(resumed.waiting.is_empty() && resumed.error.is_none(), "{:?}", resumed.error);
        assert_eq!(echo.0.lock().unwrap().len(), 1, "the request isn't sent a second time: {:?}", echo.0.lock().unwrap());
        assert!(resumed.rounds.is_empty(), "nothing new was sent");
        assert_eq!(host.0, vec![(Capability::NetworkEgress, txt(&format!("/pay/{first_value}"))), (Capability::NetworkEgress, txt("ok"))], "it goes on with what the request was answered");

        // Asked again the same byte for byte, though its key now says another request (the
        // value it holds is no longer the secret's, set anew meanwhile): answered from the record
        // all the same, by the request as it was sent.
        let same = Echo::default();
        let first_sourced = [(first_given.0.clone(), &first_given.1)];
        let again_same = run_rounds(&flow, &[], &first_sourced, &r, &mut Performed::default(), &same, &limits, &mut holon_core::NoObserver, &mut waits, &first.rounds, &[], &ByName("k", second_value));
        assert!(same.0.lock().unwrap().is_empty() && again_same.rounds.is_empty(), "the same bytes aren't sent again: {:?}", same.0.lock().unwrap());

        // Asked by the whole request instead, it is sent again: what this test is about.
        let again = Echo::default();
        run_rounds(&flow, &[], &sourced, &r, &mut Performed::default(), &again, &limits, &mut holon_core::NoObserver, &mut waits, &first.rounds, &[], &Whole);
        assert_eq!(*again.0.lock().unwrap(), vec![format!("https://api.example.com/pay/{second_value}")]);

        // One key for two requests that differ only in the secret's value; another for any other change.
        let asked = |path: String| record(&[("method", txt("POST")), ("url", txt(&format!("https://api.example.com{path}")))]);
        let key = |value: &'static str, path: String| ByName("k", value).key(&asked(path));
        assert_eq!(key(first_value, format!("/pay/{first_value}")), key(second_value, format!("/pay/{second_value}")));
        assert_ne!(key(first_value, format!("/pay/{first_value}")), key(second_value, format!("/refund/{second_value}")));
    }

    /// Guards: every node a round evaluates is a step, counted afresh each round: the least
    /// `max_steps` the two-round flow finishes with is its last round's 8 nodes, not the two
    /// rounds' sum; one fewer fails it with `StepBudget`, performing nothing.
    /// Why: a node outside any function is a step too. Composites can share a child, so a unit
    /// of few nodes can evaluate a great many; and a round only grows, so counting per round is
    /// what `holon verify`, evaluating once with every answer, counts again.
    /// If it fails: `Asking::step` (or `take_steps`) isn't the round's budget, the budget isn't
    /// made afresh each round, or `run_rounds` doesn't read `exhausted` after the round.
    #[test]
    fn every_node_a_round_evaluates_is_a_step_counted_per_round() {
        let mut r = MapResolver::new();
        let flow = chained(&mut r);
        let run = |max_steps: u64| {
            let (echo, mut host) = (Echo::default(), Performed::default());
            let limits = Limits { max_steps, ..Limits::request_run() };
            let out = run_rounds(&flow, &[], &[], &r, &mut host, &echo, &limits, &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
            (out, host.0)
        };
        let least = (1..100).find(|&m| run(m).0.error.is_none()).expect("it finishes");
        assert_eq!(least, 8, "the last round's nodes, each a step");
        let (out, performed) = run(least - 1);
        assert_eq!(out.error, Some(RunError::StepBudget { max: least - 1 }));
        assert_eq!(out.outputs, vec![None]);
        assert!(performed.is_empty(), "nothing performed");
        assert!(out.error.unwrap().message().starts_with(STEP_BUDGET));
    }

    /// `chained`, its first answer's body passed through a module on the way to the second
    /// request: the module runs in the second round and again in the third.
    fn chained_through_a_module(r: &mut MapResolver) -> Unit {
        let mut flow = chained(r);
        let module = Unit {
            body: UnitBody::Module { bytes: b"passes its input on".to_vec() },
            params: vec![Param::frozen("fuel", "100")],
            inputs: vec![Port { name: "in".into(), ty: Type::Any }],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        };
        let UnitBody::Composite { children, edges } = &mut flow.body else { unreachable!() };
        children.push(r.insert(module).unwrap()); // 8
        let i = edges.iter().position(|x| *x == e(2, 0, 4, 0)).unwrap();
        edges.splice(i..=i, [e(2, 0, 8, 0), e(8, 0, 4, 0)]);
        flow
    }

    /// Runs modules by passing their input on, burning `cost` a call (all it is given if that's
    /// less, out of fuel), and counts its calls.
    struct Burning<'a>(&'a MapResolver, u64, std::cell::Cell<usize>);
    impl Resolver for Burning<'_> {
        fn resolve(&self, hash: &Hash) -> Option<&Unit> {
            self.0.resolve(hash)
        }
        fn run_module(&self, _bytes: &[u8], limits: &holon_core::ModuleLimits, inputs: &[Option<Value>]) -> holon_core::ModuleRun {
            self.2.set(self.2.get() + 1);
            match limits.fuel < self.1 {
                true => holon_core::ModuleRun { outputs: Err("out of fuel".into()), burned: limits.fuel, out_of_fuel: true },
                false => holon_core::ModuleRun { outputs: Ok(vec![inputs[0].clone()]), burned: self.1, out_of_fuel: false },
            }
        }
    }

    /// Runs modules by passing their input on, and keeps a mebibyte of its own at each call: what
    /// a round that calls one holds.
    struct Keeping<'a>(&'a MapResolver, std::cell::RefCell<Vec<Vec<u8>>>);
    const KEEPS: usize = 1 << 20;
    impl Resolver for Keeping<'_> {
        fn resolve(&self, hash: &Hash) -> Option<&Unit> {
            self.0.resolve(hash)
        }
        fn run_module(&self, _bytes: &[u8], _limits: &holon_core::ModuleLimits, inputs: &[Option<Value>]) -> holon_core::ModuleRun {
            self.1.borrow_mut().push(vec![1; KEEPS]);
            holon_core::ModuleRun { outputs: Ok(vec![inputs[0].clone()]), burned: 0, out_of_fuel: false }
        }
    }

    /// Guards: what a round holds is held to the limits' `max_memory`, counted afresh each
    /// round: the flow whose module keeps a mebibyte in each of two rounds finishes within one
    /// and a half a round, though it holds two by its end; within half of one it fails with
    /// `MemoryBudget`, performing nothing. Within a `map`, the round stops at the item past it,
    /// not at the map's end: the module mapped over 32 items is called twice.
    /// Why: a round past its memory must fail the run, not answer from what it computed before
    /// it was stopped; and each round starts from what the rounds before it left, so counting
    /// from the run's start would fail a run whose rounds are each within it. A map takes its
    /// steps a batch at a time: looked at only between batches, a round's memory could go past
    /// its grant 256 items' worth.
    /// If it fails: `Asking`'s budget isn't held to `max_memory` (`with_memory`), isn't made
    /// afresh each round, or `run_rounds` doesn't read `out_of_memory` after the round; on the
    /// map, `Asking` doesn't say what its budget may hold (`Observer::held_at_most`), so `Asks`
    /// doesn't look at each item.
    #[test]
    fn what_a_round_holds_is_held_to_max_memory_afresh_each_round() {
        let mut r = MapResolver::new();
        let flow = chained_through_a_module(&mut r);
        // The same module, mapped over a list.
        let module = Unit { body: UnitBody::Module { bytes: b"passes its input on".to_vec() }, params: vec![Param::frozen("fuel", "100")], ..leaf("module", vec![], 1, Type::Any) };
        let module = r.insert(module).unwrap();
        let map = r.insert(Unit { params: vec![Param::frozen("fn", module.iter().map(|b| format!("{b:02x}")).collect::<String>())], ..leaf("map", vec![], 1, Type::list_any()) }).unwrap();
        let mapped = Unit {
            body: UnitBody::Composite { children: vec![map], edges: vec![e(BOUNDARY, 0, 0, 0), e(0, 0, BOUNDARY, 0)] },
            params: vec![],
            inputs: vec![Port { name: "xs".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        };
        let engine = Keeping(&r, std::cell::RefCell::new(Vec::new()));
        let run_on = |flow: &Unit, inputs: &[Option<Value>], max_memory: u64| {
            engine.1.borrow_mut().clear();
            let (echo, mut host) = (Echo::default(), Performed::default());
            let limits = Limits { max_memory, ..Limits::request_run() };
            let out = run_rounds(flow, inputs, &[], &engine, &mut host, &echo, &limits, &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
            (out, host.0)
        };
        let run = |max_memory: u64| run_on(&flow, &[], max_memory);
        let (out, performed) = run(KEEPS as u64 * 3 / 2);
        assert_eq!(out.error, None, "a round's memory is enough");
        assert_eq!(engine.1.borrow().len(), 2, "the module kept a mebibyte in each of two rounds");
        assert_eq!(performed, vec![(Capability::NetworkEgress, txt("/first/next"))]);
        let (out, performed) = run(KEEPS as u64 / 2);
        assert_eq!(out.error, Some(RunError::MemoryBudget { max: KEEPS as u64 / 2 }));
        assert_eq!(out.outputs, vec![None]);
        assert!(performed.is_empty(), "nothing performed");
        let message = out.error.unwrap().message();
        assert!(message.starts_with(STEP_BUDGET) && message.contains("max_memory"), "{message}");

        let items = Some(Value::List(vec![txt("x"); 32]));
        let (out, _) = run_on(&mapped, std::slice::from_ref(&items), KEEPS as u64 * 3 / 2);
        assert_eq!(out.error, Some(RunError::MemoryBudget { max: KEEPS as u64 * 3 / 2 }));
        assert_eq!(engine.1.borrow().len(), 2, "the map stopped at the item past the round's memory");
        let (out, _) = run_on(&mapped, &[items], KEEPS as u64 * 64);
        assert_eq!((out.error, engine.1.borrow().len()), (None, 32), "within it, every item");
    }

    /// Guards: a round's module calls draw on fuel of its own, the limits' `max_fuel`, afresh
    /// each round: the flow whose module runs once in each of two rounds finishes on one call's
    /// fuel, not two; an instruction less fails it with `FuelBudget`, performing nothing.
    /// Why: per round is what `holon verify`, evaluating once with every answer, burns again; and
    /// a round past its fuel must fail the run, not leave a node absent and answer as if it
    /// had computed.
    /// If it fails: `Asking::take_fuel` (or `give_back_fuel`) isn't the round's budget, the budget
    /// isn't made afresh with `max_fuel`, or `run_rounds` doesn't read `out_of_fuel`.
    #[test]
    fn every_module_call_a_round_makes_burns_that_rounds_fuel() {
        const COST: u64 = 40;
        let mut r = MapResolver::new();
        let flow = chained_through_a_module(&mut r);
        let engine = Burning(&r, COST, std::cell::Cell::new(0));
        let run = |max_fuel: u64| {
            let (echo, mut host) = (Echo::default(), Performed::default());
            let limits = Limits { max_fuel, ..Limits::request_run() };
            let out = run_rounds(&flow, &[], &[], &engine, &mut host, &echo, &limits, &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
            (out, host.0)
        };
        let (out, performed) = run(COST);
        assert_eq!(out.error, None, "one call's fuel a round is enough");
        assert_eq!(engine.2.replace(0), 2, "the module ran in two rounds");
        assert_eq!(performed, vec![(Capability::NetworkEgress, txt("/first/next"))]);
        let (out, performed) = run(COST - 1);
        assert_eq!(out.error, Some(RunError::FuelBudget { max: COST - 1 }));
        assert_eq!(out.outputs, vec![None]);
        assert!(performed.is_empty(), "nothing performed");
        let message = out.error.unwrap().message();
        assert!(message.starts_with(STEP_BUDGET) && message.contains("max_fuel"), "{message}");
    }

    #[test]
    fn the_request_budget_fails_the_run_and_performs_nothing() {
        let mut r = MapResolver::new();
        let flow = chained(&mut r);
        let (echo, mut host) = (Echo::default(), Performed::default());
        let limits = Limits { max_requests: 1, ..Limits::request_run() };
        let out = run_rounds(&flow, &[], &[], &r, &mut host, &echo, &limits, &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
        assert_eq!(out.error, Some(RunError::RequestBudget { max: 1 }));
        assert_eq!(out.outputs, vec![None]);
        assert!(host.0.is_empty(), "nothing performed");
        assert_eq!(echo.0.lock().unwrap().len(), 1, "the first request went out; the second never did");
    }

    #[test]
    fn the_time_budget_fails_the_run() {
        struct Slow;
        impl Performer for Slow {
            fn perform(&self, _: &Value, limits: &Limits) -> Value {
                std::thread::sleep(Duration::from_millis(30).min(limits.request_timeout));
                record(&[("status", Value::Number(200.0)), ("body", txt("/x"))])
            }
        }
        let mut r = MapResolver::new();
        let flow = chained(&mut r);
        let mut host = Performed::default();
        let limits = Limits { run_budget: Duration::from_millis(20), ..Limits::request_run() };
        let out = run_rounds(&flow, &[], &[], &r, &mut host, &Slow, &limits, &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
        assert_eq!(out.error, Some(RunError::TimeBudget { budget: Duration::from_millis(20) }));
        assert!(host.0.is_empty());
    }

    /// **Guards:** a try after the first sends no call from a node an earlier try called out
    /// from, unless the record answers it (R115): the run fails, held back, nothing sent. By
    /// node, item numbers out: a list that grew at its end or its front between the tries is held
    /// back too. A node no earlier try called from sends; so does an earlier part's (`prior`
    /// without `tried`: a resumed run, not a retry).
    ///
    /// **Why:** a run tried again because what it read changed sent its call again with the new
    /// contents (a price read as 5, then as 7): the far side was asked twice, each time to act.
    ///
    /// **If it fails:** the row names the case; `run_rounds` doesn't hold back by
    /// `holon_core::node_path`, or holds back where nothing was sent before.
    #[test]
    fn a_try_after_the_first_sends_no_second_call_from_a_node() {
        let mut r = MapResolver::new();
        let port = |name: &str| Port { name: name.into(), ty: Type::Any };
        let request = r.insert(leaf("http_request", vec![Param::frozen("method", "GET"), Param::frozen("host", "api.example.com")], 3, Type::Record)).unwrap();
        let body = r.insert(leaf("field", vec![Param::frozen("name", "body")], 1, Type::Any)).unwrap();
        // fetch(path) = request(path).body; the flow maps it over a list at [0] and calls once at [1].
        let fetch = Unit { body: UnitBody::Composite { children: vec![request, body], edges: vec![e(BOUNDARY, 0, 0, 0), e(0, 0, 1, 0), e(1, 0, BOUNDARY, 0)] }, params: vec![], inputs: vec![port("path")], outputs: vec![port("out")] };
        let fetch = r.insert(fetch).unwrap();
        let each = r.insert(Unit { body: UnitBody::Primitive { name: "map".into() }, params: vec![Param::frozen("fn", holon_core::hex(fetch))], inputs: vec![Port { name: "list".into(), ty: Type::list_any() }], outputs: vec![port("out")] }).unwrap();
        let flow = Unit {
            body: UnitBody::Composite { children: vec![each, request, body], edges: vec![e(BOUNDARY, 0, 0, 0), e(BOUNDARY, 1, 1, 0), e(1, 0, 2, 0), e(0, 0, BOUNDARY, 0), e(2, 0, BOUNDARY, 1)] },
            params: vec![],
            inputs: vec![port("paths"), port("path")],
            outputs: vec![port("all"), port("one")],
        };
        let inputs = |paths: &[&str], one: &str| vec![Some(Value::List(paths.iter().map(|p| txt(p)).collect())), Some(txt(one))];
        let run = |inputs: &[Option<Value>], prior: &[Round], tried: &[Round]| {
            let echo = Echo::default();
            let out = run_rounds(&flow, inputs, &[], &r, &mut Performed::default(), &echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, prior, tried, &Whole);
            let asked = echo.0.into_inner().unwrap().iter().map(|u| u.trim_start_matches("https://api.example.com").to_string()).collect::<Vec<_>>();
            (out, asked)
        };
        let (first, asked) = run(&inputs(&["/a", "/b"], "/x"), &[], &[]);
        assert_eq!((first.error.as_ref(), asked.len()), (None, 3), "the first try sends each");
        let tried = first.rounds.as_slice();
        let held = |node: Vec<u32>| Some(RunError::HeldBack { node });
        // (what changed, the inputs, whether the rounds are this part's earlier tries, the error, what is sent)
        type Row<'a> = (&'a str, Vec<Option<Value>>, bool, Option<RunError>, Vec<&'a str>);
        let rows: [Row; 6] = [
            ("nothing: each call answered from the record", inputs(&["/a", "/b"], "/x"), true, None, vec![]),
            ("the one call's request, at [1]", inputs(&["/a", "/b"], "/y"), true, held(vec![1]), vec![]),
            ("the list grew at its end: a call at [0, 0] no try sent", inputs(&["/a", "/b", "/c"], "/x"), true, held(vec![0, 0]), vec![]),
            ("the list grew at its front: each item's call moved", inputs(&["/z", "/a", "/b"], "/x"), true, held(vec![0, 0]), vec![]),
            ("an earlier part's call, not an earlier try's", inputs(&["/a", "/b"], "/y"), false, None, vec!["/y"]),
            ("the list shrank: what is left answered from the record", vec![Some(Value::List(vec![txt("/a")])), Some(txt("/x"))], true, None, vec![]),
        ];
        for (why, inputs, retry, error, sends) in rows {
            let (out, asked) = run(&inputs, tried, if retry { tried } else { &[] });
            assert_eq!((out.error, asked), (error, sends.iter().map(|s| s.to_string()).collect::<Vec<_>>()), "{why}");
        }
        // A node the first try never called out from: its list empty then.
        let (empty, _) = run(&inputs(&[], "/x"), &[], &[]);
        let (out, asked) = run(&inputs(&["/a"], "/x"), &empty.rounds, &empty.rounds);
        assert_eq!((out.error, asked), (None, vec!["/a".to_string()]), "a node no earlier try called from sends");
        assert!(RunError::HeldBack { node: vec![0, 0] }.message().starts_with(HELD_BACK));
    }

    /// **Guards:** a round that sends nothing and computes past the run's time stops at it, with
    /// the time budget's error and nothing performed, though it is within its steps.
    ///
    /// **Why:** the clock was read only after a round that sent requests, so a round that only
    /// computes ran to its steps however long they took: a request computing the square of a
    /// list held a server's thread past its caller's patience and the run's budget alike, and
    /// `holon verify` ran it again for as long.
    ///
    /// **If it fails:** `Asking` doesn't read the clock as the round steps (`late`), or
    /// `run_rounds` doesn't look at it after the round before its steps.
    #[test]
    fn a_round_that_only_computes_stops_at_the_runs_time() {
        let mut r = MapResolver::new();
        // For each item of a list, whether it is in the list: a scan of the list each, n² in all.
        let includes = r.insert(leaf("includes", vec![], 2, Type::Boolean)).unwrap();
        let port = |name: &str| Port { name: name.into(), ty: Type::Any };
        let is_in = Unit {
            body: UnitBody::Composite { children: vec![includes], edges: vec![e(BOUNDARY, 1, 0, 0), e(BOUNDARY, 0, 0, 1), e(0, 0, BOUNDARY, 0)] },
            params: vec![],
            inputs: vec![port("x"), port("xs")],
            outputs: vec![port("in")],
        };
        let is_in = r.insert(is_in).unwrap();
        let each = Unit {
            body: UnitBody::Primitive { name: "map".into() },
            params: vec![Param::frozen("fn", holon_core::hex(is_in))],
            inputs: vec![Port { name: "list".into(), ty: Type::list_any() }, port("extra0")],
            outputs: vec![port("out")],
        };
        let xs = Value::List((0..20_000).map(|i| Value::Number(f64::from(i))).collect());
        let budget = Duration::from_millis(50);
        let limits = Limits { run_budget: budget, max_steps: holon_core::MAX_STEPS_CEILING, ..Limits::request_run() };
        let mut host = Performed::default();
        let started = Instant::now();
        let out = run_rounds(&each, &[Some(xs.clone()), Some(xs)], &[], &r, &mut host, &Echo::default(), &limits, &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
        let took = started.elapsed();
        // The error is a step's own word that it found the time up: a round that only computes
        // and runs its work out ends with none, the clock being read only as it steps. How soon
        // after the budget it stopped isn't held: on a loaded machine anything takes longer.
        assert_eq!(out.error, Some(RunError::TimeBudget { budget }), "stopped for its time, within its steps");
        assert_eq!(out.outputs, vec![None]);
        assert!(out.rounds.is_empty() && host.0.is_empty(), "nothing sent or performed");
        assert!(took >= budget, "stopped at its budget, not before it: {took:?}");
    }

    /// A request inside a mapped function is sent for each item, in one round, each at its own
    /// path (the map's, then the item's index); the next round answers each its own, and a
    /// replay from the record gives the same.
    #[test]
    fn a_request_inside_map_is_sent_and_answered_per_item() {
        let mut r = MapResolver::new();
        let request = r.insert(leaf("http_request", vec![Param::frozen("method", "GET"), Param::frozen("host", "api.example.com")], 3, Type::Record)).unwrap();
        let body = r.insert(leaf("field", vec![Param::frozen("name", "body")], 1, Type::Any)).unwrap();
        // fetch(path) = http_request(path).body
        let fetch = Unit {
            body: UnitBody::Composite { children: vec![request, body], edges: vec![e(BOUNDARY, 0, 0, 0), e(0, 0, 1, 0), e(1, 0, BOUNDARY, 0)] },
            params: vec![],
            inputs: vec![Port { name: "path".into(), ty: Type::Text }],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        };
        let hfetch = r.insert(fetch).unwrap();
        let map = Unit { params: vec![Param::frozen("fn", hfetch.iter().map(|b| format!("{b:02x}")).collect::<String>())], ..leaf("map", vec![], 1, Type::list_any()) };
        let hmap = r.insert(map).unwrap();
        let flow = Unit {
            body: UnitBody::Composite { children: vec![hmap], edges: vec![e(BOUNDARY, 0, 0, 0), e(0, 0, BOUNDARY, 0)] },
            params: vec![],
            inputs: vec![Port { name: "paths".into(), ty: Type::list_any() }],
            outputs: vec![Port { name: "out".into(), ty: Type::list_any() }],
        };
        let paths = Value::List(vec![txt("/a"), txt("/b")]);
        let echo = Echo::default();
        let out = run_rounds(&flow, &[Some(paths.clone())], &[], &r, &mut Performed::default(), &echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
        assert_eq!(out.outputs, vec![Some(Value::List(vec![txt("/a"), txt("/b")]))]);
        assert_eq!(out.rounds.len(), 1, "both items' requests go out in one round");
        assert_eq!(out.rounds[0].exchanges.iter().map(|x| x.path.clone()).collect::<Vec<_>>(), vec![vec![0, 0, 0], vec![0, 1, 0]]);
        // Performed side by side, so in either order.
        let mut sent = echo.0.lock().unwrap().clone();
        sent.sort();
        assert_eq!(sent, ["https://api.example.com/a", "https://api.example.com/b"]);
        let replayed = holon_core::eval_observed(&flow, &[Some(paths)], &r, &mut Recorded::new(&out.rounds)).into_vec();
        assert_eq!(replayed, out.outputs, "a replay answers each item from the record");
    }

    /// `respond if go then http.get(â€¦) else "skipped"`, as `if` lowers:
    /// `coalesce(guard(go, http_request("/json")), "skipped")`, with `node` in the request's place.
    fn branch(r: &mut MapResolver, node: Unit) -> Unit {
        let constant = |v: &str| leaf("const", vec![Param::frozen("value", v), Param::frozen("type", "Text")], 0, Type::Text);
        // The path goes to the node if it takes one (an `await` doesn't).
        let mut edges = if node.inputs.is_empty() { vec![] } else { vec![e(0, 0, 1, 0)] };
        let children = vec![
            r.insert(constant("/json")).unwrap(),                      // 0
            r.insert(node).unwrap(),                                   // 1
            r.insert(leaf("guard", vec![], 2, Type::Any)).unwrap(),    // 2
            r.insert(constant("skipped")).unwrap(),                    // 3
            r.insert(leaf("coalesce", vec![], 2, Type::Any)).unwrap(), // 4
        ];
        edges.extend([e(BOUNDARY, 0, 2, 0), e(1, 0, 2, 1), e(2, 0, 4, 0), e(3, 0, 4, 1), e(4, 0, BOUNDARY, 0)]);
        Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![],
            inputs: vec![Port { name: "go".into(), ty: Type::Boolean }],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        }
    }

    /// The leak lazy choices fix: a request in a branch not taken was sent anyway, and its answer
    /// thrown away. Now it is sent only when its branch is taken.
    #[test]
    fn a_request_in_a_branch_not_taken_is_not_sent() {
        let mut r = MapResolver::new();
        let flow = branch(&mut r, leaf("http_request", vec![Param::frozen("method", "GET"), Param::frozen("host", "api.example.com")], 3, Type::Record));
        let run = |go: bool, echo: &Echo| run_rounds(&flow, &[Some(Value::Boolean(go))], &[], &r, &mut Performed::default(), echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);

        let echo = Echo::default();
        let out = run(false, &echo);
        assert_eq!(out.outputs, vec![Some(txt("skipped"))]);
        assert!(echo.0.lock().unwrap().is_empty(), "nothing sent");
        assert!(out.rounds.is_empty());

        let echo = Echo::default();
        let out = run(true, &echo);
        assert_eq!(out.outputs, vec![Some(record(&[("status", Value::Number(200.0)), ("body", txt("/json"))]))]);
        assert_eq!(*echo.0.lock().unwrap(), vec!["https://api.example.com/json"], "sent once");
    }

    /// An `await` in a branch not taken doesn't park the run: nothing waits on it.
    #[test]
    fn an_await_in_a_branch_not_taken_does_not_park_the_run() {
        let mut r = MapResolver::new();
        let flow = branch(&mut r, leaf("await", vec![Param::frozen("name", "paid"), Param::frozen("expires", "60")], 0, Type::Any));
        let run = |go: bool| run_rounds(&flow, &[Some(Value::Boolean(go))], &[], &r, &mut Performed::default(), &Echo::default(), &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
        let out = run(false);
        assert!(out.waiting.is_empty());
        assert_eq!(out.outputs, vec![Some(txt("skipped"))]);
        let out = run(true);
        assert_eq!(out.waiting.iter().map(|w| (w.name.as_str(), w.expires, w.approval.as_deref(), w.shows.as_ref())).collect::<Vec<_>>(), vec![("paid", 60, None, None)]);
        assert_eq!(out.outputs, vec![None], "waiting, not fallen through to the else");
    }

    /// A collection read in a branch not taken isn't made, so it isn't recorded against the run.
    #[test]
    fn a_read_in_a_branch_not_taken_is_not_made() {
        #[derive(Default)]
        struct Reads(Vec<Vec<u32>>);
        impl Waits for Reads {
            fn event(&self, _: &str) -> Option<Value> {
                None
            }
            fn token(&mut self, _: &str) -> String {
                String::new()
            }
            fn read(&mut self, path: &[u32], _: &Value) -> Option<Value> {
                self.0.push(path.to_vec());
                Some(txt("stored"))
            }
        }
        let mut r = MapResolver::new();
        let flow = branch(&mut r, leaf("collection_get", vec![Param::frozen("collection", "prices")], 1, Type::Any));
        let run = |go: bool, reads: &mut Reads| run_rounds(&flow, &[Some(Value::Boolean(go))], &[], &r, &mut Performed::default(), &Echo::default(), &Limits::request_run(), &mut holon_core::NoObserver, reads, &[], &[], &Whole);
        let mut reads = Reads::default();
        assert_eq!(run(false, &mut reads).outputs, vec![Some(txt("skipped"))]);
        assert!(reads.0.is_empty());
        let mut reads = Reads::default();
        assert_eq!(run(true, &mut reads).outputs, vec![Some(txt("stored"))]);
        assert_eq!(reads.0, vec![vec![1]]);
    }

    #[test]
    fn a_flow_without_requests_is_one_round_like_fire() {
        let mut r = MapResolver::new();
        let egress = r.insert(leaf("egress", vec![], 1, Type::Effect)).unwrap();
        let flow = Unit {
            body: UnitBody::Composite { children: vec![egress], edges: vec![e(BOUNDARY, 0, 0, 0), e(0, 0, BOUNDARY, 0)] },
            params: vec![],
            inputs: vec![Port { name: "in".into(), ty: Type::Text }],
            outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
        };
        let mut host = Performed::default();
        let out = run_rounds(&flow, &[Some(txt("hi"))], &[], &r, &mut host, &Echo::default(), &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[], &[], &Whole);
        assert!(out.rounds.is_empty() && out.error.is_none() && out.waiting.is_empty());
        assert_eq!(host.0, vec![(Capability::NetworkEgress, txt("hi"))]);
    }

    #[test]
    fn a_rounds_requests_to_one_host_respect_its_concurrency_limit() {
        struct Counting {
            now: Mutex<usize>,
            peak: Mutex<usize>,
        }
        impl Performer for Counting {
            fn perform(&self, _: &Value, _: &Limits) -> Value {
                {
                    let mut n = self.now.lock().unwrap();
                    *n += 1;
                    let mut p = self.peak.lock().unwrap();
                    *p = (*p).max(*n);
                }
                std::thread::sleep(Duration::from_millis(15));
                *self.now.lock().unwrap() -= 1;
                record(&[("status", Value::Number(200.0))])
            }
        }
        let request = |path: &str| {
            record(&[
                ("method", txt("GET")),
                ("url", txt(&format!("https://api.example.com{path}"))),
                ("headers", record(&[])),
                ("body", txt("")),
            ])
        };
        let pending: Vec<(Vec<u32>, Value)> = (0..7).map(|i| (vec![i], request(&format!("/{i}")))).collect();
        let counting = Counting { now: Mutex::new(0), peak: Mutex::new(0) };
        let limits = Limits { per_host_concurrency: 3, ..Limits::request_run() };
        let exchanges = perform_all(pending, &counting, &limits, Instant::now());
        assert_eq!(exchanges.len(), 7);
        assert!(*counting.peak.lock().unwrap() <= 3, "peak {}", counting.peak.lock().unwrap());
        assert_eq!(exchanges.iter().map(|x| x.path[0]).collect::<Vec<_>>(), (0..7).collect::<Vec<_>>());
    }

    /// Calls to one flow's actions change things there: they go one at a time, in the order of the
    /// nodes that ask, however many the host's concurrency allows (docs/L4s/triggers-L4.md §9).
    #[test]
    fn calls_to_one_flow_go_one_at_a_time_in_order() {
        struct Ordered {
            now: Mutex<usize>,
            peak: Mutex<usize>,
            seen: Mutex<Vec<String>>,
        }
        impl Performer for Ordered {
            fn perform(&self, request: &Value, _: &Limits) -> Value {
                let Value::Record(r) = request else { panic!() };
                {
                    let mut n = self.now.lock().unwrap();
                    *n += 1;
                    let mut p = self.peak.lock().unwrap();
                    *p = (*p).max(*n);
                }
                std::thread::sleep(Duration::from_millis(5));
                *self.now.lock().unwrap() -= 1;
                let Some(Value::Text(action)) = r.get("action") else { panic!() };
                self.seen.lock().unwrap().push(action.clone());
                record(&[("answer", Value::Number(1.0))])
            }
        }
        let call = |flow: &str, action: &str| record(&[("flow", txt(flow)), ("action", txt(action)), ("input", record(&[]))]);
        // Asked out of order: the nodes' paths say the order.
        let pending = vec![
            (vec![2], call("billing", "c")),
            (vec![0], call("billing", "a")),
            (vec![1], call("billing", "b")),
        ];
        let performer = Ordered { now: Mutex::new(0), peak: Mutex::new(0), seen: Mutex::new(vec![]) };
        let exchanges = perform_all(pending, &performer, &Limits::request_run(), Instant::now());
        assert_eq!(exchanges.len(), 3);
        assert_eq!(*performer.peak.lock().unwrap(), 1, "one at a time");
        assert_eq!(*performer.seen.lock().unwrap(), ["a", "b", "c"], "in the order of the nodes");
        assert_eq!(host_of(&call("billing", "a")), "flow:billing");
        assert_eq!(host_of(&call("ledger", "a")), "flow:ledger", "a group for each flow");
    }
}
