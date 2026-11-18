//! Replaying a version on a deployment's recent runs, offline: what a proposal or a draft would
//! have done with the runs the deployment took, whatever started them. Each run is run again as
//! `holon verify` runs one (`rerun`), from what its entry records, but through the new version's
//! handler of the same kind and label (`handler_of`: a route, a WebSocket's block, a clock, a
//! file's, a mail's, a feed's or a queue's handler, an action); and what it produces is compared
//! with what the run produced: the status it answered, its outputs, the effects it performed,
//! where it waited.
//!
//! Nothing is sent. An outbound request is answered by what the run was answered to the very
//! same request, and a read by what the run read for the same one, wherever in the flow it is
//! asked from: a new version moves nodes. One the run never made can't be answered offline, and
//! the run can't be told ("can't tell"), not failed. So for a node that computes with a secret or
//! a token (the log holds a placeholder, never the value): one that passes it along, or picks past
//! it, gives the same with any placeholder and is computed; one whose answer depends on which
//! placeholder computed with the value, and the run can't be told (`Fed`). Nor can a run that
//! reads a clock or a random the run didn't, or one that failed then. What `Fed` computes stands
//! in for the node, so the round's budget never counts it: it computes on a budget of its own,
//! each round twice the round's steps and fuel (it computes a node twice), and a version that
//! wants more can't be told either.
//!
//! Each run is held to what the deployment is granted a run of its kind (`within`): each round to
//! its steps, fuel and memory, the run to its time and its count of requests. A replay needs no
//! approval, and anyone who may read the deployment starts one (an agent too), so it computes no
//! more than a person approved for the deployment's own runs. A version that asks for more has it
//! once deployed; replayed, a run that needs it doesn't finish, and says so. A run that asks what
//! its run didn't (a request, a read) can't be told, so it goes no further (`ByRead::claim`).
//!
//! And a replay as a whole is held (`Replays`): it runs for `DEADLINE` at most, the runs it hasn't
//! reached by then counted, not run (`not_reached`), and the server runs `AT_ONCE` at a time, the
//! next answered 503: a replay holds one of the control plane's threads while it runs.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// What `GET /deployments/<slug>/replay` answers.
#[derive(Serialize, Debug)]
pub(crate) struct Replayed {
    /// The version replayed (hex).
    pub(crate) unit: String,
    pub(crate) replayed: usize,
    pub(crate) same: usize,
    pub(crate) differ: usize,
    pub(crate) cant_tell: usize,
    /// The runs asked for that it didn't reach before its deadline (`Replays`): the oldest.
    pub(crate) not_reached: usize,
    /// Newest first.
    pub(crate) runs: Vec<RunReplayed>,
}

#[derive(Serialize, Debug)]
pub(crate) struct RunReplayed {
    pub(crate) seq: u64,
    pub(crate) at_ms: u64,
    pub(crate) kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) route: Option<String>,
    /// `same`, `differs` or `cant_tell`.
    pub(crate) outcome: &'static str,
    /// How it differs, each a phrase; or why it can't be told, one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) why: Vec<String>,
    /// The status it answered then, and the status the version answers (a request's).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) now_status: Option<u16>,
}

/// How many recent runs a replay takes, unless asked for another number, and at most.
const RUNS: usize = 20;
const MOST_RUNS: usize = 200;

/// How many replays the server runs at once ([`Replays`]).
const AT_ONCE: usize = 2;
/// How long a replay runs, at most ([`Replays`]).
const DEADLINE: Duration = Duration::from_secs(30);

/// The replays a server is running, and how many it runs at once and for how long: a replay runs
/// on a thread of the control plane, as long as its runs compute, so many at once, or one long
/// one, would leave none to answer anyone else. Each runs until its deadline, then counts the runs
/// it didn't reach (`not_reached`); one more than `at_once` is answered 503.
pub(crate) struct Replays {
    going: AtomicUsize,
    at_once: usize,
    deadline: Duration,
}

