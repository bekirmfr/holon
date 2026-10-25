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
//! rounds, since every round after the first needs a request) and a wall-clock budget. Past
//! either, the run fails and nothing is performed. A single request that breaks a limit does not
//! fail the run: its node gets an error response and the flow decides.
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

use holon_core::{eval_observed, hash_value, Answer, Capability, Hash, Observer, Resolver, Slot, Unit, Value};

use crate::{perform_effects, Host, SourcedResolver};

/// The numbers a run is held to (§4.2; the host grants them, §4.2.1). Defaults follow what the
/// systems a flow talks to already expect.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    /// The whole run, every round and request included.
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
        }
    }
}

/// Performs one outbound request. `request` is the record core built (`{method, url, headers,
/// body}`); the result is the node's response: `{status, headers, body}`, or, when the request
/// broke a rule or a limit or failed, `{status: 0, error: "..."}`. Never panics on a bad request:
/// every outcome is a response. `&self` and `Sync`, so a round's requests can go out at once.
pub trait Performer: Sync {
    fn perform(&self, request: &Value, limits: &Limits) -> Value;
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
}

impl RunError {
    pub fn message(&self) -> String {
        match self {
            RunError::RequestBudget { max } => format!("the run needed more than {max} outbound requests"),
            RunError::TimeBudget { budget } => format!("the run took longer than {budget:?}"),
            RunError::Overtaken => "what the run read changed before it could call out; it ran again".to_string(),
            RunError::Busy { key } => format!("busy: {key} is held by another run"),
        }
    }
}

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

