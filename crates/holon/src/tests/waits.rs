//! What an action that waits does, and what its caller is answered, against what its text says.
//!
//! An action that waits (`await event(…)`, `await approval(…)`) reads as JavaScript's async
//! function: its body runs to its first `await` and hands its caller a promise; the rest runs once
//! the wait is answered. Holon keeps that order and says more (docs/LANGUAGE.md §5.4): the caller
//! is answered `{run, waiting}`, a token for each event it waits at, as the run parks; a return in
//! the row right above the wait answers `{answer, run}` and nothing waits; a test there with no
//! value ends the run, answering nothing; a wait that runs out gives `{ expired: true }`, so what
//! follows is made then too, but for what uses a value it doesn't give (§6.3).
//!
//! The grid crosses what stands above the wait (`ABOVE`, each with the inputs that make its tests
//! hold, not hold and have no value), the kind of wait (`WAITS`), what stands past it (`PAST`,
//! each using what the wait gave and not), what starts the action (`STARTERS`) and how the wait
//! ends (`ENDS`). Each run is read from the server: what the caller is answered, whether the run is
//! listed as waiting, what it made before the wait ended and what after. It is held to the same
//! run read as a flow reads it (`reads`, with JavaScript's reading beside it): where the two
//! readings differ is one of Holon's own rules, counted; where the flow differs from its reading,
//! the test fails. (Two debts of reached were owed here until C6, a wait past a return that
//! answered, or past a test with no value, still waiting: a wait now takes a trigger, tied to
//! where its line is reached.)

use super::semantics::{grid_state_with, Block};
use super::*;

/// What stands above the wait: nothing, or the row of returns right above it (§5.4).
const ABOVE: &[(&str, &str)] = &[
    ("nothing", ""),
    ("a return", "  if (input.stop) return \"stopped\"\n"),
    ("a bare return", "  if (input.stop) return\n"),
    ("two returns", "  if (input.stop) return \"stopped\"\n  if (input.n > 10) return \"big\"\n"),
];