impl Default for Replays {
    fn default() -> Replays {
        Replays { going: AtomicUsize::new(0), at_once: AT_ONCE, deadline: DEADLINE }
    }
}

impl Replays {
    /// Replays `at_once` at a time, each for `deadline` at most.
    #[cfg(test)]
    pub(crate) fn new(at_once: usize, deadline: Duration) -> Replays {
        Replays { going: AtomicUsize::new(0), at_once, deadline }
    }

    /// A place among those running, while it is held; none when every one is taken.
    pub(crate) fn enter(&self) -> Option<Going<'_>> {
        let at_once = self.at_once;
        self.going.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < at_once).then_some(n + 1)).ok().map(|_| Going(&self.going))
    }
}

/// A replay running ([`Replays::enter`]): its place is given back when it is dropped.
pub(crate) struct Going<'a>(&'a AtomicUsize);

impl Drop for Going<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The limits a deployment grants its runs (its permit's): a request's (a route's, a WebSocket
/// block's, an action's, a waiting run's later part), and a clock's (a tick's, a file's, a mail's,
/// a feed's or a queue's handler).
pub(crate) struct Granted<'a> {
    pub(crate) request: &'a Limits,
    pub(crate) clock: &'a Limits,
}

impl Granted<'_> {
    /// What a run of `kind` (its entry's) is granted, as the server ran it (`fire_with_state`'s
    /// `clock`).
    fn of(&self, kind: &str) -> &Limits {
        match kind {
            "tick" | "file" | "mail" | "feed" | "queue" => self.clock,
            _ => self.request,
        }
    }
}

