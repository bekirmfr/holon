//! What a run's record says beyond its steps, as the web console's run page says it
//! (`decisions.ts`, `flowFacts.ts`): who decided each approval it waited at, the secrets it was
//! given and where its requests took them (never a value: the log has none), what it read, and
//! whether a witness holds its line. Also what a flow is connected to (its feeds, queues and
//! channels), and a check of every run log.

use serde_json::Value;

use super::app::{Said, Tone};

/// A value as the log keeps it (`{"type": …, "value": …}`), as plain JSON: null when it isn't one.
pub(crate) fn plain(v: &Value) -> Value {
    serde_json::from_value::<crate::SValue>(v.clone())
        .ok()
        .and_then(|s| holon_core::stringify_json(&s.into_core()))
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or(Value::Null)
}

/// Names in a sentence: "ada", "ada and bob", "ada, bob and cy".
fn and(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn texts(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// Seconds since the epoch, as a person reads a time.
fn at_seconds(s: u64) -> String {
    crate::engines::time_text(s as i64 * 1_000_000).replace('T', " ")
}

/// The decision an approval's event is (plain), said: who approved, who declined and why, or that
/// it ran out. None for a value that isn't one (an event's request).
pub(crate) fn decision_said(v: &Value) -> Option<String> {
    let (Some(approved), Some(_)) = (v["approved"].as_bool(), v["by"].as_array()) else { return None };
    let names = texts(&v["by"]);
    if approved {
        return Some(format!("approved by {}", and(&names)));
    }
    if let Some(declined) = v["declined_by"].as_str() {
        return Some(format!("declined by {declined}: {}", v["reason"].as_str().unwrap_or_default()));
    }
    let before = if names.is_empty() { String::new() } else { format!(", approved by {} before", and(&names)) };
    Some(if v["expired"] == true { format!("ran out undecided{before}") } else { format!("not approved{before}") })
}

/// One decision (plain) in a line: when, who (an agent says so), what they decided as which step,
/// why, what they noted, and the binding that let them, with the access document's version it was
/// read in, which `holon access show --version N` prints for an auditor.
pub(crate) fn decided_line(d: &Value) -> String {
    let who = d["who"].as_str().unwrap_or("?");
    let who = if d["kind"] == "agent" { format!("{who} (an agent)") } else { who.to_string() };
    let slot = d["slot"].as_u64().unwrap_or(0);
    let step = match d["role"].as_str() {
        Some(role) => format!(" as {role}"),
        None if slot > 0 => format!(" as slot {slot}"),
        None => String::new(),
    };
    let why = d["reason"].as_str().map(|r| format!(": {r}")).unwrap_or_default();
    let noted = if d["note"].is_null() { String::new() } else { format!(" · noted {}", d["note"]) };
    let through = match (d["binding"].as_u64(), d["access"].as_u64()) {
        (Some(b), Some(a)) => format!(" · through binding {b} (holon access show --version {a})"),
        (Some(b), None) => format!(" · through binding {b}"),
        _ => String::new(),
    };
    let did = if d["approved"] == true { "approved" } else { "declined" };
    format!("{}: {who} {did}{step}{why}{noted}{through}", at_seconds(d["at"].as_u64().unwrap_or(0)))
}

/// The approvals a run's record says were decided (its `events`, each point's), each with who
/// decided it and how: what its Decided part shows.
pub(crate) fn decided(r: &Value) -> Vec<(Tone, String)> {
    let mut out = vec![];
    for (point, event) in r["events"].as_object().into_iter().flatten() {
        let v = plain(event);
        let Some(said) = decision_said(&v) else { continue };
        out.push((if v["approved"] == true { Tone::Good } else { Tone::Warn }, format!("\"{point}\": {said}")));
        out.extend(v["decisions"].as_array().into_iter().flatten().map(|d| (Tone::Plain, format!("  {}", decided_line(d)))));
    }
    out
}

/// A secret a run was given: its version and fingerprint, as its marks name them, where its value
/// is kept, the hosts its requests took it to (a request whose text holds its mark), and those
/// whose request was stopped before it left (refused, answered no status).
#[derive(Debug, PartialEq)]
pub(crate) struct SecretUse {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) print: Option<String>,
    pub(crate) kept: Option<String>,
    pub(crate) to: Vec<String>,
    pub(crate) stopped: Vec<String>,
}

fn host_of(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    (!scheme.is_empty()).then(|| rest.split(['/', '?', '#']).next().unwrap_or(rest))
}

pub(crate) fn secret_uses(r: &Value) -> Vec<SecretUse> {
    let exchanges: Vec<&Value> = r["rounds"].as_array().into_iter().flatten().filter_map(|x| x["exchanges"].as_array()).flatten().collect();
    r["secrets"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, mark)| {
            let words: Vec<&str> = mark.as_str().unwrap_or_default().split_whitespace().collect();
            let version = words.iter().find(|w| w.len() > 1 && w.starts_with('v') && w[1..].bytes().all(|b| b.is_ascii_digit())).map(|w| w.to_string());
            let print = words.iter().find(|w| w.starts_with('#')).map(|w| w.to_string());
            let c = &r["committed"][name];
            let kept = c["kind"].as_str().map(|k| {
                let whose = if c["server"] == true { ", the server's own" } else { "" };
                match k {
                    "local" => format!("sealed on the server{whose}"),
                    "reference" => format!("read from where it is kept{whose}"),
                    other => format!("{other}{whose}"),
                }
            });
            let (mut to, mut stopped): (Vec<String>, Vec<String>) = (vec![], vec![]);
            for x in &exchanges {
                if !x["request"].to_string().contains(&format!("‹secret:{name}")) {
                    continue;
                }
                let url = plain(&x["request"])["url"].as_str().unwrap_or_default().to_string();
                let host = host_of(&url).map(str::to_string).unwrap_or(if url.is_empty() { "a request".into() } else { url });
                let answered = plain(&x["response"]);
                let went = answered["status"].as_u64().is_some_and(|s| s > 0) || answered["error"].is_null();
                let into = if went { &mut to } else { &mut stopped };
                if !into.contains(&host) {
                    into.push(host);
                }
            }
            SecretUse { name: name.clone(), version, print, kept, to, stopped }
        })
        .collect()
}