/// What the action is started with: its tests hold, don't, have no value, or the second holds.
const INPUTS: &[(&str, &str)] = &[
    ("a test holds", r#"{"stop":true,"n":1}"#),
    ("no test holds", r#"{"stop":false,"n":1}"#),
    ("a test has no value", r#"{"n":1}"#),
    ("the second test holds", r#"{"stop":false,"n":50}"#),
];

/// The inputs (of `INPUTS`) each of `ABOVE` is started with.
fn inputs_for(above: &str) -> &'static [usize] {
    match above {
        "nothing" => &[1],
        "two returns" => &[0, 1, 2, 3],
        _ => &[0, 1, 2],
    }
}

/// A kind of wait: its lines, the value past it that uses what it gave, the points it waits at,
/// whether an event's token is handed out for each, and whether what it gives on expiry holds that
/// value (an approval's `by` does, an event's `method` doesn't).
struct Wait {
    name: &'static str,
    text: &'static str,
    value: &'static str,
    points: &'static [&'static str],
    tokens: bool,
    kept_on_expiry: bool,
}

const WAITS: &[Wait] = &[
    Wait { name: "an event", text: "  const ok = await event(\"ok\", { expires: \"1h\" })\n", value: "ok.method", points: &["ok"], tokens: true, kept_on_expiry: false },
    Wait { name: "an approval", text: "  const ok = await approval(\"ok\", { expires: \"1h\" })\n", value: "JSON.stringify(ok.by)", points: &["ok"], tokens: false, kept_on_expiry: true },
    Wait {
        name: "two events",
        text: "  const ok = await event(\"ok\", { expires: \"1h\" })\n  const ok2 = await event(\"ok2\", { expires: \"1h\" })\n",
        value: "ok.method",
        points: &["ok", "ok2"],
        tokens: true,
        kept_on_expiry: false,
    },
];

/// What stands past the wait: its mark, the statement using what the wait gave (`{V}`), and the
/// same not using it. A statement that is a `return` is the action's last.
const PAST: &[(&str, &str, &str)] = &[
    ("", "", ""),
    ("w", "state.x = {V}", "state.x = \"v\""),
    ("p", "marks.put(\"k\", {V})", "marks.put(\"k\", \"v\")"),
    ("c", "updates.send({ by: {V} })", "updates.send({ n: 1 })"),
    ("m", "ops.send({ to: \"a@acme.com\", subject: {V}, text: \"hi\" })", "ops.send({ to: \"a@acme.com\", subject: \"s\", text: \"hi\" })"),
    ("q", "jobs.send({ data: { by: {V} } })", "jobs.send({ data: { n: 1 } })"),
    ("r", "const sent = await api.post(\"/hit\", { body: {V} })", "const sent = await api.post(\"/hit\", { body: \"x\" })"),
    ("s", "ping({ by: {V} })", "ping({})"),
    ("n", "return notes[{V}] ?? \"none\"", "return notes[\"k\"] ?? \"none\""),
];

/// What starts the action.
const STARTERS: &[&str] = &["a route", "a function", "a list's function", "a clock", "a queue", "by hand"];

/// How the wait ends: answered (each event's link opened, the approval approved), run out, or not.
const ENDS: &[&str] = &["answered", "expired", "never"];

/// One action of the grid, and what starts it.
#[derive(Clone, Copy)]
struct Case {
    above: &'static str,
    wait: &'static Wait,
    past: &'static str,
    uses: bool,
    starter: &'static str,
}

impl Case {
    fn at(&self) -> String {
        format!("{} | {} | {}{} | {}", self.above, self.wait.name, if self.past.is_empty() { "nothing" } else { self.past }, if self.uses { " using it" } else { "" }, self.starter)
    }
}

fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    for (above, _) in ABOVE {
        for wait in WAITS {
            for (past, _, _) in PAST {
                for uses in if past.is_empty() { &[false][..] } else { &[true, false][..] } {
                    for starter in STARTERS {
                        out.push(Case { above, wait, past, uses: *uses, starter });
                    }
                }
            }
        }
    }
    out
}

/// The queue a worker's queue-started flows listen to.
fn starts(w: usize) -> String {
    format!("w{w}starts")
}

/// The flow of `case`, for worker `w`.
fn flow_text(case: &Case, w: usize, host: &str) -> String {
    let (above, wait) = (ABOVE.iter().find(|(a, _)| *a == case.above).unwrap().1, case.wait);
    let (_, uses, plain) = PAST.iter().find(|(m, ..)| *m == case.past).unwrap();
    let past = if case.uses { uses.replace("{V}", wait.value) } else { plain.to_string() };
    let last = if past.starts_with("return") { String::new() } else { "  return \"done\"\n".to_string() };
    let past = if past.is_empty() { String::new() } else { format!("  {past}\n") };
    let r#async = "async ";
    let mut body = format!("\nexport const pay = action({async}(input) => {{\n{above}{}{past}{last}}})\n", wait.text);
    if case.past == "s" {
        body = format!("\nexport const ping = action((x) => ++state.pinged)\n{body}");
    }
    let starter = match case.starter {
        "a route" => "\nroute.post(\"/go\", (req) => {\n  const started = pay(req.json)\n  return started\n})\n".to_string(),
        "a function" => "\nfunction go(x) {\n  return pay(x)\n}\n\nroute.post(\"/go\", (req) => {\n  const started = go(req.json)\n  return started\n})\n".to_string(),
        "a list's function" => "\nroute.post(\"/go\", (req) => {\n  const started = [req.json].map((x) => pay(x))\n  return started\n})\n".to_string(),
        "a clock" => "\nroute.post(\"/set\", (req) => {\n  state.input = req.json\n  return \"ok\"\n})\n\nevery(\"1s\", () => {\n  pay(now > 0 ? state.input : undefined)\n})\n".to_string(),
        "a queue" => format!("\non.queue({q}, (m) => {{\n  pay(m.data)\n}})\n", q = starts(w)),
        _ => String::new(),
    };
    body.push_str(&starter);
    if wait.tokens {
        for p in wait.points {
            body.push_str(&format!("\nroute.get(\"/{p}/:token\", () => \"resumed\", {{ resumes: \"{p}\" }})\n"));
        }
    }
    let uses = |what: &str| body.contains(what);
    let mut holon = vec!["action"];
    holon.push(if wait.tokens { "event" } else { "approval" });
    if case.starter == "a clock" {
        holon.push("every");
    }
    holon.push("flow");
    if case.starter == "a queue" {
        holon.push("on");
    }
    if uses("route.") {
        holon.push("route");
    }
    holon.sort();
    let mut text = format!("import {{ {} }} from \"holon\"\n", holon.join(", "));
    if uses("state.") {
        text.push_str("import state from \"state\"\n");
    }
    for (name, source) in [("marks", "collection:marks"), ("notes", "collection:notes"), ("updates", "channel:updates"), ("ops", "mail:ops"), ("jobs", "queue:jobs")] {
        if uses(&format!("{name}.")) || uses(&format!("{name}[")) {
            text.push_str(&format!("import {name} from \"{source}\"\n"));
        }
    }
    if uses("api.") {
        text.push_str(&format!("import api from \"host:{host}\"\n"));
    }
    if case.starter == "a queue" {
        text.push_str(&format!("import {q} from \"queue:{q}\"\n", q = starts(w)));
    }
    if case.starter == "a clock" {
        text.push_str("import { now } from \"holon:clock\"\n");
    }
    let port = if uses("route.") { format!(", {{ port: {} }}", 26000 + w) } else { String::new() };
    text.push_str(&format!("\nflow(\"waits-{w}\"{port})\n{body}"));
    text
}

/// What a run did, as read from the server: what its caller was answered (`no caller` for a clock
/// or a queue), whether it was listed as waiting, and what it made before its wait ended and after.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    caller: String,
    listed: bool,
    before: String,
    after: String,
}

