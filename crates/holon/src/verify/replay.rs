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
//! reads a clock or a random the run didn't, or one that failed then.

use super::*;

/// What `GET /deployments/<slug>/replay` answers.
#[derive(Serialize, Debug)]
pub(crate) struct Replayed {
    /// The version replayed (hex).
    pub(crate) unit: String,
    pub(crate) replayed: usize,
    pub(crate) same: usize,
    pub(crate) differ: usize,
    pub(crate) cant_tell: usize,
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

/// `GET /deployments/<slug>/replay?unit=<hex>[&runs=N]`: the version `unit` (staged, `POST
/// /unit`, or a proposal's) replayed on the deployment's last `N` runs. Changes nothing.
pub(crate) fn handle_get_replay(store: &SharedStore, folders: &folders::Folders, deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, request: &Request) -> Response {
    let Some(unit) = request.get_param("unit").as_deref().and_then(parse_hash) else {
        return err(400, "unit: the version to replay, as its hash (hex), staged with POST /unit");
    };
    let runs = match request.get_param("runs").map(|v| v.parse::<usize>()) {
        None => RUNS,
        Some(Ok(n)) if (1..=MOST_RUNS).contains(&n) => n,
        Some(_) => return err(400, format!("runs: how many recent runs, 1 to {MOST_RUNS}")),
    };
    let (log, redactor) = match deployments.lock().unwrap().get(slug) {
        Some(dep) => (std::sync::Arc::clone(&dep.log), crate::secrets::Redactor::of(&dep.secrets)),
        None => return err(404, format!("no deployment {slug:?}")),
    };
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
    let lines = log.with(|log| log.lines());
    // The file texts its runs read, as an export ships them (none holding a secret).
    let export = match export_of(store, folders, Some(&redactor), slug, lines, String::new(), Vec::new(), String::new(), u64::MAX) {
        Ok(export) => export,
        Err(e) => return err(500, e),
    };
    Response::json(&replay(&export, unit, &resolver, runs))
}

/// `unit` replayed on the last `runs` runs among `export`'s lines that ran.
pub(crate) fn replay(export: &Export, unit: Hash, resolver: &impl holon_core::Resolver, runs: usize) -> Replayed {
    let entries: Vec<LogEntry> = export.lines.iter().filter_map(|l| crate::wire::read_json::<LogEntry>(l.as_bytes()).ok()).collect();
    let files = BTreeMap::new();
    let texts = Texts { blobs: &export.blobs, blob_bytes: &export.blob_bytes, files: &files };
    let marks = Marks::new();
    let mut out = Replayed { unit: to_hex(&unit), replayed: 0, same: 0, differ: 0, cant_tell: 0, runs: Vec::new() };
    let ran: Vec<usize> = (0..entries.len()).filter(|&i| !entries[i].run_id.is_empty()).collect();
    for &i in ran.iter().rev().take(runs) {
        let entry = &entries[i];
        let earlier: Vec<&LogEntry> = entries[..i].iter().filter(|e| e.run_id == entry.run_id).collect();
        let (outcome, why, now_status) = match replay_one(entry, &earlier, &texts, unit, resolver, &marks) {
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

fn replay_one(entry: &LogEntry, earlier: &[&LogEntry], texts: &Texts, unit: Hash, resolver: &impl holon_core::Resolver, marks: &Marks) -> Replay {
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
    let performer = ByRequest { used: Mutex::new(vec![false; answers.len()]), answers, missed: Mutex::new(Vec::new()) };
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
    };
    let limits = Limits { run_budget: Duration::from_secs(3600), max_requests: usize::MAX, ..Limits::clock_run() };
    let mut host = RecordingHost::new();
    let mut observer = Fed { marks, other: Marks::new(), resolver, consumed: false };
    let outcome = run_rounds(root, &inputs, &sourced, resolver, &mut host, &performer, &limits, &mut observer, &mut waits, &[]);

    if let Some(missed) = performer.missed.into_inner().unwrap().into_iter().next() {
        return Replay::CantTell(format!("it sends {missed}, which the run didn't: what it would be answered can't be told offline"));
    }
    if let Some(missed) = waits.missed.into_iter().next() {
        return Replay::CantTell(missed);
    }
    if observer.consumed {
        return Replay::CantTell("it computes with a secret, a token or a tainted value, which the log never holds".into());
    }
    // As `rerun` has it: the server refuses a write of a secret's value.
    let keeps = |c: &Capability| matches!(c, Capability::StateWrite(_) | Capability::CollectionWrite(_) | Capability::DbWrite(_) | Capability::ChannelSend(_) | Capability::FileWrite(_));
    if host.performed.iter().any(|(c, p)| keeps(c) && marks.carries_secret(p)) {
        return Replay::CantTell("it writes a secret's value, which the server refuses".into());
    }
    let status = (entry.kind == "request").then(|| data::respond(outcome.outputs.first().and_then(Option::as_ref)).status);
    if let Some(e) = &outcome.error {
        return Replay::Differs(vec![format!("it doesn't finish: {e:?}")], now_status(status));
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
struct ByRequest {
    answers: Vec<(Value, Value)>,
    used: Mutex<Vec<bool>>,
    missed: Mutex<Vec<String>>,
}

impl Performer for ByRequest {
    fn perform(&self, request: &Value, limits: &Limits) -> Value {
        let mut used = self.used.lock().unwrap();
        let same: Vec<usize> = (0..self.answers.len()).filter(|&i| self.answers[i].0 == *request).collect();
        match same.iter().find(|&&i| !used[i]).or(same.last()) {
            Some(&i) => {
                used[i] = true;
                self.answers[i].1.clone()
            }
            None => {
                self.missed.lock().unwrap().push(asked(request));
                Offline.perform(request, limits)
            }
        }
    }
}

/// A request as a replay names it: its method and its address, not its headers or body.
fn asked(request: &Value) -> String {
    let Value::Record(r) = request else { return "a request".into() };
    let text = |k: &str| match r.get(k) {
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
struct ByRead {
    reads: Vec<(Value, Result<Option<Value>, String>)>,
    events: BTreeMap<String, Value>,
    marks: Marks,
    missed: Vec<String>,
}

impl Waits for ByRead {
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
struct Fed<'a, R> {
    marks: &'a Marks,
    other: Marks,
    resolver: &'a R,
    consumed: bool,
}

impl<R: holon_core::Resolver> Observer for Fed<'_, R> {
    fn asserted(&mut self, _path: &[u32], unit: &holon_core::Unit, inputs: &[Option<&Value>]) -> Option<Vec<Option<Value>>> {
        if matches!(unit.body, holon_core::UnitBody::Composite { .. }) || !inputs.iter().flatten().any(|v| self.marks.carried(v)) {
            return None;
        }
        if !capabilities(unit, self.resolver).is_empty() {
            return None;
        }
        let (one, two) = (self.marks.start.as_str(), self.other.start.as_str());
        let given: Vec<Option<Value>> = inputs.iter().map(|v| v.cloned()).collect();
        let swapped: Vec<Option<Value>> = inputs.iter().map(|v| v.map(|v| swap(v, one, two))).collect();
        let a = holon_core::eval(unit, &given, self.resolver);
        let b = holon_core::eval(unit, &swapped, self.resolver);
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