/// A waiting point a run is waiting at: an `await` with no event yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub name: String,
    /// How long it waits, in seconds.
    pub expires: u64,
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
/// does, performing outbound requests with `performer` within `limits`, and the final round's
/// effects with `host`, unless the run waits. `observer` sees every round's steps (after a
/// `round(n)` call) as in a single evaluation. `waits` answers waiting points and tokens;
/// `prior` holds the rounds of an earlier part of the same run (a resumed one), whose requests
/// are answered from it, not sent again. The outcome's rounds are the new ones.
#[allow(clippy::too_many_arguments)]
pub fn run_rounds(
    root: &Unit,
    inputs: &[Option<Value>],
    sourced: &[(Capability, Value)],
    real: &impl Resolver,
    host: &mut impl Host,
    performer: &impl Performer,
    limits: &Limits,
    observer: &mut impl Observer,
    waits: &mut impl Waits,
    prior: &[Round],
) -> RunOutcome {
    let started = Instant::now();
    let resolver = SourcedResolver::new(sourced, real);
    let mut answers: BTreeMap<(Vec<u32>, Hash), Value> = prior
        .iter()
        .flat_map(|r| &r.exchanges)
        .map(|x| ((x.path.clone(), request_key(&x.request)), x.response.clone()))
        .collect();
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
        let mut asking =
            Asking { inner: &mut *observer, answers: &answers, pending: Vec::new(), waits: &mut *waits, waiting: Vec::new() };
        let outputs = eval_observed(root, inputs, &resolver, &mut asking).into_vec();
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
        let exchanges = perform_all(pending, performer, &this_round, started);
        for ex in &exchanges {
            answers.insert((ex.path.clone(), request_key(&ex.request)), ex.response.clone());
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
    pending: Vec<(Vec<u32>, Value)>,
    waits: &'a mut W,
    waiting: Vec<Waiting>,
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
    fn answer(&mut self, path: &[u32], request: &Value) -> Answer {
        if let Some(name) = text_field(request, "await") {
            let event = self.waits.event(name);
            if event.is_none() && !self.waiting.iter().any(|w| w.name == name) {
                let expires = match request {
                    Value::Record(r) => match r.get("expires") {
                        Some(Value::Number(n)) => *n as u64,
                        _ => holon_core::MAX_AWAIT_SECONDS,
                    },
                    _ => holon_core::MAX_AWAIT_SECONDS,
                };
                self.waiting.push(Waiting { name: name.to_string(), expires });
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
        let key = (path.to_vec(), request_key(request));
        if let Some(answer) = self.answers.get(&key) {
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
            .into_values()
            .map(|requests| {
                scope.spawn(move || {
                    let mut done = Vec::new();
                    for batch in requests.chunks(limits.per_host_concurrency.max(1)) {
                        std::thread::scope(|inner| {
                            let handles: Vec<_> = batch
                                .iter()
                                .map(|(path, request)| {
                                    inner.spawn(move || {
                                        let t0 = Instant::now();
                                        let response = performer.perform(request, limits);
                                        Exchange {
                                            path: path.clone(),
                                            request: request.clone(),
                                            response,
                                            start_us: us(t0),
                                            end_us: us(Instant::now()),
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

/// The `host[:port]` of a request's URL, for grouping.
fn host_of(request: &Value) -> String {
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
        let out = run_rounds(&flow, &[], &[], &r, &mut host, &echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[]);
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
        let first = run_rounds(&flow, &[], &[], &r, &mut host, &echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut waits, &[]);
        assert_eq!(first.waiting, vec![Waiting { name: "paid".into(), expires: 60 }]);
        assert!(host.0.is_empty(), "a waiting run performs nothing");
        assert!(matches!(&first.outputs[0], Some(Value::Effect { payload, .. }) if **payload == txt("tok-paid")), "its interim output is there");

        waits.events.insert("paid".into(), txt("ok"));
        let resumed = run_rounds(&flow, &[], &[], &r, &mut host, &echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut waits, &first.rounds);
        assert!(resumed.waiting.is_empty());
        assert_eq!(host.0, vec![(Capability::NetworkEgress, txt("tok-paid")), (Capability::NetworkEgress, txt("ok"))]);
    }

    #[test]
    fn the_request_budget_fails_the_run_and_performs_nothing() {
        let mut r = MapResolver::new();
        let flow = chained(&mut r);
        let (echo, mut host) = (Echo::default(), Performed::default());
        let limits = Limits { max_requests: 1, ..Limits::request_run() };
        let out = run_rounds(&flow, &[], &[], &r, &mut host, &echo, &limits, &mut holon_core::NoObserver, &mut NoWaits, &[]);
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
        let out = run_rounds(&flow, &[], &[], &r, &mut host, &Slow, &limits, &mut holon_core::NoObserver, &mut NoWaits, &[]);
        assert_eq!(out.error, Some(RunError::TimeBudget { budget: Duration::from_millis(20) }));
        assert!(host.0.is_empty());
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
        let out = run_rounds(&flow, &[Some(paths.clone())], &[], &r, &mut Performed::default(), &echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[]);
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
        let run = |go: bool, echo: &Echo| run_rounds(&flow, &[Some(Value::Boolean(go))], &[], &r, &mut Performed::default(), echo, &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[]);

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
        let run = |go: bool| run_rounds(&flow, &[Some(Value::Boolean(go))], &[], &r, &mut Performed::default(), &Echo::default(), &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[]);
        let out = run(false);
        assert!(out.waiting.is_empty());
        assert_eq!(out.outputs, vec![Some(txt("skipped"))]);
        let out = run(true);
        assert_eq!(out.waiting, vec![Waiting { name: "paid".into(), expires: 60 }]);
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
        let run = |go: bool, reads: &mut Reads| run_rounds(&flow, &[Some(Value::Boolean(go))], &[], &r, &mut Performed::default(), &Echo::default(), &Limits::request_run(), &mut holon_core::NoObserver, reads, &[]);
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
        let out = run_rounds(&flow, &[Some(txt("hi"))], &[], &r, &mut host, &Echo::default(), &Limits::request_run(), &mut holon_core::NoObserver, &mut NoWaits, &[]);
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
}