impl std::fmt::Display for Seen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; {}; before: {:?}; after: {:?}", self.caller, if self.listed { "listed waiting" } else { "not listed" }, self.before, self.after)
    }
}

/// Where the row above the wait leaves a run: answered by a return (its value, none for a bare
/// one), ended by a test with no value, or on to the wait.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Row {
    Returns(Option<&'static str>),
    Ends,
    Waits,
}

/// The row's outcome on `input`, as a flow reads it or (`js`) as JavaScript does: there, a test
/// with no value is false and the run goes on.
fn row(above: &str, input: &str, js: bool) -> Row {
    let v: serde_json::Value = serde_json::from_str(input).unwrap();
    let (stop, n) = (v["stop"].as_bool(), v["n"].as_f64());
    let first = |value: Option<&'static str>| match stop {
        Some(true) => Some(Row::Returns(value)),
        Some(false) => None,
        None => (!js).then_some(Row::Ends),
    };
    match above {
        "nothing" => Row::Waits,
        "a return" => first(Some("\"stopped\"")).unwrap_or(Row::Waits),
        "a bare return" => first(None).unwrap_or(Row::Waits),
        "two returns" => first(Some("\"stopped\"")).unwrap_or(if n.is_some_and(|n| n > 10.0) { Row::Returns(Some("\"big\"")) } else { Row::Waits }),
        other => panic!("{other}"),
    }
}

/// What a run of `case` on `input` ended `end` does, read as a flow reads it, or (`js`) as
/// JavaScript does: the action's body to its wait, a promise its caller is handed, the rest once
/// the wait gives something. Its caller is answered as §5.4 says, the same either way: JavaScript
/// has no token, and a promise pending is a run waiting. A token is handed out for each event, none
/// for an approval; a run started by hand (`POST /trigger/…`) is answered that it waits, with no
/// token, as that route answers every run it starts.
fn reads(case: &Case, input: &str, end: &str, js: bool) -> Seen {
    let row = row(case.above, input, js);
    let caller = match (case.starter, row) {
        ("a clock" | "a queue", _) => "no caller".to_string(),
        (_, Row::Returns(Some(v))) => format!("answers {v}"),
        (_, Row::Returns(None) | Row::Ends) => "answers nothing".to_string(),
        (_, Row::Waits) if case.starter == "by hand" || !case.wait.tokens => "waiting".to_string(),
        (_, Row::Waits) => format!("waiting {}", case.wait.points.iter().map(|p| format!("{p}(token)")).collect::<Vec<_>>().join(" ")),
    };
    let waits = row == Row::Waits;
    // On expiry the wait gives `{ expired: true }` (an approval's with `by`): a statement using a
    // value it doesn't give is made in JavaScript, with `undefined`, and not in a flow (§6.3).
    let made = match end {
        "never" => false,
        "expired" => js || !case.uses || case.wait.kept_on_expiry,
        _ => true,
    };
    Seen { caller, listed: waits, before: String::new(), after: if waits && made { case.past.to_string() } else { String::new() } }
}

