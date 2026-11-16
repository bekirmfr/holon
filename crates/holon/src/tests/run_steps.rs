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

    // What every other route is sent: a version of kept, the run waiting, a proposal, the run that
    // kept the value.
    let plan = holon_author::agent::plan(&KEPT.replace("return \"kept\"", "return \"kept, again\"")).unwrap();
    let heads: Value = serde_json::from_str(&send(&state, ada, "GET", "/deployments/kept/log/heads", Value::Null).1).unwrap();
    let seq = heads["entries"].as_array().unwrap().iter().find(|e| e["kind"] == "request").unwrap()["seq"].as_u64().unwrap();
    let runs: Value = serde_json::from_str(&send(&state, ada, "GET", "/deployments/steps/runs", Value::Null).1).unwrap();
    let waiting = runs["runs"][0]["run_id"].as_str().unwrap().to_string();
    let proposal = {
        let other = holon_author::agent::plan(&KEPT.replace("return \"kept\"", "return \"kept, an agent's\"")).unwrap();
        for unit in &other.units {
            assert_eq!(ask(&state, Some(&tokens["scout"]), "POST", "/unit", unit.clone()).0, 200);
        }
        let (code, said) = send(&state, &tokens["scout"], "POST", "/deploy", json!({ "slug": "kept", "unit": other.unit, "document": other.document }));
        assert_eq!(code, 202, "an agent's version waits: {said}");
        serde_json::from_str::<Value>(&said).unwrap()["proposal"]["id"].as_u64().unwrap()
    };
    // ada, who holds it, reads the state a restore would put back.
    let (code, said) = send(&state, ada, "POST", "/deployments/kept/restore", json!({ "run": seq, "state": true, "dry_run": true }));
    assert!(code == 200 && said.contains(KEPT_CANARY), "ada's dry run says the values: {code} {said}");
    let got = Got { units: plan.units.clone(), unit: plan.unit.clone(), document: plan.document.clone(), waiting, proposal, seq };
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

/// The `error` an answer says, or nothing.
fn error_of(said: &str) -> String {
    super::access_table::error_of(said)
}
