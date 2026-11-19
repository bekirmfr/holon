//! Reading a run's steps and values is a grant of its own, `run.steps` (the audit register's R55,
//! docs/L4s/access-and-approvals-L4.md §4): every route and every agent tool that answers what a run
//! did asks it, and refuses whoever doesn't hold it, never answering them a thinner run. The list of
//! runs (when, what started each, its status, how long, who by) stays `run.view`'s. The agent role
//! doesn't hold it: a person gives it, binding a role that does where the flow is.
//!
//! What a flow keeps in its state is `state.view`'s the same way (R56): no route answers a value of
//! it, or one a run read or left, to whoever holds the flow's runs but not its state. Nor does a
//! route that does something (R57): every route that answers anything is swept, a GET or not.

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
    serve(test_state(), Arc::new(Accounts::memory()))
}

/// [`server`], on `state` with `book`: one keeping its files in a data folder, say.
fn serve(mut state: AppState, book: Arc<Accounts>) -> (AppState, Arc<Accounts>, BTreeMap<&'static str, String>) {
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

/// The table of every surface (R60, surfaces.rs) names only routes this file sweeps: each route a
/// command, a subcommand or a tool calls is one of ROUTES, written as ROUTES writes it, so what
/// holds the routes to their grants holds every surface built on them. Its tools are those
/// `holon mcp` serves, no more and no fewer.
#[test]
fn every_surface_calls_only_swept_routes() {
    use crate::surfaces::{COMMANDS, RUN_FILE, TOOLS};
    let mut wrong = vec![];
    for surface in COMMANDS.iter().chain(TOOLS) {
        for route in surface.routes {
            let (method, path) = route.split_once(' ').unwrap_or_default();
            if !crate::can::ROUTES.iter().any(|r| (r.method, r.path) == (method, path)) {
                wrong.push(format!("{}: {route} is no route of can.rs's ROUTES", surface.name));
            }
        }
        if surface.routes.iter().collect::<BTreeSet<_>>().len() != surface.routes.len() {
            wrong.push(format!("{}: a route named twice", surface.name));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    let served: BTreeSet<String> = crate::mcp::tool_list().as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    let tabled: BTreeSet<String> = TOOLS.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(tabled, served);
    assert!(RUN_FILE.routes.is_empty() && RUN_FILE.local.is_some());
}

/// The manual's page of surfaces (docs/manual/reference/surfaces.md) is the table, written out:
/// the run file first, then what an export is, then each command and each tool with what it
/// reaches, what guards that, and what it can show. `HOLON_BLESS=1` writes it again; until then a
/// change to the table fails here.
#[test]
fn every_surface_is_in_the_manual() {
    use crate::surfaces::{Surface, COMMANDS, RUN_FILE, TOOLS};
    let reaches = |s: &Surface| {
        let mut parts: Vec<String> = s.routes.iter().map(|r| format!("`{r}`")).collect();
        parts.extend(s.local.as_ref().map(|l| l.reads.to_string()));
        if parts.is_empty() { "nothing: text of its own".to_string() } else { parts.join(", ") }
    };
    let guarded = |s: &Surface| {
        let grants = (!s.routes.is_empty()).then(|| "the grants of whoever signs in".to_string());
        let local = s.local.as_ref().map(|l| if s.routes.is_empty() { l.guarded_by.to_string() } else { format!("besides, {}", l.guarded_by) });
        let all: Vec<String> = grants.into_iter().chain(local).collect();
        if all.is_empty() { "-".to_string() } else { all.join("; ") }
    };
    let shows = |s: &Surface| {
        let said: Vec<String> = s
            .shows
            .iter()
            .map(|w| match w {
                crate::surfaces::Shows::RunValues => format!("a run's values: {}", w.grant()),
                crate::surfaces::Shows::State => format!("a flow's state: {}", w.grant()),
                crate::surfaces::Shows::LogLines => format!("log lines: {}", w.grant()),
            })
            .collect();
        if said.is_empty() { "-".to_string() } else { said.join("; ") }
    };
    let run_file = RUN_FILE.local.as_ref().unwrap();
    let mut text = String::from(
        "<!-- Written from crates/holon/src/surfaces.rs, the table the tests hold every surface to:
     HOLON_BLESS=1 cargo test -p holon every_surface_is_in_the_manual writes it again. -->
# What reaches a server, and what guards it

A Holon server keeps flows' runs, their state and the lines they log. Everything that reaches them
does so in one of three ways: through the server's routes, held to the grants of whoever signs in
(who may do what: [People, agents and services](../guides/people.md)); through a file on a
machine, guarded by that machine's permissions; or neither, a command that prints text of its own.
This page lists each surface, the `holon` commands and the tools `holon mcp` serves an agent, with
what it reaches and what guards it. A test runs each one against a server and fails on a route it
calls that isn't listed here, and on a command or a tool that isn't.

Three grants hold what a surface can show:

- a run's values (what it was sent, what each step computed): `run.steps` on the flow;
- a flow's state (its values, a cell's changes, a session's snapshot): `state.view`;
- the lines a flow logs: `run.view`, and an agent only where a person let agents read them (the
  deploy settings' \"agents may read its logs\").

Whoever doesn't hold one is refused, with the grant named, or is shown nothing of it. An agent is
refused, besides, what only a person does: approving, granting, starting, stopping and deleting.

## The run file

",
    );
    text.push_str(&format!(
        "Reaches: {}.\n\nGuarded by: {}.\n\nSo on the server's own machine, whoever can read the run file is the server's admin, whatever the\ngrants say: the grants hold the run file to nothing, and keeping the machine's accounts to those\nwho should be its admin is what guards it. A command run with an agent's or a service's token\n(HOLON_TOKEN) or a `holon login` session signs in as that instead, and is held to its grants.\n`holon access local` says which account the run file signs in as.\n\n",
        run_file.reads, run_file.guarded_by
    ));
    text.push_str(
        "## An export is a file no grant follows

`holon export`, and the editor's export, write a deployment's runs to a file, with every value
they were sent and computed. The server answers it only to whoever holds `run.steps` and
`run.export` on the flow. Once the file is written, no grant follows it: whoever can read the file
reads every value in it, and `holon verify` checks it with no server at all. Keep an export where
only those who hold `run.steps` on its flow can read it.

## Commands

| Command | Reaches | Guarded by | Reads of what the server keeps, and the grant it takes |
|---|---|---|---|
",
    );
    for s in COMMANDS {
        text.push_str(&format!("| `holon {}` | {} | {} | {} |\n", s.name, reaches(s), guarded(s), shows(s)));
    }
    text.push_str("\n## Agent tools\n\n`holon mcp` signs in as an agent: with a person's token, or with none, it refuses every tool\nbut `reference` and `check`.\n\n| Tool | Reaches | Reads of what the server keeps, and the grant it takes |\n|---|---|---|\n");
    for s in TOOLS {
        text.push_str(&format!("| `{}` | {} | {} |\n", s.name, reaches(s), shows(s)));
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual/reference/surfaces.md");
    let kept = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    if kept != text && std::env::var_os("HOLON_BLESS").is_some() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    assert!(kept == text, "{} is out of date: HOLON_BLESS=1 cargo test -p holon every_surface_is_in_the_manual writes it again", path.display());
}

/// The hash of a state a run left: state.view's, never in the list of runs.
const STATE_HASH: &str = "5ea7e0f1c2d3b4a5968778695a4b3c2d1e0f1a2b3c4d5e6f708192a3b4c5d6e7";

/// The list of runs says each run by what it names, field by field: when, what started it, its
/// status, how long, who by, where it waits, whether it left a state to restore to; never its URL,
/// its error, the state's hash or any value of it. A field added to the log's entries is in the
/// list only once it is named in `RunHead`.
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
        state_after: Some(STATE_HASH.into()),
        ..Default::default()
    };
    let head = serde_json::to_value(crate::recorder::RunHead::from(&entry)).unwrap();
    let keys: BTreeSet<&str> = head.as_object().unwrap().keys().map(String::as_str).collect();
    let named = ["seq", "session", "at_ms", "duration_ms", "kind", "route", "method", "status", "failed", "left_state", "by", "run_id", "unit", "parked", "retries", "lost_since"];
    assert_eq!(keys, BTreeSet::from(named));
    assert!(!head.to_string().contains(CANARY) && !head.to_string().contains(STATE_HASH), "{head}");
    assert_eq!((head["failed"].as_bool(), head["route"].as_str(), head["status"].as_u64()), (Some(true), Some("POST /check"), Some(500)));
    // A restore to a run needs to know it left a state, as `holon history` (the VS Code history)
    // offers one: whether, never which.
    assert_eq!(head["left_state"].as_bool(), Some(true));
    let none = crate::recorder::RunHead::from(&crate::recorder::LogEntry { state_after: None, ..entry });
    assert!(!none.left_state, "a run that left no state says so");
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

/// What the sweep of every route sends that the server made: a version of `kept` to deploy (its
/// units, its unit and its document), the run of `steps` waiting at its approval, an agent's
/// proposal, and the run of `kept` that kept the value, to go back to.
#[derive(Default)]
struct Got {
    units: Vec<Vec<u8>>,
    unit: String,
    document: Value,
    waiting: String,
    proposal: u64,
    seq: u64,
}

/// What every other route is sent, `kept` deployed and its value kept by a request: a version of
/// kept, the run of `steps` waiting, a proposal scout makes, the run that kept the value.
fn what_rob_sends(state: &AppState, tokens: &BTreeMap<&'static str, String>) -> Got {
    let ada = &tokens["ada"];
    let plan = holon_author::agent::plan(&KEPT.replace("return \"kept\"", "return \"kept, again\"")).unwrap();
    let heads: Value = serde_json::from_str(&send(state, ada, "GET", "/deployments/kept/log/heads", Value::Null).1).unwrap();
    let seq = heads["entries"].as_array().unwrap().iter().find(|e| e["kind"] == "request").unwrap()["seq"].as_u64().unwrap();
    let runs: Value = serde_json::from_str(&send(state, ada, "GET", "/deployments/steps/runs", Value::Null).1).unwrap();
    let waiting = runs["runs"][0]["run_id"].as_str().unwrap().to_string();
    let proposal = {
        let other = holon_author::agent::plan(&KEPT.replace("return \"kept\"", "return \"kept, an agent's\"")).unwrap();
        for unit in &other.units {
            assert_eq!(ask(state, Some(&tokens["scout"]), "POST", "/unit", unit.clone()).0, 200);
        }
        let (code, said) = send(state, &tokens["scout"], "POST", "/deploy", json!({ "slug": "kept", "unit": other.unit, "document": other.document }));
        assert_eq!(code, 202, "an agent's version waits: {said}");
        serde_json::from_str::<Value>(&said).unwrap()["proposal"]["id"].as_u64().unwrap()
    };
    Got { units: plan.units.clone(), unit: plan.unit.clone(), document: plan.document.clone(), waiting, proposal, seq }
}

/// Every route that isn't a GET, as rob sends it in the sweep (R57), in an order that leaves each
/// something to do, `kept` deleted last: its method, its path, its body, and what it answers him
/// (202 for an action that waits at its approval; 403 for a state put back, which he may not read;
/// 400 for the route that says how a flow's actions are run; 204 for a secret deleted).
/// A route of the table that isn't here is named in `UNSENT`, or `every_route_is_swept` fails.
fn every_other(got: &Got) -> Vec<(&'static str, String, Vec<u8>, u16)> {
    let j = |v: Value| serde_json::to_vec(&v).unwrap();
    let mut sent: Vec<(&'static str, String, Vec<u8>, u16)> = got.units.iter().map(|u| ("POST", "/unit".to_string(), u.clone(), 200)).collect();
    let check = json!({ "method": "POST", "url": "/check", "headers": [["content-type", "application/json"]], "body": json!({ "key": "test_key" }).to_string() });
    let at = |seq: u64, what: Value| {
        let mut body = json!({ "run": seq });
        body.as_object_mut().unwrap().extend(what.as_object().unwrap().clone());
        j(body)
    };
    sent.extend([
        ("POST", "/deploy/preview".into(), j(json!({ "slug": "kept", "unit": got.unit })), 200),
        ("POST", "/deploy".into(), j(json!({ "slug": "kept", "unit": got.unit, "document": got.document })), 200),
        ("POST", "/flows".into(), j(json!({ "name": "rob-saved", "document": {} })), 200),
        ("DELETE", "/flows/rob-saved".into(), vec![], 200),
        ("POST", format!("/proposals/{}/decline", got.proposal), j(json!({ "reason": "not now" })), 200),
        ("POST", "/summary/check".into(), vec![], 200),
        ("POST", "/permissions/reload".into(), vec![], 200),
        // It answers, as a 400, how its actions are run.
        ("POST", "/trigger/steps".into(), vec![], 400),
        ("POST", "/trigger/steps/pay".into(), j(json!({ "key": "test_key" })), 202),
        ("POST", "/deployments/steps/request".into(), j(check), 200),
        ("POST", format!("/deployments/steps/runs/{}/decide", got.waiting), j(json!({ "name": "payout", "approve": true })), 200),
        ("POST", "/deployments/kept/tick".into(), j(json!({ "clock": "every 1h" })), 200),
        ("POST", "/deployments/kept/restore".into(), at(got.seq, json!({ "state": true, "dry_run": true })), 403),
        ("POST", "/deployments/kept/restore".into(), at(got.seq, json!({ "state": true })), 403),
        ("POST", "/deployments/kept/restore".into(), at(got.seq, json!({ "code": true, "dry_run": true })), 200),
        ("POST", "/deployments/kept/restore".into(), at(got.seq, json!({ "code": true })), 200),
        ("POST", "/deployments/kept/settings".into(), j(json!({ "agents_propose": false })), 200),
        ("POST", "/deployments/kept/grant".into(), j(json!({ "revoke": ["StateWrite(\"seen\")"] })), 200),
        ("POST", "/deployments/kept/grant".into(), j(json!({ "grant": ["StateWrite(\"seen\")"] })), 200),
        ("POST", "/deployments/kept/secrets/k".into(), j(json!({ "value": "first-value" })), 200),
        ("POST", "/deployments/kept/secrets/k".into(), j(json!({ "value": "second-value" })), 200),
        ("POST", "/deployments/kept/secrets/k/rollback".into(), j(json!({ "version": 1 })), 200),
        ("DELETE", "/deployments/kept/secrets/k".into(), vec![], 204),
        ("POST", "/deployments/kept/pause".into(), vec![], 200),
        ("POST", "/deployments/kept/run".into(), vec![], 200),
        ("POST", "/deployments/kept/stop".into(), vec![], 200),
        ("DELETE", "/deployments/kept".into(), vec![], 200),
    ]);
    sent
}

/// Why each route that isn't a GET isn't sent in the sweep: none reads a flow's runs or its state.
const HOST: &str = "a resource of the host's, bound or let go: no flow's runs or state";
const PEOPLE: &str = "who signs in and what each holds: no flow's runs or state";
const SELF: &str = "the caller's own sign-in: no flow's runs or state";
const UNSENT: &[(&str, &str, &str)] = &[
    ("OPTIONS", "*", "a CORS preflight: headers, no body"),
    ("POST", "/auth/setup", SELF),
    ("POST", "/auth/sign-in", SELF),
    ("POST", "/auth/invite", SELF),
    ("POST", "/auth/password", SELF),
    ("POST", "/auth/prefs", SELF),
    ("POST", "/auth/sign-out", SELF),
    ("POST", "/admin/folders", HOST),
    ("POST", "/admin/folders/sweep", HOST),
    ("POST", "/admin/folders/pick", HOST),
    ("DELETE", "/admin/folders/{name}", HOST),
    ("POST", "/admin/storage", HOST),
    ("POST", "/admin/store/sweep", HOST),
    ("POST", "/admin/keys/rotate", HOST),
    ("POST", "/admin/keys/forget", HOST),
    ("POST", "/admin/databases", HOST),
    ("DELETE", "/admin/databases/{name}", HOST),
    ("POST", "/admin/mails", HOST),
    ("DELETE", "/admin/mails/{name}", HOST),
    ("POST", "/admin/feeds", HOST),
    ("DELETE", "/admin/feeds/{name}", HOST),
    ("POST", "/admin/queues", HOST),
    ("DELETE", "/admin/queues/{name}", HOST),
    ("POST", "/admin/secrets/{name}", HOST),
    ("POST", "/admin/secrets/{name}/rollback", HOST),
    ("POST", "/admin/secrets/{name}/cap", HOST),
    ("DELETE", "/admin/secrets/{name}", HOST),
    ("POST", "/people", PEOPLE),
    ("POST", "/admin/oidc", PEOPLE),
    ("POST", "/people/{name}/tokens", PEOPLE),
    ("DELETE", "/people/{name}/tokens/{id}", PEOPLE),
    ("POST", "/people/{name}/reset", PEOPLE),
    ("DELETE", "/people/{name}", PEOPLE),
    ("PATCH", "/people/{name}", PEOPLE),
    ("POST", "/bindings", PEOPLE),
    ("DELETE", "/bindings/{id}", PEOPLE),
    ("PUT", "/access/approvals", PEOPLE),
    ("POST", "/access/proposals/{id}/approve", PEOPLE),
    ("POST", "/access/proposals/{id}/decline", PEOPLE),
    ("PUT", "/access/local", PEOPLE),
    ("PUT", "/roles/{name}", PEOPLE),
    ("DELETE", "/roles/{name}", PEOPLE),
    ("PUT", "/groups/{name}", PEOPLE),
    ("DELETE", "/groups/{name}", PEOPLE),
];

/// Every route of the table is swept (R57): a GET by `every_get`, any other sent by
/// `every_other`, or named in `UNSENT`, with why; never both. A new route fails this until it is
/// one or the other.
#[test]
fn every_route_is_swept() {
    let sent = every_other(&Got { units: vec![vec![]], waiting: "r1".into(), ..Default::default() });
    let mut wrong = vec![];
    for route in crate::can::ROUTES {
        let named = UNSENT.iter().any(|(method, path, _)| (*method, *path) == (route.method, route.path));
        let swept = route.method == "GET" && route.path != "*" || sent.iter().any(|(method, path, _, _)| crate::can::find(method, path).is_some_and(|(r, _)| std::ptr::eq(r, route)));
        if named == swept {
            wrong.push(format!("{} {}: {}", route.method, route.path, if named { "both swept and named unsent" } else { "neither swept nor named unsent" }));
        }
    }
    for (method, path, _) in UNSENT {
        if !crate::can::ROUTES.iter().any(|r| (r.method, r.path) == (*method, *path)) {
            wrong.push(format!("{method} {path}: named unsent, and no route of the table"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The class, for a flow's state (R56) and every route that answers anything (R57): rob holds
/// every action but state.view and run.steps, and no route answers him a value `kept` keeps, nor
/// one of `steps`' runs: not a GET (its state, a cell's changes, a session's snapshot, a clock
/// tick's history), nor any other, each sent so that it does what it does (a clock ticked now, a
/// restore, a deploy, an approval decided, the flow deleted). The routes that answer them refuse
/// him, naming what they ask, but for ticking, which answers him when it ticked and which clock,
/// and that its outputs were left out, naming run.steps. cy, a viewer, reads the value on each
/// GET; ada, an admin, on a tick and on a state restore's dry run.
#[test]
fn a_flows_state_is_read_only_with_state_view() {
    let (state, book, tokens) = server();
    let all_but: Vec<String> = crate::can::Action::ALL.iter().filter(|a| !matches!(a, crate::can::Action::RunSteps | crate::can::Action::StateView)).map(|a| a.name().to_string()).collect();
    book.define_role(RoleText { name: "runs".into(), about: None, allows: all_but, kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    let rob = book.accept(&book.invite("rob", "runs", None, "ada").unwrap(), PW).unwrap().0;
    let ada = &tokens["ada"];
    deploy(&state, ada, "kept", KEPT);
    let request = json!({ "method": "POST", "url": "/note", "headers": [["content-type", "application/json"]], "body": json!({ "note": KEPT_CANARY }).to_string() });
    let (code, said) = send(&state, ada, "POST", "/deployments/kept/request", request);
    assert!(code == 200 && said.contains("kept"), "{code} {said}");
    let (code, said) = send(&state, ada, "POST", "/deployments/kept/tick", json!({ "clock": "every 1h" }));
    assert!(code == 200 && said.contains(KEPT_CANARY), "ada's tick answers its outputs: {code} {said}");
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

    let got = what_rob_sends(&state, &tokens);
    // ada, who holds it, reads the state a restore would put back.
    let (code, said) = send(&state, ada, "POST", "/deployments/kept/restore", json!({ "run": got.seq, "state": true, "dry_run": true }));
    assert!(code == 200 && said.contains(KEPT_CANARY), "ada's dry run says the values: {code} {said}");
    for (method, path, body, answered) in every_other(&got) {
        let (code, said) = ask(&state, Some(&rob), method, &path, body);
        assert!(!said.contains(KEPT_CANARY) && !said.contains(CANARY), "rob {method} {path} ({code}) answers a value: {said}");
        assert_eq!(code, answered, "rob {method} {path}: {said}");
        if code == 403 {
            assert!(error_of(&said).contains("state.view"), "rob {method} {path}: {said}");
        }
        if path.ends_with("/tick") {
            let tick: Value = serde_json::from_str(&said).unwrap();
            let keys: BTreeSet<&str> = tick.as_object().unwrap().keys().map(String::as_str).collect();
            assert_eq!(keys, BTreeSet::from(["at_epoch_seconds", "label", "left_out"]), "{said}");
            assert!(tick["left_out"].as_str().unwrap().contains("holon bind viewer --to rob --flow kept"), "what would give them: {said}");
        }
    }
}

/// A flow listening to a queue and a feed (R87, R91): what each has seen is kept in the data
/// folder, and read on the listener's own thread as it starts.
const LISTENS: &str = r#"import { flow, on, route } from "holon"
import state from "state"
import events from "queue:events"
import prices from "feed:prices"

flow("listens", { port: 18095 })

on.queue(events, (m) => {
  state.last = m.id
})

on.feed(prices, (e) => {
  state.price = e.data
})

route.get("/", () => "listening")
"#;

/// Each route the sweeps leave unsent (`UNSENT`), sent the data folder's sweep with a body, in an
/// order that leaves each something to do and the caller's own sign-out last. Not the CORS
/// preflight (headers, no body), nor the folder dialog (it opens on the server's screen and
/// touches no file of the data folder). `bound` is a folder outside the data folder, a person's.
fn sent_to_the_host(bound: &std::path::Path, invite: &str) -> Vec<(&'static str, String, Value)> {
    let folder = bound.join("inbox");
    let _ = std::fs::create_dir_all(&folder);
    vec![
        ("POST", "/admin/folders".into(), json!({ "name": "inbox", "path": folder.display().to_string(), "access": "read-write", "keep": "bytes" })),
        ("POST", "/admin/folders/sweep".into(), json!({})),
        ("DELETE", "/admin/folders/inbox".into(), Value::Null),
        ("POST", "/admin/storage".into(), json!({ "warn_at": "90%" })),
        ("POST", "/admin/store/sweep".into(), json!({})),
        ("POST", "/admin/keys/rotate".into(), json!({ "scope": "server" })),
        ("POST", "/admin/keys/forget".into(), json!({ "slug": "kept" })),
        ("POST", "/admin/databases".into(), json!({ "name": "shop", "path": bound.join("shop.db").display().to_string(), "schema": "create table t (k text not null)" })),
        ("DELETE", "/admin/databases/shop".into(), Value::Null),
        ("POST", "/admin/mails".into(), json!({ "name": "ops", "smtp": "env:HOLON_SECRET_OPS_SMTP", "from": "Ops <ops@acme.com>", "to": "*@acme.com" })),
        ("DELETE", "/admin/mails/ops".into(), Value::Null),
        ("POST", "/admin/feeds".into(), json!({ "name": "more", "url": "env:HOLON_SECRET_PRICES_URL" })),
        ("DELETE", "/admin/feeds/more".into(), Value::Null),
        ("POST", "/admin/queues".into(), json!({ "name": "more", "url": "env:HOLON_SECRET_EVENTS_URL" })),
        ("DELETE", "/admin/queues/more".into(), Value::Null),
        ("POST", "/admin/secrets/k".into(), json!({ "value": "r87-value", "to": [] })),
        ("POST", "/admin/secrets/k/rollback".into(), json!({ "version": 1 })),
        ("POST", "/admin/secrets/k/cap".into(), json!({ "to": [] })),
        ("DELETE", "/admin/secrets/k".into(), Value::Null),
        ("POST", "/people".into(), json!({ "name": "bot", "kind": "agent", "role": "agent" })),
        ("POST", "/admin/oidc".into(), json!({})),
        ("POST", "/people/scout/tokens".into(), json!({ "label": "more" })),
        ("DELETE", "/people/scout/tokens/1".into(), Value::Null),
        ("POST", "/people/cy/reset".into(), json!({})),
        ("PATCH", "/people/cy".into(), json!({ "role": "viewer" })),
        ("DELETE", "/people/bot".into(), Value::Null),
        ("POST", "/bindings".into(), json!({ "who": { "principal": "cy" }, "role": "viewer", "scope": { "flows": { "slug": "steps" } } })),
        ("DELETE", "/bindings/1".into(), Value::Null),
        ("PUT", "/access/approvals".into(), json!({ "exclude_deployer": false })),
        ("POST", "/access/proposals/1/approve".into(), json!({})),
        ("POST", "/access/proposals/1/decline".into(), json!({ "reason": "not now" })),
        ("PUT", "/access/local".into(), json!({ "account": "cy" })),
        ("PUT", "/roles/r87".into(), json!({ "allows": ["flow.view"], "kinds": ["person"] })),
        ("DELETE", "/roles/r87".into(), Value::Null),
        ("PUT", "/groups/g87".into(), json!({ "members": ["cy"] })),
        ("DELETE", "/groups/g87".into(), Value::Null),
        ("POST", "/auth/setup".into(), json!({ "code": "not-the-code", "name": "ada", "password": PW })),
        ("POST", "/auth/sign-in".into(), json!({ "name": "ada", "password": PW })),
        ("POST", "/auth/invite".into(), json!({ "token": invite, "password": "another horse battery" })),
        ("POST", "/auth/password".into(), json!({ "current": PW, "new": PW })),
        ("POST", "/auth/prefs".into(), json!({ "theme": "dark" })),
        ("POST", "/auth/sign-out".into(), json!({})),
    ]
}

/// What of `text` is around `part`: a few dozen characters each side.
fn around(text: &str, part: &str) -> String {
    let at = text.find(part).unwrap_or(0);
    let start = (0..=at.saturating_sub(100)).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
    let end = (at + part.len() + 100).min(text.len());
    let end = (end..=text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
    text[start..end].to_string()
}

/// Guards: no answer of the server's, to anyone, says where its data folder is or why a file
/// there failed, with every file there failing (audit R87): every route R57 holds complete (every
/// GET, for each flow; every other, as rob's sweep sends it; and the host's and people's routes
/// that sweep leaves unsent, each with a body), sent as ada (an admin) and as hal (every action
/// but host.view), with the data folder made to fail on the sweep's thread (every file and every
/// key: at_rest.rs `data_folder`) and under the folder on every thread; and what a queue's and a
/// feed's listener note when what they have seen can't be read (R91), in the events and both
/// listings. Each answer is held by writers.rs's `where_or_why`: no part of the folder's path,
/// its name, a file's name under it, the temporary folder, an OS error's words, or the failure's
/// own (`at_rest::MADE_TO_FAIL`). A GET that says where to ada before the fault too (she views
/// the host) may say it still, never why; the admin's warnings in a GET to her are hers, and
/// what a GET says is read without them, so nothing hangs on a warning the machine has (its disk
/// past `warn_at`). Set up first so each reader is reached: a policy file
/// naming a backup folder, console lines, a folder read whose text is kept, and a log past what it
/// keeps in memory and in its file, cut into sealed segments. Then a second pass fails alone each
/// place a request reaches only behind another that passes, or that reads what was kept before
/// (at_rest::fail_sites), with the request that reaches it. Every place in the sources
/// (`hook_sites`, by file and function) has an answer that reached it or a reason no request does
/// (`REASONS`), never both: one with neither fails the sweep.
/// Why: answers passed on where a server's file failed and why, each found by reading (R71, R72,
/// R85, R91): the class, swept.
/// If it fails: the answer named tells whoever asked a fixed text of what failed; where and why
/// go to the admin's warnings and stderr (failing::Failed, R71).
#[test]
fn no_answer_says_where_or_why_the_data_folder_failed() {
    let dir = std::env::temp_dir().join(format!("holon-r87-{}-{}", std::process::id(), rand::random::<u32>()));
    let bound = dir.with_extension("bound");
    for d in [&dir, &bound] {
        std::fs::create_dir_all(d).unwrap();
    }
    let at = |name: &str| dir.join(name).to_string_lossy().into_owned();
    // What it binds: a queue, a feed and a mail server, each a stand-in, their URLs in a secret
    // source outside the data folder.
    let (sqs, feed, sink) = (crate::sqs::stand_in::StandIn::start(), crate::sse::stand_in::StandIn::start(), crate::smtp::sink::start("PLAIN LOGIN"));
    sqs.make("events");
    let env = dir.with_extension("env");
    std::fs::write(&env, format!("HOLON_SECRET_EVENTS_URL={}\nHOLON_SECRET_PRICES_URL={}\nHOLON_SECRET_OPS_SMTP=smtp://ops:s3cret@127.0.0.1:{}\n", sqs.url("events"), feed.url("/events"), sink.port)).unwrap();
    let sources = || crate::secret_sources::Sources::new(Some(env.clone())).unwrap();
    // Its store on disk, nothing kept above it: every unit and value a run reads is read from the
    // data folder, where it fails (a value kept in memory doesn't, as with a real disk's fault).
    let disk = FileStore::new(dir.join("store"), crate::at_rest::store()).unwrap();
    let values = disk.values_dir();
    let shared = SharedStore::on_disk(Box::new(disk), values);
    shared.4.set_budget(0);
    let mut state = test_state_in(shared, crate::paths::Place::parse(&at("deployments.json")));
    // Its policy in a file, naming a backup folder in the data folder: the storage page reads
    // both, and a reload the file.
    let backup = dir.join("backup");
    std::fs::create_dir_all(backup.join("kept")).unwrap();
    let policy = format!("[grants]\nallow_plain_http = true\nallow_private_addresses = true\n\n[storage]\nbackup = '{}'\n", backup.display());
    std::fs::write(at("holon.toml"), &policy).unwrap();
    state.policy_path = crate::paths::Place::parse(&at("holon.toml"));
    *state.policy.write().unwrap() = Arc::new(crate::permissions::Policy::parse(&policy).unwrap());
    state.secrets = Arc::new(crate::secrets::SecretStore::open(&at("secrets.json"), Arc::clone(crate::at_rest::keys()), sources()).unwrap());
    state.folders = Arc::new(
        crate::folders::Folders::open(&at("folders.json"), std::slice::from_ref(&dir))
            .unwrap()
            .with_databases(crate::databases::Databases::open(&at("databases.json"), std::slice::from_ref(&dir)).unwrap().with_sources(sources()))
            .with_mails(crate::mails::Mails::open(&at("mails.json")).unwrap().with_sources(sources()))
            .with_feeds(crate::feeds::Feeds::open(&at("feeds.json")).unwrap().with_sources(sources()))
            .with_queues(crate::queues::Queues::open(&at("queues.json")).unwrap().with_sources(sources()))
            .with_blobs(crate::blobs::Blobs::at(dir.join("blobs")))
            .with_logs(crate::logs::Logs::beside(&state.deployments_path, crate::logs::Caps::default())),
    );
    state.activity = crate::activity::Activity::beside(&state.deployments_path);
    let (state, book, tokens) = serve(state, Arc::new(Accounts::open(&at("accounts.json")).unwrap()));
    let ada = tokens["ada"].clone();
    let all_but: Vec<String> = crate::can::Action::ALL.iter().filter(|a| !matches!(a, crate::can::Action::HostView)).map(|a| a.name().to_string()).collect();
    book.define_role(RoleText { name: "all-but-host".into(), about: None, allows: all_but, kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    let hal = book.accept(&book.invite("hal", "all-but-host", None, "ada").unwrap(), PW).unwrap().0;
    let invite = book.invite("ivy", "viewer", None, "ada").unwrap();
    for (path, body) in [("/admin/queues", json!({ "name": "events", "url": "env:HOLON_SECRET_EVENTS_URL" })), ("/admin/feeds", json!({ "name": "prices", "url": "env:HOLON_SECRET_PRICES_URL" }))] {
        let (code, said) = send(&state, &ada, "POST", path, body);
        assert_eq!(code, 200, "{path}: {said}");
    }
    deploy(&state, &ada, "kept", KEPT);
    deploy(&state, &ada, "listens", LISTENS);
    let note = json!({ "method": "POST", "url": "/note", "headers": [["content-type", "application/json"]], "body": json!({ "note": "a note" }).to_string() });
    assert_eq!(send(&state, &ada, "POST", "/deployments/kept/request", note.clone()).0, 200);
    // A folder outside the data folder, whose texts a run reads are kept in it; a flow that reads
    // one and says so on the console; and kept's runs past what its log keeps in memory and in its
    // file, cut into sealed segments: what the logs', the files' and the storage pages read back.
    let texts = bound.join("texts");
    std::fs::create_dir_all(&texts).unwrap();
    // Past what a run's record holds inline (folders::LOG_INLINE_BYTES): kept in the store of texts
    // (blobs.rs), which export and the folders' sweep read back.
    let text = "a text kept\n".repeat(700);
    assert!(text.len() as u64 > crate::folders::LOG_INLINE_BYTES);
    std::fs::write(texts.join("a.txt"), &text).unwrap();
    let (code, said) = send(&state, &ada, "POST", "/admin/folders", json!({ "name": "texts", "path": texts.display().to_string(), "access": "read", "keep": "bytes" }));
    assert_eq!(code, 200, "{said}");
    deploy(&state, &ada, "texts", TEXTS);
    let read = json!({ "method": "GET", "url": "/read", "headers": [], "body": "" });
    let (code, said) = send(&state, &ada, "POST", "/deployments/texts/request", read);
    assert!(code == 200 && said.contains("a text kept"), "{said}");
    // Kept long enough ago for the folders' sweep to look at it (it spares what was kept in the
    // last folders::SWEEP_SPARES_MS): the store's files dated back. A run still names it, so the
    // sweep keeps it.
    let long_ago = std::time::SystemTime::now() - std::time::Duration::from_millis(2 * crate::folders::SWEEP_SPARES_MS);
    for d in [dir.join("blobs"), dir.join("blobs").join("chunks")] {
        for e in std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.path().is_file()) {
            std::fs::File::options().write(true).open(e.path()).and_then(|f| f.set_modified(long_ago)).unwrap();
        }
    }
    for _ in 0..crate::recorder::HOT + 20 {
        assert_eq!(send(&state, &ada, "POST", "/deployments/kept/request", note.clone()).0, 200);
    }
    // Its file keeping 40 runs as written, the older sealed in segments beside it, by a cut asked
    // for now (as the logs' sync makes it). 40 is more than it holds in memory
    // (recorder::IN_MEMORY), so some runs are read from its file alone (Log::plan), some from its
    // segments (read_segment).
    {
        let deployments = state.deployments.lock().unwrap();
        deployments["kept"].log.with(|log| {
            log.set_hot(crate::recorder::IN_MEMORY + 8);
            log.want_cut();
        });
        deployments["kept"].log.cut_if_planned();
        assert!(deployments["kept"].log.with(|log| log.sealed_holds().0) > 0, "kept's log is cut into sealed segments");
    }
    let got = what_rob_sends(&state, &tokens);
    let mut gets: Vec<String> = ["steps", "kept", "listens", "texts"].into_iter().flat_map(every_get).collect();
    gets.sort();
    gets.dedup();
    let callers = [("ada", ada.as_str()), ("hal", hal.as_str())];
    // The host's and people's routes the sweeps leave unsent: each sent here, but two.
    let to_the_host = sent_to_the_host(&bound, &invite);
    let left: Vec<String> = UNSENT
        .iter()
        .filter(|(method, path, _)| (*method, *path) != ("OPTIONS", "*") && (*method, *path) != ("POST", "/admin/folders/pick"))
        .filter(|(method, path, _)| !to_the_host.iter().any(|(m, p, _)| crate::can::find(m, p).is_some_and(|(r, _)| (r.method, r.path) == (*method, *path))))
        .map(|(method, path, _)| format!("{method} {path}"))
        .collect();
    assert!(left.is_empty(), "left unsent, and not sent the data folder's sweep: {left:?}");

    // The disk warned of on every machine, as a floor no disk has makes it (tests.rs's
    // the_host_warns_of_a_full_disk_and_a_full_cache_once_each sets one): its warning names the
    // data folder's disk, in the admin's warnings, so every run looks at the same answers, whatever
    // this machine's disk holds, and finds a GET that hands the warnings to hal.
    let (code, said) = send(&state, &ada, "POST", "/admin/storage", json!({ "warn_at": "1000T" }));
    assert_eq!(code, 200, "{said}");
    let (_, warned) = send(&state, &ada, "GET", "/admin/warnings", Value::Null);
    assert!(warned.contains("\"disk:"), "the data folder's disk is warned of: {warned}");
    // An answer without the admin's warnings in it. They say where and why, as they are meant to
    // (failing::note_file: a listener's record that can't be read, its path and why), to whoever
    // views the host alone. What else a GET says is read without them, before the fault as under
    // it, so what it may say doesn't hang on a warning this machine happens to have: its disk past
    // `[storage] warn_at` (rob's sweep sets 90%) is warned of by the data folder's path.
    let outside_warnings = |said: &str| match serde_json::from_str::<Value>(said) {
        Ok(Value::Object(mut answer)) if answer.contains_key("warnings") => {
            answer.remove("warnings");
            Value::Object(answer).to_string()
        }
        _ => said.to_string(),
    };
    // What each GET says before the fault, outside the admin's warnings: where, to whoever may
    // view the host.
    let mut before: BTreeMap<(String, String), bool> = BTreeMap::new();
    for (who, token) in callers {
        for path in &gets {
            let (_, said) = send(&state, token, "GET", path, Value::Null);
            before.insert((who.to_string(), path.clone()), !super::writers::where_or_why(&outside_warnings(&said), &dir, &[]).is_empty());
        }
    }

    // Every place the sources ask the hook from, by name, and by its place as the hook notes it.
    let sites = hook_sites();
    let names: BTreeMap<String, String> = sites.iter().map(|(name, place)| (place.clone(), name.clone())).collect();

    // The data folder failing: every file under it, on every thread; every file and key on this
    // one. The listeners start now, each reading what it has seen on a thread of its own (R91).
    let _ = crate::at_rest::fired();
    crate::at_rest::fail_folder(&dir);
    crate::faults::fail(crate::at_rest::FAILING, false);
    let mut queues_held = crate::dequeue::Held::default();
    let mut feeds_held = crate::subscriptions::Held::default();
    crate::dequeue::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut queues_held);
    crate::subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut feeds_held);
    let mut found: Vec<String> = Vec::new();
    for listing in ["queues", "feeds"] {
        let held = soon(|| send(&state, &ada, "GET", &format!("/deployments/listens/{listing}"), Value::Null).1.contains("unreadable"));
        if !held {
            found.push(format!("the listener of /deployments/listens/{listing} never held: {}", send(&state, &ada, "GET", &format!("/deployments/listens/{listing}"), Value::Null).1));
        }
    }
    // Each hook's answers: those it failed, those of them that said where or why, and what of.
    let mut hooks: BTreeMap<String, (usize, usize, BTreeSet<String>)> = BTreeMap::new();
    let starting = hooks_fired(&mut hooks, &names, false);
    let mut codes: BTreeMap<u16, usize> = BTreeMap::new();
    let whys = [crate::at_rest::MADE_TO_FAIL, "os error"];
    // Each answer looked at: what of where or why it says, and which hooks failed for it.
    let mut look = |who: &str, method: &str, path: &str, code: u16, said: &str| -> Vec<String> {
        *codes.entry(code).or_default() += 1;
        // A GET that says where to ada before the fault too (one that views the host) may say it
        // still: never why. The admin's warnings in a GET to her are hers, left out
        // (`outside_warnings`); the same GET to hal is held whole, so a warning that reaches
        // whoever doesn't view the host is found.
        let also = method == "GET" && before.get(&(who.to_string(), path.to_string())).copied().unwrap_or(false);
        let said = if method == "GET" && who == "ada" { outside_warnings(said) } else { said.to_string() };
        let said = said.as_str();
        let parts = super::writers::where_or_why(said, &dir, &[crate::at_rest::MADE_TO_FAIL]);
        let parts: Vec<String> = parts.into_iter().filter(|p| !also || whys.contains(&p.as_str())).collect();
        let at = hooks_fired(&mut hooks, &names, !parts.is_empty());
        if let Some(first) = parts.first() {
            found.push(format!(
                "{who} {method} {path} ({code}){}: {parts:?}, from {}: …{}…",
                if also { " [where before the fault too]" } else { "" },
                if at.is_empty() { "no hook".to_string() } else { at.join(", ") },
                around(said, first)
            ));
        }
        at
    };
    for (who, token) in callers {
        for path in &gets {
            let (code, said) = send(&state, token, "GET", path, Value::Null);
            look(who, "GET", path, code, &said);
        }
        for (method, path, body, _) in every_other(&got) {
            let (code, said) = ask(&state, Some(token), method, &path, body);
            look(who, method, &path, code, &said);
        }
        for (method, path, body) in &to_the_host {
            let (code, said) = send(&state, token, method, path, body.clone());
            look(who, method, path, code, &said);
        }
    }
    crate::faults::heal();
    crate::at_rest::heal_folder(&dir);
    // What the server writes by itself as its warnings start (storage.rs `Sweeps::watch`, on the
    // housekeeping tick no request runs): a line in the activity record, which more than the admin
    // reads (release audit R114: it held each warning whole, where and why). Run with the fault
    // healed, the files it failed still noted failing (nothing has read them since), and the
    // activity read again by each caller.
    let _ = crate::at_rest::fired();
    state.sweeps.watch(&state);
    let _ = crate::at_rest::fired();
    for (who, token) in callers {
        let (code, said) = send(&state, token, "GET", "/activity?limit=500", Value::Null);
        assert!(who != "ada" || said.contains("\"warned\""), "the watcher wrote no warning to read: {said}");
        look(who, "GET", "/activity", code, &said);
    }

    // The second pass: each place a request reaches only behind another that passes there, or
    // that reads what was kept before, failed alone (at_rest::fail_sites), for the files under
    // the data folder, with the request that reaches it.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    // A forget's keys: a keyring of its own, so the server's (every test's) keeps its root; a
    // scope sealed under it for each round that forgets one, as a forget of a scope forgotten
    // already sweeps nothing.
    let private = crate::at_rest::Keyring::memory();
    for gone in ["gone", "gone-events"] {
        private.seal_file(&crate::at_rest::deployment(gone), crate::at_rest::role::LOG, b"gone's").unwrap();
    }
    let forgot = "a forget, two hours on (keys::forget, as POST /admin/keys/forget answers)";
    let fresh = |slug: &'static str, port: u16| {
        let text = format!("import {{ flow, route }} from \"holon\"\n\nflow(\"{slug}\", {{ port: {port} }})\n\nroute.get(\"/\", () => \"hi\")\n");
        let (deployed, run) = (deploy_answer(&state, &ada, slug, &text), send(&state, &ada, "POST", &format!("/deployments/{slug}/request"), json!({ "method": "GET", "url": "/", "headers": [], "body": "" })));
        (deployed.0.max(run.0), format!("{} {}", deployed.1, run.1))
    };
    type Ask<'a> = Box<dyn Fn() -> (u16, String) + 'a>;
    let host = || send(&state, &ada, "GET", "/admin/host", Value::Null);
    // GET /admin/host over a log cut into sealed segments, of a deployment of its own: kept's are
    // gone by now, with kept (rob's sweep deletes it last, and a deleted deployment's segments go
    // with it). The deploy, its runs and the cut aren't what this round fails (the fault is the
    // host's walk alone), so the host's answer is the one looked at, as a GET.
    let sealed = || {
        let _ = fresh("fresh-sealed", 18074);
        let run = json!({ "method": "GET", "url": "/", "headers": [], "body": "" });
        for _ in 0..11 {
            let _ = send(&state, &ada, "POST", "/deployments/fresh-sealed/request", run.clone());
        }
        if let Some(d) = state.deployments.lock().unwrap().get("fresh-sealed") {
            d.log.with(|log| {
                log.set_hot(2);
                log.want_cut();
            });
            d.log.cut_if_planned();
        }
        host()
    };
    let alone: Vec<(&str, &str, Ask)> = vec![
        ("events.rs: Events::open", "a deploy, and its run", Box::new(|| fresh("fresh-events", 18071))),
        ("recorder.rs: Log::open_retaining#1", "a deploy, and its run", Box::new(|| fresh("fresh-segments", 18072))),
        ("recorder.rs: Log::open_retaining#2", "a deploy, and its run", Box::new(|| fresh("fresh-log", 18073))),
        ("storage.rs: Sweeps::kept_view#2", "GET /admin/host", Box::new(sealed)),
        ("storage.rs: backup_view#2", "GET /admin/host", Box::new(host)),
        ("blobs.rs: Blobs::get#1", "GET /deployments/texts/export", Box::new(|| send(&state, &ada, "GET", "/deployments/texts/export", Value::Null))),
        ("blobs.rs: Blobs::get#2", "GET /deployments/texts/export", Box::new(|| send(&state, &ada, "GET", "/deployments/texts/export", Value::Null))),
        ("blobs.rs: Blobs::sweep#1", "POST /admin/folders/sweep", Box::new(|| send(&state, &ada, "POST", "/admin/folders/sweep", json!({})))),
        ("blobs.rs: Blobs::sweep#2", "POST /admin/folders/sweep", Box::new(|| send(&state, &ada, "POST", "/admin/folders/sweep", json!({})))),
        ("blobs.rs: Blobs::sweep#3", "POST /admin/folders/sweep", Box::new(|| send(&state, &ada, "POST", "/admin/folders/sweep", json!({})))),
        ("at_rest.rs: LineSeal::try_read_lines", forgot, Box::new(|| (200, crate::keys::forget(&state, &private, "gone-events", "ada", now + 2 * crate::sweep::SPARES_MS).to_string()))),
        ("sweep.rs: pack", forgot, Box::new(|| (200, crate::keys::forget(&state, &private, "gone", "ada", now + 2 * crate::sweep::SPARES_MS).to_string()))),
    ];
    let (mut rounds, mut missed) = (BTreeSet::new(), Vec::new());
    for (site, what, ask) in &alone {
        let Some(place) = sites.get(*site) else {
            missed.push(format!("{site}: a second pass's round for a place not in the sources"));
            continue;
        };
        let _ = crate::at_rest::fired();
        crate::at_rest::fail_sites(&dir, std::slice::from_ref(place));
        let (code, said) = ask();
        crate::at_rest::heal_sites(&dir);
        // A deployment the round opened under the fault may keep a log that can't be read whole
        // for as long as it runs (`Log::unread`: its sealed segments' folder couldn't be listed).
        // What reads every line to tell what a log names fails on it, and a sweep that can't tell
        // removes nothing (release audit R92). So it goes after its round, as its admin would
        // remove it, and the forgets after it sweep what the others name. Nor does a removal keep
        // its records from it, and it removes nothing, since it would delete the runs it couldn't
        // read: its admin restarts the server first, as the warnings say, and here its log is
        // opened again from its files, as a start opens it.
        let logs: Vec<_> = state.deployments.lock().unwrap().iter().map(|(slug, d)| (slug.clone(), std::sync::Arc::clone(&d.log))).collect();
        for (slug, _) in logs.into_iter().filter(|(_, log)| log.each_line(|_| {}).is_err()) {
            let _ = send(&state, &ada, "POST", &format!("/deployments/{slug}/stop"), Value::Null);
            let reopened = crate::recorder::Log::for_deployment(&state.deployments_path, &slug);
            if let Some(d) = state.deployments.lock().unwrap().get_mut(&slug) {
                d.log = std::sync::Arc::new(crate::recorder::LogCell::new(reopened));
            }
            let (removed, why) = send(&state, &ada, "DELETE", &format!("/deployments/{slug}"), Value::Null);
            if removed != 200 {
                missed.push(format!("{slug}: its log can't be read whole after the round of {site}, and its removal answered {removed}: {}", around(&why, "")));
            }
        }
        // A round's request, looked at as itself: a GET as a GET.
        let (method, path) = what.split_once(' ').filter(|(m, p)| m.chars().all(|c| c.is_ascii_uppercase()) && p.starts_with('/')).unwrap_or(("alone", *what));
        let at = look("ada", method, path, code, &said);
        if at.iter().any(|a| a == site || a.strip_prefix(*site).is_some_and(|rest| rest.starts_with(" (on "))) {
            rounds.insert(site.to_string());
        } else {
            missed.push(format!("{site}: failed alone, {what} didn't reach it ({code}: {})", around(&said, "")));
        }
    }
    found.extend(missed);
    // The store's hook failed a read (in the first pass: the second fails none of the store's): a
    // store whose root is spelled otherwise than the data folder (by case, or with a verbatim
    // prefix) would leave its reads unfailed, and the sweep blind to them.
    if !hooks.values().any(|(_, _, what)| what.iter().any(|w| w.contains("<hash>, there"))) {
        found.push("no answer failed at a store read: the store's hook never failed a file that was there".to_string());
    }
    // Each listener stopped as its holder goes.
    drop((queues_held, feeds_held));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&bound);
    let _ = std::fs::remove_file(&env);

    // Every place in the sources has an answer that reached it, or a reason no request does: never
    // both, and a reason for a place the sources no longer hold is stale.
    let answered = |site: &str| hooks.iter().any(|(at, (n, _, _))| *n > 0 && (at == site || at.strip_prefix(site).is_some_and(|rest| rest.starts_with(" (on "))));
    for site in sites.keys() {
        match (answered(site), REASONS.iter().any(|(s, _)| s == site)) {
            (true, true) => found.push(format!("{site}: answered, and REASONS says no request reaches it")),
            (false, false) => found.push(format!("{site}: no answer reached it, and REASONS gives no reason")),
            _ => {}
        }
    }
    for (site, _) in REASONS.iter().filter(|(s, _)| !sites.contains_key(*s)) {
        found.push(format!("{site}: in REASONS, and no place in the sources"));
    }
    let mut table: Vec<String> = sites
        .keys()
        .map(|site| match (hooks.get(site), REASONS.iter().find(|(s, _)| s == site)) {
            (Some((n, said, what)), _) => format!("{site}: {n} answers{}, {said} said where or why ({})", if rounds.contains(site) { " (alone)" } else { "" }, what.iter().cloned().collect::<Vec<_>>().join(", ")),
            (None, Some((_, why))) => format!("{site}: no request reaches it: {why}"),
            (None, None) if answered(site) => format!("{site}: reached on a thread of its own (below)"),
            (None, None) => format!("{site}: no answer"),
        })
        .collect();
    table.extend(hooks.iter().filter(|(site, _)| site.contains(" (on ") || site.contains(" (no site")).map(|(site, (n, said, _))| format!("{site}: {n} answers, {said} said where or why")));
    // The table, passed or not (seen with --nocapture).
    eprintln!("the data folder's hooks, each with the answers it failed or why none does:\n{}", table.join("\n"));
    assert!(
        found.is_empty(),
        "{} answers say where the data folder is or why it failed (codes answered: {codes:?}; the listeners starting failed {starting:?}):\n{}\n\nthe hooks, each with the answers it failed:\n{}",
        found.len(),
        found.join("\n"),
        table.join("\n")
    );
}

/// Guards: what the server warns of by itself is in its activity record in the warning's told
/// words alone, and only for whoever views the host, as the warnings are: ada (the admin) reads the
/// line, without where or why; cy (a viewer on the host), wes (a role reading every flow) and scout
/// (an agent) read no `warned` line at all.
/// Why: every warning went into the activity whole, its file's path and the OS's words, and GET
/// /activity gave all of it to whoever views every flow (release audit R114; shown live with a
/// viewer, an editor and an agent, each with its own token).
/// If it fails: storage.rs `Sweeps::watch` records more than `Warning::told`, or activity.rs
/// `handle_get_activity` gives `warned` lines to whoever doesn't view the host.
#[test]
fn what_the_server_warns_of_is_the_hosts_in_the_activity_in_fixed_words() {
    let dir = std::path::PathBuf::from(unique_temp_path("r114-warned"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut state = test_state_at(Place::parse(&dir.join("deployments.json").display().to_string()));
    state.activity = crate::activity::Activity::beside(&state.deployments_path);
    let (state, _, tokens) = serve(state, Arc::new(Accounts::memory()));
    // A file failing now, as a reader notes one: where and why, the admin's.
    let place = dir.join("seen").join("events.json").display().to_string();
    let told = crate::recorder::RUNS_UNREAD;
    let failed = crate::failing::Failed::new(told, format!("{place}: Access is denied. (os error 5)"));
    crate::failing::note_file(&state.deployments_path, told, &place, Err(&failed));
    state.sweeps.watch(&state);
    for (who, token) in &tokens {
        let (code, said) = send(&state, token, "GET", "/activity?limit=500", Value::Null);
        assert_eq!(code, 200, "{who}: {said}");
        let answer: Value = serde_json::from_str(&said).unwrap();
        let warned: Vec<&str> = answer["entries"].as_array().unwrap().iter().filter(|e| e["action"] == crate::activity::WARNED).map(|e| e["target"].as_str().unwrap_or_default()).collect();
        if *who == "ada" {
            assert!(warned.contains(&told), "ada reads what it warned of in its told words: {warned:?}");
        } else {
            assert!(warned.is_empty(), "{who} doesn't view the host, and reads what it warned of: {warned:?}");
        }
        assert!(!said.contains("r114-warned") && !said.contains("os error"), "{who} reads where or why in the activity: {said}");
    }
    // The admin's warnings say where and why, as they are meant to.
    let (_, warnings) = send(&state, &tokens["ada"], "GET", "/admin/warnings", Value::Null);
    assert!(warnings.contains("r114-warned") && warnings.contains("os error 5"), "{warnings}");
    crate::failing::note_file(&state.deployments_path, told, &place, Ok(()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The hooks that failed since last asked (at_rest::fired), each counted in `hooks` against one
/// more answer, and whether that answer said where or why: their names (`names`, by place), one
/// failed on a thread of its own (a listener's) marked so. Another test's (a thread named for it,
/// failing its own data folder meanwhile) is none of this sweep's.
fn hooks_fired(hooks: &mut BTreeMap<String, (usize, usize, BTreeSet<String>)>, names: &BTreeMap<String, String>, said_where: bool) -> Vec<String> {
    let me = std::thread::current().name().map(str::to_string);
    let mut at: Vec<String> = crate::at_rest::fired()
        .into_iter()
        .filter(|fired| me.as_deref() == Some(fired.on.as_str()) || !fired.on.starts_with("tests::"))
        .map(|fired| {
            let name = names.get(&fired.at).cloned().unwrap_or_else(|| format!("{} (no site in the sources)", fired.at));
            let site = if me.as_deref() == Some(fired.on.as_str()) { name } else { format!("{name} (on {})", fired.on) };
            hooks.entry(site.clone()).or_default().2.insert(fired.what);
            site
        })
        .collect();
    at.sort();
    at.dedup();
    for site in &at {
        let hook = hooks.get_mut(site).unwrap();
        hook.0 += 1;
        hook.1 += usize::from(said_where);
    }
    at
}

/// Every place the server's sources ask the data folder's hook from, by name: its file and the
/// function it is in (`at_rest.rs: Keyring::persist`, as source.rs's `fn_spans` names it), with
/// `#k` after when that function asks it more than once (the k-th, from 1), so a line moved
/// doesn't make it another; each with its place, `file:line`, as the hook notes it
/// (at_rest::fired). A place is a call to one of at_rest::disk's readers, anywhere outside the
/// tests, or one of at_rest.rs's own asks. A `#[cfg(test)]` function's are the tests' (recorder.rs's
/// `Log::lines`): no request reaches them.
fn hook_sites() -> BTreeMap<String, String> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let (mut files, mut sites) = (vec![src.clone()], BTreeMap::new());
    while let Some(at) = files.pop() {
        if at.is_dir() {
            files.extend(std::fs::read_dir(&at).unwrap().flatten().map(|e| e.path()));
            continue;
        }
        let name = at.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        if !name.ends_with(".rs") || name.ends_with("tests.rs") || name.starts_with("tests/") || name.contains("/tests/") {
            continue;
        }
        let text = std::fs::read_to_string(&at).unwrap();
        let code = super::source::inert(&text);
        let (spans, in_test_fns) = (super::source::fn_spans(&code), test_fns(&code));
        // Each function's asks, in order: their lines, from 1.
        let mut by_fn: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (n, (line, test)) in text.lines().zip(super::source::in_test_modules(&text)).enumerate() {
            let test = test || in_test_fns.get(n).copied().unwrap_or(false);
            let line = line.trim();
            let reads = ["disk::read(", "disk::read_to_string(", "disk::open(", "disk::read_dir("].iter().any(|r| line.contains(r));
            let asks = name == "at_rest.rs" && line.contains("data_folder(") && !line.contains("fn data_folder(") && !line.contains("super::data_folder(");
            if !test && !line.starts_with("//") && (reads || asks) {
                let within = super::source::fn_at(&spans, n).unwrap_or("(no function)").to_string();
                by_fn.entry(within).or_default().push(n + 1);
            }
        }
        for (within, lines) in by_fn {
            for (k, line) in lines.iter().enumerate() {
                let site = if lines.len() > 1 { format!("{name}: {within}#{}", k + 1) } else { format!("{name}: {within}") };
                sites.insert(site, format!("{name}:{line}"));
            }
        }
    }
    sites
}

/// What keeps the server's copy of keys.json in its backup folder (storage.rs's `archive`, on the
/// housekeeping tick): a keyring with a file alone, never the tests' (kept in memory).
const BACKUP_COPY: &str = "keys.json copied to the backup folder on the housekeeping tick (storage.rs, archive): what fails is the backup's warning, never an answer, and only a keyring with a file reaches it";

/// What a log's cut-back does on the logs' sync, every second.
const CUT: &str = "a log cut back to what it keeps, on the logs' sync (main.rs, sync_logs): what fails is said on stderr and the cut waits; no answer waits for it";

/// What the housekeeping tick does: counting, archiving, expiring, sweeping texts.
const TICK: &str = "the housekeeping tick (storage.rs): what fails is said on stderr and the warnings, and tried again at the next tick; no answer waits for it";

/// What the server reads as it starts.
const START: &str = "read as the server starts: what fails is said on stderr, and a file it can't go on without stops the start (main.rs, refuse_to_start); no request reaches it";

/// The places in the sources no request reaches, and why: what fails there is said to whoever
/// runs the server (stderr, the warnings), never in an answer. One that an answer reached, or that
/// the sources no longer hold, fails the sweep, as one in neither does.
const REASONS: &[(&str, &str)] = &[
    ("at_rest.rs: read", START),
    ("at_rest.rs: read_sealed_in", "a mail or a message spooled to be sent, read back as the server starts (mails.rs, queues.rs: recover): what fails is said on stderr; no request reaches it"),
    ("at_rest.rs: Keyring::persist#2", "keys.json written by a keyring with a file, which the tests' own isn't (kept in memory): writers.rs's keys_that_cant_be_written_tell_whoever_rotated_or_forgot_neither_where_nor_why sends a rotation and a forget to one that is, made to fail"),
    ("at_rest.rs: Keyring::version", "a key opening what was read: each request's read of what it opens goes through at_rest::disk first (a run log's file and segments opened afresh, release audit R92), which fails before it under the same fault; and a key has no path, so a round can't fail it alone without failing every test's keys"),
    ("at_rest.rs: Keyring::keep_copy_in#1", BACKUP_COPY),
    ("at_rest.rs: Keyring::keep_copy_in#2", BACKUP_COPY),
    ("at_rest.rs: Keyring::keep_copy_in#3", BACKUP_COPY),
    ("heads.rs: Exporter::new", START),
    ("mails.rs: Mails::recover", START),
    ("queues.rs: Queues::recover", START),
    ("recorder.rs: CutMade::catch_up", CUT),
    ("recorder.rs: CutPlan::make", CUT),
    ("recorder.rs: Log::cut_back", CUT),
    ("recorder.rs: Log::rewrite_file", CUT),
    ("storage.rs: Sweeps::count", TICK),
    ("storage.rs: archive#1", TICK),
    ("storage.rs: archive#2", TICK),
    ("storage.rs: expire_removed#1", TICK),
    ("storage.rs: expire_removed#2", TICK),
    ("storage.rs: expire_removed#3", TICK),
    ("storage.rs: expire_removed#4", TICK),
];

/// `text` deployed as `slug` by ada, approving what it asks for: the deploy's answer.
fn deploy_answer(state: &AppState, ada: &str, slug: &str, text: &str) -> (u16, String) {
    let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
    for unit in &plan.units {
        let (code, said) = ask(state, Some(ada), "POST", "/unit", unit.clone());
        if code != 200 {
            return (code, said);
        }
    }
    let (code, preview) = send(state, ada, "POST", "/deploy/preview", json!({ "slug": slug, "unit": plan.unit }));
    if code != 200 {
        return (code, preview);
    }
    let preview: Value = serde_json::from_str(&preview).unwrap();
    send(state, ada, "POST", "/deploy", json!({ "slug": slug, "unit": plan.unit, "document": plan.document, "acknowledge": preview["needs_approval"] }))
}

/// A flow that reads a file of a folder bound to it, its text kept in the data folder (a folder's
/// `keep`: bytes), and says so on its console.
const TEXTS: &str = r#"import { flow, route } from "holon"
import texts from "folder:texts"

flow("texts", { port: 18096 })

route.get("/read", () => {
  console.log("read a.txt")
  return texts["a.txt"]
})
"#;

/// For each line of `code` (made inert), whether it is a `#[cfg(test)]` function's: the attribute
/// to the brace that closes its body.
fn test_fns(code: &str) -> Vec<bool> {
    let lines: Vec<&str> = code.lines().collect();
    let mut test = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != "#[cfg(test)]" {
            i += 1;
            continue;
        }
        let mut m = i + 1;
        while m < lines.len() && (lines[m].trim().is_empty() || lines[m].trim_start().starts_with("#[")) {
            m += 1;
        }
        let item = lines.get(m).map_or("", |l| l.trim_start().trim_start_matches("pub(crate) ").trim_start_matches("pub(super) ").trim_start_matches("pub "));
        if !item.starts_with("fn ") {
            i += 1;
            continue;
        }
        let (mut depth, mut opened, mut j) = (0i32, false, m);
        while j < lines.len() {
            for c in lines[j].chars() {
                match c {
                    '{' => (depth, opened) = (depth + 1, true),
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            if opened && depth <= 0 {
                break;
            }
            j += 1;
        }
        let last = j.min(lines.len() - 1);
        test[i..=last].fill(true);
        i = last + 1;
    }
    test
}

/// The `error` an answer says, or nothing.
fn error_of(said: &str) -> String {
    super::access_table::error_of(said)
}