/// `GET /deployments/<slug>/replay?unit=<hex>[&runs=N]`: the version `unit` (staged, `POST
/// /unit`, or a proposal's) replayed on the deployment's last `N` runs. Changes nothing.
pub(crate) fn handle_get_replay(
    store: &SharedStore,
    folders: &folders::Folders,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    replays: &Replays,
    slug: &str,
    request: &Request,
) -> Response {
    let Some(unit) = request.get_param("unit").as_deref().and_then(parse_hash) else {
        return err(400, "unit: the version to replay, as its hash (hex), staged with POST /unit");
    };
    let runs = match request.get_param("runs").map(|v| v.parse::<usize>()) {
        None => RUNS,
        Some(Ok(n)) if (1..=MOST_RUNS).contains(&n) => n,
        Some(_) => return err(400, format!("runs: how many recent runs, 1 to {MOST_RUNS}")),
    };
    let (log, redactor, request_limits, clock_limits, plain_http) = match deployments.lock().unwrap().get(slug) {
        Some(dep) => (
            std::sync::Arc::clone(&dep.log),
            crate::secrets::Redactor::of(&dep.secrets),
            dep.permit.request_limits.clone(),
            dep.permit.clock_limits.clone(),
            dep.permit.outbound.plain_http.clone(),
        ),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    let Some(_going) = replays.enter() else {
        return err(503, format!("the server is running {} replays already: try again in a moment", replays.at_once)).with_additional_header("Retry-After", "5");
    };
    let deadline = Instant::now() + replays.deadline;
    let loaded = match units::closure(store, unit) {
        Ok(loaded) => loaded,
        Err(e) => return e.response(),
    };
    if loaded.resolve(&unit).is_none() {
        return err(404, format!("no unit {}: stage it first (POST /unit)", to_hex(&unit)));
    }
    let resolver = holon_module::WithEngine::new(&loaded, holon_module::shared());
    if read_server(loaded.resolve(&unit).expect("resolved above"), &resolver).is_err() {
        return err(400, "this unit isn't a flow's version: it has no routes, clocks or actions to replay");
    }
    // Read with the log's lock released: its runs go on meanwhile (release audit R45).
    let (lines, _) = log.lines_to_head();
    // The file texts its runs read, as an export ships them (none holding a secret).
    let export = match export_of(store, folders, Some(&redactor), slug, lines, String::new(), Vec::new(), String::new(), u64::MAX) {
        Ok(export) => export,
        Err(e) => return err(500, e),
    };
    Response::json(&replay(&export, unit, &resolver, runs, &Granted { request: &request_limits, clock: &clock_limits }, deadline, &plain_http))
}

/// The limits a replay holds a run of `parts` (its entry and the earlier parts of the same run) to:
/// `holon verify`'s (`rerun_limits`), but what the deployment is `granted` a run of its kind, never
/// more, as a replay needs no approval: each round's steps, fuel and memory, the run's time, and its
/// requests, as many as its parts were each granted (a later part sends its earlier parts' again).
/// And no time past the replay's deadline: `left` of it.
fn within(granted: &Limits, parts: usize, left: Duration) -> Limits {
    Limits {
        max_steps: granted.max_steps,
        max_fuel: granted.max_fuel,
        max_memory: granted.max_memory,
        run_budget: granted.run_budget.min(left),
        max_requests: granted.max_requests.saturating_mul(parts),
        ..super::rerun_limits()
    }
}

/// `unit` replayed on the last `runs` runs among `export`'s lines that ran, newest first, each held
/// `within` what the deployment is `granted` a run of its kind, until `deadline`: the runs left by
/// then are counted as not reached; a request named as it would go on the wire to the hosts
/// granted plain HTTP (`plain_http`).
pub(crate) fn replay(export: &Export, unit: Hash, resolver: &impl holon_core::Resolver, runs: usize, granted: &Granted, deadline: Instant, plain_http: &BTreeSet<String>) -> Replayed {
    let entries: Vec<LogEntry> = export.lines.iter().filter_map(|l| crate::wire::read_json::<LogEntry>(l.as_bytes()).ok()).collect();
    let files = BTreeMap::new();
    let texts = Texts { blobs: &export.blobs, blob_bytes: &export.blob_bytes, files: &files };
    let marks = Marks::new();
    let mut out = Replayed { unit: to_hex(&unit), replayed: 0, same: 0, differ: 0, cant_tell: 0, not_reached: 0, runs: Vec::new() };
    let ran: Vec<usize> = (0..entries.len()).filter(|&i| !entries[i].run_id.is_empty()).collect();
    let asked: Vec<usize> = ran.iter().rev().take(runs).copied().collect();
    for (n, &i) in asked.iter().enumerate() {
        let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|left| !left.is_zero()) else {
            out.not_reached = asked.len() - n;
            break;
        };
        let entry = &entries[i];
        let earlier: Vec<&LogEntry> = entries[..i].iter().filter(|e| e.run_id == entry.run_id).collect();
        let granted = granted.of(&entry.kind);
        let limits = within(granted, earlier.len() + 1, left);
        // Its time cut short by the replay's deadline: a run stopped by that isn't one that differs.
        let cut_short = limits.run_budget < granted.run_budget;
        let (outcome, why, now_status) = match replay_one(entry, &earlier, &texts, unit, resolver, &marks, &limits, cut_short, plain_http) {
            Replay::Same(now) => ("same", Vec::new(), now),
            Replay::Differs(why, now) => ("differs", why, now),
            Replay::CantTell(why) => ("cant_tell", vec![why], None),
        };
        match outcome {
            "same" => out.same += 1,
            "differs" => out.differ += 1,
            _ => out.cant_tell += 1,
        }
        out.replayed += 1;
        let status = if entry.kind == "request" { entry.status } else { None };
        out.runs.push(RunReplayed { seq: entry.seq, at_ms: entry.at_ms, kind: entry.kind.clone(), route: entry.route.clone(), outcome, why, status, now_status });
    }
    out
}

enum Replay {
    /// With the status it answers, a request's.
    Same(Option<u16>),
    Differs(Vec<String>, Option<u16>),
    CantTell(String),
}

