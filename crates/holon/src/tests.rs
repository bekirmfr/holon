//! In-process integration tests. No real server, no network, no port — `route()`/`with_cors()`
//! are plain functions over `Request`/`Response`, and `Request::fake_http` builds a real
//! `Request` value without a socket, so every endpoint is exercised exactly as the real server
//! would run it, just called directly. This replaces the manual `curl` walkthroughs used to
//! verify each inch while building this crate — a real gap, named plainly rather than left
//! implicit, once the surface grew past the first couple of endpoints. `run_due_clock_ticks`
//! (pulled out of `spawn_clock_scheduler` for exactly this reason) means the scheduler is
//! tested with a controlled `now`, not real sleeping.

use super::*;
use holon_core::{hash_value, Edge, BOUNDARY};
use holon_host::fire_sourced_with;

mod access_table;
mod scopes;
mod tags;
/// What a handler's text does, against what the same text does as JavaScript.
mod semantics;

pub(crate) fn test_state() -> AppState {
    test_state_with(SharedStore::new(Box::new(MemStore::new())))
}

/// The same over `store`: an on-disk one, say.
fn test_state_with(store: SharedStore) -> AppState {
    test_state_in(store, Place::Nowhere)
}

/// One whose deployments file is at `at`: a run a flow's start parks is kept there too.
fn test_state_at(at: Place) -> AppState {
    test_state_in(SharedStore::new(Box::new(MemStore::new())), at)
}

fn test_state_in(store: SharedStore, at: Place) -> AppState {
    let deployments = Arc::new(Mutex::new(BTreeMap::new()));
    store.1.connect(Arc::clone(&deployments), at.clone());
    AppState {
        store,
        deployments,
        deployments_path: at,
        flows: Mutex::new(BTreeMap::new()),
        flows_path: Place::Nowhere,
        port: TEST_PORT,
        listeners: None,
        data_limits: dataplane::DataLimits::default(),
        policy: crate::sync::RwLock::new(Arc::new(Policy::default())),
        policy_path: Place::Nowhere,
        secrets: Arc::new(SecretStore::memory()),
        folders: Arc::new(folders::Folders::memory()),
        access: access::Access::default(),
        activity: activity::Activity::memory(),
        editor: None,
        sweeps: Default::default(),
        checked: Default::default(),
        proposals: Default::default(),
    }
}

const TEST_PORT: u16 = 7420;

fn host_header() -> (String, String) {
    ("Host".to_string(), format!("127.0.0.1:{TEST_PORT}"))
}

/// A request as the editor sends it: our Host, and for a POST the Content-Type its body is
/// (a JSON body starts with `{` or `[`; preimages and trigger text go as raw bytes).
pub(crate) fn req(method: &str, url: &str, body: Vec<u8>) -> Request {
    req_with(method, url, body, vec![])
}

/// `req`, signed in with `token`.
pub(crate) fn req_as(token: &str, method: &str, url: &str, body: Vec<u8>) -> Request {
    req_with(method, url, body, vec![("Authorization".to_string(), format!("Bearer {token}"))])
}

/// Who signs in on a server with accounts: ada, an admin, signed in; claude, an agent ada owns,
/// with its token, as `holon mcp` signs in.
pub(crate) struct People {
    pub(crate) ada: String,
    pub(crate) claude: String,
}

/// The router in-process, as the server `holon mcp` reaches (mcp.rs `Api`), signed in with
/// `token`: an agent's, as `holon mcp` signs in, or a person's, as the editor does.
pub(crate) struct Signed<'a> {
    pub(crate) state: &'a AppState,
    pub(crate) token: &'a str,
}

impl crate::mcp::Api for Signed<'_> {
    fn server(&self) -> String {
        "the test server".into()
    }
    fn send(&self, method: &str, path: &str, body: Option<crate::mcp::Body>) -> Result<(u16, Vec<u8>), String> {
        let bytes = match body {
            None => vec![],
            Some(crate::mcp::Body::Json(v)) => serde_json::to_vec(&v).unwrap(),
            // `req` sends bytes not starting with `{` as octet-stream.
            Some(crate::mcp::Body::Bytes(b)) => b,
        };
        let response = route(self.state, &req_as(self.token, method, path, bytes));
        Ok((response.status_code, body_bytes(response)))
    }
}

/// Give `state` accounts: ada and claude (above).
pub(crate) fn with_people(state: &mut AppState) -> People {
    use crate::accounts::{Accounts, Kind};
    let accounts = Arc::new(Accounts::memory());
    let (ada, _) = accounts.setup(&accounts.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
    accounts.add("claude", Kind::Agent, "agent", "ada", "ada").unwrap();
    let (claude, _) = accounts.create_token("claude", "Claude Code", "ada").unwrap();
    state.access = access::Access::new(accounts, vec![], vec![]);
    People { ada, claude }
}

fn req_with(method: &str, url: &str, body: Vec<u8>, more: Vec<(String, String)>) -> Request {
    let mut headers = vec![host_header()];
    headers.extend(more);
    if matches!(method, "POST" | "PUT" | "PATCH") {
        let json = matches!(body.first(), Some(b'{' | b'['));
        let ty = if json { "application/json" } else { "application/octet-stream" };
        headers.push(("Content-Type".to_string(), ty.to_string()));
    }
    Request::fake_http(method, url, headers, body)
}

/// A data-plane request with no headers.
fn fake_data(method: &str, url: &str, body: Vec<u8>) -> data::DataRequest {
    data::DataRequest { method: method.into(), url: url.into(), headers: vec![], body, by: None }
}

pub(crate) fn body_bytes(response: Response) -> Vec<u8> {
    let (mut reader, _) = response.data.into_reader_and_size();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).unwrap();
    buf
}

fn body_json(response: Response) -> serde_json::Value {
    crate::wire::read_json(&body_bytes(response)).expect("response body was not valid JSON")
}

/// A unique path in the OS temp dir, for the one test that needs real disk IO
/// (persistence) rather than the `:memory:` isolation every other test uses. Not a
/// directory or file by itself — `FileStore::new` creates the directory it needs;
/// `std::fs::write` creates the file it needs.
fn unique_temp_path(label: &str) -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!("holon-server-test-{label}-{}-{nanos}", std::process::id()));
    path.to_string_lossy().to_string()
}

/// `store_read(key)` — reads this deployment's own state at `key`.
fn test_store_read(key: &str) -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "store_read".into() },
        params: vec![Param::frozen("key", key)],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Any }],
    }
}

/// `store_write(key)` — writes this deployment's own state at `key`.
fn test_store_write(key: &str) -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "store_write".into() },
        params: vec![Param::frozen("key", key)],
        inputs: vec![Port { name: "value".into(), ty: Type::Any }],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    }
}

/// A flow whose one action, `put`, writes what it is sent at `key`: the state most tests here
/// keep, written by one run and still there for the next (checked through
/// `GET /deployments/<slug>/state`).
fn writer_text(key: &str) -> String {
    format!("import {{ action, flow }} from \"holon\"\nimport state from \"state\"\n\nflow(\"writer\")\n\nexport const put = action((value) => {{\n  state.{key} = value\n}})\n")
}

/// A flow without state: its action `echo` answers what it is sent.
const ECHO: &str = "import { action, flow } from \"holon\"\n\nflow(\"echo\")\n\nexport const echo = action((value) => value)\n";

/// A flow whose action `bump` adds one to `count` each run.
const COUNTER: &str = "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"counter\")\n\nexport const bump = action(() => {\n  state.count = (state.count ?? 0) + 1\n})\n";

/// A flow of one clock: every 5 seconds it writes the time of the tick at `last_tick`.
const TICKER: &str = "import { every, flow } from \"holon\"\nimport state from \"state\"\nimport { now } from \"holon:clock\"\n\nflow(\"tick\")\n\nevery(\"5s\", () => {\n  state.last_tick = now\n})\n";

/// `text` compiled with the language and its units staged: its unit, to deploy under any slug.
fn stage_text(state: &AppState, text: &str) -> Hash {
    let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
    for u in &plan.units {
        assert_eq!(route(state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    parse_hash(&plan.unit).unwrap()
}

/// [`writer_text`] staged: its unit.
fn stage_writer(state: &AppState, key: &str) -> Hash {
    stage_text(state, &writer_text(key))
}

/// Run `slug`'s action `name`, sending `value` as its JSON.
fn run_action_named(state: &AppState, slug: &str, name: &str, value: serde_json::Value) -> Response {
    route(state, &req("POST", &format!("/trigger/{slug}/{name}"), serde_json::to_vec(&value).unwrap()))
}

/// A writer's run: `text` sent to its `put`.
fn put(state: &AppState, slug: &str, text: &str) -> Response {
    run_action_named(state, slug, "put", serde_json::json!(text))
}

/// A flow whose action `call` sends a GET for the path it is sent to `host`, and answers with the
/// response.
fn caller_text(host: &str) -> String {
    format!("import {{ action, flow }} from \"holon\"\nimport api from \"host:{host}\"\n\nflow(\"caller\")\n\nexport const call = action(async (path) => {{\n  const r = await api.get(path)\n  return r\n}})\n")
}

// ── /health ──────────────────────────────────────────────────────────────────────────

#[test]
fn health_check() {
    let state = test_state();
    let r = route(&state, &req("GET", "/health", vec![]));
    assert_eq!(r.status_code, 200);
    assert_eq!(body_bytes(r), b"ok");
}

#[test]
fn version_says_the_hosts() {
    let state = test_state();
    let r = route(&state, &req("GET", "/version", vec![]));
    assert_eq!(r.status_code, 200);
    assert_eq!(body_bytes(r), env!("CARGO_PKG_VERSION").as_bytes());
}

// ── /unit ────────────────────────────────────────────────────────────────────────────

#[test]
fn unit_round_trips() {
    let state = test_state();
    let bytes = encode_preimage(&test_unit()).unwrap();
    let expected_hash = to_hex(&hash_unit(&test_unit()).unwrap());

    let post = route(&state, &req("POST", "/unit", bytes.clone()));
    assert_eq!(post.status_code, 200);
    assert_eq!(body_json(post)["hash"], expected_hash);

    let get = route(&state, &req("GET", &format!("/unit/{expected_hash}"), vec![]));
    assert_eq!(get.status_code, 200);
    assert_eq!(body_bytes(get), bytes);
}

#[test]
fn unit_get_rejects_bad_hash_and_404s_a_missing_one() {
    let state = test_state();
    assert_eq!(route(&state, &req("GET", "/unit/deadbeef", vec![])).status_code, 400);

    let zeros = "0".repeat(64);
    assert_eq!(route(&state, &req("GET", &format!("/unit/{zeros}"), vec![])).status_code, 404);
}

// ── /deploy + /trigger ───────────────────────────────────────────────────────────────

#[test]
fn trigger_unknown_slug_is_404() {
    let state = test_state();
    assert_eq!(route(&state, &req("POST", "/trigger/nope", b"x".to_vec())).status_code, 404);
}

#[test]
fn deploy_rejects_a_unit_with_no_driver_capability() {
    let state = test_state();
    route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
    let leaf_hash = hash_unit(&test_unit()).unwrap();
    let body =
        serde_json::to_vec(&serde_json::json!({"slug": "nope", "unit": to_hex(&leaf_hash)})).unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 400);
}

// ── the clock scheduler, with a controlled clock — no real sleeping ────────────────────

/// A clock ticks when it is due and not before: due at deploy, then once its period has passed,
/// each tick kept in the deployment's history with what it did.
#[test]
fn clock_ticks_fire_when_due_and_not_before() {
    let state = test_state();
    let unit = stage_text(&state, TICKER);
    let ack = serde_json::json!({ "acknowledge": ["Clock", "StateWrite(\"last_tick\")"] });
    assert_eq!(deploy_json(&state, "tick", unit, ack).status_code, 200);

    let t0 = SystemTime::now();

    // Due immediately — a clock is due at deploy.
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0);
    let h1 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
    assert_eq!(body_json(h1)["ticks"].as_array().unwrap().len(), 1);

    // Not due yet — only 1s of a 5s period has passed.
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(1));
    let h2 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
    assert_eq!(body_json(h2)["ticks"].as_array().unwrap().len(), 1);

    // Due again.
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(6));
    let h3 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
    let ticks = body_json(h3)["ticks"].as_array().unwrap().clone();
    assert_eq!(ticks.len(), 2);
    assert_eq!(ticks[0]["performed"][0]["capability"], "StateWrite(\"last_tick\")");
}

/// A clock run's history is redacted as its log entry is: whoever may read the deployment reads
/// it. Here the write of a secret into state is refused, but the run's outputs still hold it; and a
/// write of something computed from it is refused too, its output withheld.
#[test]
fn a_clock_runs_history_never_shows_a_secret() {
    let state = test_state();
    let plan = holon_author::agent::plan("import { base64, every, flow } from \"holon\"\nimport state from \"state\"\nimport api_key from \"secret:api_key\"\n\nflow(\"leak\")\n\nevery(\"5s\", () => {\n  state.k = api_key\n  state.encoded = base64(api_key)\n})\n").unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let unit = parse_hash(&plan.unit).unwrap();
    let body = serde_json::json!({ "slug": "leak", "unit": plan.unit });
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&body).unwrap())));
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap();
    assert_eq!(deploy_json(&state, "leak", unit, serde_json::json!({ "acknowledge": asked })).status_code, 200);
    let value = "sk_live_kept_off_the_history";
    let set = serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap();
    assert_eq!(route(&state, &req("POST", "/deployments/leak/secrets/api_key", set)).status_code, 200);

    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, SystemTime::now());
    let history = String::from_utf8(body_bytes(route(&state, &req("GET", "/deployments/leak/history", vec![])))).unwrap();
    assert!(history.contains("\"ticks\":[{"), "it ticked: {history}");
    assert!(!history.contains(value), "the history leaks it: {history}");
    assert!(history.contains("‹secret:api_key v1 #"), "{history}");
    assert!(!history.contains("c2tfbGl2ZV9rZXB0") && history.contains("‹withheld: computed from a secret›"), "{history}");
    let current = body_json(route(&state, &req("GET", "/deployments/leak/state", vec![])));
    assert!(current["state"].get("encoded").is_none(), "the derived write was refused: {current}");
    assert!(state.deployments.lock().unwrap()["leak"].history[0].outputs.iter().flatten().all(|o| !format!("{o:?}").contains(value)), "not kept either");
}

#[test]
fn deployments_history_for_unknown_slug_is_404() {
    let state = test_state();
    assert_eq!(route(&state, &req("GET", "/deployments/nope/history", vec![])).status_code, 404);
}

/// Deleting a deployment takes its actions with it: a running one is stopped first, and once it
/// is gone its action is not found, as for a slug never deployed, and it is off the list.
#[test]
fn delete_deployment_stops_its_actions_and_removes_it_from_the_list() {
    let state = test_state();
    let unit = stage_text(&state, ECHO);
    assert_eq!(deploy_json(&state, "echo", unit, serde_json::json!({ "acknowledge": ["Action(\"echo\")"] })).status_code, 200);
    let echo = || run_action_named(&state, "echo", "echo", serde_json::json!("x")).status_code;

    // Works before deletion.
    assert_eq!(echo(), 200);

    assert_eq!(route(&state, &req("DELETE", "/deployments/echo", vec![])).status_code, 409, "stop it first");
    assert_eq!(route(&state, &req("POST", "/deployments/echo/stop", vec![])).status_code, 200);
    let deleted = route(&state, &req("DELETE", "/deployments/echo", vec![]));
    assert_eq!(deleted.status_code, 200);
    let deleted = body_json(deleted);
    assert_eq!(deleted["unit"], to_hex(&unit));
    assert_eq!(deleted["records"], serde_json::Value::Null, "kept in memory only, its records have nowhere to go");

    // 404s after deletion — same as a slug that was never deployed.
    assert_eq!(echo(), 404);
    let list = route(&state, &req("GET", "/deployments", vec![]));
    assert_eq!(body_json(list)["deployments"].as_array().unwrap().len(), 0);
}

/// Deleting a clock's deployment stops its ticks: the scheduler only acts on what is in the table.
#[test]
fn delete_deployment_stops_future_clock_ticks() {
    let state = test_state();
    let unit = stage_text(&state, TICKER);
    let ack = serde_json::json!({ "acknowledge": ["Clock", "StateWrite(\"last_tick\")"] });
    assert_eq!(deploy_json(&state, "tick", unit, ack).status_code, 200);

    let t0 = SystemTime::now();
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0);
    let h1 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
    assert_eq!(body_json(h1)["ticks"].as_array().unwrap().len(), 1);

    assert_eq!(route(&state, &req("POST", "/deployments/tick/stop", vec![])).status_code, 200);
    assert_eq!(route(&state, &req("DELETE", "/deployments/tick", vec![])).status_code, 200);

    // The scheduler now has nothing to find for this slug — a tick "due" by the old
    // schedule simply doesn't fire, because the deployment isn't in the table anymore.
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(6));
    assert_eq!(route(&state, &req("GET", "/deployments/tick/history", vec![])).status_code, 404);
}

#[test]
fn delete_unknown_deployment_is_404() {
    let state = test_state();
    let answer = route(&state, &req("DELETE", "/deployments/nope", vec![]));
    assert_eq!(answer.status_code, 404);
    assert_eq!(body_json(answer)["error"], "no deployment \"nope\"", "it says why, for holon remove to print");
}

// ── capability-consent ───────────────────────────────────────────────────────────────

/// A deploy acknowledges the whole manifest its unit derives, or is refused: none of it, or part
/// of it, isn't enough.
#[test]
fn deploy_refuses_without_full_capability_acknowledgment() {
    let state = test_state();
    let writer = stage_writer(&state, "k");

    // No acknowledgment at all.
    assert_eq!(deploy_json(&state, "w", writer, serde_json::json!({})).status_code, 400);

    // Only half the real manifest acknowledged — still refused, not partially accepted.
    let partial = serde_json::json!({ "acknowledge": ["Action(\"put\")"] });
    assert_eq!(deploy_json(&state, "w", writer, partial).status_code, 400);

    // The whole derived manifest acknowledged — succeeds.
    let full = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, full).status_code, 200);
}

/// A unit nothing would start is refused, saying what to add: a handler deployed without its
/// server, and a server with no handler.
#[test]
fn a_deploy_nothing_would_start_is_refused_saying_what_to_add() {
    let state = test_state();
    let refused = |unit: Hash| {
        let r = deploy_json(&state, "idle", unit, serde_json::json!({ "acknowledge": [] }));
        assert_eq!(r.status_code, 400);
        assert_eq!(body_json(r)["error"], crate::deploy::STARTS_NOTHING);
    };
    // Echo's action, the unit its server runs, deployed alone (the plan's units are children first).
    stage_text(&state, ECHO);
    let units = holon_author::agent::plan(ECHO).unwrap().units;
    let handler = decode_preimage(&units[units.len() - 2]).unwrap();
    assert_eq!(holon_core::declaration_of(&handler), Some("action"));
    refused(hash_unit(&handler).unwrap());
    refused(stage(&state, &Unit { body: UnitBody::Composite { children: vec![], edges: vec![] }, params: vec![Param::frozen("declare", "server")], inputs: vec![], outputs: vec![] }));
    assert_eq!(body_json(route(&state, &req("GET", "/deployments", vec![])))["deployments"], serde_json::json!([]));
}

/// A flow's outbound host puts that host in its manifest: deploy needs it acknowledged, by name,
/// and "the network" in general isn't enough.
#[test]
fn deploy_needs_each_requested_host_acknowledged_by_name() {
    let state = test_state();
    let unit = stage_text(&state, &caller_text("api.example.com"));
    let deploy = |ack: serde_json::Value| deploy_json(&state, "caller", unit, serde_json::json!({ "acknowledge": ack }));
    let refused = deploy(serde_json::json!(["Action(\"call\")", "NetworkEgress"]));
    assert_eq!(refused.status_code, 400);
    assert!(body_json(refused)["error"].as_str().unwrap().contains("HttpRequest(\"api.example.com\")"));
    assert_eq!(deploy(serde_json::json!(["Action(\"call\")", "HttpRequest(\"api.example.com\")"])).status_code, 200);
}

/// A deployed flow's outbound request goes through the round loop and the outbound rules: here the
/// host is this machine (127.0.0.1), which no deployment may reach without an explicit grant, so
/// the request is refused at perform time. The refusal is the node's response, which the action
/// answers with, and the flow log records the round: the request, the response, and its node.
#[test]
fn a_deployed_request_is_performed_between_rounds_and_logged() {
    let state = test_state();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let unit = stage_text(&state, &caller_text(&host));
    let ack = serde_json::json!({ "acknowledge": ["Action(\"call\")", format!("HttpRequest({host:?})")] });
    assert_eq!(deploy_json(&state, "caller", unit, ack).status_code, 200);
    let answered = body_json(run_action_named(&state, "caller", "call", serde_json::json!("/status")));
    assert_eq!(answered["answer"]["error"], "blocked_address", "this machine is a private address: {answered}");

    let entry = &log_entries(&state, "caller")[0];
    let exchange = &entry["rounds"][0]["exchanges"][0];
    // The node's path leads, through the units as stored, from the action's handler to the request.
    let handler = state.deployments.lock().unwrap()["caller"].server.as_ref().unwrap().actions[0].handler;
    let mut node = state.store.get(&handler).unwrap().unwrap();
    for i in exchange["path"].as_array().unwrap() {
        let UnitBody::Composite { children, .. } = &node.body else { panic!("{:?} leads past a leaf", exchange["path"]) };
        node = state.store.get(&children[i.as_u64().unwrap() as usize]).unwrap().unwrap();
    }
    assert!(matches!(&node.body, UnitBody::Primitive { name } if name == "http_request"), "the request node, by path: {:?}", node.body);
    let url = |v: &serde_json::Value| v["value"].as_array().unwrap().iter().find(|kv| kv[0] == "url").unwrap()[1]["value"].clone();
    assert_eq!(url(&exchange["request"]), format!("https://{host}/status"));
    let error = exchange["response"]["value"].as_array().unwrap().iter().find(|kv| kv[0] == "error").unwrap()[1]["value"].clone();
    assert_eq!(error, "blocked_address");
    assert!(entry["error"].is_null(), "a refused request doesn't fail the run");
    assert!(entry["timeline"].as_array().unwrap().iter().any(|t| t["round"] == 1), "timings from both rounds");
    drop(listener);
}

// ── permissions: the manifest asks, the host grants ─────────────────────────────────

fn deploy_json(state: &AppState, slug: &str, unit: Hash, extra: serde_json::Value) -> Response {
    let mut body = serde_json::json!({ "slug": slug, "unit": to_hex(&unit) });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    route(state, &req("POST", "/deploy", serde_json::to_vec(&body).unwrap()))
}

/// What policy grants without asking needs no acknowledgment; a redeploy asks only for what the
/// previous grant didn't cover; and what policy never grants is refused, naming why.
#[test]
fn deploy_asks_for_what_policy_doesnt_grant_and_only_once() {
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nauto = [\"StateWrite(*)\"]").unwrap());
    let writer = stage_writer(&state, "k");
    // StateWrite is granted by policy; running its action isn't.
    let refused = deploy_json(&state, "w", writer, serde_json::json!({}));
    assert_eq!(refused.status_code, 400);
    let message = body_json(refused)["error"].as_str().unwrap().to_string();
    assert!(message.contains("Action(\"put\")") && !message.contains("StateWrite"), "{message}");
    assert_eq!(deploy_json(&state, "w", writer, serde_json::json!({ "acknowledge": ["Action(\"put\")"] })).status_code, 200);
    // Redeploying the same thing asks for nothing; raising a limit asks for that alone.
    assert_eq!(deploy_json(&state, "w", writer, serde_json::json!({})).status_code, 200);
    let raise = serde_json::json!({ "requirements": { "limits": { "request_timeout_ms": 60000 } } });
    let asked = deploy_json(&state, "w", writer, raise.clone());
    assert!(body_json(asked)["error"].as_str().unwrap().contains("Limit(request_timeout_ms=60000)"));
    let mut approved = raise.clone();
    approved["acknowledge"] = serde_json::json!(["Limit(request_timeout_ms=60000)"]);
    assert_eq!(deploy_json(&state, "w", writer, approved).status_code, 200);
    let grant = body_json(route(&state, &req("GET", "/deployments/w/grant", vec![])));
    assert!(grant["grant"]["items"].as_array().unwrap().contains(&serde_json::json!("Limit(request_timeout_ms=60000)")));
    // Above the ceiling, or an exception this policy never grants: refused outright.
    let huge = deploy_json(&state, "w", writer, serde_json::json!({ "requirements": { "limits": { "max_requests": 5000 } } }));
    assert!(body_json(huge)["error"].as_str().unwrap().contains("ceiling of 1000"));
    let private = deploy_json(&state, "w", writer, serde_json::json!({ "requirements": { "private_addresses": true } }));
    assert!(body_json(private)["error"].as_str().unwrap().contains("never grants private addresses"));
    let plain = deploy_json(&state, "w", writer, serde_json::json!({ "requirements": { "plain_http": ["bma"] } }));
    let message = body_json(plain)["error"].as_str().unwrap().to_string();
    assert!(message.contains("never grants plain HTTP, asked for \"bma\"") && message.contains("plain HTTP to"), "{message}");
    // A preview says what a deploy would ask for, and changes nothing.
    let preview = |extra: serde_json::Value| {
        let mut body = serde_json::json!({ "slug": "w", "unit": to_hex(&writer) });
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&body).unwrap())))
    };
    let p = preview(serde_json::json!({ "requirements": { "limits": { "max_requests": 80 } } }));
    assert_eq!(p["needs_approval"], serde_json::json!(["Limit(max_requests=80)"]), "the rest is already granted");
    assert!(p["already_granted"].as_array().unwrap().contains(&serde_json::json!("Action(\"put\")")));
    assert!(p["refused"].is_null());
    let p = preview(serde_json::json!({ "requirements": { "private_addresses": true } }));
    assert!(p["refused"].as_str().unwrap().contains("private addresses"));
    // The policy and the deployment's manifest are readable.
    let policy = body_json(route(&state, &req("GET", "/permissions", vec![])));
    assert_eq!(policy["defaults"]["max_requests"], 50);
    let manifest = body_json(route(&state, &req("GET", "/deployments/w/manifest", vec![])));
    assert_eq!(manifest["not_granted"], serde_json::json!([]));
}

/// How long a deployment's log keeps its runs is a deploy setting, held to the host's policy:
/// keeping fewer is the author's to choose, keeping more than the default asks (it is the host's
/// disk), and past the ceiling is refused. A grant lowered later holds the log at once.
/// A flow's `keep` line is its retention: the plan an agent deploys (`holon mcp`) asks for it,
/// and the log keeps that many runs.
#[test]
fn a_flows_keep_line_is_the_retention_its_deploy_asks_for() {
    let state = test_state();
    let plan = holon_author::agent::plan("import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", { keep: { runs: 3 } })\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n").unwrap_or_else(|e| panic!("{e}"));
    let requirements = plan.requirements.clone().expect("its keep line");
    assert_eq!(requirements, serde_json::json!({ "limits": { "keep_runs": 3 } }));
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "kept", "unit": plan.unit, "requirements": requirements })).unwrap())));
    let ask = serde_json::json!({ "acknowledge": preview["needs_approval"], "requirements": requirements });
    assert_eq!(deploy_json(&state, "kept", parse_hash(&plan.unit).unwrap(), ask).status_code, 200);
    for _ in 0..5 {
        assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);
    }
    let log = body_json(route(&state, &req("GET", "/deployments/kept/log", vec![])));
    let seqs: Vec<u64> = log["entries"].as_array().unwrap().iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, [2, 3, 4]);
}

/// What a flow writes with `console.log` is kept beside its runs, not in them (logs.rs): each line
/// holds its run and its level, a run's record holds no line and no log capability, and the deploy
/// asks for no approval to write one.
#[test]
fn a_flow_writes_log_lines_beside_its_runs_and_not_in_them() {
    let mut state = test_state();
    let deploy = |slug: &str, text: &str| {
        let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
        for u in &plan.units {
            assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
        }
        let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": slug, "unit": plan.unit })).unwrap())));
        assert!(!preview["needs_approval"].to_string().contains("Log("), "a log line is no permission to approve: {preview}");
        assert_eq!(deploy_json(&state, slug, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": preview["needs_approval"] })).status_code, 200);
    };
    let flow = |name: &str| format!("import {{ action, flow }} from \"holon\"\nimport state from \"state\"\n\nflow(\"{name}\")\n\nexport const bump = action(() => {{\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  console.log(\"bumped\", n)\n  console.error(\"careful\")\n}})\n");
    deploy("noisy", &flow("noisy"));
    deploy("quiet", &flow("quiet"));
    for _ in 0..2 {
        assert_eq!(route(&state, &req("POST", "/trigger/noisy/bump", b"{}".to_vec())).status_code, 200);
    }
    assert_eq!(route(&state, &req("POST", "/trigger/quiet/bump", b"{}".to_vec())).status_code, 200);

    let lines = state.folders.logs.read("noisy", &crate::logs::Query::default());
    assert_eq!(
        lines.iter().map(|l| (l.run, l.level.as_str(), l.text.as_str())).collect::<Vec<_>>(),
        [(0, "log", "bumped 1"), (0, "error", "careful"), (1, "log", "bumped 2"), (1, "error", "careful")],
        "each line with its run and its level, oldest first"
    );
    assert_eq!(state.folders.logs.read("quiet", &crate::logs::Query::default()).len(), 2, "a deployment's own");

    // A person reads them through the route, filtered; an agent is refused, since no grant lets it
    // yet; and a deployment that isn't there is none.
    let p = with_people(&mut state);
    let read = body_json(route(&state, &req_as(&p.ada, "GET", "/deployments/noisy/logs?level=error&until=0", vec![])));
    assert_eq!(read["lines"].as_array().unwrap().iter().map(|l| l["text"].as_str().unwrap()).collect::<Vec<_>>(), ["careful"], "an error, up to run 0: {read}");
    assert_eq!(route(&state, &req_as(&p.claude, "GET", "/deployments/noisy/logs", vec![])).status_code, 403);
    assert_eq!(route(&state, &req_as(&p.ada, "GET", "/deployments/nope/logs", vec![])).status_code, 404);

    // Not in the run's record: its text holds neither a line nor the capability that wrote one.
    let runs = body_json(route(&state, &req_as(&p.ada, "GET", "/deployments/noisy/runs", vec![])));
    let record = runs["runs"].to_string();
    assert!(!record.contains("bumped") && !record.contains("careful") && !record.contains("Log("), "{record}");
}

/// A log line never shows a secret (logs.rs): a line computed from one, its value in it as it is or
/// encoded, is withheld, as a performed effect's payload is; a line at another level stays.
#[test]
fn a_log_line_never_shows_a_secret() {
    let state = test_state();
    let text = "import { action, base64, flow } from \"holon\"\nimport api_key from \"secret:api_key\"\n\nflow(\"leaky\")\n\nexport const run = action(() => {\n  console.log(\"the key is\", api_key)\n  console.warn(\"encoded\", base64(api_key))\n  console.debug(\"done\")\n})\n";
    let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "leaky", "unit": plan.unit })).unwrap())));
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap();
    assert_eq!(deploy_json(&state, "leaky", parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": asked })).status_code, 200);
    let value = "sk_live_kept_out_of_the_lines";
    let set = serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap();
    assert_eq!(route(&state, &req("POST", "/deployments/leaky/secrets/api_key", set)).status_code, 200);
    assert_eq!(route(&state, &req("POST", "/trigger/leaky/run", b"{}".to_vec())).status_code, 200);

    let lines = state.folders.logs.read("leaky", &crate::logs::Query::default());
    let written: Vec<(&str, &str)> = lines.iter().map(|l| (l.level.as_str(), l.text.as_str())).collect();
    assert_eq!(lines.len(), 3, "{written:?}");
    assert!(lines.iter().all(|l| !l.text.contains(value) && !l.text.contains("c2tfbGl2ZV9rZXB0")), "a line shows it: {written:?}");
    assert_eq!((lines[0].text.as_str(), lines[1].text.as_str()), (crate::taint::WITHHELD, crate::taint::WITHHELD), "withheld: {written:?}");
    assert_eq!(lines[2].text, "done", "a line from nothing secret is written: {written:?}");
}

/// The host at a glance (summary.rs): each deployment's runs by hour, the totals and the newest
/// runs; and a check reads every log's chain again, which the summary then says.
#[test]
fn the_summary_counts_runs_by_hour_and_says_the_last_check_of_the_chain() {
    let state = test_state();
    let plan = holon_author::agent::plan("import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"glance\")\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n").unwrap_or_else(|e| panic!("{e}"));
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "glance", "unit": plan.unit })).unwrap())));
    assert_eq!(deploy_json(&state, "glance", parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": preview["needs_approval"] })).status_code, 200);
    let quiet = body_json(route(&state, &req("GET", "/summary", vec![])));
    assert_eq!(quiet["deployments"][0]["p50_ms"], serde_json::Value::Null, "no runs, no median: {quiet}");
    assert_eq!(quiet["deployments"][0]["last_ms"], serde_json::Value::Null, "no runs, no last: {quiet}");
    for _ in 0..3 {
        assert_eq!(route(&state, &req("POST", "/trigger/glance/bump", b"{}".to_vec())).status_code, 200);
    }
    let summary = body_json(route(&state, &req("GET", "/summary?hours=6", vec![])));
    assert_eq!(summary["hours"], 6);
    let flow = &summary["deployments"][0];
    assert_eq!(flow["slug"], "glance");
    assert_eq!(flow["status"], "running");
    assert_eq!(flow["runs"], serde_json::json!([0, 0, 0, 0, 0, 3]), "{flow}");
    assert_eq!(flow["failed"], serde_json::json!([0, 0, 0, 0, 0, 0]));
    assert_eq!(flow["on_record"], 3);
    assert_eq!(summary["totals"], serde_json::json!({ "runs": 3, "failed": 0, "running": 1, "paused": 0, "stopped": 0 }));
    let latest = summary["latest"].as_array().unwrap();
    assert_eq!(latest.iter().map(|r| r["seq"].as_u64().unwrap()).collect::<Vec<_>>(), [2, 1, 0], "the newest first");
    assert!(latest.iter().all(|r| r.get("request").is_none() && r["failed"] == false), "{latest:?}");
    // Its median run time: the middle of the three it ran.
    let mut took: Vec<f64> = latest.iter().map(|r| r["duration_ms"].as_f64().unwrap()).collect();
    took.sort_by(f64::total_cmp);
    assert_eq!(flow["p50_ms"].as_f64(), Some(took[1]), "{flow}");
    // When it last ran: its newest run's start.
    assert_eq!(flow["last_ms"], latest[0]["at_ms"], "{flow}");
    assert_eq!(summary["chain"]["checked"], serde_json::Value::Null, "no check yet");
    // Checked: every line follows the one before, and the summary says so until the next.
    let check = body_json(route(&state, &req("POST", "/summary/check", vec![])));
    assert_eq!((check["runs"].as_u64(), check["breaks"].as_array().map(Vec::len)), (Some(3), Some(0)), "{check}");
    assert_eq!(check["heads"][0][1], flow["head"]);
    let summary = body_json(route(&state, &req("GET", "/summary", vec![])));
    assert_eq!(summary["hours"], 24);
    assert_eq!(summary["chain"]["checked"]["runs"], 3);
}

#[test]
fn a_store_sweep_clears_the_states_no_kept_run_names_and_nothing_else() {
    let dir = unique_temp_path("sweep");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let state = test_state_with(SharedStore::on_disk(Box::new(disk), values.clone()));
    let plan = holon_author::agent::plan("import { action, flow } from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\n\nflow(\"kept\", { keep: { runs: 3 } })\n\nexport const bump = action(() => {\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  notes[`k${n}`] = n\n})\n").unwrap_or_else(|e| panic!("{e}"));
    let requirements = plan.requirements.clone().unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "kept", "unit": plan.unit, "requirements": requirements })).unwrap())));
    let ask = serde_json::json!({ "acknowledge": preview["needs_approval"], "requirements": requirements });
    assert_eq!(deploy_json(&state, "kept", parse_hash(&plan.unit).unwrap(), ask).status_code, 200);
    for _ in 0..10 {
        assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);
    }
    let log = body_json(route(&state, &req("GET", "/deployments/kept/log", vec![])));
    let kept: Vec<(u64, String)> = log["entries"].as_array().unwrap().iter().map(|e| (e["seq"].as_u64().unwrap(), e["state_after"].as_str().unwrap().to_string())).collect();
    assert_eq!(kept.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [7, 8, 9]);
    let files = || std::fs::read_dir(&values).unwrap().count();
    let before = files();
    let sweep = |spare: u64, dry_run: bool| sweep::sweep(&state.store, &state.deployments, SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64, spare, dry_run, false).unwrap().unwrap();

    // Everything is new: spared, whoever names it.
    let new = sweep(sweep::SPARES_MS, false);
    assert_eq!((new.values, new.spared > 0, files()), (0, true, before));
    // The dry run counts, and removes nothing.
    let dry = sweep(0, true);
    assert!(dry.values > 0 && dry.kept > 0, "{dry:?}");
    assert_eq!(files(), before);
    let swept = sweep(0, false);
    assert_eq!(swept, dry, "the dry run said what it would do");
    assert!(swept.values >= 7, "at least the seven cut runs' snapshots: {swept:?}");
    assert_eq!(files(), before - swept.values);
    assert_eq!(sweep(0, false).values, 0, "and once it has, nothing more");

    // What is kept is whole: each kept run's state, the live state with its collection, the
    // version's document, and a restore to the oldest kept run.
    for (seq, snapshot) in &kept {
        restore::load_snapshot(snapshot, &state.store).unwrap_or_else(|e| panic!("run {seq}'s state: {e}"));
    }
    let notes = body_json(route(&state, &req("GET", "/deployments/kept/collections/notes", vec![])));
    assert_eq!(notes["items"].as_array().unwrap().len(), 10, "{notes}");
    let versions = body_json(route(&state, &req("GET", "/deployments/kept/versions", vec![])));
    assert!(versions.to_string().contains("kept"), "{versions}");
    let back = serde_json::to_vec(&serde_json::json!({ "run": 7, "state": true })).unwrap();
    assert_eq!(route(&state, &req("POST", "/deployments/kept/restore", back)).status_code, 200);
    let after = body_json(route(&state, &req("GET", "/deployments/kept/collections/notes", vec![])));
    assert_eq!(after["items"].as_array().unwrap().len(), 8, "run 7 left eight notes: {after}");
    assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);

    // A value nothing names, put just now, waits out the hour; the endpoint says so.
    state.store.clone().put_value(&Value::Text("nobody's".into())).unwrap();
    let answer = body_json(route(&state, &req("POST", "/admin/store/sweep", br#"{"dry_run":true}"#.to_vec())));
    assert_eq!(answer["dry_run"], true);
    assert_eq!(answer["swept"]["values"], 0, "{answer}");
    assert!(answer["spared"]["values"].as_u64().unwrap() > 0, "{answer}");
    assert!(sweep(0, false).values > 0, "past its hour, it goes");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The server sweeps its store on its own (R6): what an admin's sweep would remove, the same way;
/// and not again until something is put or a sweep spared something.
#[test]
fn the_server_sweeps_its_store_on_its_own_and_not_again_for_nothing() {
    let dir = unique_temp_path("sweep-scheduled");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let state = test_state_with(SharedStore::on_disk(Box::new(disk), values.clone()));
    let plan = holon_author::agent::plan("import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", { keep: { runs: 2 } })\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n").unwrap_or_else(|e| panic!("{e}"));
    let requirements = plan.requirements.clone().unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "kept", "unit": plan.unit, "requirements": requirements })).unwrap())));
    let ask = serde_json::json!({ "acknowledge": preview["needs_approval"], "requirements": requirements });
    assert_eq!(deploy_json(&state, "kept", parse_hash(&plan.unit).unwrap(), ask).status_code, 200);
    let bump = || assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);
    for _ in 0..6 {
        bump();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    let mut scheduled = sweep::Scheduled::default();
    let first = scheduled.sweep(&state.store, &state.deployments, now).unwrap().unwrap();
    assert_eq!((first.values, first.spared > 0), (0, true), "within the hour, all spared: {first:?}");
    let again = scheduled.sweep(&state.store, &state.deployments, now).unwrap();
    assert!(again.is_some(), "a sweep that spared some is made again");
    // Past the hour: what the cut runs left goes, as an admin's dry run says it would.
    let later = now + 2 * sweep::SPARES_MS;
    let would = sweep::sweep(&state.store, &state.deployments, later, sweep::SPARES_MS, true, false).unwrap().unwrap();
    let swept = scheduled.sweep(&state.store, &state.deployments, later).unwrap().unwrap();
    // The same, but that the server's own sweep packs what it keeps, past the hour too.
    assert_eq!(sweep::Swept { packed: 0, packed_bytes: 0, ..swept.clone() }, would);
    assert_eq!(swept.packed, swept.kept, "{swept:?}");
    assert!(swept.values > 0 && swept.spared == 0, "{swept:?}");
    assert_eq!(scheduled.sweep(&state.store, &state.deployments, later).unwrap(), None, "nothing put since, nothing to find");
    bump();
    assert!(scheduled.sweep(&state.store, &state.deployments, later).unwrap().is_some(), "a put since, and it sweeps");
    let log = body_json(route(&state, &req("GET", "/deployments/kept/log", vec![])));
    for e in log["entries"].as_array().unwrap() {
        restore::load_snapshot(e["state_after"].as_str().unwrap(), &state.store).unwrap_or_else(|err| panic!("a kept run's state: {err}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The server sweeps on `[storage] sweep_every`'s schedule (storage.rs): the first round a
/// `sweep_every` after it begins, the next one after that round, none when it is off; and the
/// admin's view says when the next is due and what the last did.
#[test]
fn the_server_sweeps_as_often_as_its_storage_setting_says() {
    let dir = unique_temp_path("sweep-every");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[storage]\nsweep_every = \"1m\"").unwrap());
    let t0 = 1_000_000_000;
    let host = || body_json(route(&state, &req("GET", "/admin/host", vec![])))["sweeps"].clone();
    assert!(!state.sweeps.tick(&state, t0), "the first look starts the clock");
    assert_eq!((host()["every"].as_str(), host()["next_at_ms"].as_u64()), (Some("1m"), Some(t0 + 60_000)));
    assert!(!state.sweeps.tick(&state, t0 + 59_000));
    assert!(state.sweeps.tick(&state, t0 + 60_000), "due a minute after");
    let swept = host();
    assert_eq!(swept["store"]["at_ms"], t0 + 60_000, "{swept}");
    assert_eq!(swept["store"]["error"], serde_json::Value::Null, "{swept}");
    assert_eq!(swept["folders"]["at_ms"], t0 + 60_000, "{swept}");
    assert!(!state.sweeps.tick(&state, t0 + 61_000), "not again until a minute after it");
    // Nothing put since a sweep that spared nothing: the next round has nothing to look at.
    let next = host()["next_at_ms"].as_u64().unwrap();
    assert!(state.sweeps.tick(&state, next));
    assert_eq!(host()["store"]["skipped"], true, "{}", host());
    // Off: never, however long it waits.
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[storage]\nsweep_every = \"off\"").unwrap());
    assert!(!state.sweeps.tick(&state, next + 365 * 86_400_000));
    assert_eq!((host()["every"].as_str(), host()["next_at_ms"].as_u64()), (Some("off"), None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// What the host holds (`GET /admin/host`): its memory and caches against their sizes, the disk
/// the store is on, and what the store holds, counted at the first look and again after a round
/// of sweeps, with what was put since.
#[test]
fn the_host_says_what_it_holds_in_memory_and_on_its_disks() {
    let dir = unique_temp_path("host-view");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let state = test_state_with(SharedStore::on_disk(Box::new(disk), values.clone()));
    let host = || body_json(route(&state, &req("GET", "/admin/host", vec![])));
    let before = host();
    assert_eq!(before["kept"]["store"], serde_json::Value::Null, "not counted before the first look: {before}");
    assert_eq!(before["memory"]["flow_cache"]["budget"], units::FLOW_CACHE_BYTES, "{before}");
    if cfg!(any(target_os = "linux", windows)) {
        assert!(before["memory"]["resident"].as_u64().unwrap() > 1 << 20, "{before}");
        assert!(before["memory"]["peak"].as_u64() >= before["memory"]["resident"].as_u64(), "{before}");
    }
    for i in 0..3 {
        state.store.clone().put_value(&Value::Text(format!("value {i}"))).unwrap();
    }
    let t0 = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    state.sweeps.tick(&state, t0);
    let counted = host();
    let store = &counted["kept"]["store"];
    let on_disk = std::fs::read_dir(&values).unwrap().count() as u64;
    assert_eq!((store["values"].as_u64(), store["put_since"].as_u64()), (Some(on_disk), Some(0)), "{counted}");
    assert!(store["on_disk"].as_u64().unwrap() >= store["bytes"].as_u64().unwrap(), "whole blocks: {counted}");
    state.store.clone().put_value(&Value::Text("one more".into())).unwrap();
    assert_eq!(host()["kept"]["store"]["put_since"], 1);
    let disks = counted["disks"].as_array().unwrap();
    assert_eq!(disks.len(), 1, "one disk, named once: {counted}");
    assert_eq!(disks[0]["holds"], serde_json::json!(["store"]));
    let (size, free, used) = (disks[0]["size"].as_u64().unwrap(), disks[0]["free"].as_u64().unwrap(), disks[0]["used"].as_u64().unwrap());
    assert!(size > 0 && free <= size && used <= size, "{counted}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// What each flow holds (`GET /deployments/<slug>/holds`, and a row each in `GET /admin/host`):
/// its log in memory (about three times the lines its kept runs were written as) and on disk, its
/// live state, its flow in the cache; and its states in the store once a store sweep (an admin's
/// dry run too) has walked them.
#[test]
fn each_flow_says_what_it_holds_in_memory_and_on_disk() {
    let dir = unique_temp_path("holds");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    let plan = holon_author::agent::plan("import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", { keep: { runs: 3 } })\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n").unwrap_or_else(|e| panic!("{e}"));
    let requirements = plan.requirements.clone().unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "kept", "unit": plan.unit, "requirements": requirements })).unwrap())));
    let ask = serde_json::json!({ "acknowledge": preview["needs_approval"], "requirements": requirements });
    assert_eq!(deploy_json(&state, "kept", parse_hash(&plan.unit).unwrap(), ask).status_code, 200);
    for _ in 0..5 {
        assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);
    }
    let holds = || body_json(route(&state, &req("GET", "/deployments/kept/holds", vec![])));
    let h = holds();
    let log = &h["log"];
    assert_eq!(log["entries"], 3, "the runs it keeps: {h}");
    let entry_bytes = log["entry_bytes"].as_u64().unwrap();
    assert!(entry_bytes > 1000, "{h}");
    assert_eq!(log["memory"].as_u64(), Some(entry_bytes * 3));
    assert!(h["state_memory"].as_u64().unwrap() > 0, "it keeps n: {h}");
    let flow_own = h["flow_cache"]["own"].as_u64().unwrap();
    assert!(flow_own > 0, "{h}");
    assert_eq!(h["memory"].as_u64(), Some(entry_bytes * 3 + h["state_memory"].as_u64().unwrap() + flow_own));
    assert_eq!((h["store"].clone(), h["shares_at_ms"].clone()), (serde_json::Value::Null, serde_json::Value::Null), "not counted before a sweep: {h}");
    // A store sweep, an admin's dry run, walks what each names.
    let dry = route(&state, &req("POST", "/admin/store/sweep", br#"{"dry_run":true}"#.to_vec()));
    assert_eq!(dry.status_code, 200);
    let h = holds();
    assert!(h["store"]["values"].as_u64().unwrap() > 0, "its live state and kept runs' states: {h}");
    assert!(h["shares_at_ms"].as_u64().is_some());
    assert_eq!(h["disk"].as_u64(), Some(h["log"]["file"]["on_disk"].as_u64().unwrap() + h["store"]["bytes"].as_u64().unwrap()));
    let host = body_json(route(&state, &req("GET", "/admin/host", vec![])));
    assert_eq!(host["deployments"]["each"][0]["slug"], "kept", "{host}");
    assert_eq!(route(&state, &req("GET", "/deployments/nobody/holds", vec![])).status_code, 404);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The host warns of a disk past `[storage] warn_at` and of a cache at its size most of the last
/// hour, each with what to do; says each once in its activity as it starts; and an admin sets the
/// threshold as the other storage settings.
#[test]
fn the_host_warns_of_a_full_disk_and_a_full_cache_once_each() {
    let dir = unique_temp_path("warnings");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let mut state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    let path = std::path::PathBuf::from(unique_temp_path("warn-policy")).with_extension("toml");
    state.policy_path = path.display().to_string().into();
    let warnings = || body_json(route(&state, &req("GET", "/admin/warnings", vec![])))["warnings"].as_array().cloned().unwrap();
    let set = |body: &str| route(&state, &req("POST", "/admin/storage", body.as_bytes().to_vec()));
    // Nothing near a 2 KiB floor (nor past the 1 KiB it acts at, warned of too): no warning.
    assert_eq!(set(r#"{"warn_at":"2K","full_at":"1K"}"#).status_code, 200);
    assert_eq!(warnings(), Vec::<serde_json::Value>::new());
    // A floor no disk has: its disk warns, with what to do.
    assert_eq!(set(r#"{"warn_at":"1000T","full_at":"1K"}"#).status_code, 200);
    let w = warnings();
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0]["key"].as_str().unwrap().starts_with("disk:"), "{w:?}");
    assert!(w[0]["says"].as_str().unwrap().contains("it warns at 1000T"), "{w:?}");
    assert!(w[0]["todo"].as_str().unwrap().contains("holon host"), "{w:?}");
    assert_eq!(body_json(route(&state, &req("GET", "/admin/host", vec![])))["warnings"].as_array().unwrap().len(), 1);
    // Said once in the activity, however many looks see it.
    let t0 = 1_000_000_000;
    for i in 0..3 {
        state.sweeps.tick(&state, t0 + i);
    }
    let warned = body_json(route(&state, &req("GET", "/activity?limit=50", vec![])))["entries"].as_array().unwrap().iter().filter(|e| e["action"] == "warned").count();
    assert_eq!(warned, 1);
    assert_eq!(set(r#"{"warn_at":"100%"}"#).status_code, 400, "not a threshold");
    assert_eq!(set(r#"{"warn_at":"1K"}"#).status_code, 400, "not past where it acts: full_at is 1K");
    assert_eq!(set(r#"{"warn_at":"2K"}"#).status_code, 200);
    // A value cache at its size at most looks of the last hour (or the five minutes it waits
    // for at the least) warns; a look or two doesn't.
    state.store.4.set_budget(4 << 10);
    let mut hashes = Vec::new();
    for i in 0..40 {
        hashes.push(state.store.clone().put_value(&Value::Text(format!("a value some {i} runs read, {}", "x".repeat(100)))).unwrap());
    }
    // Between each look, runs read more than it keeps: it lets go of some.
    let reads = || {
        for h in &hashes {
            state.store.get_value(h).unwrap();
        }
    };
    for i in 0..3 {
        reads();
        state.sweeps.tick(&state, t0 + 10 + i);
    }
    assert!(warnings().iter().all(|w| w["key"] != "cache:values"), "not yet: {:?}", warnings());
    for i in 0..40 {
        reads();
        state.sweeps.tick(&state, t0 + 20 + i);
    }
    let w = warnings();
    assert!(w.iter().any(|w| w["key"] == "cache:values" && w["todo"].as_str().unwrap().contains("--value-cache")), "{w:?}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&path);
}

/// Past `full_at`, `when_full = "pause"` pauses every deployment that records runs (one whose
/// runs are all unrecorded goes on), and resumes them once back under `warn_at`; each on its
/// timeline. One a person paused stays paused.
#[test]
fn a_full_disk_pauses_what_records_runs_until_there_is_room() {
    // A server whose data folder is on a disk (a temp folder's), so it has one to be full.
    let mut state = claims_state("");
    let path = std::path::PathBuf::from(unique_temp_path("full-policy")).with_extension("toml");
    // The policy the claims server has, in the file the settings are written into.
    std::fs::write(&path, "[grants]\nallow_plain_http = true\nallow_private_addresses = true\n").unwrap();
    state.policy_path = path.display().to_string().into();
    let disk = unique_temp_path("full-disk");
    std::fs::create_dir_all(&disk).unwrap();
    state.deployments_path = std::path::Path::new(&disk).join("deployments.json").display().to_string().into();
    let set = |body: &str| route(&state, &req("POST", "/admin/storage", body.as_bytes().to_vec()));
    let recorded = deploy_text_to(&state, "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", { port: 8301 })\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n", &[]);
    let quiet = deploy_text_to(&state, "import { flow, route } from \"holon\"\n\nflow(\"health\", { port: 8302 })\n\nroute.get(\"/health\", () => \"ok\", { log: \"unrecorded\" })\n", &[]);
    let status = |slug: &str| state.deployments.lock().unwrap()[slug].life.status;
    // Floors of free space no disk has: its disk is full.
    assert_eq!(set(r#"{"warn_at":"1000T","full_at":"999T","when_full":"pause"}"#).status_code, 200);
    state.sweeps.act(&state, 1);
    assert_eq!(status(&recorded), crate::lifecycle::Status::Paused, "it records runs");
    assert_eq!(status(&quiet), crate::lifecycle::Status::Running, "its runs are all unrecorded");
    let host = body_json(route(&state, &req("GET", "/admin/host", vec![])));
    assert_eq!(host["acting"]["paused"], serde_json::json!([recorded.clone()]), "{host}");
    assert!(host["warnings"][0]["todo"].as_str().unwrap().contains("paused until it is back under"), "{host}");
    let events = |slug: &str| body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
    assert!(events(&recorded).contains("paused by the host: the disk was full"), "{}", events(&recorded));
    // A person resumes it while it is still full: the next look pauses it again.
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{recorded}/run"), b"{}".to_vec())).status_code, 200);
    state.sweeps.act(&state, 2);
    assert_eq!(status(&recorded), crate::lifecycle::Status::Paused);
    // Room again (floors no disk is under): it resumes, and says so.
    assert_eq!(set(r#"{"full_at":"512"}"#).status_code, 400, "a size says its unit");
    assert_eq!(set(r#"{"warn_at":"2K","full_at":"1K"}"#).status_code, 200);
    state.sweeps.act(&state, 3);
    assert_eq!(status(&recorded), crate::lifecycle::Status::Running);
    assert!(events(&recorded).contains("resumed by the host: there is room on its disk again"));
    assert_eq!(body_json(route(&state, &req("GET", "/admin/host", vec![])))["acting"]["paused"], serde_json::json!([]));
    // Paused by a person, it isn't the host's to resume.
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{recorded}/pause"), b"{}".to_vec())).status_code, 200);
    state.sweeps.act(&state, 4);
    assert_eq!(status(&recorded), crate::lifecycle::Status::Paused);
    // A full_at in another unit than warn_at, reached first (a floor of free space no disk has,
    // and a percentage no disk is past): paused, warned of, and not resumed at the next look.
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{recorded}/run"), b"{}".to_vec())).status_code, 200);
    assert_eq!(set(r#"{"warn_at":"99%","full_at":"1000T"}"#).status_code, 200);
    let pauses = || events(&recorded).matches("paused by the host").count();
    let before = pauses();
    for look in 5..8 {
        state.sweeps.act(&state, look);
        assert_eq!(status(&recorded), crate::lifecycle::Status::Paused, "look {look}");
    }
    assert_eq!(pauses(), before + 1, "paused once, not paused and resumed at each look: {}", events(&recorded));
    let w = body_json(route(&state, &req("GET", "/admin/warnings", vec![])))["warnings"].clone();
    assert!(w.as_array().unwrap().iter().any(|w| w["key"].as_str().unwrap().starts_with("disk:") && w["todo"].as_str().unwrap().contains("paused until")), "warned of: {w}");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&disk);
}

/// A pack of the store's values that can't be read is warned of, and the server opens its store
/// all the same.
#[test]
fn a_pack_that_cant_be_read_is_warned_of_and_the_store_opens() {
    let dir = unique_temp_path("pack-unreadable");
    {
        let disk = FileStore::new(&dir).unwrap();
        disk.packs().pack(&[([7u8; 32], b"a value".to_vec())]).unwrap();
    }
    let packs = std::path::Path::new(&dir).join("values").join("packs");
    let pack = std::fs::read_dir(&packs).unwrap().flatten().next().unwrap().path();
    std::fs::write(&pack, b"not a pack").unwrap();
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    let w = body_json(route(&state, &req("GET", "/admin/warnings", vec![])))["warnings"].clone();
    assert!(w.as_array().unwrap().iter().any(|w| w["key"] == format!("pack:{}", pack.display()) && w["todo"].as_str().unwrap().contains("restore")), "{w}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The first cut of a full disk rolling sweeps the store with the log already cut back: what
/// only the runs it cut named goes (once an hour old), at once.
#[test]
fn a_full_disk_rolling_sweeps_what_only_the_cut_runs_named() {
    let dir = unique_temp_path("roll-sweep-store");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let mut state = test_state_with(SharedStore::on_disk(Box::new(disk), values.clone()));
    let data = unique_temp_path("roll-sweep-data");
    std::fs::create_dir_all(&data).unwrap();
    state.deployments_path = std::path::Path::new(&data).join("deployments.json").display().to_string().into();
    let path = std::path::PathBuf::from(unique_temp_path("roll-sweep-policy")).with_extension("toml");
    state.policy_path = path.display().to_string().into();
    let plan = holon_author::agent::plan("import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", { keep: { runs: 200 } })\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n").unwrap_or_else(|e| panic!("{e}"));
    let requirements = plan.requirements.clone().unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "kept", "unit": plan.unit, "requirements": requirements })).unwrap())));
    let ask = serde_json::json!({ "acknowledge": preview["needs_approval"], "requirements": requirements });
    assert_eq!(deploy_json(&state, "kept", parse_hash(&plan.unit).unwrap(), ask).status_code, 200);
    for _ in 0..120 {
        assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);
    }
    // Each run put the state it left: a value of its own, which its record names.
    let held = || std::fs::read_dir(&values).unwrap().flatten().filter(|e| e.file_name().len() == 64).count() + state.store.2.packs().map_or(0, |p| p.held().values);
    let before = held();
    assert!(before >= 120, "{before}");
    let set = |body: &str| route(&state, &req("POST", "/admin/storage", body.as_bytes().to_vec()));
    assert_eq!(set(r#"{"warn_at":"1000T","full_at":"999T","when_full":"roll"}"#).status_code, 200);
    // Two hours on: what was put is past what a sweep spares.
    state.sweeps.act(&state, crate::deploy::epoch_now() * 1000 + 2 * 3600 * 1000);
    assert_eq!(state.deployments.lock().unwrap()["kept"].log.with(|l| l.holds().3), 60, "cut back at once");
    let after = held();
    assert!(before - after >= 55, "the states only the 60 runs cut named went: {before} then {after}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_file(&path);
}

/// A log's size on disk is its file and its sealed segments: what roll picks the biggest by, and
/// what the host's view counts its logs by.
#[test]
fn a_full_disk_rolls_the_log_biggest_with_its_sealed_runs() {
    let mut state = claims_state("");
    let path = std::path::PathBuf::from(unique_temp_path("roll-sealed-policy")).with_extension("toml");
    std::fs::write(&path, "[grants]\nallow_plain_http = true\nallow_private_addresses = true\n").unwrap();
    state.policy_path = path.display().to_string().into();
    let disk = unique_temp_path("roll-sealed-disk");
    std::fs::create_dir_all(&disk).unwrap();
    state.deployments_path = std::path::Path::new(&disk).join("deployments.json").display().to_string().into();
    let many = deploy_text_to(&state, "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"many\", { port: 8304 })\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n", &[]);
    let wide = deploy_text_to(&state, "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"wide\", { port: 8305 })\n\nexport const keep = action((input) => {\n  state.s = input\n})\n", &[]);
    // Two lines in each file as written, the rest sealed.
    for slug in [&many, &wide] {
        state.deployments.lock().unwrap()[slug].log.with(|l| l.set_hot(2));
    }
    for _ in 0..600 {
        assert_eq!(route(&state, &req("POST", &format!("/trigger/{many}/bump"), b"{}".to_vec())).status_code, 200);
    }
    // Long lines that seal small: its file is the bigger, its segments the smaller, by margins a
    // run's timings don't decide (as precise as the platform's clock: more digits on Linux than on
    // Windows, so its runs seal bigger there).
    for i in 0..30 {
        let body = serde_json::json!({ "s": format!("{i}{}", "x".repeat(5_000)) });
        assert_eq!(route(&state, &req("POST", &format!("/trigger/{wide}/keep"), serde_json::to_vec(&body).unwrap())).status_code, 200);
    }
    crate::recorder::sync_logs(&state.deployments).unwrap();
    let logs = std::path::Path::new(&disk).join("deployments.logs");
    let file = |slug: &str| std::fs::metadata(logs.join(format!("{slug}.jsonl"))).unwrap().len();
    let sealed = |slug: &str| state.deployments.lock().unwrap()[slug].log.with(|l| l.sealed_holds().2);
    assert!(file(&wide) > file(&many), "wide's file is the bigger: {} and {}", file(&wide), file(&many));
    assert!(file(&many) + sealed(&many) > file(&wide) + sealed(&wide), "many holds the more: {} + {} and {} + {}", file(&many), sealed(&many), file(&wide), sealed(&wide));
    // The host's view counts the logs with their segments.
    let host = body_json(route(&state, &req("GET", "/admin/host", vec![])));
    let jsonl = std::fs::read_dir(&logs).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".jsonl")).count() as u64;
    assert_eq!(host["kept"]["logs"]["files"].as_u64(), Some(jsonl), "segments are no more logs: {host}");
    assert!(host["kept"]["logs"]["bytes"].as_u64().unwrap() >= file(&many) + sealed(&many) + file(&wide) + sealed(&wide), "{host}");
    let set = |body: &str| route(&state, &req("POST", "/admin/storage", body.as_bytes().to_vec()));
    assert_eq!(set(r#"{"warn_at":"1000T","full_at":"999T","when_full":"roll"}"#).status_code, 200);
    state.sweeps.act(&state, 1);
    let kept = |slug: &str| state.deployments.lock().unwrap()[slug].log.with(|l| l.holds().0);
    assert_eq!((kept(&many), kept(&wide)), (250, 30), "the one holding the most on disk is cut");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&disk);
}

/// Past `full_at`, `when_full = "roll"` cuts the log keeping the most to half (never below 20),
/// look after look, and keeps it as granted again once back under `warn_at`.
#[test]
fn a_full_disk_rolls_the_log_keeping_the_most() {
    let mut state = claims_state("");
    let path = std::path::PathBuf::from(unique_temp_path("roll-policy")).with_extension("toml");
    // The policy the claims server has, in the file the settings are written into.
    std::fs::write(&path, "[grants]\nallow_plain_http = true\nallow_private_addresses = true\n").unwrap();
    state.policy_path = path.display().to_string().into();
    let disk = unique_temp_path("roll-disk");
    std::fs::create_dir_all(&disk).unwrap();
    state.deployments_path = std::path::Path::new(&disk).join("deployments.json").display().to_string().into();
    let set = |body: &str| route(&state, &req("POST", "/admin/storage", body.as_bytes().to_vec()));
    let slug = deploy_text_to(&state, "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", { port: 8303 })\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n", &[]);
    for _ in 0..120 {
        assert_eq!(route(&state, &req("POST", &format!("/trigger/{slug}/bump"), b"{}".to_vec())).status_code, 200);
    }
    let kept = || state.deployments.lock().unwrap()[&slug].log.with(|l| l.holds().0);
    assert_eq!(kept(), 120);
    assert_eq!(set(r#"{"warn_at":"1000T","full_at":"999T","when_full":"roll"}"#).status_code, 200);
    state.sweeps.act(&state, 1);
    assert_eq!(kept(), 60, "halved");
    for i in 0..4 {
        state.sweeps.act(&state, 2 + i);
    }
    assert_eq!(kept(), storage::ROLL_FLOOR, "never below the floor");
    // The file is cut back by the periodic sync, as a full log's is.
    crate::recorder::sync_logs(&state.deployments).unwrap();
    let file = std::path::Path::new(&disk).join("deployments.logs").join(format!("{slug}.jsonl"));
    assert_eq!(std::fs::read_to_string(&file).unwrap().lines().count(), storage::ROLL_FLOOR, "its file cut back too");
    let events = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
    assert!(events.contains("the host cut its log from 120 runs to its newest 60"), "{events}");
    assert_eq!(body_json(route(&state, &req("GET", "/admin/host", vec![])))["acting"]["rolled"], serde_json::json!([slug.clone()]));
    // Room again: kept as granted, so it grows back as runs come.
    assert_eq!(set(r#"{"warn_at":"2K","full_at":"1K"}"#).status_code, 200);
    state.sweeps.act(&state, 10);
    for _ in 0..30 {
        route(&state, &req("POST", &format!("/trigger/{slug}/bump"), b"{}".to_vec()));
    }
    assert_eq!(kept(), 50, "the 20 it kept and 30 since");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&disk);
}

/// With `[storage] backup`, the runs a log lets go of are written there first, and made into
/// gzipped exports `holon verify` checks: with the runs still kept, every run there was, none
/// twice. A backup that can't be written to keeps them in the log, and the host warns.
#[test]
fn the_runs_a_log_lets_go_of_are_backed_up_as_exports_verify_checks() {
    let dir = unique_temp_path("backup-store");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let mut state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    let data = unique_temp_path("backup-data");
    std::fs::create_dir_all(&data).unwrap();
    state.deployments_path = std::path::Path::new(&data).join("deployments.json").display().to_string().into();
    let path = std::path::PathBuf::from(unique_temp_path("backup-policy")).with_extension("toml");
    state.policy_path = path.display().to_string().into();
    let backup = std::path::PathBuf::from(unique_temp_path("backup"));
    let set = |body: serde_json::Value| route(&state, &req("POST", "/admin/storage", serde_json::to_vec(&body).unwrap()));
    assert_eq!(set(serde_json::json!({ "backup": backup.display().to_string() })).status_code, 200);
    assert_eq!(set(serde_json::json!({ "backup": "s3://bucket" })).status_code, 400);
    let plan = holon_author::agent::plan("import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", { keep: { runs: 5 } })\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n").unwrap_or_else(|e| panic!("{e}"));
    let requirements = plan.requirements.clone().unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "kept", "unit": plan.unit, "requirements": requirements })).unwrap())));
    let ask = serde_json::json!({ "acknowledge": preview["needs_approval"], "requirements": requirements });
    assert_eq!(deploy_json(&state, "kept", parse_hash(&plan.unit).unwrap(), ask).status_code, 200);
    // The look gives the log the backup; then runs, and the periodic sync's cut-backs.
    state.sweeps.act(&state, 1);
    for i in 0..40 {
        assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);
        if i % 7 == 0 {
            crate::recorder::sync_logs(&state.deployments).unwrap();
        }
    }
    crate::recorder::sync_logs(&state.deployments).unwrap();
    // The folder holds other things too (a drive's, or a synced folder's): they are left alone.
    // Even one named as a batch was before the server's files had their prefix.
    let theirs = [backup.join("projects/data.jsonl"), backup.join("projects/empty.jsonl"), backup.join("kept/notes.jsonl"), backup.join("kept/photo.json.gz"), backup.join(format!("kept/{:020}-{:020}.jsonl", 1, 2))];
    for (i, p) in theirs.iter().enumerate() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, if i == 1 { "" } else { "{\"mine\":1}\n" }).unwrap();
    }
    // A batch that can't be made an export (not text): the others are made all the same, and it
    // stays, warned of.
    let bad = backup.join("kept").join(storage::batch_name(0, 0));
    std::fs::write(&bad, [0xff, 0xfe, b'\n']).unwrap();
    storage::archive(&state);
    assert!(bad.exists(), "the batch that can't be made an export stays");
    let batches_left = std::fs::read_dir(backup.join("kept")).unwrap().flatten().filter(|e| e.path() != bad && storage::is_batch(&e.file_name().to_string_lossy())).count();
    assert_eq!(batches_left, 0, "the other batches made exports all the same");
    let w = body_json(route(&state, &req("GET", "/admin/warnings", vec![])))["warnings"].clone();
    assert!(w.as_array().unwrap().iter().any(|w| w["key"] == "backup-export" && w["says"].as_str().unwrap().contains(&bad.display().to_string())), "{w}");
    std::fs::remove_file(&bad).unwrap();
    storage::archive(&state);
    let w = body_json(route(&state, &req("GET", "/admin/warnings", vec![])))["warnings"].clone();
    assert!(!w.as_array().unwrap().iter().any(|w| w["key"] == "backup-export"), "no longer warned of once it is gone: {w}");
    for p in &theirs {
        assert!(p.exists(), "{} left alone", p.display());
    }
    assert!(!backup.join("projects").read_dir().unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".json.gz")), "nothing exported of what isn't a batch");
    let exports: Vec<std::path::PathBuf> = std::fs::read_dir(backup.join("kept")).unwrap().flatten().map(|e| e.path()).filter(|p| !theirs.contains(p)).collect();
    assert!(!exports.is_empty() && exports.iter().all(|p| { let n = p.file_name().unwrap().to_string_lossy(); n.starts_with("holon-archive-kept-runs-") && n.ends_with(".json.gz") }), "{exports:?}");
    let mut seqs = Vec::new();
    for p in &exports {
        let export = verify::read_export(p).unwrap();
        assert_eq!(export.deployment, "kept");
        let report = verify::check(&export, None, &BTreeMap::new());
        assert!(report.ok(), "{}", verify::describe(&report, None));
        assert!(report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
        seqs.extend(export.lines.iter().map(|l| serde_json::from_str::<recorder::LogEntry>(l).unwrap().seq));
    }
    let kept: Vec<u64> = state.deployments.lock().unwrap()["kept"].log.after(None, usize::MAX).iter().map(|e| e.seq).collect();
    let file: Vec<u64> = std::fs::read_to_string(std::path::Path::new(&data).join("deployments.logs/kept.jsonl")).unwrap().lines().map(|l| serde_json::from_str::<recorder::LogEntry>(l).unwrap().seq).collect();
    seqs.extend(file.iter().copied());
    seqs.sort_unstable();
    assert_eq!(seqs, (0..40).collect::<Vec<u64>>(), "backed up and still in the file: every run, once");
    assert_eq!(kept.len(), 5);
    let host = body_json(route(&state, &req("GET", "/admin/host", vec![])));
    assert_eq!(host["backup"]["exports"].as_u64(), Some(exports.len() as u64), "{host}");
    // A backup that can't be written to (a file stands where its folder would be): no cut.
    let blocked = std::path::PathBuf::from(unique_temp_path("backup-blocked"));
    std::fs::write(&blocked, "not a folder").unwrap();
    assert_eq!(set(serde_json::json!({ "backup": blocked.display().to_string() })).status_code, 200);
    state.sweeps.act(&state, 2);
    let lines = || std::fs::read_to_string(std::path::Path::new(&data).join("deployments.logs/kept.jsonl")).unwrap().lines().count();
    for _ in 0..20 {
        route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec()));
        crate::recorder::sync_logs(&state.deployments).unwrap();
    }
    assert!(lines() > 20, "kept in the file: {} lines", lines());
    let w = body_json(route(&state, &req("GET", "/admin/warnings", vec![])))["warnings"].clone();
    assert!(w.as_array().unwrap().iter().any(|w| w["key"] == "backup" && w["todo"].as_str().unwrap().contains("kept in the logs")), "{w}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_file(&blocked);
    let _ = std::fs::remove_file(&path);
}

/// The server's own sweep packs the store's values that outlive it (named, older than it spares):
/// their files go, a pack holds them, and every kept state and collection reads back as before.
/// Values packed that nothing names later are swept from the pack.
#[test]
fn the_values_a_sweep_keeps_are_packed_and_read_back() {
    let dir = unique_temp_path("pack-store");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let state = test_state_with(SharedStore::on_disk(Box::new(disk), values.clone()));
    let plan = holon_author::agent::plan("import { action, flow } from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\n\nflow(\"kept\", { keep: { runs: 3 } })\n\nexport const bump = action(() => {\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  notes[`k${n}`] = n\n})\n").unwrap_or_else(|e| panic!("{e}"));
    let requirements = plan.requirements.clone().unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "kept", "unit": plan.unit, "requirements": requirements })).unwrap())));
    let ask = serde_json::json!({ "acknowledge": preview["needs_approval"], "requirements": requirements });
    assert_eq!(deploy_json(&state, "kept", parse_hash(&plan.unit).unwrap(), ask).status_code, 200);
    let bump = || assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);
    for _ in 0..10 {
        bump();
    }
    let loose = || std::fs::read_dir(&values).unwrap().flatten().filter(|e| e.file_name().len() == 64).count();
    let before = loose();
    let later = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 2 * sweep::SPARES_MS;
    let mut scheduled = sweep::Scheduled::default();
    let swept = scheduled.sweep(&state.store, &state.deployments, later).unwrap().unwrap();
    assert!(swept.values > 0 && swept.packed > 0, "{swept:?}");
    assert_eq!(swept.packed, swept.kept, "every value it keeps, packed");
    assert_eq!(loose(), 0, "no value left a file of its own (of {before})");
    let packs = state.store.2.packs().unwrap().held();
    assert_eq!((packs.packs, packs.values), (1, swept.packed));
    // Read back from the pack: each kept run's state, and the live collection, the value cache
    // let go of so every read is from disk.
    state.store.4.set_budget(0);
    let log = body_json(route(&state, &req("GET", "/deployments/kept/log", vec![])));
    for e in log["entries"].as_array().unwrap() {
        restore::load_snapshot(e["state_after"].as_str().unwrap(), &state.store).unwrap_or_else(|err| panic!("a kept run's state: {err}"));
    }
    let notes = body_json(route(&state, &req("GET", "/deployments/kept/collections/notes", vec![])));
    assert_eq!(notes["items"].as_array().unwrap().len(), 10, "{notes}");
    // More runs: the packed states of runs cut since are named no more, and are swept from it.
    for _ in 0..5 {
        bump();
    }
    let much_later = later + 2 * sweep::SPARES_MS;
    let again = sweep::sweep(&state.store, &state.deployments, much_later, sweep::SPARES_MS, false, false).unwrap().unwrap();
    assert!(again.values > 0, "{again:?}");
    let packed = state.store.2.packs().unwrap().held();
    assert!(packed.values < packs.values, "{packed:?} after {packs:?}");
    let notes = body_json(route(&state, &req("GET", "/deployments/kept/collections/notes", vec![])));
    assert_eq!(notes["items"].as_array().unwrap().len(), 15, "{notes}");
    assert_eq!(body_json(route(&state, &req("GET", "/admin/host", vec![])))["kept"]["store"], serde_json::Value::Null, "counted at the first look only");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The states runs sealed in their log's segments left stay named: a store sweep keeps them, and
/// each kept run's state reads back, sealed or not.
#[test]
fn the_states_sealed_runs_left_are_kept_by_a_sweep() {
    let dir = unique_temp_path("sealed-sweep");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let mut state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    let data = unique_temp_path("sealed-data");
    std::fs::create_dir_all(&data).unwrap();
    state.deployments_path = std::path::Path::new(&data).join("deployments.json").display().to_string().into();
    let plan = holon_author::agent::plan("import { action, flow } from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\n\nflow(\"kept\", { keep: { runs: 60 } })\n\nexport const bump = action(() => {\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  notes[`k${n}`] = n\n})\n").unwrap_or_else(|e| panic!("{e}"));
    let requirements = plan.requirements.clone().unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "kept", "unit": plan.unit, "requirements": requirements })).unwrap())));
    let ask = serde_json::json!({ "acknowledge": preview["needs_approval"], "requirements": requirements });
    assert_eq!(deploy_json(&state, "kept", parse_hash(&plan.unit).unwrap(), ask).status_code, 200);
    state.deployments.lock().unwrap()["kept"].log.with(|l| l.set_hot(5));
    for i in 0..80 {
        assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);
        if i % 4 == 0 {
            crate::recorder::sync_logs(&state.deployments).unwrap();
        }
    }
    crate::recorder::sync_logs(&state.deployments).unwrap();
    let (segments, sealed, _) = state.deployments.lock().unwrap()["kept"].log.with(|l| l.sealed_holds());
    assert!(segments > 0 && sealed > 30, "{segments} segments, {sealed} runs sealed");
    let swept = sweep::sweep(&state.store, &state.deployments, SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 2 * sweep::SPARES_MS, sweep::SPARES_MS, false, false).unwrap().unwrap();
    assert!(swept.values > 0, "the cut runs' states went: {swept:?}");
    state.store.4.set_budget(0);
    let kept = state.deployments.lock().unwrap()["kept"].log.after(None, usize::MAX);
    assert_eq!(kept.len(), 60);
    for e in &kept {
        let snapshot = e.state_after.as_deref().unwrap_or_else(|| panic!("run {} left a state", e.seq));
        restore::load_snapshot(snapshot, &state.store).unwrap_or_else(|err| panic!("run {}'s state: {err}", e.seq));
    }
    let host = body_json(route(&state, &req("GET", "/deployments/kept/holds", vec![])));
    assert!(host["log"]["sealed"]["segments"].as_u64().unwrap() > 0, "{host}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&data);
}

/// An admin sets how often the server sweeps (`POST /admin/storage`): written into `holon.toml`'s
/// `[storage]`, the rest of the file as it was, comments too, and in force at once; a setting that
/// isn't one changes nothing.
#[test]
fn an_admin_sets_how_often_the_server_sweeps_in_its_policy_file() {
    let mut state = test_state();
    let path = std::path::PathBuf::from(unique_temp_path("policy")).with_extension("toml");
    let before = "# the host's policy\n[limits]\nrequest_timeout_ms = 20000 # long calls\n";
    std::fs::write(&path, before).unwrap();
    state.policy_path = path.display().to_string().into();
    let set = |body: &str| route(&state, &req("POST", "/admin/storage", body.as_bytes().to_vec()));
    let answer = set(r#"{"sweep_every":"30m"}"#);
    assert_eq!(answer.status_code, 200);
    assert_eq!(body_json(answer)["sweeps"]["every"], "30m");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.starts_with(before), "the rest kept as it was: {written}");
    assert!(written.contains("[storage]\nsweep_every = \"30m\""), "{written}");
    assert_eq!(state.policy().storage.sweep_every, Some(Duration::from_secs(1800)));
    assert_eq!(state.policy().request.request_timeout, Duration::from_millis(20000), "the rest still in force");
    assert_eq!(body_json(route(&state, &req("GET", "/permissions", vec![])))["storage"]["sweep_every"], "30m");
    // Set again: the key changed in place.
    assert_eq!(set(r#"{"sweep_every":"off"}"#).status_code, 200);
    assert_eq!(std::fs::read_to_string(&path).unwrap().matches("sweep_every").count(), 1);
    assert_eq!(state.policy().storage.sweep_every, None);
    for wrong in [r#"{"sweep_every":"5s"}"#, r#"{"sweep_every":"soon"}"#, "{}"] {
        let answer = set(wrong);
        assert_eq!(answer.status_code, 400, "{wrong}");
        assert_eq!(state.policy().storage.sweep_every, None, "{wrong} changed nothing");
    }
    assert!(std::fs::read_to_string(&path).unwrap().contains("sweep_every = \"off\""));
    let _ = std::fs::remove_file(&path);
}

/// `GET /permissions` answers the policy file's text as it was read, comments and all, for a
/// person to read whole (web-console-L4.md §9.3): the file as in force, not as it is on disk since;
/// read again, the new text. Without a file, none: the defaults are in force.
#[test]
fn the_policy_answers_its_file_as_read() {
    let mut state = test_state();
    let text_of = |state: &AppState| body_json(route(state, &req("GET", "/permissions", vec![])))["text"].clone();
    assert_eq!(text_of(&state), serde_json::Value::Null, "no file: the defaults");
    let path = std::path::PathBuf::from(unique_temp_path("policy-text")).with_extension("toml");
    let first = "# the host's policy\n[limits]\nmax_requests = 40 # a quiet host\n";
    std::fs::write(&path, first).unwrap();
    state.policy_path = path.display().to_string().into();
    let text = || text_of(&state);
    assert_eq!(route(&state, &req("POST", "/permissions/reload", vec![])).status_code, 200);
    assert_eq!(text(), first);
    // Edited on disk, it is the text in force until read again.
    let second = "[limits]\nmax_requests = 30\n";
    std::fs::write(&path, second).unwrap();
    assert_eq!(text(), first, "not read again yet");
    let reloaded = body_json(route(&state, &req("POST", "/permissions/reload", vec![])));
    assert_eq!(reloaded["text"], second, "the reload answers it too");
    assert_eq!(text(), second);
    // A file that doesn't parse changes nothing, its text neither.
    std::fs::write(&path, "[limits]\nnot_a_limit = 1\n").unwrap();
    assert_eq!(route(&state, &req("POST", "/permissions/reload", vec![])).status_code, 400);
    assert_eq!(text(), second);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_in_memory_store_has_nothing_to_sweep() {
    let state = test_state();
    let answer = route(&state, &req("POST", "/admin/store/sweep", b"{}".to_vec()));
    assert_eq!(answer.status_code, 409);
}

#[test]
fn a_deployment_keeps_as_many_runs_as_its_retention_says() {
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[limits]\nkeep_runs = 5\n[ceilings]\nkeep_runs = 50").unwrap());
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!(["Action(\"put\")", "StateWrite(\"k\")"]);
    let keeping = |n: u64| serde_json::json!({ "acknowledge": ack, "requirements": { "limits": { "keep_runs": n } } });
    let seqs = || -> Vec<u64> {
        let log = body_json(route(&state, &req("GET", "/deployments/w/log", vec![])));
        log["entries"].as_array().unwrap().iter().map(|e| e["seq"].as_u64().unwrap()).collect()
    };
    let run = |n: usize| {
        for _ in 0..n {
            assert_eq!(put(&state, "w", "x").status_code, 200);
        }
    };

    assert_eq!(deploy_json(&state, "w", writer, keeping(2)).status_code, 200, "keeping fewer asks nobody");
    run(4);
    assert_eq!(seqs(), [2, 3]);

    let more = deploy_json(&state, "w", writer, keeping(20));
    assert!(body_json(more)["error"].as_str().unwrap().contains("Limit(keep_runs=20)"), "keeping more than the default asks");
    let mut approved = keeping(20);
    approved["acknowledge"].as_array_mut().unwrap().push(serde_json::json!("Limit(keep_runs=20)"));
    assert_eq!(deploy_json(&state, "w", writer, approved).status_code, 200);
    run(8);
    assert_eq!(seqs(), (2..12).collect::<Vec<u64>>(), "the two it kept, and eight more");

    let past = deploy_json(&state, "w", writer, keeping(100));
    assert!(body_json(past)["error"].as_str().unwrap().contains("ceiling of 50"));

    // The host lowers it: the log keeps that many from now on.
    let lower = serde_json::to_vec(&serde_json::json!({ "lower": { "keep_runs": 3 } })).unwrap();
    assert_eq!(route(&state, &req("POST", "/deployments/w/grant", lower)).status_code, 200);
    assert_eq!(seqs(), [9, 10, 11]);
    let missing = route(&state, &req("POST", "/deployments/w/restore", serde_json::to_vec(&serde_json::json!({ "run": 1, "state": true, "dry_run": true })).unwrap()));
    let message = body_json(missing)["error"].as_str().unwrap_or_default().to_string();
    assert!(message.contains("it keeps the newest 3 runs"), "{message}");
}

/// A flow's settings change in place where they narrow it (web-console-L4.md §8.4): a limit
/// lowered, fewer runs kept, at once and noted on its grant. One raised widens what it may do, so
/// it is refused, saying it takes a new version, and nothing in the same change applies.
#[test]
fn settings_lower_in_place_and_a_raise_takes_a_new_version() {
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[limits]\nkeep_runs = 5\n[ceilings]\nkeep_runs = 50").unwrap());
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, ack).status_code, 200);
    for _ in 0..5 {
        assert_eq!(put(&state, "w", "x").status_code, 200);
    }
    let settings = |body: serde_json::Value| route(&state, &req("POST", "/deployments/w/settings", serde_json::to_vec(&body).unwrap()));
    let grant = || body_json(route(&state, &req("GET", "/deployments/w/grant", vec![])))["grant"].clone();
    let kept = || log_entries(&state, "w").len();

    // Lowered, with agents kept from proposing, in one change: the log keeps that many at once.
    let lowered = settings(serde_json::json!({ "lower": { "keep_runs": 2, "run_budget_ms": 1000 }, "agents_propose": false }));
    assert_eq!(lowered.status_code, 200);
    let entry = body_json(lowered);
    assert_eq!(entry["agents_propose"], false, "{entry}");
    assert_eq!(grant()["lowered"], serde_json::json!({ "keep_runs": 2, "run_budget_ms": 1000 }));
    assert_eq!(kept(), 2);
    let changes: Vec<String> = grant()["changes"].as_array().unwrap().iter().map(|c| c["change"].as_str().unwrap().to_string()).collect();
    assert_eq!(changes, ["lowered keep_runs to 2", "lowered run_budget_ms to 1000"]);

    // Raised: refused whole, the agents' setting with it.
    let raised = settings(serde_json::json!({ "lower": { "keep_runs": 4 }, "agents_propose": true }));
    assert_eq!(raised.status_code, 400);
    let said = body_json(raised)["error"].as_str().unwrap_or_default().to_string();
    assert!(said.contains("keep_runs is 2") && said.contains("a new version, which a person approves"), "{said}");
    assert_eq!(grant()["lowered"]["keep_runs"], 2);
    assert!(state.deployments.lock().unwrap()["w"].agents_propose_off, "nothing in the change applied");

    // What isn't a setting, or is nothing, is refused.
    for bad in [serde_json::json!({}), serde_json::json!({ "lower": { "no_such_limit": 1 } }), serde_json::json!({ "raise": { "keep_runs": 9 } })] {
        assert_eq!(settings(bad.clone()).status_code, 400, "{bad}");
    }
    assert_eq!(route(&state, &req("POST", "/deployments/none/settings", serde_json::to_vec(&serde_json::json!({ "agents_propose": true })).unwrap())).status_code, 404);
}

/// With the exceptions granted (a host whose policy allows them, a deployment that asks), an
/// outbound request reaches a server on this machine over plain HTTP and its answer flows on.
#[test]
fn granted_exceptions_let_a_request_reach_a_local_server() {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(1) {
            let mut stream = stream.unwrap();
            let _ = read_request(&mut stream);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        }
    });
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true").unwrap());
    let unit = stage_text(&state, &caller_text(&host));
    let ack = serde_json::json!(["Action(\"call\")", format!("HttpRequest({host:?})"), format!("PlainHttp({host:?})"), "PrivateAddresses"]);
    let requirements = serde_json::json!({ "plain_http": [host], "private_addresses": true });
    let deployed = deploy_json(&state, "local", unit, serde_json::json!({ "acknowledge": ack, "requirements": requirements }));
    assert_eq!(deployed.status_code, 200);
    let answered = body_json(run_action_named(&state, "local", "call", serde_json::json!("/hi")));
    assert_eq!(answered["answer"]["body"], "hello", "the answer's body is the action's: {answered}");

    // The run checks offline: its request is answered from its own record, not sent again.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/local/export", vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert_eq!(report.reruns, vec![(0, verify::Rerun::Matched)]);
}

/// A version replayed on a deployment's recent runs, offline (G11): the same version answers every
/// run as it ran; one that prices large orders higher differs on those; one that asks a host the
/// runs never asked can't be told; one without the action differs on every run. Nothing changes.
#[test]
fn a_version_replays_on_its_deployments_recent_runs() {
    let state = test_state();
    let quote = |body: &str| {
        format!("import {{ action, flow }} from \"holon\"
import state from \"state\"

flow(\"quote\")

export const price = action((n) => {{
  state.last = n
  {body}
}})
")
    };
    let v1 = stage_text(&state, &quote("return n * 2"));
    let ack = serde_json::json!({ "acknowledge": ["Action(\"price\")", "StateWrite(\"last\")"] });
    assert_eq!(deploy_json(&state, "quote", v1, ack).status_code, 200);
    for n in [1, 5, 20] {
        assert_eq!(run_action_named(&state, "quote", "price", serde_json::json!(n)).status_code, 200);
    }
    let replay = |unit: Hash, more: &str| route(&state, &req("GET", &format!("/deployments/quote/replay?unit={}{more}", to_hex(&unit)), vec![]));
    let counts = |r: &serde_json::Value| (r["replayed"].as_u64().unwrap(), r["same"].as_u64().unwrap(), r["differ"].as_u64().unwrap(), r["cant_tell"].as_u64().unwrap());

    let same = body_json(replay(v1, ""));
    assert_eq!(counts(&same), (3, 3, 0, 0), "{same}");
    assert_eq!(same["runs"].as_array().unwrap().iter().map(|r| r["seq"].as_u64().unwrap()).collect::<Vec<_>>(), vec![2, 1, 0], "newest first");

    let v2 = stage_text(&state, &quote("return n > 10 ? n * 3 : n * 2"));
    let r = body_json(replay(v2, ""));
    assert_eq!(counts(&r), (3, 2, 1, 0), "{r}");
    assert_eq!((&r["runs"][0]["outcome"], &r["runs"][0]["why"]), (&serde_json::json!("differs"), &serde_json::json!(["its outputs"])), "{r}");
    assert_eq!(counts(&body_json(replay(v2, "&runs=2"))), (2, 1, 1, 0), "the last two");

    let asks = "import { action, flow } from \"holon\"
import state from \"state\"
import api from \"host:api.example.com\"

flow(\"quote\")

export const price = action(async (n) => {
  state.last = n
  const r = await api.get(\"/rate\")
  return r.status
})
";
    let r = body_json(replay(stage_text(&state, asks), ""));
    assert_eq!(counts(&r), (3, 0, 0, 3), "{r}");
    let why = r["runs"][0]["why"][0].as_str().unwrap();
    assert!(why.starts_with("it sends GET https://api.example.com/rate, which the run didn't"), "{why}");

    let renamed = "import { action, flow } from \"holon\"

flow(\"quote\")

export const quote = action((n) => n * 2)
";
    let r = body_json(replay(stage_text(&state, renamed), ""));
    assert_eq!(counts(&r), (3, 0, 3, 0), "{r}");
    assert_eq!(r["runs"][0]["why"], serde_json::json!(["the version has no action \"price\""]));

    // It changed nothing: the deployment's log and state are as the runs left them.
    assert_eq!(log_entries(&state, "quote").len(), 3);
    assert_eq!(body_json(route(&state, &req("GET", "/deployments/quote/state", vec![])))["state"]["last"]["value"], serde_json::json!(20.0));
    assert_eq!(replay(v1, "&runs=0").status_code, 400);
    assert_eq!(replay([7u8; 32], "").status_code, 404, "a unit not staged");
    assert_eq!(route(&state, &req("GET", &format!("/deployments/nope/replay?unit={}", to_hex(&v1)), vec![])).status_code, 404);
}

/// A run of every kind is run again through the handler its kind and label name
/// (`verify::handler_of`), by `holon verify` and a replay alike. Each kind the host logs a run as
/// (every `run_entry` and `run_entry_taking` in its source) is a kind of handler's, and every
/// handler of every sample is found by its run's kind and label, and by a resumed part's: given
/// what its run was sent (a request, a WebSocket block's record, an action's input) or finding
/// its start among what the run sourced (a tick, a file, a mail, a feed's event, a queue's
/// message). A server's handlers are taken field by field, so a new kind of handler fails to
/// compile here until it has its row.
#[test]
fn a_run_of_every_kind_finds_the_handler_it_ran_by_its_kind_and_label() {
    let kinds: BTreeSet<&str> = ["request", "websocket", "tick", "file", "mail", "feed", "queue", "trigger", "resume"].into();
    fn sources(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut texts = Vec::new();
    sources(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut texts);
    let logged: BTreeSet<String> = texts
        .iter()
        .flat_map(|t| ["run_entry(\"", "run_entry_taking(\""].into_iter().flat_map(move |call| t.split(call).skip(1)))
        .filter_map(|rest| rest.split_once('"').map(|(kind, _)| kind.to_string()))
        .collect();
    assert_eq!(logged.iter().map(String::as_str).collect::<BTreeSet<_>>(), kinds, "the kinds the host logs a run as");

    let state = test_state();
    let mut found = BTreeSet::new();
    for sample in holon_author::samples::samples() {
        let unit = stage_text(&state, sample.text);
        let resolver = closure(&state.store, unit).unwrap();
        let Ok(server) = read_server(resolver.resolve(&unit).unwrap(), &resolver) else { continue };
        let Server { port: _, taint: _, tags: _, routes, clocks, files, mails, feeds, queues, websockets, actions } = &server;
        let label = |l: &String| l.clone();
        let handlers: Vec<(&str, String, Hash, bool)> = [
            routes.iter().map(|r| ("request", format!("{} {}", r.method, r.path), r.endpoint, true)).collect::<Vec<_>>(),
            websockets.iter().flat_map(|w| w.blocks()).map(|b| ("websocket", label(&b.label), b.handler, true)).collect(),
            clocks.iter().map(|c| ("tick", label(&c.label), c.handler, false)).collect(),
            files.iter().map(|f| ("file", label(&f.label), f.handler, false)).collect(),
            mails.iter().map(|m| ("mail", label(&m.label), m.handler, false)).collect(),
            feeds.iter().map(|f| ("feed", label(&f.label), f.handler, false)).collect(),
            queues.iter().map(|q| ("queue", label(&q.label), q.handler, false)).collect(),
            actions.iter().map(|a| ("trigger", label(&a.label), a.handler, true)).collect(),
        ]
        .concat();
        assert_eq!(handlers.len(), server.handlers().count(), "{}: every handler", sample.name);
        // A resumed run's later part is its handler's, whichever started it.
        for (kind, label, handler, sent) in handlers {
            for kind in [kind, "resume"] {
                assert_eq!(verify::handler_of(&server, kind, &label), Ok((handler, sent)), "{}: a {kind} run of {label:?}", sample.name);
                found.insert(kind);
            }
        }
    }
    assert_eq!(found, kinds, "the samples have a handler of every kind");
}

/// A replayed version's outbound requests are answered by what the run was answered to the same
/// request, wherever the version asks it from; one the run never sent can't be told.
#[test]
fn a_replay_answers_a_request_the_run_sent_and_cant_tell_one_it_didnt() {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(1) {
            let mut stream = stream.unwrap();
            let _ = read_request(&mut stream);
            let _ = stream.write_all(b"HTTP/1.1 200 OK
Content-Length: 5
Connection: close

hello");
        }
    });
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]
allow_plain_http = true
allow_private_addresses = true").unwrap());
    let unit = stage_text(&state, &caller_text(&host));
    let ack = serde_json::json!(["Action(\"call\")", format!("HttpRequest({host:?})"), format!("PlainHttp({host:?})"), "PrivateAddresses"]);
    let requirements = serde_json::json!({ "plain_http": [host], "private_addresses": true });
    assert_eq!(deploy_json(&state, "local", unit, serde_json::json!({ "acknowledge": ack, "requirements": requirements })).status_code, 200);
    assert_eq!(body_json(run_action_named(&state, "local", "call", serde_json::json!("/hi")))["answer"]["body"], "hello");

    let version = |body: &str| {
        let text = format!("import {{ action, flow }} from \"holon\"
import api from \"host:{host}\"

flow(\"caller\")

export const call = action(async (path) => {{
{body}
}})
");
        let unit = stage_text(&state, &text);
        body_json(route(&state, &req("GET", &format!("/deployments/local/replay?unit={}", to_hex(&unit)), vec![])))
    };
    // The body alone, from the answer the run had: it differs, and is told.
    let r = version("  const r = await api.get(path)
  return r.body");
    assert_eq!((&r["differ"], &r["runs"][0]["why"]), (&serde_json::json!(1), &serde_json::json!(["its outputs"])), "{r}");
    let r = version("  const r = await api.get(path)
  return r");
    assert_eq!(r["same"], 1, "{r}");
    // A second request the run never sent.
    let r = version("  const r = await api.get(path)
  const s = await api.get(\"/more\")
  return [r, s]");
    assert_eq!(r["cant_tell"], 1, "{r}");
    assert!(r["runs"][0]["why"][0].as_str().unwrap().starts_with(&format!("it sends GET https://{host}/more")), "{r}");
}

/// The grant is the boundary at run time: an effect it doesn't cover isn't performed, whatever
/// the graph says. (A grant can only lose an item by being changed under a running deployment;
/// this sets that up directly.)
#[test]
fn an_effect_outside_the_grant_is_refused_at_perform_time() {
    let state = test_state();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, ack).status_code, 200);
    {
        let mut table = state.deployments.lock().unwrap();
        let dep = table.get_mut("w").unwrap();
        let mut narrowed = dep.grant.clone();
        narrowed.items.remove("StateWrite(\"k\")");
        dep.permit = Arc::new(Permit::new(dep.unit, &narrowed, &state.policy()));
    }
    assert_eq!(put(&state, "w", "value").status_code, 200);
    let entry = &log_entries(&state, "w")[0];
    assert_eq!(entry["performed"].as_array().map_or(0, Vec::len), 0, "the write was refused: {entry}");
    let current = body_json(route(&state, &req("GET", "/deployments/w/state", vec![])));
    assert!(current["state"].get("k").is_none(), "and nothing was stored: {current}");
}

fn change_grant(state: &AppState, slug: &str, change: serde_json::Value) -> Response {
    route(state, &req("POST", &format!("/deployments/{slug}/grant"), serde_json::to_vec(&change).unwrap()))
}

/// The host takes an item back without a redeploy: the deployment pauses, flagged, until the item
/// is granted back, and then resumes on its own. Every change is recorded on the grant, and a
/// change that can't apply (an item it doesn't hold, or never asked for) changes nothing.
#[test]
fn revoking_an_item_pauses_until_it_is_granted_back() {
    let state = test_state();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, ack).status_code, 200);

    let revoked = change_grant(&state, "w", serde_json::json!({ "revoke": ["StateWrite(\"k\")"] }));
    assert_eq!(revoked.status_code, 200);
    let view = body_json(revoked);
    assert_eq!(view["status"], "paused");
    assert!(view["attention"].as_str().unwrap().contains("StateWrite(\"k\")"), "{view}");
    assert_eq!(view["grant"]["revoked"], serde_json::json!(["StateWrite(\"k\")"]));
    assert_eq!(put(&state, "w", "v").status_code, 409, "paused");
    assert_eq!(route(&state, &req("POST", "/deployments/w/run", vec![])).status_code, 409, "can't be run past it");

    // What can't apply is refused whole.
    for bad in [
        serde_json::json!({ "revoke": ["StateWrite(\"k\")"] }),
        serde_json::json!({ "grant": ["StateWrite(\"other\")"] }),
        serde_json::json!({ "grant": ["StateWrite(\"k\")"], "revoke": ["Clock"] }),
        serde_json::json!({}),
        serde_json::json!({ "revoke_all": true }),
    ] {
        assert_eq!(change_grant(&state, "w", bad.clone()).status_code, 400, "{bad}");
    }
    assert_eq!(state.deployments.lock().unwrap()["w"].life.status, Status::Paused);

    let restored = body_json(change_grant(&state, "w", serde_json::json!({ "grant": ["StateWrite(\"k\")"] })));
    assert_eq!(restored["status"], "running", "covered again: it resumes");
    assert!(restored["attention"].is_null());
    let changes: Vec<&str> = restored["grant"]["changes"].as_array().unwrap().iter().map(|c| c["change"].as_str().unwrap()).collect();
    assert_eq!(changes, ["revoked StateWrite(\"k\")", "granted StateWrite(\"k\")"]);
    assert_eq!(put(&state, "w", "v").status_code, 200);
    assert!(body_json(route(&state, &req("GET", "/deployments/w/state", vec![])))["state"].get("k").is_some());

    // A pause the person chose isn't the grant's to undo.
    change_grant(&state, "w", serde_json::json!({ "revoke": ["StateWrite(\"k\")"] }));
    assert_eq!(route(&state, &req("POST", "/deployments/w/stop", vec![])).status_code, 200);
    let regranted = body_json(change_grant(&state, "w", serde_json::json!({ "grant": ["StateWrite(\"k\")"] })));
    assert_eq!(regranted["status"], "stopped");
}

/// Kept running instead: the deployment answers, and only what was revoked fails (an outbound
/// call to a revoked host says `revoked`, not `blocked_host`).
#[test]
fn a_revocation_can_keep_the_deployment_running_with_the_revoked_calls_failing() {
    let state = test_state();
    let host = "api.example.com";
    let unit = stage_text(&state, &caller_text(host));
    let ack = serde_json::json!(["Action(\"call\")", format!("HttpRequest({host:?})")]);
    assert_eq!(deploy_json(&state, "caller", unit, serde_json::json!({ "acknowledge": ack })).status_code, 200);

    let change = serde_json::json!({ "revoke": [format!("HttpRequest({host:?})")], "keep_running": true });
    let view = body_json(change_grant(&state, "caller", change));
    assert_eq!(view["status"], "running");
    assert!(view["attention"].is_null());
    assert_eq!(view["grant"]["keep_running"], true);

    assert_eq!(run_action_named(&state, "caller", "call", serde_json::json!("/status")).status_code, 200);
    let log = body_json(route(&state, &req("GET", "/deployments/caller/log", vec![])));
    let exchange = &log["entries"][0]["rounds"][0]["exchanges"][0];
    let error = exchange["response"]["value"].as_array().unwrap().iter().find(|kv| kv[0] == "error").unwrap()[1]["value"].clone();
    assert_eq!(error, "revoked");
    assert_eq!(log["entries"][0]["grant"], view["hash"], "the log names the grant in effect");

    // Switched back to pausing, the uncovered grant pauses it.
    let view = body_json(change_grant(&state, "caller", serde_json::json!({ "keep_running": false })));
    assert_eq!(view["status"], "paused");
}

/// Lowering a limit applies to the next run, and needs nobody's approval; raising one takes a
/// deploy. Lowering a limit the flow asked for below what it asked takes that item back.
#[test]
fn a_limit_can_be_lowered_without_a_redeploy_but_not_raised() {
    let state = test_state();
    let writer = stage_writer(&state, "k");
    let requirements = serde_json::json!({ "limits": { "request_timeout_ms": 60000 } });
    let ack = serde_json::json!(["Action(\"put\")", "StateWrite(\"k\")", "Limit(request_timeout_ms=60000)"]);
    let deployed = deploy_json(&state, "w", writer, serde_json::json!({ "acknowledge": ack, "requirements": requirements }));
    assert_eq!(deployed.status_code, 200);
    let limits = |state: &AppState| state.deployments.lock().unwrap()["w"].permit.request_limits.clone();
    assert_eq!(limits(&state).request_timeout, Duration::from_secs(60));

    // A limit it didn't ask about: lowered, and it keeps running.
    let before = body_json(route(&state, &req("GET", "/deployments/w/grant", vec![])))["hash"].clone();
    let view = body_json(change_grant(&state, "w", serde_json::json!({ "lower": { "max_requests": 5 } })));
    assert_eq!(view["status"], "running");
    assert_eq!(limits(&state).max_requests, 5);
    assert_ne!(view["hash"], before, "a lowered limit changes the grant's hash");
    assert_eq!(change_grant(&state, "w", serde_json::json!({ "lower": { "max_requests": 6 } })).status_code, 400, "raising");
    assert_eq!(change_grant(&state, "w", serde_json::json!({ "lower": { "max_request": 1 } })).status_code, 400, "unknown");

    // Below what it asked for: that item is no longer granted, so it pauses.
    let view = body_json(change_grant(&state, "w", serde_json::json!({ "lower": { "request_timeout_ms": 20000 } })));
    assert_eq!(view["status"], "paused");
    assert_eq!(limits(&state).request_timeout, Duration::from_secs(20));
    let view = body_json(change_grant(&state, "w", serde_json::json!({ "grant": ["Limit(request_timeout_ms=60000)"] })));
    assert_eq!(view["status"], "running");
    assert_eq!(limits(&state).request_timeout, Duration::from_secs(60));
    assert_eq!(limits(&state).max_requests, 5, "the other lowered limit stays");

    // A redeploy doesn't undo what the host lowered.
    let redeployed = deploy_json(&state, "w", writer, serde_json::json!({ "acknowledge": ack, "requirements": requirements }));
    assert_eq!(redeployed.status_code, 200);
    assert_eq!(limits(&state).max_requests, 5, "still lowered after a redeploy");
    assert_eq!(limits(&state).request_timeout, Duration::from_secs(60), "what the deploy asked anew is granted");
}

/// The policy file read again holds every deployment to it at once: a ceiling lowered below what a
/// deployment was granted pauses it, and raising it back resumes it. A file that doesn't parse
/// changes nothing.
#[test]
fn reloading_the_policy_holds_every_deployment_to_it() {
    let mut state = test_state();
    let path = unique_temp_path("policy");
    state.policy_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let requirements = serde_json::json!({ "limits": { "request_timeout_ms": 60000 } });
    let ack = serde_json::json!(["Action(\"put\")", "StateWrite(\"k\")", "Limit(request_timeout_ms=60000)"]);
    assert_eq!(deploy_json(&state, "w", writer, serde_json::json!({ "acknowledge": ack, "requirements": requirements })).status_code, 200);
    let reload = |state: &AppState| route(state, &req("POST", "/permissions/reload", b"{}".to_vec()));

    std::fs::write(&path, "[ceilings]\nrequest_timeout_ms = 45000\n").unwrap();
    let reloaded = reload(&state);
    assert_eq!(reloaded.status_code, 200);
    let reply = body_json(reloaded);
    assert_eq!(reply["ceilings"]["request_timeout_ms"], 45000);
    assert!(reply["attention"]["w"].as_str().unwrap().contains("no longer grants"), "{reply}");
    assert_eq!(state.deployments.lock().unwrap()["w"].life.status, Status::Paused);

    std::fs::write(&path, "[ceilings\n").unwrap();
    assert_eq!(reload(&state).status_code, 400);
    assert_eq!(state.policy().ceilings["request_timeout_ms"], 45000, "unchanged");

    std::fs::write(&path, "").unwrap();
    assert_eq!(body_json(reload(&state))["attention"]["w"], serde_json::Value::Null);
    assert_eq!(state.deployments.lock().unwrap()["w"].life.status, Status::Running);
    let _ = std::fs::remove_file(&path);
}

/// At load, a grant is checked against what the deployment needs now: one that no longer covers
/// it is paused, flagged, and can't run until it is deployed again (which asks for approval).
/// A record from before grants is granted what it was acknowledged for.
#[test]
fn load_rechecks_each_grant_and_migrates_older_records() {
    let mut state = test_state();
    let path = unique_temp_path("grants");
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, ack.clone()).status_code, 200);

    // As if granted less than it needs: drop an item from the stored grant.
    let mut file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file[0]["grant"]["items"] = serde_json::json!(["Action(\"put\")"]);
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    let w = &reloaded["w"];
    assert_eq!(w.life.status, Status::Paused);
    assert!(w.attention.as_deref().unwrap().contains("StateWrite(\"k\")"), "{:?}", w.attention);
    *state.deployments.lock().unwrap() = reloaded;
    let listed = body_json(route(&state, &req("GET", "/deployments", vec![])));
    assert!(listed["deployments"][0]["attention"].is_string());
    assert_eq!(route(&state, &req("POST", "/deployments/w/run", vec![])).status_code, 409);
    // Deploying again asks for the missing item, and then it runs.
    assert_eq!(deploy_json(&state, "w", writer, serde_json::json!({ "acknowledge": ["Action(\"put\")"] })).status_code, 400);
    assert_eq!(deploy_json(&state, "w", writer, ack).status_code, 200);
    assert_eq!(put(&state, "w", "ok").status_code, 200);

    // A record without a grant: granted its derived capabilities, by migration.
    let mut file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file[0].as_object_mut().unwrap().remove("grant");
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let migrated = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(migrated["w"].grant.by, "migrated");
    assert!(migrated["w"].attention.is_none() && migrated["w"].grant.items.contains("StateWrite(\"k\")"));
    let _ = std::fs::remove_file(&path);
}

/// Every logged run carries its id, the nonce in it, and the grant it was held to; the id can be
/// recomputed from the entry alone, and two runs of the same input still differ.
#[test]
fn a_run_id_is_recomputable_from_its_log_entry() {
    let state = test_state();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, ack).status_code, 200);
    put(&state, "w", "same");
    put(&state, "w", "same");
    let log = body_json(route(&state, &req("GET", "/deployments/w/log", vec![])));
    let entries = log["entries"].as_array().unwrap();
    assert_ne!(entries[0]["run_id"], entries[1]["run_id"], "same input, different nonce");
    let grant = body_json(route(&state, &req("GET", "/deployments/w/grant", vec![])));
    assert_eq!(entries[1]["grant"], grant["hash"]);

    let e = &entries[1];
    let sv = |v: &serde_json::Value| serde_json::from_value::<SValue>(v.clone()).unwrap().into_core();
    let state_before = State::from_values(
        e["state_before"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), sv(v))).collect(),
    )
    .unwrap();
    let sourced: Vec<(Capability, Value)> = e["sourced"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (crate::durable::capability_named(p[0].as_str().unwrap()).unwrap(), sv(&p[1])))
        .collect();
    let hex = e["nonce"].as_str().unwrap();
    let nonce: [u8; 16] = (0..16).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap();
    assert_eq!(hex.len(), 32, "128 bits");
    // An action's run has one input: what it was sent.
    let recomputed = run_id_of(writer, &[Some(sv(&e["request"]))], Some(&state_before), &sourced, &nonce);
    assert_eq!(e["run_id"], to_hex(&recomputed));
}

/// A secret is set over the control plane and never returned by it; a run granted it reads it;
/// and no log entry or reply carries its value, which reads `‹secret:name›` wherever it would
/// have appeared.
#[test]
fn a_secret_reaches_its_runs_and_nothing_else() {
    let state = test_state();
    // Its action checks what it is sent against the secret, and would keep the secret in state.
    let text = "import { action, flow } from \"holon\"\nimport state from \"state\"\nimport api_key from \"secret:api_key\"\n\nflow(\"s\")\n\nexport const check = action((value) => {\n  state.k = api_key\n  return value === api_key\n})\n";
    let flow_hash = stage_text(&state, text);
    let value = "sk_test_123456";
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "s", "unit": to_hex(&flow_hash) })).unwrap())));
    assert_eq!(preview["missing_secrets"], serde_json::json!(["api_key"]));
    assert!(preview["needs_approval"].as_array().unwrap().contains(&serde_json::json!("SecretRead(\"api_key\")")));

    // Set it (refusing a too-short one); listing gives names only.
    let set = |v: &str| route(&state, &req("POST", "/deployments/s/secrets/api_key", serde_json::to_vec(&serde_json::json!({ "value": v })).unwrap())).status_code;
    assert_eq!(set("short"), 400);
    assert_eq!(set(value), 200);
    let listed: serde_json::Value = serde_json::from_slice(&body_bytes(route(&state, &req("GET", "/deployments/s/secrets", vec![])))).unwrap();
    assert_eq!(listed["names"], serde_json::json!(["api_key"]));
    assert_eq!(listed["secrets"][0]["version"], 1);
    assert_eq!(listed["secrets"][0]["source"], "local");
    assert!(!listed.to_string().contains(value), "never the value: {listed}");
    // A source other than a value: refused when it can't be read, never added half-set.
    let from = |f: &str| route(&state, &req("POST", "/deployments/s/secrets/other", serde_json::to_vec(&serde_json::json!({ "from": f })).unwrap()));
    // What an env secret can name here: none in this server's (empty) environment.
    let env: serde_json::Value = serde_json::from_slice(&body_bytes(route(&state, &req("GET", "/admin/secrets/env", vec![])))).unwrap();
    assert_eq!(env, serde_json::json!({ "names": [], "dotenv": null }));
    let refused = from("env:HOLON_SECRET_NOT_SET_ANYWHERE");
    assert_eq!(refused.status_code, 400);
    assert!(String::from_utf8(body_bytes(refused)).unwrap().contains("isn't set"));
    assert_eq!(from("env:PATH").status_code, 400);
    assert_eq!(state.secrets.names("s"), vec!["api_key"]);
    // History and rollback, over the control plane: versions and events, never a value.
    assert_eq!(set("another-value-xyz"), 200);
    let history = |name: &str| route(&state, &req("GET", &format!("/deployments/s/secrets/{name}"), vec![]));
    let h: serde_json::Value = serde_json::from_slice(&body_bytes(history("api_key"))).unwrap();
    assert_eq!(h["versions"][0]["version"], 2);
    assert_eq!(h["events"].as_array().unwrap().len(), 2);
    assert!(!h.to_string().contains(value) && !h.to_string().contains("another-value"), "{h}");
    assert_eq!(history("nope").status_code, 404);
    let rollback = |v: u32| route(&state, &req("POST", "/deployments/s/secrets/api_key/rollback", serde_json::to_vec(&serde_json::json!({ "version": v })).unwrap()));
    assert_eq!(rollback(2).status_code, 400, "already current");
    let dry = route(&state, &req("POST", "/deployments/s/secrets/api_key/rollback", br#"{"version":1,"dry_run":true}"#.to_vec()));
    let dry: serde_json::Value = serde_json::from_slice(&body_bytes(dry)).unwrap();
    assert_eq!((dry["to"].clone(), dry["current"].clone(), dry["version"].clone(), dry["source"].clone()), (1.into(), 2.into(), 3.into(), "local".into()));
    // Each value's fingerprint, as the log marks it, never the value: the two differ.
    let (to, current) = (dry["to_print"].as_str().unwrap(), dry["current_print"].as_str().unwrap());
    assert!(to.len() == 8 && current.len() == 8 && to != current, "{dry}");
    assert!(!dry.to_string().contains(value) && !dry.to_string().contains("another-value"), "a dry run: what it would do, never a value");
    assert_eq!(String::from_utf8(body_bytes(rollback(1))).unwrap(), r#"{"version":3}"#);
    assert_eq!(*state.secrets.vault("s").lock().unwrap().current["api_key"], value);

    let ack = serde_json::json!(["Action(\"check\")", "SecretRead(\"api_key\")", "StateWrite(\"k\")"]);
    assert_eq!(deploy_json(&state, "s", flow_hash, serde_json::json!({ "acknowledge": ack })).status_code, 200);
    let fire = |body: &str| String::from_utf8(body_bytes(run_action_named(&state, "s", "check", serde_json::json!(body)))).unwrap();
    let matched = fire(value);
    assert!(matched.contains(r#""answer":true"#), "the run read the secret: {matched}");
    assert!(fire("something else").contains(r#""answer":false"#));

    // The log keeps what each node produced, so a replay can show a node it can't recompute:
    // `equals` said true, and the secret node's own output is its placeholder.
    let log = body_json(route(&state, &req("GET", "/deployments/s/log", vec![])));
    let produced: Vec<serde_json::Value> = log["entries"][0]["timeline"].as_array().unwrap().iter().filter_map(|t| t.get("outputs").cloned()).collect();
    assert!(produced.contains(&serde_json::json!([{ "type": "Boolean", "value": true }])), "{produced:?}");
    // Marked with the version and fingerprint the entry says the run was given.
    let given = log["entries"][0]["secrets"]["api_key"].as_str().unwrap_or_default().to_string();
    assert!(given.starts_with("v3 #"), "the version the test set last: {log}");
    assert!(produced.contains(&serde_json::json!([{ "type": "Text", "value": format!("‹secret:api_key {given}›") }])), "{produced:?}");

    // Nowhere in what the control plane says about it.
    for url in ["/deployments/s/log", "/deployments/s/state", "/deployments/s/sessions", "/deployments/s/secrets", "/deployments/s/grant"] {
        let text = String::from_utf8(body_bytes(route(&state, &req("GET", url, vec![])))).unwrap();
        assert!(!text.contains(value), "{url} leaks it: {text}");
        if url.ends_with("/state") {
            assert!(!text.contains("\"k\""), "a secret is never stored in state: {text}");
        }
    }
    // Not in the value store on disk either: nothing that contains it was ever written.
    assert!(state.deployments.lock().unwrap()["s"].state.as_ref().unwrap().state.lock().unwrap().get("k").is_none());
    assert!(!matched.contains(value), "the action's answer doesn't either");

    // Deleting the deployment forgets its secrets.
    route(&state, &req("POST", "/deployments/s/stop", vec![]));
    assert_eq!(route(&state, &req("DELETE", "/deployments/s", vec![])).status_code, 200);
    assert!(state.secrets.names("s").is_empty());
}

/// A durable run end to end. \`POST /orders\` answers with a resume token and waits at \`paid\`; its
/// state write depends on the event, so nothing is written yet. \`GET /paid/:token\`, declared
/// \`resume = paid\`, answers its caller and delivers its request to the run, which finishes: the
/// write happens then, once, under the same run id. A run started before a redeploy finishes on
/// the version it started on; one nobody resumes expires; waiting runs survive a restart.
#[test]
fn a_durable_run_waits_resumes_by_token_and_expires() {
    let mut state = test_state();
    let path = unique_temp_path("durable");
    state.deployments_path = path.clone().into();
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let iface = |name: &str, params: Vec<Param>| Unit { params, ..holon_core::interface_of(name).unwrap() };
    let frozen = |k: &str, v: &str| Param::frozen(k, v);
    // The order route, writing the event's \`field\` once paid.
    let orders = |field: &str| {
        let children = vec![
            iface("resume_token", vec![frozen("await", "paid")]),
            iface("await", vec![frozen("name", "paid"), frozen("expires", "3600")]),
            iface("field", vec![frozen("name", field)]),
            test_store_write("paid"),
        ];
        let edges = vec![e(1, 0, 2, 0), e(2, 0, 3, 0), e(0, 0, BOUNDARY, 0), e(3, 0, BOUNDARY, 1)];
        endpoint(&state, "POST", "/orders", children, edges, 1)
    };
    // The resume route: answers "thanks".
    let thanks = stage(&state, &iface("const", vec![frozen("value", "thanks"), frozen("type", "Text")]));
    let paid_route = stage(
        &state,
        &Unit {
            body: UnitBody::Composite { children: vec![thanks], edges: vec![e(0, 0, BOUNDARY, 0)] },
            params: vec![frozen("declare", "endpoint"), frozen("method", "GET"), frozen("path", "/paid/:token"), frozen("resume", "paid")],
            inputs: vec![Port { name: "request".into(), ty: Type::Record }],
            outputs: vec![Port { name: "response".into(), ty: Type::Any }],
        },
    );
    let server = |orders: Hash| {
        stage(
            &state,
            &Unit {
                body: UnitBody::Composite { children: vec![orders, paid_route], edges: vec![] },
                params: vec![frozen("declare", "server"), frozen("port", "18090")],
                inputs: vec![],
                outputs: vec![],
            },
        )
    };
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "Await(\"paid\")", "StateWrite(\"paid\")"] });
    let v1 = server(orders("method"));
    assert_eq!(deploy_json(&state, "shop", v1, ack.clone()).status_code, 200);
    let call = |method: &str, url: &str| {
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, "shop", &fake_data(method, url, vec![]));
        (r.status, String::from_utf8(r.body).unwrap())
    };
    let written = || state.deployments.lock().unwrap()["shop"].state.as_ref().unwrap().state.lock().unwrap().get("paid").cloned();
    let runs = || body_json(route(&state, &req("GET", "/deployments/shop/runs", vec![])))["runs"].as_array().unwrap().clone();

    // Order: answered with its token, waiting, nothing written yet.
    let (code, token) = call("POST", "/orders");
    assert_eq!(code, 200);
    assert!(token.starts_with("rt_"), "{token}");
    assert_eq!(written(), None, "a waiting run performs nothing");
    assert_eq!(runs().len(), 1);
    assert_eq!(runs()[0]["waiting"][0]["name"], "paid");
    let listed = String::from_utf8(body_bytes(route(&state, &req("GET", "/deployments/shop/runs", vec![])))).unwrap();
    let log = String::from_utf8(body_bytes(route(&state, &req("GET", "/deployments/shop/log", vec![])))).unwrap();
    assert!(!listed.contains(&token) && !log.contains(&token), "a token is never listed or logged");
    assert!(log.contains("‹token:paid›"));
    // It survives a restart, and its record holds no token: it is derived again when needed.
    assert_eq!(load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap()["shop"].parked.len(), 1);
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains(&token) && !on_disk.contains("\"tokens\""), "a run record holds no token");
    // Holding a value as deep as one may be (an event delivered to it), it survives a restart too:
    // past serde_json's own stop, the deployments file didn't read back, and the server didn't start.
    let deep = (0..holon_core::MAX_VALUE_DEPTH).fold(Value::Number(1.0), |v, _| Value::Record([("k".to_string(), v)].into_iter().collect()));
    let mut records: serde_json::Value = crate::wire::read_json(on_disk.as_bytes()).unwrap();
    let shop = records.as_array_mut().unwrap().iter_mut().find(|r| r["slug"] == "shop").unwrap();
    shop["parked"][0]["events"]["earlier"] = serde_json::to_value(SValue::from_core(&deep)).unwrap();
    let deep_path = unique_temp_path("durable-deep");
    std::fs::write(&deep_path, serde_json::to_vec(&records).unwrap()).unwrap();
    let reloaded = load_deployments(&Place::parse(&deep_path), &state.store, &state.policy()).unwrap();
    assert_eq!(reloaded["shop"].parked.values().next().unwrap().events["earlier"].clone().into_core(), deep);

    // A wrong token: refused before the route's handler runs.
    assert_eq!(call("GET", "/paid/rt_nope").0, 404);
    // The right one: the route answers, and the run finishes, once.
    assert_eq!(call("GET", &format!("/paid/{token}")), (200, "thanks".to_string()));
    assert_eq!(written(), Some(Value::Text("GET".into())), "the event reached the run, and its write happened");
    assert!(runs().is_empty());
    assert_eq!(call("GET", &format!("/paid/{token}")).0, 404, "a token resumes once");
    let log = String::from_utf8(body_bytes(route(&state, &req("GET", "/deployments/shop/log", vec![])))).unwrap();
    assert!(!log.contains(&token), "not in the paying request's entry (its URL carries it), nor in the event the run was given");
    let entries = body_json(route(&state, &req("GET", "/deployments/shop/log", vec![])))["entries"].as_array().unwrap().clone();
    let resumed = entries.iter().find(|x| x["kind"] == "resume").unwrap();
    let first = entries.iter().find(|x| x["kind"] == "request" && x["route"] == "POST /orders").unwrap();
    assert_eq!(resumed["run_id"], first["run_id"], "one run, two parts");
    assert_eq!(first["parked"], serde_json::json!(["paid"]));
    assert!(replays_as_it_verifies(&state, "shop").contains("resume"), "a request's run resumed is its route's");

    // Pinned: a run started on v1 finishes on v1 after v2 is deployed (v2 would write the path).
    let (_, token2) = call("POST", "/orders");
    let v2 = server(orders("path"));
    assert_eq!(deploy_json(&state, "shop", v2, ack.clone()).status_code, 200);
    assert_eq!(runs().len(), 1, "waiting runs are kept across a redeploy");
    assert_eq!(call("GET", &format!("/paid/{token2}")).0, 200);
    assert_eq!(written(), Some(Value::Text("GET".into())), "v1's behaviour, not v2's");
    // And it is logged as v1's, whose canvas shows it, while v2's own runs are v2's.
    let entries = body_json(route(&state, &req("GET", "/deployments/shop/log", vec![])))["entries"].as_array().unwrap().clone();
    assert_eq!(entries.last().unwrap()["kind"], "resume");
    assert_eq!(entries.last().unwrap()["unit"], to_hex(&v1));
    let paying = &entries[entries.len() - 2];
    assert_eq!((paying["kind"].as_str(), paying["unit"].as_str()), (Some("request"), Some(to_hex(&v2).as_str())));

    // A run parked before tokens were derived kept the random one it was handed: still accepted.
    call("POST", "/orders");
    let legacy = "rt_0123456789abcdef0123456789abcdef".to_string();
    state.deployments.lock().unwrap().get_mut("shop").unwrap().parked.values_mut().next().unwrap().tokens.insert("paid".into(), legacy.clone());
    assert_eq!(call("GET", &format!("/paid/{legacy}")).0, 200);
    assert!(runs().is_empty());

    // Expiry: nobody pays; past its deadline it gets {expired: true} and finishes (its write
    // reads a field the event doesn't have, so nothing is written).
    *state.deployments.lock().unwrap().get_mut("shop").unwrap().state.as_ref().unwrap().state.lock().unwrap() = State::new();
    call("POST", "/orders");
    assert_eq!(runs().len(), 1);
    durable::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 4000);
    assert!(runs().is_empty(), "expired and finished");
    assert_eq!(written(), None);
    let entries = body_json(route(&state, &req("GET", "/deployments/shop/log", vec![])))["entries"].as_array().unwrap().clone();
    assert_eq!(entries.last().unwrap()["events"]["paid"]["value"][0][0], "expired");
    let _ = std::fs::remove_file(&path);
}

/// An approval is decided by people signed in, never by a token: two different people approve a
/// two-person approval and the run goes on with who they were; one decline, with a reason, ends
/// it; one that runs out goes on as expired, with who had approved. An agent, a viewer, an API
/// token and the same person twice are refused, and each decision is in the run's log and the
/// activity log.
#[test]
fn an_approval_is_decided_by_different_people_never_by_a_token() {
    let mut state = test_state();
    let text = r#"import { action, approval, flow } from "holon"
import state from "state"

flow("payouts")

export const pay = action(async (order) => {
  const decision = await approval("payout", { expires: "1h", people: 2 })
  state.decided = decision
  return decision.approved
})
"#;
    let unit = stage_text(&state, text);
    let ack = serde_json::json!({ "acknowledge": ["Action(\"pay\")", "Approval(\"payout/2\")", "StateWrite(\"decided\")"] });
    assert_eq!(deploy_json(&state, "payouts", unit, ack).status_code, 200);
    let runs = |state: &AppState, token: Option<&str>| {
        let mut headers = req("GET", "/deployments/payouts/runs", vec![]).headers().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        headers.extend(token.map(|t| ("Authorization".to_string(), format!("Bearer {t}"))));
        body_json(route(state, &Request::fake_http("GET", "/deployments/payouts/runs", headers, vec![])))["runs"].as_array().unwrap().clone()
    };
    // Three runs wait; a start is handed no token for an approval, and none resumes it.
    let mut ids = vec![];
    for amount in [10, 20, 30] {
        let started = run_action_named(&state, "payouts", "pay", serde_json::json!({ "amount": amount }));
        assert!(started.is_success());
        assert!(!String::from_utf8(body_bytes(started)).unwrap().contains("rt_"), "no token for an approval");
        let waiting = runs(&state, None);
        let new = waiting.iter().map(|r| r["run_id"].as_str().unwrap().to_string()).find(|id| !ids.contains(id)).unwrap();
        ids.push(new);
    }
    let first = runs(&state, None).into_iter().find(|r| r["run_id"] == ids[0].as_str()).unwrap();
    assert_eq!((&first["waiting"][0]["name"], &first["waiting"][0]["people"]), (&serde_json::json!("payout"), &serde_json::json!(2)));
    assert!(durable::claim_by_token(&state.deployments, "payouts", "payout", &durable::token_for(&ids[0], "payout")).is_none(), "a token doesn't resume an approval");
    // A run that waits has no answer yet, and hasn't failed: counted, not as an error, and its log
    // entry says where it waits, with no status.
    let (count, errors) = state.deployments.lock().unwrap().get("payouts").map(|d| (d.trigger_count, d.error_count)).unwrap();
    assert_eq!((count, errors), (3, 0), "three runs, none failed");
    let entries = body_json(route(&state, &req("GET", "/deployments/payouts/log", vec![])))["entries"].as_array().unwrap().clone();
    assert!(entries.iter().all(|e| e["status"].is_null() && e["parked"] == serde_json::json!(["payout"])), "{entries:?}");

    // From here on, people sign in.
    let People { ada, claude } = with_people(&mut state);
    let accounts = Arc::clone(state.access.accounts().unwrap());
    let bob = accounts.accept(&accounts.invite("bob", "editor", None, "ada").unwrap(), "correct horse battery").unwrap().0;
    let cy = accounts.accept(&accounts.invite("cy", "viewer", None, "ada").unwrap(), "correct horse battery").unwrap().0;
    accounts.add("ci", crate::accounts::Kind::Service, "editor", "ada", "ada").unwrap();
    let ci = accounts.create_token("ci", "CI on GitHub", "ada").unwrap().0;
    let decide = |token: &str, run: &str, body: serde_json::Value| {
        let r = route(&state, &req_as(token, "POST", &format!("/deployments/payouts/runs/{run}/decide"), serde_json::to_vec(&body).unwrap()));
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
    };
    let approve = serde_json::json!({ "name": "payout", "approve": true });
    let decided = || {
        let v = state.deployments.lock().unwrap()["payouts"].state.as_ref().unwrap().state.lock().unwrap().get("decided").cloned().unwrap();
        crate::wire::read_json::<serde_json::Value>(holon_core::stringify_json(&v).unwrap().as_bytes()).unwrap()
    };

    assert_eq!(decide(&cy, &ids[0], approve.clone()).0, 403, "a viewer only reads");
    let (code, said) = decide(&ci, &ids[0], approve.clone());
    assert_eq!(code, 403);
    assert!(said.contains("ci is a service, a script, not a person"), "{said}");
    assert_eq!(decide(&claude, &ids[0], approve.clone()).0, 403, "an agent never decides");
    let (code, said) = decide(&ada, &ids[0], approve.clone());
    assert_eq!(code, 200, "{said}");
    assert!(said.contains(r#""outcome":"waiting""#) && said.contains(r#""approved_by":["ada"]"#), "{said}");
    let (code, said) = decide(&ada, &ids[0], approve.clone());
    assert_eq!(code, 409);
    assert!(said.contains("ada already approved \\\"payout\\\": it needs 2 different people"), "{said}");
    let listed = runs(&state, Some(&ada)).into_iter().find(|r| r["run_id"] == ids[0].as_str()).unwrap();
    assert_eq!(listed["waiting"][0]["approved"][0]["by"], "ada", "{listed}");
    assert_eq!(decide(&bob, &ids[0], serde_json::json!({ "name": "payout", "approve": false })).0, 400, "a decline says why");
    assert_eq!(decide(&bob, &ids[0], serde_json::json!({ "name": "paid", "approve": true })).0, 404);
    let (code, said) = decide(&bob, &ids[0], serde_json::json!({ "name": "payout", "approve": true, "reason": "the invoice matches" }));
    assert_eq!(code, 200, "{said}");
    assert!(said.contains(r#""outcome":"approved""#), "{said}");
    let d = decided();
    assert_eq!((&d["approved"], &d["by"]), (&serde_json::json!(true), &serde_json::json!(["ada", "bob"])), "{d}");
    assert_eq!(d["approvals"][1]["reason"], "the invoice matches");
    assert!(!runs(&state, Some(&ada)).iter().any(|r| r["run_id"] == ids[0].as_str()), "decided, it finished");
    assert_eq!(decide(&bob, &ids[0], approve.clone()).0, 404, "decided once");

    // One decline is enough.
    let (code, said) = decide(&bob, &ids[1], serde_json::json!({ "name": "payout", "approve": false, "reason": "  not this vendor  " }));
    assert_eq!(code, 200, "{said}");
    assert!(said.contains(r#""outcome":"declined""#), "{said}");
    let d = decided();
    assert_eq!((&d["approved"], &d["declined_by"], &d["reason"], &d["by"]), (&serde_json::json!(false), &serde_json::json!("bob"), &serde_json::json!("not this vendor"), &serde_json::json!([])), "{d}");

    // Nobody else decides in time: it goes on as expired, with who approved.
    assert_eq!(decide(&ada, &ids[2], approve.clone()).0, 200);
    durable::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 4000);
    let d = decided();
    assert_eq!((&d["approved"], &d["expired"], &d["by"]), (&serde_json::json!(false), &serde_json::json!(true), &serde_json::json!(["ada"])), "{d}");
    assert!(runs(&state, Some(&ada)).is_empty());

    // Each resumed part's log entry holds the decision it was given; the activity log, who decided.
    let as_ada = |url: &str| body_json(route(&state, &req_as(&ada, "GET", url, vec![])));
    let entries = as_ada("/deployments/payouts/log")["entries"].as_array().unwrap().clone();
    let resumed: Vec<_> = entries.iter().filter(|e| e["kind"] == "resume").collect();
    assert_eq!(resumed.len(), 3);
    assert!(resumed.iter().all(|e| e["events"].get("payout").is_some()), "{resumed:?}");
    let activity = as_ada("/activity")["entries"].as_array().unwrap().clone();
    let did = |action: &str, by: &str| activity.iter().filter(|e| e["action"] == action && e["by"] == by).count();
    assert_eq!((did("approved", "ada"), did("approved", "bob"), did("declined", "bob")), (2, 1, 1), "{activity:?}");
}

/// Who may decide an approval: a person by their name, the OS user's local admin by the name it
/// runs as; not an agent, a service, a viewer, or anyone on a server without accounts.
#[test]
fn only_a_person_decides_an_approval() {
    use crate::accounts::Kind;
    let caller = |name: Option<&str>, kind: Option<Kind>, role: Option<access::Role>| match (name, kind, role) {
        (Some(name), Some(kind), Some(role)) => access::Caller::holding(name, kind, role),
        _ => access::Caller::default(),
    };
    let decider = |c: access::Caller| durable::decider(&c, "d", "ok").map_err(|refusal| refusal.0);
    assert_eq!(decider(caller(Some("ada"), Some(Kind::Person), Some(access::Role::Editor))), Ok("ada".to_string()));
    assert_eq!(decider(caller(Some("ada"), Some(Kind::Person), Some(access::Role::Admin))), Ok("ada".to_string()));
    assert_eq!(decider(caller(Some("local:ada"), Some(Kind::Person), Some(access::Role::Admin))), Ok("ada".to_string()), "the same person as the account");
    let refused = [
        (Some("claude"), Some(Kind::Agent), Some(access::Role::Agent), "claude may not do that: an agent proposes"),
        (Some("ci"), Some(Kind::Service), Some(access::Role::Editor), "ci is a service"),
        (Some("cy"), Some(Kind::Person), Some(access::Role::Viewer), "a viewer only reads"),
        (None, None, None, "no accounts"),
    ];
    for (name, kind, role, why) in refused {
        let said = decider(caller(name, kind, role)).unwrap_err();
        assert!(said.contains(why), "{name:?}: {said}");
    }
}

/// What a waiting run keeps that may hold a secret (the request that started it, the requests it
/// made, the events delivered to it) is sealed in the deployments file under the secrets master
/// key, which is kept apart from it, and the file is its owner's alone. The run still resumes
/// after a restart.
#[test]
fn a_waiting_runs_record_is_sealed_on_disk() {
    let mut state = test_state();
    let dir = unique_temp_path("sealed-parked");
    std::fs::create_dir_all(&dir).unwrap();
    let path = format!("{dir}/deployments.json");
    state.deployments_path = path.clone().into();
    let key = crate::sealing::KeySource::File(std::path::PathBuf::from(format!("{dir}/secrets.key")));
    state.secrets = Arc::new(SecretStore::open(&format!("{dir}/secrets.json"), &key, crate::secret_sources::Sources::none()).unwrap());
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let iface = |name: &str, params: Vec<Param>| Unit { params, ..holon_core::interface_of(name).unwrap() };
    let frozen = |k: &str, v: &str| Param::frozen(k, v);
    let children = vec![
        iface("resume_token", vec![frozen("await", "paid")]),
        iface("await", vec![frozen("name", "paid"), frozen("expires", "3600")]),
        iface("field", vec![frozen("name", "method")]),
        test_store_write("paid"),
    ];
    let edges = vec![e(1, 0, 2, 0), e(2, 0, 3, 0), e(0, 0, BOUNDARY, 0), e(3, 0, BOUNDARY, 1)];
    let orders = endpoint(&state, "POST", "/orders", children, edges, 1);
    let thanks = stage(&state, &iface("const", vec![frozen("value", "thanks"), frozen("type", "Text")]));
    let paid_route = stage(
        &state,
        &Unit {
            body: UnitBody::Composite { children: vec![thanks], edges: vec![e(0, 0, BOUNDARY, 0)] },
            params: vec![frozen("declare", "endpoint"), frozen("method", "GET"), frozen("path", "/paid/:token"), frozen("resume", "paid")],
            inputs: vec![Port { name: "request".into(), ty: Type::Record }],
            outputs: vec![Port { name: "response".into(), ty: Type::Any }],
        },
    );
    let server = stage(
        &state,
        &Unit {
            body: UnitBody::Composite { children: vec![orders, paid_route], edges: vec![] },
            params: vec![frozen("declare", "server"), frozen("port", "18094")],
            inputs: vec![],
            outputs: vec![],
        },
    );
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "Await(\"paid\")", "StateWrite(\"paid\")"] });
    assert_eq!(deploy_json(&state, "sealed", server, ack).status_code, 200);
    // A secret set for it rebuilds its vault, as a source read again does: it still seals.
    state.secrets.set("sealed", "api_key", "sk_live_abcdef", Some("ada")).unwrap();
    assert!(state.deployments.lock().unwrap()["sealed"].secrets.lock().unwrap().sealer.is_some(), "a vault rebuilt keeps its sealer");
    // The order carries its caller's credential, which the waiting run keeps with its request.
    let mut order = fake_data("POST", "/orders", vec![]);
    order.headers.push(("authorization".into(), "Bearer caller_credential_xyz".into()));
    let token = String::from_utf8(serve_data(&state.store, &state.deployments, &state.deployments_path, "sealed", &order).body).unwrap();
    assert!(state.deployments.lock().unwrap()["sealed"].parked.values().next().unwrap().sealed.is_none(), "open in memory");

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("caller_credential_xyz"), "the file holds the credential: {on_disk}");
    assert!(on_disk.contains("\"sealed\""), "{on_disk}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "its owner's alone");
    }

    // A restart: loaded sealed, opened with the deployment's vault, and it resumes.
    let mut reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    {
        let dep = reloaded.get_mut("sealed").unwrap();
        dep.secrets = state.secrets.vault("sealed");
        let sealer = dep.secrets.lock().unwrap().sealer.clone();
        let run = dep.parked.values_mut().next().unwrap();
        assert!(run.sealed.is_some() && run.inputs.is_empty(), "loaded sealed");
        assert!(run.open("other-slug", sealer.as_deref()).is_err(), "bound to its deployment");
        run.open("sealed", sealer.as_deref()).unwrap();
        assert!(format!("{:?}", run.inputs).contains("caller_credential_xyz"), "opened whole");
    }
    *state.deployments.lock().unwrap() = reloaded;
    let paid = serve_data(&state.store, &state.deployments, &state.deployments_path, "sealed", &fake_data("GET", &format!("/paid/{token}"), vec![]));
    assert_eq!(paid.status, 200);
    assert!(state.deployments.lock().unwrap()["sealed"].parked.is_empty(), "resumed and finished");
    let _ = std::fs::remove_dir_all(&dir);
}

/// With secrets kept in memory only (`--secrets :memory:`) and the deployments on disk, a waiting
/// run is sealed all the same, under a key that is never kept: the file never holds what it
/// keeps, and after a restart (its secrets gone with it) it says it can't be opened.
#[test]
fn a_waiting_run_is_sealed_on_disk_with_secrets_in_memory_too() {
    let mut state = test_state();
    let dir = unique_temp_path("sealed-memory");
    std::fs::create_dir_all(&dir).unwrap();
    let path = format!("{dir}/deployments.json");
    state.deployments_path = path.clone().into();
    state.secrets = Arc::new(SecretStore::memory());
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let iface = |name: &str, params: Vec<Param>| Unit { params, ..holon_core::interface_of(name).unwrap() };
    let frozen = |k: &str, v: &str| Param::frozen(k, v);
    let children = vec![
        iface("resume_token", vec![frozen("await", "paid")]),
        iface("await", vec![frozen("name", "paid"), frozen("expires", "3600")]),
        iface("field", vec![frozen("name", "method")]),
        test_store_write("paid"),
    ];
    let edges = vec![e(1, 0, 2, 0), e(2, 0, 3, 0), e(0, 0, BOUNDARY, 0), e(3, 0, BOUNDARY, 1)];
    let orders = endpoint(&state, "POST", "/orders", children, edges, 1);
    let server = stage(
        &state,
        &Unit {
            body: UnitBody::Composite { children: vec![orders], edges: vec![] },
            params: vec![frozen("declare", "server"), frozen("port", "18095")],
            inputs: vec![],
            outputs: vec![],
        },
    );
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "Await(\"paid\")", "StateWrite(\"paid\")"] });
    assert_eq!(deploy_json(&state, "sealed", server, ack).status_code, 200);
    let mut order = fake_data("POST", "/orders", vec![]);
    order.headers.push(("authorization".into(), "Bearer caller_credential_xyz".into()));
    serve_data(&state.store, &state.deployments, &state.deployments_path, "sealed", &order);
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains("caller_credential_xyz"), "the file holds the credential: {on_disk}");
    assert!(on_disk.contains("\"sealed\""), "{on_disk}");
    // A restart, with a new in-memory store: its key is another, and the run says so.
    let mut reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    let dep = reloaded.get_mut("sealed").unwrap();
    dep.secrets = SecretStore::memory().vault("sealed");
    let sealer = dep.secrets.lock().unwrap().sealer.clone();
    let run = dep.parked.values_mut().next().unwrap();
    assert!(run.open("sealed", sealer.as_deref()).is_err(), "its key went with the server");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A token resumes its run once even when requests carrying it arrive together: the first claims
/// the run as its token is checked, so a second finds nothing waiting while the first runs, and
/// the deadline passing doesn't expire it under the resume either. Once the resume lets go of a
/// run still waiting, its token works again.
#[test]
fn a_token_resumes_its_run_once_however_many_requests_carry_it() {
    let state = test_state();
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let iface = |name: &str, params: Vec<Param>| Unit { params, ..holon_core::interface_of(name).unwrap() };
    let frozen = |k: &str, v: &str| Param::frozen(k, v);
    let children = vec![
        iface("resume_token", vec![frozen("await", "paid")]),
        iface("await", vec![frozen("name", "paid"), frozen("expires", "3600")]),
        iface("field", vec![frozen("name", "method")]),
        test_store_write("paid"),
    ];
    let edges = vec![e(1, 0, 2, 0), e(2, 0, 3, 0), e(0, 0, BOUNDARY, 0), e(3, 0, BOUNDARY, 1)];
    let orders = endpoint(&state, "POST", "/orders", children, edges, 1);
    let thanks = stage(&state, &iface("const", vec![frozen("value", "thanks"), frozen("type", "Text")]));
    let paid_route = stage(
        &state,
        &Unit {
            body: UnitBody::Composite { children: vec![thanks], edges: vec![e(0, 0, BOUNDARY, 0)] },
            params: vec![frozen("declare", "endpoint"), frozen("method", "GET"), frozen("path", "/paid/:token"), frozen("resume", "paid")],
            inputs: vec![Port { name: "request".into(), ty: Type::Record }],
            outputs: vec![Port { name: "response".into(), ty: Type::Any }],
        },
    );
    let server = stage(
        &state,
        &Unit {
            body: UnitBody::Composite { children: vec![orders, paid_route], edges: vec![] },
            params: vec![frozen("declare", "server"), frozen("port", "18093")],
            inputs: vec![],
            outputs: vec![],
        },
    );
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "Await(\"paid\")", "StateWrite(\"paid\")"] });
    assert_eq!(deploy_json(&state, "once", server, ack).status_code, 200);
    let call = |method: &str, url: &str| serve_data(&state.store, &state.deployments, &state.deployments_path, "once", &fake_data(method, url, vec![])).status;
    let resumes = || {
        let log = body_json(route(&state, &req("GET", "/deployments/once/log", vec![])));
        log["entries"].as_array().unwrap().iter().filter(|x| x["kind"] == "resume").count()
    };
    let token = String::from_utf8(serve_data(&state.store, &state.deployments, &state.deployments_path, "once", &fake_data("POST", "/orders", vec![])).body).unwrap();

    // While a resume holds the run, its token finds nothing, and its deadline doesn't expire it.
    let held = durable::claim_by_token(&state.deployments, "once", "paid", &token).expect("the run waits for its token");
    assert_eq!(call("GET", &format!("/paid/{token}")), 404, "a second resume while the first runs");
    durable::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 4000);
    assert_eq!(resumes(), 0, "not expired under the resume");
    // Let go with the run still waiting (a resume that failed before it ran): the token works again.
    drop(held);
    assert_eq!(call("GET", &format!("/paid/{token}")), 200);
    assert_eq!(resumes(), 1);

    // Many requests at once with one token: one resumes it, the rest are told it was used.
    let token = String::from_utf8(serve_data(&state.store, &state.deployments, &state.deployments_path, "once", &fake_data("POST", "/orders", vec![])).body).unwrap();
    let statuses: Vec<u16> = std::thread::scope(|s| {
        let calls: Vec<_> = (0..8).map(|_| s.spawn(|| call("GET", &format!("/paid/{token}")))).collect();
        calls.into_iter().map(|c| c.join().unwrap()).collect()
    });
    assert_eq!(statuses.iter().filter(|s| **s == 200).count(), 1, "{statuses:?}");
    assert!(statuses.iter().all(|s| *s == 200 || *s == 404), "{statuses:?}");
    assert_eq!(resumes(), 2, "the second order's run resumed once");
}

/// How long runs may wait and how many may wait at once are limits the host grants: an `await`
/// longer than the default is refused until the deployment asks for it (and never past the
/// ceiling), and parking holds to the grant in force, a lowered one included.
#[test]
fn waiting_is_held_to_the_granted_limits() {
    let state = test_state();
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let iface = |name: &str, params: Vec<Param>| Unit { params, ..holon_core::interface_of(name).unwrap() };
    let frozen = |k: &str, v: &str| Param::frozen(k, v);
    let server = |expires: &str| {
        let children = vec![
            iface("resume_token", vec![frozen("await", "paid")]),
            iface("await", vec![frozen("name", "paid"), frozen("expires", expires)]),
            iface("field", vec![frozen("name", "method")]),
            test_store_write("paid"),
        ];
        let edges = vec![e(1, 0, 2, 0), e(2, 0, 3, 0), e(0, 0, BOUNDARY, 0), e(3, 0, BOUNDARY, 1)];
        let orders = endpoint(&state, "POST", "/orders", children, edges, 1);
        stage(
            &state,
            &Unit {
                body: UnitBody::Composite { children: vec![orders], edges: vec![] },
                params: vec![frozen("declare", "server"), frozen("port", "18091")],
                inputs: vec![],
                outputs: vec![],
            },
        )
    };
    let sixty_days = (60 * 24 * 3600).to_string();
    let long = server(&sixty_days);
    let ack = ["NetworkIngress", "Await(\"paid\")", "StateWrite(\"paid\")"];

    // Sixty days is longer than the default month: refused, naming the limit to ask for.
    let refused = deploy_json(&state, "shop", long, serde_json::json!({ "acknowledge": ack }));
    assert_eq!(refused.status_code, 400);
    let why = body_json(refused)["error"].as_str().unwrap().to_string();
    assert!(why.contains(&format!("max_await_seconds={sixty_days}")), "{why}");
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "shop", "unit": to_hex(&long) })).unwrap())));
    assert!(preview["refused"].as_str().unwrap().contains("max_await_seconds"), "{preview}");

    // Asked for, it needs approval like any raised limit; past the ceiling it is refused.
    let asking = |n: u64| serde_json::json!({ "limits": { "max_await_seconds": n } });
    let raise = format!("Limit(max_await_seconds={sixty_days})");
    let approved = [&ack[..], &[raise.as_str()]].concat();
    let n: u64 = sixty_days.parse().unwrap();
    assert_eq!(deploy_json(&state, "shop", long, serde_json::json!({ "acknowledge": ack, "requirements": asking(n) })).status_code, 400);
    assert!(body_json(deploy_json(&state, "shop", long, serde_json::json!({ "acknowledge": ack, "requirements": asking(100 * 24 * 3600) })))["error"]
        .as_str()
        .unwrap()
        .contains("ceiling"));
    assert_eq!(deploy_json(&state, "shop", long, serde_json::json!({ "acknowledge": approved, "requirements": asking(n) })).status_code, 200);

    // Lowered to one waiting run, and to an hour's wait, kept running: the first order waits (an
    // hour, not sixty days), the second is refused.
    let lowered = serde_json::json!({ "lower": { "max_waiting_runs": 1, "max_await_seconds": 3600 }, "keep_running": true });
    assert_eq!(body_json(change_grant(&state, "shop", lowered))["status"], "running");
    let call = || serve_data(&state.store, &state.deployments, &state.deployments_path, "shop", &fake_data("POST", "/orders", vec![])).status;
    let before = epoch_now();
    assert_eq!(call(), 200);
    let deadline = state.deployments.lock().unwrap()["shop"].parked.values().next().unwrap().waiting[0].expires_at;
    assert!(deadline <= before + 3600 + 1, "held to the lowered wait: {deadline} vs {before}");
    assert_eq!(call(), 503, "as many runs waiting as it may");
    assert_eq!(state.deployments.lock().unwrap()["shop"].parked.len(), 1);
}

// ── persistence ──────────────────────────────────────────────────────────────────────

/// A deployment survives a restart: a fresh load from the same file and store has it again, its
/// triggers derived anew, and it runs.
#[test]
fn deployments_persist_across_a_simulated_restart() {
    let deployments_path = unique_temp_path("deployments");
    let store_dir = unique_temp_path("store");

    // A real on-disk store, not MemStore — the deployment definition surviving only means
    // something if the unit it points at is still there too.
    let backend: DynStore = Box::new(FileStore::new(&store_dir).unwrap());
    let store = SharedStore::new(backend);

    // "First run": nothing to load yet, deploy, which persists write-through.
    let unit = {
        let deployments = Arc::new(Mutex::new(load_deployments(&Place::parse(&deployments_path), &store, &Policy::default()).unwrap()));
        assert_eq!(deployments.lock().unwrap().len(), 0);

        let state = AppState {
            store: store.clone(),            deployments,
            deployments_path: Place::parse(&deployments_path),
            flows: Mutex::new(BTreeMap::new()),
            flows_path: Place::Nowhere,
            port: TEST_PORT,
            listeners: None,
            data_limits: dataplane::DataLimits::default(),
            policy: crate::sync::RwLock::new(Arc::new(Policy::default())),
            policy_path: Place::Nowhere,
            secrets: Arc::new(SecretStore::memory()),
            folders: Arc::new(folders::Folders::memory()),
            access: access::Access::default(),
            activity: activity::Activity::memory(),
            editor: None,
            sweeps: Default::default(),
            checked: Default::default(),
            proposals: Default::default(),
        };
        let unit = stage_text(&state, ECHO);
        let ack = serde_json::json!({ "acknowledge": ["Action(\"echo\")"] });
        assert_eq!(deploy_json(&state, "echo", unit, ack).status_code, 200);
        unit
    };

    // "Restart": a fresh load from the same path/store reconstructs the deployment, with
    // triggers re-derived (not read back from disk), and it actually works.
    let reloaded = load_deployments(&Place::parse(&deployments_path), &store, &Policy::default()).unwrap();
    assert_eq!(reloaded.len(), 1);
    let dep = reloaded.get("echo").unwrap();
    assert_eq!(dep.unit, unit);
    assert_eq!(dep.triggers, vec!["action echo".to_string()]);

    let state2 = AppState {
        store: store.clone(),        deployments: Arc::new(Mutex::new(reloaded)),
        deployments_path: Place::parse(&deployments_path),
        flows: Mutex::new(BTreeMap::new()),
        flows_path: Place::Nowhere,
        port: TEST_PORT,
        listeners: None,
        data_limits: dataplane::DataLimits::default(),
        policy: crate::sync::RwLock::new(Arc::new(Policy::default())),
        policy_path: Place::Nowhere,
        secrets: Arc::new(SecretStore::memory()),
        folders: Arc::new(folders::Folders::memory()),
        access: access::Access::default(),
        activity: activity::Activity::memory(),
        editor: None,
        sweeps: Default::default(),
        checked: Default::default(),
        proposals: Default::default(),
    };
    let ran = run_action_named(&state2, "echo", "echo", serde_json::json!("restarted"));
    assert_eq!(ran.status_code, 200);
    assert_eq!(body_json(ran)["answer"], "restarted");

    let _ = std::fs::remove_file(&deployments_path);
    let _ = std::fs::remove_dir_all(&store_dir);
}

// ── multi-flow overlap ───────────────────────────────────────────────────────────────

/// Every clock due in a pass ticks in it: one unit deployed under two slugs, both due at once.
#[test]
fn multiple_due_clock_deployments_all_fire_in_one_pass() {
    let state = test_state();
    let unit = stage_text(&state, TICKER);

    // The same unit, deployed twice under different slugs — a legitimate real scenario (a flow of
    // clocks alone listens on no port), and enough to exercise the per-tick fan-out.
    for slug in ["tick-a", "tick-b"] {
        let ack = serde_json::json!({ "acknowledge": ["Clock", "StateWrite(\"last_tick\")"] });
        assert_eq!(deploy_json(&state, slug, unit, ack).status_code, 200);
    }

    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, SystemTime::now());

    for slug in ["tick-a", "tick-b"] {
        let h = route(&state, &req("GET", &format!("/deployments/{slug}/history"), vec![]));
        let ticks = body_json(h)["ticks"].as_array().unwrap().clone();
        assert_eq!(ticks.len(), 1, "slug {slug} should have exactly one tick recorded");
        assert_eq!(ticks[0]["performed"][0]["capability"], "StateWrite(\"last_tick\")");
    }
}

// ── flows: named workspace documents (save / list / load / delete) ────────────────────

#[test]
fn flow_saves_lists_loads_and_deletes() {
    let state = test_state();

    // Save — the document is opaque; positions/viewport are exactly the kind of thing it
    // carries, and the server must hand back byte-identical structure without touching it.
    let doc = serde_json::json!({
        "nodes": [{ "id": "n1", "position": { "x": 120.5, "y": -40.0 }, "data": { "name": "const" } }],
        "edges": [],
        "viewport": { "x": 12.0, "y": 34.0, "zoom": 1.5 },
    });
    let save = serde_json::to_vec(&serde_json::json!({ "name": "greet", "document": doc })).unwrap();
    let r = route(&state, &req("POST", "/flows", save));
    assert_eq!(r.status_code, 200);
    assert_eq!(body_json(r)["name"], "greet");

    // List — name + freshness only, never the document.
    let list = body_json(route(&state, &req("GET", "/flows", vec![])));
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["name"], "greet");
    assert!(list[0].get("document").is_none());

    // Load — the document comes back exactly as saved, viewport and positions intact.
    let loaded = body_json(route(&state, &req("GET", "/flows/greet", vec![])));
    assert_eq!(loaded["document"], doc);

    // Delete, then both lookups agree it's gone.
    assert_eq!(route(&state, &req("DELETE", "/flows/greet", vec![])).status_code, 200);
    assert_eq!(route(&state, &req("GET", "/flows/greet", vec![])).status_code, 404);
    let empty = body_json(route(&state, &req("GET", "/flows", vec![])));
    assert_eq!(empty.as_array().unwrap().len(), 0);
}

#[test]
fn saving_the_same_name_overwrites_it() {
    // A flow is a mutable workspace file — save means save, the same name replaces.
    let state = test_state();
    let v1 = serde_json::to_vec(&serde_json::json!({ "name": "wip", "document": { "rev": 1 } })).unwrap();
    let v2 = serde_json::to_vec(&serde_json::json!({ "name": "wip", "document": { "rev": 2 } })).unwrap();
    assert_eq!(route(&state, &req("POST", "/flows", v1)).status_code, 200);
    assert_eq!(route(&state, &req("POST", "/flows", v2)).status_code, 200);

    let list = body_json(route(&state, &req("GET", "/flows", vec![])));
    assert_eq!(list.as_array().unwrap().len(), 1); // still one flow, not two
    let loaded = body_json(route(&state, &req("GET", "/flows/wip", vec![])));
    assert_eq!(loaded["document"]["rev"], 2);
}

#[test]
fn flow_save_rejects_a_bad_name_and_a_malformed_body() {
    let state = test_state();
    let bad_name =
        serde_json::to_vec(&serde_json::json!({ "name": "no spaces!", "document": {} })).unwrap();
    assert_eq!(route(&state, &req("POST", "/flows", bad_name)).status_code, 400);
    assert_eq!(route(&state, &req("POST", "/flows", b"not json".to_vec())).status_code, 400);
    // Neither attempt saved anything.
    let list = body_json(route(&state, &req("GET", "/flows", vec![])));
    assert_eq!(list.as_array().unwrap().len(), 0);
}

#[test]
fn deleting_an_unknown_flow_is_404() {
    let state = test_state();
    assert_eq!(route(&state, &req("DELETE", "/flows/nope", vec![])).status_code, 404);
}

#[test]
fn flows_persist_across_a_simulated_restart() {
    let flows_path = unique_temp_path("flows");
    let doc = serde_json::json!({ "nodes": [], "viewport": { "zoom": 2.0 } });

    // "First run": save one flow against a real on-disk path, then drop the whole state.
    {
        let mut state = test_state();
        state.flows_path = flows_path.clone().into();
        let save =
            serde_json::to_vec(&serde_json::json!({ "name": "keeper", "document": doc })).unwrap();
        assert_eq!(route(&state, &req("POST", "/flows", save)).status_code, 200);
    }

    // "Restart": a fresh state loads the table the way main() does, from the same path.
    let mut state2 = test_state();
    state2.flows = Mutex::new(load_flows(&Place::parse(&flows_path)).unwrap());
    state2.flows_path = flows_path.clone().into();
    let loaded = body_json(route(&state2, &req("GET", "/flows/keeper", vec![])));
    assert_eq!(loaded["document"], doc);

    let _ = std::fs::remove_file(&flows_path);
}

/// Loading never skips what it can't load, and never drops persisted state.
#[test]
fn unloadable_records_are_an_error_and_state_always_survives_a_restart() {
    // Regression 1: a record whose unit wasn't in the store (e.g. `--store :memory:` against
    // a persisted file) was skipped, and the next save then dropped it and its state.
    let state = test_state();
    let path = unique_temp_path("unloadable");
    let missing = format!("{{\"slug\":\"gone\",\"unit\":\"{}\",\"trigger_count\":3,\"error_count\":0}}", "ab".repeat(32));
    for contents in [format!("[{missing}]"), r#"[{"slug":"bad","unit":"not-hex","trigger_count":0,"error_count":0}]"#.to_string()] {
        std::fs::write(&path, &contents).unwrap();
        let e = load_deployments(&Place::parse(&path), &state.store, &state.policy()).err().expect("must refuse, not skip");
        assert!(e.contains("deployment"), "{e}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents, "file left as it was");
    }
    std::fs::write(&path, r#"[{"name":"../evil","document":{},"saved_at_epoch_seconds":0}]"#).unwrap();
    assert!(load_flows(&Place::parse(&path)).is_err());

    // Regression 2: a redeploy keeps a slug's state across a stateless version, but loading
    // only restored state for units deriving a state capability, so a restart dropped it.
    let mut state = test_state();
    state.deployments_path = path.clone().into();
    let deploy = |unit: Hash, ack: serde_json::Value| {
        let body = serde_json::to_vec(&serde_json::json!({ "slug": "writer", "unit": to_hex(&unit), "acknowledge": ack }))
            .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
    };
    let writer = stage_writer(&state, "k");
    deploy(writer, serde_json::json!(["Action(\"put\")", "StateWrite(\"k\")"]));
    assert_eq!(put(&state, "writer", "kept").status_code, 200);
    let stateless = stage_text(&state, ECHO);
    deploy(stateless, serde_json::json!(["Action(\"echo\")"]));

    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    let held = reloaded["writer"].state.as_ref().expect("state must survive the restart").state.lock().unwrap().clone();
    assert_eq!(held.get("k"), Some(&Value::Text("kept".into())));
    let _ = std::fs::remove_file(&path);
}

/// State is kept by reference: the deployments file holds each key's value hash, the values
/// live in the content store, and the state's Merkle root comes back the same after a restart.
/// A session snapshot is references too, so restoring a session gives back the very same root,
/// and snapshots holding the same value share it.
#[test]
fn state_is_kept_by_reference_and_its_root_survives_a_restart() {
    let mut state = test_state();
    let path = unique_temp_path("merkle");
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let body = serde_json::to_vec(&serde_json::json!({
        "slug": "writer", "unit": to_hex(&writer), "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
    let write = |text: &str| assert_eq!(put(&state, "writer", text).status_code, 200);
    let current = || body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    let act = |action: &str, body: &str| {
        assert_eq!(route(&state, &req("POST", &format!("/deployments/writer/{action}"), body.as_bytes().to_vec())).status_code, 200)
    };
    write("kept");
    let first = current();
    assert_eq!(first["state"]["k"]["value"], "kept");
    let root = first["root"].as_str().unwrap().to_string();
    assert_eq!(root.len(), 64);

    // The state is kept by its head: a file naming a snapshot of references, the deployments
    // file holding no state at all; and a restart gets the same root back.
    let file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(file[0].get("state").is_none() && file[0].get("state_refs").is_none(), "no state in the deployments file: {}", file[0]);
    let head = std::fs::read_to_string(state_head_for(&Place::parse(&path), "writer").unwrap()).unwrap();
    let hash = hash_value(&Value::Text("kept".into())).unwrap();
    assert_eq!(crate::restore::snapshot_refs(head.trim(), &state.store).unwrap()["k"], hash);
    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(to_hex(&reloaded["writer"].state.as_ref().unwrap().state.lock().unwrap().root()), root);

    // Stop (session 1 keeps "kept"), start fresh and write something else (session 2), then
    // restore session 1: the root is the one it had, and its snapshot shares the value's hash.
    act("stop", "");
    act("run", r#"{"from":"fresh"}"#);
    write("other");
    assert_ne!(current()["root"], root.as_str());
    act("stop", "");
    act("run", r#"{"from":1}"#);
    assert_eq!(current()["root"], root.as_str(), "session 1 restored exactly");
    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(to_hex(&reloaded["writer"].state.as_ref().unwrap().state.lock().unwrap().root()), root, "and kept by the head");
    act("stop", "");
    let sessions = body_json(route(&state, &req("GET", "/deployments/writer/sessions", vec![])));
    let s = |i: usize| &sessions["sessions"][i];
    assert_eq!(s(0)["state"]["k"]["value"], "kept", "the sessions API still answers with values");
    assert_eq!(s(0)["root"], root.as_str());
    assert_eq!(s(1)["state"]["k"]["value"], "other");
    assert_eq!(s(0)["state_refs"]["k"], s(2)["state_refs"]["k"], "one value, one hash, stored once");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(Path::new(&path).with_extension("state"));
    let _ = std::fs::remove_dir_all(Path::new(&path).with_extension("logs"));
}

/// Regression: a persistence file that failed to parse used to load as an empty table, so
/// the first deploy or save after startup overwrote every deployment, its state, or every
/// flow. Loading is now an `Err` (main() refuses to start) and the file is left untouched.
#[test]
fn corrupt_persistence_file_is_an_error_not_an_empty_table() {
    let state = test_state();
    for (label, contents) in [("truncated", &b"[{\"slug\": \"wri"[..]), ("foreign", &b"{\"not\": \"a list\"}"[..])] {
        let path = unique_temp_path(&format!("corrupt-{label}"));
        std::fs::write(&path, contents).unwrap();
        assert!(load_deployments(&Place::parse(&path), &state.store, &state.policy()).is_err(), "{label} deployments file");
        assert!(load_flows(&Place::parse(&path)).is_err(), "{label} flows file");
        assert_eq!(std::fs::read(&path).unwrap(), contents, "the bad file must be left as it was");
        let _ = std::fs::remove_file(&path);
    }
    // A missing file is still a normal first run, not an error.
    let missing = unique_temp_path("missing");
    assert!(load_deployments(&Place::parse(&missing), &state.store, &Policy::default()).unwrap().is_empty());
    assert!(load_flows(&Place::parse(&missing)).unwrap().is_empty());
}

/// Persisting goes through `write_atomically`: the file always holds one whole, parseable
/// table, and no temp file is left next to it.
#[test]
fn persist_writes_whole_files_and_leaves_no_temp_files() {
    let dir = unique_temp_path("persist-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let path = format!("{dir}/flows.json");
    let mut flows = BTreeMap::new();
    for name in ["a", "b"] {
        flows.insert(
            name.to_string(),
            FlowRecord { name: name.to_string(), saved_at_epoch_seconds: 0, document: serde_json::json!({}), saved_by: None, uses: None },
        );
        persist_flows(&Place::parse(&path), &flows);
    }
    flows.remove("a");
    persist_flows(&Place::parse(&path), &flows); // shrinking: no stale tail from the longer write
    assert_eq!(load_flows(&Place::parse(&path)).unwrap().keys().collect::<Vec<_>>(), vec!["b"]);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── every honestly-answerable source is supplied, whatever started the run ────────────────

/// An action's run is given the clock and randomness, though a person started it, not a clock.
/// Regression: `random` was once supplied by no run at all, and the clock only on a clock's tick,
/// so a flow read nothing there, silently (an absent value is dropped before compute runs).
#[test]
fn an_actions_run_is_given_the_clock_and_randomness() {
    let state = test_state();
    let text = "import { action, bytes_to_text, flow } from \"holon\"\nimport { now } from \"holon:clock\"\nimport { random, random_bytes } from \"holon:random\"\n\nflow(\"sources\")\n\nexport const read = action(() => ({ at: now, r: random, bytes: bytes_to_text(random_bytes, { encoding: \"base64\" }) }))\n";
    let unit = stage_text(&state, text);
    let ack = serde_json::json!({ "acknowledge": ["Action(\"read\")", "Clock", "Random"] });
    assert_eq!(deploy_json(&state, "sources", unit, ack).status_code, 200);
    let answered = body_json(run_action_named(&state, "sources", "read", serde_json::json!({})));
    assert!(answered["answer"]["at"].is_number() && answered["answer"]["r"].is_number(), "real numbers, not nothing: {answered}");
    let r = answered["answer"]["r"].as_f64().unwrap();
    assert!((0.0..1.0).contains(&r), "from 0 up to 1: {r}");
    // The run's random bytes, 32, the number read from their first 53 bits.
    let bytes = holon_core::text_bytes(answered["answer"]["bytes"].as_str().unwrap(), "base64").unwrap();
    assert_eq!(bytes.len(), 32, "{answered}");
    assert_eq!(r, (u64::from_be_bytes(bytes[..8].try_into().unwrap()) >> 11) as f64 / (1u64 << 53) as f64);
}

// ── per-deployment state ─────────────────────────────────────────────────────────────

/// What one run writes is there for the next: the write is in the run's log entry and in the
/// state, and a second run's write replaces it in the same slot.
#[test]
fn trigger_with_state_persists_across_calls() {
    let state = test_state();
    let writer = stage_writer(&state, "counter");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"counter\")"] });
    assert_eq!(deploy_json(&state, "writer", writer, ack).status_code, 200);

    // First run: writes "hello".
    assert_eq!(put(&state, "writer", "hello").status_code, 200);
    let entry = &log_entries(&state, "writer")[0];
    assert_eq!(entry["performed"][0]["capability"], "StateWrite(\"counter\")");
    assert_eq!(entry["performed"][0]["payload"]["value"], "hello");

    let s1 = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    assert_eq!(s1["state"]["counter"]["value"], "hello");

    // Second run, a real *different* call: overwrites — proving this is the same state slot
    // persisting across two separate runs, not something reset or freshly allocated each time.
    assert_eq!(put(&state, "writer", "world").status_code, 200);
    let s2 = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    assert_eq!(s2["state"]["counter"]["value"], "world");
}

/// Regression: `/deploy` on an existing slug used to install a fresh, empty state map, so
/// shipping a fix silently erased everything the previous version had written — and a fire
/// in flight at the time committed into the orphaned old map instead.
#[test]
fn redeploy_keeps_state() {
    let state = test_state();
    let deploy = |unit: Hash, acknowledge: serde_json::Value| {
        let body = serde_json::to_vec(&serde_json::json!({
            "slug": "writer",
            "unit": to_hex(&unit),
            "acknowledge": acknowledge,
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
    };
    let state_arc = || state.deployments.lock().unwrap()["writer"].state.clone().unwrap();

    let v1 = stage_writer(&state, "counter");
    deploy(v1, serde_json::json!(["Action(\"put\")", "StateWrite(\"counter\")"]));
    assert_eq!(put(&state, "writer", "hello").status_code, 200);
    let before = state_arc();

    // Same unit redeployed: state survives, and it is the very same map, not a copy.
    deploy(v1, serde_json::json!(["Action(\"put\")", "StateWrite(\"counter\")"]));
    assert!(Arc::ptr_eq(&before, &state_arc()));
    let s = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    assert_eq!(s["state"]["counter"]["value"], "hello");

    // A different stateful version writing another key: the old key is still there.
    let v2 = stage_writer(&state, "other");
    deploy(v2, serde_json::json!(["Action(\"put\")", "StateWrite(\"other\")"]));
    assert_eq!(put(&state, "writer", "world").status_code, 200);
    let s = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    assert_eq!(s["state"]["counter"]["value"], "hello");
    assert_eq!(s["state"]["other"]["value"], "world");

    // A stateless version in between doesn't discard it either.
    let stateless = stage_text(&state, ECHO);
    deploy(stateless, serde_json::json!(["Action(\"echo\")"]));
    deploy(v1, serde_json::json!(["Action(\"put\")", "StateWrite(\"counter\")"]));
    let s = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    assert_eq!(s["state"]["counter"]["value"], "hello");
    assert!(Arc::ptr_eq(&before, &state_arc()));
}

// ── data plane: a server declaration answered on its own port ─────────────────────────

fn stage(state: &AppState, unit: &Unit) -> Hash {
    assert_eq!(route(state, &req("POST", "/unit", encode_preimage(unit).unwrap())).status_code, 200);
    hash_unit(unit).unwrap()
}

fn leaf(name: &str, params: Vec<Param>, inputs: Vec<(&str, Type)>, out: Type) -> Unit {
    Unit {
        body: UnitBody::Primitive { name: name.into() },
        params,
        inputs: inputs.into_iter().map(|(n, ty)| Port { name: n.into(), ty }).collect(),
        outputs: vec![Port { name: "out".into(), ty: out }],
    }
}

/// An endpoint wrapping `children` wired by `edges`: input 0 is the request, output 0 the
/// response, and `extra_outputs` more outputs carry effects.
fn endpoint(state: &AppState, method: &str, path: &str, children: Vec<Unit>, edges: Vec<Edge>, extra_outputs: usize) -> Hash {
    let children = children.iter().map(|u| stage(state, u)).collect();
    let mut outputs = vec![Port { name: "response".into(), ty: Type::Any }];
    for i in 0..extra_outputs {
        outputs.push(Port { name: format!("effect{i}"), ty: Type::Effect });
    }
    stage(
        state,
        &Unit {
            body: UnitBody::Composite { children, edges },
            params: vec![
                Param::frozen("declare", "endpoint"),
                Param::frozen("method", method),
                Param::frozen("path", path),
            ],
            inputs: vec![Port { name: "request".into(), ty: Type::Record }],
            outputs,
        },
    )
}

/// A todo API keyed by title: the stored `todos` is a record from title to item. GET /todos
/// answers the whole record (`{}` until the first write); POST /todos parses the body and
/// inserts it under its title; GET /todos/{title} looks one up, through the editor's respond
/// template (a 404 when the title isn't there); DELETE /todos/{title} removes it (answered
/// 204, the write being its only output).
fn todo_server(state: &AppState, port: &str) -> Hash {
    let todos = || leaf("default", vec![Param::frozen("type", "Json"), Param::frozen("value", "{}")], vec![("value", Type::Any)], Type::Any);
    let field = |name: &str| leaf("field", vec![Param::frozen("name", name)], vec![("record", Type::Record)], Type::Any);
    let keyed = |name: &str, extra: Vec<(&'static str, Type)>, out: Type| {
        let mut inputs = vec![("record", Type::Record), ("key", Type::Text)];
        inputs.extend(extra);
        leaf(name, vec![], inputs, out)
    };
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let list = endpoint(state, "GET", "/todos", vec![test_store_read("todos"), todos()], vec![e(0, 0, 1, 0), e(1, 0, BOUNDARY, 0)], 0);
    let create = endpoint(
        state,
        "POST",
        "/todos",
        vec![
            field("body"),
            leaf("json_parse", vec![], vec![("text", Type::Text)], Type::Any),
            field("title"),
            test_store_read("todos"),
            todos(),
            keyed("insert", vec![("value", Type::Any)], Type::Record),
            test_store_write("todos"),
        ],
        vec![
            e(BOUNDARY, 0, 0, 0), // request → field(body)
            e(0, 0, 1, 0),        // body → json_parse
            e(1, 0, 2, 0),        // item → field(title)
            e(3, 0, 4, 0),        // store_read → default({})
            e(4, 0, 5, 0),        // todos → insert
            e(2, 0, 5, 1),        // title → insert's key
            e(1, 0, 5, 2),        // item → insert's value
            e(5, 0, 6, 0),        // inserted → store_write
            e(1, 0, BOUNDARY, 0), // response: the item
            e(6, 0, BOUNDARY, 1), // effect: the write
        ],
        1,
    );
    let one = endpoint(
        state,
        "GET",
        "/todos/{title}",
        vec![
            field("params"),
            field("title"),
            test_store_read("todos"),
            todos(),
            keyed("lookup", vec![], Type::Any),
            // The respond template (holon-author's templates.rs) at status 200, missing 404.
            leaf("record", vec![], vec![], Type::Record),
            leaf("const", vec![Param::frozen("value", "200"), Param::frozen("type", "Number")], vec![], Type::Number),
            leaf("with_field", vec![Param::frozen("name", "status")], vec![("record", Type::Record), ("value", Type::Any)], Type::Record),
            leaf("with_field", vec![Param::frozen("name", "body")], vec![("record", Type::Record), ("value", Type::Any)], Type::Record),
            leaf(
                "default",
                vec![Param::frozen("value", r#"{"status":404,"body":{"error":"not found"}}"#), Param::frozen("type", "Json")],
                vec![("value", Type::Any)],
                Type::Any,
            ),
        ],
        vec![
            e(BOUNDARY, 0, 0, 0), // request → field(params)
            e(0, 0, 1, 0),        // params → field(title)
            e(2, 0, 3, 0),        // store_read → default({})
            e(3, 0, 4, 0),        // todos → lookup
            e(1, 0, 4, 1),        // title → lookup's key
            e(5, 0, 7, 0),        // {} → with_field(status)
            e(6, 0, 7, 1),        // 200 → its value
            e(7, 0, 8, 0),        // {status} → with_field(body)
            e(4, 0, 8, 1),        // the item → body; absent when the title isn't there
            e(8, 0, 9, 0),        // the envelope → default: passed through, or the 404
            e(9, 0, BOUNDARY, 0), // response
        ],
        0,
    );
    let delete = endpoint(
        state,
        "DELETE",
        "/todos/{title}",
        vec![
            field("params"),
            field("title"),
            test_store_read("todos"),
            todos(),
            keyed("remove", vec![], Type::Record),
            test_store_write("todos"),
        ],
        vec![
            e(BOUNDARY, 0, 0, 0),
            e(0, 0, 1, 0),
            e(2, 0, 3, 0),
            e(3, 0, 4, 0),
            e(1, 0, 4, 1),
            e(4, 0, 5, 0),
            e(5, 0, BOUNDARY, 0), // the write is the response: 204
        ],
        0,
    );
    stage(
        state,
        &Unit {
            body: UnitBody::Composite { children: vec![list, create, one, delete], edges: vec![] },
            params: vec![Param::frozen("declare", "server"), Param::frozen("port", port)],
            inputs: vec![],
            outputs: vec![],
        },
    )
}

/// An items API over one collection: PUT /items/:id stores the body's JSON under the id,
/// GET /items/:id reads it back, GET /items lists two at a time (from after `?after=`), and
/// DELETE /items/:id removes it. Items persist between runs and across a reload, the collection's
/// root is a cell of the state, and each run's reads are in its log entry.
#[test]
fn a_collection_keeps_items_by_key_across_runs_and_restarts() {
    let mut state = test_state();
    let path = unique_temp_path("collection");
    state.deployments_path = path.clone().into();
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let iface = |name: &str, params: Vec<Param>| Unit { params, ..holon_core::interface_of(name).unwrap() };
    let frozen = |k: &str, v: &str| Param::frozen(k, v);
    let items = || frozen("collection", "items");
    let id = || vec![iface("field", vec![frozen("name", "params")]), iface("field", vec![frozen("name", "id")])];

    let put = endpoint(
        &state,
        "PUT",
        "/items/:id",
        [id(), vec![iface("field", vec![frozen("name", "body")]), iface("json_parse", vec![]), iface("collection_put", vec![items()])]].concat(),
        vec![e(BOUNDARY, 0, 0, 0), e(0, 0, 1, 0), e(BOUNDARY, 0, 2, 0), e(2, 0, 3, 0), e(1, 0, 4, 0), e(3, 0, 4, 1), e(3, 0, BOUNDARY, 0), e(4, 0, BOUNDARY, 1)],
        1,
    );
    let get = endpoint(
        &state,
        "GET",
        "/items/:id",
        [id(), vec![iface("collection_get", vec![items()])]].concat(),
        vec![e(BOUNDARY, 0, 0, 0), e(0, 0, 1, 0), e(1, 0, 2, 0), e(2, 0, BOUNDARY, 0)],
        0,
    );
    let list = endpoint(
        &state,
        "GET",
        "/items",
        vec![
            iface("field", vec![frozen("name", "query")]),
            iface("field", vec![frozen("name", "after")]),
            iface("default", vec![frozen("value", ""), frozen("type", "Text")]),
            iface("collection_list", vec![items(), frozen("limit", "2")]),
        ],
        vec![e(BOUNDARY, 0, 0, 0), e(0, 0, 1, 0), e(1, 0, 2, 0), e(2, 0, 3, 1), e(3, 0, BOUNDARY, 0)],
        0,
    );
    let gone = endpoint(
        &state,
        "DELETE",
        "/items/:id",
        [id(), vec![iface("collection_delete", vec![items()]), iface("const", vec![frozen("value", "deleted"), frozen("type", "Text")])]].concat(),
        vec![e(BOUNDARY, 0, 0, 0), e(0, 0, 1, 0), e(1, 0, 2, 0), e(3, 0, BOUNDARY, 0), e(2, 0, BOUNDARY, 1)],
        1,
    );
    let server = stage(
        &state,
        &Unit {
            body: UnitBody::Composite { children: vec![put, get, list, gone], edges: vec![] },
            params: vec![frozen("declare", "server"), frozen("port", "18093")],
            inputs: vec![],
            outputs: vec![],
        },
    );

    // Deploying asks to read and write the collection by name.
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "shelf", "unit": to_hex(&server) })).unwrap())));
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap();
    assert_eq!(asked, ["CollectionRead(\"items\")", "CollectionWrite(\"items\")", "NetworkIngress"]);
    assert_eq!(deploy_json(&state, "shelf", server, serde_json::json!({ "acknowledge": asked })).status_code, 200);

    let call = |state: &AppState, method: &str, url: &str, body: &str| {
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, "shelf", &fake_data(method, url, body.as_bytes().to_vec()));
        (r.status, String::from_utf8(r.body).unwrap())
    };
    for n in 1..=3 {
        assert_eq!(call(&state, "PUT", &format!("/items/{n}"), &format!("{{\"n\":{n}}}")).0, 200);
    }
    assert_eq!(call(&state, "GET", "/items/2", ""), (200, r#"{"n":2}"#.to_string()));
    assert_eq!(call(&state, "GET", "/items", ""), (200, r#"{"1":{"n":1},"2":{"n":2}}"#.to_string()), "a page of two");
    assert_eq!(call(&state, "GET", "/items?after=2", ""), (200, r#"{"3":{"n":3}}"#.to_string()), "the next page");
    assert_eq!(call(&state, "DELETE", "/items/2", "").0, 200);
    assert_ne!(call(&state, "GET", "/items/2", "").0, 200, "gone");
    assert_eq!(call(&state, "GET", "/items", ""), (200, r#"{"1":{"n":1},"3":{"n":3}}"#.to_string()));

    // Its root is a state cell; a run's reads are in its log entry.
    let shown = body_json(route(&state, &req("GET", "/deployments/shelf/state", vec![])));
    assert!(shown["state"]["#collection:items"]["value"].as_str().unwrap().len() == 64, "{shown}");
    let log = body_json(route(&state, &req("GET", "/deployments/shelf/log", vec![])));
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["reads"][0]["request"]["value"][1][0], "list", "{last}");

    // The editor reads a collection's items, live or as a logged run found it.
    let items = |q: &str| route(&state, &req("GET", &format!("/deployments/shelf/collections/items{q}"), vec![]));
    let keys = |v: &serde_json::Value| v["items"].as_array().unwrap().iter().map(|kv| kv[0].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(keys(&body_json(items(""))), ["1", "3"]);
    let first = body_json(items("?limit=1"));
    assert_eq!((keys(&first), first["next"].as_str()), (vec!["1".to_string()], Some("1")));
    assert_eq!(body_json(items("?key=3"))["items"][0][1]["value"][0][1]["value"], 3.0);
    // Before the delete, item 2 was there: a run's state before has that root.
    let entries = log["entries"].as_array().unwrap();
    let before_delete = entries.iter().find(|x| x["route"] == "DELETE /items/:id").unwrap()["state_before"]["#collection:items"]["value"].as_str().unwrap().to_string();
    assert_eq!(keys(&body_json(items(&format!("?root={before_delete}")))), ["1", "2", "3"]);
    assert_eq!(items(&format!("?root={}", "ab".repeat(32))).status_code, 404, "a root the log never showed");
    assert_eq!(route(&state, &req("GET", "/deployments/shelf/collections/nothing", vec![])).status_code, 404);

    // Persisted: reloaded from disk, the items are there.
    *state.deployments.lock().unwrap() = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(call(&state, "GET", "/items/3", ""), (200, r#"{"n":3}"#.to_string()));
    let _ = std::fs::remove_file(&path);
}

/// Runs of one deployment no longer queue on its state: they run side by side and commit only
/// if what they read is unchanged, running again otherwise. Many at once lose no update.
#[test]
fn concurrent_runs_lose_no_update() {
    let state = test_state();
    let unit = stage_text(&state, COUNTER);
    let ack = serde_json::json!(["Action(\"bump\")", "StateRead(\"count\")", "StateWrite(\"count\")"]);
    assert_eq!(deploy_json(&state, "c", unit, serde_json::json!({ "acknowledge": ack })).status_code, 200);
    std::thread::scope(|s| {
        for _ in 0..24 {
            s.spawn(|| assert_eq!(run_action_named(&state, "c", "bump", serde_json::json!({})).status_code, 200));
        }
    });
    let shown = body_json(route(&state, &req("GET", "/deployments/c/state", vec![])));
    assert_eq!(shown["state"]["count"]["value"], 24.0, "{shown}");
}

/// A run waiting on a slow outbound call no longer holds up the next: the second finishes while
/// the first still waits. The first then finds the counter it read has changed, and runs again
/// from the new state; the call it already made is answered from its record, not sent again.
#[test]
fn a_slow_run_holds_up_no_other_and_retries_without_resending() {
    let (host, hits, gate) = gated_counting_server();
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true").unwrap());
    // Each run counts, and calls the path it was sent; the call's answer is its own.
    let text = format!("import {{ action, flow }} from \"holon\"\nimport state from \"state\"\nimport api from \"host:{host}\"\n\nflow(\"slow\")\n\nexport const bump = action(async (path) => {{\n  state.count = (state.count ?? 0) + 1\n  const r = await api.get(path)\n  return r.body\n}})\n");
    let unit = stage_text(&state, &text);
    let ack = serde_json::json!([
        "Action(\"bump\")", "StateRead(\"count\")", "StateWrite(\"count\")",
        format!("HttpRequest({host:?})"), format!("PlainHttp({host:?})"), "PrivateAddresses",
    ]);
    let requirements = serde_json::json!({ "plain_http": [host], "private_addresses": true });
    assert_eq!(deploy_json(&state, "slow", unit, serde_json::json!({ "acknowledge": ack, "requirements": requirements })).status_code, 200);

    gate.shut();
    std::thread::scope(|s| {
        let first = s.spawn(|| run_action_named(&state, "slow", "bump", serde_json::json!("/slow")).status_code);
        let out = gate.out_at_once(1);
        let second = out.then(|| run_action_named(&state, "slow", "bump", serde_json::json!("/fast")).status_code);
        let held = gate.out() == 1;
        gate.open();
        assert!(out, "the slow call went out");
        assert_eq!(second, Some(200));
        assert!(held, "the second run finished while the first's call was still out, not held up by it");
        assert_eq!(first.join().unwrap(), 200);
    });
    let shown = body_json(route(&state, &req("GET", "/deployments/slow/state", vec![])));
    assert_eq!(shown["state"]["count"]["value"], 2.0, "both counted: {shown}");
    assert_eq!((hits_of(&hits, "/slow"), hits_of(&hits, "/fast")), (1, 1), "the slow call went out once");
    let log = body_json(route(&state, &req("GET", "/deployments/slow/log", vec![])));
    let retried: Vec<u64> = log["entries"].as_array().unwrap().iter().map(|x| x["retries"].as_u64().unwrap_or(0)).collect();
    assert_eq!(retried.iter().sum::<u64>(), 1, "the slow run ran once more: {retried:?}");
    // The run that ran again checks offline: the call its first try made is in its entry.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/slow/export", vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert!(report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
}

/// A port nothing listens on just now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// One request read whole from `stream`, its head as sent and its body (by its Content-Length),
/// for a server of the tests to read before it answers: a client may send its head and body in two
/// writes, and a socket closed with the body unread is reset (on Windows), which can drop the
/// answer before the client reads it. None: the connection ended before its head did.
fn read_request(stream: &mut std::net::TcpStream) -> Option<(String, Vec<u8>)> {
    use std::io::Read;
    let mut got = Vec::new();
    let mut buf = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => got.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&got[..head_end]).into_owned();
    let length: usize = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
    while got.len() < head_end + length {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
        }
    }
    Some((head, got.split_off(head_end)))
}

/// One raw HTTP/1.1 exchange on `port`: everything the server sends back until it closes.
fn raw_http(port: u16, request: &[u8]) -> String {
    use std::io::Write;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(request).unwrap();
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// The data plane on a server deployment's own port, through a real socket: it answers, refuses
/// a body over its limit before reading it, closes a connection whose headers don't arrive in
/// time, and lets go of the port once the deployment stops.
#[test]
fn a_server_deployment_answers_on_its_own_port_within_limits() {
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    state.data_limits.header_timeout = Duration::from_millis(300);
    state.data_limits.max_body_bytes = 1024;
    let port = free_port();
    let api = todo_server(&state, &port.to_string());
    assert_eq!(deploy_server(&state, "todo", api).status_code, 200);
    sync_listeners(&state);

    let body = r#"{"title":"buy milk"}"#;
    let posted = raw_http(port, format!("POST /todos HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
    assert!(posted.starts_with("HTTP/1.1 200"), "{posted}");
    let listed = raw_http(port, b"GET /todos HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    assert!(listed.starts_with("HTTP/1.1 200") && listed.contains("buy milk"), "{listed}");
    assert!(raw_http(port, b"GET /nope HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").starts_with("HTTP/1.1 404"));

    // Asked by a name it wasn't given (a page's, rebound to this machine): refused, saying how to
    // let a proxy's name in; by an address, localhost or a listed name, answered.
    let named = raw_http(port, b"GET /todos HTTP/1.1\r\nHost: evil.example:80\r\nConnection: close\r\n\r\n");
    assert!(named.starts_with("HTTP/1.1 421") && named.contains("--allow-host evil.example"), "{named}");
    for host in ["localhost", &format!("localhost:{port}"), "[::1]", "app.localhost"] {
        let answered = raw_http(port, format!("GET /todos HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes());
        assert!(answered.starts_with("HTTP/1.1 200"), "{host}: {answered}");
    }

    // Over the body limit: refused on its Content-Length, nothing read.
    let large = raw_http(port, b"POST /todos HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n");
    assert!(large.starts_with("HTTP/1.1 413"), "{large}");

    // Headers that never finish: the connection is closed, not held.
    let started = std::time::Instant::now();
    let slow = raw_http(port, b"GET /todos HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    assert!(started.elapsed() < Duration::from_secs(5), "closed after the header timeout: {slow:?}");
    assert!(!slow.contains("200"), "{slow}");

    // Stopped: the port is released.
    assert_eq!(route(&state, &req("POST", "/deployments/todo/stop", vec![])).status_code, 200);
    let released = (0..50).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
    });
    assert!(released, "the port is free again");
}

/// A flow's WebSockets through a real socket (host-capabilities L4 §5.12.14): a GET there without
/// the upgrade is a 426, a page from another site a 403, an open block's refusal its own status;
/// one accepted runs `open` (the first message its answer, or a subscription), each message
/// `message` (its answer sent back, its packs to every subscriber) and `close` once, with the
/// page's code or the host's: a binary message 1003, one too big 1009, too many a second 1008, a
/// message where there is no message block 1003, a pause 1001. Each run is logged, `open` with the
/// handshake's status, and `holon verify` runs each again.
#[test]
fn a_websocket_runs_its_blocks_through_a_real_socket() {
    use std::net::TcpStream;
    use tungstenite::client::IntoClientRequest;
    use tungstenite::{Message, WebSocket};
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let port = free_port();
    let text = format!(
        "import {{ flow, route, status }} from \"holon\"\nimport people from \"collection:people\"\nimport room from \"channel:room\"\n\nflow(\"chat\", {{ port: {port} }})\n\nroute.websocket(\"/room/:name\", {{\n  open: (req) => {{\n    people[req.conn] = req.params.name\n    room.send({{ joined: req.params.name }})\n    return room.subscribe()\n  }},\n  message: (m) => {{\n    room.send({{ from: people[m.conn], text: m.text }})\n    return {{ echo: m.text }}\n  }},\n  close: (c) => {{\n    delete people[c.conn]\n    room.send({{ left: c.params.name, code: c.code }})\n  }},\n}}, {{ log: {{ message: \"lazy\" }} }})\n\nroute.websocket(\"/closed\", {{ open: () => status(403, {{ error: \"closed\" }}) }})\n\nroute.websocket(\"/hello\", {{ open: (req) => `hello ${{req.query.who ?? \"you\"}}` }})\n"
    );
    let slug = deploy_plain_text(&state, &text);
    sync_listeners(&state);
    let sockets = Arc::clone(&state.deployments.lock().unwrap()[&slug].sockets);
    let open = |path: &str, origin: Option<&str>| -> Result<WebSocket<TcpStream>, u16> {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut request = format!("ws://127.0.0.1:{port}{path}").into_client_request().unwrap();
        if let Some(origin) = origin {
            request.headers_mut().insert("origin", origin.parse().unwrap());
        }
        match tungstenite::client(request, stream) {
            Ok((ws, _)) => Ok(ws),
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(r))) => Err(r.status().as_u16()),
            Err(e) => panic!("{path}: {e}"),
        }
    };
    let said = |ws: &mut WebSocket<TcpStream>| loop {
        match ws.read().unwrap() {
            Message::Text(t) => break serde_json::from_str::<serde_json::Value>(&t).unwrap_or_else(|_| serde_json::Value::String(t.to_string())),
            Message::Close(c) => panic!("closed: {c:?}"),
            _ => {}
        }
    };
    // The code the server closed it with, its close answered as a browser answers it.
    let closed_with = |ws: &mut WebSocket<TcpStream>| {
        let code = loop {
            match ws.read() {
                Ok(Message::Close(c)) => break c.map(|c| u16::from(c.code)),
                Ok(_) => {}
                Err(e) => panic!("{e}"),
            }
        };
        while ws.read().is_ok() {}
        code
    };

    // Not a WebSocket's opening request, a page from elsewhere, a refusal: nothing opens.
    let plain = raw_http(port, b"GET /room/ann HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    assert!(plain.starts_with("HTTP/1.1 426") && plain.to_ascii_lowercase().contains("upgrade: websocket"), "{plain}");
    assert_eq!(open("/hello", Some("http://evil.example")).err(), Some(403));
    assert_eq!(open("/closed", None).err(), Some(403));

    // The page's own origin: open's answer is the first message; with no message block, a
    // message closes it.
    let mut hello = open("/hello?who=ann", Some(&format!("http://127.0.0.1:{port}"))).unwrap();
    assert_eq!(said(&mut hello), serde_json::json!("hello ann"));
    hello.send(Message::text("hi")).unwrap();
    assert_eq!(closed_with(&mut hello), Some(1003));

    // Two in a room: each message runs, its answer back to its sender, its pack to both once its
    // record is where its level puts it (lazy: the next sync).
    let mut a = open("/room/ann", None).unwrap();
    let mut b = open("/room/bob", None).unwrap();
    assert_eq!(said(&mut a), serde_json::json!({ "joined": "bob" }));
    b.send(Message::text("hi")).unwrap();
    assert_eq!(said(&mut b), serde_json::json!({ "echo": "hi" }));
    crate::recorder::sync_logs(&state.deployments).unwrap();
    let pack = serde_json::json!({ "from": "bob", "text": "hi" });
    assert_eq!((said(&mut a), said(&mut b)), (pack.clone(), pack));

    // A binary message closes its connection (1003), and close runs with the host's code.
    b.send(Message::binary(vec![1u8, 2, 3])).unwrap();
    assert_eq!(closed_with(&mut b), Some(1003));
    assert_eq!(said(&mut a), serde_json::json!({ "left": "bob", "code": 1003 }));
    // Past max_message_bytes (64 KiB): 1009.
    a.send(Message::text("x".repeat(70 << 10))).unwrap();
    assert_eq!(closed_with(&mut a), Some(1009));
    // More than messages_per_second (10, bursts of 20): 1008.
    let mut c = open("/room/cy", None).unwrap();
    for n in 0..30 {
        c.send(Message::text(format!("{n}"))).unwrap();
    }
    assert_eq!(closed_with(&mut c), Some(1008));
    eventually("each connection's messages and its close ran", || sockets.count() == 0);
    // Paused, its connections close (1001), and their close runs all the same.
    let mut d = open("/room/dee", None).unwrap();
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/pause"), vec![])).status_code, 200);
    assert_eq!(closed_with(&mut d), Some(1001));

    eventually("dee's close ran", || sockets.count() == 0);
    let entries = log_entries(&state, &slug);
    // Each run's entry, with its record as plain JSON.
    let runs = |label: &str| {
        let plain = |v: &serde_json::Value| serde_json::from_str::<serde_json::Value>(&holon_core::stringify_json(&serde_json::from_value::<SValue>(v.clone()).unwrap().into_core()).unwrap()).unwrap();
        let ran = entries.iter().filter(|e| e["kind"] == "websocket" && e["route"] == label);
        ran.map(|e| serde_json::json!({ "status": e["status"], "request": plain(&e["request"]) })).collect::<Vec<_>>()
    };
    let opened: Vec<_> = runs("WebSocket /room/:name open").iter().map(|e| (e["request"]["params"]["name"].clone(), e["status"].clone())).collect();
    assert_eq!(opened.len(), 4, "{opened:?}");
    assert!(opened.iter().all(|(_, status)| *status == 101), "{opened:?}");
    assert_eq!(runs("WebSocket /closed open")[0]["status"], 403);
    let mut ended: Vec<(String, u64)> = runs("WebSocket /room/:name close")
        .iter()
        .map(|e| (e["request"]["params"]["name"].as_str().unwrap().to_string(), e["request"]["code"].as_u64().unwrap()))
        .collect();
    ended.sort();
    assert_eq!(ended, [("ann".into(), 1009), ("bob".into(), 1003), ("cy".into(), 1008), ("dee".into(), 1001)]);
    let messages = runs("WebSocket /room/:name message");
    assert!(messages.iter().any(|e| e["request"]["text"] == "hi" && e["request"]["conn"].as_str().is_some_and(|c| c.len() == 16)), "{messages:?}");
    // The ones that came before the close ran: the burst's.
    assert!(messages.iter().filter(|e| e["request"]["params"]["name"] == "cy").count() >= 20, "{messages:?}");
    let people = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/collections/people"), vec![])));
    assert_eq!(people["items"].as_array().map(Vec::len), Some(0), "who left was let go: {people}");

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert!(report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    assert!(replays_as_it_verifies(&state, &slug).contains("websocket"));
}

/// A connection its open block accepts but the host can't hold (its channel has all the
/// subscribers it may) is refused, 503, and its close runs as a dropped connection's does (1006):
/// what open kept of it is let go.
#[test]
fn a_connection_accepted_but_not_held_runs_its_close() {
    use std::net::TcpStream;
    use tungstenite::client::IntoClientRequest;
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let port = free_port();
    let text = format!(
        "import {{ flow, route }} from \"holon\"
import people from \"collection:people\"
import gone from \"collection:gone\"
import room from \"channel:room\"

flow(\"full\", {{ port: {port} }})

route.websocket(\"/room/:name\", {{
  open: (req) => {{
    people[req.conn] = req.params.name
    return room.subscribe()
  }},
  close: (c) => {{
    delete people[c.conn]
    gone[c.params.name] = c.code
  }},
}})
"
    );
    // Its channel holds one subscriber: the deploy asks for it, as the flow's `limits` would.
    let plan = holon_author::agent::plan(&text).unwrap_or_else(|e| panic!("{e}"));
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let one = serde_json::json!({ "limits": { "max_subscribers": 1 } });
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit, "requirements": one })).unwrap())));
    let asked = preview["needs_approval"].clone();
    let deployed = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": asked, "document": plan.document, "requirements": one }));
    let status = deployed.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(deployed)));
    let slug = plan.name;
    sync_listeners(&state);
    let open = |path: &str| {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        match tungstenite::client(format!("ws://127.0.0.1:{port}{path}").into_client_request().unwrap(), stream) {
            Ok((ws, _)) => Ok(ws),
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(r))) => Err(r.status().as_u16()),
            Err(e) => panic!("{path}: {e}"),
        }
    };
    let _ann = open("/room/ann").unwrap();
    assert_eq!(open("/room/bob").err(), Some(503));
    let items = |name: &str| body_json(route(&state, &req("GET", &format!("/deployments/{slug}/collections/{name}"), vec![])))["items"].clone();
    let people = items("people");
    assert_eq!(people.as_array().map(Vec::len), Some(1), "only ann is held: {people}");
    let gone = items("gone");
    assert!(gone.to_string().contains("bob") && gone.to_string().contains("1006"), "bob's close ran, as a dropped connection's: {gone}");
}

/// A route whose runs can never wait (unrecorded and granted, reading only its request, the clock
/// and random numbers, its flow in memory) is answered on the data plane's own worker, without the
/// hop onto the blocking pool (docs/L4s/request-path-L4.md §4.5 a): through a real socket it
/// answers as any route does, CORS and all. Any other keeps the hop, since a worker that waits holds
/// up every connection on it (docs/L4s/resilience-audit-L4.md, R2): a lazy one (its log's lock), one
/// reading a collection (the store's lock while a write syncs) or a folder, a strict one, one that
/// calls out; and a light one too until its flow is loaded.
#[test]
fn a_light_route_answers_on_the_data_planes_worker_as_any_route_does() {
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let port = free_port();
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport notes from \"collection:notes\"\nimport docs from \"folder:docs\"\nimport httpbin from \"host:httpbin.org\"\n\nflow(\"light\", {{ port: {port}, browsers: \"https://a.example\" }})\n\nroute.get(\"/health\", () => \"ok\", {{ log: \"unrecorded\" }})\n\nroute.get(\"/notes\", () => notes.list({{ limit: 50 }}), {{ log: \"lazy\" }})\n\nroute.post(\"/notes\", (req) => {{\n  notes[req.json.id] = req.json.text\n  return \"saved\"\n}})\n\nroute.get(\"/uuid\", async () => (await httpbin.get(\"/uuid\")).body)\n\nroute.get(\"/file\", () => docs.get(\"a.txt\"), {{ log: \"lazy\" }})\n"
    );
    let dir = std::env::temp_dir().join(format!("holon-light-docs-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let docs = serde_json::json!({ "name": "docs", "path": dir.display().to_string(), "access": "read" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&docs).unwrap())).status_code, 200);
    let slug = deploy_plain_text(&state, &text);
    // The pages it answers are a deploy setting, which this deploy leaves out.
    state.deployments.lock().unwrap().get_mut(&slug).unwrap().browsers = vec!["https://a.example".to_string()];
    sync_listeners(&state);
    let here = Mutex::new(dataplane::Here::default());
    let light = |method: &str, url: &str| dataplane::runs_here_for(&state.store, &state.deployments, &slug, &fake_data(method, url, vec![]), &here);
    // Its flow let go of by the flow cache: the pool, which loads it.
    state.store.3.set_budget(0);
    assert!(!light("GET", "/health"), "its flow not in memory: the pool");
    state.store.3.set_budget(crate::units::FLOW_CACHE_BYTES);
    let unit = state.deployments.lock().unwrap()[&slug].unit;
    assert!(crate::units::closure(&state.store, unit).is_ok(), "loaded, as a run on the pool loads it");
    assert!(light("GET", "/health"), "unrecorded, reading only its request, loaded: here");
    assert!(light("GET", "/health"), "and again, from what it worked out");
    assert!(!light("GET", "/notes?x=1"), "lazy, reading a collection: the pool");
    assert!(!light("GET", "/file"), "a folder may take as long as it likes: the pool");
    assert!(!light("POST", "/notes") && !light("GET", "/uuid") && !light("GET", "/nowhere"), "strict, calling out, or no route: the pool");

    let body = r#"{"id":"a","text":"hi"}"#;
    let posted = raw_http(port, format!("POST /notes HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
    assert!(posted.starts_with("HTTP/1.1 200"), "{posted}");
    let health = raw_http(port, b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    assert!(health.starts_with("HTTP/1.1 200") && health.ends_with("ok"), "{health}");
    let listed = raw_http(port, b"GET /notes HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://a.example\r\nConnection: close\r\n\r\n");
    assert!(listed.starts_with("HTTP/1.1 200") && listed.contains(r#"{"a":"hi"}"#), "{listed}");
    assert!(listed.to_ascii_lowercase().contains("access-control-allow-origin: https://a.example"), "CORS as for any route: {listed}");
    // Many at once, each answered.
    std::thread::scope(|s| {
        for _ in 0..16 {
            s.spawn(|| assert!(raw_http(port, b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").starts_with("HTTP/1.1 200")));
        }
    });
    assert_eq!(state.deployments.lock().unwrap()[&slug].tallies["GET /health"].runs, 17);
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![])).status_code, 200);
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Guards:** a panic ends the run it was in and nothing else, whatever started the run and
/// wherever in it the panic is (docs/engineering/themes/resilience.md, G3).
///
/// **Why:** a panic is a fault in Holon's own code, which a flow can't ask for and nobody plans.
/// What drives runs is shared: one thread ticks every deployment's clocks and the deadlines of
/// every waiting run, one watches every folder, a connection's worker answers its requests. A
/// panic that isn't caught where the run is ends whichever of those it was on.
///
/// **If it fails:** one faulty run stops every deployment's clocks and deadlines, or its folder's
/// runs, until the server is restarted, with nothing in any log; or a caller's connection is
/// dropped with no answer.
///
/// The test asks for the panic (`panic_for_tests`): in a run itself, where every run passes
/// (`fire_with_state`), and in a request's handler before its run (answered where its connection
/// is, dataplane.rs). Each way a run starts is a row. Calm, it runs. Faulted, whatever drives it
/// comes back, a caller is told 500, and the deployment beside it runs all the same. Calm again,
/// it runs: nothing that drives runs was ended. A run that panicked is on record as one that
/// failed. The rows are every way a run starts that a test drives without a far side: a request
/// answered on its connection's worker and one on the pool, an action, a clock, a file, and a
/// waiting run going on at its deadline. A feed's, a queue's, a mailbox's and a socket's runs
/// pass the same function.
#[test]
fn a_panic_ends_its_run_and_nothing_else() {
    use crate::listeners::panic_for_tests as fault;
    use std::cell::Cell;
    let mut state = claims_state("");
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let dir = std::env::temp_dir().join(format!("holon-panics-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let folder = serde_json::json!({ "name": "inbox", "path": dir.display().to_string(), "access": "read-write" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&folder).unwrap())).status_code, 200);
    // Two deployments of one text: the one that faults, and one beside it.
    let text = |name: &str, port: u16| {
        format!(
            r#"import {{ action, event, every, flow, on, route }} from "holon"
import state from "state"
import inbox from "folder:inbox"

flow("{name}", {{ port: {port} }})

route.get("/{name}-light/:x", (req) => `light ${{req.params.x}}`, {{ log: "unrecorded" }})

route.get("/{name}-hit", () => `hit ${{++state.hits}}`)

route.get("/{name}-seen", () => `${{state.hits ?? 0}} ${{state.bumps ?? 0}} ${{state.ticks ?? 0}} ${{state.files ?? 0}} ${{state.later ?? 0}}`)

export const bump = action(() => ++state.bumps)

export const later = action(async () => {{
  const go = await event("go", {{ expires: "1h" }})
  state.later = (state.later ?? 0) + (go.expired ? 1 : 1)
}})

every("1s", () => {{
  ++state.ticks
}})

on.file.created(inbox, (f) => {{
  ++state.files
}})
"#
        )
    };
    let ports = [("faulty", free_port()), ("beside", free_port())];
    for (name, port) in ports {
        assert_eq!(deploy_plain_text(&state, &text(name, port)), name);
    }
    sync_listeners(&state);
    let get = |port: u16, path: &str| raw_http(port, format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes());
    let status = |answered: &str| answered.get(9..12).and_then(|s| s.parse::<u16>().ok()).unwrap_or_else(|| panic!("no status: {answered:?}"));
    // What a deployment has counted: its hits, bumps, ticks, files and waiting runs gone on.
    let seen = |name: &str| -> Vec<u64> { data_call(&state, name, "GET", &format!("/{name}-seen"), "").1.split(' ').map(|n| n.parse().unwrap()).collect() };

    // The passes that drive runs nobody asks for, each for every deployment at once.
    let passes = Cell::new(0u64);
    let tick = || {
        passes.set(passes.get() + 2);
        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, SystemTime::now() + Duration::from_secs(passes.get()));
    };
    let dropped = Cell::new(0u32);
    let drop_a_file = || {
        dropped.set(dropped.get() + 1);
        std::fs::write(dir.join(format!("f{}.txt", dropped.get())), "dropped").unwrap();
        // New, settling; then the same, and it runs.
        for _ in 0..2 {
            arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
        }
    };
    let deadline = || durable::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 4000);

    // A way a run starts: what it counts in `seen` (a light request counts nothing: its answer
    // says it ran), what its log entry is called, what to do calm beforehand, and how it starts,
    // answering what its caller was told when it has one. `all` starts every deployment's at once.
    struct Row<'a> {
        what: &'a str,
        counts: Option<usize>,
        logged: Option<&'a str>,
        before: Option<&'a dyn Fn(&str)>,
        start: &'a dyn Fn(&str, u16) -> Option<u16>,
        all: bool,
    }
    let light = |name: &str, port: u16| Some(status(&get(port, &format!("/{name}-light/x"))));
    let hit = |name: &str, port: u16| Some(status(&get(port, &format!("/{name}-hit"))));
    let bump = |name: &str, _: u16| Some(run_action_named(&state, name, "bump", serde_json::json!({})).status_code);
    let wait = |name: &str| assert_eq!(run_action_named(&state, name, "later", serde_json::json!({})).status_code, 202, "{name} waits");
    let ticked = |_: &str, _: u16| -> Option<u16> {
        tick();
        None
    };
    let filed = |_: &str, _: u16| -> Option<u16> {
        drop_a_file();
        None
    };
    let expired = |_: &str, _: u16| -> Option<u16> {
        deadline();
        None
    };
    let rows = [
        Row { what: "a request answered on its connection's worker", counts: None, logged: None, before: None, start: &light, all: false },
        Row { what: "a request answered on the pool", counts: Some(0), logged: Some("request"), before: None, start: &hit, all: false },
        Row { what: "an action", counts: Some(1), logged: Some("trigger"), before: None, start: &bump, all: false },
        Row { what: "a clock", counts: Some(2), logged: Some("tick"), before: None, start: &ticked, all: true },
        Row { what: "a file", counts: Some(3), logged: Some("file"), before: None, start: &filed, all: true },
        Row { what: "a waiting run at its deadline", counts: Some(4), logged: Some("resume"), before: Some(&wait), start: &expired, all: true },
    ];
    // Where the panic is: in the run, for every row; in the request's handler before its run, for
    // the requests.
    type Applies<'a> = &'a dyn Fn(&Row) -> bool;
    let (every_row, requests): (Applies, Applies) = (&|_| true, &|row| row.what.starts_with("a request"));
    let faults = [("in its run", vec!["a run of faulty"], every_row), ("in its request's handler", vec!["/faulty-light", "/faulty-hit"], requests)];

    // Loaded by a first run, so the light route is answered on the worker from here on.
    assert_eq!(light("faulty", ports[0].1), Some(200));
    let here = Mutex::new(dataplane::Here::default());
    assert!(dataplane::runs_here_for(&state.store, &state.deployments, "faulty", &fake_data("GET", "/faulty-light/x", vec![]), &here), "answered on the worker");

    let ran = |row: &Row, name: &str, was: &[u64], told: Option<u16>| match row.counts {
        Some(at) => seen(name)[at] > was[at],
        None => told == Some(200),
    };
    let mut failed_runs = 0;
    for (wherever, asked, applies) in &faults {
        for row in rows.iter().filter(|row| applies(row)) {
            let says = format!("{}, a panic {wherever}", row.what);
            let start = |faulted: bool| {
                // What comes before is calm: only the run that goes on from it is faulted.
                for (name, _) in ports {
                    if let Some(before) = row.before {
                        before(name);
                    }
                }
                let was: Vec<Vec<u64>> = ports.iter().map(|(name, _)| seen(name)).collect();
                if faulted {
                    asked.iter().for_each(|what| fault::on(what));
                }
                let told: Vec<Option<u16>> = match row.all {
                    true => vec![(row.start)("faulty", ports[0].1); 2],
                    false => ports.iter().map(|(name, port)| (row.start)(name, *port)).collect(),
                };
                asked.iter().for_each(|what| fault::off(what));
                (was, told)
            };
            let (was, told) = start(false);
            assert!(ran(row, "faulty", &was[0], told[0]), "{says}: calm, it runs");
            let (was, told) = start(true);
            assert!(!ran(row, "faulty", &was[0], told[0]), "{says}: faulted, it didn't run");
            if !row.all {
                assert_eq!(told[0], Some(500), "{says}: its caller is told");
            }
            assert!(ran(row, "beside", &was[1], told[1]), "{says}: the deployment beside it runs all the same");
            let (was, told) = start(false);
            assert!(ran(row, "faulty", &was[0], told[0]), "{says}: calm again, it runs");
            if *wherever == "in its run" && row.logged.is_some() {
                failed_runs += 1;
            }
        }
    }

    // A run that panicked is on record as one that failed, under what started it; a request whose
    // handler panicked before its run has none (G3's limit). Nothing of the deployment beside it
    // failed.
    let failed = |name: &str| -> Vec<String> {
        log_entries(&state, name).iter().filter(|e| e["error"].as_str().is_some_and(|e| e.starts_with(crate::firing::PANICKED))).map(|e| e["kind"].as_str().unwrap().to_string()).collect()
    };
    let mut expected: Vec<String> = rows.iter().filter_map(|row| row.logged.map(String::from)).collect();
    expected.sort();
    let mut logged = failed("faulty");
    logged.sort();
    assert_eq!(logged, expected, "each run that panicked is logged as failed");
    assert_eq!(logged.len(), failed_runs);
    assert!(log_entries(&state, "beside").iter().all(|e| e["error"].is_null()), "nothing of the deployment beside it failed");
    for (name, _) in ports {
        assert_eq!(route(&state, &req("POST", &format!("/deployments/{name}/stop"), vec![])).status_code, 200);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Guards:** a server told to stop starts no run, whichever way a run starts, and the run going
/// on when it was told goes on to its end and its log line (stopping.rs).
///
/// **Why:** the server exits once no run goes on. A run a trigger started after it was told would
/// keep the stop waiting, or be cut off at the exit half done: its calls made, its line never
/// written.
///
/// **If it fails:** a stop that never ends while a clock keeps starting runs, or a run cut off at
/// the exit with what it did on no record; or the run going on refused its own end.
///
/// Each way a run starts that a test drives without a far side is a row: a request answered on
/// its connection's worker and one on the pool, a WebSocket opened, an action over the control
/// plane (`admitted`), a clock, a file and a waiting run at its deadline. Before the stop each
/// runs; after it none does, and a caller is told 503. A feed's, a queue's, a mailbox's and a
/// socket message's runs ask `takes_runs` as the clock's and the file's do: the sweep at the end
/// holds every file that starts runs to asking.
#[test]
fn a_server_told_to_stop_starts_no_run_and_finishes_the_one_going_on() {
    use std::cell::Cell;
    use tungstenite::client::IntoClientRequest;
    let (host, hits, gate) = gated_counting_server();
    let mut state = claims_state("");
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let dir = std::env::temp_dir().join(format!("holon-stopping-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let folder = serde_json::json!({ "name": "inbox", "path": dir.display().to_string(), "access": "read-write" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&folder).unwrap())).status_code, 200);
    let port = free_port();
    let text = format!(
        r#"import {{ action, event, every, flow, on, route }} from "holon"
import state from "state"
import inbox from "folder:inbox"
import api from "host:{host}"

flow("stopping", {{ port: {port} }})

route.get("/light/:x", (req) => `light ${{req.params.x}}`, {{ log: "unrecorded" }})

route.get("/hit", () => `hit ${{++state.hits}}`)

route.websocket("/hello", {{ open: () => "hello" }})

export const bump = action(() => ++state.bumps)

export const later = action(async () => {{
  const go = await event("go", {{ expires: "1h" }})
  state.later = (state.later ?? 0) + (go.expired ? 1 : 1)
}})

export const slow = action(async () => {{
  const r = await api.get("/slow")
  state.slow = r.body
  return r.body
}})

every("1s", () => {{
  ++state.ticks
}})

on.file.created(inbox, (f) => {{
  ++state.files
}})
"#
    );
    assert_eq!(deploy_text_to(&state, &text, &[&host]), "stopping");
    sync_listeners(&state);
    let get = |path: &str| raw_http(port, format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes());
    let status = |answered: &str| answered.get(9..12).and_then(|s| s.parse::<u16>().ok()).unwrap_or_else(|| panic!("no status: {answered:?}"));
    let shown = |key: &str| body_json(route(&state, &req("GET", "/deployments/stopping/state", vec![])))["state"][key]["value"].clone();
    let waiting = || state.deployments.lock().unwrap()["stopping"].parked.len();

    // A way a run starts: the state key its run counts (none: its answer says it ran), and how it
    // starts, answering what its caller was told when it has one.
    struct Row<'a> {
        what: &'a str,
        counts: Option<&'a str>,
        start: &'a dyn Fn() -> Option<u16>,
    }
    let light = || Some(status(&get("/light/x")));
    let hit = || Some(status(&get("/hit")));
    let socket = || {
        let stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        match tungstenite::client(format!("ws://127.0.0.1:{port}/hello").into_client_request().unwrap(), stream) {
            Ok(_) => Some(101),
            Err(tungstenite::HandshakeError::Failure(tungstenite::Error::Http(r))) => Some(r.status().as_u16()),
            Err(e) => panic!("{e}"),
        }
    };
    let bump = || Some(admitted(&state, &req("POST", "/trigger/stopping/bump", b"{}".to_vec()), &std::sync::atomic::AtomicUsize::new(0), 1).status_code);
    let passes = Cell::new(0u64);
    let tick = || {
        passes.set(passes.get() + 2);
        run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, SystemTime::now() + Duration::from_secs(passes.get()));
        None
    };
    let dropped = Cell::new(0u32);
    let file = || {
        dropped.set(dropped.get() + 1);
        std::fs::write(dir.join(format!("f{}.txt", dropped.get())), "dropped").unwrap();
        // New, settling; then the same, and it runs.
        for _ in 0..2 {
            arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
        }
        None
    };
    let deadline = || {
        scheduler::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 4000);
        None
    };
    let rows = [
        Row { what: "a request answered on its connection's worker", counts: None, start: &light },
        Row { what: "a request answered on the pool", counts: Some("hits"), start: &hit },
        Row { what: "a WebSocket opened", counts: None, start: &socket },
        Row { what: "an action over the control plane", counts: Some("bumps"), start: &bump },
        Row { what: "a clock", counts: Some("ticks"), start: &tick },
        Row { what: "a file", counts: Some("files"), start: &file },
        Row { what: "a waiting run at its deadline", counts: Some("later"), start: &deadline },
    ];
    let count = |row: &Row| row.counts.map_or(0.0, |key| shown(key).as_f64().unwrap_or(0.0));
    let ran = |row: &Row, was: f64, told: Option<u16>| match row.counts {
        Some(_) => count(row) > was,
        None => told.is_some_and(|s| s < 300),
    };

    assert_eq!(run_action_named(&state, "stopping", "later", serde_json::json!({})).status_code, 202, "a run waits");
    for row in &rows {
        let was = count(row);
        let told = (row.start)();
        assert!(ran(row, was, told), "{}: it runs ({told:?})", row.what);
    }
    let here = Mutex::new(dataplane::Here::default());
    assert!(dataplane::runs_here_for(&state.store, &state.deployments, "stopping", &fake_data("GET", "/light/x", vec![]), &here), "answered on the worker");
    assert_eq!(run_action_named(&state, "stopping", "later", serde_json::json!({})).status_code, 202, "another waits");

    // A run going on, its call out, as the server is told to stop.
    gate.shut();
    std::thread::scope(|s| {
        let going_on = s.spawn(|| run_action_named(&state, "stopping", "slow", serde_json::json!({})));
        let out = gate.out_at_once(1);
        let logged = log_entries(&state, "stopping").len();
        state.store.1.stop();
        let refused: Vec<(&str, bool, Option<u16>)> = rows
            .iter()
            .map(|row| {
                let was = count(row);
                let told = (row.start)();
                (row.what, ran(row, was, told), told)
            })
            .collect();
        let (left_waiting, logged_after) = (waiting(), log_entries(&state, "stopping").len());
        gate.open();
        let answered = going_on.join().unwrap().status_code;
        assert!(out, "its call went out");
        for (what, ran, told) in refused {
            assert!(!ran, "{what}: told to stop, it doesn't run");
            assert!(told.is_none_or(|s| s == 503), "{what}: its caller is told to try again: {told:?}");
        }
        assert_eq!(left_waiting, 1, "the waiting run waits on");
        assert_eq!(logged_after, logged, "no run started");
        assert_eq!(answered, 200, "the run going on goes on to its end");
    });
    assert_eq!(shown("slow"), "slow");
    assert_eq!(hits_of(&hits, "/slow"), 1);
    let entries = log_entries(&state, "stopping");
    assert!(entries.iter().any(|e| e["kind"] == "trigger" && e["error"].is_null() && e.to_string().contains("/slow")), "its line is written");

    // Every file that starts runs asks first, but those whose runs start only where another asked.
    let asked_elsewhere = [
        // Where a run is fired.
        "firing.rs",
        // An action: over the control plane (`admitted`), or by a flow's call inside a counted run.
        "deploy.rs",
        // A waiting run going on: at a route (`serve_flow`), a decision (`admitted`) or its
        // deadline (`scheduler::expire_waiting`).
        "durable.rs",
    ];
    for entry in std::fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src")).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".rs") || name == "tests.rs" || asked_elsewhere.contains(&name.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap();
        if text.contains("fire_with_state(") {
            assert!(text.contains(".takes_runs(") || text.contains(".stopping()"), "{name} starts runs: it asks `Deployment::takes_runs` before each, as the others do");
        }
    }
    assert_eq!(route(&state, &req("POST", "/deployments/stopping/stop", vec![])).status_code, 200);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A server deployment whose port another program holds says so on its entry, rather than looking
/// like it answers; once the port is free, deploying again listens there and clears it.
#[test]
fn a_server_deployment_whose_port_is_taken_says_it_is_not_answering() {
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let port = free_port();
    let holder = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let api = todo_server(&state, &port.to_string());
    assert_eq!(deploy_server(&state, "todo", api).status_code, 200);

    let entry = |state: &AppState| {
        let listed = body_json(route(state, &req("GET", "/deployments", vec![])));
        listed["deployments"].as_array().unwrap().iter().find(|d| d["slug"] == "todo").unwrap().clone()
    };
    let taken = entry(&state);
    let why = taken["listen_error"].as_str().unwrap_or_else(|| panic!("no listen_error: {taken}"));
    assert!(why.contains(&format!("port {port}")), "{why}");

    drop(holder);
    assert_eq!(deploy_server(&state, "todo", api).status_code, 200);
    let freed = entry(&state);
    assert!(freed.get("listen_error").is_none(), "{freed}");
    let listed = raw_http(port, b"GET /todos HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    assert!(listed.starts_with("HTTP/1.1 200"), "{listed}");
    assert_eq!(route(&state, &req("POST", "/deployments/todo/stop", vec![])).status_code, 200);
}

fn deploy_server(state: &AppState, slug: &str, unit: Hash) -> Response {
    let body = serde_json::to_vec(&serde_json::json!({
        "slug": slug,
        "unit": to_hex(&unit),
        "acknowledge": ["NetworkIngress", "StateRead(\"todos\")", "StateWrite(\"todos\")"],
    }))
    .unwrap();
    route(state, &req("POST", "/deploy", body))
}

#[test]
fn a_server_deployment_answers_requests_and_keeps_state() {
    let mut state = test_state();
    let path = unique_temp_path("todo-api");
    state.deployments_path = path.clone().into();
    let api = todo_server(&state, "18080");
    let deployed = deploy_server(&state, "todo-api", api);
    let code = deployed.status_code;
    let shown = body_json(deployed);
    assert_eq!(code, 200, "{shown}");
    assert_eq!(shown["triggers"], serde_json::json!(["Http :18080"]));

    let call = |method: &str, url: &str, body: &str| {
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, "todo-api",
            &fake_data(method, url, body.as_bytes().to_vec()));
        (r.status, String::from_utf8(r.body).unwrap())
    };
    assert_eq!(call("GET", "/todos", ""), (200, "{}".to_string()));
    assert_eq!(call("POST", "/todos", r#"{"title":"buy milk"}"#), (200, r#"{"title":"buy milk"}"#.to_string()));
    assert_eq!(call("POST", "/todos", r#"{"title":"walk","done":true}"#).0, 200);
    assert_eq!(
        call("GET", "/todos?ignored=1", ""),
        (200, r#"{"buy milk":{"title":"buy milk"},"walk":{"done":true,"title":"walk"}}"#.to_string())
    );
    // The title in the path is the key, percent-decoded.
    assert_eq!(call("GET", "/todos/buy%20milk", ""), (200, r#"{"title":"buy milk"}"#.to_string()));
    assert_eq!(call("GET", "/todos/nope", ""), (404, r#"{"error":"not found"}"#.to_string()), "a missing title is a 404");
    assert_eq!(call("POST", "/todos", r#"{"title":"walk"}"#).0, 200, "the same title overwrites");
    assert_eq!(call("GET", "/todos/walk", ""), (200, r#"{"title":"walk"}"#.to_string()));
    assert_eq!(call("DELETE", "/todos/walk", "").0, 204);
    assert_eq!(call("GET", "/todos", ""), (200, r#"{"buy milk":{"title":"buy milk"}}"#.to_string()));
    assert_eq!(call("POST", "/todos", "not json").0, 500, "a body json_parse refuses settles to no response");
    assert_eq!(call("GET", "/nope", "").0, 404);
    assert_eq!(call("DELETE", "/todos", "").0, 405);

    // The listing shows the port and the request count; a server without actions runs nothing
    // by name.
    let listed = body_json(route(&state, &req("GET", "/deployments", vec![])));
    assert_eq!(listed["deployments"][0]["port"], 18080);
    assert_eq!(listed["deployments"][0]["trigger_count"], 13);
    assert_eq!(route(&state, &req("POST", "/trigger/todo-api", b"x".to_vec())).status_code, 404);

    // A second server on the same port is refused; a broken declaration is a 400.
    let other = todo_server(&state, "18080");
    assert_eq!(deploy_server(&state, "other", other).status_code, 409);
    let bad = todo_server(&state, "0");
    assert_eq!(deploy_server(&state, "bad", bad).status_code, 400);

    // A restart reloads it as a server, with its items.
    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(reloaded["todo-api"].server.as_ref().and_then(|s| s.port), Some(18080));
    assert_eq!(reloaded["todo-api"].triggers, vec!["Http :18080"], "labelled as when it was deployed");
    let todos = reloaded["todo-api"].state.as_ref().unwrap().state.lock().unwrap().get("todos").unwrap().clone();
    assert!(matches!(todos, Value::Record(ref items) if items.len() == 1));
    let _ = std::fs::remove_file(&path);
}

/// A list of todos filled in bulk: POST /todos/bulk extends the stored list with the posted
/// array (where `append` would store the array as one item), and GET /todos lists them.
#[test]
fn a_bulk_route_extends_the_stored_list_with_an_array() {
    let state = test_state();
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let empty = || leaf("default", vec![Param::frozen("type", "Json"), Param::frozen("value", "[]")], vec![("value", Type::Any)], Type::Any);
    let list = endpoint(&state, "GET", "/todos", vec![test_store_read("todos"), empty()], vec![e(0, 0, 1, 0), e(1, 0, BOUNDARY, 0)], 0);
    let bulk = endpoint(
        &state,
        "POST",
        "/todos/bulk",
        vec![
            leaf("field", vec![Param::frozen("name", "body")], vec![("record", Type::Record)], Type::Any),
            leaf("json_parse", vec![], vec![("text", Type::Text)], Type::Any),
            test_store_read("todos"),
            empty(),
            leaf("extend", vec![], vec![("list", Type::list_any()), ("items", Type::list_any())], Type::list_any()),
            test_store_write("todos"),
        ],
        vec![
            e(BOUNDARY, 0, 0, 0), // request → field(body)
            e(0, 0, 1, 0),        // body → json_parse
            e(2, 0, 3, 0),        // store_read → default([])
            e(3, 0, 4, 0),        // the stored list → extend
            e(1, 0, 4, 1),        // the posted array → extend's items
            e(4, 0, 5, 0),        // extended → store_write
            e(4, 0, BOUNDARY, 0), // response: the whole list
            e(5, 0, BOUNDARY, 1), // effect: the write
        ],
        1,
    );
    let api = stage(
        &state,
        &Unit {
            body: UnitBody::Composite { children: vec![list, bulk], edges: vec![] },
            params: vec![Param::frozen("declare", "server"), Param::frozen("port", "18084")],
            inputs: vec![],
            outputs: vec![],
        },
    );
    assert_eq!(deploy_server(&state, "bulk", api).status_code, 200);
    let call = |method: &str, url: &str, body: &str| {
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, "bulk",
            &fake_data(method, url, body.as_bytes().to_vec()));
        (r.status, String::from_utf8(r.body).unwrap())
    };
    assert_eq!(call("POST", "/todos/bulk", r#"[{"title":"buy milk"},{"done":false,"title":"walk"}]"#).0, 200);
    assert_eq!(call("POST", "/todos/bulk", r#"[{"title":"later"}]"#).0, 200);
    assert_eq!(
        call("GET", "/todos", ""),
        (200, r#"[{"title":"buy milk"},{"done":false,"title":"walk"},{"title":"later"}]"#.to_string()),
        "a flat list, in order"
    );
    assert_eq!(call("POST", "/todos/bulk", r#"{"title":"not an array"}"#).0, 500, "one item is not a list: no response");
    assert_eq!(call("GET", "/todos", "").1.matches("title").count(), 3, "and nothing was written");
}

/// The flow log records every run with what it read and produced, enough to run it again
/// exactly; and a deployment keeps the canvas it was deployed from.
#[test]
fn the_flow_log_records_each_run_well_enough_to_replay_it() {
    let state = test_state();
    let api = todo_server(&state, "18083");
    let body = serde_json::to_vec(&serde_json::json!({
        "slug": "todo",
        "unit": to_hex(&api),
        "acknowledge": ["NetworkIngress", "StateRead(\"todos\")", "StateWrite(\"todos\")"],
        "document": { "nodes": [], "edges": [], "viewport": { "x": 0, "y": 0, "zoom": 1 } },
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
    let call = |method: &str, url: &str, body: &str| {
        serve_data(&state.store, &state.deployments, &state.deployments_path, "todo",
            &fake_data(method, url, body.as_bytes().to_vec()))
        .status
    };
    assert_eq!(call("POST", "/todos", r#"{"title":"a"}"#), 200);
    assert_eq!(call("POST", "/todos", r#"{"title":"b"}"#), 200);
    assert_eq!(call("GET", "/nope", ""), 404);

    let log = body_json(route(&state, &req("GET", "/deployments/todo/log", vec![])));
    assert_eq!(log["next"], 3);
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    let second = &entries[1];
    assert_eq!((second["kind"].as_str(), second["route"].as_str(), second["status"].as_u64()), (Some("request"), Some("POST /todos"), Some(200)));
    assert_eq!(second["unit"], to_hex(&api), "the version: the server's unit, not its route's endpoint");
    assert_eq!(second["session"], 1);
    assert_eq!(second["response_body"], r#"{"title":"b"}"#);
    assert_eq!(second["state_before"]["todos"]["value"].as_array().unwrap().len(), 1, "it read the record with one item");
    assert_eq!(second["performed"][0]["capability"], "StateWrite(\"todos\")");
    assert_eq!((entries[2]["status"].as_u64(), entries[2]["route"].is_null()), (Some(404), true), "an unrouted request is logged too");
    // Each step's timing. This fixture's endpoint holds its 7 nodes directly ([k]); the stored
    // record a store_read reads is injected as a small composite, timed one level down.
    let paths: Vec<Vec<u64>> = second["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["path"].as_array().unwrap().iter().map(|p| p.as_u64().unwrap()).collect())
        .collect();
    let mut top: Vec<u64> = paths.iter().filter(|p| p.len() == 1).map(|p| p[0]).collect();
    top.sort();
    assert_eq!(top, vec![0, 1, 2, 3, 4, 5, 6], "one timing per node");
    assert!(paths.iter().all(|p| p.len() <= recorder::Timer::DEPTH));
    assert!(second["timeline"].as_array().unwrap().iter().all(|t| t["end_us"].as_f64() >= t["start_us"].as_f64()));
    let newer = body_json(route(&state, &req("GET", "/deployments/todo/log?after=1", vec![])));
    assert_eq!(newer["entries"].as_array().unwrap().len(), 1);

    // Replay: the entry alone (request, state read, sourced values) reproduces the run.
    let resolver = closure(&state.store, api).unwrap();
    let server = read_server(resolver.resolve(&api).unwrap(), &resolver).unwrap();
    let endpoint = resolver.resolve(&server.routes.iter().find(|r| r.method == "POST").unwrap().endpoint).unwrap();
    let sv = |v: &serde_json::Value| serde_json::from_value::<SValue>(v.clone()).unwrap().into_core();
    let mut sourced: Vec<(Capability, Value)> = second["state_before"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (Capability::StateRead(k.clone()), sv(v)))
        .collect();
    for pair in second["sourced"].as_array().unwrap() {
        let cap = match pair[0].as_str().unwrap() {
            "Clock" => Capability::Clock,
            "Random" => Capability::Random,
            _ => Capability::NetworkIngress,
        };
        sourced.push((cap, sv(&pair[1])));
    }
    let mut host = RecordingHost::new();
    let replayed = fire_sourced_with(endpoint, &[Some(sv(&second["request"]))], &sourced, &resolver, &mut host);
    let recorded: Vec<Option<Value>> =
        second["outputs"].as_array().unwrap().iter().map(|o| (!o.is_null()).then(|| sv(o))).collect();
    assert_eq!(replayed, recorded, "replaying the entry gives exactly what was recorded");

    // The canvas it was deployed from, and a redeploy keeps the log.
    let doc = route(&state, &req("GET", "/deployments/todo/document", vec![]));
    assert_eq!((doc.status_code, body_json(doc)["viewport"]["zoom"].as_u64()), (200, Some(1)));
    let again = serde_json::to_vec(&serde_json::json!({
        "slug": "todo", "unit": to_hex(&api),
        "acknowledge": ["NetworkIngress", "StateRead(\"todos\")", "StateWrite(\"todos\")"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", again)).status_code, 200);
    assert_eq!(body_json(route(&state, &req("GET", "/deployments/todo/log", vec![])))["next"], 3);
    assert_eq!(route(&state, &req("GET", "/deployments/todo/document", vec![])).status_code, 200, "kept");
}

/// Run, pause, stop, and the sessions a deployment keeps until it is deleted.
#[test]
fn a_deployment_runs_pauses_stops_and_keeps_every_session() {
    let mut state = test_state();
    let path = unique_temp_path("lifecycle");
    state.deployments_path = path.clone().into();
    let api = todo_server(&state, "18082");
    assert_eq!(deploy_server(&state, "todo", api).status_code, 200);
    let call = |method: &str, body: &str| {
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, "todo",
            &fake_data(method, "/todos", body.as_bytes().to_vec()));
        (r.status, String::from_utf8(r.body).unwrap())
    };
    let act = |action: &str, body: &str| {
        let r = route(&state, &req("POST", &format!("/deployments/todo/{action}"), body.as_bytes().to_vec()));
        (r.status_code, body_json(r))
    };

    assert_eq!(call("POST", r#"{"title":"a"}"#).0, 200);
    // Paused: no answers, nothing to trigger, the session goes on.
    let (code, entry) = act("pause", "");
    assert_eq!((code, entry["status"].as_str()), (200, Some("paused")));
    assert_eq!(call("GET", "").0, 503);
    assert_eq!(act("pause", "").0, 409, "already paused");
    assert_eq!(act("run", "").1["session"], 1, "resume continues session 1");
    assert_eq!(call("GET", ""), (200, r#"{"a":{"title":"a"}}"#.to_string()));

    // Delete only once stopped. Stopping ends session 1 and keeps its state.
    assert_eq!(route(&state, &req("DELETE", "/deployments/todo", vec![])).status_code, 409);
    assert_eq!(act("stop", "").1["status"], "stopped");
    assert_eq!(call("GET", "").0, 503);
    assert_eq!(act("stop", "").0, 409, "already stopped");

    // Session 2 starts fresh; session 3 restores session 1's snapshot.
    assert_eq!(act("run", r#"{"from":"fresh"}"#).1["session"], 2);
    assert_eq!(call("GET", ""), (200, "{}".to_string()));
    assert_eq!(call("POST", r#"{"title":"b"}"#).0, 200);
    act("stop", "");
    assert_eq!(act("run", r#"{"from":1}"#).1["session"], 3);
    assert_eq!(call("GET", ""), (200, r#"{"a":{"title":"a"}}"#.to_string()));
    assert_eq!(act("run", r#"{"from":"sideways"}"#).0, 400);

    let sessions = body_json(route(&state, &req("GET", "/deployments/todo/sessions", vec![])));
    let summary: Vec<(u64, &str, u64)> = sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["number"].as_u64().unwrap(), s["from"].as_str().unwrap(), s["requests"].as_u64().unwrap()))
        .collect();
    assert_eq!(summary, vec![(1, "deploy", 2), (2, "fresh", 2), (3, "session 1", 1)], "a refused (paused) request is not counted");
    assert_eq!(sessions["sessions"][1]["state"]["todos"]["value"][0][0], "b", "session 2 held only b");
    assert!(sessions["sessions"][2]["state"].is_null(), "the running session has no snapshot yet");

    // A restart keeps the status and every session.
    act("stop", "");
    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(reloaded["todo"].life.status, Status::Stopped);
    assert_eq!(reloaded["todo"].life.sessions.len(), 3);
    assert_eq!(route(&state, &req("DELETE", "/deployments/todo", vec![])).status_code, 200);
    let _ = std::fs::remove_file(&path);
}

/// The editor's "send live": a request sent over the control plane reaches the deployed server
/// exactly as one on its port would, state included.
#[test]
fn a_request_sent_through_the_control_plane_reaches_the_server() {
    let state = test_state();
    let api = todo_server(&state, "18081");
    assert_eq!(deploy_server(&state, "todo", api).status_code, 200);
    let send = |slug: &str, body: serde_json::Value| {
        let r = route(&state, &req("POST", &format!("/deployments/{slug}/request"), serde_json::to_vec(&body).unwrap()));
        (r.status_code, body_json(r))
    };
    let (code, sent) = send("todo", serde_json::json!({
        "method": "POST", "url": "/todos", "headers": [["Content-Type", "application/json"]], "body": "{\"title\":\"milk\"}"
    }));
    assert_eq!((code, &sent["status"], &sent["body"]), (200, &serde_json::json!(200), &serde_json::json!("{\"title\":\"milk\"}")));
    let (_, listed) = send("todo", serde_json::json!({ "method": "GET", "url": "/todos?x=1" }));
    assert_eq!(listed["status"], 200);
    assert_eq!(listed["body"], "{\"milk\":{\"title\":\"milk\"}}");
    assert!(listed["headers"].as_array().unwrap().iter().any(|h| h[0] == "content-type" && h[1] == "application/json"));
    assert_eq!(send("todo", serde_json::json!({ "method": "GET", "url": "/nope" })).1["status"], 404);

    assert_eq!(send("missing", serde_json::json!({ "method": "GET", "url": "/" })).0, 404);
    assert_eq!(send("todo", serde_json::json!({ "method": "GET", "url": "todos" })).0, 400);
    assert_eq!(send("todo", serde_json::json!({ "url": "/todos" })).0, 400, "method is required");
}

/// Regression: `persist_deployments` ran with the table lock held, and inside it locked
/// every deployment's state. So while one deployment was mid-fire (holding its own state
/// lock), any persist parked on that lock *while holding the table*, and every endpoint
/// and the clock scheduler waited behind it.
#[test]
fn a_fire_in_progress_does_not_stall_the_table() {
    use std::sync::mpsc;
    use std::time::Duration;

    let mut state = test_state();
    let path = unique_temp_path("stall");
    state.deployments_path = path.clone().into();
    let deploy = |state: &AppState, slug: &str, unit: Hash, ack: serde_json::Value| {
        let body = serde_json::to_vec(&serde_json::json!({ "slug": slug, "unit": to_hex(&unit), "acknowledge": ack }))
            .unwrap();
        route(state, &req("POST", "/deploy", body)).status_code
    };
    let a = stage_writer(&state, "k");
    assert_eq!(deploy(&state, "a", a, serde_json::json!(["Action(\"put\")", "StateWrite(\"k\")"])), 200);

    // Simulate "a" mid-fire: hold its state lock, as fire_with_state does for a whole round.
    let a_state = state.deployments.lock().unwrap()["a"].state.clone().unwrap();
    let mid_fire = a_state.state.lock().unwrap();

    std::thread::scope(|s| {
        // Deploying "b" persists, and persisting needs "a"'s state, so this waits for "a".
        let deploying = s.spawn(|| deploy(&state, "b", a, serde_json::json!(["Action(\"put\")", "StateWrite(\"k\")"])));
        std::thread::sleep(Duration::from_millis(200));

        // ...but the table itself must stay usable meanwhile.
        let (tx, rx) = mpsc::channel();
        let shared = &state;
        s.spawn(move || tx.send(route(shared, &req("GET", "/deployments", vec![])).status_code).unwrap());
        let listed = rx.recv_timeout(Duration::from_secs(5));

        // Let "a" finish before asserting, so a failure reports instead of hanging the scope.
        drop(mid_fire);
        assert_eq!(listed, Ok(200), "GET /deployments stalled behind a fire");
        assert_eq!(deploying.join().unwrap(), 200);
    });

    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(reloaded.keys().collect::<Vec<_>>(), vec!["a", "b"]);
    let _ = std::fs::remove_file(&path);
}

/// What a clock's tick writes is kept for the next: each tick writes the time it ticked at.
#[test]
fn clock_tick_with_state_persists_across_ticks() {
    let state = test_state();
    let unit = stage_text(&state, TICKER);
    let ack = serde_json::json!({ "acknowledge": ["Clock", "StateWrite(\"last_tick\")"] });
    assert_eq!(deploy_json(&state, "clock-writer", unit, ack).status_code, 200);

    let t0 = SystemTime::now();
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0);
    let epoch0 = t0.duration_since(UNIX_EPOCH).unwrap().as_secs() as f64;
    let s1 = body_json(route(&state, &req("GET", "/deployments/clock-writer/state", vec![])));
    assert_eq!(s1["state"]["last_tick"]["value"], epoch0);

    let t1 = t0 + Duration::from_secs(6);
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t1);
    let epoch1 = t1.duration_since(UNIX_EPOCH).unwrap().as_secs() as f64;
    let s2 = body_json(route(&state, &req("GET", "/deployments/clock-writer/state", vec![])));
    assert_eq!(s2["state"]["last_tick"]["value"], epoch1);
    assert_ne!(epoch0, epoch1); // genuinely a new value, not a stale re-read
}

/// The honest-absence side, end to end: a fresh deployment's key that's never been written
/// evaluates to nothing, so what it feeds (here a copy into another key) isn't written, while the
/// run still answers — the same rule `holon-core`'s own tests prove in isolation, confirmed
/// through a real run.
#[test]
fn reading_a_never_written_key_evaluates_to_nothing() {
    let state = test_state();
    let text = "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"reader\")\n\nexport const copy = action(() => {\n  state.copy = state.never_written\n  return \"done\"\n})\n";
    let unit = stage_text(&state, text);
    let ack = serde_json::json!({ "acknowledge": ["Action(\"copy\")", "StateRead(\"never_written\")", "StateWrite(\"copy\")"] });
    assert_eq!(deploy_json(&state, "reader", unit, ack).status_code, 200);

    let ran = body_json(run_action_named(&state, "reader", "copy", serde_json::json!({})));
    assert_eq!(ran["answer"], "done", "{ran}");
    let entry = &log_entries(&state, "reader")[0];
    assert_eq!(entry["performed"].as_array().map_or(0, Vec::len), 0, "{entry}");
    let s = body_json(route(&state, &req("GET", "/deployments/reader/state", vec![])));
    assert!(s["state"].get("copy").is_none(), "{s}");
}

/// The deployments listing says whether each has state, and how many runs it had and how many
/// failed.
#[test]
fn deployments_listing_shows_state_and_counters() {
    let state = test_state();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "stateful", writer, ack).status_code, 200);
    let echo = stage_text(&state, ECHO);
    assert_eq!(deploy_json(&state, "stateless", echo, serde_json::json!({ "acknowledge": ["Action(\"echo\")"] })).status_code, 200);

    put(&state, "stateful", "x");
    put(&state, "stateful", "y");

    let list = body_json(route(&state, &req("GET", "/deployments", vec![])));
    let entries = list["deployments"].as_array().unwrap();
    let stateful = entries.iter().find(|e| e["slug"] == "stateful").unwrap();
    assert_eq!(stateful["has_state"], true);
    assert_eq!(stateful["trigger_count"], 2);
    assert_eq!(stateful["error_count"], 0);
    let stateless = entries.iter().find(|e| e["slug"] == "stateless").unwrap();
    assert_eq!(stateless["has_state"], false);
    assert_eq!(stateless["trigger_count"], 0);
}

/// A state is read only from a deployment that has one: no such slug is a 404, one without state a
/// 400.
#[test]
fn get_state_404s_for_unknown_slug_and_400s_for_stateless_deployment() {
    let state = test_state();
    assert_eq!(route(&state, &req("GET", "/deployments/nope/state", vec![])).status_code, 404);

    let echo = stage_text(&state, ECHO);
    assert_eq!(deploy_json(&state, "stateless", echo, serde_json::json!({ "acknowledge": ["Action(\"echo\")"] })).status_code, 200);

    assert_eq!(route(&state, &req("GET", "/deployments/stateless/state", vec![])).status_code, 400);
}

/// A deployment's state survives a restart: a fresh load from the same file and store holds what
/// the last run wrote, and the deployment runs on from it.
#[test]
fn state_persists_across_a_simulated_restart() {
    let deployments_path = unique_temp_path("state-deployments");
    let store_dir = unique_temp_path("state-store");
    let backend: DynStore = Box::new(FileStore::new(&store_dir).unwrap());
    let store = SharedStore::new(backend);

    // "First run": deploy and run once — writes "before-restart".
    {
        let deployments = Arc::new(Mutex::new(load_deployments(&Place::parse(&deployments_path), &store, &Policy::default()).unwrap()));
        let state = AppState {
            store: store.clone(),            deployments,
            deployments_path: Place::parse(&deployments_path),
            flows: Mutex::new(BTreeMap::new()),
            flows_path: Place::Nowhere,
            port: TEST_PORT,
            listeners: None,
            data_limits: dataplane::DataLimits::default(),
            policy: crate::sync::RwLock::new(Arc::new(Policy::default())),
            policy_path: Place::Nowhere,
            secrets: Arc::new(SecretStore::memory()),
            folders: Arc::new(folders::Folders::memory()),
            access: access::Access::default(),
            activity: activity::Activity::memory(),
            editor: None,
            sweeps: Default::default(),
            checked: Default::default(),
            proposals: Default::default(),
        };
        let writer = stage_writer(&state, "counter");
        let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"counter\")"] });
        assert_eq!(deploy_json(&state, "writer", writer, ack).status_code, 200);
        assert_eq!(put(&state, "writer", "before-restart").status_code, 200);
    }

    // "Restart": a fresh load from the same path/store — the state survives, and the
    // deployment can still be run again afterward.
    let reloaded = load_deployments(&Place::parse(&deployments_path), &store, &Policy::default()).unwrap();
    let dep = reloaded.get("writer").unwrap();
    let restored_state = dep.state.as_ref().unwrap().state.lock().unwrap();
    assert_eq!(restored_state.get("counter"), Some(&Value::Text("before-restart".to_string())));
    drop(restored_state);

    let state2 = AppState {
        store: store.clone(),        deployments: Arc::new(Mutex::new(reloaded)),
        deployments_path: Place::parse(&deployments_path),
        flows: Mutex::new(BTreeMap::new()),
        flows_path: Place::Nowhere,
        port: TEST_PORT,
        listeners: None,
        data_limits: dataplane::DataLimits::default(),
        policy: crate::sync::RwLock::new(Arc::new(Policy::default())),
        policy_path: Place::Nowhere,
        secrets: Arc::new(SecretStore::memory()),
        folders: Arc::new(folders::Folders::memory()),
        access: access::Access::default(),
        activity: activity::Activity::memory(),
        editor: None,
        sweeps: Default::default(),
        checked: Default::default(),
        proposals: Default::default(),
    };
    assert_eq!(put(&state2, "writer", "after-restart").status_code, 200);
    let s = body_json(route(&state2, &req("GET", "/deployments/writer/state", vec![])));
    assert_eq!(s["state"]["counter"]["value"], "after-restart");

    let _ = std::fs::remove_file(&deployments_path);
    let _ = std::fs::remove_dir_all(&store_dir);
}

// ── CORS ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cors_allows_localhost_origin_and_omits_the_header_for_others() {
    let state = test_state();

    let allowed =
        Request::fake_http("GET", "/health", vec![host_header(), ("Origin".to_string(), "http://localhost:5173".to_string())], vec![]);
    let r1 = with_cors(&state.access, &allowed, route(&state, &allowed));
    assert!(r1
        .headers
        .iter()
        .any(|(k, v)| k.as_ref() == "Access-Control-Allow-Origin" && v.as_ref() == "http://localhost:5173"));

    let disallowed =
        Request::fake_http("GET", "/health", vec![host_header(), ("Origin".to_string(), "http://evil.example".to_string())], vec![]);
    let r2 = with_cors(&state.access, &disallowed, route(&state, &disallowed));
    assert!(!r2.headers.iter().any(|(k, _)| k.as_ref() == "Access-Control-Allow-Origin"));
}

#[test]
fn cors_allows_every_method_this_api_actually_uses() {
    // A real, shipped bug, not a hypothetical: Access-Control-Allow-Methods once listed
    // "GET, POST, OPTIONS" — DELETE was missing. The server-side OPTIONS handling
    // (`route`'s own preflight branch) still answered 204 every time regardless, so nothing
    // about the SERVER looked broken in isolation — a browser reads the *header's own
    // contents*, not just the preflight's status code, and silently refuses to ever send
    // the real DELETE if its method isn't listed. `assert!(..headers..)` alone (the test
    // above) would never have caught this — it only checks the header is PRESENT, never
    // what it actually says. This checks the one thing that matters: every method a route
    // in can.rs's table answers is actually named in the value.
    let state = test_state();
    let req = Request::fake_http(
        "OPTIONS",
        "/deployments/whatever",
        vec![host_header(), ("Origin".to_string(), "http://localhost:5173".to_string())],
        vec![],
    );
    let res = with_cors(&state.access, &req, route(&state, &req));
    let methods = res
        .headers
        .iter()
        .find(|(k, _)| k.as_ref() == "Access-Control-Allow-Methods")
        .map(|(_, v)| v.as_ref().to_string())
        .unwrap_or_default();
    for method in crate::can::ROUTES.iter().map(|r| r.method) {
        assert!(methods.split(", ").any(|m| m == method), "Access-Control-Allow-Methods {methods:?} is missing {method}");
    }
}

// ── control-plane request gate ───────────────────────────────────────────────────────

fn raw(method: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Request {
    let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Request::fake_http(method, url, headers, body.to_vec())
}

fn saved_flow_names(state: &AppState) -> Vec<String> {
    state.flows.lock().unwrap().keys().cloned().collect()
}

/// Regression: CORS only hid responses, so any web page could still *send* a simple
/// `text/plain` POST and overwrite a saved flow, deploy, or trigger. And with no Host check,
/// a DNS-rebinding page was same-origin with full access.
#[test]
fn cross_site_and_rebinding_requests_are_refused_before_routing() {
    let state = test_state();
    let host = ("Host", "127.0.0.1:7420");
    let save = br#"{"name": "victim", "document": {}}"#;
    // What it is, its headers, and the status it must get.
    type Attack<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16);
    let attacks: [Attack; 7] = [
        ("simple text/plain POST from another site", vec![host, ("Origin", "http://evil.example"), ("Content-Type", "text/plain")], 403),
        ("JSON POST from another site", vec![host, ("Origin", "http://evil.example"), ("Content-Type", "application/json")], 403),
        ("sandboxed iframe (Origin: null)", vec![host, ("Origin", "null"), ("Content-Type", "application/json")], 403),
        ("look-alike origin", vec![host, ("Origin", "http://localhost.evil.example"), ("Content-Type", "application/json")], 403),
        ("DNS rebinding (attacker's hostname)", vec![("Host", "rebind.evil.example:7420"), ("Content-Type", "application/json")], 403),
        ("wrong port in Host", vec![("Host", "127.0.0.1:9999"), ("Content-Type", "application/json")], 403),
        ("form POST with no Origin", vec![host, ("Content-Type", "application/x-www-form-urlencoded")], 415),
    ];
    for (label, headers, status) in attacks {
        let r = route(&state, &raw("POST", "/flows", &headers, save));
        assert_eq!(r.status_code, status, "{label}");
    }
    assert!(saved_flow_names(&state).is_empty(), "no attack may have saved a flow");

    // A missing Host is refused too, even for a GET.
    assert_eq!(route(&state, &raw("GET", "/health", &[], b"")).status_code, 403);

    // The editor (allowed origin) and curl (no Origin) both still work.
    let editor = [host, ("Origin", "http://localhost:5173"), ("Content-Type", "application/json; charset=utf-8")];
    assert_eq!(route(&state, &raw("POST", "/flows", &editor, save)).status_code, 200);
    let curl = [("Host", "localhost:7420"), ("Content-Type", "application/json")];
    assert_eq!(route(&state, &raw("POST", "/flows", &curl, save)).status_code, 200);
    assert_eq!(saved_flow_names(&state), vec!["victim"]);
}

/// Regression: `read_body` had no cap, so one huge POST could exhaust memory.
#[test]
fn oversized_body_is_refused() {
    let state = test_state();
    let at_cap = vec![b' '; MAX_BODY_BYTES as usize];
    let r = route(&state, &req("POST", "/flows", at_cap));
    assert_ne!(r.status_code, 413, "a body exactly at the cap is read (and then fails to parse)");
    let over = vec![b' '; MAX_BODY_BYTES as usize + 1];
    assert_eq!(route(&state, &req("POST", "/flows", over)).status_code, 413);
}

/// A run is logged with the unit it ran, and a redeploy keeps the canvas of the version it
/// replaces: a run logged on an earlier version is shown on its own canvas, not the newest.
#[test]
fn each_run_names_its_version_and_each_version_keeps_its_canvas() {
    let state = test_state();
    let v1 = stage_writer(&state, "a");
    let v2 = stage_writer(&state, "b");
    let deploy = |unit: Hash, key: &str, rev: u32| {
        let ack = serde_json::json!(["Action(\"put\")", format!("StateWrite(\"{key}\")")]);
        deploy_json(&state, "v", unit, serde_json::json!({ "acknowledge": ack, "document": { "rev": rev } })).status_code
    };
    let fire = || put(&state, "v", "hello").status_code;
    assert_eq!(deploy(v1, "a", 1), 200);
    assert_eq!(fire(), 200);
    assert_eq!(deploy(v2, "b", 2), 200);
    assert_eq!(fire(), 200);

    let log = body_json(route(&state, &req("GET", "/deployments/v/log", vec![])));
    let units: Vec<&str> = log["entries"].as_array().unwrap().iter().map(|e| e["unit"].as_str().unwrap()).collect();
    assert_eq!(units, [to_hex(&v1), to_hex(&v2)], "each run names the version it ran");

    let doc = |query: &str| route(&state, &req("GET", &format!("/deployments/v/document{query}"), vec![]));
    assert_eq!(body_json(doc(""))["rev"], 2);
    assert_eq!(body_json(doc(&format!("?unit={}", to_hex(&v1))))["rev"], 1, "the earlier version's canvas");
    assert_eq!(body_json(doc(&format!("?unit={}", to_hex(&v2))))["rev"], 2);
    let gone = doc("?unit=00ff");
    assert_eq!(gone.status_code, 404);
    assert!(String::from_utf8(body_bytes(gone)).unwrap().contains("keeps no canvas"));
}

/// Every deploy is a version, kept for good; restoring an earlier one's code deploys it again as a
/// new version, checked against today's grant, and leaves the state as it is.
#[test]
fn every_deploy_is_a_version_and_restoring_one_is_a_new_one() {
    let state = test_state();
    let v1 = stage_writer(&state, "a");
    let v2 = stage_writer(&state, "b");
    let deploy = |unit: Hash, key: &str, rev: u32| {
        let ack = serde_json::json!(["Action(\"put\")", format!("StateWrite(\"{key}\")")]);
        body_json(deploy_json(&state, "r", unit, serde_json::json!({ "acknowledge": ack, "document": { "rev": rev } })))
    };
    assert_eq!(deploy(v1, "a", 1)["version"], 1);
    assert_eq!(deploy(v2, "b", 2)["version"], 2);
    assert_eq!(put(&state, "r", "hello").status_code, 200);
    let versions = || body_json(route(&state, &req("GET", "/deployments/r/versions", vec![])))["versions"].clone();
    let listed = versions();
    assert_eq!(listed[0]["number"], 2, "newest first");
    assert_eq!((listed[0]["current"].clone(), listed[1]["current"].clone()), (true.into(), false.into()));
    assert_eq!(listed[1]["unit"], to_hex(&v1));
    assert_eq!(listed[1]["restorable"], true);

    let restore = |body: serde_json::Value| route(&state, &req("POST", "/deployments/r/restore", body.to_string().into_bytes()));
    // v1 writes `a`, which the grant v2 has now doesn't cover: approved again, not inherited.
    let preview = body_json(restore(serde_json::json!({ "version": 1, "code": true, "dry_run": true })));
    assert_eq!(preview["needs_approval"], serde_json::json!(["StateWrite(\"a\")"]));
    // What is granted now, beside what the code asks for: a grant diff.
    assert!(!preview["granted_now"].as_array().unwrap().contains(&"StateWrite(\"a\")".into()), "{preview}");
    assert!(preview["items"].as_array().unwrap().contains(&"StateWrite(\"a\")".into()), "{preview}");
    assert_eq!(versions().as_array().unwrap().len(), 2, "a dry run deploys nothing");
    let unapproved = restore(serde_json::json!({ "version": 1, "code": true }));
    assert_eq!(unapproved.status_code, 400);
    assert!(String::from_utf8(body_bytes(unapproved)).unwrap().contains("StateWrite(\\\"a\\\")"));

    let done = body_json(restore(serde_json::json!({ "version": 1, "code": true, "acknowledge": ["StateWrite(\"a\")"] })));
    assert_eq!((done["version"].clone(), done["unit"].clone()), (3.into(), to_hex(&v1).into()));
    let listed = versions();
    assert_eq!((listed[0]["number"].clone(), listed[0]["restored_from"].clone(), listed[0]["current"].clone()), (3.into(), 1.into(), true.into()));
    assert_eq!(body_json(route(&state, &req("GET", "/deployments/r/document", vec![])))["rev"], 1, "its canvas came back with it");
    let st = body_json(route(&state, &req("GET", "/deployments/r/state", vec![])));
    assert!(st.to_string().contains("\"b\""), "the state is as v2 left it: {st}");
    assert_eq!(put(&state, "r", "again").status_code, 200, "it runs as v1 again");
    let log = body_json(route(&state, &req("GET", "/deployments/r/log", vec![])));
    assert_eq!(log["entries"].as_array().unwrap().last().unwrap()["unit"], to_hex(&v1));

    for (body, status) in [
        (serde_json::json!({ "version": 3, "code": true }), 409),
        (serde_json::json!({ "version": 9, "code": true }), 404),
        (serde_json::json!({ "version": 2 }), 400),
        (serde_json::json!({ "version": 2, "run": 1, "code": true }), 400),
    ] {
        assert_eq!(restore(body.clone()).status_code, status, "{body}");
    }
    assert_eq!(route(&state, &req("POST", "/deployments/nope/restore", br#"{"version":1,"code":true}"#.to_vec())).status_code, 404);
}

/// The state a run left, or a version left, is put back as the live state, in a new session: the
/// state it replaces is kept as the last session's and can be put back in turn. A dry run says
/// which keys would change and how many runs were logged since; a stopped deployment, a run not
/// in the log and the version deployed now are refused.
#[test]
fn a_runs_or_a_versions_state_is_put_back_in_a_new_session() {
    let state = test_state();
    let writer = stage_writer(&state, "a");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"a\")"] });
    assert_eq!(deploy_json(&state, "s", writer, ack.clone()).status_code, 200);
    let fire = |text: &str| assert_eq!(put(&state, "s", text).status_code, 200);
    fire("one");
    fire("two");
    let log = body_json(route(&state, &req("GET", "/deployments/s/log", vec![])));
    let entries = log["entries"].as_array().unwrap().clone();
    let (one, two) = (entries[0]["seq"].as_u64().unwrap(), entries[1]["seq"].as_u64().unwrap());
    assert!(entries.iter().all(|e| e["state_after"].is_string()), "each run names the state it left");
    let now = || body_json(route(&state, &req("GET", "/deployments/s/state", vec![]))).to_string();
    let restore = |body: serde_json::Value| route(&state, &req("POST", "/deployments/s/restore", body.to_string().into_bytes()));

    let dry = body_json(restore(serde_json::json!({ "run": one, "state": true, "dry_run": true })));
    assert_eq!(dry["state"]["changed"], serde_json::json!(["a"]));
    // Each key it touches, as it is now and as it would be: a state diff.
    assert_eq!(dry["state"]["values"]["a"]["now"]["value"], "two");
    assert_eq!(dry["state"]["values"]["a"]["then"]["value"], "one");
    assert_eq!((dry["state"]["runs_since"].clone(), dry["state"]["effects_since"].clone()), (1.into(), serde_json::json!({})));
    assert!(dry["code"].is_null(), "no code to deploy");
    assert!(now().contains("\"two\""), "a dry run changes nothing");

    let done = body_json(restore(serde_json::json!({ "run": one, "state": true })));
    assert_eq!((done["state"]["from"].clone(), done["state"]["session"].clone()), (format!("run {one}").into(), 2.into()));
    assert!(now().contains("\"one\"") && !now().contains("\"two\""), "run {one}'s state is live: {}", now());
    let sessions = body_json(route(&state, &req("GET", "/deployments/s/sessions", vec![])));
    let s = |i: usize| sessions["sessions"][i].clone();
    assert_eq!(s(0)["state"]["a"]["value"], "two", "the state it replaced is session 1's");
    assert_eq!((s(1)["from"].clone(), s(1)["ended_at"].clone()), (format!("run {one}").into(), serde_json::Value::Null));
    assert_eq!(body_json(restore(serde_json::json!({ "run": two, "state": true })))["state"]["session"], 3, "and is put back in turn");
    assert!(now().contains("\"two\""));

    // A version's state is the state the version after it was deployed with.
    assert_eq!(deploy_json(&state, "s", writer, ack).status_code, 200);
    fire("three");
    let dry = body_json(restore(serde_json::json!({ "version": 1, "state": true, "dry_run": true })));
    assert_eq!((dry["state"]["changed"].clone(), dry["state"]["runs_since"].clone()), (serde_json::json!(["a"]), 1.into()));
    let versions = body_json(route(&state, &req("GET", "/deployments/s/versions", vec![])))["versions"].clone();
    assert_eq!((versions[0]["state_kept"].clone(), versions[1]["state_kept"].clone()), (false.into(), true.into()));
    assert_eq!(body_json(restore(serde_json::json!({ "version": 1, "state": true })))["state"]["from"], "version 1");
    assert!(now().contains("\"two\""), "the state version 1 left: {}", now());

    let log = body_json(route(&state, &req("GET", "/deployments/s/log", vec![])));
    let three = log["entries"].as_array().unwrap().last().unwrap()["seq"].as_u64().unwrap();
    for (body, status) in [
        (serde_json::json!({ "version": 2, "state": true }), 409),
        (serde_json::json!({ "run": three + 9, "state": true }), 404),
        (serde_json::json!({ "run": three, "code": true }), 409),
        (serde_json::json!({ "version": 1, "run": three, "state": true }), 400),
        (serde_json::json!({ "run": three }), 400),
    ] {
        assert_eq!(restore(body.clone()).status_code, status, "{body}");
    }
    route(&state, &req("POST", "/deployments/s/stop", vec![]));
    let stopped = restore(serde_json::json!({ "run": three, "state": true }));
    assert_eq!(stopped.status_code, 409);
    assert!(String::from_utf8(body_bytes(stopped)).unwrap().contains("it is stopped"));
}

/// A run on an earlier version restores both: that version's code again, as a new version, then
/// the state the run left. A deployment without state has none to restore.
#[test]
fn a_runs_code_and_state_come_back_together() {
    let state = test_state();
    let v1 = stage_writer(&state, "a");
    let v2 = stage_writer(&state, "b");
    let deploy = |unit: Hash, key: &str| {
        let ack = serde_json::json!(["Action(\"put\")", format!("StateWrite(\"{key}\")")]);
        deploy_json(&state, "both", unit, serde_json::json!({ "acknowledge": ack })).status_code
    };
    let fire = |text: &str| assert_eq!(put(&state, "both", text).status_code, 200);
    assert_eq!(deploy(v1, "a"), 200);
    fire("first");
    let seq = body_json(route(&state, &req("GET", "/deployments/both/log", vec![])))["entries"][0]["seq"].as_u64().unwrap();
    assert_eq!(deploy(v2, "b"), 200);
    fire("second");
    let restore = |body: serde_json::Value| route(&state, &req("POST", "/deployments/both/restore", body.to_string().into_bytes()));

    let dry = body_json(restore(serde_json::json!({ "run": seq, "code": true, "state": true, "dry_run": true })));
    assert_eq!(dry["code"], 1, "the version it ran on");
    assert_eq!(dry["needs_approval"], serde_json::json!(["StateWrite(\"a\")"]));
    assert_eq!(dry["state"]["removed"], serde_json::json!(["b"]), "b was written since");
    assert!(dry["state"]["values"]["b"]["then"].is_null() && !dry["state"]["values"]["b"]["now"].is_null(), "{dry}");
    assert_eq!(dry["state"]["collections"], serde_json::json!({}), "no collection here");
    let unapproved = restore(serde_json::json!({ "run": seq, "code": true, "state": true }));
    assert_eq!(unapproved.status_code, 400);
    assert_eq!(body_json(route(&state, &req("GET", "/deployments/both/versions", vec![])))["versions"].as_array().unwrap().len(), 2, "nothing changed");

    let done = body_json(restore(serde_json::json!({ "run": seq, "code": true, "state": true, "acknowledge": ["StateWrite(\"a\")"] })));
    assert_eq!((done["code"].clone(), done["version"].clone(), done["unit"].clone()), (1.into(), 3.into(), to_hex(&v1).into()));
    assert_eq!(done["state"]["from"], format!("run {seq}"));
    let st = body_json(route(&state, &req("GET", "/deployments/both/state", vec![]))).to_string();
    assert!(st.contains("\"first\"") && !st.contains("\"second\""), "the state run {seq} left: {st}");

    let plain = stage_text(&state, ECHO);
    assert_eq!(deploy_json(&state, "plain", plain, serde_json::json!({ "acknowledge": ["Action(\"echo\")"] })).status_code, 200);
    let none = route(&state, &req("POST", "/deployments/plain/restore", br#"{"version":1,"state":true}"#.to_vec()));
    assert_eq!(none.status_code, 409);
    assert!(String::from_utf8(body_bytes(none)).unwrap().contains("keeps no state"));
}

/// What was done to a deployment is on its timeline, each with who: deploys, secrets (never a
/// value), grant changes, the host's own pause and resume, pausing, stopping and running, and
/// restores; each after the runs before it, in a chain kept across a restart, exported, and checked
/// by verify, which finds an event changed outside holon.
#[test]
fn what_was_done_to_a_deployment_is_its_events() {
    let mut state = test_state();
    let path = unique_temp_path("events");
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "ev", writer, ack).status_code, 200);
    assert_eq!(put(&state, "ev", "one").status_code, 200);
    let secret = serde_json::to_vec(&serde_json::json!({ "value": "a-value-never-shown" })).unwrap();
    assert_eq!(route(&state, &req("POST", "/deployments/ev/secrets/api_key", secret)).status_code, 200);
    assert!(change_grant(&state, "ev", serde_json::json!({ "revoke": ["StateWrite(\"k\")"] })).is_success());
    assert!(change_grant(&state, "ev", serde_json::json!({ "grant": ["StateWrite(\"k\")"] })).is_success());
    for (action, body) in [("pause", ""), ("run", ""), ("stop", ""), ("run", r#"{"from":"latest"}"#)] {
        assert!(route(&state, &req("POST", &format!("/deployments/ev/{action}"), body.as_bytes().to_vec())).is_success(), "{action}");
    }
    assert!(route(&state, &req("POST", "/deployments/ev/restore", br#"{"run":0,"state":true}"#.to_vec())).is_success());

    let listed = body_json(route(&state, &req("GET", "/deployments/ev/events", vec![])));
    let events = listed["events"].as_array().unwrap().clone();
    let said: Vec<(String, String)> = events.iter().map(|e| (e["kind"].as_str().unwrap().into(), e["what"].as_str().unwrap().into())).collect();
    let kinds: Vec<&str> = said.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        ["deploy", "secret", "grant", "lifecycle", "grant", "lifecycle", "lifecycle", "lifecycle", "lifecycle", "lifecycle", "restore"],
        "{said:#?}"
    );
    assert_eq!(said[0].1, "deployed version 1");
    assert!(said[1].1.starts_with("set the secret api_key, version 1, from a value"), "{}", said[1].1);
    assert!(said[2].1.contains("StateWrite") && said[3].1.starts_with("paused: "), "{said:#?}");
    assert_eq!(said[5].1, "resumed: its grant covers it again");
    assert_eq!(said[6..10].iter().map(|(_, w)| w.as_str()).collect::<Vec<_>>(), ["paused", "resumed", "stopped", "ran from latest, in session 2"]);
    assert_eq!(said[10].1, "put back the state run 0 left, in session 3");
    assert!(events[3]["by"].is_null() && events[5]["by"].is_null(), "the host's own: by nobody");
    assert_eq!((events[0]["run_seq"].clone(), events[1]["run_seq"].clone()), (0.into(), 1.into()), "the secret was set after run 0");
    assert_eq!(events[1]["name"], "api_key");
    assert_eq!((events[10]["session"].clone(), events[10]["from"].clone()), (3.into(), "run 0".into()), "what going back to the restore puts back");
    assert!(listed["breaks"].as_array().unwrap().is_empty());

    // Exported with the runs, checked, and never holding a secret's value.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/ev/export", vec![])))).unwrap();
    assert_eq!(export.events.len(), 11);
    assert!(!export.events.concat().contains("a-value-never-shown"));
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.events == 11, "{}", verify::describe(&report, None));
    let witnessed = recorder::line_hash(&export.events[4]);
    assert_eq!(verify::check(&export, Some(&witnessed), &BTreeMap::new()).witnessed_event, Some(4));
    let mut changed = verify::Export { events: export.events.clone(), ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
    changed.events[2] = changed.events[2].replace("StateWrite", "StateRead");
    let report = verify::check(&changed, None, &BTreeMap::new());
    assert_eq!((report.ok(), report.event_breaks.clone()), (false, vec![3]));

    // Kept for good, across a restart.
    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(reloaded["ev"].events.entries().len(), 11);
    assert!(reloaded["ev"].events.breaks().is_empty());
    route(&state, &req("POST", "/deployments/ev/stop", vec![]));
    assert_eq!(route(&state, &req("DELETE", "/deployments/ev", vec![])).status_code, 200);
    let _ = std::fs::remove_file(&path);
}

/// A deploy's note is kept with the grant it makes: on its event, read back across a restart and
/// exported, and in the activity. A deploy without one, or with only spaces, keeps none.
#[test]
fn a_deploys_note_is_kept_on_the_grants_event() {
    let mut state = test_state();
    let path = unique_temp_path("deploy-note");
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"], "note": "  checked the key it writes with Ada  " });
    assert_eq!(deploy_json(&state, "noted", writer, ack).status_code, 200);
    assert_eq!(deploy_json(&state, "noted", writer, serde_json::json!({ "note": "   " })).status_code, 200);

    let events = body_json(route(&state, &req("GET", "/deployments/noted/events", vec![])))["events"].as_array().unwrap().clone();
    assert_eq!(events.len(), 2, "{events:#?}");
    assert_eq!((events[0]["kind"].as_str(), events[0]["note"].as_str()), (Some("deploy"), Some("checked the key it writes with Ada")));
    assert!(events[1].get("note").is_none(), "{:?}", events[1]);
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/noted/export", vec![])))).unwrap();
    assert!(export.events[0].contains("checked the key it writes with Ada"));
    assert!(verify::check(&export, None, &BTreeMap::new()).ok());
    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(reloaded["noted"].events.entries()[0].note.as_deref(), Some("checked the key it writes with Ada"));

    let activity = body_json(route(&state, &req("GET", "/activity?limit=50", vec![])))["entries"].as_array().unwrap().clone();
    let deployed: Vec<_> = activity.iter().filter(|e| e["action"] == "deployed" && e["target"] == "noted").collect();
    assert_eq!(deployed.len(), 2, "{activity:#?}");
    assert_eq!((deployed[0].get("note"), deployed[1]["note"].as_str()), (None, Some("checked the key it writes with Ada")), "newest first");
    route(&state, &req("POST", "/deployments/noted/stop", vec![]));
    assert_eq!(route(&state, &req("DELETE", "/deployments/noted", vec![])).status_code, 200);
    let _ = std::fs::remove_file(&path);
}

/// A deployments file from before rc.30 kept the last 10 earlier versions' documents only: they
/// are numbered and their documents moved into the store at load, the current version added. They
/// open on their canvas but don't restore, having no record of what their deploys asked.
#[test]
fn versions_from_before_are_numbered_and_open_but_dont_restore() {
    let state = test_state();
    let legacy = |unit: &str, doc: serde_json::Value| -> crate::wire::Version {
        serde_json::from_value(serde_json::json!({ "unit": unit, "document": doc })).unwrap()
    };
    let current = || crate::wire::Version { document: Some(serde_json::json!(3)), ..legacy("u3", serde_json::Value::Null) };
    let versions = crate::deploy::complete_versions(vec![legacy("u1", serde_json::json!(1)), legacy("u2", serde_json::json!(2))], current(), &state.store).unwrap();
    assert_eq!(versions.iter().map(|v| (v.number, v.unit.as_str())).collect::<Vec<_>>(), [(1, "u1"), (2, "u2"), (3, "u3")]);
    assert!(versions.iter().all(|v| v.document.is_none() && v.document_ref.is_some() && v.request.is_none()));
    // Written again and read back: nothing added twice.
    let again = crate::deploy::complete_versions(versions.clone(), current(), &state.store).unwrap();
    assert_eq!(again, versions);
}

/// A deployment's versions survive a restart, and an older release's reader finds none rather
/// than failing on them (they are written under a new name).
#[test]
fn versions_survive_a_restart() {
    let mut state = test_state();
    let path = unique_temp_path("kept-versions");
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"], "document": { "rev": 1 } });
    assert_eq!(deploy_json(&state, "kept", writer, ack.clone()).status_code, 200);
    assert_eq!(deploy_json(&state, "kept", writer, ack).status_code, 200);
    let text = std::fs::read_to_string(&path).unwrap();
    let file: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(file[0].get("versions").is_none(), "the old name isn't written");
    let kept = file[0]["versions_kept"].as_array().unwrap();
    assert_eq!(kept.len(), 2);
    assert!(kept.iter().all(|v| v.get("document").is_none() && v["document_ref"].is_string()), "canvases are kept by hash, not inline");

    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    let versions = &reloaded["kept"].versions;
    assert_eq!(versions.iter().map(|v| v.number).collect::<Vec<_>>(), [1, 2]);
    assert!(versions.iter().all(|v| v.request.is_some() && v.document_ref.is_some()));
    route(&state, &req("POST", "/deployments/kept/stop", vec![]));
    assert_eq!(route(&state, &req("DELETE", "/deployments/kept", vec![])).status_code, 200);
    let _ = std::fs::remove_file(&path);
}

/// A deployment's flow log is kept beside the deployments file: a restart reads it back, so a
/// past run can still be replayed, and deleting the deployment deletes it.
#[test]
fn a_flow_log_survives_a_restart_and_goes_with_its_deployment() {
    let mut state = test_state();
    let path = unique_temp_path("kept-log");
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "kept", writer, ack).status_code, 200);
    for _ in 0..2 {
        assert_eq!(put(&state, "kept", "hello").status_code, 200);
    }
    let file = recorder::file_for(&Place::parse(&path), "kept").unwrap();
    assert!(file.exists());

    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    let entries = reloaded["kept"].log.with(|log| log.after(None, 10));
    assert_eq!(entries.iter().map(|e| e.seq).collect::<Vec<_>>(), [0, 1], "read back after a restart");
    assert_eq!(entries[1].unit, to_hex(&writer));

    route(&state, &req("POST", "/deployments/kept/stop", vec![]));
    assert_eq!(route(&state, &req("DELETE", "/deployments/kept", vec![])).status_code, 200);
    assert!(!file.exists(), "deleted with its deployment");
    let _ = std::fs::remove_file(&path);
}

/// The runs a deployment logs are a hash chain through the routes: the log says its head, a
/// restart reads it back unbroken, and a run changed in the file outside holon shows as a break
/// at the run after it.
#[test]
fn a_flow_log_is_a_chain_and_a_changed_run_shows_as_a_break() {
    let mut state = test_state();
    let path = unique_temp_path("chained-log");
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "chained", writer, ack).status_code, 200);
    for _ in 0..3 {
        assert_eq!(put(&state, "chained", "hello").status_code, 200);
    }
    let log = body_json(route(&state, &req("GET", "/deployments/chained/log", vec![])));
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries[0].get("prev"), None, "the first run has nothing before it");
    let file = recorder::file_for(&Place::parse(&path), "chained").unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(entries[1]["prev"], recorder::line_hash(lines[0]));
    assert_eq!(log["head"], recorder::line_hash(lines[2]));
    assert_eq!(log["breaks"], serde_json::json!([]));
    assert!(load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap()["chained"].log.with(|log| log.breaks().is_empty()));

    // The first run's body changed in the file: the second no longer follows it.
    let changed = text.replacen("hello", "jello", 1);
    assert_ne!(changed, text, "the run's request is in the file");
    std::fs::write(&file, changed).unwrap();
    let reloaded = load_deployments(&Place::parse(&path), &state.store, &state.policy()).unwrap();
    assert_eq!(reloaded["chained"].log.with(|log| log.breaks().to_vec()), [1]);

    route(&state, &req("POST", "/deployments/chained/stop", vec![]));
    route(&state, &req("DELETE", "/deployments/chained", vec![]));
    let _ = std::fs::remove_file(&path);
}

/// A server with accounts, through its routes: set up with the printed code, then signed in,
/// an invite accepted, each role doing what it may, and an admin's API token.
/// `path` as written, and as a router might also take it: with a trailing slash, its last segment
/// empty, and each slash doubled in turn.
fn spellings(path: &str) -> Vec<String> {
    let mut all = vec![path.to_string(), format!("{path}/")];
    if let Some((head, _)) = path.rsplit_once('/')
        && !head.is_empty()
    {
        all.push(format!("{head}/"));
    }
    all.extend(path.match_indices('/').map(|(i, _)| format!("{}/{}", &path[..i], &path[i..])));
    all
}

/// Whatever the gate keeps for some, nobody else reaches, however its path is spelled. For every
/// route can.rs's table asks `can` about, every caller the gate refuses it to (no one, a viewer, an
/// editor, an agent, a service), under every spelling a router might take for its path, is refused
/// before anything answers it: 401 or 403, or a bare 404 for a path no route has. A handler's
/// answer (a 400 saying why, a 404 naming what is missing) means the gate and the router read that
/// path apart, as they once did a secret's with no name.
#[test]
fn the_gate_and_the_router_read_every_path_alike() {
    use crate::accounts::{Accounts, Kind};
    use crate::can::{Rule, ROUTES};
    use access::Role;
    let pw = "correct horse battery";
    let mut state = test_state();
    let accounts = std::sync::Arc::new(Accounts::memory());
    let (ada, _) = accounts.setup(&accounts.setup_code().unwrap(), "ada", pw).unwrap();
    let person = |name: &str, role: Role| accounts.accept(&accounts.invite(name, role.name(), None, "ada").unwrap(), pw).unwrap().0;
    let (cy, bob) = (person("cy", Role::Viewer), person("bob", Role::Editor));
    let token = |name: &str, kind, role: Role| {
        accounts.add(name, kind, role.name(), "ada", "ada").unwrap();
        accounts.create_token(name, "the sweep", "ada").unwrap().0
    };
    let (claude, ci) = (token("claude", Kind::Agent, Role::Agent), token("ci", Kind::Service, Role::Editor));
    let ci_token = accounts.tokens("ci")[0].id.clone();
    state.access = access::Access::new(std::sync::Arc::clone(&accounts), vec![], vec![]);
    // Sent as if through a proxy, so a folder dialog (pick.rs) is never shown here.
    let request = |method: &str, url: &str, token: Option<&str>| {
        let mut headers = vec![("X-Forwarded-For".to_string(), "203.0.113.9".to_string())];
        headers.extend(token.map(|t| ("Authorization".to_string(), format!("Bearer {t}"))));
        req_with(method, url, vec![], headers)
    };
    let callers = [("no one", None), ("cy, a viewer", Some(cy.as_str())), ("bob, an editor", Some(bob.as_str())), ("claude, an agent", Some(claude.as_str())), ("ci, a service", Some(ci.as_str()))];
    let hash = "0".repeat(64);
    let mut sent = 0;
    for row in ROUTES.iter().filter(|r| matches!(r.rule, Rule::Can(..))) {
        // ci's tokens, the deployment d, its secret k.
        let people = row.path.starts_with("/people/");
        let path = row
            .path
            .split('/')
            .map(|s| match s {
                "{slug}" => "d",
                "{name}" if people => "ci",
                "{name}" => "k",
                "{id}" if people => ci_token.as_str(),
                "{id}" => "1",
                "{hash}" => hash.as_str(),
                s => s,
            })
            .collect::<Vec<_>>()
            .join("/");
        let admitted = |token: Option<&str>| state.access.admit(&request(row.method, &path, token)).is_ok();
        // Not vacuous: an admin is let through, and no one isn't.
        assert!(admitted(Some(&ada)), "{} {path}: ada, an admin, is let through", row.method);
        let refused: Vec<_> = callers.iter().filter(|(_, token)| !admitted(*token)).collect();
        assert!(refused.iter().any(|(who, _)| *who == "no one"), "{} {path}: no one is let through", row.method);
        for spelled in spellings(&path) {
            for (who, token) in &refused {
                let answer = route(&state, &request(row.method, &spelled, *token));
                let status = answer.status_code;
                let said = String::from_utf8_lossy(&body_bytes(answer)).into_owned();
                let refused = matches!(status, 401 | 403) || (status == 404 && said.is_empty());
                assert!(refused, "{} {spelled} ({path} spelled otherwise), by {who}: answered {status} {said}", row.method);
                sent += 1;
            }
        }
    }
    assert!(sent > 1000, "{sent} requests sent");
}

/// What `/auth/owned` lists is what its caller makes and revokes tokens for. Each caller (an admin,
/// an editor, a viewer, the run file's user, an agent, a service) is listed exactly the agents and
/// services they own, each with its tokens; whoever isn't an admin lists, makes and revokes the
/// tokens of those and is refused every other's, an admin every one's.
#[test]
fn what_one_owns_is_listed_and_its_tokens_are_theirs_to_make() {
    use crate::accounts::Kind;
    use access::Role;
    use std::collections::BTreeSet;
    let mut state = test_state();
    let People { ada, claude } = with_people(&mut state);
    let accounts = Arc::clone(state.access.accounts().unwrap());
    let person = |name: &str, role: Role| accounts.accept(&accounts.invite(name, role.name(), None, "ada").unwrap(), "correct horse battery").unwrap().0;
    let (bob, cy, local) = (person("bob", Role::Editor), person("cy", Role::Viewer), accounts.new_local("bma"));
    // claude is ada's (with_people); bob owns two, cy one, the OS user its agent.
    for (name, kind, role, owner) in [("ci", Kind::Service, Role::Editor, "bob"), ("nightly", Kind::Service, Role::Viewer, "bob"), ("scout", Kind::Agent, Role::Agent, "cy"), ("bma-agent", Kind::Agent, Role::Agent, "bma")] {
        accounts.add(name, kind, role.name(), owner, "ada").unwrap();
        accounts.create_token(name, "first", "ada").unwrap();
    }
    let ci = accounts.create_token("ci", "a script", "bob").unwrap().0;
    let all = ["claude", "ci", "nightly", "scout", "bma-agent"];
    let callers = [("ada", &ada, true), ("bob", &bob, false), ("cy", &cy, false), ("bma", &local, true), ("claude", &claude, false), ("ci", &ci, false)];
    let ask = |token: &str, method: &str, url: &str, body: serde_json::Value| {
        let bytes = if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() };
        let answer = route(&state, &req_as(token, method, url, bytes));
        let status = answer.status_code;
        (status, serde_json::from_slice::<serde_json::Value>(&body_bytes(answer)).unwrap_or_default())
    };
    let ids = |name: &str| accounts.tokens(name).iter().map(|t| t.id.clone()).collect::<BTreeSet<_>>();
    for (me, token, admin) in callers {
        let owns: BTreeSet<&str> = all.iter().copied().filter(|n| accounts.principal(n).and_then(|p| p.owner).as_deref() == Some(me)).collect();
        let (status, listed) = ask(token, "GET", "/auth/owned", serde_json::Value::Null);
        assert_eq!(status, 200, "{me}: {listed}");
        let listed = listed.as_array().unwrap();
        assert_eq!(listed.iter().map(|p| p["name"].as_str().unwrap()).collect::<BTreeSet<_>>(), owns, "{me} is listed what they own");
        for p in listed {
            let shown = p["tokens"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap().to_string()).collect::<BTreeSet<_>>();
            assert_eq!(shown, ids(p["name"].as_str().unwrap()), "{me}: {p}");
        }
        for name in all {
            let may = admin || owns.contains(name);
            let before = ids(name);
            let tokens = format!("/people/{name}/tokens");
            let (listing, _) = ask(token, "GET", &tokens, serde_json::Value::Null);
            let (making, made) = ask(token, "POST", &tokens, serde_json::json!({ "label": format!("by {me}") }));
            assert_eq!((listing, making), if may { (200, 200) } else { (403, 403) }, "{me}, {name}'s tokens: {made}");
            // What they made they revoke; one they didn't, they may not.
            let id = made["entry"]["id"].as_str().map(String::from).unwrap_or_else(|| before.first().unwrap().clone());
            let (revoking, _) = ask(token, "DELETE", &format!("{tokens}/{id}"), serde_json::Value::Null);
            assert_eq!(revoking, if may { 204 } else { 403 }, "{me} revokes {name}'s token");
            assert_eq!(ids(name), before, "{me}: {name}'s tokens are as they were");
        }
    }
}

/// A key a flow taints is tainted wherever a run has it, whatever started the run: in a request (a
/// header, the query as sent and as read, a path parameter, a JSON body at depth, a number held as
/// a number), in what a call sends and what the far side answers (and the cookie it sets), in a
/// WebSocket's message, in an action's input and what it writes, and in what a later run reads
/// back; and so are the credentials every run taints (a Bearer token, a Basic password, a
/// cookie). No form of any value is in the runs' log, their listing, their lines, the export,
/// `holon verify`'s report or a replay's; each is marked `‹taint:key›` there. Each caller is
/// answered with the values as they are, and the runs check and replay as themselves.
#[test]
fn a_tainted_keys_values_never_reach_what_the_host_keeps_wherever_a_run_has_them() {
    use std::io::Write;
    use tungstenite::client::IntoClientRequest;
    use tungstenite::Message;
    // The far side: it answers a card under a tainted key, and sets a session.
    let (far_number, far_session) = ("5500000000000004", "fs-6b2d9e4a1c");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let far = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            if read_request(&mut stream).is_none() {
                continue;
            }
            let body = format!(r#"{{"card":{{"number":"{far_number}"}},"holder":"Ada"}}"#);
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: far_session={far_session}; Path=/; HttpOnly\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = stream.write_all(format!("{head}{body}").as_bytes());
        }
    });
    let mut state = claims_state("");
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let port = free_port();
    let text = format!(
        r#"import {{ action, flow, route }} from "holon"
import far from "host:{far}"
import notes from "collection:notes"

flow("teller", {{ port: {port}, taint: ["x-api-key", "number", "code", "token", "pin"] }})

route.post("/pay/:code", (req) => {{
  const key = req.headers["x-api-key"]
  console.log("paying", key, req.query.token, req.json.card.number)
  return {{ key, code: req.params.code, card: req.json.card.number, token: req.query.token, cookie: req.headers.cookie, auth: req.headers.authorization }}
}})

route.post("/card", async (req) => {{
  const got = await far.post("/cards", {{ headers: {{ "x-api-key": req.json.key }}, body: {{ holder: "Ada" }} }})
  return got.body
}})

route.websocket("/ws", {{ message: (m) => ({{ echo: m.text }}) }})

export const keep = action((input) => {{
  notes[input.id] = input
  return input.id
}})

export const look = action((input) => notes[input.id])
"#
    );
    deploy_text(&state, &text, &far);
    sync_listeners(&state);

    // A request, with the credentials every run taints.
    let (key, bearer, password, cookie, token, code, card) = ("kx-7f3a9c2e1b", "bt-4d8e2f6a0c", "pw-9b1c5e7d3f", "ck-2a6f8e0b4d", "tk/3c+7e=1a5f", "cd-8e4a2c6f0b", "4111111111111111");
    let basic = "YWRhOnB3LTliMWM1ZTdkM2Y="; // base64 of "ada:" and the password
    let sent_token = "tk%2F3c%2B7e%3D1a5f";
    let send = |url: String, body: String| {
        let request = data::DataRequest {
            method: "POST".into(),
            url,
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("X-Api-Key".into(), key.into()),
                ("Authorization".into(), format!("Bearer {bearer}")),
                ("Proxy-Authorization".into(), format!("Basic {basic}")),
                ("Cookie".into(), format!("sid={cookie}; theme=dark")),
            ],
            body: body.into_bytes(),
            by: None,
        };
        let answered = serve_data(&state.store, &state.deployments, &state.deployments_path, "teller", &request);
        (answered.status, String::from_utf8(answered.body).unwrap())
    };
    let (status, answer) = send(format!("/pay/{code}?token={sent_token}"), format!(r#"{{"card": {{"number": {card}}}, "amount": "12.00"}}"#));
    assert_eq!(status, 200, "{answer}");
    for value in [key, bearer, code, card, token] {
        assert!(answer.contains(value), "the caller is answered with {value:?} as it is: {answer}");
    }
    // A call: the key it sends, and what the far side answers.
    let sent_key = "kx-out-5d1e8b3f";
    let (status, answer) = send("/card".into(), format!(r#"{{"key": "{sent_key}"}}"#));
    assert!(status == 200 && answer.contains(far_number), "{status} {answer}");
    // A WebSocket's message.
    let ws_number = "6011000000000004";
    let stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let (mut ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap(), stream).unwrap();
    ws.send(Message::text(format!(r#"{{"number":"{ws_number}"}}"#))).unwrap();
    let echoed = loop {
        if let Message::Text(t) = ws.read().unwrap() {
            break t.to_string();
        }
    };
    assert!(echoed.contains(ws_number), "{echoed}");
    drop(ws);
    // An action's input, written; and a later run that reads it back.
    let (pin, kept_token) = ("pn-0e7c3a9d5b", "at-4c8f2e6a0d");
    assert_eq!(run_action_named(&state, "teller", "keep", serde_json::json!({ "id": "n1", "pin": pin, "token": kept_token })).status_code, 200);
    let looked = body_json(run_action_named(&state, "teller", "look", serde_json::json!({ "id": "n1" })));
    assert!(looked.to_string().contains(pin), "{looked}");
    crate::recorder::sync_logs(&state.deployments).unwrap();

    let never = [key, bearer, password, cookie, token, sent_token, code, card, basic, "ada:pw-9b1c5e7d3f", sent_key, far_number, far_session, ws_number, pin, kept_token];
    let get = |path: &str| body_json(route(&state, &req("GET", path, vec![])));
    let unit = stage_text(&state, &text);
    let export = get("/deployments/teller/export");
    let report = verify::check(&serde_json::from_value(export.clone()).unwrap(), None, &BTreeMap::new());
    let replayed = get(&format!("/deployments/teller/replay?unit={}", to_hex(&unit)));
    let kept = [
        ("its log", get("/deployments/teller/log").to_string()),
        ("its runs", get("/deployments/teller/runs").to_string()),
        ("its lines", get("/deployments/teller/logs").to_string()),
        ("its export", export.to_string()),
        ("holon verify", verify::describe(&report, None)),
        ("a replay", replayed.to_string()),
    ];
    for (place, shown) in &kept {
        for value in never {
            assert!(!shown.contains(value), "{place} holds {value:?}: {shown}");
        }
    }
    let log = &kept[0].1;
    for k in ["x-api-key", "number", "code", "token", "pin", "authorization", "proxy-authorization", "cookie", "set-cookie"] {
        assert!(log.contains(&format!("‹taint:{k}›")), "the log marks {k}: {log}");
    }
    assert!(kept[2].1.contains("‹taint:x-api-key›") && kept[2].1.contains("‹taint:number›"), "its lines mark them: {}", kept[2].1);
    assert!(report.ok(), "the runs check as themselves: {}", kept[4].1);
    assert_eq!(replayed["differ"], 0, "the runs replay as themselves, or can't be told: {replayed}");
    let same = replays_as_it_verifies(&state, "teller");
    assert!(["request", "websocket", "trigger"].iter().all(|k| same.contains(*k)), "{same:?}");
}

/// Every body the control plane reads goes through `read_body`, which reads it by the deadline:
/// a handler reading one itself would hold its place for as long as the client likes. (Its own
/// tests' servers read as they like.)
#[test]
fn every_body_the_control_plane_reads_is_read_by_the_deadline() {
    let mut read_elsewhere = Vec::new();
    let mut dirs = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))];
    while let Some(dir) = dirs.pop() {
        for path in std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()) {
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Each test-only module left out: from its `#[cfg(test)]` to the `}` that closes it,
            // the first at the start of a line.
            let (mut lines, mut in_tests) = (text.lines().enumerate().peekable(), false);
            let mut code = Vec::new();
            while let Some((n, line)) = lines.next() {
                let opens = line == "#[cfg(test)]" && lines.peek().is_some_and(|(_, l)| l.starts_with("mod ") || l.starts_with("pub(crate) mod "));
                in_tests = (in_tests || opens) && line != "}";
                if !in_tests && line != "}" {
                    code.push((n, line));
                }
            }
            for (n, line) in code.into_iter().filter(|(_, l)| l.contains(".data()")) {
                let in_read_body = path.ends_with("http.rs") && line.trim_start().starts_with("let read = owned.data()");
                if !in_read_body {
                    read_elsewhere.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(read_elsewhere.is_empty(), "a body read but through read_body: {read_elsewhere:#?}");
}

/// A body that stops, or trickles in, holds its request's place no longer than the deadline, on
/// every route here that reads one: past it the place is given back, and one that trickles is
/// told 408. A whole body goes through as before.
#[test]
fn a_body_that_stops_or_trickles_gives_its_place_back_by_the_deadline() {
    use std::io::Write as _;
    use std::net::TcpStream;
    use std::sync::atomic::{AtomicUsize, Ordering::Acquire};
    use std::time::Instant;
    static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
    let paths = ["/unit", "/flows", "/admin/storage"];
    let most = paths.len();
    let state: &'static AppState = Box::leak(Box::new(test_state()));
    let server = rouille::Server::new("127.0.0.1:0", move |request| admitted(state, request, &IN_FLIGHT, most)).unwrap();
    let at = server.server_addr();
    let (_, stop) = server.stoppable();
    let send = |path: &str, body: &[u8], length: usize| {
        let mut s = TcpStream::connect(at).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(s, "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{TEST_PORT}\r\nContent-Type: application/json\r\nContent-Length: {length}\r\n\r\n").unwrap();
        s.write_all(body).unwrap();
        s
    };
    let status = |s: &mut TcpStream| {
        let mut head = [0u8; 12];
        s.read_exact(&mut head).map(|_| String::from_utf8_lossy(&head[9..]).into_owned()).unwrap_or_else(|e| format!("no answer: {e}"))
    };
    let until = |what: &str, by: Duration, done: &dyn Fn() -> bool| {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < by, "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };

    // Bodies that stop part way: each holds its place while it waits, every place is held...
    // (A body of 1 KiB or less tiny_http reads itself before handing the request on, holding no
    // place: these say they are longer.)
    let long = 100_000;
    let stopped: Vec<TcpStream> = paths.iter().map(|path| send(path, b"{\"a\"", long)).collect();
    until("every route reads its body", Duration::from_secs(5), &|| IN_FLIGHT.load(Acquire) == most);
    assert_eq!(status(&mut send("/unit", b"{}", 2)), "503", "every place is held");
    // ...and each gives it back by the deadline.
    until("a stopped body held its place past the deadline", BODY_DEADLINE + Duration::from_secs(2), &|| IN_FLIGHT.load(Acquire) == 0);

    // A body that trickles in, a byte every 100ms, is told 408 at the deadline.
    for path in paths {
        let mut s = send(path, b"", long);
        let mut trickle = s.try_clone().unwrap();
        std::thread::spawn(move || {
            for _ in 0..99 {
                std::thread::sleep(Duration::from_millis(100));
                if trickle.write_all(b" ").is_err() {
                    return;
                }
            }
        });
        let started = Instant::now();
        assert_eq!(status(&mut s), "408", "{path}: a body that trickles is cut off");
        let took = started.elapsed();
        assert!(took >= BODY_DEADLINE && took < BODY_DEADLINE + Duration::from_secs(2), "{path}: told after {took:?}");
    }
    until("a trickled body held its place", Duration::from_secs(2), &|| IN_FLIGHT.load(Acquire) == 0);

    // A whole body is read as before: the route answers it.
    for path in paths {
        let answered = status(&mut send(path, b"{}", 2));
        assert!(!["408", "503"].contains(&answered.as_str()) && !answered.starts_with("no answer"), "{path}: a whole body answered {answered}");
    }
    drop(stopped);
    let _ = stop.send(());
}

#[test]
fn accounts_through_the_routes() {
    let mut state = test_state();
    let accounts = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let code = accounts.setup_code().unwrap();
    state.access = access::Access::new(std::sync::Arc::clone(&accounts), vec![], vec![]);
    let call = |method: &str, url: &str, body: serde_json::Value, token: Option<&str>| {
        let bytes = if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() };
        let mut headers = req(method, url, bytes.clone()).headers().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        if let Some(t) = token {
            headers.push(("Authorization".into(), format!("Bearer {t}")));
        }
        route(&state, &Request::fake_http(method, url, headers, bytes))
    };
    let null = serde_json::Value::Null;
    assert_eq!(body_json(call("GET", "/auth/state", null.clone(), None))["setup"], true);
    assert_eq!(call("GET", "/flows", null.clone(), None).status_code, 403, "not set up yet");
    let pw = "correct horse battery";
    assert_eq!(call("POST", "/auth/setup", serde_json::json!({ "code": "NOPE-NOPE", "name": "ada", "password": pw }), None).status_code, 400);
    let ada = body_json(call("POST", "/auth/setup", serde_json::json!({ "code": code, "name": "ada", "password": pw }), None))["token"].as_str().unwrap().to_string();
    assert_eq!(call("GET", "/flows", null.clone(), None).status_code, 401, "now everyone signs in");
    assert_eq!(call("GET", "/flows", null.clone(), Some(&ada)).status_code, 200);

    // An invite: the link's holder sets their password and is signed in, as a viewer.
    let invite = body_json(call("POST", "/people", serde_json::json!({ "name": "cy", "kind": "person", "role": "viewer" }), Some(&ada)))["invite"].as_str().unwrap().to_string();
    assert_eq!(body_json(call("GET", &format!("/auth/invite?token={invite}"), null.clone(), None))["name"], "cy");
    let cy = body_json(call("POST", "/auth/invite", serde_json::json!({ "token": invite, "password": pw }), None))["token"].as_str().unwrap().to_string();
    let me = body_json(call("GET", "/auth/me", null.clone(), Some(&cy)));
    assert_eq!((&me["kind"], &me["role"]), (&serde_json::json!("person"), &serde_json::json!("viewer")));
    assert_eq!(call("POST", "/flows", serde_json::json!({ "name": "x", "document": {} }), Some(&cy)).status_code, 403);
    assert_eq!(call("GET", "/people", null.clone(), Some(&cy)).status_code, 403);
    // Signing in again, with the password cy set; then out.
    let again = body_json(call("POST", "/auth/sign-in", serde_json::json!({ "name": "cy", "password": pw }), None))["token"].as_str().unwrap().to_string();
    assert_eq!(call("POST", "/auth/sign-out", null.clone(), Some(&again)).status_code, 204);
    assert_eq!(call("GET", "/flows", null.clone(), Some(&again)).status_code, 401);

    // An admin makes cy an editor; a service for CI writes with its token, never managing people.
    assert_eq!(body_json(call("PATCH", "/people/cy", serde_json::json!({ "role": "editor" }), Some(&ada)))["role"], "editor");
    assert_eq!(call("POST", "/flows", serde_json::json!({ "name": "x", "document": {} }), Some(&cy)).status_code, 200);
    let added = body_json(call("POST", "/people", serde_json::json!({ "name": "ci", "kind": "service", "role": "editor" }), Some(&ada)));
    assert_eq!((&added["kind"], &added["owner"], &added["tokens"]), (&serde_json::json!("service"), &serde_json::json!("ada"), &serde_json::json!([])), "owned by whoever added it: {added}");
    assert_eq!(call("POST", "/people", serde_json::json!({ "name": "ops", "kind": "service", "role": "admin" }), Some(&ada)).status_code, 400, "a service is never an admin");
    let ci = body_json(call("POST", "/people/ci/tokens", serde_json::json!({ "label": "CI on GitHub" }), Some(&ada)))["token"].as_str().unwrap().to_string();
    assert_eq!(call("POST", "/flows", serde_json::json!({ "name": "y", "document": {} }), Some(&ci)).status_code, 200);
    assert_eq!(call("GET", "/people", null.clone(), Some(&ci)).status_code, 403);
    let me = body_json(call("GET", "/auth/me", null.clone(), Some(&ci)));
    assert_eq!((&me["name"], &me["kind"], &me["owner"]), (&serde_json::json!("ci"), &serde_json::json!("service"), &serde_json::json!("ada")));
    let activity = body_json(call("GET", "/activity", null.clone(), Some(&ada)));
    let saved_by_ci = activity["entries"].as_array().unwrap().iter().any(|e| e["action"] == "saved" && e["by"] == "ci");
    assert!(saved_by_ci, "{activity}");

    // An agent cy owns: cy makes and revokes its tokens, not the service's ada owns.
    call("POST", "/people", serde_json::json!({ "name": "claude", "kind": "agent", "role": "agent", "owner": "cy" }), Some(&ada));
    let made = body_json(call("POST", "/people/claude/tokens", serde_json::json!({ "label": "Claude Code" }), Some(&cy)));
    let (claude, id) = (made["token"].as_str().unwrap().to_string(), made["entry"]["id"].as_str().unwrap().to_string());
    assert_eq!(body_json(call("GET", "/auth/me", null.clone(), Some(&claude)))["name"], "claude");
    assert_eq!(body_json(call("GET", "/people/claude/tokens", null.clone(), Some(&cy))).as_array().unwrap().len(), 1);
    assert_eq!(call("POST", "/people/ci/tokens", serde_json::json!({ "label": "mine now" }), Some(&cy)).status_code, 403);
    assert_eq!(call("POST", "/people/nobody/tokens", serde_json::json!({ "label": "x" }), Some(&cy)).status_code, 403);
    assert_eq!(call("POST", "/people/claude/tokens", serde_json::json!({ "label": "x" }), Some(&claude)).status_code, 403, "an agent makes no tokens");
    assert_eq!(call("DELETE", &format!("/people/claude/tokens/{id}"), null.clone(), Some(&cy)).status_code, 204);
    assert_eq!(call("GET", "/flows", null.clone(), Some(&claude)).status_code, 401);
    // cy owns claude, so stays until claude has another owner.
    assert_eq!(call("DELETE", "/people/cy", null.clone(), Some(&ada)).status_code, 400);
    assert_eq!(body_json(call("PATCH", "/people/claude", serde_json::json!({ "owner": "ada" }), Some(&ada)))["owner"], "ada");
    assert_eq!(call("DELETE", "/people/cy", null.clone(), Some(&ada)).status_code, 204);
    let listed = body_json(call("GET", "/people", null.clone(), Some(&ada)));
    let kinds: Vec<_> = listed.as_array().unwrap().iter().map(|p| (p["name"].as_str().unwrap(), p["kind"].as_str().unwrap())).collect();
    assert_eq!(kinds, [("ada", "person"), ("ci", "service"), ("claude", "agent")]);
}

/// The local token (local.rs) is the OS user's admin, before setup and after, named in the
/// activity log as that user; another `hl_` token, or the last run's, is nobody.
#[test]
fn the_local_token_is_its_users_admin_for_this_run() {
    let mut state = test_state();
    let accounts = std::sync::Arc::new(crate::accounts::Accounts::memory());
    let stale = accounts.new_local("ada");
    let local = accounts.new_local("ada");
    state.access = access::Access::new(std::sync::Arc::clone(&accounts), vec![], vec![]);
    let call = |method: &str, url: &str, body: Vec<u8>, token: &str| {
        let mut headers = req(method, url, body.clone()).headers().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        headers.push(("Authorization".into(), format!("Bearer {token}")));
        route(&state, &Request::fake_http(method, url, headers, body))
    };
    assert_eq!(call("GET", "/flows", vec![], &local).status_code, 200, "before setup");
    assert_eq!(call("GET", "/flows", vec![], &stale).status_code, 403, "the last run's token");
    accounts.setup(&accounts.setup_code().unwrap(), "bob", "correct horse battery").unwrap();
    assert_eq!(call("GET", "/flows", vec![], &stale).status_code, 401);
    assert_eq!(call("GET", "/flows", vec![], "hl_nope").status_code, 401);
    assert_eq!(call("GET", "/people", vec![], &local).status_code, 200, "an admin");
    // An agent it adds is owned by the OS user, by its name, with no account of that name here.
    let added = body_json(call("POST", "/people", br#"{"name":"ada-agent","kind":"agent","role":"agent"}"#.to_vec(), &local));
    assert_eq!((&added["owner"], &added["created_by"]), (&serde_json::json!("ada"), &serde_json::json!("local:ada")), "{added}");
    assert!(call("POST", "/flows", br#"{"name":"mine","document":{}}"#.to_vec(), &local).is_success());
    let activity = body_json(call("GET", "/activity", vec![], &local));
    let by_local = activity["entries"].as_array().unwrap().iter().any(|e| e["action"] == "saved" && e["by"] == "local:ada");
    assert!(by_local, "{activity}");
}

/// With accounts, a save and a deploy remember who made them, and every action is in the
/// activity log with who did it, a deleted deployment's included.
#[test]
fn what_is_done_names_who_did_it() {
    let mut state = test_state();
    let accounts = std::sync::Arc::new(crate::accounts::Accounts::memory());
    accounts.setup(&accounts.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
    let bob = accounts.accept(&accounts.invite("bob", "editor", None, "ada").unwrap(), "correct horse battery").unwrap().0;
    let bob = bob.as_str();
    state.access = access::Access::new(accounts, vec![], vec![]);
    let as_bob = |method: &str, url: &str, body: Vec<u8>| {
        let r = req(method, url, body.clone());
        let mut headers = r.headers().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        headers.push(("Authorization".into(), format!("Bearer {bob}")));
        route(&state, &Request::fake_http(method, url, headers, body))
    };
    let saved = as_bob("POST", "/flows", br#"{"name":"mine","document":{}}"#.to_vec());
    assert_eq!(body_json(saved)["saved_by"], "bob");
    assert_eq!(body_json(as_bob("GET", "/flows", vec![]))[0]["saved_by"], "bob");

    let plan = holon_author::agent::plan(ECHO).unwrap();
    for u in &plan.units {
        as_bob("POST", "/unit", u.clone());
    }
    let body = serde_json::json!({ "slug": "echo", "unit": plan.unit, "acknowledge": ["Action(\"echo\")"] });
    assert_eq!(as_bob("POST", "/deploy", serde_json::to_vec(&body).unwrap()).status_code, 200);
    assert_eq!(body_json(as_bob("GET", "/deployments", vec![]))["deployments"][0]["deployed_by"], "bob");

    // Pausing, stopping and deleting are recorded too, and outlive the deployment.
    for action in ["pause", "stop"] {
        assert!(as_bob("POST", &format!("/deployments/echo/{action}"), b"{}".to_vec()).is_success(), "{action}");
    }
    assert!(as_bob("DELETE", "/deployments/echo", vec![]).is_success());
    let activity = body_json(as_bob("GET", "/activity?limit=10", vec![]));
    let done: Vec<(String, String, String)> = activity["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["action"].as_str().unwrap().into(), e["target"].as_str().unwrap().into(), e["by"].as_str().unwrap().into()))
        .collect();
    let bob = |a: &str, t: &str| (a.to_string(), t.to_string(), "bob".to_string());
    assert_eq!(done, [bob("deleted", "echo"), bob("stopped", "echo"), bob("paused", "echo"), bob("deployed", "echo"), bob("saved", "mine")], "newest first");
}

/// What a save or a deploy places is kept beside its document and listed, so who uses a holon
/// is answered from the listings; a malformed list is refused.
#[test]
fn saves_and_deploys_list_the_holons_they_place() {
    let state = test_state();
    let save = |name: &str, uses: serde_json::Value| {
        route(&state, &req("POST", "/flows", serde_json::to_vec(&serde_json::json!({ "name": name, "document": {}, "uses": uses })).unwrap()))
    };
    assert!(save("slugify", serde_json::json!([])).is_success());
    assert!(save("blog", serde_json::json!([{ "flow": "slugify", "saved_at": 7 }])).is_success());
    assert!(route(&state, &req("POST", "/flows", br#"{"name":"old","document":{}}"#.to_vec())).is_success());
    assert_eq!(save("bad", serde_json::json!([{ "flow": "../x", "saved_at": 1 }])).status_code, 400);
    let listed = body_json(route(&state, &req("GET", "/flows", vec![])));
    let uses_of = |name: &str| listed.as_array().unwrap().iter().find(|f| f["name"] == name).unwrap()["uses"].clone();
    assert_eq!(uses_of("blog"), serde_json::json!([{ "flow": "slugify", "saved_at": 7 }]));
    assert_eq!(uses_of("slugify"), serde_json::json!([]));
    assert_eq!(uses_of("old"), serde_json::Value::Null, "saved before uses were recorded: absent, not empty");

    let unit = stage_text(&state, ECHO);
    let body = serde_json::json!({ "slug": "blog", "unit": to_hex(&unit), "acknowledge": ["Action(\"echo\")"], "uses": [{ "flow": "slugify", "saved_at": 7 }] });
    assert!(route(&state, &req("POST", "/deploy", serde_json::to_vec(&body).unwrap())).is_success());
    let deployed = body_json(route(&state, &req("GET", "/deployments", vec![])));
    assert_eq!(deployed["deployments"][0]["uses"], serde_json::json!([{ "flow": "slugify", "saved_at": 7 }]));
}

/// Two saves of a flow in the same second still get different stamps, the later one later.
#[test]
fn every_save_of_a_flow_is_stamped_later_than_the_last() {
    let state = test_state();
    let save = || body_json(route(&state, &req("POST", "/flows", br#"{"name":"quick","document":{}}"#.to_vec())))["saved_at_epoch_seconds"].as_u64().unwrap();
    let (first, second, third) = (save(), save(), save());
    assert!(first < second && second < third, "{first} {second} {third}");
}

/// A deployment answers web pages from other sites only when it says so: their preflight, and
/// the header that lets the page read the answer; a site it doesn't name gets neither.
#[test]
fn a_deployment_answers_the_web_pages_it_names() {
    let state = test_state();
    let unit = todo_server(&state, "18131");
    let deploy = |browsers: serde_json::Value| {
        let body = serde_json::json!({ "slug": "todos", "unit": to_hex(&unit), "acknowledge": ["NetworkIngress", "StateRead(\"todos\")", "StateWrite(\"todos\")"], "browsers": browsers });
        route(&state, &req("POST", "/deploy", serde_json::to_vec(&body).unwrap()))
    };
    assert_eq!(deploy(serde_json::json!(["not an origin"])).status_code, 400);
    assert!(deploy(serde_json::json!(["https://app.example"])).is_success());
    let from = |origin: &str, method: &str| {
        let request = data::DataRequest { method: method.into(), url: "/todos".into(), headers: vec![("Origin".into(), origin.into())], body: vec![], by: None };
        serve_data(&state.store, &state.deployments, &state.deployments_path, "todos", &request)
    };
    let allow = |r: &data::DataResponse| r.headers.iter().find(|(k, _)| k == "access-control-allow-origin").map(|(_, v)| v.clone());
    let pre = from("https://app.example", "OPTIONS");
    assert_eq!((pre.status, allow(&pre).as_deref()), (204, Some("https://app.example")));
    assert_eq!(allow(&from("https://app.example", "GET")).as_deref(), Some("https://app.example"));
    assert_eq!(allow(&from("https://elsewhere.example", "GET")), None);
    assert_ne!(from("https://elsewhere.example", "OPTIONS").status, 204);
    // Any site, once it says `*`.
    assert!(deploy(serde_json::json!(["*"])).is_success());
    assert_eq!(allow(&from("https://elsewhere.example", "GET")).as_deref(), Some("https://elsewhere.example"));
}

/// `holon export`'s file: `--to` a folder and `--as` a name (`.json` added when it has none),
/// each defaulted; `-o` the whole path, alone.
#[test]
fn an_export_goes_to_a_folder_as_a_name() {
    use std::path::Path;
    let at = |to, name, out| verify::export_path("ledger", to, name, out);
    assert_eq!(at(None, None, None).unwrap(), Path::new("ledger-runs.json"));
    assert_eq!(at(Some("audits"), None, None).unwrap(), Path::new("audits").join("ledger-runs.json"));
    assert_eq!(at(None, Some("tuesday"), None).unwrap(), Path::new("tuesday.json"));
    assert_eq!(at(Some("audits"), Some("t.txt"), None).unwrap(), Path::new("audits").join("t.txt"));
    assert_eq!(at(None, None, Some("x/y.json")).unwrap(), Path::new("x/y.json"));
    assert!(at(None, Some("audits/t"), None).unwrap_err().contains("--to"));
    assert!(at(Some("audits"), None, Some("y.json")).unwrap_err().contains("not both"));
}

#[test]
fn at_takes_an_address_or_a_port_alone() {
    assert_eq!(parse_at("8080").unwrap().to_string(), "127.0.0.1:8080");
    assert_eq!(parse_at("0.0.0.0:7420").unwrap().to_string(), "0.0.0.0:7420");
    assert_eq!(parse_at("[::1]:9000").unwrap().to_string(), "[::1]:9000");
    for bad in ["", "localhost:8080", "8080:", "127.0.0.1", "70000"] {
        assert!(parse_at(bad).is_err(), "{bad:?}");
    }
}

/// Listening beyond this machine warns that it's plain HTTP, naming what is open; loopback doesn't.
#[test]
fn listening_beyond_loopback_warns_that_it_is_plain_http() {
    let local: std::net::IpAddr = [127, 0, 0, 1].into();
    assert_eq!(plain_http_warning(parse_at("7420").unwrap(), local), None);
    assert_eq!(plain_http_warning(parse_at("[::1]:7420").unwrap(), "::1".parse().unwrap()), None);
    let both = plain_http_warning(parse_at("0.0.0.0:7420").unwrap(), [0, 0, 0, 0].into()).unwrap();
    assert!(both.contains("the server and editor (0.0.0.0:7420) and deployed flows' ports (0.0.0.0)"), "{both}");
    assert!(both.contains("no TLS of its own"), "{both}");
    let flows = plain_http_warning(parse_at("7420").unwrap(), [10, 0, 0, 5].into()).unwrap();
    assert!(flows.contains("deployed flows' ports (10.0.0.5) take") && !flows.contains("editor"), "{flows}");
}

/// Each run's state follows from the one before, checked offline: it began from the state the run
/// before it left, and left what its write makes of it. A state_after rewritten (and the lines
/// chained again) re-runs as before, but the state no longer follows, there and at the next run;
/// a run that began from the state another began with ran beside it.
#[test]
fn verify_checks_that_each_runs_state_follows_from_the_one_before() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("state-follows").into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "follows", writer, ack).status_code, 200);
    for body in ["hello", "there", "again"] {
        assert_eq!(put(&state, "follows", body).status_code, 200);
    }
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/follows/export", vec![])))).unwrap();
    // A copy with each line changed by `edit`, the runs a second apart, chained again.
    let rechained = |edit: &dyn Fn(usize, &mut serde_json::Value)| {
        let mut copy = verify::Export { lines: Vec::new(), ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
        let mut prev = String::new();
        for (i, line) in export.lines.iter().enumerate() {
            let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
            v["at_ms"] = serde_json::json!(1_800_000_000_000u64 + i as u64 * 1000);
            v["duration_ms"] = serde_json::json!(2.0);
            edit(i, &mut v);
            if !prev.is_empty() {
                v["prev"] = serde_json::Value::String(prev.clone());
            }
            let line = serde_json::to_string(&v).unwrap();
            prev = recorder::line_hash(&line);
            copy.lines.push(line);
        }
        copy.head = prev;
        copy
    };
    let states = |e: &verify::Export| {
        let s = verify::check(e, None, &BTreeMap::new()).states.expect("its runs kept the state they left");
        (s.began, s.beside, s.left, s.breaks)
    };

    // As the server wrote them (a millisecond apart, so they may have run beside each other).
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    let s = report.states.as_ref().unwrap();
    assert_eq!((&s.began, &s.breaks), (&vec![1, 2], &vec![]));

    // A second apart, each is checked whole: the first has no run before it here.
    let apart = rechained(&|_, _| {});
    let report = verify::check(&apart, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert_eq!(states(&apart), (vec![1, 2], vec![], vec![0, 1, 2], vec![]));
    let text = verify::describe(&report, None);
    assert!(text.contains("state: 2 runs began from the state the run before them left, 3 left what their writes make of the one they began with"), "{text}");
    assert!(text.contains("state not checked (1 run: 0), where it began: no run before it here"), "{text}");

    // Run 1 says it left another state: it re-runs as before, but its state doesn't follow, nor
    // does run 2's.
    let forged = rechained(&|i, v| {
        if i == 1 {
            v["state_after"] = serde_json::json!("00".repeat(32));
        }
    });
    let report = verify::check(&forged, None, &BTreeMap::new());
    assert!(report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{:?}", report.reruns);
    assert!(!report.ok());
    let breaks = vec![
        (1, "the state it left isn't the one it began with, its writes applied".to_string()),
        (2, "it began from a state the run before it (run 1) didn't leave, nor a run beside it".to_string()),
    ];
    assert_eq!(states(&forged).3, breaks);
    assert!(verify::describe(&report, None).contains("run 2's state DOESN'T FOLLOW: it began from a state the run before it (run 1) didn't leave"));

    // Run 2 began from the state run 1 began with, as it would have beside it: it follows, and
    // what it left (beside another) is checked in no later run's state. Its line is without the
    // commits it began after and made (as one logged before they were numbered): its place and
    // its times say it ran beside run 1.
    let first: serde_json::Value = serde_json::from_str(&export.lines[1]).unwrap();
    let beside = rechained(&|i, v| {
        v.as_object_mut().unwrap().remove("commit");
        if i == 2 {
            v["state_before"] = first["state_before"].clone();
            v["at_ms"] = serde_json::json!(1_800_000_000_000u64 + 1001);
        }
    });
    let (began, beside_another, left, breaks) = states(&beside);
    assert_eq!((began, beside_another, breaks), (vec![1, 2], vec![2], vec![]));
    assert!(left.contains(&0) && !left.contains(&2), "{left:?}");

    // Run 0's state put back, and a run after it: it began from the state the restore put back.
    let restore = serde_json::json!({ "run": 0, "state": true });
    assert_eq!(route(&state, &req("POST", "/deployments/follows/restore", restore.to_string().into_bytes())).status_code, 200);
    assert_eq!(put(&state, "follows", "later").status_code, 200);
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/follows/export", vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    let s = report.states.as_ref().unwrap();
    assert_eq!((&s.began, &s.restored), (&vec![1, 2, 3], &vec![3]));
    assert!(verify::describe(&report, None).contains("(1 of them after a restore, from the state it put back)"));
    // Its event rewritten to say run 1's was put back: run 3 didn't begin from that.
    let mut changed = verify::Export { ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
    let restored = changed.events.iter().position(|l| l.contains("\"restore\"")).unwrap();
    changed.events[restored] = changed.events[restored].replace("\"run 0\"", "\"run 1\"");
    let s = verify::check(&changed, None, &BTreeMap::new()).states.unwrap();
    assert_eq!(s.breaks, vec![(3, "it began from a state other than the one the restore before it put back (run 1's)".to_string())]);
}

/// A deployment's runs, exported and checked offline: every run runs again to what its entry
/// says. A copy re-chained after changing what a run performed still links, but that run no longer
/// re-runs to its entry; a witnessed head is found only in the copy it came from.
#[test]
fn exported_runs_verify_and_a_rewritten_log_doesnt() {
    let mut state = test_state();
    let path = unique_temp_path("verified-log");
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "verified", writer, ack).status_code, 200);
    for body in ["hello", "there", "again"] {
        assert_eq!(put(&state, "verified", body).status_code, 200);
    }
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/verified/export", vec![])))).unwrap();
    assert_eq!(export.lines.len(), 3);
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert_eq!(report.reruns.iter().filter(|(_, r)| *r == verify::Rerun::Matched).count(), 3);
    assert_eq!(report.witnessed, Some(Some(2)));

    // Someone with the data folder rewrites what run 1 performed, and chains the lines again.
    let mut forged = verify::Export { lines: Vec::new(), ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
    let mut prev = String::new();
    for (i, line) in export.lines.iter().enumerate() {
        let mut v: serde_json::Value = serde_json::from_str(line).unwrap();
        if i == 1 {
            let performed = v["performed"].to_string().replace("there", "elsewhere");
            assert_ne!(performed, v["performed"].to_string());
            v["performed"] = serde_json::from_str(&performed).unwrap();
        }
        if !prev.is_empty() {
            v["prev"] = serde_json::Value::String(prev.clone());
        }
        let line = serde_json::to_string(&v).unwrap();
        prev = recorder::line_hash(&line);
        forged.lines.push(line);
    }
    forged.head = prev;
    let report = verify::check(&forged, Some(&export.head), &BTreeMap::new());
    assert!(report.breaks.is_empty(), "re-chained, it links");
    assert_eq!(report.reruns[1], (1, verify::Rerun::Differs("the effects it performed".into())));
    assert_eq!(report.witnessed, Some(None), "the witnessed head isn't in the rewritten log");
    assert!(!report.ok());

    route(&state, &req("POST", "/deployments/verified/stop", vec![]));
    route(&state, &req("DELETE", "/deployments/verified", vec![]));
    let _ = std::fs::remove_file(&path);
}

/// Removing a deployment keeps its run records (web console L4 §8.4): its log and timeline are
/// written first as an export `holon verify` checks, beside the logs, and the answer says where.
/// Deployed again, the slug starts a new chain: its first run names no line before it, and the
/// removed runs stay, checked as they were.
#[test]
fn a_removed_deployment_keeps_its_run_records_and_its_slug_starts_a_new_chain() {
    let mut state = test_state();
    let path = unique_temp_path("removed-log");
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "gone", writer, ack.clone()).status_code, 200);
    for body in ["hello", "there", "again"] {
        assert_eq!(put(&state, "gone", body).status_code, 200);
    }
    let head = body_json(route(&state, &req("GET", "/deployments/gone/log", vec![])))["head"].as_str().unwrap().to_string();
    assert_eq!(route(&state, &req("POST", "/deployments/gone/stop", vec![])).status_code, 200);
    let removed = body_json(route(&state, &req("DELETE", "/deployments/gone", vec![])));
    let kept = std::path::PathBuf::from(removed["records"].as_str().unwrap_or_else(|| panic!("where its records are: {removed}")));
    let logs = recorder::file_for(&Place::parse(&path), "gone").unwrap();
    assert_eq!(kept.parent(), Some(logs.with_file_name("removed").as_path()), "{}", kept.display());
    assert!(!logs.exists(), "its live log goes");

    let export = verify::read_export(&kept).unwrap();
    assert_eq!((export.lines.len(), export.head.as_str()), (3, head.as_str()));
    assert!(export.events.iter().any(|e| e.contains("\"deploy")), "its timeline: {:?}", export.events);
    let report = verify::check(&export, Some(&head), &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));

    // The slug deployed again: a new chain, the old records where they were.
    assert_eq!(deploy_json(&state, "gone", writer, ack).status_code, 200);
    assert!(log_entries(&state, "gone").is_empty(), "no run on its new record yet");
    assert_eq!(put(&state, "gone", "anew").status_code, 200);
    let fresh = log_entries(&state, "gone");
    assert_eq!((fresh.len(), fresh[0]["seq"].as_u64(), fresh[0].get("prev")), (1, Some(0), None), "{fresh:?}");
    assert_eq!(verify::read_export(&kept).unwrap().lines, export.lines);

    route(&state, &req("POST", "/deployments/gone/stop", vec![]));
    route(&state, &req("DELETE", "/deployments/gone", vec![]));
    let _ = std::fs::remove_dir_all(logs.parent().unwrap());
    let _ = std::fs::remove_file(&path);
}

/// A run that computes with a secret (here, signing the body under it) can't be computed again
/// from its entry: the log holds a placeholder, never the value. What the log kept for that node
/// stands in, and the rest of the run is checked against it: the entry with its answer rewritten
/// (and the line chained again) differs; one rewritten everywhere, that node's own output too,
/// passes, since what the node produced is taken from the log. The report says how many nodes.
#[test]
fn a_replay_cant_tell_a_run_that_computes_with_a_secret() {
    let state = claims_state("");
    let set = req("POST", "/deployments/signer/secrets/signing_key", serde_json::to_vec(&serde_json::json!({ "value": "whsec_test_123456" })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);
    let signer = |answer: &str| {
        format!("import {{ flow, hmac_sha256, route, status }} from \"holon\"
import signing_key from \"secret:signing_key\"

flow(\"signer\", {{ port: 18199 }})

route.post(\"/sign\", (req) => {answer})
")
    };
    let text = signer("hmac_sha256(signing_key, req.body) ?? status(400)");
    deploy_text(&state, &text, "127.0.0.1:9");
    assert_eq!(data_call(&state, "signer", "POST", "/sign", "hello").0, 200);
    let replay = |text: &str| {
        let unit = stage_text(&state, text);
        body_json(route(&state, &req("GET", &format!("/deployments/signer/replay?unit={}", to_hex(&unit)), vec![])))
    };
    // The log holds the key's placeholder: what the signing gives with it isn't the run's.
    let r = replay(&text);
    assert_eq!((&r["cant_tell"], &r["runs"][0]["why"][0]), (&serde_json::json!(1), &serde_json::json!("it computes with a secret, a token or a tainted value, which the log never holds")), "{r}");
    // One that passes the key along computes the same with its placeholder, and is told.
    let r = replay(&signer("({ signed: req.body, key: signing_key }).signed"));
    assert_eq!((&r["differ"], &r["runs"][0]["why"]), (&serde_json::json!(1), &serde_json::json!(["its answer"])), "{r}");
    assert_eq!((&r["runs"][0]["status"], &r["runs"][0]["now_status"]), (&serde_json::json!(200), &serde_json::json!(200)));
    let r = replay(&signer("status(503)"));
    assert_eq!(r["runs"][0]["why"][0], "it answers 503 where it answered 200", "{r}");
}

#[test]
fn a_run_that_computes_with_a_secret_is_checked_given_what_that_node_produced() {
    let state = claims_state("");
    let value = "whsec_test_123456";
    let set = req("POST", "/deployments/signer/secrets/signing_key", serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);
    let text = "import { flow, hmac_sha256, route, status } from \"holon\"\nimport signing_key from \"secret:signing_key\"\n\nflow(\"signer\", { port: 18199 })\n\nroute.post(\"/sign\", (req) => hmac_sha256(signing_key, req.body) ?? status(400))\n";
    let slug = deploy_text(&state, text, "127.0.0.1:9");
    assert_eq!(slug, "signer");
    let (status, signature, _) = data_call(&state, &slug, "POST", "/sign", "hello");
    assert_eq!(status, 200);
    let real = holon_core::primitives::compute("hmac_sha256", &[&Value::Text(value.into()), &Value::Text("hello".into())], &[Param::frozen("encoding", "hex")]);
    assert_eq!(real, Some(Value::Text(signature.clone())), "the run signed with the secret's value");

    // Exported, the run checks: the log holds the secret's placeholder, so the signing node's
    // recorded output stands in, and the answer built from it is the one logged.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/signer/export", vec![])))).unwrap();
    assert_eq!(export.lines.len(), 1);
    assert!(!export.lines[0].contains(value), "the log never holds the value");
    assert!(export.lines[0].contains(&signature), "it holds the signature the run answered");
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert_eq!(report.reruns, vec![(0, verify::Rerun::Matched)], "{}", verify::describe(&report, None));
    assert_eq!(report.given, BTreeMap::from([(0, 1)]), "one node stood in for: the signing");
    assert!(verify::describe(&report, None).contains("1 run: 0 checked given what the log kept for 1 node fed a secret"), "{}", verify::describe(&report, None));
    assert!(report.ok(), "{}", verify::describe(&report, None));

    let forged_signature: String = signature.chars().rev().collect();
    assert_ne!(forged_signature, signature);
    let rechained = |line: String| {
        let head = recorder::line_hash(&line);
        (verify::Export { lines: vec![line], head: head.clone(), ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() }, head)
    };
    // Someone with the data folder rewrites the answer the run gave, and chains the line again:
    // the signing node's recorded output still gives the real signature, so the run differs.
    let mut v: serde_json::Value = serde_json::from_str(&export.lines[0]).unwrap();
    let outputs = v["outputs"].to_string().replace(&signature, &forged_signature);
    assert_ne!(outputs, v["outputs"].to_string());
    v["outputs"] = serde_json::from_str(&outputs).unwrap();
    let (forged, head) = rechained(serde_json::to_string(&v).unwrap());
    let report = verify::check(&forged, Some(&head), &BTreeMap::new());
    assert!(report.breaks.is_empty(), "re-chained, it links");
    assert_eq!(report.reruns, vec![(0, verify::Rerun::Differs("its outputs".into()))], "{}", verify::describe(&report, None));
    assert!(!report.ok(), "the forged export fails: {}", verify::describe(&report, None));
    // Rewritten everywhere, the signing node's own recorded output too, it passes: what a node
    // fed a secret produced is taken from the log, not checked. That is the trust that remains.
    let (forged, head) = rechained(export.lines[0].replace(&signature, &forged_signature));
    let report = verify::check(&forged, Some(&head), &BTreeMap::new());
    assert_eq!(report.reruns, vec![(0, verify::Rerun::Matched)], "{}", verify::describe(&report, None));
    assert_eq!(report.given, BTreeMap::from([(0, 1)]));
}

/// A condition reads a value that isn't a Boolean as JavaScript does (`!""` is true, `[]` is
/// true), an `if` on an absent one runs neither branch, nor what follows one that returns, and a
/// status alone answers it with an empty body, a 204 with none.
#[test]
fn a_condition_reads_javascripts_truthiness_and_a_status_alone_answers() {
    let state = claims_state("");
    let text = "import { flow, route, status } from \"holon\"\nimport state from \"state\"\n\nflow(\"truthy\", { port: 18302 })\n\nroute.post(\"/not\", (req) => {\n  if (!req.json.s) {\n    return status(422, \"empty\")\n  }\n  return status(200, \"full\")\n})\n\nroute.post(\"/if\", (req) => {\n  if (req.json.s) {\n    state.yes = (state.yes ?? 0) + 1\n  } else {\n    state.no = (state.no ?? 0) + 1\n  }\n  return \"counted\"\n})\n\nroute.get(\"/stats\", () => ({ yes: state.yes ?? 0, no: state.no ?? 0 }))\n\nroute.post(\"/bare\", () => status(400))\n\nroute.post(\"/pick\", (req) => {\n  if (req.json.x === 1) {\n    return status(204)\n  }\n  if (req.json.x === 2) {\n    return status(409)\n  }\n  return status(200, { x: req.json.x })\n})\n";
    let slug = deploy_text(&state, text, "127.0.0.1:9");
    let call = |method: &str, path: &str, body: &str| {
        let (status, answer, _) = data_call(&state, &slug, method, path, body);
        (status, answer)
    };
    for (body, answer) in [(r#"{"s":""}"#, (422, "empty")), (r#"{"s":0}"#, (422, "empty")), (r#"{"s":false}"#, (422, "empty")), (r#"{"s":"a"}"#, (200, "full")), (r#"{"s":[]}"#, (200, "full")), (r#"{"s":{}}"#, (200, "full"))] {
        assert_eq!(call("POST", "/not", body), (answer.0, answer.1.to_string()), "{body}");
    }
    // Absent is still absent: `!` of it has no value, and the `if` runs neither branch; one of
    // them returns, so nothing after it runs either, and the route answers 404.
    assert_eq!(call("POST", "/not", "{}"), (404, r#"{"error":"not found"}"#.to_string()));
    for body in [r#"{"s":"x"}"#, r#"{"s":""}"#, r#"{"s":{}}"#, r#"{"s":0}"#, "{}"] {
        assert_eq!(call("POST", "/if", body), (200, "counted".to_string()), "{body}");
    }
    assert_eq!(call("GET", "/stats", ""), (200, r#"{"no":2,"yes":2}"#.to_string()), "each request counted once, on its side, but the one with no s");
    assert_eq!(call("POST", "/bare", ""), (400, String::new()));
    assert_eq!(call("POST", "/pick", r#"{"x":1}"#), (204, String::new()));
    assert_eq!(call("POST", "/pick", r#"{"x":2}"#), (409, String::new()));
    assert_eq!(call("POST", "/pick", r#"{"x":3}"#), (200, r#"{"x":3}"#.to_string()));
    assert_eq!(call("POST", "/pick", "{}"), (404, r#"{"error":"not found"}"#.to_string()), "no x: neither branch, nor the last return");
}

/// A caller who sends text that looks like a placeholder (`‹token:paid›`) gets nothing taken as
/// given: the log escapes it (only a mark the log wrote reads as one), and verify computes the
/// run's id and every node from it again. The caller's answer, and the request as the run saw it,
/// are the text it sent. Rewritten everywhere, the answer differs: nothing stood in for it.
#[test]
fn a_look_alike_of_a_placeholder_is_computed_with_not_taken_as_given() {
    let state = claims_state("");
    let text = "import { flow, route } from \"holon\"\n\nflow(\"echo\", { port: 18198 })\n\nroute.post(\"/echo\", (req) => `got ${req.body}`)\n";
    let slug = deploy_text(&state, text, "127.0.0.1:9");
    let (status, answer, _) = data_call(&state, &slug, "POST", "/echo", "‹token:paid›");
    assert_eq!((status, answer.as_str()), (200, "got ‹token:paid›"), "the caller gets what it sent");

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/echo/export", vec![])))).unwrap();
    assert_eq!(export.lines.len(), 1);
    assert!(export.lines[0].contains("got ‹‹token:paid›") && export.lines[0].contains("\"escaped\":true"), "{}", export.lines[0]);
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert_eq!(report.reruns, vec![(0, verify::Rerun::Matched)], "{}", verify::describe(&report, None));
    assert!(report.given.is_empty() && report.ids_given.is_empty(), "nothing taken as given: {}", verify::describe(&report, None));
    assert!(!verify::describe(&report, None).contains("given"), "{}", verify::describe(&report, None));

    let line = export.lines[0].replace("got ‹‹token:paid›", "got ‹‹token:other›");
    assert_ne!(line, export.lines[0]);
    let head = recorder::line_hash(&line);
    let forged = verify::Export { lines: vec![line], head: head.clone(), ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
    let report = verify::check(&forged, Some(&head), &BTreeMap::new());
    assert_eq!(report.reruns, vec![(0, verify::Rerun::Differs("its outputs".into()))], "{}", verify::describe(&report, None));
}

/// A run that received a real secret or a real token was given its id from the value, which the
/// log never holds: verify keeps the log's run id for it and says so, and checks the rest. Here a
/// bearer token equal to a secret, and a resume route's token in its path.
#[test]
fn a_run_that_received_a_secret_or_a_token_keeps_its_logged_run_id() {
    let state = claims_state("");
    let value = "tok_live_abcdef123";
    let set = req("POST", "/deployments/gate/secrets/api_token", serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);
    let text = "import { action, event, flow, route, secure_equals, status } from \"holon\"\nimport api_token from \"secret:api_token\"\nimport seen from \"collection:seen\"\n\nflow(\"gate\", { port: 18197 })\n\nroute.post(\"/me\", (req) => {\n  const ok = secure_equals(req.body, `Bearer ${api_token}`)\n  const started = visit(req.path)\n  return (ok ? `/paid/${started.waiting.paid}` : undefined) ?? status(401)\n})\n\nexport const visit = action(async (path) => {\n  const paid = await event(\"paid\", { expires: \"1h\" })\n  seen[\"last\"] = { at: path, via: paid.method }\n})\n\nroute.get(\"/paid/:token\", () => \"thanks\", { resumes: \"paid\" })\n";
    let slug = deploy_text(&state, text, "127.0.0.1:9");
    let (status, link, _) = data_call(&state, &slug, "POST", "/me", &format!("Bearer {value}"));
    assert_eq!(status, 200, "{link}");
    let (status, thanks, _) = data_call(&state, &slug, "GET", &link, "");
    assert_eq!((status, thanks.as_str()), (200, "thanks"));

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/gate/export", vec![])))).unwrap();
    let token = &link["/paid/".len()..];
    for l in &export.lines {
        assert!(!l.contains(value) && !l.contains(token), "the log holds neither the secret nor the token: {l}");
    }
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    let seqs: Vec<u64> = report.reruns.iter().map(|(seq, _)| *seq).collect();
    assert!(seqs.len() >= 3, "the request, the resume and the resumed part: {seqs:?}");
    let entry = |route: &str| export.lines.iter().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).find(|e| e["route"] == route).unwrap();
    // The run the request started parks, and is logged, before the request's run is.
    let (request, resume) = (entry("POST /me"), entry("GET /paid/:token"));
    assert!(report.ids_given.contains(&request["seq"].as_u64().unwrap()), "the request carried the secret: {}", verify::describe(&report, None));
    assert!(report.ids_given.contains(&resume["seq"].as_u64().unwrap()), "the resume carried the token: {}", verify::describe(&report, None));
    assert!(verify::describe(&report, None).contains("checked given the run id the log kept"), "{}", verify::describe(&report, None));
}

// ── key claims: a key read before a call is held until the run commits ───────────────────────
// (docs/L4s/key-claims-L4.md §10)

/// A local HTTP server that counts hits per path: `/slow` answers after 400 ms, the rest at once.
/// Lives as long as the test process.
fn counting_server() -> (String, Hits) {
    let (host, hits, _) = gated_counting_server();
    (host, hits)
}

/// A counting server's hits: path → how many requests came for it.
type Hits = Arc<Mutex<BTreeMap<String, usize>>>;

/// The longest a shut [`Gate`] holds a call: under the 30 s a run waits for an answer, so a run
/// held up behind a held call is let go by the gate, not by its own timeout, and fails its test
/// rather than hanging it.
const GATE_HOLDS: Duration = Duration::from_secs(20);

/// What a test holds `/slow` calls with, to see what happens while one is out without timing it:
/// on a loaded machine (a sweep on every core beside it) "it took under 780 ms" fails, where "both
/// calls were out at once" doesn't. Open, as it starts, a call answers after 400 ms; shut, once
/// the test opens it ([`GATE_HOLDS`] at most).
#[derive(Default)]
struct Gate {
    calls: Mutex<GateCalls>,
    changed: crate::sync::Condvar,
}

#[derive(Default)]
struct GateCalls {
    shut: bool,
    /// The `/slow` calls out now.
    out: usize,
}

impl Gate {
    fn shut(&self) {
        self.calls.lock().unwrap().shut = true;
    }

    fn open(&self) {
        self.calls.lock().unwrap().shut = false;
        self.changed.notify_all();
    }

    /// The `/slow` calls out now.
    fn out(&self) -> usize {
        self.calls.lock().unwrap().out
    }

    /// Whether `n` calls were out at once, waiting up to 15 s for them (less than a shut gate
    /// holds one, so none of them was let go meanwhile).
    fn out_at_once(&self, n: usize) -> bool {
        let calls = self.calls.lock().unwrap();
        self.changed.wait_timeout_while(calls, Duration::from_secs(15), |c| c.out < n).unwrap().0.out >= n
    }

    /// One `/slow` call, answered once this returns.
    fn pass(&self) {
        let mut calls = self.calls.lock().unwrap();
        calls.out += 1;
        self.changed.notify_all();
        if calls.shut {
            calls = self.changed.wait_timeout_while(calls, GATE_HOLDS, |c| c.shut).unwrap().0;
        } else {
            drop(calls);
            std::thread::sleep(Duration::from_millis(400));
            calls = self.calls.lock().unwrap();
        }
        calls.out -= 1;
        self.changed.notify_all();
    }
}

/// [`counting_server`], and the [`Gate`] its `/slow` calls pass.
fn gated_counting_server() -> (String, Hits, Arc<Gate>) {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let hits = Arc::new(Mutex::new(BTreeMap::new()));
    let counted = Arc::clone(&hits);
    let gate = Arc::new(Gate::default());
    let passing = Arc::clone(&gate);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let counted = Arc::clone(&counted);
            let passing = Arc::clone(&passing);
            std::thread::spawn(move || {
                let Some((head, _)) = read_request(&mut stream) else { return };
                let path = head.split_whitespace().nth(1).unwrap_or("").split('?').next().unwrap_or("").to_string();
                *counted.lock().unwrap().entry(path.clone()).or_insert(0) += 1;
                let body = match path.as_str() {
                    "/slow" => {
                        passing.pass();
                        "slow"
                    }
                    "/name" => "k",
                    // What a token exchange answers.
                    "/token" => r#"{"access_token":"tok_live_abcdef123456","token_type":"bearer","expires_in":3600}"#,
                    _ => "fast",
                };
                let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            });
        }
    });
    (host, hits, gate)
}

/// A secret computed into something else is kept out of the log as surely as its value is: a
/// Basic auth header (base64 of the key), a key with quotes and newlines sent inside JSON (escaped
/// there), and a field read out of a JSON secret. Redaction finds a value, not what is computed from
/// it, so the nodes it reaches keep no outputs and the requests built from them are withheld.
#[test]
fn a_secret_computed_into_something_else_stays_out_of_the_log() {
    let (host, _) = counting_server();
    let state = claims_state("");
    let text = format!(
        r#"import {{ base64, flow, route }} from "holon"
import api_key from "secret:api_key"
import pem from "secret:pem"
import service_account from "secret:service_account"
import host from "host:{host}"

flow("leaky", {{ port: 18141 }})

route.post("/go", async () => {{
  const basic = await host.get("/basic", {{
    headers: {{ authorization: `Basic ${{base64(api_key)}}` }},
  }})
  const account = JSON.parse(service_account)
  const sent = await host.post("/json", {{
    headers: {{ "content-type": "application/json", "x-email": account.client_email }},
    body: JSON.stringify({{ key: pem }}),
  }})
  return {{ basic: basic.status, sent: sent.status }}
}})
"#
    );
    let slug = deploy_text(&state, &text, &host);
    let set = |name: &str, value: &str| {
        let body = serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap();
        assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/secrets/{name}"), body)).status_code, 200, "{name}");
    };
    set("api_key", "sk_live_basic_secret_value");
    set("pem", "-----BEGIN KEY-----\nMIIabc\"quoted\"\n-----END KEY-----");
    set("service_account", r#"{"client_email":"bot@x.iam","private_key":"pk_private_value_123"}"#);
    let (status, body, _) = data_call(&state, &slug, "POST", "/go", "");
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, r#"{"basic":200,"sent":200}"#);

    let log = String::from_utf8(body_bytes(route(&state, &req("GET", &format!("/deployments/{slug}/log"), vec![])))).unwrap();
    assert!(!log.contains("c2tfbGl2ZV9iYXNp"), "base64 of the key: {log}");
    assert!(!log.contains("MIIabc"), "the escaped key: {log}");
    assert!(!log.contains("pk_private_value_123") && !log.contains("bot@x.iam"), "fields of the JSON secret: {log}");
    assert!(log.contains("‹withheld: computed from a secret›"), "the derived parts say they were withheld: {log}");
    assert!(log.contains("‹secret:pem v1 #"), "the escaped key is redacted where it stands: {log}");
    // The export says no more than the log.
    let export = String::from_utf8(body_bytes(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    assert!(!export.contains("c2tfbGl2ZV9iYXNp") && !export.contains("MIIabc") && !export.contains("pk_private_value_123"));
}

/// A secret goes only where the flow's approval shows it going. The flow sends `token` to one host
/// (its graph says so); a caller's body that holds the token's value, bound for another host, isn't
/// sent: the graph can't follow a value that came in from outside, but the host finds it by its
/// value. The export checks: the one request that left with it went where the approval shows, and
/// an export changed to say it went elsewhere is caught.
#[test]
fn a_secret_goes_only_where_its_approval_shows_it_going() {
    let (shown, shown_hits) = counting_server();
    let (other, other_hits) = counting_server();
    let state = claims_state("");
    let text = format!(
        r#"import {{ flow, route }} from "holon"
import token from "secret:token"
import shown from "host:{shown}"
import other from "host:{other}"

flow("sends", {{ port: 18143 }})

route.post("/go", async (req) => {{
  const authed = await shown.get("/api", {{ headers: {{ authorization: `Bearer ${{token}}` }} }})
  const passed = await other.post("/relay", {{ body: req.body }})
  return {{
    authed: authed.status,
    passed: passed.error ?? passed.status,
    message: passed.message ?? "",
  }}
}})
"#
    );
    let slug = deploy_text_to(&state, &text, &[&shown, &other]);
    let set = req("POST", &format!("/deployments/{slug}/secrets/token"), serde_json::to_vec(&serde_json::json!({ "value": "tok_live_sends_42" })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);

    // Something else passed on: sent.
    let (status, body, _) = data_call(&state, &slug, "POST", "/go", "hello");
    assert_eq!((status, body.as_str()), (200, r#"{"authed":200,"message":"","passed":200}"#));
    // The token's value passed on to the other host: not sent.
    let (status, body, _) = data_call(&state, &slug, "POST", "/go", "key=tok_live_sends_42");
    assert_eq!(status, 200, "{body}");
    let answer: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(answer["passed"], "blocked_secret", "{body}");
    let message = answer["message"].as_str().unwrap();
    assert!(message.contains(&format!("the secret \"token\" isn't sent to {other}")) && message.contains(&format!("https://{shown}")), "{message}");
    assert!(!message.contains("tok_live"), "{message}");
    assert_eq!((hits_of(&shown_hits, "/api"), hits_of(&other_hits, "/relay")), (2, 1), "the other host got only the first");

    // Exported, it checks: two requests carried the token (the second was refused before it left).
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert_eq!((report.secret_requests, report.secrets_outside.len()), (2, 0), "{}", verify::describe(&report, None));
    assert!(verify::describe(&report, None).contains("secrets: 2 outbound requests carried one, each to a host its approval shows it going to"));

    // Changed to say the first went to the other host, it is caught (and the chain breaks).
    let mut changed: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    changed.lines[0] = changed.lines[0].replace(&format!("{shown}/api"), &format!("{other}/api"));
    let report = verify::check(&changed, None, &BTreeMap::new());
    assert!(!report.ok());
    assert_eq!(report.secrets_outside, vec![(0, "token".to_string(), other.clone())], "{}", verify::describe(&report, None));
    assert!(verify::describe(&report, None).contains(&format!("run 0 SENT the secret \"token\" to {other}, where its approval doesn't show it going")));
}

/// The server's own secret, bound once for every deployment that reads it, with where it may go:
/// a flow that sends it there deploys and runs with it; one that sends it elsewhere is refused at
/// deploy, unless it has its own of that name; and a cap narrowed after deploy holds from the next
/// request. The deployment's list says it reads the server's, and never a value.
#[test]
fn the_servers_secret_goes_only_where_an_admin_said_it_may() {
    let (shown, shown_hits) = counting_server();
    let (other, other_hits) = counting_server();
    let state = claims_state("");
    let bind = serde_json::json!({ "value": "srv_live_shared_77", "to": [shown] });
    let r = route(&state, &req("POST", "/admin/secrets/api_key", serde_json::to_vec(&bind).unwrap()));
    let status = r.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(r)));
    let flow = |name: &str, port: u16, host: &str| {
        format!(
            r#"import {{ flow, route }} from "holon"
import api_key from "secret:api_key"
import api from "host:{host}"

flow("{name}", {{ port: {port} }})

route.get("/go", async () => {{
  const called = await api.get("/api", {{ headers: {{ authorization: `Bearer ${{api_key}}` }} }})
  return {{ status: called.error ?? called.status, message: called.message ?? "" }}
}})
"#
        )
    };

    // Sent where the admin said: it deploys, and the run sends the server's value.
    let slug = deploy_text_to(&state, &flow("uses-shared", 18191, &shown), &[&shown]);
    let (status, body, _) = data_call(&state, &slug, "GET", "/go", "");
    assert_eq!((status, body.as_str()), (200, r#"{"message":"","status":200}"#));
    assert_eq!(hits_of(&shown_hits, "/api"), 1);
    let listed = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/secrets"), vec![])));
    assert_eq!(listed["secrets"][0]["name"], "api_key");
    assert_eq!(listed["secrets"][0]["server"], true, "{listed}");
    assert_eq!(listed["secrets"][0]["to"], serde_json::json!([shown]), "{listed}");
    assert!(!listed.to_string().contains("srv_live"), "never a value: {listed}");

    // Sent elsewhere: refused, in the preview and at deploy.
    let elsewhere = holon_author::agent::plan(&flow("sends-elsewhere", 18192, &other)).unwrap();
    for u in &elsewhere.units {
        route(&state, &req("POST", "/unit", u.clone()));
    }
    let requirements = serde_json::json!({ "plain_http": [other], "private_addresses": true });
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "sends-elsewhere", "unit": elsewhere.unit, "requirements": requirements })).unwrap())));
    let refused = preview["refused"].as_str().unwrap_or_else(|| panic!("{preview}"));
    assert!(refused.contains(&format!("the server's secret \"api_key\" may go only to https://{shown}, and this flow sends it to https://{other}")), "{refused}");
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap();
    let r = deploy_json(&state, "sends-elsewhere", parse_hash(&elsewhere.unit).unwrap(), serde_json::json!({ "acknowledge": asked, "requirements": requirements }));
    assert_eq!(r.status_code, 400);
    // With its own api_key, it isn't held to the server's.
    let own = serde_json::json!({ "value": "own_live_key_88" });
    assert_eq!(route(&state, &req("POST", "/deployments/sends-elsewhere/secrets/api_key", serde_json::to_vec(&own).unwrap())).status_code, 200);
    let r = deploy_json(&state, "sends-elsewhere", parse_hash(&elsewhere.unit).unwrap(), serde_json::json!({ "acknowledge": asked, "requirements": requirements }));
    let status = r.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(r)));
    let (_, body, _) = data_call(&state, "sends-elsewhere", "GET", "/go", "");
    assert_eq!(body, r#"{"message":"","status":200}"#, "its own goes where its approval shows");
    assert_eq!(hits_of(&other_hits, "/api"), 1);

    // Narrowed after deploy: the next request with the server's isn't sent.
    let cap = serde_json::json!({ "to": [] });
    assert_eq!(route(&state, &req("POST", "/admin/secrets/api_key/cap", serde_json::to_vec(&cap).unwrap())).status_code, 200);
    let (_, body, _) = data_call(&state, &slug, "GET", "/go", "");
    let answer: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(answer["status"], "blocked_secret", "{body}");
    assert!(answer["message"].as_str().unwrap().contains(&format!("the server's secret \"api_key\" isn't sent to {shown}: an admin said it may go only to nowhere")), "{body}");
    assert_eq!(hits_of(&shown_hits, "/api"), 1, "not sent");

    // The admin's list says where it may go.
    let bound = body_json(route(&state, &req("GET", "/admin/secrets", vec![])));
    assert_eq!(bound["secrets"][0]["to"], serde_json::json!([]), "{bound}");
    // Unbound, the deployment reads nothing for it.
    assert_eq!(route(&state, &req("DELETE", "/admin/secrets/api_key", vec![])).status_code, 204);
    let listed = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/secrets"), vec![])));
    assert_eq!(listed["secrets"], serde_json::json!([]), "{listed}");
}

/// A token a call answers to a request that held a secret (an OAuth exchange) is as good as the
/// secret until it expires: kept out of the log by its value, with a fingerprint like a secret's,
/// and out of state, as a secret is. What the caller needs it for still works: the run sends it
/// on, the log shows the same mark there, and the run checks; a run that tried to keep it is
/// skipped by verify, its write refused. The same answer to a request with no secret is clean.
#[test]
fn a_token_a_call_answers_with_a_secret_is_kept_out_of_the_log_and_out_of_state() {
    let (host, _) = counting_server();
    let state = claims_state("");
    let text = format!(
        r#"import {{ flow, route }} from "holon"
import state from "state"
import client_secret from "secret:client_secret"
import host from "host:{host}"

flow("oauth", {{ port: 18150 }})

route.post("/call", async (req) => {{
  const got = await host.post("/token", {{
    headers: {{ "x-client-secret": client_secret }},
    body: req.body,
  }})
  const parsed = JSON.parse(got.body)
  const used = await host.get("/name", {{ headers: {{ authorization: parsed.access_token }} }})
  return {{ used: used.status, expires: parsed.expires_in }}
}})

route.post("/cache", async (req) => {{
  const got = await host.post("/token", {{
    headers: {{ "x-client-secret": client_secret }},
    body: req.body,
  }})
  const parsed = JSON.parse(got.body)
  state.cached = parsed.access_token
  state.expires = parsed.expires_in
  return "cached"
}})

route.post("/open", async (req) => {{
  const got = await host.post("/token", {{ body: req.body }})
  const parsed = JSON.parse(got.body)
  state.open_token = parsed.access_token
  return "open"
}})
"#
    );
    let slug = deploy_text(&state, &text, &host);
    let set = req("POST", &format!("/deployments/{slug}/secrets/client_secret"), serde_json::to_vec(&serde_json::json!({ "value": "cs_live_client_secret_1" })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);
    let token = "tok_live_abcdef123456";

    let (status, body, _) = data_call(&state, &slug, "POST", "/call", "grant");
    assert_eq!((status, body.as_str()), (200, r#"{"expires":3600,"used":200}"#), "the run used the token");
    let (status, body, _) = data_call(&state, &slug, "POST", "/cache", "grant");
    assert_eq!((status, body.as_str()), (200, "cached"));
    data_call(&state, &slug, "POST", "/open", "grant");

    let current = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/state"), vec![])));
    assert!(current["state"].get("cached").is_none(), "the token's write was refused: {current}");
    assert!(current["state"].get("expires").is_some(), "its other write went through: {current}");
    assert!(current["state"].get("open_token").is_some(), "a token from a call that sent no secret is an ordinary answer: {current}");

    let log = String::from_utf8(body_bytes(route(&state, &req("GET", &format!("/deployments/{slug}/log"), vec![])))).unwrap();
    let entries = log_entries(&state, &slug);
    let line = |at: usize| entries[at].to_string();
    assert!(!line(0).contains(token) && !line(1).contains(token), "the token is not in the log: {log}");
    assert!(line(2).contains(token), "a token no secret fetched is an ordinary value: {}", line(2));
    let mark = |l: &str| {
        let at = l.find("‹token:answer access_token #").unwrap_or_else(|| panic!("the token's mark: {l}"));
        l[at..l[at..].find('›').map(|e| at + e + '›'.len_utf8()).unwrap()].to_string()
    };
    let (m0, m1) = (mark(&line(0)), mark(&line(1)));
    assert_eq!(m0, m1, "the same token reads the same in both runs");
    assert!(line(0).matches(&m0).count() >= 2, "in the answer, and where the run sent it on: {}", line(0));

    // Exported, the run that used it checks, and the one that tried to keep it is skipped.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    assert!(!export.lines[0].contains(token) && !export.lines[1].contains(token));
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert_eq!(report.reruns[0], (0, verify::Rerun::Matched), "{}", verify::describe(&report, None));
    assert!(matches!(&report.reruns[1], (1, verify::Rerun::Skipped(why)) if why.contains("wrote a secret's value")), "{}", verify::describe(&report, None));
    assert!(report.ok(), "{}", verify::describe(&report, None));
    // Where the token went: to the host its approval shows what is answered to the secret going.
    assert_eq!((report.token_requests, report.tokens_outside.len()), (1, 0), "{}", verify::describe(&report, None));
    assert!(verify::describe(&report, None).contains("tokens: 1 outbound request carried what a call answered to a secret, each to a host its approval shows that going to"));
    // Changed to say it went elsewhere, it is caught.
    let mut changed = export;
    changed.lines[0] = changed.lines[0].replace(&format!("{host}/name"), "elsewhere.test/name");
    let report = verify::check(&changed, None, &BTreeMap::new());
    assert_eq!(report.tokens_outside, vec![(0, "answer access_token".to_string(), "elsewhere.test".to_string())], "{}", verify::describe(&report, None));
    assert!(verify::describe(&report, None).contains("run 0 SENT a token (answer access_token) to elsewhere.test, where its approval doesn't show what is answered to its secret going"));
}

/// A route that answers with a secret, or with what is computed from it, asks for it at deploy
/// (`SecretAnswer("token")`), where an hmac of one asks nothing; a deployment not approved for it
/// answers 500 and its log says why, and once approved the caller gets the answer.
#[test]
fn a_route_answers_with_a_secret_only_where_a_person_approved_it() {
    let state = claims_state("");
    let text = "import { base64, flow, hmac_sha256, route, status } from \"holon\"\nimport key from \"secret:key\"\nimport token from \"secret:token\"\n\nflow(\"answers\", { port: 18178 })\n\nroute.get(\"/plain\", () => token)\n\nroute.get(\"/encoded\", () => base64(token))\n\nroute.post(\"/sign\", (req) => hmac_sha256(key, req.body) ?? status(400))\n";
    let plan = holon_author::agent::plan(text).unwrap();
    for u in &plan.units {
        route(&state, &req("POST", "/unit", u.clone()));
    }
    let unit = parse_hash(&plan.unit).unwrap();
    for (name, value) in [("token", "tok_live_987654321"), ("key", "whsec_test_123456")] {
        let set = req("POST", &format!("/deployments/answers/secrets/{name}"), serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap());
        assert_eq!(route(&state, &set).status_code, 200);
    }
    let answer = "SecretAnswer(\"token\")".to_string();
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "answers", "unit": plan.unit })).unwrap())));
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap();
    assert!(asked.contains(&answer), "{asked:?}");
    assert!(!asked.iter().any(|a| a.starts_with("SecretAnswer(\"key\"")), "an hmac of a secret is no answer with it: {asked:?}");
    let all_but: Vec<&String> = asked.iter().filter(|a| **a != answer).collect();

    // Not acknowledged, it isn't deployed.
    assert_ne!(deploy_json(&state, "answers", unit, serde_json::json!({ "acknowledge": all_but })).status_code, 200);

    // Approved, the caller gets it.
    let r = deploy_json(&state, "answers", unit, serde_json::json!({ "acknowledge": asked }));
    let status = r.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(r)));
    let (status, body, _) = data_call(&state, "answers", "GET", "/plain", "");
    assert_eq!((status, body.as_str()), (200, "tok_live_987654321"));

    // Taken back, the deployment kept running: the answers that would carry it are refused.
    let view = body_json(change_grant(&state, "answers", serde_json::json!({ "revoke": [answer], "keep_running": true })));
    assert_eq!(view["status"], "running");
    for path in ["/plain", "/encoded"] {
        let (status, body, _) = data_call(&state, "answers", "GET", path, "");
        assert_eq!(status, 500, "{path}: {body}");
        assert!(body.contains("may not answer its callers with the secret \\\"token\\\"") && !body.contains("tok_live"), "{path}: {body}");
    }
    assert_eq!(data_call(&state, "answers", "POST", "/sign", "hello").0, 200, "the hmac answers");
    let entries = log_entries(&state, "answers");
    let refused = entries.iter().rev().find(|e| e["route"] == "GET /plain").unwrap();
    assert!(refused["error"].as_str().unwrap().contains("refused to answer with the secret \"token\""), "{refused}");
    assert!(!serde_json::to_string(&entries).unwrap().contains("tok_live"), "the log never holds it");
}

/// The deploy preview says where each secret is sent, beside the items it asks for: the Slack
/// path goes to Slack; the Stripe secret only verifies a signature, so it goes nowhere. The plan
/// the CLI and the agent tools deploy from says the same.
#[test]
fn the_preview_says_where_each_secret_is_sent() {
    let state = claims_state("");
    let plan = holon_author::agent::plan(include_str!("../../holon-author/samples/stripe-to-slack.holon.ts")).unwrap();
    for u in &plan.units {
        route(&state, &req("POST", "/unit", u.clone()));
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "stripe", "unit": plan.unit })).unwrap())));
    assert_eq!(preview["sent"], serde_json::json!({ "slack_webhook_path": ["hooks.slack.com"] }));
    assert_eq!(serde_json::to_value(&plan.sent).unwrap(), preview["sent"]);
    let said = holon_author::agent::describe_sending("SecretRead(\"slack_webhook_path\")", &plan.sent);
    assert_eq!(said, "read the secret \"slack_webhook_path\", and send it to https://hooks.slack.com");
}

/// What is computed from a secret isn't kept: a state, collection or channel write of it is
/// refused, as a write of its value is (it would sit on disk, in every later snapshot, and in the
/// channel's packs), while the run's other writes go through; and the run's answer, though the
/// caller gets it (the deploy approved `SecretAnswer`), is withheld from the log, its output and
/// its response body alike. Verify skips such a run rather than calling it changed.
#[test]
fn what_is_computed_from_a_secret_is_neither_kept_nor_logged() {
    let state = claims_state("");
    let text = r#"import { base64, flow, route } from "holon"
import state from "state"
import api_key from "secret:api_key"
import kept from "collection:kept"
import news from "channel:news"

flow("derived", { port: 18144 })

route.post("/go", (req) => {
  const token = `${req.body}:${base64(api_key)}`
  state.token = token
  kept["k"] = token
  news.send({ t: token })
  state.plain = req.body
  return { echoed: token }
})
"#;
    let slug = deploy_text(&state, text, "x");
    let value = "sk_live_derived_secret_value";
    let set = req("POST", &format!("/deployments/{slug}/secrets/api_key"), serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);
    let encoded = "c2tfbGl2ZV9kZXJpdmVkX3NlY3JldF92YWx1ZQ==";
    let (status, body, _) = data_call(&state, &slug, "POST", "/go", "hello");
    assert_eq!((status, body), (200, format!(r#"{{"echoed":"hello:{encoded}"}}"#)), "the caller gets its answer");

    let current = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/state"), vec![])));
    assert!(current["state"].get("token").is_none(), "the derived write was refused: {current}");
    assert!(current["state"].get("plain").is_some(), "the clean one went through: {current}");
    let items = route(&state, &req("GET", &format!("/deployments/{slug}/collections/kept"), vec![]));
    assert!(items.status_code == 404 || body_json(items)["items"].as_array().unwrap().is_empty(), "nothing put in the collection");

    let entries = log_entries(&state, &slug);
    let entry = entries.last().unwrap();
    let performed: Vec<&str> = entry["performed"].as_array().unwrap().iter().map(|p| p["capability"].as_str().unwrap()).collect();
    assert_eq!(performed, [r#"StateWrite("plain")"#], "no channel send to rebuild packs from: {entry}");
    assert_eq!(entry["response_body"], "‹withheld: computed from a secret›", "{entry}");
    let log = String::from_utf8(body_bytes(route(&state, &req("GET", &format!("/deployments/{slug}/log"), vec![])))).unwrap();
    assert!(!log.contains("c2tfbGl2ZV9kZXJp"), "base64 of the key: {log}");

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert!(matches!(report.reruns[..], [(0, verify::Rerun::Skipped(_))]), "{}", verify::describe(&report, None));
    assert!(report.ok(), "{}", verify::describe(&report, None));
}

/// The read-call-write flows the claim tests run, calling `host` over plain HTTP: the call is
/// written in the `??` it falls back to, so it is made only when there is no prior record.
fn idem_text(host: &str) -> String {
    format!(
        r#"import {{ action, event, flow, route }} from "holon"
import seen from "collection:seen"
import host from "host:{host}"

flow("idem", {{ port: 18131 }})

route.post("/go/:k", async (req) => {{
  const prior = seen[req.params.k]
  const result = prior ?? {{ got: (await host.get("/slow")).body }}
  seen[req.params.k] = result
  return result
}})

route.put("/set/:k", (req) => {{
  seen[req.params.k] = {{ got: req.body }}
  return {{ set: req.body }}
}})

route.get("/seen/:k", (req) => seen[req.params.k])

route.post("/chain", async (req) => {{
  const mine = seen["b"] ?? "none"
  const name = (await host.get("/name")).body
  const other = seen[name] ?? "nobody"
  const done = (await host.get("/fast", {{ headers: {{ "x-other": JSON.stringify(other) }} }})).body
  seen["b"] = {{ mine, other, done, who: req.body }}
  return {{ other }}
}})

route.post("/hold/:k", (req) => {{
  const started = hold(req.params.k)
  return started.waiting.paid
}})

export const hold = action(async (k) => {{
  const prior = seen[k]
  const got = (await host.get("/fast")).body
  const paid = await event("paid", {{ expires: "1h" }})
  seen[k] = {{ got, paid: paid.body, prior: prior ?? "none" }}
}})

route.get("/paid/:token", () => "ok", {{ resumes: "paid" }})
"#
    )
}

/// A test state whose policy lets a flow call a local plain-HTTP host, with `extra` policy lines.
fn claims_state(extra: &str) -> AppState {
    let state = test_state();
    let policy = format!("[grants]\nallow_plain_http = true\nallow_private_addresses = true\n{extra}");
    *state.policy.write().unwrap() = Arc::new(Policy::parse(&policy).unwrap());
    state
}

/// Compile `text` with the language, stage its units, and deploy it acknowledging what it asks;
/// its slug.
fn deploy_text(state: &AppState, text: &str, host: &str) -> String {
    deploy_text_to(state, text, &[host])
}

/// The same, for a flow that calls several local hosts.
fn deploy_text_to(state: &AppState, text: &str, hosts: &[&str]) -> String {
    let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
    for u in &plan.units {
        assert_eq!(route(state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let unit = parse_hash(&plan.unit).unwrap();
    let requirements = serde_json::json!({ "plain_http": hosts, "private_addresses": true });
    let preview = body_json(route(
        state,
        &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit, "requirements": requirements })).unwrap()),
    ));
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap_or_default();
    let r = deploy_json(state, &plan.name, unit, serde_json::json!({ "acknowledge": asked, "requirements": requirements }));
    let status = r.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(r)));
    plan.name
}

/// A data-plane request to `slug`: its status, body and how long it took.
fn data_call(state: &AppState, slug: &str, method: &str, url: &str, body: &str) -> (u16, String, Duration) {
    let t0 = std::time::Instant::now();
    let r = serve_data(&state.store, &state.deployments, &state.deployments_path, slug, &fake_data(method, url, body.as_bytes().to_vec()));
    (r.status, String::from_utf8(r.body).unwrap(), t0.elapsed())
}

fn log_entries(state: &AppState, slug: &str) -> Vec<serde_json::Value> {
    body_json(route(state, &req("GET", &format!("/deployments/{slug}/log"), vec![])))["entries"].as_array().unwrap().clone()
}

/// The oracle a deployment's runs of every kind are held to: replayed on the version it runs now
/// (`GET /deployments/<slug>/replay`), a run of that version that `holon verify` runs again as it
/// ran replays as the same, and none replays differently. The kinds of the runs replayed as the
/// same, for the caller to say which it made.
fn replays_as_it_verifies(state: &AppState, slug: &str) -> BTreeSet<String> {
    let export: verify::Export = serde_json::from_value(body_json(route(state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    let unit = to_hex(&state.deployments.lock().unwrap()[slug].unit);
    let replayed = body_json(route(state, &req("GET", &format!("/deployments/{slug}/replay?unit={unit}&runs=200"), vec![])));
    let of_now: BTreeSet<u64> = export
        .lines
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["unit"] == unit)
        .filter_map(|e| e["seq"].as_u64())
        .collect();
    let mut same = BTreeSet::new();
    for run in replayed["runs"].as_array().unwrap().iter().filter(|r| r["seq"].as_u64().is_some_and(|s| of_now.contains(&s))) {
        let seq = run["seq"].as_u64().unwrap();
        assert_ne!(run["outcome"], "differs", "{slug}'s run {seq} replays differently on its own version: {run}");
        if report.reruns.iter().any(|(s, r)| *s == seq && *r == verify::Rerun::Matched) {
            assert_eq!(run["outcome"], "same", "{slug}'s run {seq} verifies, so it replays as the same: {run}");
            same.insert(run["kind"].as_str().unwrap().to_string());
        }
    }
    same
}

fn hits_of(hits: &Mutex<BTreeMap<String, usize>>, path: &str) -> usize {
    hits.lock().unwrap().get(path).copied().unwrap_or(0)
}

/// Two requests with one key at once: the second waits for the first's claim, finds the first's
/// record, and sends nothing; both answer the same; the processor saw one call.
#[test]
fn a_racing_pair_on_one_key_calls_out_once() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let slug = deploy_text(&state, &idem_text(&host), &host);
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| data_call(&state, &slug, "POST", "/go/k2", ""));
        std::thread::sleep(Duration::from_millis(60));
        let b = s.spawn(|| data_call(&state, &slug, "POST", "/go/k2", ""));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!((a.0, b.0), (200, 200), "{a:?} {b:?}");
    assert_eq!(a.1, b.1, "both answer the first's record");
    assert_eq!(hits_of(&hits, "/slow"), 1, "one call for one key");
    let entries = log_entries(&state, &slug);
    let second = entries.iter().find(|e| e["retries"] == 1).unwrap_or_else(|| panic!("one run ran again: {entries:#?}"));
    let claims = second["claims"].as_array().unwrap();
    assert!(claims.iter().any(|c| c["key"] == "seen[k2]" && c["waited_ms"].as_f64().unwrap() > 100.0 && c["overtaken"] == true), "{claims:?}");
    let first = entries.iter().find(|e| e["retries"] != 1).unwrap();
    let held = &first["claims"][0];
    assert_eq!((held["key"].as_str(), held["overtaken"].as_bool()), (Some("seen[k2]"), Some(false)), "{held}");
    assert!(held["waited_ms"].as_f64().unwrap() < 50.0 && held["held_ms"].as_f64().unwrap() > 300.0, "held through its slow call: {held}");
}

/// Two keys never wait for each other: both call out, side by side.
#[test]
fn different_keys_never_wait() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let slug = deploy_text(&state, &idem_text(&host), &host);
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| data_call(&state, &slug, "POST", "/go/k3", ""));
        std::thread::sleep(Duration::from_millis(60));
        let b = s.spawn(|| data_call(&state, &slug, "POST", "/go/k4", ""));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!((a.0, b.0), (200, 200));
    assert_eq!(hits_of(&hits, "/slow"), 2);
    assert!(a.2 < Duration::from_millis(700) && b.2 < Duration::from_millis(700), "not serialized: {:?} {:?}", a.2, b.2);
    for e in log_entries(&state, &slug) {
        assert_eq!(e["retries"], serde_json::Value::Null, "{e}");
        for c in e["claims"].as_array().into_iter().flatten() {
            assert!(c["waited_ms"].as_f64().unwrap() < 50.0, "{c}");
        }
    }
}

/// Two runs beside each other that write different keys of one collection commit one on the
/// other, and a run after both begins from what the second left. Each line is numbered after its
/// commit, outside the state's lock, so the two may be numbered the other way round: `holon
/// verify` takes the run a run began after from its entry's commit numbers, not the line before
/// it, and the three follow either way. Without the numbers (a log written before them) the run
/// after both broke; with them, a state the run it began after didn't leave still breaks.
#[test]
fn runs_beside_each_other_numbered_the_other_way_round_follow() {
    let (host, _) = counting_server();
    let state = claims_state("");
    let slug = deploy_text(&state, &idem_text(&host), &host);
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| data_call(&state, &slug, "POST", "/go/k6", ""));
        std::thread::sleep(Duration::from_millis(60));
        let b = s.spawn(|| data_call(&state, &slug, "POST", "/go/k7", ""));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!((a.0, b.0), (200, 200));
    assert_eq!(data_call(&state, &slug, "PUT", "/set/k8", "after").0, 200);
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert_eq!(report.states.as_ref().unwrap().breaks, vec![]);

    // Both began from the state as deployed (/slow takes 400 ms) and landed in turn; the run
    // after them began after the second.
    let lines: Vec<serde_json::Value> = export.lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    let commit = |v: &serde_json::Value| (v["commit"]["after"].as_u64().unwrap(), v["commit"]["n"].as_u64().unwrap());
    let (first, second) = if commit(&lines[0]).1 == 1 { (&lines[0], &lines[1]) } else { (&lines[1], &lines[0]) };
    assert_eq!((commit(first), commit(second), commit(&lines[2])), ((0, 1), (0, 2), (2, 3)));
    assert_eq!(lines[2]["commit"]["cell"], first["commit"]["cell"]);

    // Numbered the other way round: the second to land first, the two beside each other, the
    // run after them two seconds on; each changed by `edit` and chained again.
    let swapped = |edit: &dyn Fn(usize, &mut serde_json::Value)| {
        let mut copy = verify::Export { lines: Vec::new(), ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
        let mut prev = String::new();
        for (i, (v, at)) in [(second, 0), (first, 60), (&lines[2], 2000)].into_iter().enumerate() {
            let mut v = v.clone();
            v["seq"] = serde_json::json!(i);
            v["at_ms"] = serde_json::json!(1_800_000_000_000u64 + at);
            v["duration_ms"] = serde_json::json!(if at == 2000 { 2.0 } else { 500.0 });
            edit(i, &mut v);
            v["prev"] = serde_json::Value::String(prev.clone());
            if prev.is_empty() {
                v.as_object_mut().unwrap().remove("prev");
            }
            let line = serde_json::to_string(&v).unwrap();
            prev = recorder::line_hash(&line);
            copy.lines.push(line);
        }
        copy.head = prev;
        copy
    };
    let swapped_states = |e: &verify::Export| {
        let report = verify::check(e, None, &BTreeMap::new());
        let text = verify::describe(&report, None);
        assert!(report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{text}");
        (report.states.unwrap(), text)
    };
    let (s, text) = swapped_states(&swapped(&|_, _| {}));
    assert_eq!(s.breaks, vec![], "{text}");
    // Run 2 began from what run 0 left, its line before the one before it; run 1 beside run 0,
    // from the state run 0 began with; run 0 left its key as it wrote it, on run 1's.
    assert_eq!((&s.began, &s.beside), (&vec![1, 2], &vec![1]), "{text}");
    assert!(s.left.contains(&0), "{text}");

    // The same lines without their numbers, as a log written before them: run 2 broke.
    let (s, _) = swapped_states(&swapped(&|_, v| drop(v.as_object_mut().unwrap().remove("commit"))));
    assert_eq!(s.breaks, vec![(2, "it began from a state the run before it (run 1) didn't leave, nor a run beside it".to_string())]);

    // Run 0 says it left another state: run 2 began after it, from a state it didn't leave.
    let (s, text) = swapped_states(&swapped(&|i, v| {
        if i == 0 {
            v["state_after"] = serde_json::json!("00".repeat(32));
        }
    }));
    assert_eq!(s.breaks, vec![(2, "it began from a state the run before it (run 1) didn't leave, nor a run beside it".to_string())], "{text}");
    assert!(text.contains("run 2's state DOESN'T FOLLOW"), "{text}");
}

/// A log trimmed to its newest runs, with runs beside each other: a run that began after a commit
/// whose line is gone, before the first commit here landed, has no run before it here, as the
/// first hasn't, and isn't checked where it began. Without the commit numbers it broke.
#[test]
fn a_run_after_a_commit_the_trimmed_log_lost_has_no_run_before_it() {
    let (host, _) = counting_server();
    let state = claims_state("");
    let slug = deploy_text(&state, &idem_text(&host), &host);
    assert_eq!(data_call(&state, &slug, "PUT", "/set/k1", "p").0, 200);
    // X calls out (400 ms); Q lands meanwhile, and Y begins from what Q left and calls out too.
    let (x, y) = std::thread::scope(|s| {
        let x = s.spawn(|| data_call(&state, &slug, "POST", "/go/k6", ""));
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(data_call(&state, &slug, "PUT", "/set/k2", "q").0, 200);
        let y = s.spawn(|| data_call(&state, &slug, "POST", "/go/k7", ""));
        (x.join().unwrap(), y.join().unwrap())
    });
    assert_eq!((x.0, y.0), (200, 200));
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let lines: Vec<serde_json::Value> = export.lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    let commits: Vec<(u64, u64)> = lines.iter().map(|v| (v["commit"]["after"].as_u64().unwrap(), v["commit"]["n"].as_u64().unwrap())).collect();
    assert_eq!(commits, vec![(0, 1), (1, 2), (1, 3), (2, 4)], "P, Q, then X and Y as they landed");
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.states.as_ref().unwrap().breaks.is_empty(), "{}", verify::describe(&report, None));

    // Trimmed to X and Y, as retention trims a log: Y began after Q's commit, whose line is gone.
    let trimmed = |lines: Vec<String>| verify::Export { lines, ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
    let report = verify::check(&trimmed(export.lines[2..].to_vec()), None, &BTreeMap::new());
    let text = verify::describe(&report, None);
    assert!(report.ok(), "{text}");
    let s = report.states.unwrap();
    assert_eq!((s.breaks, &s.unchecked["where it began: no run before it here"]), (vec![], &vec![2, 3]), "{text}");

    // The same two without their numbers, as a log written before them, chained again: Y broke.
    let mut x = lines[2].clone();
    x.as_object_mut().unwrap().remove("commit");
    let x = serde_json::to_string(&x).unwrap();
    let mut y = lines[3].clone();
    y.as_object_mut().unwrap().remove("commit");
    y["prev"] = serde_json::Value::String(recorder::line_hash(&x));
    let y = serde_json::to_string(&y).unwrap();
    let report = verify::check(&verify::Export { head: recorder::line_hash(&y), ..trimmed(vec![x, y]) }, None, &BTreeMap::new());
    assert_eq!(report.states.unwrap().breaks, vec![(3, "it began from a state the run before it (run 2) didn't leave, nor a run beside it".to_string())]);
}

/// **Guards:** `holon verify` tells a run the server couldn't write (a full disk) from one
/// removed: the lost one reads as a run not in the file, which the line after it says, the chain
/// unbroken, the export passing, and the run after it unchecked where it began (it may have begun
/// from what the lost run left), not a break; a run removed reads as a break, or, the lines after
/// it chained again, as runs MISSING that no line says were lost; and one removed, the rest chained
/// again saying it lost, reads as a loss, which only the witness's head, then not found, tells.
///
/// **Why:** a line lost and a line removed leave the same hole. What tells them apart is that the
/// server says each it lost in the next line, which the witness holds, and numbers runs one after
/// another; each row is a rewrite someone who can write the file could make. The recorder's model
/// test holds the chain and the runs said lost over every order of failures; what a lost run does
/// to the state check only a deployment with state shows.
///
/// **If it fails:** a full disk reads as the log, or its state, changed outside holon; or a payout's
/// line removed and the rest chained again passes as a disk that was full, or passes unremarked.
#[test]
fn verify_tells_a_run_the_server_lost_from_one_removed() {
    let (host, _) = counting_server();
    // Its logs in files, which a write can fail to reach.
    let state = test_state_at(unique_temp_path("lost-line").into());
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true\n").unwrap());
    let slug = deploy_text(&state, &idem_text(&host), &host);
    let fail = |on: bool| state.deployments.lock().unwrap()[&slug].log.with(|l| l.fail_writes = on);
    assert_eq!(data_call(&state, &slug, "PUT", "/set/k1", "p").0, 200);
    // Run 1 changes the state, and its line can't be written.
    fail(true);
    data_call(&state, &slug, "PUT", "/set/k2", "q");
    fail(false);
    assert_eq!(data_call(&state, &slug, "PUT", "/set/k3", "r").0, 200);
    assert_eq!(data_call(&state, &slug, "PUT", "/set/k4", "s").0, 200);
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    let text = verify::describe(&report, None);
    assert!(report.ok(), "{text}");
    assert_eq!((report.runs, &report.breaks, &report.lost), (3, &vec![], &vec![(1, 1)]), "{text}");
    assert!(text.contains("run 1 isn't in the file: the server couldn't write its line, and the line after says so"), "{text}");
    let s = report.states.unwrap();
    let why = "where it began: a run between it and the run before it here, whose line the server couldn't write, may have left it";
    assert_eq!((&s.breaks, s.unchecked.get(why)), (&vec![], Some(&vec![2])), "{text}");

    // The lines: run 0's, run 2's (saying run 1 was lost), run 3's. Each rewrite keeps run 0's line
    // as it is; `chained` writes the rest after it, each `prev` the line before.
    let lines = export.lines.clone();
    let value = |i: usize| -> serde_json::Value { serde_json::from_str(&lines[i]).unwrap() };
    let chained = |rest: Vec<serde_json::Value>| {
        let mut out = vec![lines[0].clone()];
        for mut v in rest {
            v["prev"] = recorder::line_hash(out.last().unwrap()).into();
            out.push(serde_json::to_string(&v).unwrap());
        }
        out
    };
    let saying = |mut v: serde_json::Value, from: Option<u64>| {
        match from {
            Some(from) => v["lost_since"] = from.into(),
            None => drop(v.as_object_mut().unwrap().remove("lost_since")),
        }
        v
    };
    // Each: what was done, the lines, then the breaks, the runs missing, the runs said lost, and
    // whether it passes without the witness's head and with it.
    type Row = (&'static str, Vec<String>, Vec<u64>, Vec<(u64, u64)>, Vec<(u64, u64)>, bool, bool);
    let rows: Vec<Row> = vec![
        ("as the server wrote it", lines.clone(), vec![], vec![], vec![(1, 1)], true, true),
        ("the line saying run 1 was lost removed", vec![lines[0].clone(), lines[2].clone()], vec![3], vec![(1, 2)], vec![], false, false),
        ("run 2 removed, the rest chained again", chained(vec![value(2)]), vec![], vec![(1, 2)], vec![], false, false),
        ("run 2 removed, the rest chained again saying runs 1 and 2 were lost", chained(vec![saying(value(2), Some(1))]), vec![], vec![], vec![(1, 2)], true, false),
        ("run 1's loss unsaid, the rest chained again", chained(vec![saying(value(1), None), value(2)]), vec![], vec![(1, 1)], vec![], false, false),
    ];
    let witness = export.head.clone();
    for (what, lines, breaks, gaps, lost, passes, witnessed) in rows {
        let head = recorder::line_hash(lines.last().unwrap());
        let copy = verify::Export { lines, head, ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
        let report = verify::check(&copy, None, &BTreeMap::new());
        let text = verify::describe(&report, None);
        assert_eq!((&report.breaks, &report.gaps, &report.lost, report.ok()), (&breaks, &gaps, &lost, passes), "{what}:\n{text}");
        if !gaps.is_empty() {
            assert!(text.contains("MISSING: not in the file, and no line says the server couldn't write"), "{what}:\n{text}");
        }
        let report = verify::check(&copy, Some(&witness), &BTreeMap::new());
        assert_eq!(report.ok(), witnessed, "{what}, with the witness's head:\n{}", verify::describe(&report, Some(&witness)));
    }
}

/// **Guards:** a run a crash cut off between its commit and its line (its state, and its head, on
/// disk; its line never), and what `holon verify` makes of the run after it. Started again without
/// the mark of a clean stop (stopping.rs), the server notes on the deployment's timeline that the
/// state it loaded isn't the one its last run left, naming it (`persist::note_cut_off_runs`), and
/// the run after, begun from that state, is unchecked where it began, the export passing. After a
/// clean stop the same state is a break (a state changed while the server was stopped); so is a
/// state other than the one the event names; a crash that cut nothing off notes nothing.
///
/// **Why:** a run's state lands, with its head, before its line is written (C3 b), so a crash
/// between the two leaves the state ahead of the log. Unsaid, the next run's state reads as
/// changed outside holon: a false tamper alarm after every such crash. Said whatever the stop, a
/// state changed while the server was stopped would pass as a run cut off.
///
/// **If it fails:** a crash reads as tampering; or a state changed while the server was stopped,
/// or after it loaded, passes as a run a crash cut off.
#[test]
fn a_run_a_crash_cut_off_before_its_line_is_said_and_a_state_changed_while_stopped_breaks() {
    let (host, _) = counting_server();
    // Each row: whether the server crashed (no mark of a clean stop), whether a run was cut off,
    // whether the state was changed once loaded; then whether an event says a run may have been
    // cut off, the runs unchecked as such, and the runs whose state breaks.
    type Row = (bool, bool, bool, bool, Vec<u64>, Vec<u64>);
    let rows: Vec<Row> = vec![
        (true, true, false, true, vec![1], vec![]),
        (false, true, false, false, vec![], vec![1]),
        (true, true, true, true, vec![], vec![1]),
        (true, false, false, false, vec![], vec![]),
        (false, false, false, false, vec![], vec![]),
    ];
    for (crashed, cut_off, changed, said, unchecked, breaks) in rows {
        let row = format!("crashed: {crashed}, a run cut off: {cut_off}, changed once loaded: {changed}");
        let at = Place::parse(&unique_temp_path("cut-off"));
        let state = test_state_at(at.clone());
        *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true\n").unwrap());
        let slug = deploy_text(&state, &idem_text(&host), &host);
        assert_eq!(data_call(&state, &slug, "PUT", "/set/k1", "p").0, 200, "{row}");
        if cut_off {
            // Its state lands, and its head; its line never does.
            state.deployments.lock().unwrap()[&slug].log.with(|l| l.fail_writes = true);
            data_call(&state, &slug, "PUT", "/set/k2", "q");
        }
        // The server starts again from what is on disk; after a crash, crashes again before any
        // run, which says nothing more.
        let loaded = std::fs::read_to_string(persist::state_head_for(&at, &slug).unwrap()).unwrap();
        let mut reloaded = BTreeMap::new();
        for _ in 0..if crashed { 2 } else { 1 } {
            reloaded = load_deployments(&at, &state.store, &state.policy()).unwrap();
            if crashed {
                persist::note_cut_off_runs(&at, &mut reloaded);
            }
        }
        if changed {
            reloaded[&slug].state.as_ref().unwrap().state.lock().unwrap().insert("x".into(), Value::Number(1.0)).unwrap();
        }
        *state.deployments.lock().unwrap() = reloaded;
        assert_eq!(data_call(&state, &slug, "PUT", "/set/k3", "r").0, 200, "{row}");

        let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
        let report = verify::check(&export, None, &BTreeMap::new());
        let text = verify::describe(&report, None);
        let s = report.states.as_ref().unwrap();
        let broke: Vec<u64> = s.breaks.iter().map(|(seq, _)| *seq).collect();
        assert_eq!((s.unchecked.get(verify::CUT_OFF).cloned().unwrap_or_default(), broke), (unchecked, breaks.clone()), "{row}:\n{text}");
        assert_eq!(report.ok(), breaks.is_empty(), "{row}:\n{text}");
        let noted: Vec<events::Event> = export.events.iter().map(|l| serde_json::from_str::<events::Event>(l).unwrap()).filter(|e| e.loaded.is_some()).collect();
        assert_eq!(noted.len(), usize::from(said), "{row}: {:?}", export.events);
        if let Some(event) = noted.first() {
            assert_eq!((event.kind.as_str(), event.loaded.as_deref(), event.run_seq), ("lifecycle", Some(loaded.trim()), 1), "{row}");
        }
    }
}

/// **Guards:** what a run and a request take that every other one takes too (the deployments
/// table, a deployment's log, its state, its secrets, the policy): after a thread panics holding
/// each, the next run runs and is logged, and the control plane answers (sync.rs).
///
/// **Why:** a panic is caught as its run's failure (G3), but one holding a std lock poisoned it,
/// and every `lock().unwrap()` after it panicked too: one panic in a run holding the deployments
/// table failed every run and request until the server was started again.
///
/// **If it fails:** one panic takes a deployment, or the whole server, down until a restart.
#[test]
fn a_panic_holding_what_every_run_takes_stops_no_run_after_it() {
    let state = test_state();
    let unit = stage_text(&state, COUNTER);
    let ack = serde_json::json!(["Action(\"bump\")", "StateRead(\"count\")", "StateWrite(\"count\")"]);
    assert_eq!(deploy_json(&state, "c", unit, serde_json::json!({ "acknowledge": ack })).status_code, 200);
    let bump = || run_action_named(&state, "c", "bump", serde_json::json!({})).status_code;
    assert_eq!(bump(), 200);
    type Held<'a> = Box<dyn Fn() + Sync + 'a>;
    let dep = |f: &dyn Fn(&Deployment)| f(&state.deployments.lock().unwrap()["c"]);
    let rows: Vec<(&str, Held)> = vec![
        ("the deployments table", Box::new(|| {
            let _held = state.deployments.lock().unwrap();
            panic!("a run panics holding it")
        })),
        ("its log", Box::new(|| dep(&|d| d.log.with(|_| panic!("a run panics holding it"))))),
        ("its state", Box::new(|| {
            dep(&|d| {
                let _held = d.state.as_ref().unwrap().state.lock().unwrap();
                panic!("a run panics holding it")
            })
        })),
        ("its secrets", Box::new(|| {
            dep(&|d| {
                let _held = d.secrets.lock().unwrap();
                panic!("a run panics holding it")
            })
        })),
        ("the policy", Box::new(|| {
            let _held = state.policy.write().unwrap();
            panic!("a run panics holding it")
        })),
    ];
    let held = rows.len();
    for (what, hold) in rows {
        assert!(std::thread::scope(|s| s.spawn(&hold).join()).is_err(), "{what}");
        assert_eq!(bump(), 200, "a run after a panic holding {what}");
        assert_eq!(route(&state, &req("GET", "/deployments", vec![])).status_code, 200, "the control plane after a panic holding {what}");
    }
    assert_eq!(state.deployments.lock().unwrap()["c"].log.next_seq(), 1 + held as u64, "each run logged");
}

/// **Guards:** what `holon verify` makes of each kind of run an entry can say it was, and the exit
/// that follows. A run that failed is run again: what it computed is checked, and what its writes
/// made of its state (none, if its error says they were refused), not its status. One that
/// panicked isn't run again, and the export checks only so far: exit 3. A request no route took,
/// and a run turned away before it began, ran nothing: their entries must say they did nothing,
/// and a run said turned away is no run the state follows from. An entry naming no unit differs.
/// Each row rewrites one line of an honest export of three runs, one after another, and chains the
/// lines again.
///
/// **Why:** verify passed (exit 0) an export whose runs it hadn't all run again, and never ran a
/// failed run again: a failed run's line could say it left any state, and the next run begin from
/// it, unchecked. Entries "logged before" a field was kept were taken as not checkable, which a
/// forged line could claim.
///
/// **If it fails:** an export some of whose runs weren't checked passes as checked; a state a failed
/// run's line names passes unchecked; or an honest failed run reads as tampered with.
#[test]
fn every_run_an_entry_can_say_it_was_comes_to_a_verdict_and_the_exit_says_the_worst() {
    let state = test_state();
    let unit = stage_text(&state, COUNTER);
    let ack = serde_json::json!(["Action(\"bump\")", "StateRead(\"count\")", "StateWrite(\"count\")"]);
    assert_eq!(deploy_json(&state, "c", unit, serde_json::json!({ "acknowledge": ack })).status_code, 200);
    for _ in 0..3 {
        assert_eq!(run_action_named(&state, "c", "bump", serde_json::json!({})).status_code, 200);
    }
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/c/export", vec![])))).unwrap();
    // Ten seconds apart, so none ran beside another (`verify::beside`), which a run that began
    // from the state another began with may have: what it began from is the run before it's.
    let values: Vec<serde_json::Value> = export
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["at_ms"] = (v["at_ms"].as_u64().unwrap() + 10_000 * i as u64).into();
            v
        })
        .collect();
    assert_eq!(values.len(), 3);
    // `export`'s lines, changed, chained again.
    let rewritten = |change: &dyn Fn(&mut [serde_json::Value])| -> verify::Export {
        let mut changed = values.clone();
        change(&mut changed);
        let mut lines: Vec<String> = Vec::new();
        for mut v in changed {
            if let Some(before) = lines.last() {
                v["prev"] = recorder::line_hash(before).into();
            }
            lines.push(serde_json::to_string(&v).unwrap());
        }
        let head = recorder::line_hash(lines.last().unwrap());
        verify::Export { lines, head, ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() }
    };
    let failed = |v: &mut serde_json::Value, why: String| v["error"] = why.into();
    let written = format!("{}the file \"r.md\": no room", folders::WRITE_FAILED);
    let refused = format!("{}the collection \"seen\": not a record, {}", folders::WRITE_FAILED, firing::STATE_NOT_WRITTEN);
    // As the server writes an entry of a run that ran nothing (`FiredRun::nothing`).
    let emptied = |v: &mut serde_json::Value| {
        for (key, none) in [("outputs", serde_json::json!([])), ("performed", serde_json::json!([])), ("rounds", serde_json::json!([])), ("reads", serde_json::json!([])), ("state_before", serde_json::json!({}))] {
            v[key] = none;
        }
        let object = v.as_object_mut().unwrap();
        object.remove("state_after");
        object.remove("commit");
    };
    let state_left = |i: usize| values[i]["state_after"].clone();

    type Change<'a> = Box<dyn Fn(&mut [serde_json::Value]) + 'a>;
    // Run `i`'s line changed.
    fn on<'a>(i: usize, change: Box<dyn Fn(&mut serde_json::Value) + 'a>) -> Change<'a> {
        Box::new(move |lines| change(&mut lines[i]))
    }
    // Each: what was done, what each run comes to, and the exit.
    let (m, d, s, n) = ("matched", "differs", "skipped", "nothing ran");
    let rows: Vec<(&str, Change, [&str; 3], i32)> = vec![
        ("as the server wrote it", on(0, Box::new(|_| {})), [m, m, m], 0),
        ("the last failed after it finished: a file it wrote didn't land", on(2, Box::new(|v| failed(v, written.clone()))), [m, m, m], 0),
        ("the same, its line naming another state it left", on(2, Box::new(|v| {
            failed(v, written.clone());
            v["state_after"] = state_left(0);
        })), [m, m, m], 1),
        ("the last's state's writes refused, its line naming the state it began with", on(2, Box::new(|v| {
            failed(v, refused.clone());
            v["state_after"] = state_left(1);
        })), [m, m, m], 0),
        ("the same, its line naming the state its writes make", on(2, Box::new(|v| failed(v, refused.clone()))), [m, m, m], 1),
        ("the last panicked", on(2, Box::new(|v| failed(v, format!("{}a fault in Holon", firing::PANICKED)))), [m, m, s], 3),
        // As the server writes a run that failed: its outputs absent, nothing performed, its state
        // as it began.
        ("the last failed on a claim it waited for past claim_wait: it ran, and isn't run again", on(2, Box::new(|v| {
            failed(v, "busy: count is held by another run".into());
            let outputs = v["outputs"].as_array().unwrap().iter().map(|_| serde_json::Value::Null).collect();
            v["outputs"] = serde_json::Value::Array(outputs);
            v["performed"] = serde_json::json!([]);
            v["state_after"] = state_left(1);
        })), [m, m, s], 3),
        ("the last turned away, its line saying what it did", on(2, Box::new(|v| failed(v, format!("busy: x is held by another run for its whole length {}", firing::TURNED_AWAY)))), [m, m, d], 1),
        ("the last turned away, its line saying it did nothing", on(2, Box::new(|v| {
            failed(v, format!("busy: x is held by another run for its whole length {}", firing::TURNED_AWAY));
            emptied(v);
        })), [m, m, n], 0),
        // A line naming no commit it began after is taken to have begun after the line before it.
        ("the middle one failed after it finished, and the last's line names no commit", Box::new(|lines: &mut [serde_json::Value]| {
            failed(&mut lines[1], written.clone());
            lines[2].as_object_mut().unwrap().remove("commit");
        }), [m, m, m], 0),
        ("the middle one turned away, its line saying it did nothing", on(1, Box::new(|v| {
            failed(v, format!("busy: x is held by another run for its whole length {}", firing::TURNED_AWAY));
            emptied(v);
        })), [m, n, m], 1),
        ("the last a request no route took, its line saying it did nothing", on(2, Box::new(|v| {
            v["run_id"] = "".into();
            emptied(v);
        })), [m, m, n], 0),
        ("the same, its line saying what it did", on(2, Box::new(|v| v["run_id"] = "".into())), [m, m, d], 1),
        ("the last's line naming no unit", on(2, Box::new(|v| v["unit"] = "".into())), [m, m, d], 1),
    ];
    let dir = std::env::temp_dir().join(format!("holon-verdicts-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    for (what, change, verdicts, exit) in rows {
        let copy = rewritten(&*change);
        let report = verify::check(&copy, None, &BTreeMap::new());
        let text = verify::describe(&report, None);
        let came: Vec<&str> = report
            .reruns
            .iter()
            .map(|(_, r)| match r {
                verify::Rerun::Matched => m,
                verify::Rerun::Differs(_) => d,
                verify::Rerun::Skipped(_) => s,
                verify::Rerun::NothingRan(_) => n,
            })
            .collect();
        assert_eq!(came, verdicts, "{what}:\n{text}");
        let file = dir.join("c.json");
        std::fs::write(&file, serde_json::to_vec(&copy).unwrap()).unwrap();
        assert_eq!(verify::run(&[file.display().to_string()]), exit, "{what}:\n{text}");
        let verdict = text.lines().last().unwrap_or_default();
        assert_eq!((verdict == "ok", verdict == "FAILED", verdict.starts_with("INCOMPLETE")), (exit == 0, exit == 1, exit == 3), "{what}:\n{text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A run that sends nothing claims nothing: the counter under many runs at once, as before,
/// and no entry has claims.
#[test]
fn a_run_that_sends_nothing_claims_nothing() {
    let state = test_state();
    let unit = stage_text(&state, COUNTER);
    let ack = serde_json::json!(["Action(\"bump\")", "StateRead(\"count\")", "StateWrite(\"count\")"]);
    assert_eq!(deploy_json(&state, "c", unit, serde_json::json!({ "acknowledge": ack })).status_code, 200);
    std::thread::scope(|s| {
        for _ in 0..24 {
            s.spawn(|| assert_eq!(run_action_named(&state, "c", "bump", serde_json::json!({})).status_code, 200));
        }
    });
    let shown = body_json(route(&state, &req("GET", "/deployments/c/state", vec![])));
    assert_eq!(shown["state"]["count"]["value"], 24.0, "{shown}");
    for e in log_entries(&state, "c") {
        assert!(e.get("claims").is_none(), "{e}");
    }
    assert!(state.deployments.lock().unwrap()["c"].state.as_ref().unwrap().claims.held().is_empty());
}

/// A run that only writes a key another run holds waits for that run to commit: the claimer's
/// read is never changed under it, so it runs once and calls out once.
#[test]
fn a_lock_free_writer_waits_for_a_claim() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let slug = deploy_text(&state, &idem_text(&host), &host);
    let (a, w) = std::thread::scope(|s| {
        let a = s.spawn(|| data_call(&state, &slug, "POST", "/go/k5", ""));
        std::thread::sleep(Duration::from_millis(100));
        let w = s.spawn(|| data_call(&state, &slug, "PUT", "/set/k5", "mine"));
        (a.join().unwrap(), w.join().unwrap())
    });
    assert_eq!((a.0, w.0), (200, 200), "{a:?} {w:?}");
    assert!(w.2 >= Duration::from_millis(200), "the writer waited for the claim: {:?}", w.2);
    assert_eq!(hits_of(&hits, "/slow"), 1);
    assert_eq!(data_call(&state, &slug, "GET", "/seen/k5", "").1, r#"{"got":"mine"}"#, "the writer landed after the claimer");
    let entries = log_entries(&state, &slug);
    let claimer = entries.iter().find(|e| e["route"] == "POST /go/:k").unwrap();
    assert_eq!(claimer["retries"], serde_json::Value::Null, "{claimer}");
}

/// A later round that needs a key another run holds, while holding keys of its own: the run lets
/// go, waits, and runs again; its earlier request is answered from its record, not sent again.
#[test]
fn a_later_round_needing_a_held_key_restarts_without_resending() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let slug = deploy_text(&state, &idem_text(&host), &host);
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| data_call(&state, &slug, "POST", "/go/k", ""));
        std::thread::sleep(Duration::from_millis(100));
        let b = s.spawn(|| data_call(&state, &slug, "POST", "/chain", "me"));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!((a.0, b.0), (200, 200), "{a:?} {b:?}");
    assert_eq!(b.1, r#"{"other":{"got":"slow"}}"#, "the chain read what the first run wrote");
    assert_eq!((hits_of(&hits, "/name"), hits_of(&hits, "/slow"), hits_of(&hits, "/fast")), (1, 1, 1));
    let entries = log_entries(&state, &slug);
    let chain = entries.iter().find(|e| e["route"] == "POST /chain").unwrap();
    assert_eq!(chain["retries"], 1, "{chain}");
    let claims = chain["claims"].as_array().unwrap();
    assert!(claims.iter().any(|c| c["key"] == "seen[k]" && c["overtaken"] == true && c["waited_ms"].as_f64().unwrap() > 100.0), "{claims:?}");
    assert!(claims.iter().any(|c| c["key"] == "seen[b]" && c["overtaken"] == false), "{claims:?}");
}

/// Past `claim_wait_ms`, a run waiting for a held key is turned away as busy (503), naming the key.
#[test]
fn busy_after_claim_wait() {
    let (host, hits) = counting_server();
    let state = claims_state("[limits]\nclaim_wait_ms = 50");
    let slug = deploy_text(&state, &idem_text(&host), &host);
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| data_call(&state, &slug, "POST", "/go/k6", ""));
        std::thread::sleep(Duration::from_millis(100));
        let b = s.spawn(|| data_call(&state, &slug, "POST", "/go/k6", ""));
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(a.0, 200, "{a:?}");
    assert_eq!(b.0, 503, "{b:?}");
    assert!(b.1.contains("busy") && b.1.contains("seen[k6]"), "{}", b.1);
    assert!(b.2 < Duration::from_millis(300), "turned away after its wait: {:?}", b.2);
    assert_eq!(hits_of(&hits, "/slow"), 1);
    let entries = log_entries(&state, &slug);
    let busy = entries.iter().find(|e| e["status"] == 503).unwrap();
    assert!(busy["error"].as_str().unwrap().contains("seen[k6]"), "{busy}");
}

/// A run that parks lets go of its claims: the next run on its key waits for nothing.
#[test]
fn a_parked_run_releases_its_claims() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let slug = deploy_text(&state, &idem_text(&host), &host);
    let parked = data_call(&state, &slug, "POST", "/hold/k7", "");
    assert_eq!(parked.0, 200, "{parked:?}");
    assert!(state.deployments.lock().unwrap()[&slug].state.as_ref().unwrap().claims.held().is_empty(), "released at park");
    let next = data_call(&state, &slug, "POST", "/go/k7", "");
    assert_eq!(next.0, 200, "{next:?}");
    assert_eq!(hits_of(&hits, "/slow"), 1);
    let entries = log_entries(&state, &slug);
    let go = entries.iter().find(|e| e["route"] == "POST /go/:k").unwrap();
    assert!(go["claims"][0]["waited_ms"].as_f64().unwrap() < 50.0, "{go}");
    assert_eq!(go["retries"], serde_json::Value::Null, "{go}");
}

/// Two deployments append to one file of a folder bound for the whole server, many runs at once:
/// every line lands. Each deployment's commit is under its own state lock, so without a commit lock
/// on the folder both could check the file and both write it, the second over the first
/// (docs/L4s/databases-L4.md §7.1).
#[test]
fn two_deployments_appending_to_one_shared_file_lose_no_line() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-shared-ledger-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let body = serde_json::json!({ "name": "ledger", "path": dir.display().to_string(), "access": "read-write" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let text = include_str!("../../holon-author/samples/shared-ledger.holon.ts");
    let a = deploy_text(&state, text, "");
    let b = deploy_text(&state, &text.replace(r#"flow("shared-ledger""#, r#"flow("shared-ledger-2""#).replace("port: 8140", "port: 8141"), "");

    let lines: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
    std::thread::scope(|s| {
        for (i, line) in lines.iter().enumerate() {
            let (state, slug) = (&state, if i % 2 == 0 { &a } else { &b });
            s.spawn(move || {
                let (status, said, _) = data_call(state, slug, "POST", "/lines", line);
                let errors: Vec<serde_json::Value> = log_entries(state, slug).iter().map(|e| e["error"].clone()).filter(|e| !e.is_null()).collect();
                assert_eq!(status, 200, "{said}, the log says {errors:?}");
            });
        }
    });
    let written = std::fs::read_to_string(dir.join("lines.txt")).unwrap();
    let missing: Vec<&String> = lines.iter().filter(|l| !written.lines().any(|w| w == l.as_str())).collect();
    assert!(missing.is_empty(), "lost {missing:?} of {}:\n{written}", lines.len());
    assert_eq!(written.lines().count(), lines.len(), "{written}");
    // Nothing of the server's is left in it: no write beside its name, and no lock file (that is
    // for a folder other servers bind too).
    let left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left, ["lines.txt"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A flow reads and writes a SQLite database an admin bound: deploying waits for the binding (and
/// for one it can write), its writes land when the run finishes, a statement that doesn't change
/// the rows it expects lands nothing and answers 409, and its reads are in its log.
#[test]
fn a_flow_reads_and_writes_a_bound_database() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-orders-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("shop.db");
    let text = include_str!("../../holon-author/samples/orders-api.holon.ts");
    let plan = holon_author::agent::plan(text).unwrap();
    for u in &plan.units {
        route(&state, &req("POST", "/unit", u.clone()));
    }
    let preview = || body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    assert!(preview()["refused"].as_str().unwrap_or("").contains("holon database add shop"), "{}", preview());
    let bind = |access: &str, schema: Option<&str>| {
        let body = serde_json::json!({ "name": "shop", "path": file.display().to_string(), "access": access, "schema": schema });
        route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap()))
    };
    assert_eq!(bind("read", None).status_code, 400, "no database there, and no schema to make one");
    let schema = "create table orders (id integer primary key, customer text not null, total numeric not null, refunded integer not null default 0)";
    assert_eq!(bind("read", Some(schema)).status_code, 200);
    assert!(preview()["refused"].as_str().unwrap_or("").contains("read-only"), "it writes orders: {}", preview());
    assert_eq!(bind("read-write", None).status_code, 200);
    assert_eq!(body_json(route(&state, &req("GET", "/admin/databases", vec![])))[0]["engine"], "sqlite");
    let slug = deploy_text(&state, text, "");

    assert_eq!(data_call(&state, &slug, "POST", "/orders", r#"{"customer":"ada","total":"12.50"}"#).0, 201);
    assert_eq!(data_call(&state, &slug, "POST", "/orders", r#"{"customer":"ada","total":7}"#).0, 201);
    let (status, listed, _) = data_call(&state, &slug, "GET", "/orders/ada", "");
    assert_eq!(status, 200, "{listed}");
    let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(listed, serde_json::json!([{ "id": 1, "total": 12.5, "refunded": 0 }, { "id": 2, "total": 7, "refunded": 0 }]));
    let (status, one, _) = data_call(&state, &slug, "GET", "/order/2", "");
    assert_eq!((status, serde_json::from_str::<serde_json::Value>(&one).unwrap()["customer"].clone()), (200, serde_json::json!("ada")));
    assert_eq!(data_call(&state, &slug, "GET", "/order/9", "").0, 404, "no row: nothing to answer");

    // Two refunds of one order at once: one changes the row, the other finds none to change.
    let statuses: Vec<u16> = std::thread::scope(|s| {
        let calls: Vec<_> = (0..2).map(|_| s.spawn(|| data_call(&state, &slug, "POST", "/order/1/refund", "").0)).collect();
        calls.into_iter().map(|c| c.join().unwrap()).collect()
    });
    let mut sorted = statuses.clone();
    sorted.sort();
    assert_eq!(sorted, [200, 409], "{statuses:?}");
    let (status, said, _) = data_call(&state, &slug, "POST", "/order/1/refund", "");
    assert_eq!(status, 409);
    assert!(said.contains("changed 0 rows, not 1 row"), "{said}");
    let conn = rusqlite::Connection::open(&file).unwrap();
    let marked: i64 = conn.query_row("select count(*) from holon_commits", [], |r| r.get(0)).unwrap();
    assert_eq!(marked, 3, "two orders and one refund, each run marked done once");
    // Its reads are in the log, as it found them.
    let entries = log_entries(&state, &slug);
    let read = entries.iter().find(|e| e["reads"][0]["request"].to_string().contains("\"shop\"")).unwrap_or_else(|| panic!("a logged read: {entries:?}"));
    assert!(!read["reads"][0]["answer"].is_null(), "{read}");
    // Every run checks offline, its reads answered from the log: no database at hand.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    // The two refunds that changed no row failed, and say so; they are run again as every other
    // run, and match: what they computed is what the log says, their writes refused.
    let refused = entries.iter().filter(|e| e["error"].as_str().is_some_and(|e| e.contains("changed 0 rows"))).count();
    assert_eq!(refused, 2, "{entries:?}");
    assert!(report.complete() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{:?}", report.reruns);
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two deployments that read a row and write it back, at once: none loses another's update. A
/// run whose row changed between its read and its commit runs again on the row as it now is.
#[test]
fn two_deployments_counting_in_one_row_lose_no_count() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-counts-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let body = serde_json::json!({
        "name": "shop",
        "path": dir.join("counts.db").display().to_string(),
        "schema": "create table counts (id integer primary key, n integer not null); insert into counts values (1, 0)",
    });
    assert_eq!(route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let text = "import { flow, route } from \"holon\"\nimport shop from \"database:shop\"\n\nflow(\"counter\", { port: 8160 })\n\nroute.post(\"/hit/:id\", (req) => {\n  const row = shop.one(\"select n from counts where id = $1\", req.params.id)\n  shop.exec(\"update counts set n = $1 where id = $2\", row.n + 1, req.params.id, { expect: 1 })\n  return { n: row.n + 1 }\n})\n";
    let a = deploy_text(&state, text, "");
    let b = deploy_text(&state, &text.replace(r#"flow("counter""#, r#"flow("counter-2""#).replace("port: 8160", "port: 8161"), "");
    std::thread::scope(|s| {
        for i in 0..30 {
            let (state, slug) = (&state, if i % 2 == 0 { &a } else { &b });
            s.spawn(move || {
                let (status, said, _) = data_call(state, slug, "POST", "/hit/1", "");
                let errors: Vec<serde_json::Value> = log_entries(state, slug).iter().map(|e| e["error"].clone()).filter(|e| !e.is_null()).collect();
                assert_eq!(status, 200, "{said}, the log says {errors:?}");
            });
        }
    });
    let conn = rusqlite::Connection::open(dir.join("counts.db")).unwrap();
    let n: i64 = conn.query_row("select n from counts where id = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 30);
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two refunds of one order at the same moment, from two deployments bound to one database: the
/// first to call out holds the order's row (it read it and may write it) until it commits, so the
/// payment API is called once; the second waits, finds the order refunded, and sends nothing
/// (docs/L4s/databases-L4.md §7.2). Orders apart never wait for each other.
#[test]
fn two_deployments_refunding_one_order_call_the_payment_api_once() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-refunds-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let schema = "create table orders (id integer primary key, customer text not null, total numeric not null, refunded integer not null default 0); create table refunds (order_id integer not null, amount numeric not null, status integer); insert into orders (customer, total) values ('ada', 5), ('bo', 7), ('cy', 9)";
    let body = serde_json::json!({ "name": "shop", "path": dir.join("shop.db").display().to_string(), "schema": schema });
    assert_eq!(route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let text = include_str!("../../holon-author/samples/refund-once.holon.ts").replace("host:httpbin.org", &format!("host:{host}")).replace(r#""/anything/refunds""#, r#""/slow""#).replace("port: 8151", "port: 18151");
    let a = deploy_text(&state, &text, &host);
    let b = deploy_text(&state, &text.replace(r#"flow("refund-once""#, r#"flow("refund-once-2""#).replace("port: 18151", "port: 18152"), &host);
    let race = |id: u32| -> Vec<u16> {
        std::thread::scope(|s| {
            let first = s.spawn(|| data_call(&state, &a, "POST", &format!("/refund/{id}"), ""));
            std::thread::sleep(Duration::from_millis(60));
            let second = s.spawn(|| data_call(&state, &b, "POST", &format!("/refund/{id}"), ""));
            let mut statuses = vec![first.join().unwrap().0, second.join().unwrap().0];
            statuses.sort();
            statuses
        })
    };
    assert_eq!(race(1), [200, 409]);
    assert_eq!(hits_of(&hits, "/slow"), 1, "one payment for one order");
    let (_, refunds, _) = data_call(&state, &a, "GET", "/refunds/1", "");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&refunds).unwrap(), serde_json::json!([{ "amount": 5, "status": 200 }]));
    let waited = log_entries(&state, &b);
    let claim = waited.iter().flat_map(|e| e["claims"].as_array().cloned().unwrap_or_default()).find(|c| c["key"] == "shop/orders[1]").unwrap_or_else(|| panic!("{waited:#?}"));
    assert!(claim["waited_ms"].as_f64().unwrap() > 100.0 && claim["overtaken"] == true, "it waited for the first's row: {claim}");

    // Two orders at once: neither waits for the other's row.
    let statuses: Vec<u16> = std::thread::scope(|s| {
        let state = &state;
        let calls = [(&a, 2), (&b, 3)].map(|(slug, id)| s.spawn(move || data_call(state, slug, "POST", &format!("/refund/{id}"), "").0));
        calls.into_iter().map(|c| c.join().unwrap()).collect()
    });
    assert_eq!(statuses, [200, 200]);
    assert_eq!(hits_of(&hits, "/slow"), 3);
    for (slug, key) in [(&a, "shop/orders[2]"), (&b, "shop/orders[3]")] {
        let entries = log_entries(&state, slug);
        let claim = entries.iter().flat_map(|e| e["claims"].as_array().cloned().unwrap_or_default()).find(|c| c["key"] == key).unwrap_or_else(|| panic!("{entries:#?}"));
        assert!(claim["overtaken"] == false && claim["waited_ms"].as_f64().unwrap() < 250.0, "its own row, taken at once: {claim}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two deployments that read a file of one folder, call out, and write it, at once: the first to
/// call out holds the file, the second waits, finds it written, and sends nothing; another file
/// never waits.
#[test]
fn two_deployments_caching_into_one_file_call_out_once() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-memo-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let body = serde_json::json!({ "name": "memos", "path": dir.display().to_string() });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport memos from \"folder:memos\"\nimport host from \"host:{host}\"\n\nflow(\"memo\", {{ port: 18161 }})\n\nroute.post(\"/memo/:name\", async (req) => {{\n  const memo = memos[req.params.name] ?? (await host.get(\"/slow\")).body\n  memos[req.params.name] = memo\n  return memo\n}})\n"
    );
    let a = deploy_text(&state, &text, &host);
    let b = deploy_text(&state, &text.replace(r#"flow("memo""#, r#"flow("memo-2""#).replace("port: 18161", "port: 18162"), &host);
    let (first, second) = std::thread::scope(|s| {
        let first = s.spawn(|| data_call(&state, &a, "POST", "/memo/x.txt", ""));
        std::thread::sleep(Duration::from_millis(60));
        let second = s.spawn(|| data_call(&state, &b, "POST", "/memo/x.txt", ""));
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_eq!((first.0, second.0, first.1.as_str(), second.1.as_str()), (200, 200, "slow", "slow"));
    assert_eq!(hits_of(&hits, "/slow"), 1, "one fetch for one file");
    let entries = log_entries(&state, &b);
    let claim = entries.iter().flat_map(|e| e["claims"].as_array().cloned().unwrap_or_default()).find(|c| c["key"] == "memos/x.txt").unwrap_or_else(|| panic!("{entries:#?}"));
    assert_eq!(claim["overtaken"], true, "{claim}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fresh, empty database on each engine the environment names (`HOLON_TEST_POSTGRES`,
/// `HOLON_TEST_MYSQL`: a URL whose user may create databases), one per test so tests running at
/// once never share one; dropped when the test is done with it. None named: none, and it says so.
fn fresh_databases(tag: &str) -> Vec<Fresh> {
    let name = format!("holon_{tag}_{:08x}", rand::random::<u32>());
    let mut out = Vec::new();
    if let Ok(admin) = std::env::var("HOLON_TEST_POSTGRES") {
        let mut c = postgres::Client::connect(&admin, postgres::NoTls).unwrap_or_else(|e| panic!("HOLON_TEST_POSTGRES: {e}"));
        c.batch_execute(&format!("create database {name}")).unwrap();
        out.push(Fresh { engine: "postgres", url: with_database(&admin, &name), admin, name: name.clone() });
    }
    if let Ok(admin) = std::env::var("HOLON_TEST_MYSQL") {
        use mysql::prelude::Queryable;
        let mut c = mysql::Conn::new(mysql::Opts::from_url(&admin).unwrap()).unwrap_or_else(|e| panic!("HOLON_TEST_MYSQL: {e}"));
        c.query_drop(format!("create database {name}")).unwrap();
        out.push(Fresh { engine: "mysql", url: with_database(&admin, &name), admin, name: name.clone() });
    }
    if out.is_empty() {
        eprintln!("{tag}: skipped: set HOLON_TEST_POSTGRES and/or HOLON_TEST_MYSQL to run it against those engines");
    }
    out
}

/// A test's own database: its engine, and the URL to bind it by.
struct Fresh {
    engine: &'static str,
    url: String,
    admin: String,
    name: String,
}

impl Drop for Fresh {
    fn drop(&mut self) {
        match self.engine {
            "postgres" => {
                if let Ok(mut c) = postgres::Client::connect(&self.admin, postgres::NoTls) {
                    let _ = c.batch_execute(&format!("drop database if exists {} with (force)", self.name));
                }
            }
            _ => {
                use mysql::prelude::Queryable;
                if let Ok(mut c) = mysql::Opts::from_url(&self.admin).map_err(|e| e.to_string()).and_then(|o| mysql::Conn::new(o).map_err(|e| e.to_string())) {
                    let _ = c.query_drop(format!("drop database if exists {}", self.name));
                }
            }
        }
    }
}

/// `url` naming the database `name` instead of its own.
fn with_database(url: &str, name: &str) -> String {
    let (base, query) = url.split_once('?').map_or((url, None), |(b, q)| (b, Some(q)));
    let rest = base.split_once("://").map_or(base, |(_, r)| r);
    let base = match rest.find('/') {
        Some(slash) => format!("{}/{name}", &base[..base.len() - rest.len() + slash]),
        None => format!("{base}/{name}"),
    };
    query.map_or_else(|| base.clone(), |q| format!("{base}?{q}"))
}

/// A test server whose databases fetch `HOLON_SECRET_SHOP_URL` as `url`.
fn server_with_database(url: &str) -> AppState {
    let mut state = claims_state("");
    let sources = secret_sources::Sources::of(&[("HOLON_SECRET_SHOP_URL", url)]);
    state.folders = Arc::new(folders::Folders::memory().with_databases(databases::Databases::memory().with_sources(sources)));
    state
}

/// The shop's schema on `engine`: ids each engine numbers itself, exact totals (two places), a
/// refunds table.
fn shop_schema(engine: &str) -> String {
    let id = if engine == "postgres" { "integer generated by default as identity primary key" } else { "integer primary key auto_increment" };
    format!(
        "create table orders (id {id}, customer varchar(64) not null, total numeric(10,2) not null, refunded integer not null default 0); \
         create table refunds (order_id integer not null, amount numeric(10,2) not null, status integer)"
    )
}

fn bind_shop(state: &AppState, schema: Option<&str>) -> rouille::Response {
    let body = serde_json::json!({ "name": "shop", "connection": "env:HOLON_SECRET_SHOP_URL", "access": "read-write", "schema": schema });
    route(state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap()))
}

/// The admin page's resources and permissions (databases-L4 §9.1): a bound database's tables are
/// read on asking, Holon's own left out, and who holds what is each deployment's grant, a revoked
/// item among its revoked, changed only on its card.
#[test]
fn an_admin_sees_each_databases_tables_and_who_holds_what() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-holds-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let body = serde_json::json!({
        "name": "shop",
        "path": dir.join("shop.db").display().to_string(),
        "schema": "create table orders (id integer primary key, customer text not null, total numeric not null, refunded integer not null default 0); create view big as select * from orders where total > 100",
    });
    assert_eq!(route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let tables = body_json(route(&state, &req("GET", "/admin/databases/shop/tables", vec![])));
    assert_eq!(tables["tables"], serde_json::json!(["big", "orders"]), "holon_commits is Holon's: {tables}");
    assert_eq!(route(&state, &req("GET", "/admin/databases/nope/tables", vec![])).status_code, 404);
    assert_eq!(route(&state, &req("GET", "/admin/databases/shop/tables?deployment=other", vec![])).status_code, 404, "bound for the server, not for other");
    assert_eq!(route(&state, &req("POST", "/admin/databases/shop/tables", vec![])).status_code, 404);

    let slug = deploy_text(&state, include_str!("../../holon-author/samples/orders-api.holon.ts"), "");
    let held = body_json(route(&state, &req("GET", "/admin/grants", vec![])));
    let entry = held.as_array().unwrap().iter().find(|d| d["slug"] == slug.as_str()).unwrap_or_else(|| panic!("{held}"));
    assert_eq!(entry["status"], "running", "{entry}");
    let items: Vec<&str> = entry["items"].as_array().unwrap().iter().filter_map(|i| i.as_str()).collect();
    assert!(items.contains(&"DbRead(\"shop/orders\")") && items.contains(&"DbWrite(\"shop/orders\")"), "{items:?}");
    assert_eq!(entry["revoked"], serde_json::json!([]));
    assert_eq!(change_grant(&state, &slug, serde_json::json!({ "revoke": ["DbWrite(\"shop/orders\")"] })).status_code, 200);
    let held = body_json(route(&state, &req("GET", "/admin/grants", vec![])));
    let entry = held.as_array().unwrap().iter().find(|d| d["slug"] == slug.as_str()).unwrap();
    assert_eq!(entry["revoked"], serde_json::json!(["DbWrite(\"shop/orders\")"]), "{entry}");
    assert_eq!(entry["status"], "paused", "a required item revoked pauses it: {entry}");
    assert!(entry["attention"].as_str().is_some_and(|a| !a.is_empty()), "{entry}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Each engine lists its tables as a flow names them, Holon's own left out: Postgres's and
/// MySQL's from the schema, a MongoDB's collections as documents are written to them.
#[test]
fn each_engine_lists_its_tables_without_holons_own() {
    for db in fresh_databases("tables") {
        let (engine, url) = (db.engine, db.url.clone());
        let state = server_with_database(&url);
        let r = bind_shop(&state, Some(&shop_schema(engine)));
        let status = r.status_code;
        assert_eq!(status, 200, "{engine}: {}", String::from_utf8_lossy(&body_bytes(r)));
        let tables = body_json(route(&state, &req("GET", "/admin/databases/shop/tables", vec![])));
        assert_eq!(tables["tables"], serde_json::json!(["orders", "refunds"]), "{engine}: {tables}");
    }
    let Some(db) = fresh_mongo("tables") else { return };
    let state = server_with_database(&db.url);
    assert_eq!(bind_notes(&state).status_code, 200);
    let tables = || body_json(route(&state, &req("GET", "/admin/databases/notes/tables", vec![])))["tables"].clone();
    assert_eq!(tables(), serde_json::json!([]), "holon_commits and holon_claims are Holon's");
    db.notes().insert_one(mongodb::bson::doc! { "_id": "n1" }).run().unwrap();
    assert_eq!(tables(), serde_json::json!(["notes"]));
}

/// The orders-api sample runs on Postgres and MySQL with its text unchanged, the binding a
/// connection instead of a file: its totals are exact (text), its refund's `expect` holds, every
/// run is marked done in the database once, and every run checks offline.
#[test]
fn orders_api_runs_on_postgres_and_mysql_as_on_sqlite() {
    for db in fresh_databases("orders") {
        let (engine, url) = (db.engine, db.url.clone());
        let state = server_with_database(&url);
        let r = bind_shop(&state, Some(&shop_schema(engine)));
        let status = r.status_code;
        assert_eq!(status, 200, "{engine}: {}", String::from_utf8_lossy(&body_bytes(r)));
        let listed = body_json(route(&state, &req("GET", "/admin/databases", vec![])));
        assert_eq!((listed[0]["engine"].as_str(), listed[0]["connection"].as_str()), (Some(engine), Some("env:HOLON_SECRET_SHOP_URL")));
        assert!(!listed.to_string().contains(&url) && !listed.to_string().contains("@"), "the URL is never shown: {listed}");
        let text = include_str!("../../holon-author/samples/orders-api.holon.ts");
        let slug = deploy_text(&state, text, "");
        assert_eq!(data_call(&state, &slug, "POST", "/orders", r#"{"customer":"ada","total":"12.50"}"#).0, 201, "{engine}");
        assert_eq!(data_call(&state, &slug, "POST", "/orders", r#"{"customer":"ada","total":7}"#).0, 201, "{engine}");
        let (status, listed, _) = data_call(&state, &slug, "GET", "/orders/ada", "");
        assert_eq!(status, 200, "{engine}: {listed}");
        let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
        assert_eq!(listed, serde_json::json!([{ "id": 1, "total": "12.50", "refunded": 0 }, { "id": 2, "total": "7.00", "refunded": 0 }]), "{engine}");
        assert_eq!(data_call(&state, &slug, "GET", "/order/9", "").0, 404, "{engine}");
        let statuses: Vec<u16> = std::thread::scope(|s| {
            let calls: Vec<_> = (0..2).map(|_| s.spawn(|| data_call(&state, &slug, "POST", "/order/1/refund", "").0)).collect();
            let mut st: Vec<u16> = calls.into_iter().map(|c| c.join().unwrap()).collect();
            st.sort();
            st
        });
        assert_eq!(statuses, [200, 409], "{engine}");
        let (_, one, _) = data_call(&state, &slug, "GET", "/order/1", "");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&one).unwrap()["refunded"], 1, "{engine}");
        let marked = match engine {
            "postgres" => postgres::Client::connect(&url, postgres::NoTls).unwrap().query_one("select count(*) from holon_commits", &[]).unwrap().get::<_, i64>(0),
            _ => {
                use mysql::prelude::Queryable;
                mysql::Conn::new(mysql::Opts::from_url(&url).unwrap()).unwrap().query_first::<i64, _>("select count(*) from holon_commits").unwrap().unwrap()
            }
        };
        assert_eq!(marked, 3, "{engine}: two orders and one refund, each marked done once");
        let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
        let report = verify::check(&export, None, &BTreeMap::new());
        assert!(report.ok(), "{engine}: {}", verify::describe(&report, None));
    }
}

/// A statement's `$n` is the n-th value after it, whatever order its SQL names them in and however
/// often, in a read, an insert and an update, on SQLite and on every engine the environment names.
///
/// Guards: how each engine binds a flow's values to its SQL: SQLite by each `$n`'s digits,
/// Postgres as it numbers them itself, MySQL through `sql::positional`.
/// Why: SQLite numbers named parameters as it meets them, not by their digits, so `$2 … $1` took
/// the values the other way round, while the claims, read by the digits, named the row the flow
/// meant.
/// If it fails: a write lands on another row than the flow names, or on none, and a read answers
/// with another row's values.
#[test]
fn a_statements_dollar_n_is_its_nth_value_on_every_engine() {
    const ORDERS: [[usize; 3]; 6] = [[1, 2, 3], [1, 3, 2], [2, 1, 3], [2, 3, 1], [3, 1, 2], [3, 2, 1]];
    // For each order: an insert names a, b, c by it; a read by the next, one of them twice; an
    // update by the one after.
    let mut text = String::from("import { flow, route, status } from \"holon\"\nimport shop from \"database:shop\"\n\nflow(\"numbered\", { port: 8197 })\n");
    for (k, [ia, ib, ic]) in ORDERS.into_iter().enumerate() {
        let [sa, sb, sc] = ORDERS[(k + 1) % 6];
        let [ua, ub, uc] = ORDERS[(k + 2) % 6];
        text += &format!(
            "\nroute.post(\"/put/{k}\", (req) => {{\n  shop.exec(\"insert into t (a, b, c) values (${ia}, ${ib}, ${ic})\", req.json.v[0], req.json.v[1], req.json.v[2])\n  return status(201, {{ put: true }})\n}})\n\
             route.post(\"/get/{k}\", (req) => shop.one(\"select a, b, c from t where b = ${sb} and c = ${sc} and a = ${sa} and ${sa} = a\", req.json.v[0], req.json.v[1], req.json.v[2]))\n\
             route.post(\"/set/{k}\", (req) => {{\n  shop.exec(\"update t set b = ${ub} where a = ${ua} and c = ${uc}\", req.json.v[0], req.json.v[1], req.json.v[2], {{ expect: 1 }})\n  return {{ set: true }}\n}})\n"
        );
    }
    // The values in the order a statement names a, b and c by: the value for $n n-th.
    let values = |[pa, pb, pc]: [usize; 3], [a, b, c]: [&str; 3]| {
        let mut v = [""; 3];
        (v[pa - 1], v[pb - 1], v[pc - 1]) = (a, b, c);
        serde_json::json!({ "v": v }).to_string()
    };
    let schema = "create table t (a varchar(16) not null, b varchar(16) not null, c varchar(16) not null)";
    let check = |engine: &str, state: &AppState| {
        let slug = deploy_text(state, &text, "");
        for (k, order) in ORDERS.into_iter().enumerate() {
            let (a, b, c, b2) = (format!("a{k}"), format!("b{k}"), format!("c{k}"), format!("B{k}"));
            let row = |b: &str| serde_json::json!({ "a": a, "b": b, "c": c });
            let call = |path: &str, order: [usize; 3], b: &str| data_call(state, &slug, "POST", &format!("/{path}/{k}"), &values(order, [&a, b, &c]));
            let (status, said, _) = call("put", order, &b);
            assert_eq!(status, 201, "{engine}, {order:?}: {said}");
            let (status, said, _) = call("get", ORDERS[(k + 1) % 6], &b);
            assert_eq!((status, serde_json::from_str::<serde_json::Value>(&said).ok()), (200, Some(row(&b))), "{engine}: inserted by {order:?}, read by {:?}", ORDERS[(k + 1) % 6]);
            let (status, said, _) = call("set", ORDERS[(k + 2) % 6], &b2);
            assert_eq!(status, 200, "{engine}: updated by {:?}: {said}", ORDERS[(k + 2) % 6]);
            let (_, said, _) = call("get", ORDERS[(k + 1) % 6], &b2);
            assert_eq!(serde_json::from_str::<serde_json::Value>(&said).ok(), Some(row(&b2)), "{engine}: after the update by {:?}", ORDERS[(k + 2) % 6]);
        }
    };
    let dir = std::env::temp_dir().join(format!("holon-numbered-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let state = claims_state("");
    let body = serde_json::json!({ "name": "shop", "path": dir.join("shop.db").display().to_string(), "access": "read-write", "schema": schema });
    assert_eq!(route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap())).status_code, 200);
    check("sqlite", &state);
    for db in fresh_databases("numbered") {
        let state = server_with_database(&db.url);
        assert_eq!(bind_shop(&state, Some(schema)).status_code, 200, "{}", db.engine);
        check(db.engine, &state);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two servers on one Postgres (or MySQL), each running refund-once, refund one order at the same
/// moment: their own tables of claims can't see each other's, the database can. The first to call
/// out holds the order's row there until it commits, so the payment API is called once; the
/// second waits, finds the order refunded, and sends nothing. Orders apart never wait.
#[test]
fn two_servers_refunding_one_order_call_the_payment_api_once() {
    for db in fresh_databases("refunds") {
        let (engine, url) = (db.engine, db.url.clone());
        let (host, hits, gate) = gated_counting_server();
        let servers = [server_with_database(&url), server_with_database(&url)];
        assert_eq!(bind_shop(&servers[0], Some(&format!("{}; insert into orders (id, customer, total) values (1, 'ada', 5), (2, 'bo', 7), (3, 'cy', 9)", shop_schema(engine)))).status_code, 200, "{engine}");
        assert_eq!(bind_shop(&servers[1], None).status_code, 200, "{engine}");
        let text = include_str!("../../holon-author/samples/refund-once.holon.ts").replace("host:httpbin.org", &format!("host:{host}")).replace(r#""/anything/refunds""#, r#""/slow""#);
        let slugs = [deploy_text(&servers[0], &text, &host), deploy_text(&servers[1], &text, &host)];
        let (first, second) = std::thread::scope(|s| {
            let first = s.spawn(|| data_call(&servers[0], &slugs[0], "POST", "/refund/1", ""));
            std::thread::sleep(Duration::from_millis(60));
            let second = s.spawn(|| data_call(&servers[1], &slugs[1], "POST", "/refund/1", ""));
            (first.join().unwrap(), second.join().unwrap())
        });
        let mut statuses = [first.0, second.0];
        statuses.sort();
        assert_eq!(statuses, [200, 409], "{engine}: {} / {}", first.1, second.1);
        assert_eq!(hits_of(&hits, "/slow"), 1, "{engine}: one payment for one order, across servers");
        let (_, refunds, _) = data_call(&servers[1], &slugs[1], "GET", "/refunds/1", "");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&refunds).unwrap(), serde_json::json!([{ "amount": "5.00", "status": 200 }]), "{engine}");
        let entries = log_entries(&servers[1], &slugs[1]);
        let claim = entries.iter().flat_map(|e| e["claims"].as_array().cloned().unwrap_or_default()).find(|c| c["key"] == "shop/orders[1]").unwrap_or_else(|| panic!("{engine}: {entries:#?}"));
        assert!(claim["waited_ms"].as_f64().unwrap() > 100.0 && claim["overtaken"] == true, "{engine}: it waited for the other server's row: {claim}");

        // Two orders at once, one on each server: neither waits for the other's row, so both
        // calls are out at once, each holding its order's.
        gate.shut();
        let statuses: Vec<u16> = std::thread::scope(|s| {
            let calls = [(0, 2), (1, 3)].map(|(i, id)| {
                let (server, slug) = (&servers[i], &slugs[i]);
                s.spawn(move || data_call(server, slug, "POST", &format!("/refund/{id}"), "").0)
            });
            let both = gate.out_at_once(2);
            gate.open();
            assert!(both, "{engine}: side by side, not one after the other");
            calls.into_iter().map(|c| c.join().unwrap()).collect()
        });
        assert_eq!(statuses, [200, 200], "{engine}");
        assert_eq!(hits_of(&hits, "/slow"), 3, "{engine}");
    }
}

/// A program outside Holon changes a row a run read while the run is calling out: its commit's
/// check finds the row changed, and it runs again on the row as it now is, its request answered
/// from its record, not sent again.
#[test]
fn a_row_another_program_changed_sends_the_run_round_again() {
    for db in fresh_databases("counts") {
        let (engine, url) = (db.engine, db.url.clone());
        let (host, hits) = counting_server();
        let state = server_with_database(&url);
        assert_eq!(bind_shop(&state, Some("create table counts (id integer primary key, n integer not null); insert into counts values (1, 0)")).status_code, 200, "{engine}");
        let text = format!(
            "import {{ flow, route }} from \"holon\"\nimport shop from \"database:shop\"\nimport host from \"host:{host}\"\n\nflow(\"tally\", {{ port: 18171 }})\n\nroute.post(\"/tally\", async () => {{\n  const row = shop.one(\"select n from counts where id = $1\", 1)\n  const seen = (await host.get(\"/slow\")).body\n  shop.exec(\"update counts set n = $1 where id = $2\", row.n + 1, 1, {{ expect: 1 }})\n  return {{ n: row.n + 1, seen }}\n}})\n"
        );
        let slug = deploy_text(&state, &text, &host);
        let outside = || match engine {
            "postgres" => {
                postgres::Client::connect(&url, postgres::NoTls).unwrap().execute("update counts set n = 100 where id = 1", &[]).unwrap();
            }
            _ => {
                use mysql::prelude::Queryable;
                mysql::Conn::new(mysql::Opts::from_url(&url).unwrap()).unwrap().query_drop("update counts set n = 100 where id = 1").unwrap();
            }
        };
        let (status, body, _) = std::thread::scope(|s| {
            let run = s.spawn(|| data_call(&state, &slug, "POST", "/tally", ""));
            std::thread::sleep(Duration::from_millis(150));
            outside();
            run.join().unwrap()
        });
        assert_eq!(status, 200, "{engine}: {body}");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["n"], 101, "{engine}: it ran again on 100");
        assert_eq!(hits_of(&hits, "/slow"), 1, "{engine}: its request was answered from its record");
    }
}

/// A sequence's next number, handed to an API before it is written back, from a database an admin
/// set to `run` (Â§7.3 of the databases L4): each flow's text the same as without it.
fn numbering_text(host: &str, flow: &str, port: u16) -> String {
    format!(
        "import {{ flow, route }} from \"holon\"\nimport shop from \"database:shop\"\nimport host from \"host:{host}\"\n\nflow(\"{flow}\", {{ port: {port} }})\n\nroute.post(\"/next\", async () => {{\n  const row = shop.one(\"select n from seq where id = $1\", 1)\n  const sent = (await host.get(\"/slow\")).body\n  shop.exec(\"update seq set n = $1 where id = $2\", row.n + 1, 1, {{ expect: 1 }})\n  return {{ n: row.n + 1, sent }}\n}})\n"
    )
}

/// Reads the sequence and calls out, writing nothing.
fn peeking_text(host: &str, flow: &str, port: u16) -> String {
    format!(
        "import {{ flow, route }} from \"holon\"\nimport shop from \"database:shop\"\nimport host from \"host:{host}\"\n\nflow(\"{flow}\", {{ port: {port} }})\n\nroute.get(\"/peek\", async () => {{\n  const row = shop.one(\"select n from seq where id = $1\", 1)\n  const sent = (await host.get(\"/slow\")).body\n  return {{ n: row.n, sent }}\n}})\n"
    )
}

/// Whether any run of `slug` found a claimed key changed and ran again.
fn overtaken_in(state: &AppState, slug: &str) -> bool {
    log_entries(state, slug).iter().flat_map(|e| e["claims"].as_array().cloned().unwrap_or_default()).any(|c| c["overtaken"] == true)
}

/// Two requests to `first` and `second` 60 ms apart: their answers, and how long the second took.
fn two_at_once(state: &AppState, first: (&str, &str, &str), second: (&str, &str, &str)) -> ((u16, String), (u16, String, Duration)) {
    std::thread::scope(|s| {
        let a = s.spawn(|| data_call(state, first.0, first.1, first.2, ""));
        std::thread::sleep(Duration::from_millis(60));
        let b = s.spawn(|| data_call(state, second.0, second.1, second.2, ""));
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        ((a.0, a.1), b)
    })
}

/// A database an admin set to `run`: a run that may write it holds it alone from before it starts
/// until it commits, so two deployments' writers go one after the other, whole: the second never
/// starts, reads or calls out until the first has committed (it is never overtaken, as it would be
/// under a claim alone). Readers hold it shared: side by side with each other, after a writer. One
/// that would wait past its claim_wait is turned away (503), naming the database, having sent
/// nothing.
#[test]
fn a_database_held_for_whole_runs_runs_its_writers_one_at_a_time() {
    let (host, hits) = counting_server();
    let dir = std::env::temp_dir().join(format!("holon-runlock-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let bind = |state: &AppState, file: &str| {
        let body = serde_json::json!({
            "name": "shop",
            "path": dir.join(file).display().to_string(),
            "schema": "create table seq (id integer primary key, n integer not null); insert into seq values (1, 0)",
            "lock": "run",
        });
        assert_eq!(route(state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap())).status_code, 200);
    };
    let state = claims_state("");
    bind(&state, "seq.db");
    assert_eq!(body_json(route(&state, &req("GET", "/admin/databases", vec![])))[0]["lock"], "run");
    let a = deploy_text(&state, &numbering_text(&host, "numbers", 18181), &host);
    let b = deploy_text(&state, &numbering_text(&host, "numbers-2", 18182), &host);
    let (first, second) = two_at_once(&state, (&a, "POST", "/next"), (&b, "POST", "/next"));
    assert_eq!((first.0, second.0), (200, 200), "{} / {}", first.1, second.1);
    let ns: BTreeSet<String> = [&first.1, &second.1].iter().map(|b| serde_json::from_str::<serde_json::Value>(b).unwrap()["n"].to_string()).collect();
    assert_eq!(ns, BTreeSet::from(["1".to_string(), "2".to_string()]));
    assert!(second.2 > Duration::from_millis(700), "it ran after the first, whole: {:?}", second.2);
    assert!(!overtaken_in(&state, &b), "it started once the first had committed: {:#?}", log_entries(&state, &b));
    assert_eq!(hits_of(&hits, "/slow"), 2);

    // Readers side by side; a reader after a writer.
    let r1 = deploy_text(&state, &peeking_text(&host, "peek", 18183), &host);
    let r2 = deploy_text(&state, &peeking_text(&host, "peek-2", 18184), &host);
    let (one, two) = two_at_once(&state, (&r1, "GET", "/peek"), (&r2, "GET", "/peek"));
    assert_eq!((one.0, two.0), (200, 200));
    assert!(two.2 < Duration::from_millis(650), "readers share it: {:?}", two.2);
    let (_, after) = two_at_once(&state, (&a, "POST", "/next"), (&r1, "GET", "/peek"));
    assert_eq!(after.0, 200);
    assert!(after.2 > Duration::from_millis(700), "a reader waits for a writer: {:?}", after.2);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&after.1).unwrap()["n"], 3, "and reads what it wrote");

    // Past claim_wait: turned away, sending nothing.
    let quick = claims_state("[limits]\nclaim_wait_ms = 100");
    bind(&quick, "quick.db");
    let a = deploy_text(&quick, &numbering_text(&host, "numbers", 18185), &host);
    let b = deploy_text(&quick, &numbering_text(&host, "numbers-2", 18186), &host);
    let before = hits_of(&hits, "/slow");
    let (first, second) = two_at_once(&quick, (&a, "POST", "/next"), (&b, "POST", "/next"));
    assert_eq!((first.0, second.0), (200, 503), "{}", second.1);
    assert!(second.1.contains("the database \\\"shop\\\""), "it names the database: {}", second.1);
    assert_eq!(hits_of(&hits, "/slow"), before + 1, "the one turned away sent nothing");
    let turned = log_entries(&quick, &b);
    assert!(turned.last().unwrap()["error"].as_str().unwrap().starts_with("busy:"), "{turned:#?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A folder an admin set to `run`, bound by several servers: its writers go one after the other,
/// whole, and the lock is an OS lock on a file of its own in the folder, so another server's runs
/// see it too.
#[test]
fn a_folder_held_for_whole_runs_runs_its_writers_one_at_a_time() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-runlock-f-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let body = serde_json::json!({ "name": "ledger", "path": dir.display().to_string(), "shared": true, "lock": "run" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200);
    assert_eq!(body_json(route(&state, &req("GET", "/admin/folders", vec![])))[0]["lock"], "run");
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport ledger from \"folder:ledger\"\nimport host from \"host:{host}\"\n\nflow(\"tally\", {{ port: 18191 }})\n\nroute.post(\"/add\", async () => {{\n  const lines = ledger[\"lines.txt\"] ?? \"\"\n  const sent = (await host.get(\"/slow\")).body\n  ledger[\"lines.txt\"] = `${{lines}}${{sent}}\n`\n  return {{ sent }}\n}})\n"
    );
    let a = deploy_text(&state, &text, &host);
    let b = deploy_text(&state, &text.replace(r#"flow("tally""#, r#"flow("tally-2""#).replace("port: 18191", "port: 18192"), &host);
    let (first, second) = two_at_once(&state, (&a, "POST", "/add"), (&b, "POST", "/add"));
    assert_eq!((first.0, second.0), (200, 200), "{} / {}", first.1, second.1);
    assert!(second.2 > Duration::from_millis(700), "it ran after the first, whole: {:?}", second.2);
    assert!(!overtaken_in(&state, &b), "{:#?}", log_entries(&state, &b));
    assert_eq!(hits_of(&hits, "/slow"), 2);
    assert_eq!(std::fs::read_to_string(dir.join("lines.txt")).unwrap(), "slow\nslow\n");
    assert!(dir.join(locks::RUN_LOCK_FILE).exists(), "bound shared: its lock is a file in it");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two servers on one Postgres (or MySQL) an admin set to `run`: the lock is held in the database,
/// so one server's writer waits for the other's to commit, whole, its readers side by side with
/// the other's, and past claim_wait a run is turned away.
#[test]
fn a_database_held_for_whole_runs_holds_across_servers() {
    for db in fresh_databases("runlock") {
        let (engine, url) = (db.engine, db.url.clone());
        let (host, hits, gate) = gated_counting_server();
        let servers = [server_with_database(&url), server_with_database(&url)];
        for s in &servers {
            *s.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true\n[limits]\nclaim_wait_ms = 3000").unwrap());
        }
        for (i, s) in servers.iter().enumerate() {
            let schema = (i == 0).then_some("create table seq (id integer primary key, n integer not null); insert into seq values (1, 0)");
            let body = serde_json::json!({ "name": "shop", "connection": "env:HOLON_SECRET_SHOP_URL", "schema": schema, "lock": "run" });
            let r = route(s, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap()));
            let status = r.status_code;
            assert_eq!(status, 200, "{engine}: {}", String::from_utf8_lossy(&body_bytes(r)));
        }
        let writers = [deploy_text(&servers[0], &numbering_text(&host, "numbers", 18181), &host), deploy_text(&servers[1], &numbering_text(&host, "numbers", 18181), &host)];
        let (first, second) = std::thread::scope(|s| {
            let a = s.spawn(|| data_call(&servers[0], &writers[0], "POST", "/next", ""));
            std::thread::sleep(Duration::from_millis(60));
            let b = s.spawn(|| data_call(&servers[1], &writers[1], "POST", "/next", ""));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_eq!((first.0, second.0), (200, 200), "{engine}: {} / {}", first.1, second.1);
        assert!(second.2 > Duration::from_millis(700), "{engine}: it ran after the other server's, whole: {:?}", second.2);
        assert!(!overtaken_in(&servers[1], &writers[1]), "{engine}: {:#?}", log_entries(&servers[1], &writers[1]));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&second.1).unwrap()["n"], 2, "{engine}");
        let readers = [deploy_text(&servers[0], &peeking_text(&host, "peek", 18183), &host), deploy_text(&servers[1], &peeking_text(&host, "peek", 18183), &host)];
        gate.shut();
        std::thread::scope(|s| {
            let calls = [0, 1].map(|i| {
                let (server, slug) = (&servers[i], &readers[i]);
                s.spawn(move || data_call(server, slug, "GET", "/peek", "").0)
            });
            let both = gate.out_at_once(2);
            gate.open();
            assert!(both, "{engine}: readers of two servers share it, their calls out at once");
            assert_eq!(calls.map(|c| c.join().unwrap()), [200, 200], "{engine}");
        });
        assert_eq!(hits_of(&hits, "/slow"), 4, "{engine}");

        // A writer deployed where claim_wait is 100 ms, while the other server's writer runs:
        // turned away, sending nothing.
        *servers[1].policy.write().unwrap() = Arc::new(Policy::parse("[grants]
allow_plain_http = true
allow_private_addresses = true
[limits]
claim_wait_ms = 100").unwrap());
        let quick = deploy_text(&servers[1], &numbering_text(&host, "numbers-quick", 18185), &host);
        let (first, second) = std::thread::scope(|s| {
            let a = s.spawn(|| data_call(&servers[0], &writers[0], "POST", "/next", ""));
            std::thread::sleep(Duration::from_millis(60));
            let b = s.spawn(|| data_call(&servers[1], &quick, "POST", "/next", ""));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert_eq!((first.0, second.0), (200, 503), "{engine}: {}", second.1);
        assert_eq!(hits_of(&hits, "/slow"), 5, "{engine}: the one turned away sent nothing");
    }
}

/// A restore's dry run names the files the runs since wrote, which it never puts back: each once,
/// with how many writes and by which runs, its text then (a later run read it, so the log has it)
/// and now; once an edit by hand changes it on disk, it says so, and the CLI's words say it too.
/// A cell's history: the runs that changed one cell, a collection or one item of it, newest
/// first, with the value before and after; a restore between two runs is a change of its own,
/// named by its event, so each value follows from the one before. A run that touched another
/// item isn't named.
#[test]
fn a_cells_history_is_the_runs_that_changed_it_and_a_restore_between_them() {
    let state = claims_state("");
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/balance-trail.holon.ts"), "");
    for (account, amount) in [("a", 100), ("b", 5), ("a", -30)] {
        assert_eq!(data_call(&state, &slug, "POST", &format!("/accounts/{account}/move"), &format!(r#"{{"amount":{amount}}}"#)).0, 200);
    }
    let seqs: Vec<u64> = log_entries(&state, &slug).iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    let (r1, r2, r3) = (seqs[0], seqs[1], seqs[2]);
    let body = serde_json::json!({ "run": r1, "state": true }).to_string().into_bytes();
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/restore"), body)).status_code, 200);
    assert!(data_call(&state, &slug, "POST", "/accounts/a/move", r#"{"amount":1}"#).1.contains("\"balance\":101"));
    let r4 = *log_entries(&state, &slug).iter().map(|e| e["seq"].as_u64().unwrap()).collect::<Vec<_>>().last().unwrap();
    let changes = |query: &str| body_json(route(&state, &req("GET", &format!("/deployments/{slug}/changes?{query}"), vec![])));
    let n = |v: &serde_json::Value| v["value"].as_f64();

    // One item: the runs that moved a, the restore between, newest first; b's run isn't named.
    let a = changes("name=balances&item=a");
    assert_eq!((a["collection"].clone(), n(&a["now"])), (true.into(), Some(101.0)), "{a}");
    let list = a["changes"].as_array().unwrap();
    let summary: Vec<(Option<u64>, Option<f64>, Option<f64>)> = list.iter().map(|c| (c["seq"].as_u64(), n(&c["before"]), n(&c["after"]))).collect();
    assert_eq!(summary, [(Some(r4), Some(100.0), Some(101.0)), (None, Some(70.0), Some(100.0)), (Some(r3), Some(100.0), Some(70.0)), (Some(r1), None, Some(100.0))], "{a}");
    assert_eq!(list[1]["between"], serde_json::json!([r3, r4]));
    assert_eq!(list[1]["events"][0]["kind"], "restore", "{a}");
    assert_eq!((list[0]["method"].clone(), list[0]["url"].clone(), list[0]["status"].clone()), ("POST".into(), "/accounts/a/move".into(), 200.into()));
    assert_eq!(a["more"], false);

    // A cell: every run moved it.
    let moves = changes("name=moves");
    let counted: Vec<(Option<f64>, Option<f64>)> = moves["changes"].as_array().unwrap().iter().map(|c| (n(&c["before"]), n(&c["after"]))).collect();
    assert_eq!(counted, [(Some(1.0), Some(2.0)), (Some(3.0), Some(1.0)), (Some(2.0), Some(3.0)), (Some(1.0), Some(2.0)), (None, Some(1.0))], "{moves}");
    assert_eq!(moves["collection"], false);

    // A whole collection: each change names the items it changed.
    let all = changes("name=balances");
    let items: Vec<Vec<String>> = all["changes"].as_array().unwrap().iter().map(|c| c["items"].as_object().unwrap().keys().cloned().collect()).collect();
    assert_eq!(items, [vec!["a"], vec!["a", "b"], vec!["a"], vec!["b"], vec!["a"]], "{all}");
    assert_eq!(all["changes"][1]["items"]["b"]["after"], serde_json::Value::Null, "the restore took b away");
    assert_eq!(all["changes"][3]["seq"], r2);

    // Paged: the newest two, then from before the older one.
    let page = changes("name=balances&item=a&limit=2");
    assert_eq!((page["changes"].as_array().unwrap().len(), page["more"].clone()), (2, true.into()));
    let older = changes(&format!("name=balances&item=a&before={r3}"));
    assert_eq!(older["changes"].as_array().unwrap().iter().map(|c| c["seq"].as_u64()).collect::<Vec<_>>(), [Some(r1)]);
    for bad in ["", "name=moves&item=a"] {
        assert_eq!(route(&state, &req("GET", &format!("/deployments/{slug}/changes?{bad}"), vec![])).status_code, 400, "{bad}");
    }
    assert_eq!(route(&state, &req("GET", "/deployments/nobody/changes?name=x", vec![])).status_code, 404);
}

#[test]
fn a_restores_dry_run_names_the_files_written_since_and_an_edit_outside_a_run() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-files-since-{}-{}", std::process::id(), rand::random::<u32>()));
    let sheets = dir.join("sheets");
    std::fs::create_dir_all(&sheets).unwrap();
    let body = serde_json::json!({ "name": "sheets", "path": sheets.display().to_string(), "access": "read-write", "keep": "bytes" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/stock-sheet.holon.ts"), "");
    assert_eq!(data_call(&state, &slug, "PUT", "/stock/tea", r#"{"count":4}"#).0, 200);
    assert_eq!(data_call(&state, &slug, "PUT", "/stock/tea", r#"{"count":40}"#).0, 200);
    let seqs: Vec<u64> = log_entries(&state, &slug).iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    let (first, second) = (seqs[0], seqs[1]);
    let dry = || {
        let body = serde_json::json!({ "run": first, "state": true, "dry_run": true }).to_string().into_bytes();
        body_json(route(&state, &req("POST", &format!("/deployments/{slug}/restore"), body)))["state"].clone()
    };

    let change = dry();
    let file = &change["files"][0];
    assert_eq!((file["folder"].clone(), file["name"].clone(), file["writes"].clone()), ("sheets".into(), "changes.txt".into(), 1.into()), "{change}");
    assert_eq!((file["runs"].clone(), file["last"]["kind"].clone(), file["at_point"]["run"].clone()), (serde_json::json!([second]), "write".into(), first.into()));
    assert_eq!(file["outside"], false, "on disk as run {second} left it");
    assert_eq!((file["then_text"].clone(), file["now_text"].clone()), ("tea: 4\n".into(), "tea: 4\ntea: 40\n".into()), "then: the read run {second} kept");
    assert_eq!(change["collections"]["stock"]["items"]["tea"]["then"]["value"], 4.0);
    let said = crate::restore::file_line(file);
    assert_eq!(said, format!("sheets/changes.txt: 1 write since, by run {second} (the last written)"));

    // Edited by hand: not what the last run left. A restore still doesn't put it back.
    std::fs::write(sheets.join("changes.txt"), "tea: 4\ntea: 40\nchecked by hand\n").unwrap();
    let file = dry()["files"][0].clone();
    assert_eq!((file["outside"].clone(), file["now_text"].clone()), (true.into(), "tea: 4\ntea: 40\nchecked by hand\n".into()));
    let on_disk = blake3::hash(b"tea: 4\ntea: 40\nchecked by hand\n").to_hex().to_string();
    assert!(
        crate::restore::file_line(&file).ends_with(&format!("; changed outside a run since run {second}: on disk now #{}, not what run {second} left", &on_disk[..8])),
        "{}",
        crate::restore::file_line(&file)
    );
    std::fs::remove_file(sheets.join("changes.txt")).unwrap();
    assert!(crate::restore::file_line(&dry()["files"][0]).ends_with(&format!("; gone from disk since run {second}, which left it")));

    // Restored: the counts back, the file as it is.
    let body = serde_json::json!({ "run": first, "state": true }).to_string().into_bytes();
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/restore"), body)).status_code, 200);
    assert!(data_call(&state, &slug, "GET", "/stock/tea", "").1.contains("\"count\":4"));
    assert!(!sheets.join("changes.txt").exists(), "a restore doesn't touch files");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A flow reads and writes files in the folders an admin bound, by name: deploying waits for the
/// binding, a write lands whole when the run commits, a name that leaves its folder fails the run
/// with nothing written, and the log keeps a write by its hash. The runs check offline: a small
/// file's text from the log, a big one's from the export, and a folder that keeps hashes from the
/// files given (`holon verify --files`).
#[test]
fn a_flow_reads_and_writes_bound_folders_and_its_runs_verify() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-folders-flow-{}-{}", std::process::id(), rand::random::<u32>()));
    let (inbox, reports) = (dir.join("inbox"), dir.join("reports"));
    std::fs::create_dir_all(&inbox).unwrap();
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::write(inbox.join("sales.csv"), "region,amount\nnorth,10\nsouth,32\n").unwrap();
    let text = include_str!("../../holon-author/samples/inbox-report.holon.ts");

    // Not bound: deploying it is refused, saying which folder and how to bind it.
    let plan = holon_author::agent::plan(text).unwrap();
    for u in &plan.units {
        route(&state, &req("POST", "/unit", u.clone()));
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    assert!(preview["refused"].as_str().unwrap_or("").contains("holon folder add inbox"), "{preview}");

    // Bound for another deployment only: still not this one's. The admin API lists it as that
    // deployment's, and unbinds it there.
    let theirs = serde_json::json!({ "name": "inbox", "path": inbox.display().to_string(), "access": "read", "deployment": "other" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&theirs).unwrap())).status_code, 200);
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    assert!(preview["refused"].as_str().unwrap_or("").contains("--for inbox-report"), "{preview}");
    let listed = body_json(route(&state, &req("GET", "/admin/folders", vec![])));
    assert_eq!(listed[0]["deployment"], "other", "{listed}");
    assert_eq!(route(&state, &req("DELETE", "/admin/folders/inbox", vec![])).status_code, 404, "not the server's");
    assert_eq!(route(&state, &req("DELETE", "/admin/folders/inbox?deployment=other", vec![])).status_code, 204);

    let bind = |name: &str, path: &Path, access: &str, keep: &str| {
        let body = serde_json::json!({ "name": name, "path": path.display().to_string(), "access": access, "keep": keep });
        route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap()))
    };
    assert_eq!(bind("inbox", Path::new("relative/inbox"), "read", "bytes").status_code, 400, "a path is whole");
    assert_eq!(bind("inbox", &inbox, "read", "bytes").status_code, 200);
    assert_eq!(bind("reports", &reports, "read", "bytes").status_code, 200);
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    assert!(preview["refused"].as_str().unwrap_or("").contains("read-only"), "it writes reports: {preview}");
    assert_eq!(bind("reports", &reports, "read-write", "bytes").status_code, 200);
    let listed = body_json(route(&state, &req("GET", "/admin/folders", vec![])));
    assert_eq!(listed.as_array().unwrap().len(), 2, "{listed}");
    let slug = deploy_text(&state, text, "");

    let (status, body, _) = data_call(&state, &slug, "POST", "/report", r#"{"name":"sales.csv"}"#);
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["rows"], 2.0);
    let written = std::fs::read_to_string(reports.join("sales.csv.md")).unwrap();
    assert!(written.contains("| amount | 42 |"), "{written}");
    let (status, served, _) = data_call(&state, &slug, "GET", "/reports/sales.csv.md", "");
    assert_eq!((status, served.as_str()), (200, written.as_str()));
    let (_, listing, _) = data_call(&state, &slug, "GET", "/reports", "");
    assert!(listing.contains("sales.csv.md") && listing.contains(&blake3::hash(written.as_bytes()).to_hex().to_string()), "{listing}");
    assert_eq!(data_call(&state, &slug, "POST", "/report", r#"{"name":"none.csv"}"#).0, 404, "no such file: no value");
    let (status, said, _) = data_call(&state, &slug, "POST", "/report", r#"{"name":"../reports/sales.csv.md"}"#);
    assert_eq!(status, 500, "a name that leaves its folder fails the run: {said}");
    assert!(!said.contains(".."), "the caller isn't told why: {said}");
    let failed = log_entries(&state, &slug).last().unwrap()["error"].as_str().unwrap().to_string();
    assert!(failed.starts_with(folders::READ_FAILED) && failed.contains("can't name a file"), "the log says why: {failed}");
    assert!(!dir.join("reports/sales.csv.md.md").exists() && !dir.join("sales.csv.md.md").exists(), "and writes nothing");

    // A big file's text goes to the content store, not the log line.
    let big: String = std::iter::once("region,amount\n".to_string()).chain((0..40).map(|i| format!("{}-{i},1\n", "region".repeat(20)))).collect();
    std::fs::write(inbox.join("big.csv"), &big).unwrap();
    let (status, body, _) = data_call(&state, &slug, "POST", "/report", r#"{"name":"big.csv"}"#);
    assert_eq!(status, 200, "{body}");

    let log = serde_json::to_string(&log_entries(&state, &slug)).unwrap();
    let posted: Vec<serde_json::Value> = log_entries(&state, &slug).into_iter().filter(|e| e["route"] == "POST /report").collect();
    let performed = serde_json::to_string(&posted.iter().map(|e| &e["performed"]).collect::<Vec<_>>()).unwrap();
    let outputs = serde_json::to_string(&posted.iter().map(|e| &e["outputs"]).collect::<Vec<_>>()).unwrap();
    assert!(performed.contains("FileWrite") && performed.contains(&blake3::hash(written.as_bytes()).to_hex().to_string()), "{performed}");
    assert!(!performed.contains("| amount |") && !outputs.contains("| amount |"), "a write is logged by its hash, never its text");
    assert!(!log.contains("-39,1"), "a big read isn't in the log, nor in its timeline");
    let export = || -> verify::Export { serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap() };
    let first = export();
    assert!(first.blobs.values().any(|t| t == &big), "the export holds the big file's text");
    let report = verify::check(&first, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    // The one whose read failed too: it performed nothing, as the server dropped what it did.
    assert!(report.complete() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{:?}", report.reruns);
    assert_eq!(first.files_left_out, None);

    // Asked for less: the big file's text stays out, the export says so, and the run that read it
    // checks only with the file given.
    let smaller = |query: &str| -> verify::Export {
        serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export?{query}"), vec![])))).unwrap()
    };
    for query in ["without_files=1", "max_files_bytes=100"] {
        let lean = smaller(query);
        assert!(lean.blobs.is_empty(), "{query}");
        assert_eq!(lean.files_left_out, Some(verify::LeftOut { files: 1, bytes: big.len() as u64 }), "{query}");
        let without = verify::check(&lean, None, &BTreeMap::new());
        assert!(without.ok() && without.reruns.iter().any(|(_, r)| matches!(r, verify::Rerun::Skipped(why) if why.contains("left it out"))), "{query}");
        let with = verify::check(&lean, None, &folders::files_in(&inbox).unwrap());
        assert_eq!(with.reruns.iter().filter(|(_, r)| *r == verify::Rerun::Matched).count(), report.reruns.iter().filter(|(_, r)| *r == verify::Rerun::Matched).count(), "{query}");
    }
    assert_eq!(smaller(&format!("max_files_bytes={}", big.len())).blobs.len(), 1, "a budget it fits in");
    assert_eq!(route(&state, &req("GET", &format!("/deployments/{slug}/export?max_files_bytes=lots"), vec![])).status_code, 400);

    // A folder that keeps hashes: the run checks only with the file given.
    assert_eq!(bind("inbox", &inbox, "read", "hash").status_code, 200);
    assert_eq!(data_call(&state, &slug, "POST", "/report", r#"{"name":"sales.csv"}"#).0, 200);
    let kept = export();
    let last = kept.lines.len() as u64 - 1;
    let without = verify::check(&kept, None, &BTreeMap::new());
    assert!(without.ok(), "{}", verify::describe(&without, None));
    assert!(matches!(&without.reruns.last().unwrap().1, verify::Rerun::Skipped(why) if why.contains("--files")), "{:?}", without.reruns.last());
    let with = verify::check(&kept, None, &folders::files_in(&inbox).unwrap());
    assert_eq!(with.reruns.last().unwrap(), &(last, verify::Rerun::Matched), "{}", verify::describe(&with, None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The admin page's "browse…" (pick.rs): a folder's folders for the editor to click through; the
/// OS's dialog only for a request from the machine itself, never one a proxy forwarded or one
/// from elsewhere.
#[test]
fn an_admin_browses_the_servers_folders_and_only_the_machine_itself_opens_its_dialog() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-browse-route-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(dir.join("inbox")).unwrap();
    let url = format!("/admin/folders/browse?path={}", dir.display().to_string().replace('\\', "%5C").replace(':', "%3A"));
    let listed = body_json(route(&state, &req("GET", &url, vec![])));
    assert_eq!(listed["folders"][0]["name"], "inbox", "{listed}");
    assert!(listed["roots"].as_array().is_some_and(|r| !r.is_empty()), "{listed}");
    assert_eq!(route(&state, &req("GET", "/admin/folders/browse?path=not%2Fwhole", vec![])).status_code, 400);

    let pick = |headers: Vec<(String, String)>, from: &str| {
        let r = Request::fake_http_from(from.parse().unwrap(), "POST", "/admin/folders/pick", [vec![host_header(), ("Content-Type".to_string(), "application/json".to_string())], headers].concat(), b"{}".to_vec());
        let answer = route(&state, &r);
        (answer.status_code, body_json(answer)["error"].as_str().unwrap_or_default().to_string())
    };
    let (status, why) = pick(vec![("X-Forwarded-For".to_string(), "203.0.113.9".to_string())], "127.0.0.1:5000");
    assert_eq!((status, why.contains("isn't on the server's machine")), (409, true), "{why}");
    let (status, why) = pick(vec![], "203.0.113.9:5000");
    assert_eq!((status, why.contains("isn't on the server's machine")), (409, true), "{why}");
    let r = Request::fake_http_from("203.0.113.9:5000".parse().unwrap(), "GET", &url, vec![host_header()], vec![]);
    assert_eq!(body_json(route(&state, &r))["native"], false, "the listing says so too");
    assert_eq!(route(&state, &req("GET", "/admin/folders/pick", vec![])).status_code, 404);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A deployment's file limits: a read past them fails the run as it reads, and a run whose writes
/// would pass them fails before any lands; each error names the limit.
#[test]
fn a_run_past_its_file_limits_fails_and_says_which() {
    for (limits, route, url, body, limit) in [
        ("inline_file_bytes = 100", "GET", "/logs/app.log/head", "", "inline_file_bytes"),
        ("max_file_read_bytes = 200", "GET", "/logs/app.log/head", "", "max_file_read_bytes"),
        ("max_file_write_bytes = 1000", "POST", "/archive", r#"{"name":"app.log"}"#, "max_file_write_bytes"),
        ("max_files_per_run = 1", "POST", "/archive", r#"{"name":"app.log"}"#, "max_files_per_run"),
    ] {
        let state = claims_state(&format!("[limits]
{limits}
"));
        let dir = std::env::temp_dir().join(format!("holon-file-limits-{}-{}", std::process::id(), rand::random::<u32>()));
        let (logs, archive) = (dir.join("logs"), dir.join("archive"));
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(logs.join("app.log"), "a line of a log
".repeat(200)).unwrap();
        for (name, path, access) in [("logs", &logs, "read"), ("archive", &archive, "read-write")] {
            let body = serde_json::json!({ "name": name, "path": path.display().to_string(), "access": access });
            assert_eq!(route_status(&state, &body), 200);
        }
        let slug = deploy_text(&state, include_str!("../../holon-author/samples/log-archive.holon.ts"), "");
        let (status, answer, _) = data_call(&state, &slug, route, url, body);
        assert_eq!(status, 500, "{limits}: {answer}");
        let error = log_entries(&state, &slug).last().unwrap()["error"].as_str().unwrap_or_default().to_string();
        assert!(error.contains(limit), "{limits}: {error}");
        assert_eq!(std::fs::read_dir(&archive).unwrap().count(), 0, "{limits}: nothing landed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn route_status(state: &AppState, body: &serde_json::Value) -> u16 {
    route(state, &req("POST", "/admin/folders", serde_json::to_vec(body).unwrap())).status_code
}

/// How a reply is framed is the host's to say: a flow's (or an upstream's, passed on) length,
/// transfer encoding and connection headers are dropped, its other headers kept; and an interim
/// status (1xx) isn't an answer, even one computed where the language can't see it.
#[test]
fn a_reply_is_framed_by_the_host() {
    let state = claims_state("");
    let text = r#"import { flow, respond, route } from "holon"

flow("framed", { port: 18148 })

route.get("/a", () =>
  respond("HTTP/1.1 200 OK", {
    headers: {
      "content-length": "0",
      "Transfer-Encoding": "chunked",
      connection: "keep-alive",
      upgrade: "h2c",
      "x-kept": "yes",
    },
    status: 200,
  }),
)

route.get("/b", () => {
  const switching = 100 + 1
  return respond("", { headers: { upgrade: "websocket" }, status: switching })
})
"#;
    let slug = deploy_text(&state, text, "x");
    let r = serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &fake_data("GET", "/a", vec![]));
    assert_eq!((r.status, r.body.as_slice()), (200, &b"HTTP/1.1 200 OK"[..]));
    let names: Vec<&str> = r.headers.iter().map(|(k, _)| k.as_str()).collect();
    assert!(names.contains(&"x-kept"), "{names:?}");
    for framing in ["content-length", "transfer-encoding", "connection", "upgrade"] {
        assert!(!names.contains(&framing), "{framing}: {names:?}");
    }
    assert_eq!(serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &fake_data("GET", "/b", vec![])).status, 500);
}

/// Which value of a secret a run had shows in the log without the value: each entry names the
/// version and the keyed fingerprint of the secrets it was given, and its marks say the same. A
/// run before a rotation and one after read differently, both still check after it (the entry
/// holds what it needs, not the vault), and so does an entry logged before marks named versions.
#[test]
fn a_run_shows_which_secret_value_it_had_and_still_checks_after_it_rotates() {
    let state = claims_state("");
    let set = |value: &str| {
        let body = serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap();
        assert_eq!(route(&state, &req("POST", "/deployments/rotor/secrets/signing_key", body)).status_code, 200);
    };
    set("whsec_first_123456");
    let text = "import { flow, hmac_sha256, route, status } from \"holon\"\nimport signing_key from \"secret:signing_key\"\n\nflow(\"rotor\", { port: 18149 })\n\nroute.post(\"/sign\", (req) => hmac_sha256(signing_key, req.body) ?? status(400))\n";
    let slug = deploy_text(&state, text, "127.0.0.1:9");
    assert_eq!(data_call(&state, &slug, "POST", "/sign", "hello").0, 200);
    set("whsec_second_654321");
    assert_eq!(data_call(&state, &slug, "POST", "/sign", "hello").0, 200);

    let entries = log_entries(&state, &slug);
    let given: Vec<String> = entries.iter().map(|e| e["secrets"]["signing_key"].as_str().unwrap_or_default().to_string()).collect();
    assert!(given[0].starts_with("v1 #") && given[1].starts_with("v2 #"), "{given:?}");
    assert_ne!(given[0], given[1].replace("v2", "v1"), "another value, another fingerprint");
    for (entry, given) in entries.iter().zip(&given) {
        let line = entry.to_string();
        assert!(line.contains(&format!("‹secret:signing_key {given}›")), "the mark names it: {line}");
        assert!(!line.contains("whsec_first") && !line.contains("whsec_second"), "{line}");
    }

    // Exported after the rotation: both runs check, as what each was given, not the vault's now.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert_eq!(report.reruns, vec![(0, verify::Rerun::Matched), (1, verify::Rerun::Matched)], "{}", verify::describe(&report, None));
    assert!(report.ok(), "{}", verify::describe(&report, None));

    // An entry from before: no versions in it, its marks bare. Rechained, it checks as it did.
    let mut v: serde_json::Value = serde_json::from_str(&export.lines[0]).unwrap();
    v.as_object_mut().unwrap().remove("secrets");
    let bare = v.to_string().replace(&format!(" {}›", given[0]), "›");
    assert!(bare.contains("‹secret:signing_key›") && !bare.contains(&given[0]), "{bare}");
    let head = recorder::line_hash(&bare);
    let old = verify::Export { lines: vec![bare], head: head.clone(), ..serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
    let report = verify::check(&old, Some(&head), &BTreeMap::new());
    assert_eq!(report.reruns, vec![(0, verify::Rerun::Matched)], "{}", verify::describe(&report, None));
}

/// A secret used as a record's key (a map from API keys to accounts) is redacted in the log as a
/// value is, and the run still checks: verify reads the key's mark back as its placeholder.
#[test]
fn a_secret_as_a_key_is_redacted_and_the_run_checks() {
    let state = claims_state("");
    let text = r#"import { flow, insert, lookup, route } from "holon"
import partner_key from "secret:partner_key"

flow("tenants", { port: 18147 })

route.get("/who", (req) => {
  const tenants = insert({}, partner_key, "acme")
  return { tenant: lookup(tenants, req.headers["x-api-key"]) ?? "nobody" }
})
"#;
    let slug = deploy_text(&state, text, "x");
    let value = "pk_live_partner_key_value";
    let set = req("POST", &format!("/deployments/{slug}/secrets/partner_key"), serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);
    let r = serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &data::DataRequest { method: "GET".into(), url: "/who".into(), headers: vec![("x-api-key".into(), value.into())], body: vec![], by: None });
    assert_eq!((r.status, String::from_utf8(r.body).unwrap()), (200, r#"{"tenant":"acme"}"#.to_string()));
    let log = String::from_utf8(body_bytes(route(&state, &req("GET", &format!("/deployments/{slug}/log"), vec![])))).unwrap();
    assert!(!log.contains(value), "{log}");
    assert!(log.contains("‹secret:partner_key v1 #"), "{log}");
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
}

/// A file is kept out of a secret's way as a state write is: a write of a secret, or of something
/// computed from one, is refused; a part of a file read that holds one isn't kept in the log (a
/// mark there would no longer prove it part of the file); and a file text that holds one isn't
/// shipped in the export. Verify asks for the file instead of failing, and skips the refused writes.
#[test]
fn a_secret_stays_out_of_files_and_of_what_the_log_keeps_of_them() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-file-secrets-{}-{}", std::process::id(), rand::random::<u32>()));
    let (logs, out) = (dir.join("logs"), dir.join("out"));
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let value = "sk_live_file_secret_value";
    let mut text: String = (0..400).map(|i| format!("line {i} of a config\n")).collect();
    text.push_str(&format!("key = {value}\nend\n"));
    std::fs::write(logs.join("app.conf"), &text).unwrap();
    let bind = |name: &str, path: &Path, access: &str| {
        let body = serde_json::json!({ "name": name, "path": path.display().to_string(), "access": access, "keep": "bytes" });
        route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code
    };
    assert_eq!(bind("logs", &logs, "read"), 200);
    assert_eq!(bind("out", &out, "read-write"), 200);
    let flow = r#"import { base64, flow, route } from "holon"
import api_key from "secret:api_key"
import logs from "folder:logs"
import out from "folder:out"

flow("filer", { port: 18146 })

route.post("/lit", (req) => {
  out["lit.txt"] = `${req.body}:${api_key}`
  return { ok: true }
})

route.post("/der", (req) => {
  out["der.txt"] = `${req.body}:${base64(api_key)}`
  return { ok: true }
})

route.get("/tail/:name", (req) => logs.lines(req.params.name, { from: -2, limit: 2 }))

route.get("/whole/:name", (req) => logs[req.params.name])
"#;
    let slug = deploy_text(&state, flow, "");
    let set = req("POST", &format!("/deployments/{slug}/secrets/api_key"), serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);

    assert_eq!(data_call(&state, &slug, "POST", "/lit", "a").0, 200);
    assert_eq!(data_call(&state, &slug, "POST", "/der", "a").0, 200);
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "neither write landed");

    let (status, tail, _) = data_call(&state, &slug, "GET", "/tail/app.conf", "");
    assert_eq!(status, 200, "{tail}");
    assert!(tail.contains(value), "the caller gets what it read: {tail}");
    let (status, whole, _) = data_call(&state, &slug, "GET", "/whole/app.conf", "");
    assert_eq!((status, whole.len()), (200, text.len()), "{whole}");

    let log = String::from_utf8(body_bytes(route(&state, &req("GET", &format!("/deployments/{slug}/log"), vec![])))).unwrap();
    assert!(!log.contains(value), "{log}");
    let entries = log_entries(&state, &slug);
    let tail_read = &entries.iter().find(|e| e["route"] == "GET /tail/:name").unwrap()["reads"][0]["file"];
    assert!(tail_read["range"].is_object() && tail_read["range"]["text"].is_null(), "the part read isn't kept: {tail_read}");

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    assert!(!serde_json::to_string(&export).unwrap().contains(value), "the export ships no text holding it");
    assert!(export.files_not_kept >= 1, "the whole file's text is counted as not kept");
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    let skipped = report.reruns.iter().filter(|(_, r)| matches!(r, verify::Rerun::Skipped(_))).count();
    assert!(skipped >= 2, "the two reads are asked for the file: {:?}", report.reruns);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file too big to read whole is read by handle and by range: its last lines and first bytes
/// enter the run, nothing else, and the log proves them part of the file by its hashes, so the
/// runs check offline without it. A copy by handle streams the file host to host.
#[test]
fn a_big_file_is_read_by_range_and_copied_by_handle_and_its_runs_verify() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-ranges-flow-{}-{}", std::process::id(), rand::random::<u32>()));
    let (logs, archive) = (dir.join("logs"), dir.join("archive"));
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::create_dir_all(&archive).unwrap();
    let big: String = (0..60_000).map(|i| format!("line {i} of a log too big to read whole\n")).collect();
    assert!(big.len() as u64 > folders::MAX_FILE_BYTES);
    std::fs::write(logs.join("app.log"), &big).unwrap();
    let bind = |name: &str, path: &Path, access: &str, keep: &str| {
        let body = serde_json::json!({ "name": name, "path": path.display().to_string(), "access": access, "keep": keep });
        route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code
    };
    assert_eq!(bind("logs", &logs, "read", "bytes"), 200);
    assert_eq!(bind("archive", &archive, "read-write", "bytes"), 200);
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/log-archive.holon.ts"), "");

    let (status, tail, _) = data_call(&state, &slug, "GET", "/logs/app.log/tail", "");
    assert_eq!(status, 200, "{tail}");
    let tail: Vec<String> = serde_json::from_str(&tail).unwrap();
    assert_eq!((tail.len(), tail[0].as_str(), tail[19].as_str()), (20, "line 59980 of a log too big to read whole", "line 59999 of a log too big to read whole"));
    let (status, head, _) = data_call(&state, &slug, "GET", "/logs/app.log/head", "");
    assert_eq!((status, head.as_str()), (200, &big[..256]));
    assert_eq!(data_call(&state, &slug, "GET", "/logs/none.log/tail", "").0, 404, "no such file: no value");

    let (status, body, _) = data_call(&state, &slug, "POST", "/archive", r#"{"name":"app.log"}"#);
    assert_eq!(status, 200, "{body}");
    let hash = blake3::hash(big.as_bytes()).to_hex().to_string();
    let answered: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((answered["archived"].as_str().unwrap(), answered["size"].as_f64().unwrap()), (format!("app.log.{hash}").as_str(), big.len() as f64));
    assert_eq!(std::fs::read_to_string(archive.join(format!("app.log.{hash}"))).unwrap(), big, "copied whole, host to host");
    let (_, listed, _) = data_call(&state, &slug, "GET", "/archive", "");
    assert!(listed.contains(&hash), "{listed}");

    // The log holds the lines and bytes read, with what proves them part of the file; no more.
    let entries = log_entries(&state, &slug);
    let log = serde_json::to_string(&entries).unwrap();
    assert!(!log.contains("line 30000 "), "the rest of the file isn't in the log");
    let tail_read = &entries.iter().find(|e| e["route"] == "GET /logs/:name/tail").unwrap()["reads"][0]["file"];
    assert_eq!((tail_read["hash"].as_str(), tail_read["size"].as_u64()), (Some(hash.as_str()), Some(big.len() as u64)), "{tail_read}");
    assert!(tail_read["range"]["text"].as_str().unwrap().ends_with("line 59999 of a log too big to read whole\n"), "{tail_read}");
    assert!(!tail_read["range"]["proof"].as_array().unwrap().is_empty(), "{tail_read}");
    let copied = &entries.iter().find(|e| e["route"] == "POST /archive").unwrap()["performed"];
    assert!(copied.to_string().contains("FileWrite") && copied.to_string().contains(&hash), "a copy is logged by the handle it copied: {copied}");

    let export = || -> verify::Export { serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap() };
    let report = verify::check(&export(), None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert_eq!(report.reruns.iter().filter(|(_, r)| *r == verify::Rerun::Matched).count(), 5, "{:?}", report.reruns);

    // A folder that keeps hashes: a range checks only with the file given, read at its place.
    assert_eq!(bind("logs", &logs, "read", "hash"), 200);
    assert_eq!(data_call(&state, &slug, "GET", "/logs/app.log/tail", "").0, 200);
    let kept = export();
    let last = kept.lines.len() as u64 - 1;
    let without = verify::check(&kept, None, &BTreeMap::new());
    assert!(matches!(&without.reruns.last().unwrap().1, verify::Rerun::Skipped(why) if why.contains("--files")), "{:?}", without.reruns.last());
    let with = verify::check(&kept, None, &folders::files_in(&logs).unwrap());
    assert_eq!(with.reruns.last().unwrap(), &(last, verify::Rerun::Matched), "{}", verify::describe(&with, None));

    // A handle is only a record: one made up, naming a folder this flow doesn't read, copies nothing.
    let forged = "import { flow, route, status } from \"holon\"\nimport archive from \"folder:archive\"\n\nflow(\"forge\", { port: 8199 })\n\nroute.post(\"/c\", (req) => {\n  archive.copy(\"stolen.log\", { folder: \"logs\", name: \"app.log\", hash: req.json.hash })\n  return status(1)\n})\n";
    let forge = deploy_text(&state, forged, "");
    assert_eq!(data_call(&state, &forge, "POST", "/c", &format!(r#"{{"hash":"{hash}"}}"#)).0, 200);
    assert!(!archive.join("stolen.log").exists(), "a copy from a folder it doesn't read is refused");
    let outputs = log_entries(&state, &forge).last().unwrap().clone();
    assert!(outputs["outputs"].to_string().contains("file_copy:archive"), "the flow made the copy: {outputs}");
    assert_eq!(outputs["performed"], serde_json::json!([]), "and the host refused it: {outputs}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The journal sample: each run reads its page whole, so its log keeps the page's text, in the
/// blob store by content and in chunks; the folder keeps those texts for its window (a sweep
/// removes them after), and its cap stops a write that would take it past it.
#[test]
fn a_journal_keeps_what_its_runs_read_in_chunks_for_a_while_and_stays_under_its_cap() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-journal-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let bind = |keep: &str, cap: serde_json::Value| {
        let body = serde_json::json!({ "name": "journal", "path": dir.display().to_string(), "access": "read-write", "keep": keep, "cap": cap });
        route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap()))
    };
    assert_eq!(bind("bytes for 7d", serde_json::json!("1M")).status_code, 200);
    let listed = body_json(route(&state, &req("GET", "/admin/folders", vec![])));
    assert_eq!((listed[0]["keep"].as_str(), listed[0]["cap"].as_u64()), (Some("bytes for 7d"), Some(1 << 20)), "{listed}");
    assert_eq!(bind("bytes for a week", serde_json::json!(null)).status_code, 400);
    assert_eq!(bind("bytes", serde_json::json!("lots")).status_code, 400);
    assert_eq!(bind("bytes", serde_json::json!(0)).status_code, 400);
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/journal.holon.ts"), "");

    // A page already long: each run reads it whole, and keeps its text by content, in chunks.
    let page: String = (0..8_000).map(|i| format!("{i:05} an entry of the journal, written that day\n")).collect();
    std::fs::write(dir.join("today.txt"), &page).unwrap();
    let (status, body, _) = data_call(&state, &slug, "POST", "/journal/today", "one more line");
    assert_eq!(status, 200, "{body}");
    let first = state.folders.blobs.held();
    assert_eq!((first.texts, first.bytes), (1, page.len() as u64), "the page as the run read it");
    assert_eq!(data_call(&state, &slug, "POST", "/journal/today", "and another").0, 200);
    let (status, shown, _) = data_call(&state, &slug, "GET", "/journal/today", "");
    assert_eq!((status, shown.as_str()), (200, format!("{page}one more line\nand another\n").as_str()));
    let second = state.folders.blobs.held();
    assert_eq!(second.texts, 3, "the page before each run, and as the GET read it");
    let added = second.bytes - first.bytes;
    assert!(added < page.len() as u64 / 2, "a page grown by a line adds its last chunks, not a copy: {added} bytes");
    let read = &log_entries(&state, &slug)[0]["reads"][0]["file"];
    assert_eq!(read["blob"].as_str(), read["hash"].as_str(), "a whole file's text is named by its hash: {read}");

    // Its runs verify from the export, which carries those texts.
    let export = || -> verify::Export { serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap() };
    let report = verify::check(&export(), None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));

    // Within the folder's window, a sweep keeps them; past it, it removes them, and the runs that
    // read them check only with the files given.
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    assert_eq!(folders::sweep(&state.folders, &state.deployments, now + 60_000, 0).unwrap().texts, 0, "in its 7 days");
    assert_eq!(bind("bytes for 1m", serde_json::json!("1M")).status_code, 200);
    assert_eq!(folders::sweep(&state.folders, &state.deployments, now + 1_000, 0).unwrap().texts, 0, "in its minute");
    let swept = folders::sweep(&state.folders, &state.deployments, now + 120_000, 0).unwrap();
    assert_eq!((swept.texts, state.folders.blobs.held()), (3, blobs::Held::default()));
    let gone = export();
    assert_eq!((gone.files_not_kept, gone.blobs.len()), (3, 0));
    let report = verify::check(&gone, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert!(report.reruns.iter().any(|(_, r)| matches!(r, verify::Rerun::Skipped(why) if why.contains("no longer keeps"))), "{:?}", report.reruns);

    // Capped just above what it holds: a line that fits lands, one that doesn't answers 500, the
    // page as it was, and the run's log says why.
    let held = std::fs::metadata(dir.join("today.txt")).unwrap().len();
    assert_eq!(bind("bytes", serde_json::json!(held + 10)).status_code, 200);
    assert_eq!(data_call(&state, &slug, "POST", "/journal/today", "short").0, 200);
    let (status, body, _) = data_call(&state, &slug, "POST", "/journal/today", "a line too long for what is left");
    assert_eq!(status, 500, "{body}");
    assert!(body.contains("couldn't be written"), "{body}");
    assert!(std::fs::read_to_string(dir.join("today.txt")).unwrap().ends_with("and another\nshort\n"));
    let entries = log_entries(&state, &slug);
    let why = entries.last().unwrap()["error"].as_str().unwrap().to_string();
    assert!(why.contains("capped at") && why.contains("today.txt not written"), "{why}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A server on this machine that answers each request with the BLAKE3 of the body it got.
fn hashing_server() -> String {
    use std::io::Write;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some((_, got)) = read_request(&mut stream) else { continue };
            let body = blake3::hash(&got).to_hex().to_string();
            let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    host
}

/// `on.file.created(inbox, …)`: the files there at the deploy are seen, not run on; a new one
/// starts one run once it stops changing, whose handler is given its handle, and the run sends
/// its bytes by handle.
/// What the run writes into the folder it watches doesn't start a run; a file that changes runs
/// again; and the runs verify.
#[test]
fn new_files_start_runs_that_send_them_on_by_handle() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-arrivals-{}-{}", std::process::id(), rand::random::<u32>()));
    let (inbox, receipts) = (dir.join("inbox"), dir.join("receipts"));
    for (name, path) in [("inbox", &inbox), ("receipts", &receipts)] {
        std::fs::create_dir_all(path).unwrap();
        let body = serde_json::json!({ "name": name, "path": path.display().to_string(), "access": "read-write" });
        assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200);
    }
    std::fs::write(inbox.join("old.csv"), "there before").unwrap();
    let host = hashing_server();
    let text = format!(
        r#"import {{ flow, on }} from "holon"
import inbox from "folder:inbox"
import receipts from "folder:receipts"
import host from "host:{host}"

flow("drop")

on.file.created(inbox, async (f) => {{
  const sent = await host.put("/up", {{ file: f }})
  receipts[`${{f.name}}.txt`] = sent.body
  inbox[`done/${{f.name}}`] = "sent"
}})
"#
    );
    let slug = deploy_text(&state, &text, &host);
    let triggers = body_json(route(&state, &req("GET", "/deployments", vec![])))["deployments"][0]["triggers"].clone();
    assert_eq!(triggers, serde_json::json!(["on file inbox"]), "the trigger is derived from the unit: its handler's label");
    let look = || arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);

    // Dropped the moment it's deployed, before the watcher has looked: still new (old.csv was
    // seen at the deploy).
    let bytes: Vec<u8> = (0..200_000u32).flat_map(|i| i.to_le_bytes()).collect();
    std::fs::write(inbox.join("new.bin"), &bytes).unwrap();
    look(); // new, settling
    assert_eq!(log_entries(&state, &slug).len(), 0, "a file runs once it stops changing");
    look(); // the same: it runs
    let entries = log_entries(&state, &slug);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["kind"], "file");
    let sourced = entries[0]["sourced"].as_array().unwrap();
    let handle = sourced.iter().find(|p| p[0] == "FileArrival(\"inbox\")").map(|p| p[1].to_string()).unwrap_or_default();
    assert!(handle.contains("new.bin") && handle.contains("800000"), "the run's ingress is the file's handle: {sourced:?}");
    let sent = blake3::hash(&bytes).to_hex().to_string();
    assert_eq!(std::fs::read_to_string(receipts.join("new.bin.txt")).unwrap(), sent, "the server got the file's bytes");
    assert!(!receipts.join("old.csv.txt").exists(), "a file there before the deploy isn't new");
    let logged = serde_json::to_string(&entries[0]["rounds"]).unwrap();
    assert!(logged.contains(&sent) && logged.len() < 2_000, "the request is logged by hash, never its bytes: {logged}");

    // What its own run wrote (inbox/done/new.bin) starts nothing.
    look();
    look();
    assert_eq!(log_entries(&state, &slug).len(), 1);
    assert_eq!(std::fs::read_to_string(inbox.join("done/new.bin")).unwrap(), "sent");

    // A file that changes runs again.
    std::fs::write(inbox.join("new.bin"), b"shorter now").unwrap();
    look();
    look();
    assert_eq!(log_entries(&state, &slug).len(), 2);
    assert_eq!(std::fs::read_to_string(receipts.join("new.bin.txt")).unwrap(), blake3::hash(b"shorter now").to_hex().to_string());

    // Its runs check offline: the handle is what arrived, the server's answer what the log says.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A CSV of payouts (the payout-totals sample): each currency's total exact where a number's sum is
/// off, rounded half to even, kept and read back; a malformed file keeps nothing; and the runs
/// verify.
#[test]
fn a_csv_of_payouts_is_totalled_exactly() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-payouts-{}-{}", std::process::id(), rand::random::<u32>()));
    let payouts = dir.join("payouts");
    std::fs::create_dir_all(&payouts).unwrap();
    assert_eq!(route_status(&state, &serde_json::json!({ "name": "payouts", "path": payouts.display().to_string(), "access": "read" })), 200);
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/payout-totals.holon.ts"), "");
    let look = || {
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
    };
    look();
    // 0.1 + 0.2 + 0.3 as numbers is 0.6000000000000001; 1250.005 is half a cent, to even.
    std::fs::write(payouts.join("may.csv"), "\u{feff}id,currency,amount\r\n1,EUR,0.10\r\n2,EUR,0.20\r\n3,EUR,0.30\r\n4,USD,1250.005\r\n").unwrap();
    std::fs::write(payouts.join("bad.csv"), "id,currency,amount\n1,EUR,\"0.10\n").unwrap();
    look();
    let (status, answer, _) = data_call(&state, &slug, "GET", "/totals/may.csv", "");
    assert_eq!(
        (status, serde_json::from_str::<serde_json::Value>(&answer).unwrap()),
        (200, serde_json::json!({ "EUR": { "count": 3, "total": "0.60" }, "USD": { "count": 1, "total": "1250.00" } }))
    );
    let (status, _, _) = data_call(&state, &slug, "GET", "/totals/bad.csv", "");
    assert_ne!(status, 200, "a quote that never closes keeps nothing");

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// XML both ways (the payment-batch sample): a CSV of payouts becomes a pain.001 with its elements
/// in the schema's order, each amount rounded to cents and the control sum exact; the same file for
/// the same day keeps its message id, another day changes it; a file with an amount of 0, one that
/// isn't a CSV of payouts, or one the schema would refuse (an id or a name too long, an IBAN not in
/// its shape, a date that isn't one) is refused, one at its limits taken; the bank's pain.002 is
/// read back; and the runs verify.
#[test]
fn a_csv_of_payouts_becomes_a_pain001_and_its_status_report_is_read() {
    let state = claims_state("");
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/payment-batch.holon.ts"), "");
    let csv = "id,name,iban,amount,reference\nP1,Jane Roe,FR1420041010050500013M02606,100,Invoice 42\nP2,John Doe,NL91ABNA0417164300,0.10,Refund 7\nP3,Max Muster,DE75512108001245126199,0.2,Refund 8\n";
    let batch = |url: &str, csv: &str| {
        let (status, xml, _) = data_call(&state, &slug, "POST", url, csv);
        assert_eq!(status, 200, "{url}: {xml}");
        // The message id and the time it was made, read out: a hash and the clock.
        let inner = |tag: &str| xml.split(&format!("<{tag}>")).nth(1).and_then(|s| s.split(&format!("</{tag}>")).next()).unwrap().to_string();
        let (id, created) = (inner("MsgId"), inner("CreDtTm"));
        assert!(id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
        assert!(created.len() == 29 && created.ends_with("+01:00"), "{created}");
        (xml.replace(&id, "ID").replace(&created, "CREATED"), id)
    };
    let (xml, id) = batch("/batch?date=2026-11-10", csv);
    let transfer = |id: &str, amount: &str, name: &str, iban: &str, why: &str| {
        format!("      <CdtTrfTxInf>\n        <PmtId>\n          <EndToEndId>{id}</EndToEndId>\n        </PmtId>\n        <Amt>\n          <InstdAmt Ccy=\"EUR\">{amount}</InstdAmt>\n        </Amt>\n        <Cdtr>\n          <Nm>{name}</Nm>\n        </Cdtr>\n        <CdtrAcct>\n          <Id>\n            <IBAN>{iban}</IBAN>\n          </Id>\n        </CdtrAcct>\n        <RmtInf>\n          <Ustrd>{why}</Ustrd>\n        </RmtInf>\n      </CdtTrfTxInf>\n")
    };
    let expected = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Document xmlns=\"urn:iso:std:iso:20022:tech:xsd:pain.001.001.09\">\n  <CstmrCdtTrfInitn>\n    <GrpHdr>\n      <MsgId>ID</MsgId>\n      <CreDtTm>CREATED</CreDtTm>\n      <NbOfTxs>3</NbOfTxs>\n      <CtrlSum>100.30</CtrlSum>\n      <InitgPty>\n        <Nm>Acme Payouts GmbH</Nm>\n      </InitgPty>\n    </GrpHdr>\n    <PmtInf>\n      <PmtInfId>ID</PmtInfId>\n      <PmtMtd>TRF</PmtMtd>\n      <NbOfTxs>3</NbOfTxs>\n      <CtrlSum>100.30</CtrlSum>\n      <PmtTpInf>\n        <SvcLvl>\n          <Cd>SEPA</Cd>\n        </SvcLvl>\n      </PmtTpInf>\n      <ReqdExctnDt>\n        <Dt>2026-11-10</Dt>\n      </ReqdExctnDt>\n      <Dbtr>\n        <Nm>Acme Payouts GmbH</Nm>\n      </Dbtr>\n      <DbtrAcct>\n        <Id>\n          <IBAN>DE89370400440532013000</IBAN>\n        </Id>\n      </DbtrAcct>\n      <DbtrAgt>\n        <FinInstnId>\n          <BICFI>COBADEFFXXX</BICFI>\n        </FinInstnId>\n      </DbtrAgt>\n      <ChrgBr>SLEV</ChrgBr>\n{}{}{}    </PmtInf>\n  </CstmrCdtTrfInitn>\n</Document>",
        transfer("P1", "100.00", "Jane Roe", "FR1420041010050500013M02606", "Invoice 42"),
        transfer("P2", "0.10", "John Doe", "NL91ABNA0417164300", "Refund 7"),
        transfer("P3", "0.20", "Max Muster", "DE75512108001245126199", "Refund 8"),
    );
    assert_eq!(xml, expected);
    assert_eq!(batch("/batch?date=2026-11-10", csv).1, id, "the same file for the same day: one message id");
    assert_ne!(batch("/batch?date=2026-11-11", csv).1, id, "another day: another");
    let row = |r: &str| format!("id,name,iban,amount,reference\n{r}\n");
    let refused = [
        csv.replace(",0.10,", ",0,"),
        csv.replace(",0.10,", ",ten cents,"),
        "id,name\nP1,Jane Roe\n".to_string(),
        "id,name,iban,amount,reference\n".to_string(),
        // What pain.001.001.09 refuses: an id past 35 characters, a name or a reference empty or
        // past 140, an IBAN not in its shape, a control sum past 18 digits.
        row(&format!("{},Jane Roe,FR1420041010050500013M02606,1,Invoice", "P".repeat(36))),
        row("P1,,FR1420041010050500013M02606,1,Invoice"),
        row(&format!("P1,{},FR1420041010050500013M02606,1,Invoice", "N".repeat(141))),
        row("P1,Jane Roe,FR1420041010050500013M02606,1,"),
        row("P1,Jane Roe,fr14 2004 1010,1,Invoice"),
        row("P1,Jane Roe,FRX420041010050500013M02606,1,Invoice"),
        row("P1,Jane Roe,FR1420041010050500013M02606,10000000000000000,Invoice"),
    ];
    for bad in refused {
        let (status, answer, _) = data_call(&state, &slug, "POST", "/batch", &bad);
        assert_eq!(status, 400, "{bad}: {answer}");
    }
    for date in ["tomorrow", "2026-02-30", "2026-11-10T10:00:00Z", ""] {
        let (status, answer, _) = data_call(&state, &slug, "POST", &format!("/batch?date={date}"), csv);
        assert_eq!(status, 400, "{date}: {answer}");
    }
    // At the schema's limits, taken.
    batch("/batch?date=2026-11-10", &row(&format!("{},{},FR14{},9999999999999999.99,{}", "P".repeat(35), "N".repeat(140), "Ab9".repeat(10), "R".repeat(140))));

    let report = r#"<?xml version="1.0" encoding="UTF-8"?>
<Document xmlns="urn:iso:std:iso:20022:tech:xsd:pain.002.001.10">
  <CstmrPmtStsRpt>
    <GrpHdr><MsgId>STS-1</MsgId><CreDtTm>2026-11-10T09:00:00+01:00</CreDtTm></GrpHdr>
    <OrgnlGrpInfAndSts><OrgnlMsgId>M</OrgnlMsgId><OrgnlMsgNmId>pain.001.001.09</OrgnlMsgNmId><GrpSts>PART</GrpSts></OrgnlGrpInfAndSts>
    <OrgnlPmtInfAndSts>
      <OrgnlPmtInfId>M</OrgnlPmtInfId>
      <TxInfAndSts><OrgnlEndToEndId>P1</OrgnlEndToEndId><TxSts>ACCP</TxSts></TxInfAndSts>
      <TxInfAndSts><OrgnlEndToEndId>P2</OrgnlEndToEndId><TxSts>RJCT</TxSts><StsRsnInf><Rsn><Cd>AC04</Cd></Rsn></StsRsnInf></TxInfAndSts>
    </OrgnlPmtInfAndSts>
  </CstmrPmtStsRpt>
</Document>"#;
    let (status, answer, _) = data_call(&state, &slug, "POST", "/status", report);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&answer).unwrap(),
        serde_json::json!({ "group": "PART", "payments": [
            { "id": "P1", "status": "ACCP", "reason": "" },
            { "id": "P2", "status": "RJCT", "reason": "AC04" },
        ] })
    );
    for bad in [report.replace("pain.002.001.10", "pain.001.001.09"), report.replace("</Document>", ""), "<!DOCTYPE d [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><Document>&x;</Document>".to_string()] {
        let (status, answer, _) = data_call(&state, &slug, "POST", "/status", &bad);
        assert_eq!(status, 400, "{bad}: {answer}");
    }

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
}

/// Business days at a fixed offset (the due-dates sample): a payout received after the cut-off on
/// the Thursday before Easter is executed the Tuesday after, one received before it that day; an
/// invoice due on a Saturday moves to the Monday; one received now is written at the offset; and
/// the runs verify.
#[test]
fn a_payout_is_scheduled_past_the_closing_days() {
    let state = claims_state("");
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/due-dates.holon.ts"), "");
    let call = |path: &str, body: serde_json::Value| {
        let (status, answer, _) = data_call(&state, &slug, "POST", path, &body.to_string());
        assert_eq!(status, 200, "{path}: {answer}");
        serde_json::from_str::<serde_json::Value>(&answer).unwrap()
    };
    let schedule = |received: &str| call("/schedule", serde_json::json!({ "received": received, "offset": "+02:00" }));
    assert_eq!(schedule("2026-04-02T16:30:00+02:00"), serde_json::json!({ "received": "2026-04-02T16:30:00.000+02:00", "executed": "2026-04-07", "credited": "2026-04-08" }));
    assert_eq!(schedule("2026-04-02T13:59:00Z"), serde_json::json!({ "received": "2026-04-02T15:59:00.000+02:00", "executed": "2026-04-02", "credited": "2026-04-07" }));
    assert_eq!(call("/due", serde_json::json!({ "issued": "2026-01-31", "months": 1 })), serde_json::json!({ "due": "2026-03-02", "business_days": 21 }));
    let now = call("/schedule", serde_json::json!({}));
    let received = now["received"].as_str().unwrap();
    assert!(received.len() == 29 && received.ends_with("+01:00"), "now, at Frankfurt's winter offset: {received}");

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
}

/// A file's bytes (the file-checksums sample): any file, read whole, hashed as sha256sum does; a
/// small one's bytes kept in its log entry as base64, a big one's in the blob store and nowhere in
/// the log, its timeline included; bytes a route answers sent as they are; and the runs verify.
#[test]
fn a_files_bytes_are_read_hashed_logged_and_verified() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-file-bytes-{}-{}", std::process::id(), rand::random::<u32>()));
    let drop = dir.join("drop");
    std::fs::create_dir_all(&drop).unwrap();
    assert_eq!(route_status(&state, &serde_json::json!({ "name": "drop", "path": drop.display().to_string(), "access": "read" })), 200);
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/file-checksums.holon.ts"), "");
    let look = || {
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
    };
    look();
    // A PNG's first bytes, then bytes that aren't UTF-8; and 20,000 bytes, past what a log entry
    // keeps inline.
    let small = [0x89u8, 0x50, 0x4e, 0x47, 0xff, 0x00];
    let big: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(drop.join("small.png"), small).unwrap();
    std::fs::write(drop.join("big.bin"), &big).unwrap();
    look();
    let sums = [
        ("small.png", small.len(), "854e4b5f04c08dd7a8df75e0c0c9ce444f353bf1385a8a7ddbc9c73c4181ce0b", "png"),
        ("big.bin", big.len(), "93a6015a3874a774dd59fdd5db19414b301525381eb5ddcc265cdcc68bb9d350", "other"),
    ];
    for (name, size, sha256, kind) in sums {
        let (status, answer, _) = data_call(&state, &slug, "GET", &format!("/sums/{name}"), "");
        assert_eq!((status, serde_json::from_str::<serde_json::Value>(&answer).unwrap()), (200, serde_json::json!({ "size": size, "sha256": sha256, "kind": kind })), "{name}");
    }
    let entries = log_entries(&state, &slug);
    let read = |name: &str| {
        entries.iter().flat_map(|e| e["reads"].as_array().cloned().unwrap_or_default()).find(|r| r["request"].to_string().contains(name)).unwrap_or_else(|| panic!("no read of {name}: {entries:?}"))
    };
    assert_eq!(read("small.png")["answer"], serde_json::json!({ "type": "Bytes", "value": "iVBOR/8A" }), "a small file's bytes, inline");
    assert!(read("big.bin")["answer"].is_null() && read("big.bin")["file"]["blob"].is_string(), "a big one's by hash: {}", read("big.bin"));
    let timelines = serde_json::to_string(&entries.iter().map(|e| &e["timeline"]).collect::<Vec<_>>()).unwrap();
    assert!(timelines.contains("iVBOR/8A"), "a small file's bytes are in its timeline too: {timelines}");
    let whole = holon_core::bytes_text(&big, "base64").unwrap();
    assert!(!serde_json::to_string(&entries).unwrap().contains(&whole[..64]), "a big one's nor in its timeline");

    // Bytes a route answers go out as they are.
    let raw = deploy_text(&state, "import { flow, route } from \"holon\"\nimport drop from \"folder:drop\"\n\nflow(\"raw\", { port: 18303 })\n\nroute.get(\"/raw/:name\", (req) => drop.bytes(req.params.name))\n", "");
    let r = serve_data(&state.store, &state.deployments, &state.deployments_path, &raw, &fake_data("GET", "/raw/small.png", vec![]));
    assert_eq!((r.status, r.body.as_slice()), (200, &small[..]));
    assert!(r.headers.iter().any(|(k, v)| k == "content-type" && v == "application/octet-stream"), "{:?}", r.headers);

    // Its runs check offline: the small file's bytes from its entry, the big one's from the export.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    assert_eq!(export.blob_bytes.len(), 1, "the big file's bytes, in base64");
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A bank's deliveries (the statement-archive sample): a zip of camt.053 statements unzipped, and
/// one statement gzipped by flate2 gunzipped, each read for its account and closing balance; each
/// delivery archived as a zip written as bytes, a manifest beside its statements, that another
/// tool reads (flate2 inflating each file, its checksum checked); a statement
/// served back gzipped when the client takes it; what isn't a delivery refused; the archive kept
/// in the log by its hash only; and the runs verify.
#[test]
fn a_delivery_is_unzipped_read_and_archived_as_a_zip_written_as_bytes() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-statements-{}-{}", std::process::id(), rand::random::<u32>()));
    let (inbox, archive) = (dir.join("inbox"), dir.join("archive"));
    for (name, path, access) in [("inbox", &inbox, "read"), ("archive", &archive, "read-write")] {
        std::fs::create_dir_all(path).unwrap();
        assert_eq!(route_status(&state, &serde_json::json!({ "name": name, "path": path.display().to_string(), "access": access })), 200);
    }
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/statement-archive.holon.ts"), "");
    let look = || {
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
    };
    look();
    // A camt.053 statement: an opening balance, then the closing one the sample reads; and a note
    // that doesn't compress, so the zips are past what a log entry keeps inline.
    let note: String = (0..200u32).map(|i| blake3::hash(&i.to_le_bytes()).to_hex().to_string()).collect();
    let statement = |iban: &str, closing: &str, side: &str| {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><Document xmlns="urn:iso:std:iso:20022:tech:xsd:camt.053.001.08"><BkToCstmrStmt><GrpHdr><MsgId>M1</MsgId><CreDtTm>2026-11-06T18:00:00</CreDtTm></GrpHdr><Stmt><Id>S1</Id><Acct><Id><IBAN>{iban}</IBAN></Id></Acct><Bal><Tp><CdOrPrtry><Cd>OPBD</Cd></CdOrPrtry></Tp><Amt Ccy="EUR">1000.00</Amt><CdtDbtInd>CRDT</CdtDbtInd><Dt><Dt>2026-11-05</Dt></Dt></Bal><Bal><Tp><CdOrPrtry><Cd>CLBD</Cd></CdOrPrtry></Tp><Amt Ccy="EUR">{closing}</Amt><CdtDbtInd>{side}</CdtDbtInd><Dt><Dt>2026-11-06</Dt></Dt></Bal><AddtlStmtInf>{note}</AddtlStmtInf></Stmt></BkToCstmrStmt></Document>"#
        )
    };
    let (fr, de) = (statement("FR1420041010050500013M02606", "1250.50", "CRDT"), statement("DE89370400440532013000", "75.25", "DBIT"));
    let leaf = |name: &str, input: Value| holon_core::primitives::compute(name, &[&input], &[]).unwrap_or_else(|| panic!("{name} gave nothing"));
    let bytes = |s: &str| Value::Bytes(s.as_bytes().to_vec());
    let sha = |s: &str| match holon_core::primitives::compute("sha256", &[&bytes(s)], &[holon_core::Param::frozen("encoding", "hex")]) {
        Some(Value::Text(t)) => t,
        other => panic!("{other:?}"),
    };
    let Value::Bytes(zipped) = leaf("zip", Value::Record(BTreeMap::from([("fr.xml".to_string(), bytes(&fr)), ("de.xml".to_string(), bytes(&de))]))) else { panic!("zip gives bytes") };
    std::fs::write(inbox.join("2026-11-06.zip"), &zipped).unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, fr.as_bytes()).unwrap();
    std::fs::write(inbox.join("2026-11-07.xml.gz"), gz.finish().unwrap()).unwrap();
    std::fs::write(inbox.join("junk.zip"), "not a zip").unwrap();
    look();

    let (status, answer, _) = data_call(&state, &slug, "GET", "/deliveries", "");
    let summary = |file: &str, iban: &str, closing: &str, text: &str| serde_json::json!({ "file": file, "account": iban, "date": "2026-11-06", "closing": closing, "currency": "EUR", "sha256": sha(text) });
    let (fr_summary, de_summary) = (summary("fr.xml", "FR1420041010050500013M02606", "1250.50", &fr), summary("de.xml", "DE89370400440532013000", "-75.25", &de));
    assert_eq!(
        (status, serde_json::from_str::<serde_json::Value>(&answer).unwrap()),
        (
            200,
            serde_json::json!({
                "2026-11-06.zip": { "archive": "2026-11-06.zip", "statements": [de_summary, fr_summary] },
                "2026-11-07.xml.gz": { "archive": "2026-11-07.xml.zip", "statements": [summary("2026-11-07.xml", "FR1420041010050500013M02606", "1250.50", &fr)] },
                "junk.zip": { "refused": "not a zip of camt.053 statements, or one gzipped" },
            })
        )
    );

    // The archive, read as another tool reads a zip (each local header in turn, its file inflated
    // and its checksum taken by flate2, not by the zip leaf): the statements as they came, and the
    // manifest beside them.
    let unzipped = |name: &str| {
        let zip = std::fs::read(archive.join(name)).unwrap();
        let u16_at = |at: usize| usize::from(u16::from_le_bytes([zip[at], zip[at + 1]]));
        let u32_at = |at: usize| u32::from_le_bytes(zip[at..at + 4].try_into().unwrap());
        let (mut files, mut at) = (BTreeMap::new(), 0);
        while u32_at(at) == 0x0403_4b50 {
            let (method, size, named, extra) = (u16_at(at + 8), u32_at(at + 18) as usize, u16_at(at + 26), u16_at(at + 28));
            let data = &zip[at + 30 + named + extra..][..size];
            let mut file = Vec::new();
            match method {
                0 => file.extend_from_slice(data),
                8 => drop(std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(data), &mut file).unwrap()),
                m => panic!("{name}: method {m}"),
            }
            let mut crc = flate2::Crc::new();
            crc.update(&file);
            assert_eq!(crc.sum(), u32_at(at + 14), "{name}: its checksum");
            files.insert(String::from_utf8(zip[at + 30..at + 30 + named].to_vec()).unwrap(), Value::Bytes(file));
            at += 30 + named + extra + size;
        }
        files
    };
    let day = unzipped("2026-11-06.zip");
    assert_eq!(day.keys().map(String::as_str).collect::<Vec<_>>(), ["de.xml", "fr.xml", "manifest.csv"]);
    assert_eq!((&day["de.xml"], &day["fr.xml"]), (&bytes(&de), &bytes(&fr)));
    let manifest = format!(
        "file,account,date,closing,currency,sha256\r\nde.xml,DE89370400440532013000,2026-11-06,-75.25,EUR,{}\r\nfr.xml,FR1420041010050500013M02606,2026-11-06,1250.50,EUR,{}\r\n",
        sha(&de),
        sha(&fr)
    );
    assert_eq!(day["manifest.csv"], bytes(&manifest));
    assert_eq!(unzipped("2026-11-07.xml.zip").keys().map(String::as_str).collect::<Vec<_>>(), ["2026-11-07.xml", "manifest.csv"]);

    // A statement served back from its archive: gzipped when the client takes it, else as it is.
    let get = |accept: &[(String, String)]| {
        let asked = data::DataRequest { method: "GET".into(), url: "/deliveries/2026-11-06.zip/fr.xml".into(), headers: accept.to_vec(), body: vec![], by: None };
        serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &asked)
    };
    let r = get(&[("accept-encoding".into(), "gzip, br".into())]);
    assert!(r.headers.iter().any(|(k, v)| k == "content-encoding" && v == "gzip"), "{:?}", r.headers);
    let mut back = String::new();
    std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&r.body[..]), &mut back).unwrap();
    assert_eq!((r.status, back.as_str()), (200, fr.as_str()), "gzipped, flate2 reads it");
    let r = get(&[]);
    assert_eq!((r.status, r.body.as_slice()), (200, fr.as_bytes()));
    assert!(!r.headers.iter().any(|(k, _)| k == "content-encoding"), "{:?}", r.headers);

    // The archive is in the log by its hash, its bytes nowhere: not in what was performed, nor in
    // the timeline.
    let entries = log_entries(&state, &slug);
    let archived = std::fs::read(archive.join("2026-11-06.zip")).unwrap();
    let performed = serde_json::to_string(&entries.iter().map(|e| &e["performed"]).collect::<Vec<_>>()).unwrap();
    assert!(performed.contains(&blake3::hash(&archived).to_hex().to_string()), "the archive written, by its hash: {performed}");
    let whole = holon_core::bytes_text(&archived, "base64").unwrap();
    assert!(!serde_json::to_string(&entries).unwrap().contains(&whole[..64]), "its bytes aren't in the log");

    // Its runs check offline: the deliveries read from the export's blobs.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Partners' tokens (the jwt-api sample): a person trusts each partner's public key, a JWKS or a
/// PEM text, by running trust; a bearer token is verified with the key of the partner it names,
/// for payouts-api, at the time of the run. A token another partner signed, alg none, HMAC over
/// the public key, or one expired or for another audience answers 401; a payout needs
/// payouts:write (403), and a token pays once (409); and the runs verify.
#[test]
fn a_partners_token_is_verified_with_the_key_a_person_trusted_for_it() {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    let state = claims_state("");
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/jwt-api.holon.ts"), "");
    let trust = |partner: &str, key: serde_json::Value| run_action_named(&state, &slug, "trust", serde_json::json!({ "partner": partner, "key": key })).status_code;
    assert_eq!(trust("acme", serde_json::from_str(include_str!("testdata/oidc-test-jwks.json")).unwrap()), 200, "a JWKS");
    assert_eq!(trust("globex", serde_json::json!(include_str!("testdata/jwt-test-ed25519.pub.pem"))), 200, "a PEM text");

    let now = crate::deploy::epoch_now();
    let claims = |iss: &str, jti: &str, more: serde_json::Value| {
        let mut all = serde_json::json!({ "iss": iss, "sub": "svc-1", "aud": "payouts-api", "scope": "payouts:write", "jti": jti, "iat": now, "exp": now + 600 });
        for (k, v) in more.as_object().unwrap() {
            all[k] = v.clone();
        }
        all
    };
    let acme = |c: serde_json::Value| {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-key".into());
        jsonwebtoken::encode(&header, &c, &EncodingKey::from_rsa_pem(include_bytes!("testdata/oidc-test-key.pem")).unwrap()).unwrap()
    };
    let globex = |c: serde_json::Value| jsonwebtoken::encode(&Header::new(Algorithm::EdDSA), &c, &EncodingKey::from_ed_pem(include_bytes!("testdata/jwt-test-ed25519.pem")).unwrap()).unwrap();
    let call = |method: &str, url: &str, token: &str, body: &str| {
        let headers = if token.is_empty() { vec![] } else { vec![("authorization".to_string(), format!("Bearer {token}"))] };
        let asked = data::DataRequest { method: method.into(), url: url.into(), headers, body: body.as_bytes().to_vec(), by: None };
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &asked);
        (r.status, serde_json::from_slice::<serde_json::Value>(&r.body).unwrap_or_default(), r.headers)
    };

    // Who called, as each partner's key says.
    let (status, answer, _) = call("GET", "/whoami", &acme(claims("acme", "j1", serde_json::json!({}))), "");
    assert_eq!((status, &answer["partner"], &answer["subject"], &answer["scope"]), (200, &serde_json::json!("acme"), &serde_json::json!("svc-1"), &serde_json::json!("payouts:write")), "{answer}");
    assert_eq!(answer["expires"].as_f64(), Some((now + 600) as f64));
    let (status, answer, _) = call("GET", "/whoami", &globex(claims("globex", "g1", serde_json::json!({}))), "");
    assert_eq!((status, &answer["partner"]), (200, &serde_json::json!("globex")), "{answer}");

    // What doesn't verify: no token; one acme signed naming globex; alg none; HMAC with globex's
    // public key as its secret; one expired; one for another API.
    let (status, answer, headers) = call("GET", "/whoami", "", "");
    assert_eq!((status, answer["error"].as_str()), (401, Some("a token a trusted partner signed for payouts-api is needed")));
    assert!(headers.iter().any(|(k, v)| k == "www-authenticate" && v == "Bearer"), "{headers:?}");
    let b64u = |s: String| holon_core::bytes_text(s.as_bytes(), "base64url").unwrap();
    let none = format!("{}.{}.", b64u(r#"{"alg":"none"}"#.into()), b64u(claims("globex", "n1", serde_json::json!({})).to_string()));
    let confused = jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims("globex", "h1", serde_json::json!({})), &EncodingKey::from_secret(include_bytes!("testdata/jwt-test-ed25519.pub.pem"))).unwrap();
    for (what, token) in [
        ("signed by another partner", acme(claims("globex", "x1", serde_json::json!({})))),
        ("alg none", none),
        ("HMAC over the public key", confused),
        ("expired", globex(claims("globex", "e1", serde_json::json!({ "exp": now - 60 })))),
        ("for another API", globex(claims("globex", "a1", serde_json::json!({ "aud": "billing-api" })))),
    ] {
        assert_eq!(call("GET", "/whoami", &token, "").0, 401, "{what}");
        assert_eq!(call("POST", "/payouts", &token, r#"{"amount": "1.00"}"#).0, 401, "{what}");
    }

    // A payout: the scope it needs, then once per token.
    let (status, answer, _) = call("POST", "/payouts", &acme(claims("acme", "j2", serde_json::json!({ "scope": "payouts:read" }))), r#"{"amount": "5.00"}"#);
    assert_eq!((status, answer), (403, serde_json::json!({ "error": "the token's scope has no payouts:write" })));
    let token = acme(claims("acme", "j1", serde_json::json!({})));
    let (status, answer, _) = call("POST", "/payouts", &token, r#"{"amount": "120.00"}"#);
    assert_eq!((status, answer), (202, serde_json::json!({ "accepted": "acme/j1" })));
    let (status, answer, _) = call("POST", "/payouts", &token, r#"{"amount": "120.00"}"#);
    assert_eq!((status, answer), (409, serde_json::json!({ "error": "this token has asked for a payout already", "payout": "acme/j1" })));
    let (status, answer, _) = call("POST", "/payouts", &globex(claims("globex", "g1", serde_json::json!({ "scope": "payouts:read payouts:write" }))), r#"{"amount": "7.50"}"#);
    assert_eq!((status, answer), (202, serde_json::json!({ "accepted": "globex/g1" })));

    // Its runs check offline: each token verified again at the time its run saw.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
}

/// A bank's files (the pgp-outbox sample), with keys, signatures and a message GnuPG made
/// (holon-core's testdata/pgp-vectors.mjs): a person trusts the bank's Curve25519 key, its
/// fingerprint answered as GnuPG lists it, and nothing kept for what isn't a key; a payment file is
/// encrypted to the key's encryption subkey, armored, its SHA-256 and the encrypted file's kept,
/// and refused before a key is trusted; a report signed by the bank's signing subkey verifies,
/// one another key signed or with no report beside it is refused; and the runs verify, the
/// encryption done again from the random bytes the log keeps.
#[test]
fn a_payment_file_is_encrypted_to_the_banks_key_and_its_signed_reports_verified() {
    let state = claims_state("");
    let vectors: serde_json::Value = serde_json::from_str(include_str!("../../holon-core/src/testdata/pgp-vectors.json")).unwrap();
    let dir = std::env::temp_dir().join(format!("holon-pgp-{}-{}", std::process::id(), rand::random::<u32>()));
    let (payments, upload, download) = (dir.join("payments"), dir.join("upload"), dir.join("download"));
    for (name, path, access) in [("payments", &payments, "read"), ("upload", &upload, "read-write"), ("download", &download, "read")] {
        std::fs::create_dir_all(path).unwrap();
        assert_eq!(route_status(&state, &serde_json::json!({ "name": name, "path": path.display().to_string(), "access": access })), 200);
    }
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/pgp-outbox.holon.ts"), "");
    let look = || {
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
    };
    look();
    let trust = |key: &serde_json::Value| body_json(run_action_named(&state, &slug, "trust", serde_json::json!({ "key": key })));
    let kept = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/state"), vec![])))["state"]["bank"].clone();
    let sha = |b: &[u8]| match holon_core::primitives::compute("sha256", &[&Value::Bytes(b.to_vec())], &[holon_core::Param::frozen("encoding", "hex")]) {
        Some(Value::Text(t)) => t,
        other => panic!("{other:?}"),
    };

    // What isn't a key is refused and kept as nothing, so a payment file is refused.
    assert_eq!(trust(&serde_json::json!("not a key"))["answer"], serde_json::json!({ "refused": "not one OpenPGP public key its own signatures hold for" }));
    assert!(kept().is_null(), "nothing kept: {}", kept());
    std::fs::write(payments.join("early.xml"), "<Document/>").unwrap();
    look();

    // The bank's key: its fingerprint, user ID and no expiry, as GnuPG lists them.
    let fingerprints = &vectors["fingerprints"]["c25519"];
    let answered = trust(&vectors["keys"]["c25519"]);
    assert_eq!(answered["answer"]["fingerprint"], fingerprints[0], "{answered}");
    assert_eq!(answered["answer"]["user_ids"], serde_json::json!(["Bank 25519 <c25519@bank.example>"]), "{answered}");
    assert_eq!((&answered["answer"]["expires"], &answered["answer"]["revoked"]), (&serde_json::json!("never"), &serde_json::json!(false)), "{answered}");
    let pain = "<Document><CstmrCdtTrfInitn><GrpHdr><MsgId>B-1</MsgId></GrpHdr></CstmrCdtTrfInitn></Document>\n";
    std::fs::write(payments.join("pain.001.xml"), pain).unwrap();
    look();

    // Armored, a session key for the encryption subkey first: past its packet's header (gpg writes
    // the old form, 0x84, Holon the new, 0xC1), its version, key ID and algorithm are those of the
    // message gpg encrypted to the same key, bytes 3 to 12.
    let encrypted = std::fs::read(upload.join("pain.001.xml.asc")).unwrap();
    let armored = String::from_utf8(encrypted.clone()).unwrap();
    assert!(armored.starts_with("-----BEGIN PGP MESSAGE-----\n") && armored.trim_end().ends_with("-----END PGP MESSAGE-----"), "{armored}");
    let opening = |armored: &str| armored.lines().skip_while(|l| !l.is_empty()).nth(1).unwrap()[4..16].to_string();
    assert_eq!(opening(&armored), opening(vectors["messages"]["to_c25519_armored"].as_str().unwrap()));
    let (status, answer, _) = data_call(&state, &slug, "GET", "/sent", "");
    let sent: serde_json::Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(status, 200);
    assert_eq!(sent["early.xml"], serde_json::json!({ "refused": "no bank key is trusted, or none of its keys encrypts now" }));
    let one = &sent["pain.001.xml"];
    assert_eq!((&one["to"], one["sha256"].as_str(), one["encrypted"].as_str()), (&fingerprints[0], Some(sha(pain.as_bytes()).as_str()), Some(sha(&encrypted).as_str())), "{sent}");

    // The bank's reports: signed by its signing subkey (armored); by another key (bytes); a
    // signature with no report beside it.
    let data = vectors["data"].as_str().unwrap();
    let signature = |name: &str| vectors["signatures"][name].as_str().unwrap().to_string();
    std::fs::write(download.join("pain.002.xml"), data).unwrap();
    std::fs::write(download.join("pain.002.xml.sig"), signature("ed_subkey_armored")).unwrap();
    std::fs::write(download.join("forged.xml"), data).unwrap();
    std::fs::write(download.join("forged.xml.sig"), holon_core::text_bytes(&signature("other"), "base64").unwrap()).unwrap();
    std::fs::write(download.join("missing.xml.sig"), signature("ed_subkey_armored")).unwrap();
    look();
    let (_, answer, _) = data_call(&state, &slug, "GET", "/received", "");
    let refused = serde_json::json!({ "refused": "not signed by the bank's trusted key, or not there" });
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&answer).unwrap(),
        serde_json::json!({
            "pain.002.xml": { "verified": fingerprints[2], "signed": 1780272000, "sha256": sha(data.as_bytes()) },
            "forged.xml": refused,
            "missing.xml": refused,
        })
    );

    // Its runs check offline: the message encrypted again from the random bytes its run saw.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `on file` lines (the file-audit sample): several folders, files removed, and only some names.
/// Each run's `ingress` says what happened; a name no line is for starts nothing; an entry is
/// created, never replaced; and the runs verify.
#[test]
fn on_file_lines_run_on_several_folders_deletions_and_matching_names() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-file-lines-{}-{}", std::process::id(), rand::random::<u32>()));
    let (inbox, outbox, audit) = (dir.join("inbox"), dir.join("outbox"), dir.join("audit"));
    for (name, path, access) in [("inbox", &inbox, "read"), ("outbox", &outbox, "read"), ("audit", &audit, "read-write")] {
        std::fs::create_dir_all(path).unwrap();
        assert_eq!(route_status(&state, &serde_json::json!({ "name": name, "path": path.display().to_string(), "access": access })), 200);
    }
    std::fs::write(inbox.join("old.txt"), "there before").unwrap();
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/file-audit.holon.ts"), "");
    // Deployed as units alone, with no text, its handlers come in its composite's order, which
    // follows hashes rather than the order they are written in.
    let mut triggers: Vec<String> = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments", vec![])))["deployments"][0]["triggers"].clone()).unwrap();
    triggers.sort();
    assert_eq!(triggers, ["on file inbox deleted", "on file inbox matching \"*.csv\"", "on file outbox"], "each file handler its own trigger");
    let look = || {
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
    };
    let events = || -> Vec<(String, String, String)> {
        log_entries(&state, &slug)
            .iter()
            .map(|e| {
                let sourced = e["sourced"].as_array().unwrap();
                let h = &sourced.iter().find(|p| p[0].as_str().unwrap().starts_with("FileArrival")).unwrap()[1];
                let fields = h["value"].as_array().unwrap();
                let t = |k: &str| fields.iter().find(|f| f[0] == k).and_then(|f| f[1]["value"].as_str()).unwrap_or_default().to_string();
                (t("folder"), t("event"), t("name"))
            })
            .collect()
    };

    std::fs::write(inbox.join("a.csv"), "x,1\n").unwrap();
    std::fs::write(inbox.join("b.txt"), "not a csv").unwrap();
    std::fs::write(outbox.join("o.pdf"), "any name").unwrap();
    look();
    let s = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
    assert_eq!(events(), [s("inbox", "new", "a.csv"), s("outbox", "new", "o.pdf")], "b.txt isn't a csv");

    std::fs::write(inbox.join("a.csv"), "x,1\ny,2\n").unwrap();
    std::fs::remove_file(inbox.join("old.txt")).unwrap();
    look();
    assert_eq!(&events()[2..], [s("inbox", "deleted", "old.txt"), s("inbox", "changed", "a.csv")], "removals are for every name");
    let entries = log_entries(&state, &slug);
    let deleted = entries[2]["sourced"].to_string();
    assert!(!deleted.contains("\"hash\""), "a removed file has no hash: {deleted}");

    let written: Vec<String> = ["inbox", "outbox"]
        .iter()
        .flat_map(|f| std::fs::read_dir(audit.join(f)).unwrap().map(move |e| format!("{f}/{}", e.unwrap().file_name().to_string_lossy())))
        .collect();
    assert_eq!(written.len(), 4, "{written:?}");
    assert!(written.iter().any(|n| n.starts_with("inbox/") && n.ends_with("-deleted-old.txt.json")), "{written:?}");
    assert!(written.iter().any(|n| n.starts_with("outbox/") && n.ends_with("-new-o.pdf.json")), "{written:?}");

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file its own run deletes from a folder it watches for removals starts nothing; one removed
/// by anyone else does.
#[test]
fn a_file_a_run_deletes_starts_no_run() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-own-deletes-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(route_status(&state, &serde_json::json!({ "name": "inbox", "path": dir.display().to_string(), "access": "read-write" })), 200);
    std::fs::write(dir.join("keep.txt"), "there before").unwrap();
    let slug = deploy_text(&state, "import { flow, on } from \"holon\"\nimport inbox from \"folder:inbox\"\n\nflow(\"tidy\")\n\non.file.created(inbox, (f) => {\n  inbox.delete(f.name)\n}, { matching: \"*.tmp\" })\n\non.file.deleted(inbox, (f) => {\n  inbox.delete(f.name)\n})\n", "");
    let look = || {
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
    };
    std::fs::write(dir.join("a.tmp"), "scratch").unwrap();
    look();
    assert!(!dir.join("a.tmp").exists(), "its run deleted it");
    look();
    assert_eq!(log_entries(&state, &slug).len(), 1, "and that starts nothing");
    std::fs::remove_file(dir.join("keep.txt")).unwrap();
    look();
    assert_eq!(log_entries(&state, &slug).len(), 2, "someone else's removal does");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A run that read a row, waited for an event, then calls out and writes the row (§7.4 of the
/// databases L4). Parked, it holds nothing on the row.
fn waiting_tally_text(host: &str) -> String {
    format!(
        "import {{ action, event, flow, route }} from \"holon\"\nimport shop from \"database:shop\"\nimport host from \"host:{host}\"\n\nflow(\"paid-tally\", {{ port: 18191 }})\n\nroute.post(\"/hold\", () => {{\n  const started = tally()\n  return {{ token: started.waiting.paid }}\n}})\n\nexport const tally = action(async () => {{\n  const row = shop.one(\"select n from counts where id = $1\", 1)\n  const asked = (await host.get(\"/fast\")).body\n  const paid = await event(\"paid\", {{ expires: \"1h\" }})\n  const later = (await host.get(\"/slow\", {{\n    headers: {{ \"x-paid\": JSON.stringify(paid.body), \"x-asked\": asked }},\n  }})).body\n  shop.exec(\"update counts set n = $1, note = $2 where id = $3\", row.n + 1, later, 1, {{ expect: 1 }})\n}})\n\nroute.get(\"/paid/:token\", () => \"ok\", {{ resumes: \"paid\" }})\n"
    )
}

/// A resumed run checks what it shares at its commit as any run does: a program outside Holon
/// changes the row it read while its resumed part calls out, so its commit finds the row changed,
/// and it runs again from where it last waited, reading the row as it now is. The requests of
/// both its parts are answered from its record, not sent again, its event too; and it checks
/// offline. On SQLite, and on Postgres and MySQL when they are at hand.
#[test]
fn a_resumed_run_whose_row_changed_runs_again_from_its_last_wait() {
    let dir = std::env::temp_dir().join(format!("holon-resumed-row-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("counts.db");
    let schema = "create table counts (id integer primary key, n integer not null, note varchar(16)); insert into counts (id, n) values (1, 0)";
    let fresh = fresh_databases("resumed");
    let mut engines: Vec<(&str, String)> = vec![("sqlite", file.display().to_string())];
    engines.extend(fresh.iter().map(|db| (db.engine, db.url.clone())));
    for (engine, url) in engines {
        let (host, hits) = counting_server();
        let state = if engine == "sqlite" { claims_state("") } else { server_with_database(&url) };
        let bound = match engine {
            "sqlite" => {
                let body = serde_json::json!({ "name": "shop", "path": url, "access": "read-write", "schema": schema });
                route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap()))
            }
            _ => bind_shop(&state, Some(schema)),
        };
        assert_eq!(bound.status_code, 200, "{engine}");
        let slug = deploy_text(&state, &waiting_tally_text(&host), &host);
        let outside = |sql: &str| match engine {
            "sqlite" => rusqlite::Connection::open(&url).unwrap().execute_batch(sql).unwrap(),
            "postgres" => postgres::Client::connect(&url, postgres::NoTls).unwrap().batch_execute(sql).unwrap(),
            _ => {
                use mysql::prelude::Queryable;
                mysql::Conn::new(mysql::Opts::from_url(&url).unwrap()).unwrap().query_drop(sql).unwrap()
            }
        };
        let n = || match engine {
            "sqlite" => rusqlite::Connection::open(&url).unwrap().query_row("select n from counts where id = 1", [], |r| r.get::<_, i64>(0)).unwrap(),
            "postgres" => postgres::Client::connect(&url, postgres::NoTls).unwrap().query_one("select n from counts where id = 1", &[]).unwrap().get::<_, i32>(0) as i64,
            _ => {
                use mysql::prelude::Queryable;
                mysql::Conn::new(mysql::Opts::from_url(&url).unwrap()).unwrap().query_first::<i64, _>("select n from counts where id = 1").unwrap().unwrap()
            }
        };

        let (status, held, _) = data_call(&state, &slug, "POST", "/hold", "");
        assert_eq!(status, 200, "{engine}: {held}");
        let token = serde_json::from_str::<serde_json::Value>(&held).unwrap()["token"].as_str().unwrap().to_string();
        // While it waits the row changes: it is read as it is when the run resumes.
        outside("update counts set n = 10 where id = 1");
        let paid = std::thread::scope(|s| {
            let paid = s.spawn(|| data_call(&state, &slug, "GET", &format!("/paid/{token}"), ""));
            // And again while its resumed part calls out: its commit finds it changed.
            std::thread::sleep(Duration::from_millis(150));
            outside("update counts set n = 100 where id = 1");
            paid.join().unwrap()
        });
        assert_eq!(paid.0, 200, "{engine}: {paid:?}");
        assert_eq!(n(), 101, "{engine}: it ran again on 100, not over it");
        assert_eq!((hits_of(&hits, "/fast"), hits_of(&hits, "/slow")), (1, 1), "{engine}: each request went out once");
        let entries = log_entries(&state, &slug);
        let resumed = entries.iter().find(|e| e["kind"] == "resume").unwrap_or_else(|| panic!("{engine}: {entries:#?}"));
        assert_eq!(resumed["retries"], 1, "{engine}: {resumed}");
        assert!(resumed["error"].is_null(), "{engine}: {resumed}");
        let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
        let report = verify::check(&export, None, &BTreeMap::new());
        assert!(report.ok(), "{engine}: {}", verify::describe(&report, None));
        // Every part re-runs: the request and the first part of the run it started; the token it
        // was handed back, and the request that carried it, checked given what the log kept; and
        // the resumed part that ran again too.
        let matched = report.reruns.iter().filter(|(_, r)| *r == verify::Rerun::Matched).count();
        assert_eq!(matched, 4, "{engine}: the resumed part that ran again re-runs offline: {}", verify::describe(&report, None));
        assert!(replays_as_it_verifies(&state, &slug).contains("resume"), "{engine}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same for a file: a resumed run whose file someone changed while it called out runs again
/// on the file as it now is, and loses no line.
#[test]
fn a_resumed_run_whose_file_changed_runs_again_from_its_last_wait() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-resumed-file-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lines.txt"), "first\n").unwrap();
    let body = serde_json::json!({ "name": "ledger", "path": dir.display().to_string(), "access": "read-write" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let text = format!(
        "import {{ action, event, flow, route }} from \"holon\"\nimport ledger from \"folder:ledger\"\nimport host from \"host:{host}\"\n\nflow(\"paid-lines\", {{ port: 18192 }})\n\nroute.post(\"/hold\", () => {{\n  const started = append()\n  return {{ token: started.waiting.paid }}\n}})\n\nexport const append = action(async () => {{\n  const before = ledger[\"lines.txt\"] ?? \"\"\n  const paid = await event(\"paid\", {{ expires: \"1h\" }})\n  const later = (await host.get(\"/slow\", {{\n    headers: {{ \"x-paid\": JSON.stringify(paid.body) }},\n  }})).body\n  ledger[\"lines.txt\"] = `${{before}}${{later}}\n`\n}})\n\nroute.get(\"/paid/:token\", () => \"ok\", {{ resumes: \"paid\" }})\n"
    );
    let slug = deploy_text(&state, &text, &host);
    let (status, held, _) = data_call(&state, &slug, "POST", "/hold", "");
    assert_eq!(status, 200, "{held}");
    let token = serde_json::from_str::<serde_json::Value>(&held).unwrap()["token"].as_str().unwrap().to_string();
    let paid = std::thread::scope(|s| {
        let paid = s.spawn(|| data_call(&state, &slug, "GET", &format!("/paid/{token}"), ""));
        std::thread::sleep(Duration::from_millis(150));
        std::fs::write(dir.join("lines.txt"), "first\nsecond\n").unwrap();
        paid.join().unwrap()
    });
    assert_eq!(paid.0, 200, "{paid:?}");
    assert_eq!(std::fs::read_to_string(dir.join("lines.txt")).unwrap(), "first\nsecond\nslow\n", "it ran again on the file as it now is");
    assert_eq!(hits_of(&hits, "/slow"), 1, "its request went out once");
    let entries = log_entries(&state, &slug);
    let resumed = entries.iter().find(|e| e["kind"] == "resume").unwrap();
    assert_eq!(resumed["retries"], 1, "{resumed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A resumed run whose commit landed, the server stopping before it could forget the run, is
/// resumed again by the same event: it reads the row its own commit wrote, so its check finds
/// nothing changed, and its database has it marked done, so its commit applies nothing again.
#[test]
fn a_resumed_run_resumed_again_after_its_commit_applies_once() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-resumed-done-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("counts.db");
    let schema = "create table counts (id integer primary key, n integer not null, note varchar(16)); insert into counts (id, n) values (1, 0)";
    let body = serde_json::json!({ "name": "shop", "path": file.display().to_string(), "access": "read-write", "schema": schema });
    assert_eq!(route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let slug = deploy_text(&state, &waiting_tally_text(&host), &host);
    let (_, held, _) = data_call(&state, &slug, "POST", "/hold", "");
    let token = serde_json::from_str::<serde_json::Value>(&held).unwrap()["token"].as_str().unwrap().to_string();
    let record = state.deployments.lock().unwrap()[&slug].parked.values().next().cloned().unwrap();
    assert_eq!(data_call(&state, &slug, "GET", &format!("/paid/{token}"), "").0, 200);
    // As if the server stopped once it had committed: the run is still parked.
    state.deployments.lock().unwrap().get_mut(&slug).unwrap().parked.insert(record.run_id.clone(), record);
    assert_eq!(data_call(&state, &slug, "GET", &format!("/paid/{token}"), "").0, 200);
    let n: i64 = rusqlite::Connection::open(&file).unwrap().query_row("select n from counts where id = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "applied once");
    let entries = log_entries(&state, &slug);
    let again = entries.iter().filter(|e| e["kind"] == "resume").nth(1).unwrap_or_else(|| panic!("{entries:#?}"));
    assert!(again["error"].is_null() && again["retries"].is_null(), "committed at once, nothing written again: {again}");
    assert_eq!(hits_of(&hits, "/slow"), 2, "its record of the first resume was lost with the stop");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The approved-payout sample: a deposit while the payout waits for two people's approval goes
/// ahead (the waiting run holds nothing), and once two people approve it the run pays from the
/// balance as it then is; a deposit while the payout is in flight waits for the row the run holds,
/// and runs again after it. The payout went out once; no deposit is written over. A second payout,
/// declined, pays nothing.
#[test]
fn approved_payout_keeps_the_deposits_made_while_it_waited() {
    let (host, hits) = counting_server();
    let mut state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-payout-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let schema = "create table accounts (id integer primary key, balance integer not null); insert into accounts values (1, 100); create table payouts (account integer not null, amount integer not null, status integer)";
    let body = serde_json::json!({ "name": "bank", "path": dir.join("bank.db").display().to_string(), "access": "read-write", "schema": schema });
    assert_eq!(route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let text = include_str!("../../holon-author/samples/approved-payout.holon.ts").replace("host:httpbin.org", &format!("host:{host}")).replace(r#""/anything/payouts""#, r#""/slow""#);
    let slug = deploy_text(&state, &text, &host);
    // From here on, people sign in: two of them decide.
    let accounts = Arc::new(crate::accounts::Accounts::memory());
    let ada = accounts.setup(&accounts.setup_code().unwrap(), "ada", "correct horse battery").unwrap().0;
    let bob = accounts.accept(&accounts.invite("bob", "editor", None, "ada").unwrap(), "correct horse battery").unwrap().0;
    state.access = access::Access::new(accounts, vec![], vec![]);
    let state = state;
    let as_person = |token: &str, method: &str, url: &str, body: serde_json::Value| {
        let bytes = if method == "GET" { vec![] } else { serde_json::to_vec(&body).unwrap() };
        let mut headers = req(method, url, bytes.clone()).headers().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        headers.push(("Authorization".into(), format!("Bearer {token}")));
        let r = route(&state, &Request::fake_http(method, url, headers, bytes));
        (r.status_code, body_json(r))
    };
    let waiting = || as_person(&ada, "GET", &format!("/deployments/{slug}/runs"), serde_json::Value::Null).1["runs"].as_array().unwrap().clone();
    let decide = |token: &str, run: &str, approve: bool, reason: Option<&str>| as_person(token, "POST", &format!("/deployments/{slug}/runs/{run}/decide"), serde_json::json!({ "name": "payout", "approve": approve, "reason": reason }));
    let balance = || serde_json::from_str::<serde_json::Value>(&data_call(&state, &slug, "GET", "/accounts/1", "").1).unwrap()["balance"].as_i64().unwrap();

    let (status, asked, _) = data_call(&state, &slug, "POST", "/payout/1", r#"{"amount":30}"#);
    assert_eq!(status, 202, "{asked}");
    assert!(asked.contains("two people's approval"), "{asked}");
    assert_eq!(data_call(&state, &slug, "POST", "/deposit/1", r#"{"amount":50}"#).0, 200, "a deposit while it waits goes ahead");
    let run = waiting()[0]["run_id"].as_str().unwrap().to_string();
    let (code, said) = decide(&ada, &run, true, None);
    assert_eq!((code, &said["outcome"]), (200, &serde_json::json!("waiting")), "{said}");
    assert_eq!(hits_of(&hits, "/slow"), 0, "one approval of two pays nothing");
    let approved = std::thread::scope(|s| {
        let approved = s.spawn(|| decide(&bob, &run, true, Some("the invoice matches")));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(data_call(&state, &slug, "POST", "/deposit/1", r#"{"amount":20}"#).0, 200, "and one while it pays");
        approved.join().unwrap()
    });
    assert_eq!((approved.0, &approved.1["outcome"]), (200, &serde_json::json!("approved")), "{approved:?}");
    assert!(approved.1["run_error"].is_null(), "{approved:?}");
    assert_eq!(balance(), 140, "100, and 50, and 20, less 30");
    assert_eq!(hits_of(&hits, "/slow"), 1, "paid once");
    let entries = as_person(&ada, "GET", &format!("/deployments/{slug}/log"), serde_json::Value::Null).1["entries"].as_array().unwrap().clone();
    // Resumed, it held the account's row from its payout until it committed; the deposit that came
    // meanwhile waited for it, found the balance changed, and ran again on it.
    let resumed = entries.iter().find(|e| e["kind"] == "resume").unwrap_or_else(|| panic!("{entries:#?}"));
    assert!(resumed["claims"].as_array().unwrap().iter().any(|c| c["key"] == "bank/accounts[1]"), "{resumed}");
    assert!(resumed["outputs"].to_string().contains(r#"{"type":"Text","value":"bob"}"#), "who approved it: {resumed}");
    assert!(entries.iter().any(|e| e["route"] == "POST /deposit/:account" && e["retries"] == 1), "{entries:#?}");

    // A second payout, declined: nothing is sent or written.
    assert_eq!(data_call(&state, &slug, "POST", "/payout/1", r#"{"amount":10}"#).0, 202);
    let run = waiting()[0]["run_id"].as_str().unwrap().to_string();
    let (code, said) = decide(&bob, &run, false, Some("not this vendor"));
    assert_eq!((code, &said["outcome"]), (200, &serde_json::json!("declined")), "{said}");
    assert!(said["run_error"].is_null(), "{said}");
    assert_eq!((balance(), hits_of(&hits, "/slow")), (140, 1), "nothing paid");
    assert!(waiting().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Mail (host-capabilities 1.3a) ──

/// A test server with mail: its `ops` read from a dotenv file pointing at an SMTP sink, sending
/// as soon as the run is over.
fn mail_state(sink_port: u16) -> AppState {
    let dir = std::env::temp_dir().join(format!("holon-mail-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    std::fs::write(&env, format!("HOLON_SECRET_OPS_SMTP=smtp://ops:s3cret@127.0.0.1:{sink_port}\n")).unwrap();
    let mut state = claims_state("");
    let mails = mails::Mails::memory().with_sources(secret_sources::Sources::new(Some(env)).unwrap()).with_retry([Duration::from_millis(50); 4], Duration::from_secs(60));
    state.folders = Arc::new(folders::Folders::memory().with_mails(mails));
    state.folders.mails.serve(Arc::clone(&state.deployments));
    state
}

/// A mail's text, its quoted-printable undone.
fn unquoted(data: &str) -> String {
    let joined = data.replace("=\r\n", "");
    let mut out = Vec::new();
    let b = joined.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match (b[i], joined.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'=', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn wait_for_mail(sink: &smtp::sink::Sink, n: usize) -> Vec<smtp::sink::Got> {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let got = sink.got.lock().unwrap().clone();
        if got.len() >= n {
            return got;
        }
        assert!(std::time::Instant::now() < until, "{} mails, not {n}", got.len());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The approval-mail sample (host-capabilities L4 §5.2.2): asking for a payout mails its approval
/// link once the run is over, from the binding's sender, the payout attached; the link's page
/// approves nothing and its button does; approved, the payout is recorded and whoever asked is
/// mailed. One who asked from outside the binding's --to isn't: that mail is given up, and the
/// deployment's timeline says so. The log keeps each message, its attachment by hash.
#[test]
fn approval_mail_mails_the_link_then_the_approval_through_its_binding() {
    let sink = smtp::sink::start("PLAIN LOGIN");
    let state = mail_state(sink.port);
    let text = include_str!("../../holon-author/samples/approval-mail.holon.ts");
    // Not bound: refused at deploy, as a folder is.
    let plan = holon_author::agent::plan(text).unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    let refused = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": preview["needs_approval"] }));
    let said = body_json(refused)["error"].as_str().unwrap_or_default().to_string();
    assert!(said.contains("the mail \"ops\" isn't bound to send on this server"), "{said}");
    // Bound to every address at acme.com.
    let bind = serde_json::json!({ "name": "ops", "smtp": "env:HOLON_SECRET_OPS_SMTP", "from": "Treasury <treasury@acme.com>", "to": "*@acme.com" });
    let bound = route(&state, &req("POST", "/admin/mails", serde_json::to_vec(&bind).unwrap()));
    assert_eq!(bound.status_code, 200);
    let shown = String::from_utf8_lossy(&body_bytes(bound)).into_owned();
    assert!(!shown.contains("s3cret") && shown.contains(&format!("smtp://127.0.0.1:{}", sink.port)), "{shown}");
    let slug = deploy_text(&state, text, "127.0.0.1");

    let (status, body, _) = data_call(&state, &slug, "POST", "/payouts", r#"{"id":"p1","amount":5000,"iban":"DE89370400440532013000","by":"sam@acme.com"}"#);
    assert_eq!(status, 202, "{body}");
    let got = wait_for_mail(&sink, 1);
    assert_eq!((got[0].from.as_str(), got[0].to.clone()), ("treasury@acme.com", vec!["approvals@acme.com".to_string()]));
    assert!(got[0].login.is_some(), "logged in");
    let mail = &got[0].data;
    for part in ["From: Treasury <treasury@acme.com>\r\n", "To: approvals@acme.com\r\n", "Subject: Approve payout p1: 5000 to DE89370400440532013000\r\n", "Content-Disposition: attachment; filename=\"payout.csv\""] {
        assert!(mail.contains(part), "{part} in {mail}");
    }
    // The CSV, base64 as it is attached.
    let csv = "id,amount,iban,by\np1,5000,DE89370400440532013000,sam@acme.com\n";
    assert!(mail.replace("\r\n", "").contains(&smtp_base64(csv.as_bytes())), "{mail}");
    let text_part = unquoted(mail);
    let link = text_part.split("http://127.0.0.1:8186").nth(1).unwrap_or_else(|| panic!("{text_part}"));
    let link: String = link.chars().take_while(|c| !c.is_whitespace()).collect();
    assert!(link.starts_with("/approve/"), "{link}");

    // Opening the link approves nothing: a page with a button.
    let token = link.trim_start_matches("/approve/").to_string();
    let (status, page, _) = data_call(&state, &slug, "GET", &link, "");
    assert_eq!(status, 200);
    assert!(page.contains("<button>Approve</button>"), "{page}");
    assert!(!data_call(&state, &slug, "GET", "/payouts", "").1.contains("p1"), "not approved yet");
    // Its button does: recorded, and sam is mailed.
    assert_eq!(data_call(&state, &slug, "POST", &link, "").0, 200);
    let got = wait_for_mail(&sink, 2);
    assert_eq!(got[1].to, vec!["sam@acme.com".to_string()]);
    assert!(got[1].data.contains("Subject: Payout p1 approved\r\n"), "{}", got[1].data);
    assert!(got[1].data.contains("Message-ID: <") && got[0].data.contains("Message-ID: <"));
    let listed = data_call(&state, &slug, "GET", "/payouts", "").1;
    assert!(listed.contains("\"p1\"") && listed.contains(r#""approved_via":"POST""#), "{listed}");

    // Asked from outside the binding: the approvers are mailed, the asker isn't.
    let (status, _, _) = data_call(&state, &slug, "POST", "/payouts", r#"{"id":"p2","amount":10,"iban":"DE89370400440532013000","by":"eve@evil.example"}"#);
    assert_eq!(status, 202);
    let got = wait_for_mail(&sink, 3);
    let link: String = unquoted(&got[2].data).split("http://127.0.0.1:8186").nth(1).unwrap().chars().take_while(|c| !c.is_whitespace()).collect();
    assert_eq!(data_call(&state, &slug, "POST", &link, "").0, 200);
    let events = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !events().contains("gave up mailing") {
        assert!(std::time::Instant::now() < until, "{}", events());
        std::thread::sleep(Duration::from_millis(20));
    }
    let said = events();
    assert!(said.contains(r#"gave up mailing \"Payout p2 approved\" to eve@evil.example through ops: eve@evil.example isn't an address the binding allows now"#), "{said}");
    assert!(said.contains(r#"mailed \"Approve payout p1: 5000 to DE89370400440532013000\" to approvals@acme.com through ops"#), "{said}");
    assert_eq!(sink.got.lock().unwrap().len(), 3, "eve wasn't mailed");

    // The log keeps the message, its attachment by size and hash.
    let entries = log_entries(&state, &slug);
    let asked = entries.iter().find(|e| e["route"] == "POST /payouts").unwrap();
    let mailed = asked["performed"].as_array().unwrap().iter().find(|p| p["capability"] == r#"MailSend("ops")"#).unwrap_or_else(|| panic!("{asked}"));
    let payload = mailed["payload"].to_string();
    assert!(payload.contains("approvals@acme.com") && payload.contains(&blake3::hash(csv.as_bytes()).to_hex().to_string()) && !payload.contains("p1,5000"), "{payload}");
    // And no run in it holds a live token: the page's route is unrecorded, and the button's, the
    // route that resumes, keeps it redacted.
    let logged = serde_json::to_string(&entries).unwrap();
    assert!(!logged.contains(&token) && !logged.contains("GET /approve"), "{logged}");
}

fn smtp_base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// A remittance mail: from `from`, its verdict written by `verdict_by`, with a CSV of `lines`.
fn remittance(id: &str, from: &str, verdict_by: &str, lines: &str) -> Vec<u8> {
    let domain = from.rsplit('@').next().unwrap();
    format!(
        "Authentication-Results: {verdict_by}; spf=pass smtp.mailfrom={domain}; dkim=pass header.d={domain}; dmarc=pass header.from={domain}\r\n\
         From: Billing <{from}>\r\nTo: remittances@acme.com\r\nSubject: Remittance {id}\r\nDate: Fri, 06 Nov 2026 10:00:00 +0000\r\n\
         Message-ID: <{id}@{domain}>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n\
         --b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nPaid, see attached.\r\n\
         --b1\r\nContent-Type: text/csv; name=\"remit.csv\"\r\nContent-Disposition: attachment; filename=\"remit.csv\"\r\n\r\n\
         invoice,amount\r\n{lines}\r\n--b1--\r\n"
    )
    .into_bytes()
}

// ── Mail received (host-capabilities 1.3b) ──

/// The remittance-mail sample (host-capabilities L4 §5.2.4): refused until its mail is bound with
/// --imap; what the mailbox holds at the first look is seen, not run on; then each new message
/// starts one run and is moved to --done: an authenticated one settles the invoices its CSV pays in
/// full, one whose verdict isn't the mailbox server's changes nothing, and a second copy is moved
/// without a run. The log keeps each message whole, and the runs verify.
#[test]
fn remittance_mail_settles_invoices_from_authenticated_mail_and_moves_each_message() {
    let imap = imap::stand_in::start("remit", "s3cret", true);
    let dir = std::env::temp_dir().join(format!("holon-imap-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    std::fs::write(&env, format!("HOLON_SECRET_REMIT_IMAP=imap://remit:s3cret@127.0.0.1:{}\n", imap.port)).unwrap();
    let mut state = claims_state("");
    let mails = mails::Mails::memory().with_sources(secret_sources::Sources::new(Some(env)).unwrap());
    state.folders = Arc::new(folders::Folders::memory().with_mails(mails));
    state.folders.mails.serve(Arc::clone(&state.deployments));
    let text = include_str!("../../holon-author/samples/remittance-mail.holon.ts");

    // Not bound: refused at deploy.
    let plan = holon_author::agent::plan(text).unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    assert!(preview["needs_approval"].to_string().contains(r#"MailArrival(\"remit\")"#), "{preview}");
    let refused = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": preview["needs_approval"] }));
    let said = body_json(refused)["error"].as_str().unwrap_or_default().to_string();
    assert!(said.contains("the mail \"remit\" isn't bound to receive on this server"), "{said}");
    // Bound, believing mx.acme.com's verdict.
    let bind = serde_json::json!({ "name": "remit", "imap": "env:HOLON_SECRET_REMIT_IMAP", "verdict_from": "mx.acme.com" });
    let bound = route(&state, &req("POST", "/admin/mails", serde_json::to_vec(&bind).unwrap()));
    assert_eq!(bound.status_code, 200);
    let shown = String::from_utf8_lossy(&body_bytes(bound)).into_owned();
    assert!(!shown.contains("s3cret") && shown.contains("\"done\":\"Processed\""), "{shown}");
    imap.folders.lock().unwrap().put("INBOX", &remittance("old", "billing@customer.example", "mx.acme.com", "INV-1,100.00"));
    let slug = deploy_text(&state, text, "");
    let triggers = body_json(route(&state, &req("GET", "/deployments", vec![])))["deployments"][0]["triggers"].clone();
    assert_eq!(triggers, serde_json::json!(["Http :8188", "on mail remit"]));
    for invoice in [r#"{"id":"INV-1","amount":"100.00","customer":"customer.example"}"#, r#"{"id":"INV-2","amount":"50.00","customer":"customer.example"}"#] {
        assert_eq!(data_call(&state, &slug, "POST", "/invoices", invoice).0, 201);
    }
    let mut looks = inbox::Looks::default();
    let look = |looks: &mut inbox::Looks| inbox::run_mail(&state.store, &state.deployments, &state.deployments_path, looks);
    look(&mut looks);
    assert_eq!(log_entries(&state, &slug).iter().filter(|e| e["kind"] == "mail").count(), 0, "what was there at the first look isn't new");
    assert_eq!(imap.folders.lock().unwrap().of("INBOX").len(), 1, "nor moved");
    let events = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
    assert!(events.contains(&format!("listens to the mailbox of remit (imap://127.0.0.1:{} INBOX), what it holds now (1 message) isn't run on", imap.port)), "{events}");

    // INV-1 paid in full, INV-2 short; the same from someone the mailbox's server didn't vouch
    // for; and the first again.
    for m in [
        remittance("r1", "billing@customer.example", "mx.acme.com", "INV-1,100.00\r\nINV-2,40.00"),
        remittance("r2", "billing@customer.example", "evil.example", "INV-2,50.00"),
        remittance("r1", "billing@customer.example", "mx.acme.com", "INV-1,100.00\r\nINV-2,40.00"),
    ] {
        imap.folders.lock().unwrap().put("INBOX", &m);
    }
    look(&mut looks);
    let runs: Vec<serde_json::Value> = log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "mail").collect();
    assert_eq!(runs.len(), 2, "{runs:#?}");
    assert!(runs.iter().all(|r| r["route"] == "on mail remit" && r["error"].is_null()), "{runs:#?}");
    let sourced = runs[0]["sourced"].to_string();
    assert!(sourced.contains(r#"MailArrival(\"remit\")"#) && sourced.contains("INV-2,40.00") && sourced.contains("r1@customer.example"), "logged whole: {sourced}");
    let (_, invoices, _) = data_call(&state, &slug, "GET", "/invoices", "");
    let invoices: serde_json::Value = serde_json::from_str(&invoices).unwrap();
    assert_eq!(invoices["open"], serde_json::json!({ "INV-2": { "amount": "50.00", "customer": "customer.example" } }), "{invoices}");
    assert_eq!(invoices["paid"]["INV-1"]["by"], "billing@customer.example", "{invoices}");
    let (_, remittances, _) = data_call(&state, &slug, "GET", "/remittances", "");
    assert!(remittances.contains(r#""settled":["INV-1"]"#) && remittances.contains(r#""refused":"not authenticated""#), "{remittances}");
    let folders = imap.folders.lock().unwrap();
    assert_eq!((folders.of("INBOX").len(), folders.of("Processed").len()), (1, 3), "each new one moved, the old one left");
    drop(folders);
    let events = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
    assert!(events.contains("moved a second copy of <r1@customer.example> to Processed without a run"), "{events}");

    // Nothing new: nothing runs.
    look(&mut looks);
    assert_eq!(log_entries(&state, &slug).iter().filter(|e| e["kind"] == "mail").count(), 2);

    // Its runs check offline: the message is what the log says came.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));

    // Another deployment can't listen to that mailbox.
    let other = text.replace("flow(\"remittance-mail\", { port: 8188 })", "flow(\"remittance-copy\", { port: 8189 })");
    let plan = holon_author::agent::plan(&other).unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    let refused = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": preview["needs_approval"] }));
    let said = body_json(refused)["error"].as_str().unwrap_or_default().to_string();
    assert!(said.contains("is listened to by remittance-mail already"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Mailboxes that log in with OAuth (host-capabilities 1.3c) ──

/// A request to a support mailbox: from `from`, its verdict written by `verdict_by`.
fn support_request(id: &str, from: &str, verdict_by: &str, subject: &str) -> Vec<u8> {
    let domain = from.rsplit('@').next().unwrap();
    format!(
        "Authentication-Results: {verdict_by}; spf=pass smtp.mailfrom={domain}; dkim=pass header.d={domain}; dmarc=pass header.from={domain}\r\n\
         From: <{from}>\r\nTo: support@acme.com\r\nSubject: {subject}\r\nDate: Mon, 09 Nov 2026 10:00:00 +0000\r\n\
         Message-ID: <{id}@{domain}>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nIt doesn't add up.\r\n"
    )
    .into_bytes()
}

/// The support-inbox sample (host-capabilities L4 §5.2.6): its one mail bound with --oauth, both
/// URLs naming a user and no password, logs in to the mailbox and to the SMTP server with the token
/// the host asked for once and kept. An authenticated request opens a ticket and is answered
/// through the same mail; one that isn't is kept, unanswered. A token the SMTP server refuses is
/// dropped, and the mail goes with a new one. Nothing holds a secret: the binding, the log.
#[test]
fn support_inbox_receives_and_answers_through_one_mail_logged_in_with_oauth() {
    let endpoint = oauth::stand_in::start("app", "s3cret", "t0ken");
    let imap = imap::stand_in::start_with_token("support@acme.com", "t0ken", true);
    let sink = smtp::sink::start("PLAIN LOGIN XOAUTH2");
    let dir = std::env::temp_dir().join(format!("holon-oauth-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    let vars = [
        format!("HOLON_SECRET_SUPPORT_IMAP=imap://support%40acme.com@127.0.0.1:{}", imap.port),
        format!("HOLON_SECRET_SUPPORT_SMTP=smtp://support%40acme.com@127.0.0.1:{}", sink.port),
        format!("HOLON_SECRET_SUPPORT_OAUTH={}", endpoint.url("app", "s3cret", "scope=mail")),
    ];
    std::fs::write(&env, vars.join("\n") + "\n").unwrap();
    let mut state = claims_state("");
    let mails = mails::Mails::memory().with_sources(secret_sources::Sources::new(Some(env)).unwrap()).with_retry([Duration::from_millis(50); 4], Duration::from_secs(60));
    state.folders = Arc::new(folders::Folders::memory().with_mails(mails));
    state.folders.mails.serve(Arc::clone(&state.deployments));

    let bind = serde_json::json!({
        "name": "support", "imap": "env:HOLON_SECRET_SUPPORT_IMAP", "done": "Answered", "verdict_from": "mx.acme.com",
        "smtp": "env:HOLON_SECRET_SUPPORT_SMTP", "from": "Acme support <support@acme.com>", "to": "*", "oauth": "env:HOLON_SECRET_SUPPORT_OAUTH",
    });
    let bound = route(&state, &req("POST", "/admin/mails", serde_json::to_vec(&bind).unwrap()));
    assert_eq!(bound.status_code, 200);
    let shown = String::from_utf8_lossy(&body_bytes(bound)).into_owned();
    assert!(shown.contains(&format!(r#""oauth":{{"address":"http://127.0.0.1:{}/token","source":"env:HOLON_SECRET_SUPPORT_OAUTH"}}"#, endpoint.port)), "{shown}");
    assert!(!shown.contains("s3cret") && !shown.contains("t0ken"), "{shown}");
    assert_eq!(endpoint.asked.lock().unwrap().len(), 1, "one token for both servers");
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/support-inbox.holon.ts"), "");

    let mut looks = inbox::Looks::default();
    inbox::run_mail(&state.store, &state.deployments, &state.deployments_path, &mut looks);
    for m in [support_request("q1", "ann@customer.example", "mx.acme.com", "Invoice INV-7"), support_request("q2", "ann@customer.example", "evil.example", "Pay me")] {
        imap.folders.lock().unwrap().put("INBOX", &m);
    }
    inbox::run_mail(&state.store, &state.deployments, &state.deployments_path, &mut looks);
    let runs: Vec<serde_json::Value> = log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "mail").collect();
    assert_eq!(runs.len(), 2, "{runs:#?}");
    let (_, tickets, _) = data_call(&state, &slug, "GET", "/tickets", "");
    assert!(tickets.contains(r#""T-1":{"date":1794218400,"from":"ann@customer.example","subject":"Invoice INV-7""#), "{tickets}");
    assert!(tickets.contains(r#""unanswered:q2@customer.example""#), "{tickets}");
    let got = wait_for_mail(&sink, 1);
    assert_eq!(got[0].to, vec!["ann@customer.example".to_string()]);
    assert!(got[0].data.contains("Subject: [T-1] Invoice INV-7\r\n") && got[0].data.contains("From: Acme support <support@acme.com>\r\n"), "{}", got[0].data);
    assert_eq!(got[0].login.as_deref(), Some(smtp::xoauth2("support@acme.com", "t0ken").as_str()));
    assert_eq!(imap.folders.lock().unwrap().of("Answered").len(), 2);
    assert_eq!(endpoint.asked.lock().unwrap().len(), 1, "the token kept");

    // The SMTP server stops taking that token: dropped, and the answer goes with a new one.
    *sink.refuse.lock().unwrap() = Some(smtp::xoauth2("support@acme.com", "t0ken").to_string());
    *endpoint.token.lock().unwrap() = "t0ken-2".into();
    imap.folders.lock().unwrap().put("INBOX", &support_request("q3", "bob@customer.example", "mx.acme.com", "Statement"));
    inbox::run_mail(&state.store, &state.deployments, &state.deployments_path, &mut looks);
    let got = wait_for_mail(&sink, 2);
    assert_eq!((got[1].to.clone(), got[1].login.as_deref()), (vec!["bob@customer.example".to_string()], Some(smtp::xoauth2("support@acme.com", "t0ken-2").as_str())));
    assert_eq!(endpoint.asked.lock().unwrap().len(), 2);

    let logged = serde_json::to_string(&log_entries(&state, &slug)).unwrap();
    assert!(!logged.contains("s3cret") && !logged.contains("t0ken"), "{logged}");
    assert!(replays_as_it_verifies(&state, &slug).contains("mail"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── Feeds (host-capabilities 1.6a) ──

/// Wait until `holds`, for 10 seconds at most.
fn eventually(what: &str, mut holds: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !holds() {
        assert!(std::time::Instant::now() < until, "never: {what}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// The price-feed sample (host-capabilities L4 §5.9.2): refused until its feed is bound; bound to
/// a stand-in, each event the feed sends starts one run, in the order they came; the connection
/// cut, it is opened again from the last event run on (Last-Event-ID), and an event the feed sends
/// again is passed over. The state endpoint says where it is, nothing holds the URL's key, and the
/// runs verify.
#[test]
fn price_feed_runs_on_each_event_and_opens_again_from_where_it_was() {
    let feed = sse::stand_in::StandIn::start();
    let dir = std::env::temp_dir().join(format!("holon-feed-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    std::fs::write(&env, format!("HOLON_SECRET_PRICES_URL={}\n", feed.url("/events?key=s3cret"))).unwrap();
    let mut state = claims_state("");
    let feeds = feeds::Feeds::memory().with_sources(secret_sources::Sources::new(Some(env)).unwrap());
    state.folders = Arc::new(folders::Folders::memory().with_feeds(feeds));
    let text = include_str!("../../holon-author/samples/price-feed.holon.ts");

    // Not bound: refused at deploy.
    let plan = holon_author::agent::plan(text).unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    assert!(preview["needs_approval"].to_string().contains(r#"FeedEvent(\"prices\")"#), "{preview}");
    let refused = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": preview["needs_approval"] }));
    let said = body_json(refused)["error"].as_str().unwrap_or_default().to_string();
    assert!(said.contains("the feed \"prices\" isn't bound on this server"), "{said}");
    // Bound: shown without the URL's query.
    let bind = serde_json::json!({ "name": "prices", "url": "env:HOLON_SECRET_PRICES_URL" });
    let bound = route(&state, &req("POST", "/admin/feeds", serde_json::to_vec(&bind).unwrap()));
    assert_eq!(bound.status_code, 200);
    let shown = String::from_utf8_lossy(&body_bytes(bound)).into_owned();
    assert!(!shown.contains("s3cret") && shown.contains(&format!(r#""address":"{}""#, feed.url("/events"))), "{shown}");
    let slug = deploy_text(&state, text, "");
    let triggers = body_json(route(&state, &req("GET", "/deployments", vec![])))["deployments"][0]["triggers"].clone();
    assert_eq!(triggers, serde_json::json!(["Http :8187", "on feed prices"]));

    let mut held = subscriptions::Held::default();
    subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
    // Asked once to bind it, then by the deployment's connection.
    eventually("the feed is listened to", || feed.asked().len() == 2);
    feed.send("1", r#"{"symbol":"ACME","price":10.5}"#);
    feed.send("2", r#"{"symbol":"BOLT","price":3}"#);
    feed.send("3", r#"{"symbol":"ACME","price":11}"#);
    let runs = || log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "feed").collect::<Vec<_>>();
    eventually("three runs", || runs().len() == 3);
    let first = runs();
    assert!(first.iter().all(|r| r["route"] == "on feed prices" && r["error"].is_null()), "{first:#?}");
    let sourced = first[1]["sourced"].to_string();
    assert!(sourced.contains(r#"FeedEvent(\"prices\")"#) && sourced.contains("BOLT"), "logged whole: {sourced}");
    let (_, prices, _) = data_call(&state, &slug, "GET", "/prices", "");
    let prices: serde_json::Value = serde_json::from_str(&prices).unwrap();
    assert_eq!((prices["ACME"]["price"].clone(), prices["BOLT"]["price"].clone()), (serde_json::json!(11), serde_json::json!(3)), "{prices}");
    let live = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/feeds"), vec![])));
    assert_eq!((live["feeds"][0]["status"].as_str(), live["feeds"][0]["position"].as_str()), (Some("connected"), Some("3")), "{live}");
    assert_eq!(live["feeds"][0]["label"], "on feed prices", "{live}");

    // Cut: opened again from 3, which the feed sends again (passed over), then one past 1 MiB
    // (passed over, the connection read on), then 4.
    feed.cut();
    eventually("the feed is opened again", || feed.asked().len() == 3);
    assert!(feed.asked()[2].contains("Last-Event-ID: 3"), "{:?}", feed.asked());
    feed.send_lines("3", "id: 3\ndata: {\"symbol\":\"ACME\",\"price\":99}\n\n");
    feed.send("big", &format!(r#"{{"symbol":"HUGE","note":"{}"}}"#, "x".repeat(sse::MAX_EVENT)));
    feed.send("4", r#"{"symbol":"CORE","price":7}"#);
    eventually("a fourth run", || runs().len() == 4);
    assert_eq!(feed.asked().len(), 3, "read on, not opened again");
    let events = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
    assert!(events.contains(&format!("listens to the feed prices ({})", feed.url("/events"))), "{events}");
    assert!(events.contains("passed over an event of prices it ran already (3)"), "{events}");
    assert!(events.contains("passed over an event of prices past 1 MiB"), "{events}");
    let (_, prices, _) = data_call(&state, &slug, "GET", "/prices", "");
    let prices: serde_json::Value = serde_json::from_str(&prices).unwrap();
    assert_eq!((prices["ACME"]["price"].clone(), prices["CORE"]["price"].clone()), (serde_json::json!(11), serde_json::json!(7)), "{prices}");

    // Its runs check offline: each event is what the log says came.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    let logged = serde_json::to_string(&log_entries(&state, &slug)).unwrap();
    assert!(!logged.contains("s3cret"), "{logged}");

    // Stopped: its connection is closed, and nothing more runs.
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![])).status_code, 200);
    subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
    eventually("its connection is closed", || feed.open() == 0);
    feed.send("5", r#"{"symbol":"DUST","price":1}"#);
    thread::sleep(Duration::from_millis(300));
    assert_eq!(runs().len(), 4);
    let live = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/feeds"), vec![])));
    assert_eq!(live["feeds"][0]["status"], "closed", "{live}");
    assert!(replays_as_it_verifies(&state, &slug).contains("feed"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An exchange on loopback for the trading-bot sample, as Coinbase Exchange's REST API answers:
/// each request signed (`cb-access-sign`, an HMAC-SHA256 of the time, method, path and body with
/// the secret) or refused 401; `POST /orders` places an order once, a client id it has seen
/// refused 409, and waits while it is held; `GET /orders` lists the orders placed.
struct Exchange {
    host: String,
    asked: Arc<Mutex<Vec<serde_json::Value>>>,
    placed: Arc<Mutex<Vec<serde_json::Value>>>,
    held: Arc<(Mutex<bool>, crate::sync::Condvar)>,
}

impl Exchange {
    const KEY: &'static str = "ex-key-e2e";
    const PASSPHRASE: &'static str = "ex-pass-e2e";
    /// Its secret, base64 as an exchange hands it out.
    const SECRET: &'static str = "c2VjcmV0LWJ5dGVzLWZvci10aGUtZXhjaGFuZ2U=";

    fn start() -> Exchange {
        let asked: Arc<Mutex<Vec<serde_json::Value>>> = Default::default();
        let placed: Arc<Mutex<Vec<serde_json::Value>>> = Default::default();
        let held = Arc::new((Mutex::new(false), crate::sync::Condvar::new()));
        let (a, p, h) = (Arc::clone(&asked), Arc::clone(&placed), Arc::clone(&held));
        let server = rouille::Server::new("127.0.0.1:0", move |request| {
            use std::io::Read;
            let mut body = String::new();
            if let Some(mut data) = request.data() {
                let _ = data.read_to_string(&mut body);
            }
            let header = |name: &str| request.header(name).unwrap_or_default().to_string();
            let prehash = format!("{}{}{}{body}", header("cb-access-timestamp"), request.method(), request.raw_url());
            let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &holon_core::text_bytes(Exchange::SECRET, "base64").unwrap());
            let signed = holon_core::bytes_text(ring::hmac::sign(&key, prehash.as_bytes()).as_ref(), "base64").unwrap();
            if header("cb-access-sign") != signed || header("cb-access-key") != Exchange::KEY || header("cb-access-passphrase") != Exchange::PASSPHRASE {
                return Response::json(&serde_json::json!({ "message": "invalid signature" })).with_status_code(401);
            }
            if request.method() == "GET" {
                return Response::json(&*p.lock().unwrap());
            }
            let order: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            a.lock().unwrap().push(order.clone());
            let (lock, changed) = &*h;
            drop(changed.wait_while(lock.lock().unwrap(), |held| *held).unwrap());
            let mut placed = p.lock().unwrap();
            if placed.iter().any(|o| o["client_oid"] == order["client_oid"]) {
                return Response::json(&serde_json::json!({ "message": "duplicate client_oid" })).with_status_code(409);
            }
            let mut order = order;
            order["id"] = serde_json::json!(format!("ex-{}", placed.len() + 1));
            placed.push(order.clone());
            Response::json(&order)
        })
        .unwrap();
        let host = server.server_addr().to_string();
        thread::spawn(move || server.run());
        Exchange { host, asked, placed, held }
    }

    fn hold(&self, held: bool) {
        *self.held.0.lock().unwrap() = held;
        self.held.1.notify_all();
    }
}

/// The trading-bot sample (host-capabilities L4 §5.9.4) against a WebSocket ticker and an exchange
/// on loopback: the binding subscribes, runs only the ticker's messages and keeps only each
/// product's newest price while a run is slow; a price under a level places its order once, signed;
/// the price coming again places nothing, and the action run again by hand is refused by the
/// exchange; an order over the limit waits for approval, then is placed by a run of its own;
/// reconcile reads the exchange's orders back. The runs verify, and no key is in the log.
#[test]
fn trading_bot_keeps_the_newest_price_and_places_each_order_once() {
    let exchange = Exchange::start();
    let ticker = ws::stand_in::StandIn::start();
    let dir = std::env::temp_dir().join(format!("holon-bot-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    std::fs::write(&env, format!("HOLON_SECRET_TICKER_URL={}\n", ticker.url("/ws?key=s3cret"))).unwrap();
    let mut state = claims_state("");
    let feeds = feeds::Feeds::memory().with_sources(secret_sources::Sources::new(Some(env)).unwrap());
    state.folders = Arc::new(folders::Folders::memory().with_feeds(feeds));
    let text = include_str!("../../holon-author/samples/trading-bot.holon.ts").replace("\"host:api.exchange.coinbase.com\"", &format!("\"host:{}\"", exchange.host));
    let subscribe = r#"{"type":"subscribe","channels":["ticker"],"product_ids":["BTC-USD","ETH-USD"]}"#;
    let bind = serde_json::json!({
        "name": "ticker", "url": "env:HOLON_SECRET_TICKER_URL", "subscribe": [subscribe], "only": ["type=ticker"],
        "id": "product_id,sequence", "on_full": "latest", "key": "product_id",
    });
    let bound = route(&state, &req("POST", "/admin/feeds", serde_json::to_vec(&bind).unwrap()));
    let status = bound.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(bound)));
    let slug = deploy_text(&state, &text, &exchange.host);
    for (name, value) in [("exchange_key", Exchange::KEY), ("exchange_secret", Exchange::SECRET), ("exchange_passphrase", Exchange::PASSPHRASE)] {
        let set = serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap();
        assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/secrets/{name}"), set)).status_code, 200);
    }
    let call = |method: &str, path: &str, body: &str| {
        let (status, body, _) = data_call(&state, &slug, method, path, body);
        assert_eq!(status, 200, "{method} {path}: {body}");
        serde_json::from_str::<serde_json::Value>(&body).unwrap_or(serde_json::Value::String(body))
    };
    let level = call("PUT", "/levels/BTC-USD", r#"{"price":"100","size":"2"}"#);
    let id = level["id"].as_str().unwrap().to_string();
    assert_eq!(id.len(), 36, "a uuid: {id}");

    let mut held = subscriptions::Held::default();
    subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
    // Asked once to bind it, then by the deployment's connection, which subscribes.
    eventually("the feed is listened to", || ticker.asked().len() == 2 && ticker.heard() == [subscribe]);
    let price = |product: &str, price: &str, sequence: u32| ticker.text(&format!(r#"{{"type":"ticker","product_id":"{product}","price":"{price}","sequence":{sequence}}}"#));
    let live = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/feeds"), vec![])))["feeds"][0].clone();
    // Under the level, with the exchange holding its answer: the run waits on the order while
    // prices come, and each product's newest is kept.
    exchange.hold(true);
    ticker.text(r#"{"type":"subscriptions","channels":[{"name":"ticker"}]}"#);
    price("BTC-USD", "99", 1);
    eventually("the order is sent", || exchange.asked.lock().unwrap().len() == 1);
    price("BTC-USD", "98", 2);
    price("ETH-USD", "50", 1);
    price("BTC-USD", "97", 3);
    price("BTC-USD", "96", 4);
    eventually("two prices replaced", || live()["replaced"] == 2);
    let l = live();
    assert_eq!((l["waiting"].as_u64(), l["passed_over"].as_u64()), (Some(2), Some(1)), "{l}");
    exchange.hold(false);
    let runs = || log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "feed").collect::<Vec<_>>();
    eventually("three runs", || runs().len() == 3);
    let prices = call("GET", "/prices", "");
    assert_eq!((prices["BTC-USD"]["price"].as_str(), prices["BTC-USD"]["replaced"].as_u64()), (Some("96"), Some(2)), "{prices}");
    assert_eq!(prices["ETH-USD"]["price"], "50", "{prices}");
    let placed = exchange.placed.lock().unwrap().clone();
    assert_eq!(placed.len(), 1, "{placed:?}");
    assert_eq!((placed[0]["client_oid"].as_str(), placed[0]["price"].as_str(), placed[0]["size"].as_str()), (Some(id.as_str()), Some("100"), Some("2")));
    let orders = call("GET", "/orders", "");
    assert_eq!((orders["placed"][&id]["exchange"].as_str(), orders["placed"][&id]["approved_by"].as_str()), (Some("ex-1"), Some("under the limit")), "{orders}");
    // BTC's newest took the place of the one that waited, ahead of ETH's.
    assert_eq!(live()["position"], "ETH-USD/1");

    // The price under the level again: placed already, nothing sent.
    price("BTC-USD", "95", 5);
    eventually("a fourth run", || runs().len() == 4);
    assert_eq!(exchange.asked.lock().unwrap().len(), 1);
    // The order run again by hand, as a crash would run its price again: the exchange refuses
    // the client id it has seen.
    let again = body_json(run_action_named(&state, &slug, "place", serde_json::json!({ "product": "BTC-USD", "price": "100", "size": "2", "id": id })));
    assert_eq!(again["answer"]["status"], 409, "{again}");
    assert_eq!((exchange.asked.lock().unwrap().len(), exchange.placed.lock().unwrap().len()), (2, 1));

    // Worth more than 1000: it waits for approval, then a run of its own places it.
    let big = call("PUT", "/levels/ETH-USD", r#"{"price":"2000","size":"1"}"#);
    price("ETH-USD", "1999", 2);
    eventually("the order waits for approval", || call("GET", "/orders", "")["waiting"][big["id"].as_str().unwrap()]["approve"].is_string());
    let link = call("GET", "/orders", "")["waiting"][big["id"].as_str().unwrap()]["approve"].as_str().unwrap().to_string();
    assert_eq!(exchange.asked.lock().unwrap().len(), 2, "nothing sent before the approval");
    let (status, said, _) = data_call(&state, &slug, "POST", &link, r#"{"by":"ops"}"#);
    assert_eq!(status, 200, "{said}");
    eventually("the large order is placed", || exchange.placed.lock().unwrap().len() == 2);
    let orders = call("GET", "/orders", "");
    assert_eq!(orders["placed"][big["id"].as_str().unwrap()]["approved_by"], "ops", "{orders}");
    assert_eq!(orders["waiting"], serde_json::json!({}), "{orders}");

    // One nobody approves: when its hour runs out it is placed by nobody (it once was, as approved
    // by "someone": what a wait gives when it runs out has no body to read who from), and it no
    // longer waits.
    let unasked = call("PUT", "/levels/SOL-USD", r#"{"price":"3000","size":"1"}"#);
    price("SOL-USD", "2999", 1);
    eventually("the second large order waits", || call("GET", "/orders", "")["waiting"][unasked["id"].as_str().unwrap()]["approve"].is_string());
    let asked_before = exchange.asked.lock().unwrap().len();
    durable::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 4000);
    let orders = call("GET", "/orders", "");
    assert_eq!(orders["waiting"], serde_json::json!({}), "it no longer waits: {orders}");
    assert!(orders["placed"][unasked["id"].as_str().unwrap()].is_null(), "nobody placed it: {orders}");
    assert_eq!((exchange.asked.lock().unwrap().len(), exchange.placed.lock().unwrap().len()), (asked_before, 2), "nothing was sent to the exchange");
    // Read back from the exchange.
    let open = body_json(run_action_named(&state, &slug, "reconcile", serde_json::json!({})));
    assert_eq!(open["answer"].as_array().map(Vec::len), Some(2), "{open}");

    // Its runs check offline, and no key is in its log.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    let logged = serde_json::to_string(&log_entries(&state, &slug)).unwrap();
    for kept in ["s3cret", Exchange::KEY, Exchange::SECRET, Exchange::PASSPHRASE] {
        assert!(!logged.contains(kept), "{kept} is in the log");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The bluesky-mentions sample (host-capabilities L4 §5.9.6): posts from a feed shaped as
/// Jetstream is, numbered, which replays from a cursor; a connection lost is opened again from the
/// last post read, the posts sent meanwhile run, and the one at the cursor, sent again, is passed
/// over.
#[test]
fn bluesky_mentions_opens_the_feed_again_from_its_cursor_and_runs_each_post_once() {
    let jetstream = ws::stand_in::StandIn::start();
    let dir = std::env::temp_dir().join(format!("holon-posts-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    std::fs::write(&env, format!("HOLON_SECRET_POSTS_URL={}\n", jetstream.url("/subscribe?collections=app.bsky.feed.post&key=s3cret"))).unwrap();
    let mut state = claims_state("");
    let feeds = feeds::Feeds::memory().with_sources(secret_sources::Sources::new(Some(env)).unwrap());
    state.folders = Arc::new(folders::Folders::memory().with_feeds(feeds));
    let bind = serde_json::json!({ "name": "posts", "url": "env:HOLON_SECRET_POSTS_URL", "id": "payload.seq", "resume": ["query:cursor"], "only": ["payload.operation=create"] });
    let bound = route(&state, &req("POST", "/admin/feeds", serde_json::to_vec(&bind).unwrap()));
    let status = bound.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(bound)));
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/bluesky-mentions.holon.ts"), "");
    let call = |method: &str, path: &str, body: &str| {
        let (status, body, _) = data_call(&state, &slug, method, path, body);
        assert_eq!(status, 200, "{method} {path}: {body}");
        serde_json::from_str::<serde_json::Value>(&body).unwrap_or(serde_json::Value::String(body))
    };
    assert_eq!(call("PUT", "/watch", r#"{"word":"Holon"}"#), serde_json::json!({ "word": "holon" }));

    let mut held = subscriptions::Held::default();
    subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
    // A new listener asks from now: no cursor.
    eventually("the feed is listened to", || jetstream.asked().len() == 2);
    assert!(jetstream.asked()[1].starts_with("/subscribe?collections=app.bsky.feed.post&key=s3cret\n"), "{:?}", jetstream.asked());
    let post = |seq: u32, operation: &str, text: &str| {
        let payload = serde_json::json!({ "$type": "network.bsky.jetstream.subscribeEvents#commit", "did": "did:plc:ada", "seq": seq, "time": format!("2026-11-09T08:00:0{seq}Z"), "operation": operation, "collection": "app.bsky.feed.post", "rkey": format!("3k{seq}"), "record": { "text": text } });
        jetstream.text(&serde_json::json!({ "$type": "message", "payload": payload }).to_string());
    };
    post(1, "create", "hello Holon");
    post(2, "create", "nothing to see");
    post(3, "delete", "holon, deleted");
    post(4, "create", "holon again");
    let runs = || log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "feed").collect::<Vec<_>>();
    eventually("three runs", || runs().len() == 3);
    let live = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/feeds"), vec![])))["feeds"][0].clone();
    eventually("at 4", || live()["position"] == "4");
    let mentions = call("GET", "/mentions", "");
    assert_eq!(mentions.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()), Some(vec!["at://did:plc:ada/app.bsky.feed.post/3k1".to_string(), "at://did:plc:ada/app.bsky.feed.post/3k4".into()]), "{mentions}");
    assert_eq!((mentions["at://did:plc:ada/app.bsky.feed.post/3k1"]["text"].as_str(), mentions["at://did:plc:ada/app.bsky.feed.post/3k4"]["seq"].as_str()), (Some("hello Holon"), Some("4")));

    // Cut, with two posts sent while it is down: opened again from 4, the feed sends 4 again
    // (passed over), then the two.
    jetstream.cut();
    post(5, "create", "a HOLON while away");
    post(6, "create", "another while away");
    eventually("the feed is opened again", || jetstream.asked().len() == 3);
    assert!(jetstream.asked()[2].starts_with("/subscribe?collections=app.bsky.feed.post&key=s3cret&cursor=4\n"), "{:?}", jetstream.asked());
    eventually("five runs", || runs().len() == 5);
    eventually("at 6", || live()["position"] == "6");
    let ran: Vec<u32> = runs().iter().map(|r| (1..=6).find(|n| r["sourced"].to_string().contains(&format!("3k{n}"))).unwrap_or(0)).collect();
    assert_eq!(ran, [1, 2, 4, 5, 6], "each post run once");
    let events = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
    assert!(events.contains("passed over an event of posts it ran already (4)"), "{events}");
    let mentions = call("GET", "/mentions", "");
    assert_eq!(mentions.as_object().map(|m| m.len()), Some(3), "{mentions}");
    assert_eq!(mentions["at://did:plc:ada/app.bsky.feed.post/3k5"]["text"], "a HOLON while away");

    // Its runs check offline, and the URL's key is in no log.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    assert!(!serde_json::to_string(&log_entries(&state, &slug)).unwrap().contains("s3cret"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for an exchange's public trades: `GET /products/<product>/trades?after=<id>&limit=<n>`
/// answers the trades before the id, the newest first, `n` of them (1,000 at most), as Coinbase
/// Exchange pages them. Every trade id has a trade, its size the id in thousandths.
struct Trades {
    host: String,
    asked: Arc<Mutex<Vec<String>>>,
}

impl Trades {
    fn start() -> Trades {
        let asked: Arc<Mutex<Vec<String>>> = Default::default();
        let a = Arc::clone(&asked);
        let server = rouille::Server::new("127.0.0.1:0", move |request| {
            a.lock().unwrap().push(request.raw_url().to_string());
            let product = request.url().strip_prefix("/products/").and_then(|r| r.strip_suffix("/trades")).unwrap_or_default().to_string();
            let after: u64 = request.get_param("after").and_then(|a| a.parse().ok()).unwrap_or(0);
            let limit: u64 = request.get_param("limit").and_then(|l| l.parse().ok()).unwrap_or(100).min(1000);
            let page: Vec<serde_json::Value> = (after.saturating_sub(limit).max(1)..after).rev().map(|id| serde_json::json!({ "trade_id": id, "product_id": product, "size": Trades::size(id), "price": "100", "side": "buy" })).collect();
            Response::json(&page)
        })
        .unwrap();
        let host = server.server_addr().to_string();
        thread::spawn(move || server.run());
        Trades { host, asked }
    }

    /// The size of trade `id`, or of trades whose ids add up to it: its thousandths, so a volume
    /// says exactly which trades were added.
    fn size(id: u64) -> String {
        format!("{}.{:03}", id / 1000, id % 1000)
    }
}

/// The trade-tape sample (host-capabilities L4 §5.9.6): trades from a feed shaped as Coinbase
/// Exchange's matches channel, each product's numbered by trade_id, and its REST API on loopback.
/// The trades the feed skipped while the host read nothing are the next one's e.gap, fetched and
/// added: on one connection, across a reconnect, and across a restart (the binding changed: a
/// connection opened anew from the numbers kept with the position); a gap past a page adds a page
/// and keeps how many it couldn't. Each product's volume is the exact sum of its trades.
#[test]
fn trade_tape_fetches_the_trades_its_feed_skipped_and_keeps_its_volume_exact() {
    let exchange = Trades::start();
    let feed = ws::stand_in::StandIn::start();
    let dir = std::env::temp_dir().join(format!("holon-trades-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    std::fs::write(&env, format!("HOLON_SECRET_TRADES_URL={}\n", feed.url("/ws?key=s3cret"))).unwrap();
    let mut state = claims_state("");
    let feeds = feeds::Feeds::memory().with_sources(secret_sources::Sources::new(Some(env)).unwrap());
    state.folders = Arc::new(folders::Folders::memory().with_feeds(feeds));
    let text = include_str!("../../holon-author/samples/trade-tape.holon.ts").replace("\"host:api.exchange.coinbase.com\"", &format!("\"host:{}\"", exchange.host));
    let subscribe = r#"{"type":"subscribe","channels":["matches"],"product_ids":["BTC-USD","ETH-USD"]}"#;
    let bind = |buffer: u64| {
        let bind = serde_json::json!({
            "name": "trades", "url": "env:HOLON_SECRET_TRADES_URL", "subscribe": [subscribe], "only": ["type=match", "type=last_match"],
            "id": "product_id,trade_id", "sequence": "trade_id", "key": "product_id", "buffer": buffer,
        });
        let bound = route(&state, &req("POST", "/admin/feeds", serde_json::to_vec(&bind).unwrap()));
        let status = bound.status_code;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(bound)));
    };
    bind(1000);
    let slug = deploy_text(&state, &text, &exchange.host);
    let volumes = || {
        let (status, body, _) = data_call(&state, &slug, "GET", "/volumes", "");
        assert_eq!(status, 200, "{body}");
        serde_json::from_str::<serde_json::Value>(&body).unwrap()
    };
    // A product's volume, its trades, those fetched and those not: its trade ids, each once.
    let of = |v: &serde_json::Value, ids: &[std::ops::RangeInclusive<u64>], fetched: u64, unfetched: u64| {
        let count = ids.iter().map(|r| r.clone().count() as u64).sum::<u64>();
        let expected = serde_json::json!({ "volume": Trades::size(ids.iter().flat_map(|r| r.clone()).sum()), "trades": count, "fetched": fetched, "unfetched": unfetched });
        let got = serde_json::json!({ "volume": v["volume"], "trades": v["trades"], "fetched": v["fetched"], "unfetched": v["unfetched"] });
        assert_eq!(got, expected, "{v}");
    };
    let trade = |kind: &str, product: &str, id: u64| {
        feed.text(&serde_json::json!({ "type": kind, "product_id": product, "trade_id": id, "size": Trades::size(id), "price": "100", "side": "buy" }).to_string());
    };
    let runs = || log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "feed").count();
    let live = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/feeds"), vec![])))["feeds"][0].clone();
    let asked = || exchange.asked.lock().unwrap().clone();

    let mut held = subscriptions::Held::default();
    subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
    eventually("the feed is listened to", || feed.asked().len() == 2 && feed.heard() == [subscribe]);
    // On one connection: 102 and 103 never came.
    feed.text(r#"{"type":"subscriptions","channels":[{"name":"matches","product_ids":["BTC-USD","ETH-USD"]}]}"#);
    trade("last_match", "BTC-USD", 100);
    trade("match", "BTC-USD", 101);
    trade("match", "ETH-USD", 7);
    trade("match", "BTC-USD", 104);
    eventually("four runs", || runs() == 4);
    let v = volumes();
    of(&v["BTC-USD"], &[100..=104], 2, 0);
    of(&v["ETH-USD"], &[7..=7], 0, 0);
    assert_eq!(asked(), ["/products/BTC-USD/trades?after=104&limit=2"]);
    eventually("at BTC-USD/104", || live()["position"] == "BTC-USD/104");
    let l = live();
    assert_eq!((l["gaps"].as_u64(), l["passed_over"].as_u64()), (Some(2), Some(1)), "{l}");

    // Cut, with three trades while it reads nothing: opened again, its subscription sent again, the
    // exchange sends each product's last trade, then the next.
    feed.cut();
    trade("match", "BTC-USD", 105);
    trade("match", "BTC-USD", 106);
    trade("match", "ETH-USD", 8);
    eventually("the feed is opened again", || feed.asked().len() == 3);
    trade("last_match", "BTC-USD", 106);
    trade("last_match", "ETH-USD", 8);
    trade("match", "ETH-USD", 10);
    eventually("seven runs", || runs() == 7);
    let v = volumes();
    of(&v["BTC-USD"], &[100..=106], 3, 0);
    of(&v["ETH-USD"], &[7..=10], 1, 0);
    assert_eq!(asked()[1..], ["/products/BTC-USD/trades?after=106&limit=1", "/products/ETH-USD/trades?after=10&limit=1"]);

    // A restart: two trades while nothing reads, then the binding changed, so its connection is
    // opened anew from the numbers kept with the position. Then a gap past a page: the 1,000
    // before it fetched, the 91 before those not.
    feed.cut();
    trade("match", "BTC-USD", 107);
    trade("match", "BTC-USD", 108);
    bind(500);
    let before = feed.asked().len();
    subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
    eventually("the feed is opened anew", || feed.asked().len() > before);
    trade("last_match", "BTC-USD", 108);
    trade("match", "BTC-USD", 1200);
    trade("match", "ETH-USD", 11);
    eventually("ten runs", || runs() == 10);
    let v = volumes();
    of(&v["BTC-USD"], &[100..=108, 200..=1200], 1004, 91);
    of(&v["ETH-USD"], &[7..=11], 1, 0);
    assert_eq!(asked()[3..], ["/products/BTC-USD/trades?after=108&limit=1", "/products/BTC-USD/trades?after=1200&limit=1000"]);
    assert_eq!(live()["gaps"].as_u64(), Some(1 + 1091), "since this connection opened");

    // Its runs check offline, and the URL's key is in no log.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
    assert!(!serde_json::to_string(&log_entries(&state, &slug)).unwrap().contains("s3cret"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The site sample (host-capabilities L4 §5.11.14), its folder bound to a build: a deep link and
/// the root answer index.html, an asset its bytes and type, a file at the root itself, a name no
/// file can have the page; each with its hash as its etag, a matching `If-None-Match` a 304; a
/// missing asset a 404, the API beside it. A file changed since the run read its handle is a 503
/// to retry; one past `max_served_bytes`, or a deployment not granted `FileServe`, a 500. Through
/// the data plane, a big file is streamed whole, and cut, said on the timeline, when it changes
/// while it is sent. Each run is recorded with the file it answered with, and the runs verify.
#[test]
fn site_serves_a_built_app_from_its_folder() {
    use std::io::{Read, Write};
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let dir = std::env::temp_dir().join(format!("holon-site-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    let index = "<!doctype html><title>App</title><script type=module src=/assets/app.4f2a.js></script>";
    std::fs::write(dir.join("index.html"), index).unwrap();
    std::fs::write(dir.join("assets/app.4f2a.js"), "console.log('app')\n").unwrap();
    let png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00, 0xfe];
    std::fs::write(dir.join("logo.png"), png).unwrap();
    // Bigger than a file read whole (1 MiB): streamed by the data plane. Text, so it reads back
    // as sent.
    let big: String = (0..1_000_000).map(|i| format!("{i:07}\n")).collect();
    std::fs::write(dir.join("assets/big.txt"), &big).unwrap();
    let site = serde_json::json!({ "name": "site", "path": dir.display().to_string(), "access": "read" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&site).unwrap())).status_code, 200);
    let port = free_port();
    let text = include_str!("../../holon-author/samples/site.holon.ts").replace("port: 8196", &format!("port: {port}"));
    let slug = deploy_plain_text(&state, &text);
    let granted = state.deployments.lock().unwrap()[&slug].grant.items.clone();
    assert!(granted.contains("FileServe(\"site\")") && granted.contains("FileRead(\"site\")"), "{granted:?}");

    let ask = |url: &str, headers: &[(&str, &str)]| {
        let headers = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &data::DataRequest { method: "GET".into(), url: url.into(), headers, body: vec![], by: None })
    };
    let header = |r: &data::DataResponse, k: &str| r.headers.iter().find(|(h, _)| h == k).map(|(_, v)| v.clone()).unwrap_or_default();
    let etag = |bytes: &[u8]| format!("\"{}\"", blake3::hash(bytes).to_hex());
    for url in ["/docs/setup", "/", "/docs/a%20guide/part%202?x=1", "/Talk:Page", "/assets"] {
        let page = ask(url, &[]);
        assert_eq!((page.status, page.body.as_slice()), (200, index.as_bytes()), "{url}");
        assert_eq!(header(&page, "content-type"), "text/html; charset=utf-8");
        assert_eq!((header(&page, "etag"), header(&page, "cache-control")), (etag(index.as_bytes()), "no-cache".to_string()), "{url}");
    }
    let script = ask("/assets/app.4f2a.js", &[]);
    assert_eq!((script.status, script.body.as_slice()), (200, &b"console.log('app')\n"[..]));
    assert_eq!(header(&script, "content-type"), "text/javascript; charset=utf-8");
    assert_eq!(header(&script, "cache-control"), "public, max-age=31536000, immutable", "the flow's own");
    let logo = ask("/logo.png", &[]);
    assert_eq!((logo.status, logo.body.as_slice(), header(&logo, "content-type")), (200, png, "image/png".to_string()), "its exact bytes");
    // A cached copy still the file's: a 304, no body; any other etag gets the file.
    for tag in [etag(index.as_bytes()), "*".to_string(), format!("\"x\", W/{}", etag(index.as_bytes()))] {
        let cached = ask("/docs/setup", &[("If-None-Match", &tag)]);
        assert_eq!((cached.status, cached.body.len(), header(&cached, "etag")), (304, 0, etag(index.as_bytes())), "{tag}");
    }
    assert_eq!(ask("/docs/setup", &[("If-None-Match", "\"stale\"")]).status, 200);
    let missing = ask("/assets/nope.js", &[]);
    assert_eq!((missing.status, String::from_utf8_lossy(&missing.body).into_owned()), (404, r#"{"error":"not found"}"#.to_string()), "never the page");
    assert_eq!(ask("/api/hello", &[]).body, br#"{"hello":"world"}"#);
    assert_eq!(ask("/api/nope/more", &[]).status, 404);

    // Changed between the run's handle and the send: a 503 to retry, its entry saying why.
    let (permit, folders) = {
        let table = state.deployments.lock().unwrap();
        (Arc::clone(&table[&slug].permit), Arc::clone(&table[&slug].folders))
    };
    let meta = std::fs::metadata(dir.join("index.html")).unwrap();
    let read = data::Served {
        folder: "site".into(),
        name: "index.html".into(),
        size: meta.len(),
        hash: blake3::hash(index.as_bytes()).to_hex().to_string(),
        modified: folders::modified(&meta),
        content_type: "text/html; charset=utf-8".into(),
        opened: None,
    };
    let answered = |file: data::Served| data::DataResponse { status: 200, headers: vec![], body: vec![], subscribe: None, file: Some(file) };
    let (sent, error) = listeners::serve_file(answered(read.clone()), &permit, Some(&folders), &[]);
    assert_eq!((sent.status, sent.body.as_slice(), error), (200, index.as_bytes(), None));
    let (changed, error) = listeners::serve_file(answered(data::Served { hash: "0".repeat(64), ..read.clone() }), &permit, Some(&folders), &[]);
    assert_eq!((changed.status, header(&changed, "retry-after")), (503, "1".to_string()));
    assert_eq!(error.as_deref(), Some("changed: site/index.html changed since the run read it"));
    let (gone, _) = listeners::serve_file(answered(data::Served { name: "gone.html".into(), ..read.clone() }), &permit, Some(&folders), &[]);
    assert_eq!(gone.status, 503, "gone since");

    // The record says what was sent: the file, by its hash.
    let entries = log_entries(&state, &slug);
    let asset = entries.iter().find(|e| e["url"] == "/assets/app.4f2a.js").expect("the script's run");
    let hash = blake3::hash(b"console.log('app')\n").to_hex().to_string();
    assert_eq!(asset["response_body"], format!("file site/assets/app.4f2a.js (19 bytes, blake3 {}…)", &hash[..8]));
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));

    // Through the data plane: a big file streamed whole, its length said.
    sync_listeners(&state);
    let got = raw_http(port, b"GET /assets/big.txt HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    let (head, body) = got.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200") && head.to_ascii_lowercase().contains(&format!("content-length: {}", big.len())), "{head}");
    assert!(body == big, "the whole file, as it is ({} of {} bytes)", body.len(), big.len());
    // And cut when it changes while it is sent: the caller gets less than its length, and the
    // timeline says so.
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(b"GET /assets/big.txt HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").unwrap();
    let mut first = [0u8; 1024];
    let n = stream.read(&mut first).unwrap();
    assert!(String::from_utf8_lossy(&first[..n]).starts_with("HTTP/1.1 200"));
    let mut file = std::fs::OpenOptions::new().write(true).open(dir.join("assets/big.txt")).unwrap();
    std::io::Seek::seek(&mut file, std::io::SeekFrom::End(-8)).unwrap();
    file.write_all(b"changed\n").unwrap();
    drop(file);
    let mut rest = Vec::new();
    let _ = stream.read_to_end(&mut rest);
    assert!(n + rest.len() < big.len(), "cut before its end: {} bytes", n + rest.len());
    let events = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
    assert!(events.contains("site/assets/big.txt changed while it was sent: the connection was cut"), "{events}");

    // Past its limit, or not granted: a 500 saying so.
    {
        let mut table = state.deployments.lock().unwrap();
        let dep = table.get_mut(&slug).unwrap();
        let mut limited = Permit::new(dep.unit, &dep.grant, &state.policy());
        limited.files.max_served_bytes = 10;
        dep.permit = Arc::new(limited);
    }
    let past = ask("/docs/setup", &[]);
    assert!(past.status == 500 && String::from_utf8_lossy(&past.body).contains("more than the 10 a route answers with (max_served_bytes)"), "{past:?}");
    {
        let mut table = state.deployments.lock().unwrap();
        let dep = table.get_mut(&slug).unwrap();
        let mut narrowed = dep.grant.clone();
        narrowed.items.remove("FileServe(\"site\")");
        dep.permit = Arc::new(Permit::new(dep.unit, &narrowed, &state.policy()));
    }
    let refused = ask("/docs/setup", &[]);
    assert!(refused.status == 500 && String::from_utf8_lossy(&refused.body).contains(r#"may not answer its callers with files from the folder \"site\""#), "{refused:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The ledger-watch sample (host-capabilities L4 §5.9.11), on the Postgres `HOLON_TEST_POSTGRES`
/// names: its feed bound to a channel on the bound ledger, each entry committed starts one run, in
/// the order committed, and a transaction rolled back sends nothing. The listener's session ended
/// by the database (`pg_terminate_backend`), an entry written meanwhile is never sent: the timeline
/// says so once it listens again, and the next entry's run reads it from the ledger, so the balance
/// stays exact. A channel's binding without its database, or with what is a URL's, is refused;
/// nothing shows the connection; the runs verify.
#[test]
fn ledger_watch_listens_on_its_channel_and_reads_what_it_missed() {
    for fresh in fresh_databases("ledger").into_iter().filter(|f| f.engine == "postgres") {
        let mut state = claims_state("");
        let of = || secret_sources::Sources::of(&[("HOLON_SECRET_LEDGER_URL", fresh.url.as_str())]);
        let databases = databases::Databases::memory().with_sources(of());
        state.folders = Arc::new(folders::Folders::memory().with_databases(databases).with_feeds(feeds::Feeds::memory().with_sources(of())));
        let text = include_str!("../../holon-author/samples/ledger-watch.holon.ts");
        // The schema is ledger.sql, as the sample's comment gives it.
        let schema: Vec<&str> = text.lines().skip_while(|l| !l.contains("where ledger.sql is")).skip(1).take_while(|l| l.trim() != "//").map(|l| l.trim_start_matches("//").trim()).collect();
        let bind_feed = |asked: serde_json::Value| {
            let bound = route(&state, &req("POST", "/admin/feeds", serde_json::to_vec(&asked).unwrap()));
            (bound.status_code, String::from_utf8_lossy(&body_bytes(bound)).into_owned())
        };
        let channel = serde_json::json!({ "name": "entries", "database": "ledger", "channel": "ledger_entries" });
        let (status, said) = bind_feed(channel.clone());
        assert!(status == 400 && said.contains("the database ledger isn't bound on this server"), "{said}");
        let db = serde_json::json!({ "name": "ledger", "connection": "env:HOLON_SECRET_LEDGER_URL", "access": "read", "schema": schema.join(" ") });
        let bound = route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&db).unwrap()));
        let status = bound.status_code;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(bound)));
        let (status, said) = bind_feed(serde_json::json!({ "name": "entries", "database": "ledger", "channel": "ledger-entries" }));
        assert!(status == 400 && said.contains("--channel is a Postgres channel"), "{said}");
        let (status, said) = bind_feed(serde_json::json!({ "name": "entries", "database": "ledger", "channel": "ledger_entries", "url": "env:HOLON_SECRET_LEDGER_URL" }));
        assert!(status == 400 && said.contains("a channel is listened on in its --database"), "{said}");
        let (status, shown) = bind_feed(channel);
        assert_eq!(status, 200, "{shown}");
        let address = format!("{}, channel ledger_entries", fresh.url.split_once('@').map(|(_, at)| format!("postgres://{at}")).unwrap());
        assert!(shown.contains(&format!(r#""address":"{address}""#)) && shown.contains(r#""database":"ledger""#) && !shown.contains(&fresh.url), "{shown}");
        let slug = deploy_text(&state, text, "");

        let balances = || {
            let (status, body, _) = data_call(&state, &slug, "GET", "/balances", "");
            assert_eq!(status, 200, "{body}");
            serde_json::from_str::<serde_json::Value>(&body).unwrap()
        };
        let runs = || log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "feed").count();
        let live = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/feeds"), vec![])))["feeds"][0].clone();
        let events = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
        let mut ledger = postgres::Client::connect(&fresh.url, postgres::NoTls).unwrap();
        let mut write = |sql: &str| ledger.batch_execute(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));

        let mut held = subscriptions::Held::default();
        subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
        eventually("the channel is listened on", || live()["status"] == "connected");
        assert_eq!(live()["address"], address.as_str());
        write("insert into entries (account, seq, amount) values ('acct_1', 1, 120.00), ('acct_1', 2, -20.50), ('acct_2', 1, 5.25)");
        write("begin; insert into entries (account, seq, amount) values ('acct_2', 2, 1000.00); rollback");
        write("begin; insert into entries (account, seq, amount, memo) values ('acct_2', 2, 0.75, 'fee'); insert into entries (account, seq, amount) values ('acct_1', 3, 0.50); commit");
        eventually("five runs", || runs() == 5);
        let b = balances();
        assert_eq!((b["acct_1"]["balance"].as_str(), b["acct_1"]["entries"].as_u64(), b["acct_1"]["last"].as_u64()), (Some("100.00"), Some(3), Some(3)), "{b}");
        assert_eq!((b["acct_2"]["balance"].as_str(), b["acct_2"]["entries"].as_u64(), b["acct_2"]["read"].as_u64()), (Some("6.00"), Some(2), Some(0)), "{b}");
        let sourced = log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "feed").map(|e| e["sourced"].to_string()).collect::<Vec<_>>().join("\n");
        assert!(sourced.contains(r#"FeedEvent(\"entries\")"#) && sourced.contains("fee"), "logged whole: {sourced}");

        // Its session ended: entry 4 is written while nothing listens, so it is never sent; once it
        // listens again, entry 5's run reads it from the ledger.
        write("select pg_terminate_backend(pid, 5000) from pg_stat_activity where application_name = 'holon (listen)' and datname = current_database()");
        write("insert into entries (account, seq, amount) values ('acct_1', 4, 10.00)");
        eventually("it listens again", || events().contains("listens to the feed entries again: what was sent meanwhile is lost"));
        eventually("connected again", || live()["status"] == "connected");
        write("insert into entries (account, seq, amount) values ('acct_1', 5, 2.25)");
        eventually("six runs", || runs() == 6);
        let b = balances();
        assert_eq!((b["acct_1"]["balance"].as_str(), b["acct_1"]["entries"].as_u64(), b["acct_1"]["read"].as_u64(), b["acct_1"]["last"].as_u64()), (Some("112.25"), Some(5), Some(1), Some(5)), "{b}");
        let said = events();
        assert!(said.contains(&format!("listens to the feed entries ({address})")), "{said}");

        // Its runs check offline, the ledger's rows read as each run read them.
        let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
        let report = verify::check(&export, None, &BTreeMap::new());
        assert!(report.ok() && report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{}", verify::describe(&report, None));
        assert!(!serde_json::to_string(&log_entries(&state, &slug)).unwrap().contains(&fresh.url));

        // Stopped: its session is closed.
        assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![])).status_code, 200);
        subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
        let listening = |ledger: &mut postgres::Client| ledger.query_one("select count(*) from pg_stat_activity where application_name = 'holon (listen)' and datname = current_database()", &[]).unwrap().get::<_, i64>(0);
        eventually("its session is closed", || listening(&mut ledger) == 0);
    }
}

/// The payout-events sample (host-capabilities L4 §5.9.9): refused until its queues are bound;
/// bound to a stand-in for SQS, each message the queue hands out starts one run, in the order
/// received, and is deleted once its record is on disk, a failed run's too; a failed payout's retry
/// goes to the FIFO queue once its run is over, its group the account. A message handed out again
/// is passed over, the state endpoint says what each queue came to, a stopped listener leaves
/// what comes in the queue, nothing holds the URL's key, and the runs verify.
#[test]
fn payout_events_runs_each_message_once_and_sends_a_failed_payouts_retry_to_its_fifo_queue() {
    let sqs = sqs::stand_in::StandIn::start();
    sqs.make("payout-events");
    sqs.make("payout-retries.fifo");
    let dir = std::env::temp_dir().join(format!("holon-payouts-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    std::fs::write(&env, format!("HOLON_SECRET_EVENTS_URL={}\nHOLON_SECRET_RETRIES_URL={}\n", sqs.url("payout-events"), sqs.url("payout-retries.fifo"))).unwrap();
    let mut state = claims_state("");
    let queues = queues::Queues::memory().with_sources(secret_sources::Sources::new(Some(env)).unwrap()).with_retry([Duration::from_millis(50); 4], Duration::from_secs(60));
    state.folders = Arc::new(folders::Folders::memory().with_queues(queues));
    state.folders.queues.serve(Arc::clone(&state.deployments));
    let text = include_str!("../../holon-author/samples/payout-events.holon.ts");

    // Refused until both are bound.
    let plan = holon_author::agent::plan(text).unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let refused = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": [] }));
    let body = String::from_utf8_lossy(&body_bytes(refused)).to_string();
    assert!(body.contains("isn't bound on this server") && body.contains("holon queue add"), "{body}");
    for (name, url) in [("payout_events", "env:HOLON_SECRET_EVENTS_URL"), ("payout_retries", "env:HOLON_SECRET_RETRIES_URL")] {
        let bound = route(&state, &req("POST", "/admin/queues", serde_json::to_vec(&serde_json::json!({ "name": name, "url": url })).unwrap()));
        let status = bound.status_code;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(bound)));
    }
    let shown = body_json(route(&state, &req("GET", "/admin/queues", vec![])));
    let fifo: Vec<(&str, bool)> = shown.as_array().unwrap().iter().map(|q| (q["name"].as_str().unwrap(), q["fifo"].as_bool().unwrap())).collect();
    assert_eq!(fifo, [("payout_events", false), ("payout_retries", true)], "{shown}");
    assert!(!shown.to_string().contains("s3cret"), "{shown}");
    let slug = deploy_text(&state, text, "");

    let event = |kind: &str, id: &str, account: &str| serde_json::json!({ "type": kind, "payout": { "id": id, "amount": "120.00", "currency": "EUR", "account": account } }).to_string();
    let first = sqs.put("payout-events", &event("payout.paid", "po_1", "acct_1"), None);
    sqs.put("payout-events", &event("payout.failed", "po_2", "acct_1"), None);
    sqs.put("payout-events", &event("payout.failed", "po_3", "acct_2"), None);
    // One whose retry is past what the FIFO queue takes: its run fails, nothing is written or
    // sent, and it is deleted all the same.
    let large = serde_json::json!({ "type": "payout.failed", "payout": { "id": "po_4", "amount": "1.00", "currency": "EUR", "account": "acct_3", "memo": "x".repeat(300_000) } });
    sqs.put("payout-events", &large.to_string(), None);
    let runs = || log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "queue").collect::<Vec<_>>();
    let mut held = dequeue::Held::default();
    dequeue::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
    eventually("four runs", || runs().len() == 4);
    eventually("each message deleted", || sqs.messages("payout-events").is_empty());
    let ran = runs();
    assert_eq!(ran.iter().map(|r| r["route"].as_str().unwrap_or_default()).collect::<Vec<_>>(), ["on queue payout_events"; 4]);
    assert_eq!(ran.iter().map(|r| r["error"].is_string()).collect::<Vec<_>>(), [false, false, false, true]);
    assert!(ran[3]["error"].as_str().is_some_and(|e| e.starts_with(queues::SEND_FAILED) && e.contains("payout_retries")), "{}", ran[3]["error"]);
    let (status, body, _) = data_call(&state, &slug, "GET", "/payouts", "");
    assert_eq!(status, 200, "{body}");
    let payouts: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(payouts.as_object().map(|p| p.keys().cloned().collect::<Vec<_>>()), Some(vec!["po_1".to_string(), "po_2".into(), "po_3".into()]), "{payouts}");
    assert_eq!((payouts["po_1"]["status"].as_str(), payouts["po_2"]["status"].as_str(), payouts["po_1"]["event"].as_str()), (Some("paid"), Some("failed"), Some(first.as_str())), "{payouts}");
    assert!(payouts["po_1"]["at"].as_f64().is_some_and(|at| at.fract() == 0.0 && at > 1.7e9), "its time sent, in whole seconds: {payouts}");

    // The failed payouts' retries, each in its account's group, once each run was over.
    eventually("two retries", || sqs.messages("payout-retries.fifo").len() == 2);
    let retries = sqs.messages("payout-retries.fifo");
    assert_eq!(retries.iter().map(|m| m.group.as_deref().unwrap_or_default()).collect::<Vec<_>>(), ["acct_1", "acct_2"]);
    let retried: serde_json::Value = serde_json::from_str(&retries[0].body).unwrap();
    assert_eq!(retried, serde_json::json!({ "id": "po_2", "amount": "120.00", "currency": "EUR", "account": "acct_1" }));
    let asked = sqs.asked();
    assert_eq!(asked.iter().filter(|a| a.starts_with("SendMessage")).count(), 2, "{asked:?}");
    assert_eq!(asked.iter().filter(|a| *a == "DeleteMessage payout-events").count(), 4, "{asked:?}");

    // Handed out again, as a standard queue may: passed over, and deleted.
    sqs.again("payout-events", &first, &event("payout.paid", "po_1", "acct_1"));
    let events = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/events"), vec![])))["events"].to_string();
    eventually("passed over", || events().contains(&format!("passed over a message of payout_events it ran already ({first})")));
    eventually("deleted again", || sqs.messages("payout-events").is_empty());
    assert_eq!(runs().len(), 4, "run once");
    let shown = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/queues"), vec![])));
    let (listening, sending) = (&shown["listening"][0], &shown["sending"][0]);
    assert_eq!((listening["queue"].as_str(), listening["status"].as_str(), listening["run"].as_u64(), listening["passed_over"].as_u64()), (Some("payout_events"), Some("receiving"), Some(4), Some(1)), "{shown}");
    assert_eq!((sending["queue"].as_str(), sending["sent"].as_u64(), sending["waiting"].as_u64()), (Some("payout_retries"), Some(2), Some(0)), "{shown}");
    let events = events();
    assert!(events.contains("listens to the queue payout_events"), "{events}");
    assert!(!events.contains("couldn't"), "{events}");

    // Stopped: nothing listens once its last long poll is over, and a message put now waits.
    drop(held);
    eventually("the listener stops", || {
        let n = sqs.asked().len();
        thread::sleep(Duration::from_millis(1500));
        sqs.asked().len() == n
    });
    sqs.put("payout-events", &event("payout.paid", "po_5", "acct_3"), None);
    thread::sleep(Duration::from_millis(300));
    assert_eq!((runs().len(), sqs.messages("payout-events").len()), (4, 1));

    // Bound again to keep a failed run's message, and listened to again: the one put while it was
    // stopped runs; one whose run fails is left in the queue, handed out again once it is visible,
    // and run again.
    let bind = serde_json::json!({ "name": "payout_events", "url": "env:HOLON_SECRET_EVENTS_URL", "keep_failed": true });
    assert_eq!(route(&state, &req("POST", "/admin/queues", serde_json::to_vec(&bind).unwrap())).status_code, 200);
    let kept = sqs.put("payout-events", &large.to_string(), None);
    let mut held = dequeue::Held::default();
    dequeue::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
    let listening = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/queues"), vec![])))["listening"][0].clone();
    eventually("six runs", || runs().len() == 6);
    eventually("one kept", || listening()["kept"] == 1);
    assert_eq!(sqs.messages("payout-events").iter().map(|m| m.id.clone()).collect::<Vec<_>>(), std::slice::from_ref(&kept), "the failed one waits out of sight");
    sqs.expire("payout-events");
    eventually("run again", || runs().len() == 7);
    eventually("kept again", || listening()["kept"] == 2);
    assert_eq!(sqs.messages("payout-events").iter().map(|m| m.id.clone()).collect::<Vec<_>>(), [kept]);
    assert!(runs()[6]["error"].as_str().is_some_and(|e| e.starts_with(queues::SEND_FAILED)));
    drop(held);

    // Its runs check offline, and the URL's key is in no log. Those that failed past what the queue
    // takes aren't checked: what the server dropped, run again, is sent (verify isn't given its
    // bindings), so the export checks only so far.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    let failed = |r: &verify::Rerun| matches!(r, verify::Rerun::Skipped(why) if why.contains("message not sent"));
    let (matched, skipped) = (report.reruns.iter().filter(|(_, r)| *r == verify::Rerun::Matched).count(), report.reruns.iter().filter(|(_, r)| failed(r)).count());
    assert!(report.ok() && !report.complete() && (matched, skipped) == (5, 3), "{}", verify::describe(&report, None));
    assert!(!serde_json::to_string(&log_entries(&state, &slug)).unwrap().contains("s3cret"));
    assert!(replays_as_it_verifies(&state, &slug).contains("queue"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── MongoDB (slice 7) ──

/// The server compiles a flow's text with the editor's lowering, so a record literal must freeze
/// as the editor's wasm freezes it, keys sorted: no dependency of this binary may turn on
/// serde_json's `preserve_order` (bson 2, the MongoDB driver's default, does: Cargo.toml).
#[test]
fn a_record_literal_freezes_its_keys_sorted_as_the_editor_does() {
    let v: serde_json::Value = serde_json::from_str(r#"{"k":1,"done":false}"#).unwrap();
    assert_eq!(v.to_string(), r#"{"done":false,"k":1}"#, "serde_json keeps keys as written here: a dependency turned on preserve_order");
    let text = "import { flow, route } from \"holon\"\n\nflow(\"keys\", { port: 18199 })\n\nroute.get(\"/k\", (req) => req.json ?? { k: 1, done: false })\n";
    let unit = |t: &str| holon_author::agent::plan(t).unwrap_or_else(|e| panic!("{e}")).unit;
    assert_eq!(unit(text), unit(&text.replace("k: 1, done: false", "done: false, k: 1")), "one record, written in either order, is one unit");
}

/// A fresh, empty MongoDB database for one test, on the replica set `HOLON_TEST_MONGODB` names (a
/// URL with any database in its path), dropped when the test is done with it. None named: none,
/// and it says so.
fn fresh_mongo(tag: &str) -> Option<FreshMongo> {
    let Ok(admin) = std::env::var("HOLON_TEST_MONGODB") else {
        eprintln!("{tag}: skipped: set HOLON_TEST_MONGODB (a replica set's URL) to run it against MongoDB");
        return None;
    };
    let name = format!("holon_{tag}_{:08x}", rand::random::<u32>());
    let url = with_database(&admin, &name);
    let client = mongodb::sync::Client::with_uri_str(&url).unwrap_or_else(|e| panic!("HOLON_TEST_MONGODB: {e}"));
    Some(FreshMongo { url, db: client.database(&name) })
}

struct FreshMongo {
    url: String,
    db: mongodb::sync::Database,
}

impl FreshMongo {
    fn notes(&self) -> mongodb::sync::Collection<mongodb::bson::Document> {
        self.db.collection("notes")
    }
}

impl Drop for FreshMongo {
    fn drop(&mut self) {
        let _ = self.db.drop().run();
    }
}

fn bind_notes(state: &AppState) -> rouille::Response {
    let body = serde_json::json!({ "name": "notes", "connection": "env:HOLON_SECRET_SHOP_URL", "access": "read-write" });
    route(state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap()))
}

/// The atlas-notes sample on a MongoDB replica set: notes inserted, found by owner and by tag,
/// edited, marked done once (twice, the second finds no open note and answers 409), deleted; an
/// `_id` taken already answers 409; a value that would be an operator fails the run with nothing
/// written; every commit is marked once; and every run checks offline.
#[test]
fn atlas_notes_keeps_notes_as_documents_on_mongodb() {
    let Some(db) = fresh_mongo("notes") else { return };
    let state = server_with_database(&db.url);
    let r = bind_notes(&state);
    let status = r.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(r)));
    let listed = body_json(route(&state, &req("GET", "/admin/databases", vec![])));
    assert_eq!(listed[0]["engine"], "mongodb");
    assert!(!listed.to_string().contains(&db.url), "the URL is never shown: {listed}");
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/atlas-notes.holon.ts"), "");
    let call = |method: &str, path: &str, body: &str| {
        let (status, body, _) = data_call(&state, &slug, method, path, body);
        (status, body)
    };
    assert_eq!(call("POST", "/notes/ada", r#"{"id":"n1","text":"buy milk","tags":["home"]}"#).0, 201);
    assert_eq!(call("POST", "/notes/ada", r#"{"id":"n2","text":"call bo"}"#).0, 201);
    assert_eq!(call("POST", "/notes/bo", r#"{"id":"n3","text":"not ada's","tags":["home"]}"#).0, 201);
    let (status, found) = call("GET", "/notes/ada", "");
    assert_eq!(status, 200, "{found}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&found).unwrap(),
        serde_json::json!([
            { "_id": "n1", "owner": "ada", "text": "buy milk", "tags": ["home"], "done": false, "edits": 0 },
            { "_id": "n2", "owner": "ada", "text": "call bo", "tags": [], "done": false, "edits": 0 },
        ])
    );
    let (_, tagged) = call("GET", "/notes/ada/tagged/home", "");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&tagged).unwrap().as_array().map(Vec::len), Some(1), "{tagged}");
    assert_eq!(call("PUT", "/notes/ada/n1", r#"{"text":"buy oat milk"}"#).0, 200);
    let (status, edited) = call("PUT", "/notes/bo/n1", r#"{"text":"bo's edit"}"#);
    assert_eq!(status, 409, "another owner's note matches nothing: {edited}");
    assert!(edited.contains("matched 0 documents, not 1 document"), "{edited}");
    let (status, done) = call("POST", "/notes/ada/n1/done", "");
    assert_eq!((status, serde_json::from_str::<serde_json::Value>(&done).unwrap()), (200, serde_json::json!({ "done": "n1", "text": "buy oat milk" })));
    assert_eq!(call("POST", "/notes/ada/n1/done", "").0, 404, "no open note: nothing to mark");
    let (status, taken) = call("POST", "/notes/ada", r#"{"id":"n2","text":"again"}"#);
    assert_eq!(status, 409, "{taken}");
    assert!(taken.contains("a document with its _id is there already"), "{taken}");
    // A value is data: a record with an operator in it fails the run, and nothing lands.
    let (status, sneaky) = call("PUT", "/notes/ada/n2", r#"{"text":{"$set":{"owner":"mallory"}}}"#);
    assert_eq!(status, 500, "{sneaky}");
    let (status, sneaky) = call("POST", "/notes/ada", r#"{"id":{"$gt":""},"text":"x"}"#);
    assert_eq!(status, 500, "{sneaky}");
    assert_eq!(call("DELETE", "/notes/ada/n2", "").0, 204);
    assert_eq!(call("DELETE", "/notes/ada/n2", "").0, 409, "deleted already");
    let n2 = db.notes().find_one(mongodb::bson::doc! { "_id": "n2" }).run().unwrap();
    assert!(n2.is_none());
    let n1 = db.notes().find_one(mongodb::bson::doc! { "_id": "n1" }).run().unwrap().unwrap();
    assert_eq!((n1.get_str("text").unwrap(), n1.get_bool("done").unwrap(), n1.get_i32("edits").unwrap()), ("buy oat milk", true, 1));
    let marked = db.db.collection::<mongodb::bson::Document>("holon_commits").count_documents(mongodb::bson::doc! {}).run().unwrap();
    assert_eq!(marked, 6, "three inserts, an edit, a done and a delete, each marked once");
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
}

/// A document a run found that a program outside Holon changes before the run commits sends it
/// round again, its call out answered from its record: it writes from the document as it is now,
/// and the change outside isn't written over.
#[test]
fn a_document_that_changed_before_its_commit_runs_again() {
    let Some(db) = fresh_mongo("again") else { return };
    let (host, hits) = counting_server();
    let state = server_with_database(&db.url);
    assert_eq!(bind_notes(&state).status_code, 200);
    db.notes().insert_one(mongodb::bson::doc! { "_id": "n1", "owner": "ada", "text": "first", "edits": 0 }).run().unwrap();
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport notes from \"database:notes\"\nimport host from \"host:{host}\"\n\nflow(\"stamp\", {{ port: 18171 }})\n\nroute.post(\"/stamp/:id\", async (req) => {{\n  const note = notes.find_one(\"notes\", {{ _id: req.params.id }})\n  const r = await host.get(\"/slow\")\n  notes.update(\n    \"notes\",\n    {{ _id: note._id }},\n    {{ \"$set\": {{ text: `${{note.text}} (${{r.body}})` }} }},\n    {{ expect: 1 }},\n  )\n  return {{ was: note.text }}\n}})\n"
    );
    let slug = deploy_text(&state, &text, &host);
    let outside = std::thread::scope(|s| {
        let run = s.spawn(|| data_call(&state, &slug, "POST", "/stamp/n1", ""));
        std::thread::sleep(Duration::from_millis(150));
        db.notes().update_one(mongodb::bson::doc! { "_id": "n1" }, mongodb::bson::doc! { "$set": { "text": "changed outside" } }).run().unwrap();
        run.join().unwrap()
    });
    assert_eq!((outside.0, outside.1.as_str()), (200, r#"{"was":"changed outside"}"#));
    assert_eq!(hits_of(&hits, "/slow"), 1, "the call out answered from its record, not sent again");
    let n1 = db.notes().find_one(mongodb::bson::doc! { "_id": "n1" }).run().unwrap().unwrap();
    assert_eq!(n1.get_str("text").unwrap(), "changed outside (slow)");
    let entries = log_entries(&state, &slug);
    assert!(entries.iter().any(|e| e["retries"] == 1), "{entries:#?}");
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    assert!(verify::check(&export, None, &BTreeMap::new()).ok());
}

/// Two servers on one MongoDB, each stamping one note at the same moment: their own tables of
/// claims can't see each other's, the database's `holon_claims` can. The first to call out holds
/// the note until it commits, so the slow API is called once per stamp in turn, never twice at
/// once for one note; notes apart never wait.
#[test]
fn two_servers_hold_one_document_through_mongodb() {
    let Some(db) = fresh_mongo("claims") else { return };
    let (host, hits) = counting_server();
    let servers = [server_with_database(&db.url), server_with_database(&db.url)];
    for s in &servers {
        assert_eq!(bind_notes(s).status_code, 200);
    }
    db.notes().insert_one(mongodb::bson::doc! { "_id": "n1", "paid": false }).run().unwrap();
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport notes from \"database:notes\"\nimport host from \"host:{host}\"\n\nflow(\"pay-note\", {{ port: 18172 }})\n\nroute.post(\"/pay/:id\", async (req) => {{\n  const note = notes.find_one(\"notes\", {{ _id: req.params.id, paid: false }})\n  const r = await host.get(`/slow?note=${{note._id}}`)\n  notes.update(\"notes\", {{ _id: note._id }}, {{ \"$set\": {{ paid: true, by: r.body }} }}, {{ expect: 1 }})\n  return {{ paid: note._id }}\n}})\n"
    );
    let slugs = [deploy_text(&servers[0], &text, &host), deploy_text(&servers[1], &text, &host)];
    let (first, second) = std::thread::scope(|s| {
        let first = s.spawn(|| data_call(&servers[0], &slugs[0], "POST", "/pay/n1", ""));
        std::thread::sleep(Duration::from_millis(60));
        let second = s.spawn(|| data_call(&servers[1], &slugs[1], "POST", "/pay/n1", ""));
        (first.join().unwrap(), second.join().unwrap())
    });
    let mut statuses = [first.0, second.0];
    statuses.sort();
    assert_eq!(statuses, [200, 404], "{} / {}", first.1, second.1);
    assert_eq!(hits_of(&hits, "/slow"), 1, "one payment for one note, across servers");
    let claims = db.db.collection::<mongodb::bson::Document>("holon_claims").count_documents(mongodb::bson::doc! { "h.0": { "$exists": true } }).run().unwrap();
    assert_eq!(claims, 0, "every claim let go once its run was done");
}

/// A MongoDB database's run lock and claims, held in its `holon_claims` across two connections
/// (two servers): exclusive keeps out everyone, shared keeps out only exclusive, and each holder
/// lets go of its own.
#[test]
fn mongodb_holds_run_locks_shared_and_exclusive_across_servers() {
    let Some(db) = fresh_mongo("locks") else { return };
    let open = || Arc::new(mongo::Mongo::open(zeroize::Zeroizing::new(db.url.clone())).unwrap());
    let (a, b) = (open(), open());
    assert!(a.hold_run("run-a", true));
    assert!(!b.hold_run("run-b", false), "exclusive keeps out a reader");
    assert!(!b.hold_run("run-b", true), "and a writer");
    a.let_go_run("run-a", true);
    assert!(b.hold_run("run-b", false) && a.hold_run("run-c", false), "readers side by side");
    assert!(!a.hold_run("run-d", true), "a writer waits for the readers");
    b.let_go_run("run-b", false);
    a.let_go_run("run-c", false);
    assert!(a.hold_run("run-d", true));
    a.let_go_run("run-d", true);
    let row = |id: &str| claims::ClaimKey::Row { at: a.address.clone(), name: "notes".into(), table: "notes".into(), key: format!("[{id:?}]") };
    let table = claims::ClaimKey::Table { at: a.address.clone(), name: "notes".into(), table: "notes".into() };
    assert!(a.claim(&[row("n1")], "run-e").is_ok());
    assert!(b.claim(&[row("n2")], "run-f").is_ok(), "documents apart never wait");
    assert_eq!(b.claim(&[row("n1")], "run-g"), Err(row("n1")));
    assert_eq!(b.claim(std::slice::from_ref(&table), "run-g"), Err(table.clone()), "a whole collection waits for its documents");
    a.unclaim(&[row("n1")], "run-e");
    b.unclaim(&[row("n2")], "run-f");
    assert!(b.claim(std::slice::from_ref(&table), "run-g").is_ok());
    b.unclaim(&[table], "run-g");
}

/// A MongoDB is bound by its connection, never with a schema; one a server can't reach says so,
/// without its URL.
#[test]
fn a_mongodb_binds_without_a_schema() {
    let Some(db) = fresh_mongo("bind") else { return };
    let state = server_with_database(&db.url);
    let body = serde_json::json!({ "name": "notes", "connection": "env:HOLON_SECRET_SHOP_URL", "access": "read-write", "schema": "create table t (a int)" });
    let r = route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&body).unwrap()));
    let said = String::from_utf8_lossy(&body_bytes(r)).to_string();
    assert!(said.contains("has no schema to run"), "{said}");
    let nowhere = server_with_database("mongodb://ada:hunter2@127.0.0.1:1/notes?serverSelectionTimeoutMS=500");
    let r = bind_notes(&nowhere);
    let said = String::from_utf8_lossy(&body_bytes(r)).to_string();
    assert!(said.contains("can't be reached") && !said.contains("hunter2"), "{said}");
}

/// A URL pasted with its provider's placeholder still in it (Atlas's `<db_password>`, another's
/// `[YOUR-PASSWORD]`) is turned away before it is tried, saying so and showing none of the URL;
/// the same characters percent-encoded, as a real password's are, are tried.
#[test]
fn a_connection_whose_password_is_still_a_placeholder_says_so() {
    for url in [
        "mongodb+srv://ada:<db_password>@cluster0.abc12.mongodb.net/notes?retryWrites=true",
        "postgres://<username>:hunter2@db.example.com:5432/shop",
        "postgresql://postgres:[YOUR-PASSWORD]@db.abc12.supabase.co:5432/postgres",
    ] {
        let r = bind_notes(&server_with_database(url));
        let status = r.status_code;
        let said = String::from_utf8_lossy(&body_bytes(r)).to_string();
        assert!(status >= 400 && said.contains("still a placeholder") && said.contains("env:HOLON_SECRET_SHOP_URL"), "{status} {said}");
        assert!(!said.contains("example.com") && !said.contains("abc12") && !said.contains("hunter2"), "none of the URL: {said}");
    }
    let r = bind_notes(&server_with_database("mongodb://ada:%3Cpw%3E@127.0.0.1:1/notes?serverSelectionTimeoutMS=500"));
    let said = String::from_utf8_lossy(&body_bytes(r)).to_string();
    assert!(said.contains("can't be reached") && !said.contains("placeholder"), "{said}");
}

/// A channel through a real socket (`docs/L4s/channels-L4.md`): a subscriber gets each pack a
/// run sends once the run is over, as an event with its id; one asking for the kept packs, or
/// reconnecting after an id, gets those first; pausing ends the subscriptions with `closed`.
#[test]
fn a_channel_streams_each_runs_packs_to_its_subscribers() {
    use std::io::{BufRead, BufReader, Write};
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true").unwrap());
    let port = free_port();
    let text = format!(
        "import {{ flow, route, status }} from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"board\", {{ port: {port} }})\n\nroute.post(\"/t\", (req) => {{\n  updates.send({{ new: req.json.title }})\n  return status(201, {{ ok: true }})\n}})\n\nroute.get(\"/live\", () => updates.subscribe())\n\nroute.get(\"/all\", () => updates.subscribe({{ from: \"kept\" }}))\n"
    );
    let slug = deploy_text(&state, &text, "x");
    sync_listeners(&state);
    // A subscriber: the response's headers, then its events, a line at a time.
    let subscribe = |path: &str, last: Option<&str>| {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let last = last.map(|id| format!("Last-Event-ID: {id}\r\n")).unwrap_or_default();
        stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{last}\r\n").as_bytes()).unwrap();
        let mut reader = BufReader::new(stream);
        let mut head = String::new();
        while !head.ends_with("\r\n\r\n") {
            reader.read_line(&mut head).unwrap();
        }
        (head, reader)
    };
    // The next event's lines (chunk sizes and comments skipped).
    let next_event = |reader: &mut BufReader<std::net::TcpStream>| {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end().to_string();
            if line.starts_with("id:") || line.starts_with("data:") || line.starts_with("event:") {
                lines.push(line);
            } else if line.is_empty() && !lines.is_empty() {
                return lines;
            }
        }
    };
    let post = |path: &str, body: &str| raw_http(port, format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());

    let (head, mut live) = subscribe("/live", None);
    assert!(head.starts_with("HTTP/1.1 200") && head.contains("content-type: text/event-stream"), "{head}");
    assert!(post("/t", r#"{"title":"one"}"#).starts_with("HTTP/1.1 201"));
    let first = next_event(&mut live);
    let id = first[0].strip_prefix("id: ").unwrap().to_string();
    assert_eq!(first[1], r#"data: {"new":"one"}"#);
    assert!(post("/t", r#"{"title":"three"}"#).starts_with("HTTP/1.1 201"));
    assert_eq!(next_event(&mut live)[1], r#"data: {"new":"three"}"#);

    let (_, mut all) = subscribe("/all", None);
    assert_eq!(next_event(&mut all)[1], r#"data: {"new":"one"}"#, "the kept ones first");
    let (_, mut after) = subscribe("/live", Some(&id));
    assert_eq!(next_event(&mut after)[1], r#"data: {"new":"three"}"#, "the kept ones past the last it saw");

    let entries = log_entries(&state, &slug);
    assert!(entries.iter().any(|e| e["route"] == "GET /live" && e["status"] == 200), "a subscription is logged as a request");
    // What the console counts (Resources › Collections and channels): the packs kept, the
    // subscribers now, none of the packs.
    let counted = || body_json(route(&state, &req("GET", &format!("/deployments/{slug}/channels"), vec![])));
    assert_eq!(counted(), serde_json::json!({ "channels": [{ "name": "updates", "kept": 2, "subscribers": 3 }] }));

    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/pause"), vec![])).status_code, 200);
    assert_eq!(next_event(&mut live), ["event: closed", "data: {}"]);
    assert_eq!(counted()["channels"][0], serde_json::json!({ "name": "updates", "kept": 2, "subscribers": 0 }), "the kept packs stay");
    assert_eq!(route(&state, &req("GET", "/deployments/nope/channels", vec![])).status_code, 404);
}

/// A durable run's progress (`docs/L4s/channels-L4.md` §8, slice 3): the pack that doesn't wait
/// goes out as the run parks, the rest when it finishes, and the first never again, though the
/// resumed part settles it again; the channel rebuilt from the log says the same.
#[test]
fn a_waiting_run_sends_what_didnt_wait_as_it_parks() {
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]
allow_plain_http = true
allow_private_addresses = true").unwrap());
    let port = free_port();
    let text = format!(
        "import {{ action, event, flow, route }} from \"holon\"\nimport orders from \"channel:orders\"\n\nflow(\"orders\", {{ port: {port} }})\n\nroute.post(\"/orders\", (req) => {{\n  const started = payment(req.json)\n  return `/paid/${{started.waiting.paid}}`\n}})\n\nexport const payment = action(async (order) => {{\n  orders.send({{ item: order.item, status: \"waiting\" }})\n  const paid = await event(\"paid\", {{ expires: \"1d\" }})\n  orders.send({{ item: order.item, status: \"paid\", via: paid.method }})\n}})\n\nroute.get(\"/paid/:token\", () => \"thanks\", {{ resumes: \"paid\" }})\n"
    );
    let slug = deploy_text(&state, &text, "x");
    sync_listeners(&state);
    let kept = |state: &AppState| -> Vec<String> {
        let table = state.deployments.lock().unwrap();
        table[&slug].channels.kept("orders", true).iter().map(|p| format!("{} {}", p.id.unwrap(), p.json)).collect()
    };
    let posted = raw_http(port, b"POST /orders HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"item\":\"book\"}");
    assert!(posted.starts_with("HTTP/1.1 200"), "{posted}");
    assert!(posted.contains("content-length:"), "a whole answer says its length: {posted}");
    let link = posted[posted.find("/paid/").unwrap()..].trim_end_matches(['"', '\r', '\n']).to_string();
    let parked = kept(&state);
    assert_eq!(parked.len(), 1, "{parked:?}");
    assert!(parked[0].ends_with(r#"{"item":"book","status":"waiting"}"#), "{parked:?}");

    let paid = raw_http(port, format!("GET {link} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes());
    assert!(paid.starts_with("HTTP/1.1 200"), "{paid}");
    let both = kept(&state);
    assert_eq!(both.len(), 2, "the waiting one isn't sent again: {both:?}");
    assert!(both[1].ends_with(r#"{"item":"book","status":"paid","via":"GET"}"#), "{both:?}");
    let entries = log_entries(&state, &slug);
    assert!(entries.iter().any(|e| e["progress"].as_array().is_some_and(|p| p.len() == 1)), "the parked part logs what it sent");

    let table = state.deployments.lock().unwrap();
    let dep = &table[&slug];
    let rebuilt = dep.log.with(|log| channels::Channels::from_log(log, &dep.permit.channels));
    let again: Vec<String> = rebuilt.kept("orders", true).iter().map(|p| format!("{} {}", p.id.unwrap(), p.json)).collect();
    assert_eq!(again, both);
}

/// A streamed reply through a real socket (`docs/L4s/channels-L4.md` §4): the caller gets the
/// upstream's first piece while the upstream is still answering (it sends the second only once
/// the caller has the first), with its content type; the flow still gets the whole answer, which
/// it stores and logs.
#[test]
fn a_streamed_reply_forwards_the_upstream_as_it_arrives() {
    use std::io::{BufRead, BufReader, Read, Write};
    let upstream = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = upstream.local_addr().unwrap().to_string();
    let (seen_tx, seen_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let (mut stream, _) = upstream.accept().unwrap();
        let _ = read_request(&mut stream);
        let piece = |s: &str| format!("{:x}\r\n{s}\r\n", s.len());
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        stream.write_all(format!("{head}{}", piece("data: a\n\n")).as_bytes()).unwrap();
        stream.flush().unwrap();
        let _ = seen_rx.recv_timeout(Duration::from_secs(10));
        stream.write_all(format!("{}0\r\n\r\n", piece("data: b\n\n")).as_bytes()).unwrap();
    });
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true").unwrap());
    let port = free_port();
    let text = format!(
        "import {{ flow, respond, route }} from \"holon\"\nimport said from \"collection:said\"\nimport host from \"host:{host}\"\n\nflow(\"chat\", {{ port: {port} }})\n\nroute.post(\"/ask\", async (req) => {{\n  const sent = await host.post(\"/v1\", {{ body: req.json, stream: true }})\n  said[\"last\"] = sent.body\n  return respond(sent.body, {{ stream: true }})\n}})\n"
    );
    let slug = deploy_text(&state, &text, &host);
    sync_listeners(&state);

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(b"POST /ask HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
    let mut reader = BufReader::new(stream);
    let mut head = String::new();
    while !head.ends_with("\r\n\r\n") {
        reader.read_line(&mut head).unwrap();
    }
    assert!(head.starts_with("HTTP/1.1 200") && head.contains("content-type: text/event-stream"), "{head}");
    let mut got = String::new();
    while !got.contains("data: a") {
        reader.read_line(&mut got).unwrap();
    }
    assert!(!got.contains("data: b"), "the second piece isn't sent yet");
    seen_tx.send(()).unwrap();
    let mut rest = String::new();
    let _ = reader.read_to_string(&mut rest);
    assert!(rest.contains("data: b") && !rest.contains("event: error"), "{rest}");

    // The flow got the whole answer: logged as the route's response, and stored.
    let entry = log_entries(&state, &slug).into_iter().find(|e| e["route"] == "POST /ask").unwrap();
    assert_eq!(entry["status"], 200);
    assert_eq!(entry["response_body"], "data: a\n\ndata: b\n\n");
    assert!(entry["performed"].to_string().contains("CollectionWrite"), "the whole answer stored: {entry}");
}

/// A streamed call that fails (not 2xx) forwards nothing: the reply is its answer, whole, with
/// its status (the respond's status input, wired from the call's), so the caller sees the 401 and
/// the error, and the log says the same.
#[test]
fn a_streamed_reply_whose_call_fails_answers_with_its_status() {
    use std::io::{Read, Write};
    let upstream = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = upstream.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = upstream.accept().unwrap();
        let _ = read_request(&mut stream);
        let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        let answer = format!("HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        stream.write_all(answer.as_bytes()).unwrap();
    });
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true").unwrap());
    let port = free_port();
    let text = format!("import {{ flow, respond, route }} from \"holon\"\nimport host from \"host:{host}\"\n\nflow(\"chat\", {{ port: {port} }})\n\nroute.post(\"/ask\", async (req) => {{\n  const sent = await host.post(\"/v1\", {{ body: req.json, stream: true }})\n  return respond(sent.body, {{ stream: true }})\n}})\n");
    let slug = deploy_text(&state, &text, &host);
    sync_listeners(&state);

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(b"POST /ask HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    let entry = log_entries(&state, &slug).into_iter().find(|e| e["route"] == "POST /ask").unwrap();
    assert!(answer.starts_with("HTTP/1.1 401") && answer.contains("invalid x-api-key"), "{answer}\n{entry}");
    assert_eq!(entry["status"], 401);
}

/// A piped call (`docs/L4s/channels-L4.md` §8, slice 3): its upstream's pieces go to the
/// channel's subscribers as they arrive, each a text pack without an id (never kept), while the
/// run goes on; the packs it sends once it is over follow, with ids.
#[test]
fn a_piped_call_sends_its_answer_to_the_channel_as_it_arrives() {
    use std::io::{BufRead, BufReader, Write};
    let upstream = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = upstream.local_addr().unwrap().to_string();
    let (seen_tx, seen_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let (mut stream, _) = upstream.accept().unwrap();
        let _ = read_request(&mut stream);
        let piece = |s: &str| format!("{:x}\r\n{s}\r\n", s.len());
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        stream.write_all(format!("{head}{}", piece("Rivers run")).as_bytes()).unwrap();
        stream.flush().unwrap();
        let _ = seen_rx.recv_timeout(Duration::from_secs(10));
        stream.write_all(format!("{}0\r\n\r\n", piece(" to the sea.")).as_bytes()).unwrap();
    });
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true").unwrap());
    let port = free_port();
    let text = format!(
        "import {{ flow, route, status }} from \"holon\"\nimport room from \"channel:room\"\nimport host from \"host:{host}\"\n\nflow(\"room\", {{ port: {port} }})\n\nroute.post(\"/ask\", (req) => {{\n  const sent = room.pipe(host.post(\"/v1\", {{ body: req.json }}))\n  room.send({{ said: sent.body }})\n  return status(202, {{ ok: true }})\n}})\n\nroute.get(\"/live\", () => room.subscribe())\n"
    );
    deploy_text(&state, &text, &host);
    sync_listeners(&state);

    let mut live = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    live.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    live.write_all(b"GET /live HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
    let mut live = BufReader::new(live);
    let mut head = String::new();
    while !head.ends_with("\r\n\r\n") {
        live.read_line(&mut head).unwrap();
    }
    let next_event = |reader: &mut BufReader<std::net::TcpStream>| {
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let line = line.trim_end().to_string();
            if line.starts_with("id:") || line.starts_with("data:") || line.starts_with("event:") {
                lines.push(line);
            } else if line.is_empty() && !lines.is_empty() {
                return lines;
            }
        }
    };
    let asked = std::thread::spawn(move || raw_http(port, b"POST /ask HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"));
    assert_eq!(next_event(&mut live), [r#"data: "Rivers run""#], "the first piece, while the upstream holds the rest");
    seen_tx.send(()).unwrap();
    assert_eq!(next_event(&mut live), [r#"data: " to the sea.""#]);
    let done = next_event(&mut live);
    assert!(done[0].starts_with("id: "), "{done:?}");
    assert_eq!(done[1], r#"data: {"said":"Rivers run to the sea."}"#);
    assert!(asked.join().unwrap().starts_with("HTTP/1.1 202"));
}

#[test]
fn no_folder_is_or_holds_what_the_server_keeps_beside_its_files() {
    let dir = unique_temp_path("server-files");
    let deployments = format!("{dir}/data/deployments.json");
    let (logs, keys, env) = (format!("{dir}/data/deployments.logs"), format!("{dir}/keys"), format!("{dir}/conf"));
    // Kept outside ~/.holon (--data elsewhere): what is beside the deployments file is named, not
    // a folder holding it all.
    let (state, arrivals, heads) = (format!("{dir}/data/deployments.state"), format!("{dir}/data/deployments.arrivals"), format!("{dir}/data/deployments.heads"));
    let sealed = format!("{logs}/kept.sealed");
    for d in [&logs, &keys, &env, &state, &arrivals, &heads, &sealed] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(format!("{keys}/secrets.key"), "").unwrap();
    std::fs::write(format!("{env}/.env"), "").unwrap();
    let key = crate::sealing::KeySource::File(std::path::PathBuf::from(format!("{keys}/secrets.key")));
    let memory = ":memory:".to_string();
    let secrets = format!("{dir}/conf/secrets.json");
    let own = server_files(&[&deployments, &memory, &secrets], &deployments, &secrets, Some(&format!("{env}/.env")), &key);
    let folders = crate::folders::Folders::open(":memory:", &own).unwrap();
    let bind = |path: &str| folders.bind("x", path, crate::folders::FolderAccess::Read, crate::folders::Keep::Bytes, None, None);
    let bind_rw = |path: &str| folders.bind("y", path, crate::folders::FolderAccess::ReadWrite, crate::folders::Keep::Bytes, None, None);
    // The flow logs beside the deployments file (a log's sealed segments in them), its state heads,
    // arrivals and heads, the folder of the secrets key, the `.env`'s (and the secrets' audit).
    for path in [&logs, &sealed, &state, &arrivals, &heads, &keys, &env] {
        assert!(bind(path).unwrap_err().contains("own data"), "{path}");
        assert!(bind_rw(path).unwrap_err().contains("own data"), "read-write: {path}");
    }
    let elsewhere = format!("{dir}/inbox");
    std::fs::create_dir_all(&elsewhere).unwrap();
    bind(&elsewhere).unwrap();
}

#[test]
fn a_closure_too_big_to_run_is_refused_and_every_sample_is_well_inside() {
    let state = test_state();
    let mut store = state.store.clone();
    // Every sample's closure, nested well inside the bound: room for flows bigger than these.
    let mut deepest = (0, "");
    for sample in holon_author::samples::samples() {
        let plan = holon_author::agent::plan(sample.text).unwrap();
        for bytes in &plan.units {
            store.put(&decode_preimage(bytes).unwrap()).unwrap();
        }
        let root = parse_hash(&plan.unit).unwrap();
        closure(&state.store, root).unwrap();
        let mut refs = BTreeMap::new();
        let mut queue = vec![root];
        while let Some(h) = queue.pop() {
            if let Some(u) = store.get(&h).unwrap().filter(|_| !refs.contains_key(&h)) {
                queue.extend(child_refs(&u));
                refs.insert(h, child_refs(&u));
            }
        }
        deepest = deepest.max((crate::units::closure_depth(root, &refs), sample.name));
    }
    assert!(deepest.0 * 2 <= crate::units::MAX_CLOSURE_DEPTH, "{deepest:?}");

    // A chain nested one past the bound: refused, a 400 where a deploy asks.
    let mut below = store.put(&Unit { params: vec![], ..holon_core::interface_of("clock").unwrap() }).unwrap();
    let wrap = |child: Hash| Unit { body: UnitBody::Composite { children: vec![child], edges: vec![] }, params: vec![], inputs: vec![], outputs: vec![] };
    for _ in 0..crate::units::MAX_CLOSURE_DEPTH {
        below = store.put(&wrap(below)).unwrap();
    }
    let Err(e) = closure(&state.store, below) else { panic!("a closure past the bound is refused") };
    assert!(matches!(&e, crate::units::Closure::TooBig(why) if why.contains("more than 64 units deep")), "{e:?}");
    let deployed = route(&state, &req("POST", "/deploy", serde_json::to_vec(&serde_json::json!({ "slug": "deep", "unit": to_hex(&below) })).unwrap()));
    assert_eq!(deployed.status_code, 400);
    assert!(String::from_utf8_lossy(&body_bytes(deployed)).contains("too big to run"));
    // One level less is materialized.
    let inside = match store.get(&below).unwrap().unwrap().body { UnitBody::Composite { children, .. } => children[0], _ => unreachable!() };
    closure(&state.store, inside).unwrap();
}

/// A route nested as deep as a server runs (`MAX_CLOSURE_DEPTH`, the server and the leaf
/// counted) answers on a thread a runtime's size, 2 MiB, through the server's own resolver (its
/// kept children, its sourced values) and in a debug build, whose frames are larger than a
/// release's: composites in composites, and maps whose function maps again.
#[test]
fn the_deepest_route_a_server_runs_fits_a_runtimes_thread() {
    let run = |maps: bool| {
        let state = test_state();
        let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
        let any = |n: &str| Port { name: n.into(), ty: Type::Any };
        let mut below = stage(&state, &Unit { params: vec![], ..holon_core::interface_of("to_text").unwrap() });
        // The server, the endpoint and the leaf, and the levels between them up to the bound: a
        // composite each, or a composite and its map each.
        let levels = if maps { (crate::units::MAX_CLOSURE_DEPTH - 3) / 2 } else { crate::units::MAX_CLOSURE_DEPTH - 3 };
        for _ in 0..levels {
            below = if maps {
                let one = stage(&state, &Unit { inputs: vec![any("a")], ..holon_core::interface_of("list").unwrap() });
                let map = stage(&state, &Unit { params: vec![Param::frozen("fn", to_hex(&below))], ..holon_core::interface_of("map").unwrap() });
                stage(&state, &Unit { body: UnitBody::Composite { children: vec![one, map], edges: vec![e(BOUNDARY, 0, 0, 0), e(0, 0, 1, 0), e(1, 0, BOUNDARY, 0)] }, params: vec![], inputs: vec![any("x")], outputs: vec![any("out")] })
            } else {
                stage(&state, &Unit { body: UnitBody::Composite { children: vec![below], edges: vec![e(BOUNDARY, 0, 0, 0), e(0, 0, BOUNDARY, 0)] }, params: vec![], inputs: vec![any("x")], outputs: vec![any("out")] })
            };
        }
        let top = state.store.clone().get(&below).unwrap().unwrap();
        let method = Unit { params: vec![Param::frozen("name", "method")], ..holon_core::interface_of("field").unwrap() };
        let route_unit = endpoint(&state, "GET", "/deep", vec![method, top], vec![e(BOUNDARY, 0, 0, 0), e(0, 0, 1, 0), e(1, 0, BOUNDARY, 0)], 0);
        let server = stage(&state, &Unit { body: UnitBody::Composite { children: vec![route_unit], edges: vec![] }, params: vec![Param::frozen("declare", "server"), Param::frozen("port", "18144")], inputs: vec![], outputs: vec![] });
        let deployed = deploy_json(&state, "deep", server, serde_json::json!({ "acknowledge": ["NetworkIngress"] }));
        let status = deployed.status_code;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(deployed)));
        let (status, body, _) = data_call(&state, "deep", "GET", "/deep", "");
        assert_eq!(status, 200, "{body}");
        // `to_text` of the method is `"GET"`, quotes and all: the body as it is, or in JSON lists.
        let want = match maps {
            true => (0..levels).fold(r#""\"GET\"""#.to_string(), |v, _| format!("[{v}]")),
            false => r#""GET""#.to_string(),
        };
        assert_eq!(body.replace([' ', '\n'], ""), want);
    };
    for maps in [false, true] {
        std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(move || run(maps)).unwrap().join().unwrap();
    }
}

/// A flow nesting a list a level for each step of a range its request sizes took a release server
/// down at 10,000 steps: the recorder, keeping the fold's output, walked it a frame a level, past
/// the end of its thread's stack, and every deployment went with it. The fold now stops at the
/// bound (core's `MAX_VALUE_DEPTH`): a value as deep as that, a list or a record, is answered,
/// written out, recorded and read back from the log on a thread a runtime's size, in a debug
/// build; one level past it, or 10,000, is nothing, and the server answers its next request.
#[test]
fn a_value_nested_a_level_a_step_stops_at_the_bound_and_the_server_answers_on() {
    let text = r#"import { flow, range, route, status } from "holon"

flow("deep", { port: 18145 })

route.get("/deep/:n", (req) => range(Number(req.params.n)).reduce((acc, i) => [acc], 0) ?? status(422))

route.get("/text/:n", (req) => JSON.stringify(range(Number(req.params.n)).reduce((acc, i) => [acc], 0)) ?? status(422))

route.get("/record/:n", (req) => range(Number(req.params.n)).reduce((acc, i) => ({ a: acc }), 0) ?? status(422))
"#;
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let state = test_state();
            let plan = holon_author::agent::plan(text).unwrap();
            for u in &plan.units {
                assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
            }
            let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
            let deployed = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": preview["needs_approval"] }));
            assert_eq!(deployed.status_code, 200);
            let slug = plan.name;
            // A route's answer goes a level down itself, into its response (`{status, body}`): the
            // deepest it answers is one less than the bound; written out as a text, the bound.
            let max = holon_core::MAX_VALUE_DEPTH;
            for (route, at, open, close) in [("deep", max - 1, "[", "]"), ("text", max, "[", "]"), ("record", max - 1, "{\"a\":", "}")] {
                let want = format!("{}0{}", open.repeat(at), close.repeat(at));
                let (status, body, _) = data_call(&state, &slug, "GET", &format!("/{route}/{at}"), "");
                assert_eq!(status, 200, "/{route}/{at}: {body}");
                // A list or a record is answered as its JSON; a text as it is.
                assert_eq!(body.replace([' ', '\n'], ""), want, "/{route}/{at}");
                for n in [at + 1, 10_000] {
                    let (status, body, _) = data_call(&state, &slug, "GET", &format!("/{route}/{n}"), "");
                    assert_eq!(status, 422, "/{route}/{n}: {body}");
                }
            }
            let entries = log_entries(&state, &slug);
            assert!(entries.len() >= 9, "{} runs logged", entries.len());
        })
        .unwrap()
        .join()
        .unwrap();
}

/// **Guards:** whatever unit is staged, loading its closure answers: the closure, or why not. A
/// unit core's `validate` refuses is refused exactly, by itself and nested in another, before
/// anything is made of it; one it takes loads, a child nobody staged left missing.
///
/// **Why:** `POST /unit` only decodes what it stores, and an agent may call it: a unit's bytes
/// are whoever's sent them. The evaluator is written for wiring that validates, and so is what is
/// laid out for it (a shape, by the ports its edges name), so the loader is the one gate between
/// any bytes and both.
///
/// **If it fails:** a unit that doesn't validate runs (two sources for one port, a cycle, a port
/// its child doesn't have: what it computes is anyone's guess), or loading one ends the server:
/// laid out before it was validated, an edge to port four billion asked for a list of 51 GiB.
///
/// Units are drawn, a few thousand: children staged or not, edges to and from ports that exist,
/// don't, or are far out, to one port twice, round in a cycle, an effect into a sink's data. Each
/// is loaded as a root and nested, and what the loader says is what `validate` says of it; a
/// deploy of one refused is told so, and the server answers on. The kinds of mistake are counted,
/// so the drawing is known to make each.
#[test]
fn a_staged_unit_loads_or_is_refused_whatever_its_wiring() {
    struct Rng(u64);
    impl Rng {
        fn below(&mut self, m: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % m
        }
    }
    let state = test_state();
    let mut store = state.store.clone();
    // What a drawn unit's children are: leaves staged here, beside one nobody staged. The same
    // units are core's to resolve, to say what it makes of a wiring. Two are sinks, whose output
    // is an effect and whose input takes anything but one (a write that stored an effect once
    // panicked the next run under the state lock, and every request after it).
    let mut known = holon_core::MapResolver::new();
    let mut leaves: Vec<Hash> = Vec::new();
    for name in ["clock", "random", "not", "and", "add", "concat", "equals", "store_write", "egress"] {
        let interface = holon_core::interface_of(name).unwrap();
        let unit = if name == "store_write" { interface } else { Unit { params: vec![], ..interface } };
        leaves.push(store.put(&unit).unwrap());
        known.insert(unit).unwrap();
    }
    let nobody: Hash = [0xee; 32];
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let port = |rng: &mut Rng| match rng.below(20) {
        0 => u32::MAX,
        1 => u32::MAX - 1,
        2 => 4_000_000_000,
        v => (v % 3) as u32,
    };
    let types = [Type::Number, Type::Boolean, Type::Text, Type::Any, Type::Effect];
    let (mut refused, mut loaded, mut kinds) = (0, 0, BTreeMap::<String, usize>::new());
    for case in 0..3_000 {
        let n = 1 + rng.below(4) as usize;
        let children: Vec<Hash> = (0..n).map(|_| if rng.below(12) == 0 { nobody } else { leaves[rng.below(leaves.len() as u64) as usize] }).collect();
        let end = |rng: &mut Rng| match rng.below(n as u64 + 2) {
            0 => BOUNDARY,
            v => (v - 1) as u32,
        };
        let edges: Vec<Edge> = (0..rng.below(7)).map(|_| Edge { from_child: end(&mut rng), from_port: port(&mut rng), to_child: end(&mut rng), to_port: port(&mut rng) }).collect();
        let ports = |rng: &mut Rng, name: &str| (0..rng.below(3)).map(|i| Port { name: format!("{name}{i}"), ty: types[rng.below(types.len() as u64) as usize].clone() }).collect::<Vec<_>>();
        let unit = Unit { body: UnitBody::Composite { children, edges }, params: vec![], inputs: ports(&mut rng, "in"), outputs: ports(&mut rng, "out") };
        // What core makes of it: a child that isn't staged is nothing to refuse it for.
        let found: Vec<String> = holon_core::validate(&unit, &known).err().unwrap_or_default().iter().map(|e| format!("{e:?}")).collect();
        let mistakes: Vec<String> = found.iter().map(|e| e.split([' ', '(', '{']).next().unwrap().to_string()).filter(|e| e != "UnresolvedChild").collect();
        for kind in &mistakes {
            *kinds.entry(kind.clone()).or_default() += 1;
        }
        // An effect into a sink: its port takes anything else.
        *kinds.entry("EffectIntoSink".into()).or_default() += found.iter().filter(|e| e.contains("src: Effect, dst: Any")).count();
        // A unit with no one encoding (two children alike that no edge tells apart) is no unit: it
        // has no hash to be staged under.
        let Ok(staged) = store.put(&unit) else { continue };
        let nested = store.put(&Unit { body: UnitBody::Composite { children: vec![staged], edges: vec![] }, params: vec![], inputs: vec![], outputs: vec![] }).unwrap();
        for root in [staged, nested] {
            match (closure(&state.store, root), mistakes.is_empty()) {
                (Ok(held), true) => assert!(held.resolve(&staged).is_some(), "case {case}: it holds the unit"),
                (Err(crate::units::Closure::Invalid(why)), false) => assert!(why.contains(&to_hex(&staged)) && why.contains("doesn't validate"), "case {case}: {why}"),
                (Ok(_), false) => panic!("case {case}: loaded, though it doesn't validate ({mistakes:?}): {unit:?}"),
                (Err(e), _) => panic!("case {case}: {e:?} for a unit with {mistakes:?}: {unit:?}"),
            }
        }
        if mistakes.is_empty() {
            loaded += 1;
            continue;
        }
        refused += 1;
        // A deploy of one is told so, and the server answers on.
        if refused % 200 == 1 {
            let deployed = route(&state, &req("POST", "/deploy", serde_json::to_vec(&serde_json::json!({ "slug": "unwired", "unit": to_hex(&nested) })).unwrap()));
            assert_eq!(deployed.status_code, 400);
            assert!(String::from_utf8_lossy(&body_bytes(deployed)).contains("can't run"));
        }
    }
    assert!(refused > 1_000 && loaded > 300, "{refused} refused, {loaded} loaded");
    for kind in ["ChildOutOfRange", "OutputPortOutOfRange", "InputPortOutOfRange", "BoundaryInputOutOfRange", "BoundaryOutputOutOfRange", "DuplicateInputEdge", "Cycle", "TypeMismatch", "EffectIntoSink"] {
        assert!(kinds.get(kind).is_some_and(|n| *n >= 10), "the drawing makes {kind} too seldom: {kinds:?}");
    }
    assert_eq!(route(&state, &req("GET", "/deployments", vec![])).status_code, 200, "the server answers on");
    // A leaf alone, its closure loaded from the store, and again from memory.
    closure(&state.store, leaves[0]).unwrap();
    closure(&state.store, leaves[0]).unwrap();
}

/// An action's answer names its own run's entry, however many runs of its deployment log at once
/// (the seq comes from the log as the entry is written, not from a read before it).
#[test]
fn an_actions_reported_entry_is_its_own_under_concurrent_runs() {
    let state = test_state();
    let slug = deploy_plain_text(&state, "import { action, flow } from \"holon\"\n\nflow(\"echoer\")\n\nexport const echo = action((input) => ({ id: input.id }))\n");
    let answered: Vec<(String, u64)> = thread::scope(|s| {
        let workers: Vec<_> = (0..8)
            .map(|t| {
                let (state, slug) = (&state, &slug);
                s.spawn(move || {
                    (0..25)
                        .map(|i| {
                            let id = format!("t{t}-{i}");
                            let body = serde_json::to_vec(&serde_json::json!({ "id": id })).unwrap();
                            let r = body_json(route(state, &req("POST", &format!("/trigger/{slug}/echo"), body)));
                            assert_eq!(r["answer"]["id"], id.as_str(), "{r}");
                            (id, r["seq"].as_u64().unwrap())
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
    });
    let log = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/log?limit=1000"), vec![])));
    let by_seq: BTreeMap<u64, String> =
        log["entries"].as_array().unwrap().iter().map(|e| (e["seq"].as_u64().unwrap(), e["response_body"].as_str().unwrap_or_default().to_string())).collect();
    assert_eq!(by_seq.len(), 200, "every run logged once");
    for (id, seq) in answered {
        assert!(by_seq[&seq].contains(&id), "run {seq} answered {id:?}, and its entry says {:?}", by_seq[&seq]);
    }
}

/// One deployment's log being written (or cut back) holds up no other deployment's runs: each
/// log has its own lock, and the table is never held while a log is written.
#[test]
fn one_deployments_log_doesnt_hold_up_another() {
    let state = test_state();
    let busy = deploy_plain_text(&state, include_str!("../../holon-author/samples/calc.holon.ts"));
    let other = deploy_plain_text(&state, include_str!("../../holon-author/samples/hello-api.holon.ts"));
    let held = Arc::clone(&state.deployments.lock().unwrap()[&busy].log);
    let (holding, held_now) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    thread::scope(|s| {
        // Held until the other's request is answered, GATE_HOLDS at most: whether the test let
        // it go, not the time (a loaded machine takes as long as it takes, not 400 ms).
        let holder = s.spawn(move || {
            held.with(|_| {
                holding.send(()).unwrap();
                released.recv_timeout(GATE_HOLDS).is_ok()
            })
        });
        held_now.recv().unwrap();
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, &other, &fake_data("GET", "/hello/Ada", vec![]));
        let _ = release.send(());
        assert_eq!(r.status, 200);
        assert!(holder.join().unwrap(), "{other} was answered only once {busy}'s log was let go");
    });
}

/// A run is recorded strictly unless its handler says otherwise (docs/L4s/request-path-L4.md,
/// C2 e and C5): a strict request is answered once its entry is on disk, and a lazy one, a lazy
/// action and a request no route took are left to the syncer, which syncs them within a second,
/// with the deployment's events. Strict runs at once share syncs: none takes more than one.
/// (What a run is told when a write or a sync fails is recorder.rs's model's.)
#[test]
fn a_strict_run_is_on_disk_before_its_answer_and_a_lazy_one_waits_for_the_syncer() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("strict-sync").into();
    let slug = deploy_plain_text(
        &state,
        "import { action, flow, route } from \"holon\"\nimport notes from \"collection:notes\"\nimport docs from \"folder:docs\"\n\nflow(\"board\", { port: 8170 })\n\nroute.get(\"/health\", () => \"ok\")\n\nroute.get(\"/notes\", () => notes.list({ limit: 50 }), { log: \"lazy\" })\n\nroute.post(\"/notes\", (req) => {\n  notes[req.json.id] = req.json.text\n  return \"saved\"\n})\n\nexport const report = action(() => notes.list({ limit: 50 }), { log: \"lazy\" })\n\nexport const count = action(() => notes.list({ limit: 50 }))\n",
    );
    let log = Arc::clone(&state.deployments.lock().unwrap()[&slug].log);
    let call = |method: &str, url: &str, body: &str| serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &fake_data(method, url, body.as_bytes().to_vec())).status;
    let synced = || log.with(|l| l.take_dirty().inspect(|_| l.still_dirty())).is_none();
    let events_synced = || {
        let mut table = state.deployments.lock().unwrap();
        let events = &mut table.get_mut(&slug).unwrap().events;
        events.take_dirty().inspect(|_| events.still_dirty()).is_none()
    };
    let action = |name: &str| run_action(&state.store, &state.deployments, &state.deployments_path, &slug, name, Ok(Value::Record(BTreeMap::new())), None, None, &[]).status;

    // Strict: a read, and a write.
    assert_eq!(call("GET", "/health", ""), 200);
    assert_eq!((log.syncs(), synced()), (1, true), "a strict read is on disk when it answers");
    assert_eq!(call("POST", "/notes", r#"{"id":"a","text":"hi"}"#), 200);
    assert_eq!((log.syncs(), synced()), (2, true), "and a write");
    assert_eq!(action("count"), 200);
    assert_eq!((log.syncs(), synced()), (3, true), "and an action");

    // Lazy: answered before its entry is on disk; the syncer's next tick syncs it.
    assert_eq!(call("GET", "/notes", ""), 200);
    assert_eq!((log.syncs(), synced()), (3, false), "a lazy run is answered without a sync");
    assert_eq!(action("report"), 200);
    assert_eq!(call("GET", "/nowhere", ""), 404);
    assert_eq!(call("DELETE", "/health", ""), 405);
    assert_eq!((log.syncs(), synced()), (3, false), "a lazy action, and requests no route took, likewise");
    assert!(!events_synced(), "its events (its deploy's) are the syncer's too");
    assert_eq!(recorder::sync_logs(&state.deployments), Ok(()));
    assert_eq!((log.syncs(), synced()), (4, true), "one sync for all four");
    assert!(events_synced(), "and its events with them");
    assert_eq!(log.next_seq(), 7, "every run is in the log");

    // Many strict runs at once: each on disk when it answers, never more than one sync each.
    let before = log.syncs();
    std::thread::scope(|s| {
        for _ in 0..32 {
            s.spawn(|| assert_eq!(call("GET", "/health", ""), 200));
        }
    });
    let syncs = log.syncs() - before;
    assert!((1..=32).contains(&syncs), "{syncs} syncs for 32 runs");
    assert!(synced());
    eprintln!("32 strict runs at once: {syncs} syncs");
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// A board with an unrecorded health check, an unrecorded action, one that fails unrecorded, and a
/// strict write (docs/L4s/request-path-L4.md, C5).
const UNRECORDED_BOARD: &str = "import { action, flow, route, status } from \"holon\"\nimport notes from \"collection:notes\"\nimport docs from \"folder:docs\"\n\nflow(\"board\", { port: 8171 })\n\nroute.get(\"/health\", () => \"ok\", { log: \"unrecorded\" })\n\nroute.get(\"/broken\", () => status(503, \"down\"), { log: \"unrecorded\" })\n\nroute.post(\"/notes\", (req) => {\n  notes[req.json.id] = req.json.text\n  return \"saved\"\n})\n\nexport const report = action(() => notes.list({ limit: 50 }), { log: \"unrecorded\" })\n";

/// An unrecorded handler's runs leave no entry in the log, failed or not, and are counted in its
/// tally instead (runs, failures, time, the last one, the last failure's time, status and error),
/// which the log API gives and the deployments file keeps (resilience-audit-L4.md, §2.8). Each
/// unrecorded handler is an item a person approves, `Unrecorded("GET /health")`, worded in the
/// approval; taking it back records its next run, and the deployment runs on (it isn't one it
/// needs).
#[test]
fn an_unrecorded_run_leaves_no_entry_failed_or_not_and_is_counted() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("unrecorded").into();
    let plan = holon_author::agent::plan(UNRECORDED_BOARD).unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap();
    for label in ["GET /health", "GET /broken", "action report"] {
        assert!(asked.contains(&format!("Unrecorded({label:?})")), "{label} is asked for: {asked:?}");
    }
    assert!(!asked.iter().any(|a| a.contains("POST /notes")), "a strict handler isn't asked for: {asked:?}");
    let slug = deploy_plain_text(&state, UNRECORDED_BOARD);
    let call = |method: &str, url: &str, body: &str| serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &fake_data(method, url, body.as_bytes().to_vec())).status;
    let log = Arc::clone(&state.deployments.lock().unwrap()[&slug].log);
    let tally = |label: &str| state.deployments.lock().unwrap()[&slug].tallies.get(label).cloned().unwrap_or_default();

    for _ in 0..3 {
        assert_eq!(call("GET", "/health", ""), 200);
    }
    assert_eq!(log.next_seq(), 0, "no entry for an unrecorded run");
    assert_eq!((tally("GET /health").runs, tally("GET /health").failed), (3, 0));
    assert!(state.deployments.lock().unwrap()[&slug].tallies_changed, "for the periodic sync to write");
    assert!(tally("GET /health").last_at_ms > 0);
    let ran = run_action(&state.store, &state.deployments, &state.deployments_path, &slug, "report", Ok(Value::Record(BTreeMap::new())), None, None, &[]);
    assert_eq!((ran.status, ran.seq), (200, None), "an unrecorded action answers, with no entry");
    assert_eq!(tally("action report").runs, 1);

    // A failure is counted, not logged: when, and its status.
    assert_eq!(call("GET", "/broken", ""), 503);
    assert_eq!(log.next_seq(), 0, "a failed unrecorded run isn't logged either");
    let broken = tally("GET /broken");
    assert_eq!((broken.runs, broken.failed), (1, 1));
    let last = broken.last_failure.expect("the last failure kept");
    assert_eq!((last.status, last.error.as_deref()), (503, None));
    assert!(last.at_ms > 0);
    // A strict handler beside them is recorded as always.
    assert_eq!(call("POST", "/notes", r#"{"id":"a","text":"hi"}"#), 200);
    assert_eq!(log.next_seq(), 1);

    // The log API gives the tallies, and the deployments file keeps them.
    let page = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/log"), vec![])));
    assert_eq!(page["tallies"]["GET /health"]["runs"], 3, "{page}");
    // And how many times the log was synced (holon load's syncs); limit=0, the counts alone.
    assert_eq!(page["syncs"], log.syncs(), "{page}");
    let counts = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/log?limit=0"), vec![])));
    assert_eq!((counts["entries"].as_array().map(Vec::len), &counts["next"]), (Some(0), &serde_json::json!(1)), "{counts}");
    // The listing names each route with how its runs are recorded, as granted.
    let levels = |state: &AppState| {
        let listed = body_json(route(state, &req("GET", "/deployments", vec![])));
        let dep = listed["deployments"].as_array().unwrap().iter().find(|d| d["slug"] == slug.as_str()).cloned().unwrap();
        let mut levels = dep["routes"].as_array().unwrap().iter().map(|r| format!("{} {} {}", r["method"].as_str().unwrap(), r["path"].as_str().unwrap(), r["log"].as_str().unwrap())).collect::<Vec<_>>();
        levels.sort();
        levels
    };
    assert_eq!(levels(&state), ["GET /broken unrecorded", "GET /health unrecorded", "POST /notes strict"]);
    persist_deployments(&state.deployments_path, &state.deployments);
    let kept: serde_json::Value = serde_json::from_slice(&std::fs::read(state.deployments_path.at().unwrap()).unwrap()).unwrap();
    let record = kept.as_array().unwrap().iter().find(|r| r["slug"] == slug.as_str()).unwrap();
    assert_eq!(record["tallies"]["GET /health"]["runs"], 3, "{record}");

    // Taken back, the next run is recorded (strict), and the deployment runs on.
    let revoked = body_json(change_grant(&state, &slug, serde_json::json!({ "revoke": ["Unrecorded(\"GET /health\")"] })));
    assert_eq!(revoked["status"], "running", "{revoked}");
    let syncs = log.syncs();
    assert_eq!(call("GET", "/health", ""), 200);
    assert_eq!((log.next_seq(), log.syncs()), (2, syncs + 1), "recorded, and strictly");
    assert_eq!(tally("GET /health").runs, 3, "and not counted as unrecorded");
    assert_eq!(levels(&state)[1], "GET /health strict", "the listing says so");
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// What an unrecorded run's tally keeps of a failure: its error redacted as its entry would be, of
/// the deployment's secrets and of the run's own values (its request's tainted values, its resume
/// tokens), and cut short, its status at least 500 when it failed with an error; and a failed
/// unrecorded action answers that its runs aren't logged (resilience-audit-L4.md, §2.8).
#[test]
fn an_unrecorded_failure_is_kept_redacted_and_cut_short() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("unrecorded-failure").into();
    let slug = deploy_plain_text(&state, UNRECORDED_BOARD);
    let set = serde_json::to_vec(&serde_json::json!({ "value": "sk_live_42" })).unwrap();
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/secrets/api_key"), set)).status_code, 200);
    // A file named from the request, say, or a key a run lock held: what it failed on can hold
    // what it was sent.
    let long = format!("the key sk_live_42 was refused for docs/4111111111111111.txt by rt-5e9c1a7b3d: {}", "x".repeat(1000));
    let counted = crate::firing::Counted { route: Some("GET /health"), at_ms: 7, duration_ms: 1.0, status: 200, error: Some(&long) };
    let run = secrets::Redactor::tainted(&[("number".to_string(), "4111111111111111".to_string())])
        .and(secrets::Redactor::tokens(&BTreeMap::from([("paid".to_string(), "rt-5e9c1a7b3d".to_string())])));
    let kept = {
        let mut table = state.deployments.lock().unwrap();
        let dep = table.get_mut(&slug).unwrap();
        assert!(crate::firing::count_for_tests(dep, &counted, &run), "counted only");
        dep.tallies["GET /health"].last_failure.clone().unwrap()
    };
    let error = kept.error.unwrap();
    assert_eq!(kept.status, 500, "an error is a failure, answered 500");
    for (value, mark) in [("sk_live_42", "‹secret:api_key"), ("4111111111111111", "‹taint:number›"), ("rt-5e9c1a7b3d", "‹token:paid›")] {
        assert!(!error.contains(value) && error.contains(mark), "{error}");
    }
    assert!(error.chars().count() <= 301 && error.ends_with('…'), "cut short: {} chars", error.chars().count());
    // An unrecorded action is counted and never logged; one with no answer is answered 404
    // (missing), which isn't a failure.
    let failing = deploy_plain_text(&state, "import { action, flow } from \"holon\"\n\nflow(\"failing\", { port: 8173 })\n\nexport const check = action((input) => input.nowhere, { log: \"unrecorded\" })\n");
    let log = Arc::clone(&state.deployments.lock().unwrap()[&failing].log);
    let ran = run_action(&state.store, &state.deployments, &state.deployments_path, &failing, "check", Ok(Value::Record(BTreeMap::new())), None, None, &[]);
    assert_eq!((ran.status, ran.seq), (404, None), "no answer: missing, with no entry: {:?}", ran.error);
    assert_eq!(log.next_seq(), 0, "nothing logged");
    let tally = state.deployments.lock().unwrap()[&failing].tallies["action check"].clone();
    assert_eq!((tally.runs, tally.failed, tally.last_failure), (1, 0, None));
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// A handler that waits is as its author declares it (C5): a parked run keeps what it waits with
/// (its rounds, its events) in its own record, whatever its level. An unrecorded one, granted,
/// parks and resumes with no entry for either part, each counted; a lazy one's parts are logged
/// as lazy, without a sync before they are done. Its write lands either way.
#[test]
fn a_run_that_waits_parks_and_resumes_at_its_handlers_level() {
    let state = test_state_at(unique_temp_path("waits-light").into());
    let text = |level: &str, port: u16| {
        format!(
            r#"import {{ action, event, flow, route }} from "holon"
import seen from "collection:seen"

flow("hold-{level}", {{ port: {port} }})

route.post("/hold", (req) => {{
  const started = hold(req.method)
  return {{ token: started.waiting.paid }}
}}, {{ log: "{level}" }})

export const hold = action(async (asked) => {{
  const paid = await event("paid", {{ expires: "1h" }})
  seen["last"] = paid.method
}}, {{ log: "{level}" }})

route.get("/paid/:token", () => "thanks", {{ resumes: "paid" }})

route.get("/seen", () => ({{ last: seen["last"] }}))
"#
        )
    };
    for (level, port) in [("unrecorded", 18201), ("lazy", 18202)] {
        let slug = deploy_plain_text(&state, &text(level, port));
        let log = Arc::clone(&state.deployments.lock().unwrap()[&slug].log);
        let (status, held, _) = data_call(&state, &slug, "POST", "/hold", "");
        assert_eq!(status, 200, "{level}: {held}");
        let token = serde_json::from_str::<serde_json::Value>(&held).unwrap()["token"].as_str().unwrap().to_string();
        let parked = state.deployments.lock().unwrap()[&slug].parked.values().next().cloned().unwrap_or_else(|| panic!("{level}: parked"));
        assert_eq!(parked.log, level, "the parked run keeps its handler's level");
        let syncs = log.syncs();
        assert_eq!(data_call(&state, &slug, "GET", &format!("/paid/{token}"), "").0, 200, "{level}");
        assert!(state.deployments.lock().unwrap()[&slug].parked.is_empty(), "{level}: resumed, and done");
        let seen = data_call(&state, &slug, "GET", "/seen", "").1;
        assert_eq!(serde_json::from_str::<serde_json::Value>(&seen).unwrap()["last"], "GET", "{level}: its write landed");
        let routes: Vec<String> = log_entries(&state, &slug).iter().map(|e| format!("{} {}", e["kind"].as_str().unwrap(), e["route"].as_str().unwrap_or(""))).collect();
        let tally = |label: &str| state.deployments.lock().unwrap()[&slug].tallies.get(label).map_or(0, |t| t.runs);
        let counted = (tally("POST /hold"), tally("action hold"));
        if level == "unrecorded" {
            assert_eq!(routes, ["request GET /paid/:token", "request GET /seen"], "neither part logged");
            assert_eq!(counted, (1, 2), "the request counted, and each part of the run it started");
        } else {
            assert_eq!(routes, ["trigger action hold", "request POST /hold", "request GET /paid/:token", "resume action hold", "request GET /seen"]);
            assert_eq!(counted, (0, 0), "logged, not counted");
            // The resume route's request is strict, the resumed part lazy: one sync between them.
            assert_eq!(log.syncs(), syncs + 2, "the resume request's sync, and the strict /seen's; not the lazy part's");
        }
    }
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// An action run that parks hasn't answered, and hasn't failed: its entry says where it waits,
/// with no status, and neither the deployment's runs nor an unrecorded action's tally count it as
/// failed, whatever its level. One that can't park (past `max_waiting_runs`) still fails, 500.
#[test]
fn a_parked_action_run_waits_and_isnt_counted_as_failed() {
    let mut state = claims_state("[limits]\nmax_waiting_runs = 1\n");
    state.deployments_path = unique_temp_path("parked-not-failed").into();
    let text = |level: &str, port: u16| {
        format!(
            r#"import {{ action, event, flow, route }} from "holon"

flow("parks-{level}", {{ port: {port} }})

export const hold = action(async () => {{
  const paid = await event("paid", {{ expires: "1h" }})
  return paid.method
}}, {{ log: "{level}" }})

route.get("/paid/:token", () => "thanks", {{ resumes: "paid" }})
"#
        )
    };
    for (level, port) in [("strict", 18206), ("lazy", 18207), ("unrecorded", 18208)] {
        let slug = deploy_plain_text(&state, &text(level, port));
        let hold = || run_action(&state.store, &state.deployments, &state.deployments_path, &slug, "hold", Ok(Value::Record(BTreeMap::new())), None, None, &[]);
        let held = hold();
        assert_eq!((held.status, held.waiting, held.error.as_deref()), (202, true, None), "{level}: {}", held.reply);
        let refused = hold();
        assert_eq!((refused.status, refused.waiting), (500, false), "{level}: one waits already, as many as may: {}", refused.reply);
        let (runs, failed) = {
            let table = state.deployments.lock().unwrap();
            (table[&slug].trigger_count, table[&slug].error_count)
        };
        assert_eq!((runs, failed), (2, 1), "{level}: both ran, only the one that couldn't park failed");
        if level == "unrecorded" {
            let tally = state.deployments.lock().unwrap()[&slug].tallies["action hold"].clone();
            assert_eq!((tally.runs, tally.failed), (2, 1), "counted, the parked one not as failed");
            assert!(log_entries(&state, &slug).is_empty(), "neither logged");
        } else {
            let entries: Vec<serde_json::Value> = log_entries(&state, &slug).into_iter().filter(|e| e["route"] == "action hold").collect();
            let [parked, failed] = &entries[..] else { panic!("{level}: both logged: {entries:?}") };
            assert_eq!((&parked["parked"], &parked["status"]), (&serde_json::json!(["paid"]), &serde_json::Value::Null), "{level}: waiting, with no status: {parked}");
            assert_eq!(failed["status"], 500, "{level}: {failed}");
            assert!(failed["error"].as_str().is_some_and(|e| e.contains("max_waiting_runs")), "{level}: {failed}");
        }
    }
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// What a waiting run sent as it parked is kept with its record, as a logged run's is in its log:
/// after a restart, an unrecorded run's later part doesn't send it again.
#[test]
fn a_waiting_unrecorded_run_doesnt_send_again_after_a_restart() {
    let state = test_state_at(unique_temp_path("waits-sent").into());
    let text = "import { action, event, flow, route } from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"hold-sends\", { port: 18203 })\n\nroute.post(\"/hold\", (req) => {\n  const started = hold(req.method)\n  return { token: started.waiting.paid }\n}, { log: \"unrecorded\" })\n\nexport const hold = action(async (asked) => {\n  updates.send({ held: asked })\n  const paid = await event(\"paid\", { expires: \"1h\" })\n  updates.send({ paid: paid.method })\n}, { log: \"unrecorded\" })\n\nroute.get(\"/paid/:token\", () => \"thanks\", { resumes: \"paid\" })\n";
    let slug = deploy_plain_text(&state, text);
    let subscribe = |state: &AppState| {
        let table = state.deployments.lock().unwrap();
        table[&slug].channels.subscribe("updates", false, None, &table[&slug].permit.channels).unwrap()
    };
    let got = |sub: &mut channels::Subscription| std::iter::from_fn(|| sub.packs.try_recv().ok()).map(|p| p.json.clone()).collect::<Vec<_>>();
    let mut before = subscribe(&state);
    let (status, held, _) = data_call(&state, &slug, "POST", "/hold", "");
    assert_eq!(status, 200, "{held}");
    assert_eq!(got(&mut before), [r#"{"held":"POST"}"#]);
    let token = serde_json::from_str::<serde_json::Value>(&held).unwrap()["token"].as_str().unwrap().to_string();

    // A restart: the run loaded with what it sent, opened with its deployment's vault.
    let mut reloaded = load_deployments(&state.deployments_path, &state.store, &state.policy()).unwrap();
    {
        let dep = reloaded.get_mut(&slug).unwrap();
        dep.secrets = state.secrets.vault(&slug);
        let sealer = dep.secrets.lock().unwrap().sealer.clone();
        let run = dep.parked.values_mut().next().unwrap();
        assert_eq!(run.sent, [("updates".to_string(), r#"{"held":"POST"}"#.to_string())]);
        run.open(&slug, sealer.as_deref()).unwrap();
    }
    *state.deployments.lock().unwrap() = reloaded;
    let mut after = subscribe(&state);
    assert_eq!(data_call(&state, &slug, "GET", &format!("/paid/{token}"), "").0, 200);
    assert!(state.deployments.lock().unwrap()[&slug].parked.is_empty(), "resumed, and done");
    assert_eq!(got(&mut after), [r#"{"paid":"GET"}"#], "what it sent as it parked isn't sent again");
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// A logged run's packs go out once its record is on disk: a lazy run's with the next sync, a
/// strict one's, with those of the runs before it, once its own is done, in the log's order. So a
/// pack a subscriber got is in the log after a power loss too, and its id is never given again.
#[test]
fn a_runs_packs_go_out_once_its_record_is_on_disk() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("packs-on-disk").into();
    let text = "import { flow, route } from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"sends-lightly\", { port: 18204 })\n\nroute.post(\"/fast\", (req) => {\n  updates.send({ fast: req.method })\n  return \"ok\"\n}, { log: \"lazy\" })\n\nroute.post(\"/sure\", (req) => {\n  updates.send({ sure: req.method })\n  return \"ok\"\n})\n";
    let slug = deploy_plain_text(&state, text);
    let mut sub = {
        let table = state.deployments.lock().unwrap();
        table[&slug].channels.subscribe("updates", false, None, &table[&slug].permit.channels).unwrap()
    };
    let mut got = || std::iter::from_fn(|| sub.packs.try_recv().ok()).map(|p| (p.id.unwrap().to_string(), p.json.clone())).collect::<Vec<_>>();
    let fast = |seq: u64| (format!("{seq}.0"), r#"{"fast":"POST"}"#.to_string());
    let sure = |seq: u64| (format!("{seq}.0"), r#"{"sure":"POST"}"#.to_string());
    let next = || state.deployments.lock().unwrap()[&slug].log.next_seq();

    let first = next();
    assert_eq!(data_call(&state, &slug, "POST", "/fast", "").0, 200);
    assert_eq!(got(), [], "its record isn't on disk yet");
    recorder::sync_logs(&state.deployments).unwrap();
    assert_eq!(got(), [fast(first)]);

    let (lazy, strict) = (next(), next() + 1);
    assert_eq!(data_call(&state, &slug, "POST", "/fast", "").0, 200);
    assert_eq!(data_call(&state, &slug, "POST", "/sure", "").0, 200);
    assert_eq!(got(), [fast(lazy), sure(strict)], "the strict run's sync covers the lazy run before it");
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// An unrecorded handler that writes and sends is what its author wrote: approved as any
/// unrecorded one is (`Unrecorded("POST /notes")`), its runs leave no entry, what they write lands
/// all the same, and its packs go to the subscribers there then, live, with no id, and aren't kept
/// (the log, which ids and kept packs come from, doesn't hold them).
#[test]
fn an_unrecorded_handler_that_writes_and_sends_runs_as_written() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("unrecorded-writes").into();
    let text = "import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\nimport updates from \"channel:updates\"\n\nflow(\"quiet-notes\", { port: 8174 })\n\nroute.post(\"/notes\", (req) => {\n  notes[req.json.id] = req.json.text\n  updates.send({ id: req.json.id })\n  return \"saved\"\n}, { log: \"unrecorded\" })\n\nroute.get(\"/notes/:id\", (req) => ({ text: notes[req.params.id] }))\n";
    let plan = holon_author::agent::plan(text).unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap();
    assert!(asked.contains(&"Unrecorded(\"POST /notes\")".to_string()), "{asked:?}");
    let slug = deploy_plain_text(&state, text);
    let (log, channels, limits) = {
        let table = state.deployments.lock().unwrap();
        (Arc::clone(&table[&slug].log), Arc::clone(&table[&slug].channels), table[&slug].permit.channels.clone())
    };
    let mut sub = channels.subscribe("updates", false, None, &limits).unwrap();
    for id in ["a", "b"] {
        assert_eq!(data_call(&state, &slug, "POST", "/notes", &format!(r#"{{"id":"{id}","text":"hi"}}"#)).0, 200);
    }
    assert_eq!(log.next_seq(), 0, "its runs left no entry");
    assert_eq!(state.deployments.lock().unwrap()[&slug].tallies["POST /notes"].runs, 2, "they are counted");
    let live: Vec<(Option<channels::PackId>, String)> = std::iter::from_fn(|| sub.packs.try_recv().ok()).map(|p| (p.id, p.json.clone())).collect();
    assert_eq!(live, [(None, r#"{"id":"a"}"#.to_string()), (None, r#"{"id":"b"}"#.to_string())], "sent live, with no id");
    assert!(channels.kept("updates", true).is_empty(), "and not kept");
    let (_, read, _) = data_call(&state, &slug, "GET", "/notes/a", "");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&read).unwrap()["text"], "hi", "its write landed");
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// What an unrecorded handler that writes leaves out is said as such: `holon verify` takes a state
/// it may have moved for unchecked, not a break, and a restore's dry run says its runs, which
/// send, aren't counted in the runs since.
#[test]
fn what_an_unrecorded_writer_leaves_out_is_unchecked_not_a_break() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("unrecorded-verify").into();
    let text = "import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\nimport updates from \"channel:updates\"\n\nflow(\"light-verify\", { port: 8176 })\n\nroute.post(\"/notes\", (req) => {\n  notes[req.json.id] = req.json.text\n  return \"saved\"\n})\n\nroute.post(\"/quiet\", (req) => {\n  notes[req.json.id] = req.json.text\n  updates.send({ id: req.json.id })\n  return \"saved\"\n}, { log: \"unrecorded\" })\n\nroute.get(\"/notes/:id\", (req) => ({ text: notes[req.params.id] }))\n";
    let slug = deploy_plain_text(&state, text);
    assert_eq!(data_call(&state, &slug, "POST", "/notes", r#"{"id":"a","text":"hi"}"#).0, 200);
    assert_eq!(data_call(&state, &slug, "POST", "/quiet", r#"{"id":"b","text":"shh"}"#).0, 200);
    assert_eq!(data_call(&state, &slug, "GET", "/notes/a", "").0, 200);
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    assert_eq!(export.lines.len(), 2, "the unrecorded run isn't in it");
    let report = verify::check(&export, None, &BTreeMap::new());
    let said = verify::describe(&report, None);
    assert!(report.ok(), "{said}");
    let states = report.states.as_ref().unwrap();
    let why = "where it began: an unrecorded handler that writes the state may have run before it, which the log doesn't hold";
    assert_eq!(states.unchecked.get(why), Some(&vec![1]), "{said}");
    assert!(said.contains(why), "{said}");

    // Only what it may write: a cell it doesn't write that moved is a break, and so is what moved
    // where its units aren't all here, as it may then write any.
    let copy = || -> verify::Export { serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap() };
    let broke = (1, "it began from a state the run before it (run 0) didn't leave, nor a run beside it".to_string());
    let mut moved = copy();
    let mut line: serde_json::Value = serde_json::from_str(&moved.lines[1]).unwrap();
    line["state_before"]["k"] = serde_json::json!({ "type": "Number", "value": 5.0 });
    moved.lines[1] = serde_json::to_string(&line).unwrap();
    moved.head = recorder::line_hash(&moved.lines[1]);
    let report = verify::check(&moved, None, &BTreeMap::new());
    assert!(report.breaks.is_empty() && report.head_matches, "chained again");
    assert!(report.states.as_ref().unwrap().breaks.contains(&broke), "{}", verify::describe(&report, None));

    let units: BTreeMap<Hash, Unit> =
        export.units.values().map(|hex| decode_preimage(&verify::bytes_of(hex).unwrap()).unwrap()).map(|u| (hash_unit(&u).unwrap(), u)).collect();
    let closure = |from: Hash| {
        let (mut seen, mut stack) = (BTreeSet::from([from]), vec![from]);
        while let Some(h) = stack.pop() {
            stack.extend(holon_core::child_refs(&units[&h]).into_iter().filter(|c| seen.insert(*c)));
        }
        seen
    };
    let server = state.deployments.lock().unwrap()[&slug].server.clone().unwrap();
    let quiet = server.handlers().find(|(label, _, _)| label == "POST /quiet").unwrap().2;
    let mut only_its = closure(quiet);
    only_its.remove(&quiet);
    for (_, _, h) in server.handlers().filter(|(label, _, _)| label != "POST /quiet") {
        only_its.retain(|u| !closure(h).contains(u));
    }
    assert!(!only_its.is_empty());
    let mut missing = copy();
    missing.units.retain(|name, _| !only_its.iter().any(|h| to_hex(h) == *name));
    let report = verify::check(&missing, None, &BTreeMap::new());
    assert!(report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{:?}", report.reruns);
    assert!(report.states.as_ref().unwrap().breaks.contains(&broke), "{}", verify::describe(&report, None));

    let dry =body_json(route(&state, &req("POST", &format!("/deployments/{slug}/restore"), serde_json::json!({ "run": 0, "state": true, "dry_run": true }).to_string().into_bytes())));
    assert_eq!(dry["state"]["unrecorded"], serde_json::json!(["POST /quiet"]), "{dry}");
    assert!(restore::state_lines(&dry["state"]).contains("runs of POST /quiet (unrecorded) aren't logged"), "{}", restore::state_lines(&dry["state"]));
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// A lazy run's record a power loss lost leaves no mark in the log: the state the next run began
/// from moved where only a lazy handler writes, and that is unchecked, not a break; a cell no lazy
/// or unrecorded handler writes that moved is a break all the same.
#[test]
fn what_a_lost_lazy_record_moved_is_unchecked_not_a_break() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("lazy-verify").into();
    let text = "import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\nimport seen from \"collection:seen\"\n\nflow(\"lazy-verify\", { port: 18205 })\n\nroute.post(\"/notes\", (req) => {\n  notes[req.json.id] = req.json.text\n  return \"saved\"\n})\n\nroute.post(\"/seen\", (req) => {\n  seen[req.json.id] = req.json.by\n  return \"ok\"\n}, { log: \"lazy\" })\n";
    let slug = deploy_plain_text(&state, text);
    assert_eq!(data_call(&state, &slug, "POST", "/notes", r#"{"id":"a","text":"hi"}"#).0, 200);
    assert_eq!(data_call(&state, &slug, "POST", "/seen", r#"{"id":"a","by":"sam"}"#).0, 200);
    assert_eq!(data_call(&state, &slug, "POST", "/notes", r#"{"id":"b","text":"there"}"#).0, 200);
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    assert_eq!(export.lines.len(), 3);
    // The lazy run's line lost, and the line after it chained to the one before, as the log
    // goes on after a power loss.
    let lost = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut copy: verify::Export = serde_json::from_value(serde_json::to_value(&export).unwrap()).unwrap();
        let mut after: serde_json::Value = serde_json::from_str(&export.lines[2]).unwrap();
        after["prev"] = serde_json::Value::String(recorder::line_hash(&export.lines[0]));
        edit(&mut after);
        copy.lines = vec![export.lines[0].clone(), serde_json::to_string(&after).unwrap()];
        copy.head = recorder::line_hash(&copy.lines[1]);
        let report = verify::check(&copy, None, &BTreeMap::new());
        assert!(report.breaks.is_empty() && report.head_matches, "chained again: {}", verify::describe(&report, None));
        report.states.unwrap()
    };
    let states = lost(&|_| {});
    let why = "where it began: a lazy handler that writes the state may have run before it, its record lost to a power loss";
    assert_eq!(states.unchecked.get(why), Some(&vec![2]), "{:?}", states.unchecked);
    assert!(states.breaks.is_empty(), "{:?}", states.breaks);
    let states = lost(&|after| after["state_before"]["k"] = serde_json::json!({ "type": "Number", "value": 5.0 }));
    assert!(states.breaks.contains(&(2, "it began from a state the run before it (run 0) didn't leave, nor a run beside it".to_string())), "{:?}", states.breaks);
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// A host whose policy never lets a handler go unrecorded (holon.toml: grants.allow_unrecorded =
/// false) refuses a deploy that asks, saying why; the default policy allows it (§8 Q9).
#[test]
fn a_policy_without_allow_unrecorded_refuses_the_deploy() {
    assert!(Policy::default().allow_unrecorded, "on by default");
    assert!(Policy::parse("[grants]\nauto = []").unwrap().allow_unrecorded);
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_unrecorded = false").unwrap());
    let plan = holon_author::agent::plan(UNRECORDED_BOARD).unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let r = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": ["NetworkIngress"] }));
    let body = String::from_utf8_lossy(&body_bytes(r)).into_owned();
    assert!(body.contains("this host never lets a handler go unrecorded") && body.contains("grants.allow_unrecorded"), "{body}");
}

/// A stateful run writes its deployment's state head only when it changed the state, and never
/// the deployments file (docs/L4s/request-path-L4.md, C3 a and b): one that only read, or wrote
/// back the value that was there, writes nothing at all. Its counts go on in memory, and reach the
/// deployments file with the next change to the deployment's definition.
#[test]
fn a_run_writes_only_its_state_head_and_only_when_it_changed_the_state() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("persist-on-change").into();
    let counter = deploy_plain_text(&state, include_str!("../../holon-author/samples/counter.holon.ts"));
    let same = deploy_plain_text(&state, "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"same\", { port: 8199 })\n\nroute.post(\"/k\", () => {\n  state.k = \"x\"\n  return \"ok\"\n})\n");
    let call = |slug: &str, method: &str, url: &str| {
        assert_eq!(serve_data(&state.store, &state.deployments, &state.deployments_path, slug, &fake_data(method, url, vec![])).status, 200, "{method} {url}");
    };
    let file = || std::fs::read(state.deployments_path.at().unwrap()).unwrap();
    let head = |slug: &str| std::fs::read_to_string(state_head_for(&state.deployments_path, slug).unwrap()).ok();
    let counted = |slug: &str| -> (u64, u64) {
        let on_disk: serde_json::Value = serde_json::from_slice(&file()).unwrap();
        let kept = on_disk.as_array().unwrap().iter().find(|r| r["slug"] == slug).unwrap()["trigger_count"].as_u64().unwrap();
        (state.deployments.lock().unwrap()[slug].trigger_count, kept)
    };
    let deployed = file();
    assert_eq!(head(&counter), None, "no state kept yet");

    call(&counter, "GET", "/hit");
    let first = head(&counter).expect("a write keeps its head");
    for _ in 0..3 {
        call(&counter, "GET", "/last");
    }
    assert_eq!(head(&counter).as_deref(), Some(first.as_str()), "reading the state wrote no head");
    call(&counter, "GET", "/hit");
    assert_ne!(head(&counter).as_deref(), Some(first.as_str()), "a write keeps its new head");
    assert_eq!(file(), deployed, "and no run rewrote the deployments file");
    assert_eq!(counted(&counter), (5, 0), "the runs are counted in memory");
    persist_deployments(&state.deployments_path, &state.deployments);
    assert_eq!(counted(&counter), (5, 5), "and kept with the next change to the definition");

    call(&same, "POST", "/k");
    let kept = head(&same);
    assert!(kept.is_some());
    call(&same, "POST", "/k");
    assert_eq!(head(&same), kept, "writing back the value that was there changed nothing");
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    for folder in ["logs", "state"] {
        let _ = std::fs::remove_dir_all(state.deployments_path.at().unwrap().with_extension(folder));
    }
}

/// A write survives a restart through its state head alone; a stateful deployment with no head
/// starts empty; deleting a deployment removes its head; and however many runs write at once, the
/// head names the state they left.
#[test]
fn a_state_is_kept_by_its_head_across_restarts_concurrent_writes_and_a_delete() {
    let mut state = test_state();
    state.deployments_path = unique_temp_path("state-head").into();
    let counter = deploy_plain_text(&state, include_str!("../../holon-author/samples/counter.holon.ts"));
    let call = |method: &str, url: &str| {
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, &counter, &fake_data(method, url, vec![]));
        assert_eq!(r.status, 200, "{method} {url}");
        String::from_utf8(r.body).unwrap()
    };
    let reload = || load_deployments(&state.deployments_path, &state.store, &state.policy()).unwrap();
    let hits = |table: &BTreeMap<String, Deployment>| table[&counter].state.as_ref().unwrap().state.lock().unwrap().get("hits").cloned();
    assert_eq!(hits(&reload()), None, "no head: an empty state");

    // Many runs writing at once (each writes `hits` and the `visits` collection).
    thread::scope(|s| {
        for t in 0..16 {
            let call = &call;
            s.spawn(move || {
                for i in 0..5 {
                    call("POST", &format!("/visits/v{t}-{i}"));
                    call("GET", "/hit");
                }
            });
        }
    });
    let live = Arc::clone(state.deployments.lock().unwrap()[&counter].state.as_ref().unwrap());
    let head = std::fs::read_to_string(state_head_for(&state.deployments_path, &counter).unwrap()).unwrap();
    assert_eq!(head.trim(), crate::restore::snapshot_hash(&live.state.lock().unwrap()).unwrap(), "the head names the live state");
    assert_eq!(hits(&reload()), Some(Value::Number(80.0)), "every write, back after a restart");
    assert_eq!(call("GET", "/last"), call("GET", "/last"));

    assert_eq!(route(&state, &req("POST", &format!("/deployments/{counter}/stop"), vec![])).status_code, 200);
    let deleted = route(&state, &req("DELETE", &format!("/deployments/{counter}"), vec![]));
    assert_eq!(deleted.status_code, 200);
    assert!(!state_head_for(&state.deployments_path, &counter).unwrap().exists(), "the head goes with its deployment");
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    for folder in ["logs", "state"] {
        let _ = std::fs::remove_dir_all(state.deployments_path.at().unwrap().with_extension(folder));
    }
}

/// Units for the flow cache's tests: a leaf, and composites placing children.
fn cache_leaf(name: &str) -> Unit {
    Unit { params: vec![], ..holon_core::interface_of(name).unwrap() }
}
fn cache_composite(children: Vec<Hash>) -> Unit {
    Unit { body: UnitBody::Composite { children, edges: vec![] }, params: vec![], inputs: vec![], outputs: vec![] }
}

/// A closure is loaded once and then kept by its root hash; two closures that place the same unit
/// hold it once, and count it once.
#[test]
fn a_closure_is_kept_by_its_hash_and_its_units_are_shared() {
    let state = test_state();
    let mut store = state.store.clone();
    let (leaf, beside) = (cache_leaf("clock"), cache_leaf("random"));
    let (l, r) = (store.put(&leaf).unwrap(), store.put(&beside).unwrap());
    let (a_unit, c_unit) = (cache_composite(vec![l]), cache_composite(vec![l, r]));
    let (a, c) = (store.put(&a_unit).unwrap(), store.put(&c_unit).unwrap());
    // What each closure costs alone, the cache emptied between.
    let alone = |root: Hash| {
        state.store.3.set_budget(0);
        state.store.3.set_budget(crate::units::FLOW_CACHE_BYTES);
        closure(&state.store, root).unwrap();
        state.store.3.held().2
    };
    let (a_alone, c_alone) = (alone(a), alone(c));
    state.store.3.set_budget(0);
    state.store.3.set_budget(crate::units::FLOW_CACHE_BYTES);
    let first = closure(&state.store, a).unwrap();
    assert!(closure(&state.store, a).unwrap().is(&first), "the second run gets the kept closure");
    let other = closure(&state.store, c).unwrap();
    assert!(Arc::ptr_eq(&first.unit(&l).unwrap(), &other.unit(&l).unwrap()), "one leaf, held once");
    // Counted once: the two together cost what each does alone, less one leaf.
    assert_eq!(state.store.3.held(), (2, 4, a_alone + c_alone - crate::units::counted_for_tests(&leaf)));
}

/// A closure keeps each composite's shape beside its units (interpreter path, step 1b): its own
/// units find theirs, a leaf has none, and an equal unit that isn't the closure's own finds none.
#[test]
fn a_closure_keeps_its_composites_shapes_for_its_own_units() {
    let state = test_state();
    let mut store = state.store.clone();
    let (l, r) = (store.put(&cache_leaf("clock")).unwrap(), store.put(&cache_leaf("random")).unwrap());
    let a_unit = cache_composite(vec![l, r]);
    let a = store.put(&a_unit).unwrap();
    let loaded = closure(&state.store, a).unwrap();
    let held = loaded.resolve(&a).unwrap();
    assert!(loaded.shape(held).is_some(), "its composite finds its shape");
    assert!(loaded.shape(loaded.resolve(&l).unwrap()).is_none(), "a leaf has none");
    assert!(loaded.shape(&a_unit).is_none(), "an equal unit elsewhere is not its own");
    assert!(closure(&state.store, a).unwrap().shape(held).is_some(), "the kept closure keeps them");
}

/// A closure missing a unit is run as it is but not kept: once the unit is put, the next run
/// resolves it.
#[test]
fn a_partial_closure_is_loaded_again_once_it_is_whole() {
    let state = test_state();
    let mut store = state.store.clone();
    let leaf = cache_leaf("clock");
    let l = hash_unit(&leaf).unwrap();
    let a = store.put(&cache_composite(vec![l])).unwrap();
    assert!(closure(&state.store, a).unwrap().resolve(&l).is_none());
    assert_eq!(state.store.3.held().0, 0, "a partial closure isn't kept");
    store.put(&leaf).unwrap();
    assert!(closure(&state.store, a).unwrap().resolve(&l).is_some());
    assert_eq!(state.store.3.held().0, 1);
}

/// How much memory a log's entries take against the lines they were written as: what the host's
/// view counts a log in memory by (storage.rs). By hand, on a real log:
/// `HOLON_MEASURE_LOG=path/to/slug.jsonl cargo test --release -p holon -- --ignored measure_log_memory --nocapture`.
#[test]
#[ignore]
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn measure_log_memory() {
    fn in_use() -> u64 {
        // SAFETY: mallinfo2 only reads the allocator's counters.
        unsafe { libc::mallinfo2().uordblks as u64 }
    }
    let Ok(path) = std::env::var("HOLON_MEASURE_LOG") else { return };
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let written: usize = lines.iter().map(|l| l.len()).sum();
    let before = in_use();
    let entries: Vec<recorder::LogEntry> = lines.iter().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let after = in_use();
    println!("{path}: {} entries, {written} bytes as lines, {} bytes in memory: {:.2}x", entries.len(), after - before, (after - before) as f64 / written as f64);
    drop(entries);
}

/// How much memory the flow and value caches hold against what their budgets count (S1): the
/// allocator's bytes in use before and after filling each, beside the bytes each says it holds.
/// Every sample's flow loaded through the flow cache; notes-like records, lists of numbers and long
/// text kept by the value cache.
#[test]
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[ignore = "a measurement, run by hand: cargo test --release -p holon -- --ignored measure_cache_memory --nocapture"]
fn measure_cache_memory() {
    fn in_use() -> u64 {
        // SAFETY: mallinfo2 only reads the allocator's counters.
        unsafe { libc::mallinfo2().uordblks as u64 }
    }
    let state = test_state();
    let mut roots = Vec::new();
    for sample in holon_author::samples::samples() {
        let Ok(plan) = holon_author::agent::plan(sample.text) else { continue };
        for u in &plan.units {
            assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
        }
        roots.push(parse_hash(&plan.unit).unwrap());
    }
    state.store.3.set_budget(1 << 30);
    let before = in_use();
    let kept: Vec<_> = roots.iter().map(|r| crate::units::closure(&state.store, *r).ok()).collect();
    let after = in_use();
    let (closures, units, bytes) = state.store.3.held();
    println!(
        "flow cache: {closures} closures, {units} units, {bytes} bytes counted, {} bytes in memory: {:.1}x",
        after - before,
        (after - before) as f64 / bytes as f64
    );
    drop(kept);
    // And the samples without a js node alone: the engine's module, its encoding near its size in
    // memory, out of the count.
    let state = test_state();
    let mut roots = Vec::new();
    for sample in holon_author::samples::samples() {
        let Ok(plan) = holon_author::agent::plan(sample.text) else { continue };
        if sample.text.contains("js ") || sample.text.contains("js(") || sample.text.contains("```") {
            continue;
        }
        for u in &plan.units {
            assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
        }
        roots.push(parse_hash(&plan.unit).unwrap());
    }
    state.store.3.set_budget(1 << 30);
    let before = in_use();
    let kept: Vec<_> = roots.iter().map(|r| crate::units::closure(&state.store, *r).ok()).collect();
    let after = in_use();
    let (closures, units, bytes) = state.store.3.held();
    let modules = kept.iter().flatten().count();
    println!(
        "flow cache, no js ({modules} loaded): {closures} closures, {units} units, {bytes} bytes counted, {} bytes in memory: {:.1}x",
        after - before,
        (after - before) as f64 / bytes as f64
    );
    drop(kept);

    let cache = units::ValueCache::with_budget(1 << 30);
    let text = |s: &str| Value::Text(s.into());
    let values: Vec<(Hash, Value)> = (0..20_000)
        .map(|i| {
            let v = match i % 3 {
                0 => Value::Record(BTreeMap::from([("title".to_string(), text(&format!("note {i}"))), ("body".to_string(), text("some text for a note, a sentence or two long"))])),
                1 => Value::List((0..20).map(|k| Value::Number(f64::from(k) * 1.5 + f64::from(i))).collect()),
                _ => text(&format!("{i} {}", "a line of text ".repeat(40))),
            };
            (holon_core::hash_value(&v).unwrap(), v)
        })
        .collect();
    let before = in_use();
    for (h, v) in &values {
        cache.keep(*h, v);
    }
    let after = in_use();
    let (n, bytes) = cache.size();
    println!("value cache: {n} values, {bytes} bytes counted, {} bytes in memory: {:.1}x", after - before, (after - before) as f64 / bytes as f64);
}

/// The value cache's size, set (`holon serve --value-cache`): lowered, it lets go of the least
/// recently used down to it; at 0 it keeps nothing and finds nothing (S5).
#[test]
fn the_value_caches_size_is_a_setting_it_keeps_to() {
    let cache = units::ValueCache::with_budget(1 << 20);
    let values: Vec<(Hash, Value)> = (0..50).map(|i| (holon_core::hash_value(&Value::Text(format!("v{i}"))).unwrap(), Value::Text(format!("v{i} {}", "x".repeat(1000))))).collect();
    for (h, v) in &values {
        cache.keep(*h, v);
    }
    let (n, bytes) = cache.size();
    assert_eq!(n, 50);
    // The newest used again, so it is kept past the cut.
    assert!(cache.get(&values[0].0).is_some());
    cache.set_budget(bytes / 5);
    let (left, kept) = cache.size();
    assert!(kept <= bytes / 5 && left < 50 && left > 0, "{left} values, {kept} bytes");
    assert!(cache.get(&values[0].0).is_some(), "the most recently used kept");
    cache.set_budget(0);
    assert!(cache.get(&values[0].0).is_none(), "none kept");
}

/// The store keeps the values runs read from disk (units.rs's `ValueCache`, 12.4): a second
/// read is the first's, from memory, even with its file gone; it keeps nothing too big, lets go of
/// the least used past its budget, and an in-memory store keeps nothing beside itself. A
/// collection read through a run is served from it the second time.
#[test]
fn the_store_keeps_the_values_it_read_within_a_budget() {
    let text = |n: usize| Value::Text("x".repeat(n));
    let cache = units::ValueCache::with_budget(10_000);
    for i in 0..10u8 {
        cache.keep([i; 32], &text(2_000));
    }
    let (n, bytes) = cache.size();
    assert!(bytes <= 10_000 && n < 10, "past the budget, the least used go: {n} values, {bytes} bytes");
    assert!(cache.get(&[9; 32]).is_some(), "the newest stays");
    assert!(cache.get(&[0; 32]).is_none(), "the oldest went");
    cache.keep([20; 32], &text(20_000));
    assert!(cache.get(&[20; 32]).is_none(), "too big to keep");
    let none = units::ValueCache::with_budget(0);
    none.keep([1; 32], &text(1));
    assert!(none.get(&[1; 32]).is_none(), "a budget of nothing keeps nothing");

    // On disk: read once from the file, then from memory.
    let dir = unique_temp_path("value-cache");
    let disk = FileStore::new(&dir).unwrap();
    let values = disk.values_dir();
    let mut store = SharedStore::on_disk(Box::new(disk), values.clone());
    let v = Value::Text("kept".into());
    let h = store.put_value(&v).unwrap();
    assert_eq!(store.get_value(&h).unwrap(), Some(v.clone()));
    for entry in std::fs::read_dir(&values).unwrap().flatten() {
        let _ = std::fs::remove_file(entry.path()).or_else(|_| std::fs::remove_dir_all(entry.path()));
    }
    assert_eq!(store.get_value(&h).unwrap(), Some(v), "the second read is the first's, from memory");
    assert!(SharedStore::new(Box::new(holon_host::MemStore::default())).4.size().0 == 0);
    let _ = std::fs::remove_dir_all(&dir);

    // A run's collection read: the trie's values come from memory the second time.
    let store_dir = unique_temp_path("value-cache-run");
    let disk = FileStore::new(&store_dir).unwrap();
    let values = disk.values_dir();
    let mut state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    state.deployments_path = Place::Nowhere;
    let slug = deploy_plain_text(&state, "import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\n\nflow(\"c\", { port: 8302 })\n\nroute.post(\"/notes\", (req) => {\n  notes[req.json.id] = req.json.text\n  return \"saved\"\n})\n\nroute.get(\"/notes\", () => notes.list({ limit: 50 }), { log: \"lazy\" })\n");
    let call = |method: &str, body: &str| serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &fake_data(method, "/notes", body.as_bytes().to_vec()));
    assert_eq!(call("POST", r#"{"id":"a","text":"hi"}"#).status, 200);
    let first = call("GET", "");
    let kept = state.store.4.size().0;
    assert!(kept > 0, "the trie's values are kept once read");
    let second = call("GET", "");
    assert_eq!((first.body.clone(), state.store.4.size().0), (second.body, kept), "and read from there: nothing new kept");
    assert_eq!(String::from_utf8_lossy(&first.body), r#"{"a":"hi"}"#);
    let _ = std::fs::remove_dir_all(&store_dir);
}

/// Past its budget the cache lets go of the least recently used closure, and the units only it
/// held; what it let go of loads again. A budget of 0 keeps nothing.
#[test]
fn the_flow_cache_lets_go_of_the_least_used_past_its_budget() {
    let state = test_state();
    let mut store = state.store.clone();
    let (one, two) = (cache_leaf("clock"), cache_leaf("random"));
    let (l1, l2) = (store.put(&one).unwrap(), store.put(&two).unwrap());
    let (a_unit, b_unit) = (cache_composite(vec![l1]), cache_composite(vec![l2]));
    let (a, b) = (store.put(&a_unit).unwrap(), store.put(&b_unit).unwrap());
    // What each costs, as the cache counts it (its units' memory and its own), learned alone.
    let alone = |root: Hash| {
        state.store.3.set_budget(0);
        state.store.3.set_budget(crate::units::FLOW_CACHE_BYTES);
        closure(&state.store, root).unwrap();
        state.store.3.held().2
    };
    let (a_bytes, b_bytes) = (alone(a), alone(b));
    state.store.3.set_budget(0);
    state.store.3.set_budget(a_bytes.max(b_bytes));
    closure(&state.store, a).unwrap();
    assert_eq!(state.store.3.held(), (1, 2, a_bytes));
    closure(&state.store, b).unwrap();
    assert_eq!(state.store.3.held(), (1, 2, b_bytes), "a, the least recently used, went");
    let again = closure(&state.store, a).unwrap();
    assert!(again.resolve(&l1).is_some(), "what was let go of loads again");
    assert_eq!(state.store.3.held(), (1, 2, a_bytes));

    state.store.3.set_budget(0);
    assert_eq!(state.store.3.held(), (0, 0, 0), "a smaller budget lets go at once");
    let unkept = closure(&state.store, b).unwrap();
    assert!(unkept.resolve(&l2).is_some(), "a run still gets its closure");
    assert_eq!(state.store.3.held(), (0, 0, 0), "and nothing is kept");
}

/// A flow's `on file` blocks are file handlers of its server (the drop-board sample): a file
/// that settles in the folder runs the handler for new files alone, one removed runs the one for
/// deleted files, each logged under its label with the file's handle; a route reads what they
/// wrote, and their runs verify offline. What starts them can't be optional.
#[test]
fn a_servers_file_handlers_run_alone_for_their_lines_beside_its_routes_and_verify() {
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-file-blocks-{}-{}", std::process::id(), rand::random::<u32>()));
    let board = dir.join("board");
    std::fs::create_dir_all(&board).unwrap();
    assert_eq!(route_status(&state, &serde_json::json!({ "name": "board", "path": board.display().to_string(), "access": "read" })), 200);
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/drop-board.holon.ts"), "");
    // A server's handlers are in its unit's canonical order, by hash.
    let mut triggers = state.deployments.lock().unwrap()[&slug].triggers.clone();
    triggers.sort();
    assert_eq!(triggers, ["Http :8157", "on file board", "on file board deleted"]);
    let look = || {
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
        arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
    };
    let runs = || -> Vec<(String, String)> {
        log_entries(&state, &slug)
            .iter()
            .filter(|e| e["kind"] == "file")
            .map(|e| (e["route"].as_str().unwrap_or_default().to_string(), e["performed"].to_string()))
            .collect()
    };

    std::fs::write(board.join("a.csv"), "x,1\n").unwrap();
    look();
    let got = runs();
    assert_eq!(got.iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>(), ["on file board"], "the handler for new files, alone: {got:?}");
    assert!(got[0].1.contains("a.csv") && got[0].1.contains("new"), "it sent the file's name and event: {got:?}");
    let (status, page, _) = data_call(&state, &slug, "GET", "/", "");
    assert!(status == 200 && page.contains("1 files dropped so far"), "the count is the server's state, the routes' too: {page}");

    std::fs::remove_file(board.join("a.csv")).unwrap();
    look();
    let got = runs();
    assert_eq!(got.iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>(), ["on file board", "on file board deleted"], "{got:?}");
    assert!(got[1].1.contains("deleted"), "{got:?}");
    let entry = log_entries(&state, &slug).into_iter().rfind(|e| e["kind"] == "file").unwrap();
    assert!(entry["sourced"].as_array().unwrap().iter().any(|p| p[0] == "FileArrival(\"board\")"), "the handle is sourced: {entry:?}");

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert!(report.reruns.iter().filter(|(_, r)| *r == verify::Rerun::Matched).count() >= 2, "{}", verify::describe(&report, None));
    assert!(replays_as_it_verifies(&state, &slug).contains("file"));

    // What starts its file handlers can't be optional.
    let unit = state.deployments.lock().unwrap()[&slug].unit;
    let r = deploy_json(&state, &slug, unit, serde_json::json!({ "requirements": { "optional": ["FileArrival(\"board\")"] } }));
    assert_eq!(r.status_code, 400);
    assert!(String::from_utf8_lossy(&body_bytes(r)).contains("it can't be optional"));
    route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![]));
    route(&state, &req("DELETE", &format!("/deployments/{slug}"), vec![]));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A flow's `action` blocks are run by hand by their names (the ticker sample's reset): what was
/// sent is the handler's input (nothing is `{}`), its respond's body the answer, the run logged
/// as a trigger run under its label with what was sent, and it verifies offline. The old
/// `/trigger/<slug>` names the way in.
#[test]
fn a_servers_actions_run_by_name_with_what_was_sent_and_verify() {
    let mut state = claims_state("");
    state.deployments_path = unique_temp_path("action-blocks").into();
    let slug = deploy_text(&state, include_str!("../../holon-author/samples/ticker.holon.ts"), "127.0.0.1:9");
    let mut triggers = state.deployments.lock().unwrap()[&slug].triggers.clone();
    triggers.sort();
    assert_eq!(triggers, ["Http :8156", "action reset", "every 2s"]);
    let run = |body: &str| route(&state, &req("POST", &format!("/trigger/{slug}/reset"), body.as_bytes().to_vec()));
    let count = || data_call(&state, &slug, "GET", "/count", "").1;
    assert_eq!(count(), "0");

    let r = body_json(run(r#"{"to": 5}"#));
    assert_eq!(r["answer"], serde_json::json!({ "was": 0, "set": 5 }), "{r}");
    assert_eq!(count(), "5", "the action writes the routes' state");
    let entry = log_entries(&state, &slug).into_iter().find(|e| e["seq"] == r["seq"]).expect("its entry is the answer's seq");
    assert_eq!((entry["kind"].as_str(), entry["route"].as_str()), (Some("trigger"), Some("action reset")));
    assert!(entry["request"].to_string().contains("\"to\""), "what was sent is logged: {entry}");
    // Nothing sent is `{}`.
    assert_eq!(body_json(run(""))["answer"], serde_json::json!({ "was": 5, "set": 0 }));
    assert_eq!(count(), "0");

    assert_eq!(run("{not json").status_code, 400);
    assert_eq!(route(&state, &req("POST", &format!("/trigger/{slug}/nope"), vec![])).status_code, 404);
    let old = route(&state, &req("POST", &format!("/trigger/{slug}"), vec![]));
    assert_eq!(old.status_code, 400);
    assert!(String::from_utf8_lossy(&body_bytes(old)).contains(&format!("POST /trigger/{slug}/reset")));

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert!(report.reruns.iter().filter(|(_, r)| *r == verify::Rerun::Matched).count() >= 2, "{}", verify::describe(&report, None));
    assert!(replays_as_it_verifies(&state, &slug).contains("trigger"));
    // What starts an action can't be optional.
    let unit = state.deployments.lock().unwrap()[&slug].unit;
    let r = deploy_json(&state, &slug, unit, serde_json::json!({ "requirements": { "optional": ["Action(\"reset\")"] } }));
    assert_eq!(r.status_code, 400);
    assert!(String::from_utf8_lossy(&body_bytes(r)).contains("it can't be optional"));
    route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![]));
    assert_eq!(run("").status_code, 409, "a stopped deployment runs nothing");
    route(&state, &req("DELETE", &format!("/deployments/{slug}"), vec![]));

    // A flow of actions alone deploys, listening on no port, and is run by name.
    let slug = deploy_text(&state, "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"notes\")\n\nexport const note = action((n) => {\n  state.note = n\n  return \"noted\"\n})\n", "");
    assert_eq!(state.deployments.lock().unwrap()[&slug].triggers, ["action note"]);
    let r = body_json(route(&state, &req("POST", &format!("/trigger/{slug}/note"), b"\"hi\"".to_vec())));
    assert_eq!(r["answer"], "noted");
    route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![]));
    route(&state, &req("DELETE", &format!("/deployments/{slug}"), vec![]));
}

/// A flow's `every` block is a clock of its server: it ticks on its own period beside the routes,
/// each run logged and kept in the history under its label, and its runs verify offline. A flow
/// of clocks alone listens on no port, and its manifest names the clock what starts its runs.
#[test]
fn a_servers_clock_ticks_on_its_period_beside_its_routes_and_verifies() {
    let mut state = claims_state("");
    let path = unique_temp_path("clock-blocks");
    state.deployments_path = path.clone().into();
    let text = "import { every, flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"ticks\", { port: 18257 })\n\nevery(\"2s\", () => {\n  state.n = (state.n ?? 0) + 1\n})\n\nroute.get(\"/n\", () => state.n ?? 0)\n";
    let slug = deploy_text(&state, text, "127.0.0.1:9");
    let triggers = state.deployments.lock().unwrap()[&slug].triggers.clone();
    assert_eq!(triggers, ["Http :18257", "every 2s"]);

    let t0 = SystemTime::now();
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0);
    assert_eq!(data_call(&state, &slug, "GET", "/n", "").1, "1", "due at deploy");
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(1));
    assert_eq!(data_call(&state, &slug, "GET", "/n", "").1, "1", "not before its period");
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(3));
    assert_eq!(data_call(&state, &slug, "GET", "/n", "").1, "2");

    let ticks: Vec<serde_json::Value> = log_entries(&state, &slug).into_iter().filter(|e| e["kind"] == "tick").collect();
    assert_eq!(ticks.len(), 2);
    assert!(ticks.iter().all(|e| e["route"] == "every 2s"), "{ticks:?}");
    let history = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/history"), vec![])));
    assert_eq!(history["ticks"][0]["label"], "every 2s");

    // Tick now (the editor's): that clock ticks once, at once; one it doesn't have is not found.
    let tick = |clock: &str| route(&state, &req("POST", &format!("/deployments/{slug}/tick"), serde_json::to_vec(&serde_json::json!({ "clock": clock })).unwrap()));
    let r = tick("every 2s");
    assert_eq!(r.status_code, 200);
    assert_eq!(body_json(r)["label"], "every 2s");
    assert_eq!(data_call(&state, &slug, "GET", "/n", "").1, "3");
    assert_eq!(tick("every 9s").status_code, 404);

    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    assert_eq!(report.reruns.iter().filter(|(_, r)| *r == verify::Rerun::Matched).count(), 7, "three ticks and four requests");
    assert_eq!(replays_as_it_verifies(&state, &slug), ["request".to_string(), "tick".to_string()].into());

    // Clocks alone: no port, and the clock its trigger, granted with the rest.
    let alone = deploy_text(&state, "import { every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"beats\")\n\nevery(\"1h\", () => {\n  state.n = (state.n ?? 0) + 1\n})\n", "127.0.0.1:9");
    let (triggers, port) = {
        let deployments = state.deployments.lock().unwrap();
        let d = &deployments[&alone];
        (d.triggers.clone(), d.server.as_ref().map(|s| s.port))
    };
    assert_eq!((triggers, port), (vec!["every 1h".to_string()], Some(None)));
    let manifest = body_json(route(&state, &req("GET", &format!("/deployments/{alone}/manifest"), vec![])));
    assert_eq!(manifest["trigger"], "Clock");
    assert_eq!(manifest["not_granted"], serde_json::json!([]));
    // What starts its clocks can't be optional, beside routes too.
    let ticks_unit = state.deployments.lock().unwrap()[&slug].unit;
    let r = deploy_json(&state, &slug, ticks_unit, serde_json::json!({ "requirements": { "optional": ["Clock"] } }));
    assert_eq!(r.status_code, 400);
    assert!(String::from_utf8_lossy(&body_bytes(r)).contains("it can't be optional"));

    for s in [&slug, &alone] {
        route(&state, &req("POST", &format!("/deployments/{s}/stop"), vec![]));
        route(&state, &req("DELETE", &format!("/deployments/{s}"), vec![]));
    }
    let _ = std::fs::remove_file(&path);
}

// ── flow calls (docs/L4s/triggers-L4.md §9, slice 6) ─────────────────────────────────────────────

const BILLING: &str = "import { action, flow } from \"holon\"\nimport state from \"state\"\nimport charges from \"collection:charges\"\n\nflow(\"billing\")\n\nexport const charge = action((input) => {\n  charges[input.id] = input\n  return { ok: true, total: input.amount * 2 }\n})\n\nexport const reset = action(() => {\n  state.resets = (state.resets ?? 0) + 1\n})\n";
const SHOP: &str = "import { flow, route, status } from \"holon\"\nimport { charge, reset } from \"flow:billing\"\n\nflow(\"shop\", { port: 18311 })\n\nroute.post(\"/orders\", async (req) => {\n  const charged = await charge({ id: req.json.id, amount: req.json.total })\n  return charged\n})\n\nroute.post(\"/reset\", async () => {\n  await reset()\n  return status(204)\n})\n";

fn order(state: &AppState, id: &str, total: f64) -> serde_json::Value {
    let (status, body, _) = data_call(state, "shop", "POST", "/orders", &format!("{{\"id\":\"{id}\",\"total\":{total}}}"));
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

/// A call from an unrecorded handler's run: the other flow logs its run as by a run its caller's
/// log doesn't hold, and says so.
#[test]
fn a_call_from_an_unrecorded_run_says_its_run_isnt_logged() {
    let state = claims_state("");
    deploy_text(&state, BILLING, "127.0.0.1:9");
    deploy_text(&state, &SHOP.replace("  return charged
})", "  return charged
}, { log: \"unrecorded\" })"), "127.0.0.1:9");
    assert_eq!(order(&state, "o1", 5.0)["answer"]["total"], 10.0);
    assert!(log_entries(&state, "shop").is_empty(), "the caller's run isn't logged");
    let theirs = log_entries(&state, "billing");
    let by = theirs.last().unwrap()["by"].as_str().unwrap().to_string();
    assert!(by.starts_with("flow shop, run ") && by.ends_with(" (unrecorded: not in its log)"), "{by}");
}

/// A flow runs another's action in-process: the answer comes back, the other flow logs an ordinary
/// action run by the caller and its run, the caller's log keeps the call, and `holon verify`
/// replays both from their logs without calling again.
#[test]
fn a_flow_runs_another_flows_action_and_both_logs_say_so() {
    let state = claims_state("");
    deploy_text(&state, BILLING, "127.0.0.1:9");
    deploy_text(&state, SHOP, "127.0.0.1:9");
    let granted = state.deployments.lock().unwrap()["shop"].grant.items.clone();
    assert!(granted.contains("RunsAction(\"billing/charge\")") && granted.contains("RunsAction(\"billing/reset\")"), "{granted:?}");

    let answered = order(&state, "o1", 5.0);
    assert_eq!(answered["answer"]["total"], 10.0, "{answered}");
    assert_eq!(answered["answer"]["ok"], true);
    assert!(answered["error"].is_null(), "{answered}");

    // billing's run is an action run, by the calling flow and its run.
    let theirs = log_entries(&state, "billing");
    let ran = theirs.last().unwrap();
    assert_eq!(ran["route"], "action charge");
    let by = ran["by"].as_str().unwrap();
    assert!(by.starts_with("flow shop, run "), "{by}");
    assert!(ran["request"].to_string().contains("o1"), "what was sent is kept: {ran}");
    assert_eq!(answered["run"].as_u64(), Some(theirs.len() as u64 - 1), "the call says which run it was");

    // shop's run kept the call and its answer.
    let mine = log_entries(&state, "shop");
    let rounds = mine.last().unwrap()["rounds"].to_string();
    assert!(rounds.contains("billing") && rounds.contains("charge"), "{rounds}");

    // An action that only writes answers no value: `charged.answer` has none.
    let (status, _, _) = data_call(&state, "shop", "POST", "/reset", "{}");
    assert_eq!(status, 204);
    assert_eq!(log_entries(&state, "billing").last().unwrap()["route"], "action reset");

    // Each log verifies, shop's from its record of the call.
    for slug in ["shop", "billing"] {
        let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
        let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
        assert!(report.ok(), "{slug}: {}", verify::describe(&report, None));
        assert!(report.reruns.iter().all(|(_, r)| *r == verify::Rerun::Matched), "{slug}: {}", verify::describe(&report, None));
    }
}

/// A call that can't be made answers why, as a failed request does, and the caller handles it.
#[test]
fn a_call_that_cant_be_made_says_why() {
    let state = claims_state("");
    deploy_text(&state, SHOP, "127.0.0.1:9");
    let error = |id: &str| order(&state, id, 1.0)["error"].as_str().map(str::to_string);
    assert_eq!(error("a").as_deref(), Some("not_found"), "billing isn't deployed");

    deploy_text(&state, "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"billing\")\n\nexport const other = action(() => {\n  state.n = 1\n})\n", "127.0.0.1:9");
    assert_eq!(error("b").as_deref(), Some("no_action"), "it has no such action");

    deploy_text(&state, BILLING, "127.0.0.1:9");
    assert_eq!(error("c"), None, "now it runs");
    route(&state, &req("POST", "/deployments/billing/pause", vec![]));
    assert_eq!(error("d").as_deref(), Some("not_running"), "paused");
    route(&state, &req("POST", "/deployments/billing/run", vec![]));
    assert_eq!(error("e"), None);

    // Revoked, the caller pauses as with any required permission; kept running, its next call is
    // refused at once, and granted back it runs.
    let change = |body: serde_json::Value| body_json(route(&state, &req("POST", "/deployments/shop/grant", serde_json::to_vec(&body).unwrap())));
    let revoked = change(serde_json::json!({ "revoke": ["RunsAction(\"billing/charge\")"] }));
    assert!(revoked.get("error").is_none(), "{revoked}");
    assert_eq!(data_call(&state, "shop", "POST", "/orders", "{}").0, 503, "paused");
    change(serde_json::json!({ "keep_running": true }));
    assert_eq!(error("f").as_deref(), Some("refused"));
    change(serde_json::json!({ "grant": ["RunsAction(\"billing/charge\")"] }));
    assert_eq!(error("g"), None, "granted back");
}

/// A flow isn't called from its own chain, and a chain is only so long (`max_call_depth`).
#[test]
fn a_chain_of_calls_is_never_a_loop_and_is_only_so_long() {
    let state = claims_state("");
    // Each flow's action is `go`, and so is the next one's: it is imported under another name.
    let relay = |name: &str, next: Option<&str>| match next {
        Some(next) => format!(
            "import {{ action, flow }} from \"holon\"\nimport {{ go as next_go }} from \"flow:{next}\"\n\nflow(\"{name}\")\n\nexport const go = action(async (input) => {{\n  const r = await next_go(input)\n  return r\n}})\n"
        ),
        None => format!("import {{ action, flow }} from \"holon\"\n\nflow(\"{name}\")\n\nexport const go = action((input) => \"{name}\")\n"),
    };
    // ping and pong call each other: the second call back is refused, and says why.
    deploy_text(&state, &relay("ping", Some("pong")), "127.0.0.1:9");
    deploy_text(&state, &relay("pong", Some("ping")), "127.0.0.1:9");
    let r = body_json(route(&state, &req("POST", "/trigger/ping/go", b"{}".to_vec())));
    assert!(r.to_string().contains("already in this chain"), "{r}");

    // a → b → c → d → e: the fourth call down is past the default three.
    for (name, next) in [("a", Some("b")), ("b", Some("c")), ("c", Some("d")), ("d", Some("e")), ("e", None)] {
        deploy_text(&state, &relay(name, next), "127.0.0.1:9");
    }
    let r = body_json(route(&state, &req("POST", "/trigger/a/go", b"{}".to_vec()))).to_string();
    assert!(r.contains("too_deep"), "{r}");
    // One shorter (b → c → d → e) is three deep, and runs.
    let r = body_json(route(&state, &req("POST", "/trigger/b/go", b"{}".to_vec()))).to_string();
    assert!(r.contains("\\\"e\\\"") || r.contains("\"e\""), "{r}");
    assert!(!r.contains("too_deep"), "{r}");

    // It refuses itself, whatever the name.
    deploy_text(&state, "import { action, flow } from \"holon\"\nimport { go } from \"flow:me\"\n\nflow(\"me\")\n\nexport const go = action(async (input) => {\n  const r = await go(input)\n  return r\n})\n", "127.0.0.1:9");
    let r = body_json(route(&state, &req("POST", "/trigger/me/go", b"{}".to_vec()))).to_string();
    assert!(r.contains("already in this chain"), "{r}");
}

/// What a secret reaches doesn't leave in a call: the other flow's log would keep it.
#[test]
fn a_secret_is_never_sent_to_another_flow() {
    let state = claims_state("");
    deploy_text(&state, BILLING, "127.0.0.1:9");
    let text = "import { action, flow } from \"holon\"\nimport token from \"secret:token\"\nimport { charge } from \"flow:billing\"\n\nflow(\"vault\")\n\nexport const leak = action(async () => {\n  const r = await charge({ id: token, amount: 1 })\n  return r\n})\n\nexport const joined = action(async () => {\n  const r = await charge({ id: `id-${token}`, amount: 1 })\n  return r\n})\n";
    let plan = holon_author::agent::plan(text).unwrap();
    for u in &plan.units {
        route(&state, &req("POST", "/unit", u.clone()));
    }
    let unit = parse_hash(&plan.unit).unwrap();
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "vault", "unit": plan.unit })).unwrap())));
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap();
    assert_eq!(deploy_json(&state, "vault", unit, serde_json::json!({ "acknowledge": asked })).status_code, 200);
    route(&state, &req("POST", "/deployments/vault/secrets/token", serde_json::to_vec(&serde_json::json!({ "value": "s3cret-value" })).unwrap()));
    for action in ["leak", "joined"] {
        let r = body_json(route(&state, &req("POST", &format!("/trigger/vault/{action}"), b"{}".to_vec()))).to_string();
        assert!(r.contains("refused") && r.contains("can't be sent to another flow"), "{action}: {r}");
    }
    assert!(!log_entries(&state, "billing").iter().any(|e| e["route"] == "action charge"), "billing never ran");
}

/// What a flow sends another, and what it is answered, is data, as a request's body is: an
/// effect request sent would be performed by the other flow under its grants (an action
/// answering what it is given does), and one answered by the caller under its own. Core makes no
/// call whose input holds one (`flow_call_request`); an answer holding one has no JSON form, so
/// the call fails (`run_action`).
#[test]
fn a_flow_call_sends_and_hands_back_data_only() {
    let state = claims_state("");
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let iface = |name: &str, params: Vec<Param>| Unit { params, ..holon_core::interface_of(name).unwrap() };
    let frozen = |k: &str, v: &str| Param::frozen(k, v);
    // `range(1).map(store_write k)`: a list of one effect request, built as a value.
    let write = stage(&state, &test_store_write("k"));
    let effects = || vec![iface("const", vec![frozen("value", "1"), frozen("type", "Number")]), iface("range", vec![]), iface("map", vec![frozen("fn", &to_hex(&write))])];
    let deploy = |slug: &str, unit: Hash| {
        let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": slug, "unit": to_hex(&unit) })).unwrap())));
        let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap_or_default();
        let r = deploy_json(&state, slug, unit, serde_json::json!({ "acknowledge": asked }));
        let status = r.status_code;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(r)));
    };
    // A flow of actions only listens on no port.
    let server = |children: Vec<Hash>, port: Option<&str>| {
        let params = std::iter::once(frozen("declare", "server")).chain(port.map(|p| frozen("port", p))).collect();
        stage(&state, &Unit { body: UnitBody::Composite { children, edges: vec![] }, params, inputs: vec![], outputs: vec![] })
    };

    // Sent to an action that answers what it is given.
    deploy_text(&state, "import { action, flow } from \"holon\"\n\nflow(\"relay\")\n\nexport const go = action((input) => input)\n", "127.0.0.1:9");
    let mut children = effects();
    children.push(iface("flow_call", vec![frozen("flow", "relay"), frozen("action", "go")]));
    let send = endpoint(&state, "POST", "/send", children, vec![e(0, 0, 1, 0), e(1, 0, 2, 0), e(2, 0, 3, 0), e(3, 0, BOUNDARY, 0)], 0);
    deploy("sender", server(vec![send], Some("18341")));
    let (status, body, _) = data_call(&state, "sender", "POST", "/send", "{}");
    assert_eq!(status, 500, "the call isn't made, so the route has no answer: {body}");
    assert!(log_entries(&state, "sender")[0]["rounds"].as_array().is_none_or(|r| r.is_empty()), "nothing was asked");
    assert!(!log_entries(&state, "relay").iter().any(|x| x["route"] == "action go"), "relay never ran");

    // Answered by an action that makes one.
    let made = effects().iter().map(|u| stage(&state, u)).collect();
    let handler = Unit { body: UnitBody::Composite { children: made, edges: vec![e(0, 0, 1, 0), e(1, 0, 2, 0), e(2, 0, BOUNDARY, 0)] }, params: vec![], inputs: vec![], outputs: vec![Port { name: "out".into(), ty: Type::Any }] };
    let make = holon_author::routes::action_unit(stage(&state, &handler), &handler.inputs, &handler.outputs, "make").unwrap();
    deploy("maker", server(vec![stage(&state, &make)], None));
    let pull = endpoint(&state, "POST", "/pull", vec![iface("flow_call", vec![frozen("flow", "maker"), frozen("action", "make")])], vec![e(0, 0, BOUNDARY, 0)], 0);
    deploy("puller", server(vec![pull], Some("18343")));
    let (_, body, _) = data_call(&state, "puller", "POST", "/pull", "{}");
    assert!(body.contains("\"error\":\"failed\"") && body.contains("has no JSON form"), "{body}");
    assert!(!body.contains("store_write"), "nothing of the effect is handed back: {body}");
}

// ── The throughput check (docs/L4s/request-path-plan.md, step 1) ─────────────────────────────

/// Deploy a flow's text that calls nothing outside, with its document as `holon deploy` sends it
/// (deploycmd.rs), approving what its preview asks: its slug.
fn deploy_plain_text(state: &AppState, text: &str) -> String {
    let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
    for u in &plan.units {
        assert_eq!(route(state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
    let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap_or_default();
    let r = deploy_json(state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": asked, "document": plan.document }));
    let status = r.status_code;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body_bytes(r)));
    plan.name
}

/// How fast the data plane answers, measured, not asserted: four routes of three samples on an
/// on-disk store, beside 30 more deployments, each driven from 1, 8 and 64 threads for
/// `HOLON_PERF_SECS` seconds (5 by default). It calls `serve_data` directly, so it leaves out
/// hyper and the hop onto the blocking pool (`dataplane.rs`), and it fails only if an answer isn't
/// a 200. Each step of the request-path plan reports its numbers before and after.
#[test]
#[ignore = "a measurement, run by hand: cargo test --release -p holon -- --ignored request_throughput --nocapture"]
fn request_throughput() {
    let secs = std::env::var("HOLON_PERF_SECS").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(5.0);
    // On disk, as a server keeps it: the store, the deployments file and the logs.
    let store_dir = unique_temp_path("throughput-store");
    let disk = FileStore::new(&store_dir).unwrap();
    let values = disk.values_dir();
    let mut state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    state.deployments_path = unique_temp_path("throughput").into();
    let calc = include_str!("../../holon-author/samples/calc.holon.ts");
    let (hello, counter) = (include_str!("../../holon-author/samples/hello-api.holon.ts"), include_str!("../../holon-author/samples/counter.holon.ts"));
    for text in [hello, calc, counter] {
        deploy_plain_text(&state, text);
    }
    // One rewrite of a sample's text, which must find what it replaces.
    let swap = |text: &str, from: &str, to: &str| {
        assert!(text.contains(from), "{from:?} isn't in the text it rewrites:\n{text}");
        text.replacen(from, to, 1)
    };
    // Their reads again, lazy (C5): the same runs, recorded without a sync before the answer.
    let lazy = |text: &str, name: &str, port: u16, route: &str| {
        let opens = format!("route.get(\"{route}\", ");
        let line = text.lines().find(|l| l.starts_with(&opens) && l.ends_with(')')).unwrap_or_else(|| panic!("no one-line GET {route} in:\n{text}"));
        let text = swap(text, line, &format!("{}, {{ log: \"lazy\" }})", &line[..line.len() - 1]));
        let text = swap(&text, &format!("flow(\"{name}\""), &format!("flow(\"{name}-lazy\""));
        swap(&text, &format!("port: {port}"), &format!("port: {}", port + 1000))
    };
    deploy_plain_text(&state, &lazy(hello, "hello-api", 8101, "/hello/:name"));
    // Its write route goes: a lazy /last beside a strict /hit would be the same deployment's log.
    let last_only = counter.split("route.get(\"/hit\"").next().unwrap().to_string() + "route.get(\"/last\"" + counter.split("route.get(\"/last\"").nth(1).unwrap();
    assert!(!last_only.contains("route.get(\"/hit\"") && !last_only.contains("route.post(") && last_only.contains("route.get(\"/last\""), "{last_only}");
    deploy_plain_text(&state, &lazy(&last_only, "counter", 8155, "/last"));
    // And unrecorded (granted, as a person approves it): counted, no entry.
    let unrecorded = |text: &str, name: &str, port: u16, route: &str| {
        let text = swap(&lazy(text, name, port, route), "{ log: \"lazy\" }", "{ log: \"unrecorded\" }");
        let text = swap(&text, &format!("flow(\"{name}-lazy\""), &format!("flow(\"{name}-unrecorded\""));
        swap(&text, &format!("port: {}", port + 1000), &format!("port: {}", port + 2000))
    };
    deploy_plain_text(&state, &unrecorded(hello, "hello-api", 8101, "/hello/:name"));
    deploy_plain_text(&state, &unrecorded(&last_only, "counter", 8155, "/last"));
    // Deployments the measured ones share the server with, as a server with others would.
    for i in 1..=30 {
        let text = swap(&swap(calc, "flow(\"calc\"", &format!("flow(\"calc{i}\"")), "port: 8116", &format!("port: {}", 8200 + i));
        deploy_plain_text(&state, &text);
    }
    let deployments = state.deployments.lock().unwrap().len();
    let file_kb = std::fs::metadata(state.deployments_path.at().unwrap()).map_or(0, |m| m.len() / 1024);

    let routes = [
        ("hello-api", "GET", "/hello/Ada", ""),
        ("calc", "POST", "/calc", r#"{"op":"mul","a":6,"b":7}"#),
        ("counter", "GET", "/hit", ""),
        ("counter", "GET", "/last", ""),
        ("hello-api-lazy", "GET", "/hello/Ada", ""),
        ("counter-lazy", "GET", "/last", ""),
        ("hello-api-unrecorded", "GET", "/hello/Ada", ""),
        ("counter-unrecorded", "GET", "/last", ""),
    ];
    let request = |method: &str, url: &str, body: &str| {
        let headers = if body.is_empty() { vec![] } else { vec![("content-type".to_string(), "application/json".to_string())] };
        data::DataRequest { method: method.into(), url: url.into(), headers, body: body.as_bytes().to_vec(), by: None }
    };
    println!("request_throughput: an on-disk store, {deployments} deployments ({file_kb} KB deployments file), {secs}s per row (serve_data: no hyper, no blocking-pool hop)");
    println!("{:<33} {:>7} {:>10} {:>10} {:>10} {:>10}", "route", "threads", "req/s", "p50", "p99", "syncs/run");
    for (slug, method, url, body) in routes {
        let (status, answer) = {
            let r = serve_data(&state.store, &state.deployments, &state.deployments_path, slug, &request(method, url, body));
            (r.status, String::from_utf8_lossy(&r.body).into_owned())
        };
        assert_eq!(status, 200, "{method} {url} on {slug}: {answer}");
        let log = Arc::clone(&state.deployments.lock().unwrap()[slug].log);
        for threads in [1, 8, 64] {
            let syncs = log.syncs();
            let started = std::time::Instant::now();
            let stop = started + Duration::from_secs_f64(secs);
            let mut times: Vec<Duration> = thread::scope(|s| {
                let workers: Vec<_> = (0..threads)
                    .map(|_| {
                        s.spawn(|| {
                            let mut times = Vec::new();
                            while std::time::Instant::now() < stop {
                                let t = std::time::Instant::now();
                                let r = serve_data(&state.store, &state.deployments, &state.deployments_path, slug, &request(method, url, body));
                                times.push(t.elapsed());
                                assert_eq!(r.status, 200, "{method} {url} on {slug}: {}", String::from_utf8_lossy(&r.body));
                            }
                            times
                        })
                    })
                    .collect();
                workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
            });
            let elapsed = started.elapsed().as_secs_f64();
            times.sort();
            let at = |q: f64| times[((times.len() as f64 * q) as usize).min(times.len() - 1)];
            let label = format!("{slug} {method} {url}");
            let per_run = (log.syncs() - syncs) as f64 / times.len() as f64;
            println!("{label:<33} {threads:>7} {:>10.0} {:>10.2?} {:>10.2?} {per_run:>10.2}", times.len() as f64 / elapsed, at(0.50), at(0.99));
        }
    }
    // Two deployments at once, 32 threads each: what one deployment's runs cost the other's.
    let together = [routes[0], routes[1]];
    let started = std::time::Instant::now();
    let stop = started + Duration::from_secs_f64(secs);
    let counts: Vec<usize> = thread::scope(|s| {
        let workers: Vec<_> = together
            .iter()
            .flat_map(|&(slug, method, url, body)| (0..32).map(move |_| (slug, method, url, body)))
            .map(|(slug, method, url, body)| {
                let (state, request) = (&state, &request);
                s.spawn(move || {
                    let mut n = 0;
                    while std::time::Instant::now() < stop {
                        assert_eq!(serve_data(&state.store, &state.deployments, &state.deployments_path, slug, &request(method, url, body)).status, 200);
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().unwrap()).collect()
    });
    let elapsed = started.elapsed().as_secs_f64();
    let (hello, calc): (usize, usize) = (counts[..32].iter().sum(), counts[32..].iter().sum());
    println!(
        "{:<22} {:>7} {:>10.0}   (GET /hello/Ada {:.0}, POST /calc {:.0})",
        "hello + calc at once",
        64,
        (hello + calc) as f64 / elapsed,
        hello as f64 / elapsed,
        calc as f64 / elapsed
    );
    let _ = std::fs::remove_dir_all(&store_dir);
    let _ = std::fs::remove_file(state.deployments_path.at().unwrap());
    let _ = std::fs::remove_dir_all(state.deployments_path.beside("logs").unwrap());
}

/// A compute-heavy route: statistics over a list of numbers and a record per item, all with `map`
/// and `fold` in the graph (`map` and `reduce` in the text; docs/L4s/request-path-L4.md §4.5,
/// measured).
const CRUNCH: &str = r#"import { flow, route } from "holon"

flow("crunch", { port: 8311 })

route.post("/stats", (req) => {
  const xs = req.json.xs
  const count = xs.reduce((acc, x) => acc + 1 + x * 0, 0)
  const total = xs.reduce((acc, x) => acc + x, 0)
  const squares = xs.map((x) => x * x)
  const sumsq = squares.reduce((acc, x) => acc + x, 0)
  const mean = total / count
  const rows = xs.map((x) => ({ v: x, dev: x - mean, sq: (x - mean) * (x - mean) }))
  const spread = rows.reduce((acc, r) => acc + r.sq, 0)
  return { n: count, sum: total, mean, variance: sumsq / count - mean * mean, spread: spread / count }
}, { log: "unrecorded" })
"#;

/// One route run many times through serve_data on an on-disk store, for a profiler: what the
/// request-path plan's 12.4 was found with. `HOLON_PROFILE` picks it (`hello`, an unrecorded
/// greeting; `notes`, a lazy list of a collection of three; `crunch`, [`CRUNCH`] over
/// `HOLON_PROFILE_ITEMS` numbers), `HOLON_PROFILE_N` how many runs, and
/// it says how long a run took:
/// `valgrind --tool=callgrind target/release/deps/holon-… --ignored profile_one_route`.
#[test]
#[ignore = "a profile, run by hand: HOLON_PROFILE=notes cargo test --release -p holon -- --ignored profile_one_route --nocapture"]
fn profile_one_route() {
    let store_dir = unique_temp_path("profile-store");
    let disk = FileStore::new(&store_dir).unwrap();
    let values = disk.values_dir();
    let mut state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    state.deployments_path = unique_temp_path("profile").into();
    let which = std::env::var("HOLON_PROFILE").unwrap_or_else(|_| "hello".into());
    let (text, method, url, body) = match which.as_str() {
        "crunch" => (CRUNCH, "POST", "/stats", ""),
        "notes" => ("import { flow, route } from \"holon\"\nimport notes from \"collection:notes\"\n\nflow(\"p\", { port: 8301 })\n\nroute.post(\"/notes\", (req) => {\n  notes[req.json.id] = req.json.text\n  return \"saved\"\n})\n\nroute.get(\"/notes\", () => notes.list({ limit: 50 }), { log: \"lazy\" })\n", "GET", "/notes", ""),
        _ => ("import { flow, route } from \"holon\"\n\nflow(\"p\", { port: 8301 })\n\nroute.get(\"/hello/:name\", (req) => `Hello, ${req.params.name}`, { log: \"unrecorded\" })\n", "GET", "/hello/Ada", ""),
    };
    let slug = deploy_plain_text(&state, text);
    // A list of numbers to crunch, `HOLON_PROFILE_ITEMS` long (1,000 by default).
    let items: usize = std::env::var("HOLON_PROFILE_ITEMS").ok().and_then(|n| n.parse().ok()).unwrap_or(1000);
    let crunched = format!("{{\"xs\":[{}]}}", (0..items).map(|i| format!("{}.5", (i * 37) % 1000)).collect::<Vec<_>>().join(","));
    let body = if which == "crunch" { crunched.as_str() } else { body };
    for id in ["a", "b", "c"] {
        serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &fake_data("POST", "/notes", format!(r#"{{"id":"{id}","text":"hi"}}"#).into_bytes()));
    }
    let n: usize = std::env::var("HOLON_PROFILE_N").ok().and_then(|n| n.parse().ok()).unwrap_or(2000);
    let started = std::time::Instant::now();
    for _ in 0..n {
        let json = vec![("content-type".to_string(), "application/json".to_string())];
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &data::DataRequest { headers: json, ..fake_data(method, url, body.as_bytes().to_vec()) });
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    }
    eprintln!("{which}: {:.1} us a run", started.elapsed().as_secs_f64() * 1e6 / n as f64);
}

/// The server's golden runs: each sample that needs nothing outside the server (no outbound call,
/// database, folder or other flow), deployed, its routes sent three requests twice over and its
/// actions run, with each run's clock, random number and nonce fixed (`firing::fixed`); what each
/// answered and the log entries it wrote, with their timings and the grant's approval time left
/// out, compared with `tests/golden/server.txt`, the record of what the server gave when it was
/// written. A change that changes an answer or a log entry fails here, naming the run and the line
/// (docs/L4s/interpreter-path-L4.md §4). `HOLON_GOLDEN=update` writes the file afresh.
#[test]
fn the_server_records_what_its_golden_record_holds() {
    fn normalized(mut v: serde_json::Value) -> serde_json::Value {
        if let Some(o) = v.as_object_mut() {
            for k in ["at_ms", "duration_ms", "prev", "grant"] {
                o.remove(k);
            }
            // The state's id is random; where the run stands among its commits stays.
            if let Some(c) = o.get_mut("commit").and_then(|c| c.as_object_mut()) {
                c.remove("cell");
            }
            for t in o.get_mut("timeline").and_then(|t| t.as_array_mut()).into_iter().flatten() {
                if let Some(t) = t.as_object_mut() {
                    t.remove("start_us");
                    t.remove("end_us");
                }
            }
            for r in o.get_mut("rounds").and_then(|r| r.as_array_mut()).into_iter().flatten() {
                for x in r.get_mut("exchanges").and_then(|x| x.as_array_mut()).into_iter().flatten() {
                    if let Some(x) = x.as_object_mut() {
                        x.remove("start_us");
                        x.remove("end_us");
                    }
                }
            }
        }
        v
    }
    const OUTSIDE: &[&str] = &["HttpRequest(", "PlainHttp(", "PrivateAddresses", "DbRead(", "DbWrite(", "FileRead(", "FileWrite(", "FileArrival(", "RunsAction("];
    let xs: Vec<String> = (0..60).map(|i| format!("{}.25", (i * 37) % 100)).collect();
    let bodies = [
        String::new(),
        r#"{"id":"x","name":"tea","price":4,"amount":5,"qty":2,"items":[{"name":"tea","qty":2},{"name":"cake","qty":1}],"event":"gig","count":2,"text":"Hello World","title":"Hello World","author":"Ada","op":"mul","a":6,"b":7,"by":"ops","to":5}"#.to_string(),
        format!(r#"{{"xs":[{}],"items":[]}}"#, xs.join(",")),
    ];
    firing::fixed::from(Some(0));
    let state = test_state();
    let mut out = String::new();
    let mut skipped = Vec::new();
    for sample in holon_author::samples::samples() {
        let Ok(plan) = holon_author::agent::plan(sample.text) else { continue };
        for u in &plan.units {
            route(&state, &req("POST", "/unit", u.clone()));
        }
        let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": plan.name, "unit": plan.unit })).unwrap())));
        let asked: Vec<String> = serde_json::from_value(preview["needs_approval"].clone()).unwrap_or_default();
        // A start of its own action stays on the server; another flow's needs that flow.
        let own = format!("RunsAction(\"{}/", plan.name);
        if !preview["refused"].is_null() || asked.iter().any(|a| !a.starts_with(&own) && OUTSIDE.iter().any(|o| a.starts_with(o))) {
            skipped.push(sample.name);
            continue;
        }
        let r = deploy_json(&state, &plan.name, parse_hash(&plan.unit).unwrap(), serde_json::json!({ "acknowledge": asked, "document": plan.document }));
        if r.status_code != 200 {
            skipped.push(sample.name);
            continue;
        }
        let slug = plan.name.clone();
        let (routes, actions) = {
            let table = state.deployments.lock().unwrap();
            let server = table[&slug].server.clone().unwrap_or_else(|| holon_core::Server { port: None, taint: vec![], tags: vec![], routes: vec![], clocks: vec![], files: vec![], mails: vec![], feeds: vec![], queues: vec![], websockets: vec![], actions: vec![] });
            (server.routes.iter().map(|r| (r.method.clone(), r.path.clone())).collect::<Vec<_>>(), server.actions.iter().map(|a| a.name.clone()).collect::<Vec<_>>())
        };
        let log = Arc::clone(&state.deployments.lock().unwrap()[&slug].log);
        let mut seen = log.next_seq();
        let mut wrote = |out: &mut String, what: String| {
            out.push_str(&what);
            out.push('\n');
            let entries = log.with(|l| l.after(seen.checked_sub(1), usize::MAX));
            for e in entries.into_iter().filter(|e| e.seq >= seen) {
                // Each field as it is when short; a long one by its JSON's hash, so a change still
                // shows, by field.
                let entry = normalized(serde_json::to_value(&e).unwrap());
                let fields: Vec<String> = entry
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| {
                        let json = v.to_string();
                        if json.len() <= 120 { format!("{k}={json}") } else { format!("{k}=#{}", &blake3::hash(json.as_bytes()).to_hex()[..16]) }
                    })
                    .collect();
                out.push_str("  ");
                out.push_str(&fields.join(" "));
                out.push('\n');
            }
            seen = log.next_seq();
        };
        for _round in 0..2 {
            for (method, path) in &routes {
                let url: String = path.split('/').map(|seg| if seg.starts_with(':') || seg.starts_with('*') { "tea" } else { seg }).collect::<Vec<_>>().join("/");
                let url = if url.is_empty() { "/".to_string() } else { url };
                for body in &bodies {
                    let request = data::DataRequest { headers: vec![("content-type".into(), "application/json".into())], ..fake_data(method, &url, body.as_bytes().to_vec()) };
                    let answer = serve_data(&state.store, &state.deployments, &state.deployments_path, &slug, &request);
                    wrote(&mut out, format!("== {slug} · {method} {url} · body {} → {} {}", body.len(), answer.status, String::from_utf8_lossy(&answer.body)));
                }
            }
            for name in &actions {
                for body in &bodies {
                    let input = holon_core::parse_json(if body.is_empty() { "{}" } else { body }).unwrap_or(Value::Record(BTreeMap::new()));
                    let ran = run_action(&state.store, &state.deployments, &state.deployments_path, &slug, name, Ok(input), None, None, &[]);
                    wrote(&mut out, format!("== {slug} · action {name} · body {} → {} {}", body.len(), ran.status, ran.reply));
                }
            }
        }
    }
    firing::fixed::from(None);
    out.push_str(&format!("skipped (outside the server): {}\n", skipped.join(", ")));
    // A resume token is keyed by the host's key, drawn once per process: shown as a token, not
    // its value.
    let mut masked = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(i) = rest.find("rt_") {
        let after = &rest[i + 3..];
        let is_token = after.len() >= 32 && after.as_bytes()[..32].iter().all(|b| b.is_ascii_hexdigit());
        masked.push_str(&rest[..i + 3]);
        if is_token {
            masked.push_str("‹token›");
            rest = &after[32..];
        } else {
            rest = after;
        }
    }
    masked.push_str(rest);
    let out = masked;
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/server.txt");
    if std::env::var("HOLON_GOLDEN").as_deref() == Ok("update") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &out).unwrap();
        return;
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e} (HOLON_GOLDEN=update writes it)", path.display()));
    if kept == out {
        return;
    }
    let mut case = String::from("?");
    for (i, (k, n)) in kept.lines().zip(out.lines()).enumerate() {
        if k.starts_with("== ") {
            case = k.to_string();
        }
        if k != n {
            let k: String = k.chars().take(600).collect();
            let n: String = n.chars().take(600).collect();
            panic!("the server gives something else at line {} of {}, in {case}:\n  kept: {k}\n  now:  {n}", i + 1, path.display());
        }
    }
    panic!("the record has {} lines, the server now {} (last case: {case})", kept.lines().count(), out.lines().count());
}

/// What the server appends to (its activity, the witness's queue) is its owner's only, and a data
/// folder others may read is said at start: a folder it makes is its owner's only already.
#[cfg(unix)]
#[test]
fn the_server_keeps_its_files_for_its_owner_and_says_when_its_folder_isnt() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::path::PathBuf::from(unique_temp_path("owner-only"));
    holon_host::create_private_dir(&dir).unwrap();
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(crate::open_to_others(&dir), None, "its owner's only: nothing to say");
    let log = dir.join("activity.jsonl");
    std::io::Write::write_all(&mut crate::sealing::append_private(&log).unwrap(), b"{}\n").unwrap();
    assert_eq!(mode(&log), 0o600, "appended to, for its owner only");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let said = crate::open_to_others(&dir).expect("readable by others: said");
    assert!(said.contains("mode 755") && said.contains("chmod 700"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A flow's port answers a request by an address, by localhost or by a name the server was given
/// (`--allow-host`, a proxy's), and nothing else: a page loaded from a name pointed at this
/// machine (DNS rebinding) can't read a flow's answers as its own.
#[test]
fn a_flows_port_answers_only_an_address_localhost_or_a_name_it_was_given() {
    use crate::dataplane::host_allowed;
    let names = vec!["hooks.example.com".to_string()];
    for host in ["127.0.0.1", "127.0.0.1:8103", "10.0.0.5:80", "[::1]:8103", "[2001:db8::1]", "localhost", "LOCALHOST:8103", "app.localhost", "hooks.example.com", "Hooks.Example.COM:443", "hooks.example.com."] {
        assert!(host_allowed(Some(host), &names), "{host}");
    }
    assert!(host_allowed(None, &names) && host_allowed(Some(""), &names), "no host: an HTTP/1.0 client, never a browser");
    for host in ["evil.example", "evil.example:8103", "hooks.example.com.evil.example", "localhost.evil.example", "[not-an-address]", "127.0.0.1.nip.io"] {
        assert!(!host_allowed(Some(host), &names), "{host}");
    }
    assert!(!host_allowed(Some("hooks.example.com"), &[]), "a name not given");

    // Through a real port: a name given is answered.
    let mut state = test_state();
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    state.data_limits.names = names;
    let port = free_port();
    let api = todo_server(&state, &port.to_string());
    assert_eq!(deploy_server(&state, "todo", api).status_code, 200);
    sync_listeners(&state);
    let listed = raw_http(port, b"GET /todos HTTP/1.1\r\nHost: hooks.example.com\r\nConnection: close\r\n\r\n");
    assert!(listed.starts_with("HTTP/1.1 200"), "{listed}");
    let refused = raw_http(port, b"GET /todos HTTP/1.1\r\nHost: other.example.com\r\nConnection: close\r\n\r\n");
    assert!(refused.starts_with("HTTP/1.1 421"), "{refused}");
    assert_eq!(route(&state, &req("POST", "/deployments/todo/stop", vec![])).status_code, 200);
}

/// Past `MAX_IN_FLIGHT` requests being answered at once, the control plane admits no more until
/// one ends.
#[test]
fn the_control_plane_admits_so_many_requests_at_once() {
    let in_flight = std::sync::atomic::AtomicUsize::new(0);
    let held: Vec<_> = (0..3).map(|_| crate::Admitted::take(&in_flight, 3).expect("room")).collect();
    assert!(crate::Admitted::take(&in_flight, 3).is_none(), "a fourth isn't admitted");
    drop(held);
    assert_eq!(in_flight.load(std::sync::atomic::Ordering::Acquire), 0);
    assert!(crate::Admitted::take(&in_flight, 3).is_some(), "admitted again once they end");
}