/// The secrets a run read, a line each: what its Secrets part shows.
pub(crate) fn secret_lines(r: &Value) -> Vec<(Tone, String)> {
    secret_uses(r)
        .into_iter()
        .map(|s| {
            let given = [s.version, s.print].into_iter().flatten().collect::<Vec<_>>().join(" ");
            let kept = s.kept.map(|k| format!(", {k}")).unwrap_or_default();
            let mut went = if s.to.is_empty() { "sent nowhere".to_string() } else { format!("sent to {}", and(&s.to)) };
            if !s.stopped.is_empty() {
                went.push_str(&format!(" · a request to {} was stopped before it left", and(&s.stopped)));
            }
            (if s.to.is_empty() { Tone::Plain } else { Tone::Warn }, format!("{}  {given}{kept} · {went}", s.name))
        })
        .collect()
}

pub(crate) fn cut(s: &str, n: usize) -> String {
    if s.chars().count() > n { format!("{}…", s.chars().take(n - 1).collect::<String>()) } else { s.to_string() }
}

/// What a run read from outside its text: the clock, randomness, what came in (its `sourced`),
/// and each collection or file it read and what that answered (its `reads`).
pub(crate) fn read_lines(r: &Value) -> Vec<(Tone, String)> {
    let mut out = vec![];
    for pair in r["sourced"].as_array().into_iter().flatten() {
        let (Some(cap), Some(v)) = (pair[0].as_str(), pair.get(1)) else { continue };
        let what = match cap {
            "Clock" => "the clock",
            "Random" => "randomness",
            "NetworkIngress" => "what came in",
            other => other,
        };
        let said = match (cap, v["type"].as_str(), v["value"].as_f64()) {
            // The clock in seconds, or milliseconds, since the epoch: as a time.
            ("Clock", Some("Number"), Some(t)) if t < 1e11 => at_seconds(t as u64),
            ("Clock", Some("Number"), Some(t)) => super::flow::at_ms(&Value::from(t as u64)),
            // Random bytes: how many, not what (base64 says nothing to a person).
            (_, Some("Bytes"), _) => format!("{} random bytes", v["value"].as_str().map_or(0, |b| b.trim_end_matches('=').len() * 3 / 4)),
            _ => cut(&holon_author::agent::show(v).replace('\n', " "), 90),
        };
        out.push((Tone::Plain, format!("{what}: {said}")));
    }
    for read in r["reads"].as_array().into_iter().flatten() {
        let asked = cut(&holon_author::agent::show(&read["request"]).replace('\n', " "), 60);
        let answered = if read["withheld"] == true {
            "withheld: what it asked by may come from a secret".to_string()
        } else if !read["file"].is_null() {
            format!("a file, {} bytes, kept by its hash", read["file"]["size"])
        } else if read["answer"].is_null() {
            "nothing".to_string()
        } else {
            cut(&holon_author::agent::show(&read["answer"]).replace('\n', " "), 60)
        };
        out.push((Tone::Plain, format!("{asked} → {answered}")));
    }
    out
}

