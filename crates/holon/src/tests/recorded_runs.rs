//! Every sample's runs, as the server records them, for the editor's replay to be held to
//! (`editor/src/recordedRuns.test.ts`, through `editor/src/recorded.fixture.json`, which this
//! writes).

use super::*;

/// What every host a sample calls answers here: one JSON body that the samples' fetches read a
/// field of (`uuid`, `id`, …) from, or fail on, as a run would.
const ANSWER: &str = r#"{"uuid":"u-1","id":"s-1","ok":true,"price":"10.5","data":[]}"#;

/// What the editor's replay reads of a log entry (replay.ts: what it starts from, what it was
/// answered, which nodes ran and what each gave), and no more: the steps one level down, where the
/// editor places them, with what each gave (what a node fed a secret, or reading from outside
/// within itself, replays from; the verdict of a run without an answer). A step's times change from one test run to the next: each step's start and end
/// are its place in the log's list (as steps ended), which the replay reads only for the order it
/// takes its roots in.
fn for_editor(entry: &serde_json::Value) -> serde_json::Value {
    const READ: &[&str] = &["kind", "route", "request", "sourced", "state_before", "events", "secrets", "reads", "rounds", "skipped", "parked", "outputs", "status", "error"];
    let mut kept: serde_json::Map<String, serde_json::Value> = READ.iter().filter_map(|k| entry.get(*k).map(|v| (k.to_string(), v.clone()))).collect();
    let steps: Vec<serde_json::Value> = entry["timeline"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["path"].as_array().is_some_and(|p| p.len() <= 2))
        .enumerate()
        .map(|(i, t)| {
            let mut t = t.clone();
            let o = t.as_object_mut().unwrap();
            o.insert("start_us".into(), serde_json::json!(i));
            o.insert("end_us".into(), serde_json::json!(i));
            t
        })
        .collect();
    kept.insert("timeline".into(), serde_json::Value::Array(steps));
    // An exchange's times go the same way.
    for r in kept.get_mut("rounds").and_then(|r| r.as_array_mut()).into_iter().flatten() {
        for x in r.get_mut("exchanges").and_then(|x| x.as_array_mut()).into_iter().flatten() {
            if let Some(x) = x.as_object_mut() {
                x.insert("start_us".into(), serde_json::json!(0));
                x.insert("end_us".into(), serde_json::json!(0));
            }
        }
    }
    serde_json::Value::Object(kept)
}

