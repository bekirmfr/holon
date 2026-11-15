//! Reading a run's steps and values is a grant of its own, `run.steps` (the audit register's R55,
//! docs/L4s/access-and-approvals-L4.md §4): every route and every agent tool that answers what a run
//! did asks it, and refuses whoever doesn't hold it, never answering them a thinner run. The list of
//! runs (when, what started each, its status, how long, who by) stays `run.view`'s. The agent role
//! doesn't hold it: a person gives it, binding a role that does where the flow is.
//!
//! What a flow keeps in its state is `state.view`'s the same way (R56): no route answers a value of
//! it, or one a run read or left, to whoever holds the flow's runs but not its state.

use super::access_table::ask;
use super::*;
use crate::accounts::Accounts;
use crate::bindings::{RoleText, Scope, Selector, Subject};
use serde_json::{json, Value};

const PW: &str = "correct horse battery";

/// What a person sends the flow's runs: used in their steps, never answered, stored or logged.
/// Whoever doesn't hold run.steps reads it nowhere.
const CANARY: &str = "live_canary_7f3a";

/// A request whose step reads `key` and answers which kind it is, not the key; an action waiting at
/// an approval that shows nothing named, so it lists the run's input (approval_shows.rs).
const FLOW: &str = r#"import { action, approval, flow, route } from "holon"

flow("steps", { port: 18097 })

route.post("/check", (req) => {
  const key = req.json.key
  return key.startsWith("live_") ? "a live key" : "a test key"
})

export const pay = action(async (order) => {
  const decision = await approval("payout", { expires: "1h" })
  return decision.approved
})
"#;

/// `method` `path` with `body` (none when null) as the holder of `token`: its status and its body.
fn send(state: &AppState, token: &str, method: &str, path: &str, body: Value) -> (u16, String) {
    ask(state, Some(token), method, path, if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() })
}

/// `text` deployed as `slug` by ada, approving what it asks for.
fn deploy(state: &AppState, ada: &str, slug: &str, text: &str) {
    let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
    for unit in &plan.units {
        assert_eq!(ask(state, Some(ada), "POST", "/unit", unit.clone()).0, 200);
    }
    let (code, preview) = send(state, ada, "POST", "/deploy/preview", json!({ "slug": slug, "unit": plan.unit }));
    assert_eq!(code, 200, "{preview}");
    let preview: Value = serde_json::from_str(&preview).unwrap();
    let (code, said) = send(state, ada, "POST", "/deploy", json!({ "slug": slug, "unit": plan.unit, "document": plan.document, "acknowledge": preview["needs_approval"] }));
    assert_eq!(code, 200, "{said}");
}