/// Whether a witness holds a run's line, from `GET /summary`'s `chain.witness`: it took a batch
/// of heads at or after the run ended. `summary` is null when it couldn't be read.
pub(crate) fn witnessed(r: &Value, summary: &Value) -> (Tone, String) {
    if summary.is_null() {
        return (Tone::Dim, "can't be told: the server's summary can't be read".into());
    }
    let w = &summary["chain"]["witness"];
    if w.is_null() {
        return (Tone::Dim, "no witness set: start the host with --export-heads".into());
    }
    let ended = r["at_ms"].as_u64().unwrap_or(0) + r["duration_ms"].as_f64().unwrap_or(0.0).ceil() as u64;
    let address = w["address"].as_str().unwrap_or("?");
    match w["taken_ms"].as_u64() {
        Some(t) if t >= ended => (Tone::Good, format!("yes, by {address}, at {}", super::flow::at_ms(&Value::from(t)))),
        _ => {
            let waiting = w["waiting"].as_u64().unwrap_or(0);
            let more = if waiting > 0 { format!(" ({waiting} head{} wait)", if waiting == 1 { "" } else { "s" }) } else { String::new() };
            (Tone::Warn, format!("not yet: {address} takes the next batch{more}"))
        }
    }
}

/// What a flow is connected to, a line each: the feeds and queues its handlers listen to and how
/// each connection is, the queues it sends to and what came of it, and its channels.
pub(crate) fn connection_lines(feeds: &[Value], queues: &Value, channels: &[Value]) -> Vec<(Tone, String)> {
    let since = |v: &Value| v["since"].as_u64().map(|ms| format!(" since {}", super::flow::at_ms(&Value::from(ms)))).unwrap_or_default();
    let waiting = |v: &Value| match v["waiting"].as_u64().unwrap_or(0) {
        0 => String::new(),
        n => format!(" · {n} waiting"),
    };
    let mut out = vec![];
    for f in feeds {
        let name = format!("feed {} ({})", f["feed"].as_str().unwrap_or("?"), f["label"].as_str().unwrap_or("?"));
        let (tone, how) = match f["status"].as_str().unwrap_or("closed") {
            "connected" => (Tone::Good, format!("connected{}", since(f))),
            "closed" => (Tone::Dim, "not open: the flow isn't running, or the feed isn't bound".to_string()),
            other => (Tone::Warn, format!("{other}{}{}", since(f), f["error"].as_str().map(|e| format!(": {e}")).unwrap_or_default())),
        };
        let at = f["position"].as_str().map(|p| format!(" · at {p}")).unwrap_or_default();
        out.push((tone, format!("{name}: {how}{}{at}", waiting(f))));
    }
    for q in queues["listening"].as_array().into_iter().flatten() {
        let name = format!("queue {} ({})", q["queue"].as_str().unwrap_or("?"), q["label"].as_str().unwrap_or("?"));
        let ran = format!(" · {} ran", q["run"].as_u64().unwrap_or(0));
        let (tone, how) = match q["status"].as_str().unwrap_or("stopped") {
            "receiving" => (Tone::Good, format!("receiving{}{ran}", since(q))),
            "stopped" => (Tone::Dim, "not listening: the flow isn't running, or the queue isn't bound".to_string()),
            other => (Tone::Bad, format!("{other}{}{}", since(q), q["error"].as_str().map(|e| format!(": {e}")).unwrap_or_default())),
        };
        out.push((tone, format!("{name}: {how}{}", waiting(q))));
    }
    for q in queues["sending"].as_array().into_iter().flatten() {
        let gave_up = q["gave_up"].as_u64().unwrap_or(0);
        let tone = if gave_up > 0 { Tone::Bad } else { Tone::Plain };
        let lost = if gave_up > 0 { format!(" · gave up on {gave_up}") } else { String::new() };
        out.push((tone, format!("sends to queue {}: {} sent{lost}{}", q["queue"].as_str().unwrap_or("?"), q["sent"].as_u64().unwrap_or(0), waiting(q))));
    }
    for c in channels {
        out.push((Tone::Plain, format!("channel {}: {} subscribed · {} kept", c["name"].as_str().unwrap_or("?"), c["subscribers"].as_u64().unwrap_or(0), c["kept"].as_u64().unwrap_or(0))));
    }
    out
}