/// `text` with each resume token shown as a token: one is keyed by the host's key, drawn once per
/// process, so its value changes from one test run to the next (as the server golden masks it).
fn masked(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("rt_") {
        let after = &rest[i + 3..];
        let is_token = after.len() >= 32 && after.as_bytes()[..32].iter().all(|b| b.is_ascii_hexdigit());
        out.push_str(&rest[..i + 3]);
        if is_token {
            out.push_str("‹token›");
            rest = &after[32..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// **Guards:** the editor's replay of a run is held to the run as the server made it, for every
/// sample: this deploys each sample that needs nothing but the server and the hosts it calls
/// (each host answered by a server of the test's, `outbound::stand_in`), sends each route the
/// server golden's first two bodies and runs each action on them (so a route that keeps what it
/// fetched answers the second from what it kept, as cache-aside does), and writes what the log holds
/// of each run to `editor/src/recorded.fixture.json`. `recordedRuns.test.ts` replays each there as
/// view mode does and fails where the replay runs other nodes than the run did, or answers
/// otherwise.
///
/// **Why:** the replay is the editor's own walk of the canvas, beside core's. When core began to
/// make a call tied to a branch's trigger even where nothing reads its answer (S1's C4), the walk
/// still skipped it, and view mode said cache-aside's run that took its collection's arm differed
/// from itself (#158). Only an end-to-end test of that one sample saw it: the server's golden
/// deploys no sample that calls a host, and no sweep held the walk to the server's runs.
///
/// **If it fails:** a sample's runs changed, or the samples did: `HOLON_BLESS=1 cargo test -p
/// holon --bin holon every_samples_runs_go_to_the_editor` writes the fixture again, and the
/// editor's test says whether its replay still follows them.
#[test]
fn every_samples_runs_go_to_the_editor() {
    use std::io::Write;
    const OUTSIDE: &[&str] = &["DbRead(", "DbWrite(", "FileRead(", "FileWrite(", "FileArrival(", "RunsAction("];
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let at = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = read_request(&mut stream);
            let answer = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ANSWER}", ANSWER.len());
            let _ = stream.write_all(answer.as_bytes());
        }
    });
    let bodies = [
        String::new(),
        r#"{"id":"x","name":"tea","price":4,"amount":5,"qty":2,"items":[{"name":"tea","qty":2},{"name":"cake","qty":1}],"event":"gig","count":2,"text":"Hello World","title":"Hello World","author":"Ada","op":"mul","a":6,"b":7,"by":"ops","to":5}"#.to_string(),
    ];
    firing::fixed::from(Some(0));
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true").unwrap());
    let mut runs = Vec::new();
    for sample in holon_author::samples::samples() {
        let Ok(plan) = holon_author::agent::plan(sample.text) else { continue };
        for u in &plan.units {
            route(&state, &req("POST", "/unit", u.clone()));
        }
        let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
        let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap_or_default();
        let own = format!("RunsAction(\"{}/", plan.name);
        if !preview["refused"].is_null() || asked.iter().any(|a| !a.starts_with(&own) && OUTSIDE.iter().any(|o| a.starts_with(o))) {
            continue;
        }
        // Each host it calls, answered here, over plain HTTP.
        let hosts: Vec<String> = asked.iter().filter_map(|a| a.strip_prefix("HttpRequest(\"").and_then(|h| h.strip_suffix("\")"))).map(str::to_string).collect();
        for h in &hosts {
            crate::outbound::stand_in::answer(h, at);
        }
        let mut acknowledge = asked.clone();
        acknowledge.extend(hosts.iter().map(|h| format!("PlainHttp({h:?})")));
        if !hosts.is_empty() {
            acknowledge.push("PrivateAddresses".into());
        }
        let browsers: Vec<&str> = plan.browsers.as_deref().map(|b| b.split_whitespace().collect()).unwrap_or_default();
        let requirements = serde_json::json!({ "plain_http": hosts, "private_addresses": !hosts.is_empty() });
        let r = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": acknowledge, "document": plan.document, "browsers": browsers, "requirements": requirements }));
        if r.status_code != 200 {
            continue;
        }
        let slug = plan.name.clone();
        let (routes, actions) = {
            let table = state.deployments.lock().unwrap();
            let server = table[&slug].server.clone().unwrap_or_else(|| holon_core::Server { port: None, taint: vec![], never_keep: vec![], tags: vec![], routes: vec![], clocks: vec![], files: vec![], mails: vec![], feeds: vec![], queues: vec![], websockets: vec![], actions: vec![] });
            (server.routes.iter().map(|r| (r.method.clone(), r.path.clone())).collect::<Vec<_>>(), server.actions.iter().map(|a| a.name.clone()).collect::<Vec<_>>())
        };
        for (method, path) in &routes {
            let url: String = path.split('/').map(|seg| if seg.starts_with(':') || seg.starts_with('*') { "tea" } else { seg }).collect::<Vec<_>>().join("/");
            let url = if url.is_empty() { "/".to_string() } else { url };
            for body in &bodies {
                let request = data::DataRequest { headers: vec![("content-type".into(), "application/json".into())], ..fake_data(method, &url, body.as_bytes().to_vec()) };
                serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &request);
            }
        }
        for name in &actions {
            for body in &bodies {
                let input = holon_core::parse_json(if body.is_empty() { "{}" } else { body }).unwrap_or(Value::Record(BTreeMap::new()));
                run_action(&state.store, &state.deployments, &state.deployments_path, &slug, name, Ok(input), None, None, None, &[]);
            }
        }
        let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
        let entries: Vec<String> = export.lines.iter().map(|l| for_editor(&serde_json::from_str(l).unwrap()).to_string()).collect();
        // A run a line, so a change shows as the runs it changed.
        runs.push(format!("{{\"sample\":{},\"unit\":{},\"entries\":[\n{}\n]}}", serde_json::json!(sample.name), serde_json::json!(plan.unit), entries.join(",\n")));
    }
    firing::fixed::from(None);
    assert!(runs.iter().any(|r| r.starts_with("{\"sample\":\"cache-aside\"")), "cache-aside, which calls a host, is among them");
    let written = masked(&format!("{{\"runs\":[\n{}\n]}}\n", runs.join(",\n")));
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editor/src/recorded.fixture.json");
    let kept = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    if kept != written && std::env::var_os("HOLON_BLESS").is_some() {
        std::fs::write(&path, &written).unwrap();
        return;
    }
    assert!(kept == written, "{} is out of date: HOLON_BLESS=1 cargo test -p holon --bin holon every_samples_runs_go_to_the_editor writes it again", path.display());
}