/// A server with accounts and `FLOW` deployed, run twice with the canary by ada: a request, and its
/// action, which waits at its approval. Who signs in: ada, the admin; cy, a viewer; wes, a person
/// holding a viewer's every action but run.steps (the role `reads`); and scout, an agent holding the
/// agent role. Their tokens, by name.
fn server() -> (AppState, Arc<Accounts>, BTreeMap<&'static str, String>) {
    let mut state = test_state();
    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    state.access = access::Access::new(Arc::clone(&book), vec![], vec![]);
    let reads = ["flow.view", "run.view", "run.export", "state.view", "secret.list", "policy.view"];
    book.define_role(RoleText { name: "reads".into(), about: None, allows: reads.map(String::from).to_vec(), kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    let person = |name: &str, role: &str| book.accept(&book.invite(name, role, None, "ada").unwrap(), PW).unwrap().0;
    let (cy, wes) = (person("cy", "viewer"), person("wes", "reads"));
    assert_eq!(send(&state, &ada, "POST", "/people", json!({ "name": "scout", "kind": "agent", "role": "agent" })).0, 200);
    let (code, said) = send(&state, &ada, "POST", "/people/scout/tokens", json!({ "label": "its tools" }));
    assert_eq!(code, 200, "{said}");
    let scout = serde_json::from_str::<Value>(&said).unwrap()["token"].as_str().unwrap().to_string();
    deploy(&state, &ada, "steps", FLOW);
    let request = json!({ "method": "POST", "url": format!("/check?also={CANARY}"), "headers": [["content-type", "application/json"]], "body": json!({ "key": CANARY }).to_string() });
    let (code, said) = send(&state, &ada, "POST", "/deployments/steps/request", request);
    assert_eq!(code, 200, "{said}");
    assert!(said.contains("a live key") && !said.contains(CANARY), "{said}");
    assert_eq!(send(&state, &ada, "POST", "/trigger/steps/pay", json!({ "key": CANARY })).0, 202);
    let tokens = BTreeMap::from([("ada", ada), ("cy", cy), ("wes", wes), ("scout", scout)]);
    (state, book, tokens)
}

/// Every route a principal may be answered on: each GET of the table, its parts named as `slug`.
fn every_get(slug: &str) -> Vec<String> {
    crate::can::ROUTES
        .iter()
        .filter(|r| r.method == "GET" && r.path != "*")
        .map(|r| r.path.split('/').map(|s| if s.starts_with('{') { slug } else { s }).collect::<Vec<_>>().join("/"))
        .collect()
}

/// The routes that answer a run's steps and values: each asks run.steps.
const STEPS: [&str; 3] = ["/deployments/steps/log", "/deployments/steps/export", "/deployments/steps/replay"];

/// Every agent tool, with what it is asked: a new tool fails `the_tools_are_swept` until it has a
/// row here.
fn tool_calls() -> Vec<(&'static str, Value)> {
    let other = "import { flow, route } from \"holon\"\n\nflow(\"other\", { port: 18098 })\n\nroute.get(\"/\", () => \"hi\")\n";
    vec![
        ("reference", json!({})),
        ("check", json!({ "text": FLOW })),
        ("save", json!({ "text": other })),
        ("deploy", json!({ "text": FLOW.replace("a live key", "a live one") })),
        ("replay", json!({ "slug": "steps", "text": FLOW })),
        ("call", json!({ "slug": "steps", "method": "POST", "path": "/check", "body": { "key": "test_key" } })),
        ("run", json!({ "slug": "steps", "name": "pay", "input": { "key": "test_key" } })),
        ("whoami", json!({})),
        ("deployments", json!({ "slug": "steps" })),
        ("flows", json!({ "name": "steps" })),
        ("runs", json!({ "slug": "steps" })),
        ("decide", json!({ "slug": "steps", "run": "x", "name": "payout", "approve": true })),
        ("logs", json!({ "slug": "steps" })),
        ("state", json!({ "slug": "steps" })),
        ("explain", json!({ "slug": "steps", "run": 1 })),
        ("explain", json!({ "slug": "steps" })),
        ("history", json!({ "slug": "steps" })),
        ("restore", json!({ "slug": "steps", "run": 1 })),
    ]
}

/// `name` called with `args` as the holder of `token`, through the tools `holon mcp` serves: what it
/// says, answered or refused.
fn tool(state: &AppState, token: &str, name: &str, args: &Value) -> Result<String, String> {
    let connect = || -> Result<Box<dyn crate::mcp::Api + '_>, String> { Ok(Box::new(Signed { state, token })) };
    crate::mcp::call_tool(name, args, &connect, &crate::mcp::Ctx::default())
}

/// The class: scout (the agent role) and wes (a role holding every viewer's action but run.steps)
/// are answered the canary on no route the server has, and by no tool an agent has; the routes that
/// answer a run's steps refuse them, naming run.steps and, to the agent, what a person runs to give
/// it. The list of runs answers them each run, without its values. cy, a viewer, reads the canary
/// in the run's steps and in what its waiting run shows; scout does, once ada binds it the viewer
/// role on the flow, and `explain` says what each statement did.
#[test]
fn a_runs_steps_and_values_are_read_only_with_run_steps() {
    let (state, book, tokens) = server();
    for who in ["scout", "wes"] {
        for path in every_get("steps") {
            let (code, said) = send(&state, &tokens[who], "GET", &path, Value::Null);
            assert!(!said.contains(CANARY), "{who} GET {path} ({code}) answers the canary: {said}");
            if STEPS.contains(&path.as_str()) {
                assert_eq!(code, 403, "{who} GET {path}: {said}");
                assert!(error_of(&said).contains("run.steps"), "{who} GET {path}: {said}");
            }
        }
        let (code, heads) = send(&state, &tokens[who], "GET", "/deployments/steps/log/heads", Value::Null);
        assert_eq!(code, 200, "{who}: {heads}");
        let heads: Value = serde_json::from_str(&heads).unwrap();
        let entries = heads["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2, "{who}: {heads}");
        assert_eq!((entries[0]["kind"].as_str(), entries[0]["method"].as_str(), entries[0]["status"].as_u64(), entries[0]["by"].as_str()), (Some("request"), Some("POST"), Some(200), Some("ada")));
        assert_eq!(entries[1]["parked"], json!(["payout"]), "{who}: {heads}");
        let (code, waiting) = send(&state, &tokens[who], "GET", "/deployments/steps/runs", Value::Null);
        assert_eq!(code, 200, "{who}: {waiting}");
        let waiting: Value = serde_json::from_str(&waiting).unwrap();
        assert_eq!(waiting["runs"][0]["waiting"][0]["name"], "payout", "{who}: the run waits, listed without what it shows");
    }
    // The agent is told what a person runs to give it, and where.
    let refused = error_of(&send(&state, &tokens["scout"], "GET", "/deployments/steps/log", Value::Null).1);
    assert!(refused.contains("holon bind viewer --to scout --flow steps") && refused.contains("People"), "{refused}");
    assert!(!refused.contains("waits for"), "no access change waits for people here: {refused}");
    for (name, args) in tool_calls() {
        let said = tool(&state, &tokens["scout"], name, &args).unwrap_or_else(|e| e);
        assert!(!said.contains(CANARY), "scout's {name} {args} says the canary: {said}");
        if name == "explain" || name == "replay" {
            assert!(said.contains("run.steps"), "scout's {name} names what it lacks: {said}");
        }
    }
    // Those who hold it read it.
    let (code, log) = send(&state, &tokens["cy"], "GET", "/deployments/steps/log", Value::Null);
    assert!(code == 200 && log.contains(CANARY), "cy, a viewer: {code} {log}");
    let (code, waiting) = send(&state, &tokens["cy"], "GET", "/deployments/steps/runs", Value::Null);
    assert!(code == 200 && waiting.contains(CANARY), "cy, a viewer, sees the waiting run's input: {code} {waiting}");
    book.bind(Subject::Principal("scout".into()), "viewer", Scope::Flows(Selector::Slug("steps".into())), None, "ada", None).unwrap();
    let (code, log) = send(&state, &tokens["scout"], "GET", "/deployments/steps/log", Value::Null);
    assert!(code == 200 && log.contains(CANARY), "scout, given it on the flow: {code} {log}");
    let heads: Value = serde_json::from_str(&send(&state, &tokens["scout"], "GET", "/deployments/steps/log/heads", Value::Null).1).unwrap();
    let request = heads["entries"].as_array().unwrap().iter().find(|e| e["kind"] == "request").unwrap()["seq"].clone();
    let explained = tool(&state, &tokens["scout"], "explain", &json!({ "slug": "steps", "run": request })).unwrap();
    assert!(explained.contains(CANARY), "what each statement did, with its values: {explained}");
}

/// When an access change waits for people on this server, the refusal says that giving run.steps
/// does too.
#[test]
fn the_refusal_says_the_grant_waits_only_where_it_does() {
    let (state, book, tokens) = server();
    // Someone besides ada who may approve an access change.
    book.accept(&book.invite("bo", "admin", None, "ada").unwrap(), PW).unwrap();
    let (code, said) = send(&state, &tokens["ada"], "PUT", "/access/approvals", json!({ "changes": 1 }));
    assert_eq!(code, 200, "{said}");
    let refused = error_of(&send(&state, &tokens["scout"], "GET", "/deployments/steps/log", Value::Null).1);
    assert!(refused.contains("holon bind viewer --to scout --flow steps") && refused.contains("waits for"), "{refused}");
}

/// Every tool an agent has is in `tool_calls`: a new one is swept as soon as it is served.
#[test]
fn the_tools_are_swept() {
    let served: BTreeSet<String> = crate::mcp::tool_list().as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    let swept: BTreeSet<String> = tool_calls().into_iter().map(|(name, _)| name.to_string()).collect();
    assert_eq!(swept, served);
}

/// The list of runs says each run by what it names, field by field: when, what started it, its
/// status, how long, who by, where it waits; never its URL, its error or any value of it. A field
/// added to the log's entries is in the list only once it is named in `RunHead`.
#[test]
fn the_list_of_runs_says_what_it_names() {
    let entry = crate::recorder::LogEntry {
        seq: 7,
        session: 2,
        at_ms: 1,
        duration_ms: 2.5,
        kind: "request".into(),
        route: Some("POST /check".into()),
        method: Some("POST".into()),
        url: Some(format!("/check?also={CANARY}")),
        status: Some(500),
        response_body: Some(CANARY.into()),
        error: Some(format!("key {CANARY} isn't one")),
        by: Some("ada".into()),
        run_id: "r1".into(),
        unit: "u1".into(),
        parked: vec!["payout".into()],
        retries: 1,
        lost_since: Some(3),
        secrets: BTreeMap::from([("api_key".to_string(), "v1 #a1b2c3d4".to_string())]),
        ..Default::default()
    };
    let head = serde_json::to_value(crate::recorder::RunHead::from(&entry)).unwrap();
    let keys: BTreeSet<&str> = head.as_object().unwrap().keys().map(String::as_str).collect();
    let named = ["seq", "session", "at_ms", "duration_ms", "kind", "route", "method", "status", "failed", "by", "run_id", "unit", "parked", "retries", "lost_since"];
    assert_eq!(keys, BTreeSet::from(named));
    assert!(!head.to_string().contains(CANARY), "{head}");
    assert_eq!((head["failed"].as_bool(), head["route"].as_str(), head["status"].as_u64()), (Some(true), Some("POST /check"), Some(500)));
}

/// What the flow `kept` keeps in its state (R56): read by whoever holds state.view, by no one else.
const KEPT_CANARY: &str = "kept_canary_4d2b";

/// A flow that keeps what it is sent, and a clock that copies it, so a tick's outputs carry it.
const KEPT: &str = r#"import { every, flow, route } from "holon"
import state from "state"

flow("kept", { port: 18099 })

route.post("/note", (req) => {
  state.note = req.json.note
  return "kept"
})

every("1h", () => {
  state.seen = state.note
})
"#;

/// The class, for a flow's state (R56): rob holds `kept`'s flow and its runs (flow.view, run.view)
/// but not its state, and no GET route answers him a value it keeps: not its state, a cell's
/// changes, a session's snapshot, nor a clock tick's outputs. The routes that answer them refuse
/// him, naming what they ask. cy, a viewer, reads the value on each.
#[test]
fn a_flows_state_is_read_only_with_state_view() {
    let (state, book, tokens) = server();
    let runs = ["flow.view", "run.view"];
    book.define_role(RoleText { name: "runs".into(), about: None, allows: runs.map(String::from).to_vec(), kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    let rob = book.accept(&book.invite("rob", "runs", None, "ada").unwrap(), PW).unwrap().0;
    let ada = &tokens["ada"];
    deploy(&state, ada, "kept", KEPT);
    let request = json!({ "method": "POST", "url": "/note", "headers": [["content-type", "application/json"]], "body": json!({ "note": KEPT_CANARY }).to_string() });
    let (code, said) = send(&state, ada, "POST", "/deployments/kept/request", request);
    assert!(code == 200 && said.contains("kept"), "{code} {said}");
    let (code, said) = send(&state, ada, "POST", "/deployments/kept/tick", json!({ "clock": "every 1h" }));
    assert_eq!(code, 200, "{said}");
    // A session that ended, its state kept with it; then the next.
    for lifecycle in ["stop", "run"] {
        let (code, said) = send(&state, ada, "POST", &format!("/deployments/kept/{lifecycle}"), Value::Null);
        assert_eq!(code, 200, "{lifecycle}: {said}");
    }
    let asked = ["/deployments/kept/changes?name=note", "/deployments/kept/changes?name=seen"];
    for path in every_get("kept").into_iter().chain(asked.map(String::from)) {
        let (code, said) = send(&state, &rob, "GET", &path, Value::Null);
        assert!(!said.contains(KEPT_CANARY), "rob GET {path} ({code}) answers what the flow keeps: {said}");
    }
    for (path, action) in [("/deployments/kept/state", "state.view"), ("/deployments/kept/changes?name=note", "state.view"), ("/deployments/kept/sessions", "state.view"), ("/deployments/kept/history", "run.steps")] {
        let (code, said) = send(&state, &rob, "GET", path, Value::Null);
        assert!(code == 403 && error_of(&said).contains(action), "rob GET {path}: {code} {said}");
        let (code, said) = send(&state, &tokens["cy"], "GET", path, Value::Null);
        assert!(code == 200 && said.contains(KEPT_CANARY), "cy, a viewer, GET {path}: {code} {said}");
    }
}

/// The `error` an answer says, or nothing.
fn error_of(said: &str) -> String {
    super::access_table::error_of(said)
}