#[allow(clippy::too_many_arguments)]
fn replay_one(entry: &LogEntry, earlier: &[&LogEntry], texts: &Texts, unit: Hash, resolver: &impl holon_core::Resolver, marks: &Marks, limits: &Limits, cut_short: bool, plain_http: &BTreeSet<String>) -> Replay {
    let filled = match texts.with_body(entry) {
        Ok(filled) => filled,
        Err(Rerun::Skipped(why) | Rerun::Differs(why)) => return Replay::CantTell(why),
        Err(_) => return Replay::CantTell("its request's body can't be put back".into()),
    };
    let entry = filled.as_ref().unwrap_or(entry);
    if let Some(e) = &entry.error {
        return Replay::CantTell(format!("it failed then: {e}"));
    }
    let Some(version) = resolver.resolve(&unit) else {
        return Replay::CantTell("the version isn't here".into());
    };
    let Ok(server) = read_server(version, resolver) else {
        return Replay::CantTell("the version isn't a flow's".into());
    };
    let now_status = |s: Option<u16>| if entry.kind == "request" { s } else { None };

    // What it runs: the version's handler for what the run's entry names, as `rerun` finds it in
    // the run's own unit.
    let label = entry.route.clone().unwrap_or_default();
    let (handler, sent) = match handler_of(&server, &entry.kind, &label) {
        Ok(found) => found,
        // A request no route takes is answered 404 (http.rs); any other run the version has no
        // handler for doesn't run.
        Err(_) if entry.kind == "request" => {
            let why = vec![format!("the version has no route {label:?}")];
            return if entry.status == Some(404) { Replay::Same(Some(404)) } else { Replay::Differs(why, Some(404)) };
        }
        // An action's label is `action <name>`.
        Err(what) => {
            let name = label.strip_prefix("action ").unwrap_or(&label);
            return Replay::Differs(vec![format!("the version has no {what} {name:?}")], None);
        }
    };
    let Some(root) = resolver.resolve(&handler) else {
        return Replay::CantTell(format!("the handler of {label:?} isn't in the version"));
    };
    let inputs = if sent { vec![entry.request.as_ref().map(|r| marks.read(r, entry.escaped))] } else { Vec::new() };

    let mut extra = Vec::new();
    for (name, value) in &entry.sourced {
        let Some(cap) = durable::capability_named(name) else {
            return Replay::CantTell(format!("it read {name}, which a replay can't supply"));
        };
        extra.push((cap, marks.read(value, entry.escaped)));
    }
    let caps = capabilities(root, resolver);
    // What a run is lent as it starts: the version may read one the run wasn't.
    for cap in caps.iter() {
        let lent = matches!(cap, Capability::Clock | Capability::Random | Capability::FileArrival(_) | Capability::MailArrival(_) | Capability::FeedEvent(_) | Capability::QueueMessage(_));
        if lent && !extra.iter().any(|(c, _)| *c == cap) {
            return Replay::CantTell(format!("the version reads {}, which the run didn't", lent_name(&cap)));
        }
    }
    let values: BTreeMap<String, Value> = entry.state_before.iter().map(|(k, v)| (k.clone(), marks.read(v, entry.escaped))).collect();
    let Ok(state) = State::from_values(values) else {
        return Replay::CantTell("its state isn't data a state holds".into());
    };
    let mut placeholders = Vec::new();
    for cap in caps.iter() {
        if let Capability::SecretRead(name) = cap {
            let shown = entry.secrets.get(&name).map_or(name.clone(), |shown| format!("{name} {shown}"));
            placeholders.push((Capability::SecretRead(name.clone()), Value::Text(marks.of("secret", &shown))));
        }
    }
    let sourced: Vec<(Capability, &Value)> = state
        .iter()
        .map(|(k, v)| (Capability::StateRead(k.clone()), v))
        .chain(extra.iter().filter(|(_, v)| holon_host::is_storable(v)).chain(&placeholders).map(|(cap, value)| (cap.clone(), value)))
        .collect();

    // What it was answered and what it read, by what it asked: the earlier parts' too.
    let parts = || earlier.iter().copied().chain([entry]);
    let answers: Vec<(Value, Value)> = parts()
        .flat_map(|e| e.rounds.iter().map(|r| round_of(r, marks, e.escaped)))
        .flat_map(|r| r.exchanges)
        .map(|x| (x.request, x.response))
        .collect();
    let performer = ByRequest { used: Mutex::new(vec![false; answers.len()]), answers, missed: Mutex::new(Vec::new()), plain_http };
    let own = entry.reads.iter().map(|r| (r, entry.escaped));
    let reads = own
        .chain(earlier.iter().rev().flat_map(|e| e.reads.iter().map(|r| (r, e.escaped))))
        .map(|(read, escaped)| {
            let answer = texts.answer(read, marks, escaped).map_err(|not| match not {
                Rerun::Differs(why) | Rerun::Skipped(why) | Rerun::NothingRan(why) => why,
                Rerun::Matched => String::new(),
            });
            (marks.read(&read.request, escaped), answer)
        })
        .collect();
    let mut waits = ByRead {
        reads,
        events: entry.events.iter().map(|(k, v)| (k.clone(), marks.read(v, entry.escaped))).collect(),
        marks: marks.clone(),
        missed: Vec::new(),
        sent: &performer,
    };
    let mut host = RecordingHost::new();
    let mut observer = Fed::new(marks, resolver, limits);
    let outcome = run_rounds(root, &inputs, &sourced, resolver, &mut host, &performer, limits, &mut observer, &mut waits, &[]);

    if let Some(missed) = performer.missed.lock().unwrap().first().cloned() {
        return Replay::CantTell(format!("it sends {missed}, which the run didn't: what it would be answered can't be told offline"));
    }
    if let Some(missed) = waits.missed.into_iter().next() {
        return Replay::CantTell(missed);
    }
    if observer.consumed {
        return Replay::CantTell("it computes with a secret, a token or a tainted value, which the log never holds".into());
    }
    if observer.too_much {
        let (steps, fuel, memory) = observer.each_round;
        return Replay::CantTell(format!(
            "it computes with a secret, a token or a tainted value more than a replay does in a round ({steps} steps, {fuel} instructions of js and module code, {memory} bytes of memory)"
        ));
    }
    // As `rerun` has it: the server refuses a write of a secret's value.
    let keeps = |c: &Capability| matches!(c, Capability::StateWrite(_) | Capability::CollectionWrite(_) | Capability::DbWrite(_) | Capability::ChannelSend(_) | Capability::FileWrite(_));
    if host.performed.iter().any(|(c, p)| keeps(c) && marks.carries_secret(p)) {
        return Replay::CantTell("it writes a secret's value, which the server refuses".into());
    }
    let status = (entry.kind == "request").then(|| data::respond(outcome.outputs.first().and_then(Option::as_ref)).status);
    if let Some(e) = &outcome.error {
        // The server short of memory says nothing of the version: every run's memory together.
        if let holon_host::RunError::ServerMemory { .. } = e {
            return Replay::CantTell(format!("the server was short of memory as it ran: {}", e.message()));
        }
        if cut_short && matches!(e, holon_host::RunError::TimeBudget { .. }) {
            return Replay::CantTell("the replay's time ran out before it finished".into());
        }
        let why = match e {
            holon_host::RunError::StepBudget { .. }
            | holon_host::RunError::FuelBudget { .. }
            | holon_host::RunError::MemoryBudget { .. }
            | holon_host::RunError::RequestBudget { .. }
            | holon_host::RunError::TimeBudget { .. } => {
                format!("it doesn't finish within what the deployment is granted: {}", e.message())
            }
            _ => format!("it doesn't finish: {}", e.message()),
        };
        return Replay::Differs(vec![why], now_status(status));
    }

    let mut differs = Vec::new();
    if let (Some(now), Some(then)) = (status, entry.status)
        && now != then
    {
        differs.push(format!("it answers {now} where it answered {then}"));
    }
    let outputs: Vec<Option<SValue>> =
        outcome.outputs.iter().map(|o| o.as_ref().map(|v| marks.write(SValue::from_core(&folders::loggable_output(v))))).collect();
    if json(&outputs) != json(&entry.outputs) {
        differs.push(if entry.kind == "request" { "its answer".into() } else { "its outputs".into() });
    }
    let performed: Vec<PerformedEntry> = host
        .performed
        .iter()
        .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: marks.write(SValue::from_core(&folders::loggable(c, p))) })
        .collect();
    if json(&performed) != json(&entry.performed) {
        differs.push(effects(&entry.performed, &performed));
    }
    let progress: Vec<PerformedEntry> = if outcome.waiting.is_empty() { Vec::new() } else { channels::settled_sends(&outcome.outputs) }
        .iter()
        .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: marks.write(SValue::from_core(p)) })
        .collect();
    if json(&progress) != json(&entry.progress) {
        differs.push("the packs it sends as it waits".into());
    }
    let mut waiting: Vec<String> = outcome.waiting.iter().map(|w| w.name.clone()).collect();
    let mut parked = entry.parked.clone();
    waiting.sort();
    parked.sort();
    if waiting != parked {
        differs.push(match (waiting.is_empty(), parked.is_empty()) {
            (true, _) => "it doesn't wait where the run waited".into(),
            (_, true) => format!("it waits, at {}, where the run finished", waiting.join(", ")),
            _ => format!("it waits at {} where the run waited at {}", waiting.join(", "), parked.join(", ")),
        });
    }
    match differs.is_empty() {
        true => Replay::Same(now_status(status)),
        false => Replay::Differs(differs, now_status(status)),
    }
}