/// What the run's caller was answered, as `reads` says it. A start whose run answered nothing is
/// answered that the action answered 404 (by hand: `not found`), the one way a start says it.
fn caller_of(status: u16, body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return format!("answers {status} {body}") };
    let v = if let Some(first) = v.as_array().and_then(|l| l.first()) { first.clone() } else { v };
    if v["waiting"] == serde_json::Value::Bool(true) {
        return "waiting".to_string();
    }
    if let Some(waiting) = v["waiting"].as_object() {
        let points: Vec<String> = waiting.iter().map(|(p, t)| if t.as_str().is_some_and(|t| t.starts_with("rt_")) { format!(" {p}(token)") } else { format!(" {p}") }).collect();
        return format!("waiting{}", points.concat());
    }
    let nothing = |e: &str, message: &str| (e == "failed" && message == "the action answered 404") || (e == "not found" && message.is_empty());
    if let Some(e) = v["error"].as_str() {
        if nothing(e, v["message"].as_str().unwrap_or("")) {
            return "answers nothing".to_string();
        }
        return format!("error {e}: {}", v["message"].as_str().unwrap_or(""));
    }
    if v.get("answer").is_some() {
        return format!("answers {}", v["answer"]);
    }
    if v.get("run").is_some() {
        return "answers nothing".to_string();
    }
    format!("answers {status} {body}")
}

/// The marks of the log entries `slug` gained after `seen`, which moves on: a write of `x` (`w`),
/// a put (`p`), a send (`c`), a mail (`m`), a queued message (`q`), a request (`r`), a run of
/// `ping` started (`s`), a read of `notes` (`n`). The `POST /set` that sets a clock's input isn't
/// the run's.
fn gained(state: &AppState, slug: &str, seen: &mut Option<u64>) -> String {
    let after = seen.map(|seq| format!("after={seq}&")).unwrap_or_default();
    let log = body_json(route(state, &req("GET", &format!("/deployments/{slug}/log?{after}limit=50"), vec![])));
    let mut marks = Vec::new();
    for e in log["entries"].as_array().unwrap() {
        *seen = (*seen).max(e["seq"].as_u64());
        if e["route"] == "POST /set" {
            continue;
        }
        if e["route"] == "action ping" {
            marks.push('s');
            continue;
        }
        for p in e["performed"].as_array().into_iter().flatten() {
            match p["capability"].as_str().unwrap_or("") {
                "StateWrite(\"x\")" => marks.push('w'),
                "CollectionWrite(\"marks\")" => marks.push('p'),
                "ChannelSend(\"updates\")" => marks.push('c'),
                "MailSend(\"ops\")" => marks.push('m'),
                "QueueSend(\"jobs\")" => marks.push('q'),
                other => panic!("an effect the grid doesn't know: {other}"),
            }
        }
        for r in e["reads"].as_array().into_iter().flatten() {
            if r["request"].to_string().contains("\"notes\"") {
                marks.push('n');
            }
        }
        if serde_json::to_string(&e["rounds"]).unwrap().contains("/hit\"") {
            marks.push('r');
        }
    }
    marks.sort();
    marks.into_iter().collect()
}

/// The runs `slug` lists as waiting.
fn waiting(state: &AppState, slug: &str) -> Vec<String> {
    let runs = body_json(route(state, &req("GET", &format!("/deployments/{slug}/runs"), vec![])));
    runs["runs"].as_array().into_iter().flatten().filter_map(|r| r["run_id"].as_str().map(str::to_string)).collect()
}

/// Every run waiting on `state` runs out. A pass of the server's expiry ends one point of a run
/// (a run waiting at two goes on waiting at the other), so it passes until nothing of `slug`
/// waits, as the server's own passes would.
fn expire_all(state: &AppState, slug: &str) {
    for _ in 0..4 {
        if waiting(state, slug).is_empty() {
            return;
        }
        durable::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 864_000).wait();
    }
    assert!(waiting(state, slug).is_empty(), "{slug}: a run still waits after four passes of expiry");
}