/// What a check of every run log found (`POST /summary/check`), a line each, and the runs where a
/// line doesn't follow the one before: each one's flow and run.
pub(crate) fn check_lines(check: &Value) -> (Vec<Said>, Vec<(String, u64)>) {
    let breaks: Vec<(String, u64)> = check["breaks"].as_array().into_iter().flatten().filter_map(|b| Some((b["slug"].as_str()?.to_string(), b["seq"].as_u64()?))).collect();
    let heads = check["heads"].as_array().map_or(0, Vec::len);
    let mut lines = vec![(Tone::Plain, format!("{} runs read, across {heads} run log{}, at {}", check["runs"].as_u64().unwrap_or(0), if heads == 1 { "" } else { "s" }, super::flow::at_ms(&check["at_ms"])))];
    if breaks.is_empty() {
        lines.push((Tone::Good, "Every line follows the one before it: nothing was changed outside holon.".into()));
    } else {
        lines.push((Tone::Bad, format!("{} line{} don't follow the one before: changed outside holon. Enter opens the run picked.", breaks.len(), if breaks.len() == 1 { "" } else { "s" })));
    }
    lines.push((Tone::Dim, "Each log's head, the hash of its newest line:".into()));
    for h in check["heads"].as_array().into_iter().flatten() {
        let (slug, head) = (h[0].as_str().unwrap_or("?"), h[1].as_str().unwrap_or(""));
        lines.push((Tone::Plain, format!("  {slug:<20} {}", holon_author::first_chars(head, 16))));
    }
    (lines, breaks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_decision_is_said_as_the_web_console_says_it() {
        let event = json!({ "approved": true, "by": ["ada", "bob"], "decisions": [
            { "slot": 0, "who": "ada", "kind": "person", "at": 5, "approved": true, "binding": 3, "access": 7 },
            { "slot": 0, "who": "claude", "kind": "agent", "role": "checker", "at": 9, "approved": true, "reason": "the invoice matches", "note": { "score": 0.93 } },
        ] });
        assert_eq!(decision_said(&event).as_deref(), Some("approved by ada and bob"));
        let lines: Vec<String> = event["decisions"].as_array().unwrap().iter().map(decided_line).collect();
        assert!(lines[0].ends_with(": ada approved · through binding 3 (holon access show --version 7)"), "{}", lines[0]);
        assert!(lines[1].ends_with(": claude (an agent) approved as checker: the invoice matches · noted {\"score\":0.93}"), "{}", lines[1]);
        assert_eq!(decision_said(&json!({ "approved": false, "by": ["ada"], "declined_by": "bob", "reason": "not this vendor" })).as_deref(), Some("declined by bob: not this vendor"));
        assert_eq!(decision_said(&json!({ "approved": false, "by": ["ada"], "expired": true })).as_deref(), Some("ran out undecided, approved by ada before"));
        assert_eq!(decision_said(&json!({ "method": "GET" })), None, "an event's request is no decision");
    }

    #[test]
    fn a_secret_is_named_with_where_its_requests_took_it() {
        let request = |url: &str, auth: &str| json!({ "type": "Record", "value": [["headers", { "type": "Record", "value": [["authorization", { "type": "Text", "value": auth }]] }], ["url", { "type": "Text", "value": url }]] });
        let r = json!({
            "secrets": { "api_key": "v3 #a1b2c3d4", "other": "v1 #00ff00ff" },
            "committed": { "api_key": { "kind": "local", "commitment": "ab", "server": true } },
            "rounds": [{ "exchanges": [
                { "request": request("http://127.0.0.1:9/api", "Bearer ‹secret:api_key v3 #a1b2c3d4›"), "response": {} },
                { "request": request("https://example.com/x?y", "none"), "response": {} },
            ] }],
        });
        let uses = secret_uses(&r);
        assert_eq!(uses[0], SecretUse { name: "api_key".into(), version: Some("v3".into()), print: Some("#a1b2c3d4".into()), kept: Some("sealed on the server, the server's own".into()), to: vec!["127.0.0.1:9".into()], stopped: vec![] });
        assert_eq!(uses[1].to, Vec::<String>::new());
        assert_eq!(secret_lines(&r)[1].1, "other  v1 #00ff00ff · sent nowhere");
        // A request refused before it left took it nowhere, and says so.
        let mut refused = r.clone();
        refused["rounds"][0]["exchanges"][0]["response"] = json!({ "type": "Record", "value": [["error", { "type": "Text", "value": "blocked_address" }], ["status", { "type": "Number", "value": 0 }]] });
        assert!(secret_lines(&refused)[0].1.ends_with("sent nowhere · a request to 127.0.0.1:9 was stopped before it left"), "{:?}", secret_lines(&refused));
    }

    #[test]
    fn a_flows_connections_say_how_each_is() {
        let feeds = [json!({ "feed": "prices", "label": "on prices", "status": "reconnecting", "since": 1_000, "error": "refused", "waiting": 2 }), json!({ "feed": "news", "label": "on news", "status": "closed", "waiting": 0 })];
        let queues = json!({ "listening": [{ "queue": "jobs", "label": "on jobs", "status": "receiving", "since": 1_000, "waiting": 0, "run": 7 }], "sending": [{ "queue": "out", "sent": 3, "gave_up": 1, "waiting": 0 }] });
        let channels = [json!({ "name": "updates", "kept": 5, "subscribers": 2 })];
        let lines: Vec<String> = connection_lines(&feeds, &queues, &channels).into_iter().map(|(_, l)| l).collect();
        assert!(lines[0].starts_with("feed prices (on prices): reconnecting since ") && lines[0].ends_with(": refused · 2 waiting"), "{lines:?}");
        assert!(lines[1].starts_with("feed news (on news): not open"), "{lines:?}");
        assert!(lines[2].starts_with("queue jobs (on jobs): receiving since ") && lines[2].ends_with(" · 7 ran"), "{lines:?}");
        assert_eq!(lines[3], "sends to queue out: 3 sent · gave up on 1");
        assert_eq!(lines[4], "channel updates: 2 subscribed · 5 kept");
    }

    #[test]
    fn what_a_run_read_is_said_for_a_person() {
        let r = json!({ "sourced": [["Clock", { "type": "Number", "value": 1_794_959_174 }], ["Random", { "type": "Bytes", "value": "QL4KmKcW5jMEOydXKf0OiAh2yeECNQ0sn7jzeAWh2uk=" }]] });
        let lines: Vec<String> = read_lines(&r).into_iter().map(|(_, l)| l).collect();
        assert!(lines[0].starts_with("the clock: 2026-11-17 "), "{lines:?}");
        assert_eq!(lines[1], "randomness: 32 random bytes");
    }
}