/// A lent capability as a person reads it.
fn lent_name(cap: &Capability) -> String {
    match cap {
        Capability::Clock => "the clock".into(),
        Capability::Random => "a random number".into(),
        Capability::FileArrival(folder) => format!("a new file in {folder}"),
        Capability::MailArrival(mail) => format!("a mail to {mail}"),
        Capability::FeedEvent(feed) => format!("an event of {feed}"),
        Capability::QueueMessage(queue) => format!("a message of {queue}"),
        other => format!("{other:?}"),
    }
}

/// How the effects differ, by kind: `the effects it performs (StateWrite("n") 2 where 1)`.
fn effects(then: &[PerformedEntry], now: &[PerformedEntry]) -> String {
    let count = |es: &[PerformedEntry]| {
        let mut by: BTreeMap<String, usize> = BTreeMap::new();
        for e in es {
            *by.entry(e.capability.clone()).or_default() += 1;
        }
        by
    };
    let (then, now) = (count(then), count(now));
    let changed: Vec<String> = then
        .keys()
        .chain(now.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|k| then.get(*k) != now.get(*k))
        .map(|k| format!("{k} {} where {}", now.get(k).copied().unwrap_or(0), then.get(k).copied().unwrap_or(0)))
        .collect();
    match changed.is_empty() {
        true => "the effects it performs".into(),
        false => format!("the effects it performs ({})", changed.join(", ")),
    }
}