/// `run` of `slug`, waiting at the approval `ok`, approved by one person, as the decide route
/// approves it once it has found who decides (`durable::decide`, then the run resumed).
fn approve(state: &AppState, slug: &str, run: &str) {
    let holds = |_: Option<&str>| -> Option<crate::bindings::Bound> { Some(crate::bindings::Bound { binding: 0, access: 0 }) };
    let by = durable::Decider { who: "bob", kind: crate::accounts::Kind::Person, holds: &holds, via: None };
    let decision = durable::Decision { approve: true, slot: None, reason: None, note: None, at: epoch_now() };
    match durable::decide(&state.deployments, slug, run, "ok", &by, decision, &durable::Rules::of(state)) {
        Ok(durable::Outcome::Decides { claimed, event, .. }) => durable::resume_parked(&state.store, &state.deployments, &state.deployments_path, slug, claimed, "ok", event).unwrap_or_else(|e| panic!("{slug} {run}: {e}")),
        Ok(durable::Outcome::Waits { .. }) => panic!("{slug} {run}: one approval doesn't decide it"),
        Err((code, why)) => panic!("{slug} {run}: {code} {why}"),
    }
}

/// One run of `case`'s flow `slug` on `input`, its wait ended `end`, as the server did it.
fn run_one(state: &AppState, slug: &str, case: &Case, w: usize, input: &str, end: &str, seen: &mut Option<u64>) -> Seen {
    let caller = match case.starter {
        "a clock" => {
            let (status, said, _) = data_call(state, slug, "POST", "/set", input);
            assert_eq!(status, 200, "{said}");
            let r = route(state, &req("POST", &format!("/deployments/{slug}/tick"), br#"{"clock":"every 1s"}"#.to_vec()));
            let status = r.status_code;
            assert_eq!(status, 200, "tick: {}", String::from_utf8_lossy(&body_bytes(r)));
            "no caller".to_string()
        }
        "a queue" => {
            assert!(dequeue::fire_for_test(&state.store, &state.deployments, &state.deployments_path, slug, &starts(w), holon_core::parse_json(input).unwrap()), "{slug} didn't run");
            "no caller".to_string()
        }
        "by hand" => {
            let r = run_action_named(state, slug, "pay", serde_json::from_str(input).unwrap());
            let status = r.status_code;
            caller_of(status, &String::from_utf8_lossy(&body_bytes(r)))
        }
        _ => {
            let (status, said, _) = data_call(state, slug, "POST", "/go", input);
            caller_of(status, &said)
        }
    };
    let runs = waiting(state, slug);
    let before = gained(state, slug, seen);
    // A link that doesn't answer is said with what the run made after.
    let mut refused = String::new();
    for run in &runs {
        match end {
            "answered" if case.wait.tokens => {
                for p in case.wait.points {
                    let (status, said, _) = data_call(state, slug, "GET", &format!("/{p}/{}", durable::token_for(run, p)), "");
                    if status != 200 {
                        refused.push_str(&format!(" (its link at {p} answered {status}: {said})"));
                    }
                }
            }
            "answered" => approve(state, slug, run),
            _ => {}
        }
    }
    if end == "expired" {
        expire_all(state, slug);
    }
    let after = gained(state, slug, seen) + &refused;
    // What still waits runs out, unread: the next run starts with nothing waiting.
    expire_all(state, slug);
    gained(state, slug, seen);
    Seen { caller, listed: !runs.is_empty(), before, after }
}

/// Flows the reader takes and `lang::compile` refuses, as their steps written back as text read
/// as another flow: each debt, and the cases owed under it.
type Owes = fn(&Case) -> bool;
const ROUND_TRIP_DEBTS: &[(&str, Owes)] = &[
    // Struck by C11 (2026-11-25): "the printer writes a put past a wait, below a row of returns,
    // as another action" (108 flows). It wrote `marks.put(k, v)` as `marks[k] = v`, whose value
    // reads back held to the returns not taken; it writes the method where the value isn't held
    // (print.rs `EarlyAnswers::methods`).
];

/// A kind of finding: one of a flow's own rules shown.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    OwnRule(&'static str),
}

/// A flow's own rules where it differs from JavaScript here.
const NO_VALUE_ENDS: &str = "a test with no value above the wait ends the run, answering nothing (§5.4)";
const EXPIRED_GIVES_NOTHING: &str = "a value an expired wait doesn't give isn't there, so what uses it isn't made (§6.3)";

impl Kind {
    fn all() -> impl Iterator<Item = Kind> {
        [NO_VALUE_ENDS, EXPIRED_GIVES_NOTHING].into_iter().map(Kind::OwnRule)
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Kind::OwnRule(rule) => write!(f, "own rule: {rule}"),
        }
    }
}

/// The runs of `case`: each input of its row, each end. The grid takes about 45 seconds alone
/// (2026-11-23), so it runs whole with the other tests, at one size.
fn runs_of(case: &Case) -> Vec<(usize, &'static str)> {
    inputs_for(case.above).iter().flat_map(|&i| ENDS.iter().map(move |&e| (i, e))).collect()
}

/// **Guards:** an action that waits does what its text says, started from a route, a function, a
/// list's function, a clock, a queue or by hand: a return in the row above its wait answers its
/// caller `{answer, run}` and leaves nothing waiting; otherwise its caller is answered `{run,
/// waiting}` (a token for each event), the run is listed as waiting, and what stands past the wait
/// is made once the wait is answered or runs out, never before; a test with no value ends it.
/// Each run is held to the same run as a flow reads it; a difference from that fails.
///
/// **Why:** a wait parks the run where its graph's trigger says, not where its text's order does:
/// a statement past the wait tied to the wait alone asked for it in a run that already answered
/// (R162), and a send using the wait's value parked a run that returned, until the wait took a
/// trigger of its own (C6).
///
/// **If it fails:** a line names the case (what stands above the wait, the wait, what stands past
/// it, what started it), the input, how the wait ended, what the run did, and what it reads as.
#[test]
fn an_action_that_waits_does_what_its_text_says() {
    let sink = smtp::sink::start("PLAIN LOGIN");
    let sqs = sqs::stand_in::StandIn::start();
    sqs.make("jobs");
    let (host, _hits) = counting_server();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let all = cases();
    // What the language refuses is said at check: left out, and counted by its reason.
    let mut refusals: BTreeMap<String, usize> = BTreeMap::new();
    // A flow the reader takes and the round trip refuses (the canvas can't write it back as the
    // same steps) is a gap between the two translators: owed under a named debt, and run, deployed
    // by its hash with the reader alone, as the handler grid runs one.
    let (mut taken, mut round_trip_owed, mut unowed) = (Vec::new(), BTreeMap::<&str, usize>::new(), Vec::new());
    for case in &all {
        let text = flow_text(case, 0, &host);
        match holon_author::agent::plan(&text) {
            Ok(_) => taken.push((*case, false)),
            Err(e) if holon_author::lang::ts::compile(&text).is_ok() && holon_author::lang::compile(&text).is_err() => {
                taken.push((*case, true));
                match ROUND_TRIP_DEBTS.iter().find(|(_, owes)| owes(case)) {
                    Some((debt, _)) => *round_trip_owed.entry(debt).or_default() += 1,
                    None => unowed.push(format!("{} | {}", case.at(), e.lines().next().unwrap_or(""))),
                }
            }
            Err(e) => {
                let first = e.lines().next().unwrap_or("").to_string();
                *refusals.entry(format!("{} | {}", case.starter, first.split_once(": ").map_or(first.as_str(), |(_, why)| why).chars().take(160).collect::<String>())).or_default() += 1;
            }
        }
    }
    for (why, n) in &refusals {
        println!("  {n:5} refused: {why}");
    }
    for (debt, n) in &round_trip_owed {
        println!("  {n:5} flows the canvas can't write back, under \"{debt}\"");
    }
    assert!(unowed.is_empty(), "{} flows the canvas can't write back, under no debt:\n{}", unowed.len(), unowed.join("\n"));
    for (debt, _) in ROUND_TRIP_DEBTS {
        assert!(round_trip_owed.contains_key(debt), "\"{debt}\": the canvas writes these back now: strike it from ROUND_TRIP_DEBTS");
    }
    #[derive(Default)]
    struct Tally {
        runs: usize,
        as_javascript: usize,
        by_rule: BTreeMap<&'static str, usize>,
        wrong: Vec<String>,
        seen: BTreeMap<Kind, String>,
        lines: Vec<String>,
    }
    let parts: Vec<Vec<(Case, bool)>> = (0..workers).map(|w| taken.iter().skip(w).step_by(workers).copied().collect()).collect();
    let tallies: Vec<Tally> = std::thread::scope(|s| {
        let running: Vec<_> = parts
            .iter()
            .enumerate()
            .map(|(w, part)| {
                let (sink, sqs, host) = (&sink, &sqs, &host);
                s.spawn(move || {
                    let (state, dir) = grid_state_with(sink, sqs, Block::Queue, &[starts(w)]);
                    let mut t = Tally::default();
                    for (case, alone) in part {
                        let text = flow_text(case, w, host);
                        let slug = if *alone { deploy_read_alone(&state, &text, host) } else { deploy_text(&state, &text, host) };
                        let mut seen = None;
                        gained(&state, &slug, &mut seen);
                        for (i, end) in runs_of(case) {
                            let input = INPUTS[i].1;
                            let did = run_one(&state, &slug, case, w, input, end, &mut seen);
                            let (read, js) = (reads(case, input, end, false), reads(case, input, end, true));
                            t.runs += 1;
                            let at = || format!("{} | {} | {end}", case.at(), INPUTS[i].0);
                            t.lines.push(format!("{}\t{}\t{end}\t{did}\t{read}", case.at(), INPUTS[i].0));
                            let saw = |t: &mut Tally, kind: Kind| {
                                t.seen.entry(kind).or_insert_with(&at);
                            };
                            if did == read {
                                if did == js {
                                    t.as_javascript += 1;
                                } else {
                                    let rule = if row(case.above, input, true) != row(case.above, input, false) { NO_VALUE_ENDS } else { EXPIRED_GIVES_NOTHING };
                                    *t.by_rule.entry(rule).or_default() += 1;
                                    saw(&mut t, Kind::OwnRule(rule));
                                }
                                continue;
                            }
                            t.wrong.push(format!("{} | the flow did: {did} | it reads as: {read} | JavaScript: {js}", at()));
                        }
                        assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![])).status_code, 200);
                        assert_eq!(route(&state, &req("DELETE", &format!("/deployments/{slug}"), vec![])).status_code, 200);
                    }
                    let _ = std::fs::remove_dir_all(&dir);
                    t
                })
            })
            .collect();
        running.into_iter().map(|r| r.join().unwrap()).collect()
    });
    let mut all_t = Tally::default();
    for t in tallies {
        all_t.runs += t.runs;
        all_t.as_javascript += t.as_javascript;
        t.by_rule.into_iter().for_each(|(k, n)| *all_t.by_rule.entry(k).or_default() += n);
        all_t.wrong.extend(t.wrong);
        t.seen.into_iter().for_each(|(k, at)| {
            all_t.seen.entry(k).or_insert(at);
        });
        all_t.lines.extend(t.lines);
    }
    if let Some(path) = std::env::var_os("HOLON_WAITS_TSV") {
        all_t.lines.sort();
        std::fs::write(path, all_t.lines.join("\n") + "\n").unwrap();
    }
    let by_rule: usize = all_t.by_rule.values().sum();
    println!("waits: {} cases, {} the language takes, {} runs: {} as JavaScript, {by_rule} by a flow's own rule, {} wrong", all.len(), taken.len(), all_t.runs, all_t.as_javascript, all_t.wrong.len());
    for (rule, n) in &all_t.by_rule {
        println!("  {n:5} runs by the rule: {rule}");
    }
    for (kind, at) in &all_t.seen {
        println!("  kind: {kind} | first at {at}");
    }
    assert!(all_t.wrong.is_empty(), "{} runs don't do what their text says:\n{}", all_t.wrong.len(), all_t.wrong.join("\n"));
    // Every kind still shows: a rule that shows nowhere means the grid lost the run that showed it.
    let lost: Vec<String> = Kind::all().filter(|k| !all_t.seen.contains_key(k)).map(|kind| format!("{kind}: shows nowhere: strike it, or the grid lost the run that showed it")).collect();
    assert!(lost.is_empty(), "{} kinds of finding show nowhere:\n{}", lost.len(), lost.join("\n"));
}