/// The network, for a replay: what the run was answered to the very same request, each answer
/// once while another for the same request is left; a request it never made is missed.
struct ByRequest<'a> {
    answers: Vec<(Value, Value)>,
    used: Mutex<Vec<bool>>,
    missed: Mutex<Vec<String>>,
    /// The hosts granted plain HTTP, where a missed request would go as `http://`.
    plain_http: &'a BTreeSet<String>,
}

impl Performer for ByRequest<'_> {
    fn perform(&self, request: &Value, limits: &Limits) -> Value {
        let mut used = self.used.lock().unwrap();
        let same: Vec<usize> = (0..self.answers.len()).filter(|&i| self.answers[i].0 == *request).collect();
        match same.iter().find(|&&i| !used[i]).or(same.last()) {
            Some(&i) => {
                used[i] = true;
                self.answers[i].1.clone()
            }
            None => {
                self.missed.lock().unwrap().push(asked(request, self.plain_http));
                Offline.perform(request, limits)
            }
        }
    }
}

/// A request as a replay names it: its method and its address as it would go on the wire (plain
/// HTTP to a host granted it, where the request says `https://`), not its headers or body.
fn asked(request: &Value, plain_http: &BTreeSet<String>) -> String {
    let Value::Record(r) = request else { return "a request".into() };
    let text = |k: &str| match r.get(k) {
        Some(Value::Text(t)) if k == "url" => Some(crate::outbound::on_the_wire(t, plain_http)),
        Some(Value::Text(t)) => Some(t.clone()),
        _ => None,
    };
    match (text("method"), text("url")) {
        (Some(m), Some(u)) => format!("{m} {}", u.split('?').next().unwrap_or(&u)),
        (None, Some(u)) => format!("GET {}", u.split('?').next().unwrap_or(&u)),
        _ => "a request".into(),
    }
}

/// A replay's reads, events and tokens: a read answered by the run's read of the same request,
/// wherever it is asked from; one the run never made, or whose answer isn't here, is missed.
struct ByRead<'a> {
    reads: Vec<(Value, Result<Option<Value>, String>)>,
    events: BTreeMap<String, Value>,
    marks: Marks,
    missed: Vec<String>,
    /// The replay's network: a request it missed ends the run too.
    sent: &'a ByRequest<'a>,
}

impl Waits for ByRead<'_> {
    /// Before a round sends anything: a run that has asked what its run didn't (a request, a
    /// read) can't be told whatever it does next, so it goes no further. It ends as a run
    /// overtaken does, sending nothing more; `replay_one` reads what it missed first.
    fn claim(&mut self) -> holon_host::Claim {
        match self.missed.is_empty() && self.sent.missed.lock().unwrap().is_empty() {
            true => holon_host::Claim::Held,
            false => holon_host::Claim::Overtaken,
        }
    }
    fn read(&mut self, _path: &[u32], request: &Value) -> Option<Value> {
        match self.reads.iter().find(|(r, _)| r == request) {
            Some((_, Ok(answer))) => answer.clone(),
            Some((_, Err(why))) => {
                self.missed.push(why.clone());
                None
            }
            None => {
                self.missed.push("it reads what the run didn't: what it would find can't be told offline".into());
                None
            }
        }
    }
    fn event(&self, name: &str) -> Option<Value> {
        self.events.get(name).cloned()
    }
    fn token(&mut self, name: &str) -> String {
        self.marks.of("token", name)
    }
}

/// Watches a replay for a node that computes with a placeholder (a secret's or a token's): a
/// pure leaf fed one is computed twice, with it and with another, and if what it gives depends on
/// which, it computed with the value, and what it gave isn't what the run computed with the
/// value. One that only passes it along, or picks past it, gives the same and stands. A leaf that
/// asks the world (a request, a read) is judged by what it is answered, not computed here.
///
/// What it computes stands in for the node, so the round's own budget never counts it: it
/// computes on a budget of its own, made afresh each round (`each_round`: twice the round's steps,
/// fuel and memory, as it computes each node twice). Past it, it computes nothing more
/// (`too_much`).
struct Fed<'a, R> {
    marks: &'a Marks,
    other: Marks,
    resolver: &'a R,
    consumed: bool,
    each_round: (u64, u64, u64),
    budget: holon_core::Budgeted,
    too_much: bool,
}

impl<'a, R> Fed<'a, R> {
    fn new(marks: &'a Marks, resolver: &'a R, limits: &Limits) -> Self {
        let each_round = (limits.max_steps.saturating_mul(2), limits.max_fuel.saturating_mul(2), limits.max_memory.saturating_mul(2));
        Fed { marks, other: Marks::new(), resolver, consumed: false, each_round, budget: budget(each_round), too_much: false }
    }
}

/// A budget of `steps`, `fuel` and `memory`, the memory from what the thread holds now.
fn budget((steps, fuel, memory): (u64, u64, u64)) -> holon_core::Budgeted {
    holon_core::Budgeted::new(steps).with_fuel(fuel).with_memory(memory)
}

impl<R: holon_core::Resolver> Observer for Fed<'_, R> {
    fn round(&mut self, _n: usize) {
        self.budget = budget(self.each_round);
    }
    fn asserted(&mut self, _path: &[u32], unit: &holon_core::Unit, inputs: &[Option<&Value>]) -> Option<Vec<Option<Value>>> {
        if self.too_much || matches!(unit.body, holon_core::UnitBody::Composite { .. }) || !inputs.iter().flatten().any(|v| self.marks.carried(v)) {
            return None;
        }
        if !capabilities(unit, self.resolver).is_empty() {
            return None;
        }
        let (one, two) = (self.marks.start.as_str(), self.other.start.as_str());
        let given: Vec<Option<Value>> = inputs.iter().map(|v| v.cloned()).collect();
        let swapped: Vec<Option<Value>> = inputs.iter().map(|v| v.map(|v| swap(v, one, two))).collect();
        let a = holon_core::eval_observed(unit, &given, self.resolver, &mut self.budget);
        let b = holon_core::eval_observed(unit, &swapped, self.resolver, &mut self.budget);
        if self.budget.exhausted || self.budget.out_of_fuel || self.budget.out_of_memory {
            self.too_much = true;
            return None;
        }
        if a.iter().zip(&b).all(|(a, b)| *a == b.as_ref().map(|b| swap(b, two, one))) {
            return Some(a.into_vec());
        }
        self.consumed = true;
        None
    }
}

/// `v` with each `from` in its texts (a record's keys too) and bytes as `to`.
fn swap(v: &Value, from: &str, to: &str) -> Value {
    match v {
        Value::Text(t) => Value::Text(t.replace(from, to)),
        Value::List(items) => Value::List(items.iter().map(|v| swap(v, from, to)).collect()),
        Value::Record(fields) => Value::Record(fields.iter().map(|(k, v)| (k.replace(from, to), swap(v, from, to))).collect()),
        Value::Sum { arm, payload } => Value::Sum { arm: *arm, payload: Box::new(swap(payload, from, to)) },
        Value::Effect { sink, payload } => Value::Effect { sink: sink.clone(), payload: Box::new(swap(payload, from, to)) },
        Value::Bytes(b) => {
            let (from, to) = (from.as_bytes(), to.as_bytes());
            let mut out = Vec::with_capacity(b.len());
            let mut i = 0;
            while i < b.len() {
                if b[i..].starts_with(from) {
                    out.extend_from_slice(to);
                    i += from.len();
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            Value::Bytes(out)
        }
        Value::Number(_) | Value::Boolean(_) => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards: a replayed run that asks what its run didn't, a request or a read, goes no further:
    /// `ByRead::claim`, which the host asks before each round it sends, ends it as an overtaken
    /// run ends, sending nothing more. Until then, it is held.
    /// Why: a missed request was answered offline and the run went on, round after round, each
    /// computing as much as the grant allows, for a run already known to be "can't tell".
    /// If it fails: `ByRead::claim` holds a run that missed a request (the row "request") or a
    /// read ("read"), or ends one that missed nothing ("answered").
    #[test]
    fn a_replayed_run_goes_no_further_than_what_it_missed() {
        let get = |url: &str| Value::Record([("method".to_string(), Value::Text("GET".into())), ("url".to_string(), Value::Text(url.into()))].into());
        let limits = super::super::rerun_limits();
        for (row, asks, reads) in [("answered", "https://api.example.com/a", false), ("request", "https://api.example.com/b", false), ("read", "https://api.example.com/a", true)] {
            let sent = ByRequest { answers: vec![(get("https://api.example.com/a"), Value::Number(200.0))], used: Mutex::new(vec![false]), missed: Mutex::new(Vec::new()), plain_http: &BTreeSet::new() };
            let mut waits = ByRead { reads: Vec::new(), events: BTreeMap::new(), marks: Marks::new(), missed: Vec::new(), sent: &sent };
            assert!(matches!(waits.claim(), holon_host::Claim::Held), "{row}: before it asks");
            sent.perform(&get(asks), &limits);
            if reads {
                assert_eq!(waits.read(&[0], &get("a read the run didn't make")), None, "{row}");
            }
            let missed = row != "answered";
            assert_eq!(matches!(waits.claim(), holon_host::Claim::Overtaken), missed, "{row}");
        }
    }
}
