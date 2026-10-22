//! In-process integration tests. No real server, no network, no port — `route()`/`with_cors()`
//! are plain functions over `Request`/`Response`, and `Request::fake_http` builds a real
//! `Request` value without a socket, so every endpoint is exercised exactly as the real server
//! would run it, just called directly. This replaces the manual `curl` walkthroughs used to
//! verify each inch while building this crate — a real gap, named plainly rather than left
//! implicit, once the surface grew past the first couple of endpoints. `run_due_clock_ticks`
//! (pulled out of `spawn_clock_scheduler` for exactly this reason) means the scheduler is
//! tested with a controlled `now`, not real sleeping.

use super::*;
use holon_core::hash_value;
use holon_host::fire_sourced_with;

fn test_state() -> AppState {
    let backend: DynStore = Box::new(MemStore::new());
    let store = SharedStore(Arc::new(Mutex::new(backend)));
    let registry = Mutex::new(Registry::new(store.clone()));
    let cache = Mutex::new(SharedCache::new());
    let deployments = Arc::new(Mutex::new(BTreeMap::new()));
    AppState {
        store,
        registry,
        cache,
        deployments,
        deployments_path: ":memory:".to_string(),
        flows: Mutex::new(BTreeMap::new()),
        flows_path: ":memory:".to_string(),
        port: TEST_PORT,
        listeners: None,
    }
}

const TEST_PORT: u16 = 7420;

fn host_header() -> (String, String) {
    ("Host".to_string(), format!("127.0.0.1:{TEST_PORT}"))
}

/// A request as the editor sends it: our Host, and for a POST the Content-Type its body is
/// (a JSON body starts with `{` or `[`; preimages and trigger text go as raw bytes).
fn req(method: &str, url: &str, body: Vec<u8>) -> Request {
    let mut headers = vec![host_header()];
    if method == "POST" {
        let json = matches!(body.first(), Some(b'{' | b'['));
        let ty = if json { "application/json" } else { "application/octet-stream" };
        headers.push(("Content-Type".to_string(), ty.to_string()));
    }
    Request::fake_http(method, url, headers, body)
}

fn body_bytes(response: Response) -> Vec<u8> {
    let (mut reader, _) = response.data.into_reader_and_size();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).unwrap();
    buf
}

fn body_json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(response)).expect("response body was not valid JSON")
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

/// Stages a full `test_stateful_counter` graph (a driver leaf — `test_ingress()` or
/// `test_clock()` — plus `store_read(key)`/`one`/`add`/`store_write(key)`) and returns the
/// composite's hash, ready to deploy.
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

/// `driver -> store_write(key)`: writes whatever the trigger supplies (the ingress body as
/// text, or the clock's own epoch number) directly to this deployment's state at `key` —
/// no read involved. Deliberately write-only, not a read-and-increment counter: a
/// `store_read → add → store_write` graph cannot bootstrap its own first value with only
/// this primitive set — an absent key's read fails the *whole* single-output settle
/// (`fire_sourced`'s own honest-absence rule), and there's no coalesce-with-default
/// primitive to route around that. A real, named limitation of the primitive set itself,
/// not something to fight with a contrived test. This unit tests exactly what this inch
/// actually delivers instead: a write from one fire is still there for the *next* fire to
/// see — checked via `GET /deployments/<slug>/state`, not by asking the flow to read back
/// its own prior write.
fn test_stateful_writer(driver_hash: Hash, write_hash: Hash) -> Unit {
    Unit {
        body: UnitBody::Composite {
            children: vec![driver_hash, write_hash],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }, // driver → store_write.value
                Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // store_write → out
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    }
}

/// The `random` driver primitive — zero inputs, output `Number`, same shape as `clock`.
fn test_random() -> Unit {
    Unit {
        body: UnitBody::Primitive { name: "random".into() },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    }
}

/// `ingress` (unwired, present only so the composite derives `NetworkIngress` and is
/// therefore deployable at all — same convention `stage_stateful_reader` already uses)
/// alongside `driver` wired straight to the boundary output. For proving a *second*
/// driver's own capability is honestly sourced even when `NetworkIngress` is what actually
/// triggered the fire — the exact gap `Random` (and `Clock`-via-`/trigger`) had.
fn stage_ingress_plus_driver(state: &AppState, driver: Unit) -> Hash {
    let ingress = test_ingress();
    let ingress_hash = hash_unit(&ingress).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));

    let driver_hash = hash_unit(&driver).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&driver).unwrap()));

    let composite = Unit {
        body: UnitBody::Composite {
            children: vec![ingress_hash, driver_hash],
            edges: vec![Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 }],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Number }],
    };
    let composite_hash = hash_unit(&composite).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&composite).unwrap()));
    composite_hash
}

fn stage_stateful_writer(state: &AppState, driver: Unit, key: &str) -> Hash {
    let driver_hash = hash_unit(&driver).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&driver).unwrap()));

    let write = test_store_write(key);
    let write_hash = hash_unit(&write).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&write).unwrap()));

    let composite = test_stateful_writer(driver_hash, write_hash);
    let composite_hash = hash_unit(&composite).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&composite).unwrap()));

    composite_hash
}

/// `ingress, store_read(key) -> egress`: for testing the honest-absence side directly —
/// a fresh deployment's never-written key evaluates to nothing, so the whole single-output
/// settle fails, proven end to end through `holon-server`, not just at the core level.
fn stage_stateful_reader(state: &AppState, key: &str) -> Hash {
    let ingress = test_ingress();
    let ingress_hash = hash_unit(&ingress).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));

    let read = test_store_read(key);
    let read_hash = hash_unit(&read).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&read).unwrap()));

    let egress = test_sink();
    let egress_hash = hash_unit(&egress).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&egress).unwrap()));

    let composite = Unit {
        body: UnitBody::Composite {
            // ingress is an unwired child, present only so this composite derives
            // NetworkIngress and is therefore deployable/triggerable at all.
            children: vec![ingress_hash, read_hash, egress_hash],
            edges: vec![
                Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 }, // store_read → egress
                Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 }, // egress → out
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    };
    let composite_hash = hash_unit(&composite).unwrap();
    route(state, &req("POST", "/unit", encode_preimage(&composite).unwrap()));
    composite_hash
}

// ── /health ──────────────────────────────────────────────────────────────────────────

#[test]
fn health_check() {
    let state = test_state();
    let r = route(&state, &req("GET", "/health", vec![]));
    assert_eq!(r.status_code, 200);
    assert_eq!(body_bytes(r), b"ok");
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

// ── /registry ────────────────────────────────────────────────────────────────────────

#[test]
fn publish_refuses_dangling_reference_then_succeeds_once_staged() {
    let state = test_state();
    let leaf_hash = hash_unit(&test_unit()).unwrap();
    let composite_bytes = encode_preimage(&test_composite(leaf_hash)).unwrap();

    let refused = route(&state, &req("POST", "/registry/publish", composite_bytes.clone()));
    assert_eq!(refused.status_code, 400);

    route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));

    let published = route(&state, &req("POST", "/registry/publish", composite_bytes));
    assert_eq!(published.status_code, 200);

    let list = route(&state, &req("GET", "/registry", vec![]));
    assert_eq!(body_json(list)["published"].as_array().unwrap().len(), 1);
}

#[test]
fn fork_replaces_target_and_keeps_original_published() {
    let state = test_state();
    let leaf_hash = hash_unit(&test_unit()).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
    let composite = test_composite(leaf_hash);
    let composite_hash = hash_unit(&composite).unwrap();
    route(&state, &req("POST", "/registry/publish", encode_preimage(&composite).unwrap()));

    let replacement_bytes = encode_preimage(&test_replacement()).unwrap();
    let url =
        format!("/registry/fork?root={}&target={}", to_hex(&composite_hash), to_hex(&leaf_hash));
    let forked = route(&state, &req("POST", &url, replacement_bytes));
    assert_eq!(forked.status_code, 200);
    let new_hash = body_json(forked)["hash"].as_str().unwrap().to_string();
    assert_ne!(new_hash, to_hex(&composite_hash));

    let list = route(&state, &req("GET", "/registry", vec![]));
    let published: Vec<String> = body_json(list)["published"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    assert!(published.contains(&to_hex(&composite_hash)));
    assert!(published.contains(&new_hash));
}

#[test]
fn fork_rejects_unknown_root_and_target_outside_closure() {
    let state = test_state();
    let leaf_hash = hash_unit(&test_unit()).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
    let composite = test_composite(leaf_hash);
    let composite_hash = hash_unit(&composite).unwrap();
    route(&state, &req("POST", "/registry/publish", encode_preimage(&composite).unwrap()));
    let replacement_bytes = encode_preimage(&test_replacement()).unwrap();

    let bad_root = "de".repeat(32);
    let url1 = format!("/registry/fork?root={bad_root}&target={}", to_hex(&leaf_hash));
    assert_eq!(route(&state, &req("POST", &url1, replacement_bytes.clone())).status_code, 404);

    let zero_target = "0".repeat(64);
    let url2 = format!("/registry/fork?root={}&target={zero_target}", to_hex(&composite_hash));
    assert_eq!(route(&state, &req("POST", &url2, replacement_bytes)).status_code, 400);
}

// ── /cache ───────────────────────────────────────────────────────────────────────────

#[test]
fn cache_ingest_rejects_key_mismatch_then_succeeds_under_the_real_key() {
    use holon_core::entry_key;

    let state = test_state();
    route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
    let leaf_hash = hash_unit(&test_unit()).unwrap();

    let entry =
        CacheEntry { unit_hash: leaf_hash, inputs: vec![], outputs: vec![Some(Value::Number(42.0))] };
    let real_key = entry_key(&entry).unwrap();
    let body = serde_json::to_vec(&serde_json::json!({
        "unit_hash": to_hex(&leaf_hash),
        "inputs": [],
        "outputs": [{"type": "Number", "value": 42.0}],
    }))
    .unwrap();

    let zero_key = "0".repeat(64);
    let mismatch = route(&state, &req("POST", &format!("/cache/ingest?key={zero_key}"), body.clone()));
    assert_eq!(mismatch.status_code, 400);

    let ok = route(&state, &req("POST", &format!("/cache/ingest?key={}", to_hex(&real_key)), body));
    assert_eq!(ok.status_code, 200);

    let get = route(&state, &req("GET", &format!("/cache/{}", to_hex(&real_key)), vec![]));
    assert_eq!(get.status_code, 200);
    assert_eq!(body_json(get)["outputs"][0]["value"], 42.0);
}

#[test]
fn cache_verified_read_refuses_a_lying_entry() {
    use holon_core::entry_key;

    let state = test_state();
    route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
    let leaf_hash = hash_unit(&test_unit()).unwrap();

    // leaf really evaluates to 42, but this entry claims 999.
    let lie =
        CacheEntry { unit_hash: leaf_hash, inputs: vec![], outputs: vec![Some(Value::Number(999.0))] };
    let key = entry_key(&lie).unwrap();
    let body = serde_json::to_vec(&serde_json::json!({
        "unit_hash": to_hex(&leaf_hash),
        "inputs": [],
        "outputs": [{"type": "Number", "value": 999.0}],
    }))
    .unwrap();
    let ingested = route(&state, &req("POST", &format!("/cache/ingest?key={}", to_hex(&key)), body));
    assert_eq!(ingested.status_code, 200); // structurally valid — ingest doesn't re-execute

    let get = route(&state, &req("GET", &format!("/cache/{}", to_hex(&key)), vec![]));
    assert_eq!(get.status_code, 422); // but the verified read catches the lie
}

// ── /fire ────────────────────────────────────────────────────────────────────────────

#[test]
fn fire_pure_unit_settles_with_no_performed_effects() {
    let state = test_state();
    route(&state, &req("POST", "/unit", encode_preimage(&test_unit()).unwrap()));
    let leaf_hash = hash_unit(&test_unit()).unwrap();

    let body = serde_json::to_vec(&serde_json::json!({"inputs": []})).unwrap();
    let r = route(&state, &req("POST", &format!("/fire?root={}", to_hex(&leaf_hash)), body));
    assert_eq!(r.status_code, 200);
    let v = body_json(r);
    assert_eq!(v["outputs"][0]["value"], 42.0);
    assert_eq!(v["performed"].as_array().unwrap().len(), 0);
}

#[test]
fn fire_sink_unit_records_a_performed_effect() {
    let state = test_state();
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let sink_hash = hash_unit(&test_sink()).unwrap();

    let body =
        serde_json::to_vec(&serde_json::json!({"inputs": [{"type": "Text", "value": "hi"}]})).unwrap();
    let r = route(&state, &req("POST", &format!("/fire?root={}", to_hex(&sink_hash)), body));
    assert_eq!(r.status_code, 200);
    let v = body_json(r);
    assert_eq!(v["performed"][0]["capability"], "NetworkEgress");
    assert_eq!(v["performed"][0]["payload"]["value"], "hi");
}

// ── /deploy + /trigger ───────────────────────────────────────────────────────────────

#[test]
fn deploy_derives_ingress_trigger_and_the_trigger_endpoint_fires_it() {
    let state = test_state();
    let ingress = test_ingress();
    let ingress_hash = hash_unit(&ingress).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_ingress_flow(ingress_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "echo",
        "unit": to_hex(&flow_hash),
        "acknowledge": ["NetworkIngress", "NetworkEgress"],
    }))
    .unwrap();
    let d = route(&state, &req("POST", "/deploy", deploy_body));
    assert_eq!(d.status_code, 200);
    assert_eq!(body_json(d)["triggers"].as_array().unwrap(), &vec![serde_json::json!("Ingress")]);

    let list = route(&state, &req("GET", "/deployments", vec![]));
    assert_eq!(body_json(list)["deployments"].as_array().unwrap().len(), 1);

    let triggered = route(&state, &req("POST", "/trigger/echo", b"hello from test".to_vec()));
    assert_eq!(triggered.status_code, 200);
    let tv = body_json(triggered);
    assert_eq!(tv["performed"][0]["capability"], "NetworkEgress");
    assert_eq!(tv["performed"][0]["payload"]["value"], "hello from test");
}

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

#[test]
fn deploy_clock_unit_requires_a_valid_interval() {
    let state = test_state();
    let clock = test_clock();
    let clock_hash = hash_unit(&clock).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&clock).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_clock_flow(clock_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

    let no_interval = serde_json::to_vec(&serde_json::json!({
        "slug": "tick",
        "unit": to_hex(&flow_hash),
        "acknowledge": ["Clock", "NetworkEgress"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", no_interval)).status_code, 400);

    let zero_interval = serde_json::to_vec(&serde_json::json!({
        "slug": "tick",
        "unit": to_hex(&flow_hash),
        "interval_seconds": 0,
        "acknowledge": ["Clock", "NetworkEgress"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", zero_interval)).status_code, 400);

    let ok = serde_json::to_vec(&serde_json::json!({
        "slug": "tick",
        "unit": to_hex(&flow_hash),
        "interval_seconds": 5,
        "acknowledge": ["Clock", "NetworkEgress"],
    }))
    .unwrap();
    let r = route(&state, &req("POST", "/deploy", ok));
    assert_eq!(r.status_code, 200);
    assert_eq!(body_json(r)["triggers"].as_array().unwrap(), &vec![serde_json::json!("Clock")]);
}

// ── the clock scheduler, with a controlled clock — no real sleeping ────────────────────

#[test]
fn clock_ticks_fire_when_due_and_not_before() {
    let state = test_state();
    let clock = test_clock();
    let clock_hash = hash_unit(&clock).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&clock).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_clock_flow(clock_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "tick",
        "unit": to_hex(&flow_hash),
        "interval_seconds": 5,
        "acknowledge": ["Clock", "NetworkEgress"],
    }))
    .unwrap();
    route(&state, &req("POST", "/deploy", deploy_body));

    let t0 = SystemTime::now();

    // Due immediately — next_due was set to "now" at deploy time.
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0);
    let h1 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
    assert_eq!(body_json(h1)["ticks"].as_array().unwrap().len(), 1);

    // Not due yet — only 1s of a 5s interval has passed.
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(1));
    let h2 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
    assert_eq!(body_json(h2)["ticks"].as_array().unwrap().len(), 1);

    // Due again.
    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, t0 + Duration::from_secs(6));
    let h3 = route(&state, &req("GET", "/deployments/tick/history", vec![]));
    let ticks = body_json(h3)["ticks"].as_array().unwrap().clone();
    assert_eq!(ticks.len(), 2);
    assert_eq!(ticks[0]["performed"][0]["capability"], "NetworkEgress");
}

#[test]
fn deployments_history_for_unknown_slug_is_404() {
    let state = test_state();
    assert_eq!(route(&state, &req("GET", "/deployments/nope/history", vec![])).status_code, 404);
}

#[test]
fn delete_deployment_stops_ingress_trigger_and_removes_it_from_the_list() {
    let state = test_state();
    let ingress = test_ingress();
    let ingress_hash = hash_unit(&ingress).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_ingress_flow(ingress_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));
    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "echo",
        "unit": to_hex(&flow_hash),
        "acknowledge": ["NetworkIngress", "NetworkEgress"],
    }))
    .unwrap();
    route(&state, &req("POST", "/deploy", deploy_body));

    // Works before deletion.
    assert_eq!(route(&state, &req("POST", "/trigger/echo", b"x".to_vec())).status_code, 200);

    assert_eq!(route(&state, &req("DELETE", "/deployments/echo", vec![])).status_code, 409, "stop it first");
    assert_eq!(route(&state, &req("POST", "/deployments/echo/stop", vec![])).status_code, 200);
    let deleted = route(&state, &req("DELETE", "/deployments/echo", vec![]));
    assert_eq!(deleted.status_code, 200);
    assert_eq!(body_json(deleted)["unit"], to_hex(&flow_hash));

    // 404s after deletion — same as a slug that was never deployed.
    assert_eq!(route(&state, &req("POST", "/trigger/echo", b"x".to_vec())).status_code, 404);
    let list = route(&state, &req("GET", "/deployments", vec![]));
    assert_eq!(body_json(list)["deployments"].as_array().unwrap().len(), 0);
}

#[test]
fn delete_deployment_stops_future_clock_ticks() {
    let state = test_state();
    let clock = test_clock();
    let clock_hash = hash_unit(&clock).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&clock).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_clock_flow(clock_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));
    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "tick",
        "unit": to_hex(&flow_hash),
        "interval_seconds": 5,
        "acknowledge": ["Clock", "NetworkEgress"],
    }))
    .unwrap();
    route(&state, &req("POST", "/deploy", deploy_body));

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
    assert_eq!(route(&state, &req("DELETE", "/deployments/nope", vec![])).status_code, 404);
}

// ── capability-consent ───────────────────────────────────────────────────────────────

#[test]
fn deploy_refuses_without_full_capability_acknowledgment() {
    let state = test_state();
    let ingress = test_ingress();
    let ingress_hash = hash_unit(&ingress).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_ingress_flow(ingress_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

    // No acknowledgment at all.
    let no_ack =
        serde_json::to_vec(&serde_json::json!({"slug": "echo", "unit": to_hex(&flow_hash)})).unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", no_ack)).status_code, 400);

    // Only half the real manifest acknowledged — still refused, not partially accepted.
    let partial_ack = serde_json::to_vec(&serde_json::json!({
        "slug": "echo",
        "unit": to_hex(&flow_hash),
        "acknowledge": ["NetworkIngress"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", partial_ack)).status_code, 400);

    // The whole derived manifest acknowledged — succeeds.
    let full_ack = serde_json::to_vec(&serde_json::json!({
        "slug": "echo",
        "unit": to_hex(&flow_hash),
        "acknowledge": ["NetworkIngress", "NetworkEgress"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", full_ack)).status_code, 200);
}

// ── persistence ──────────────────────────────────────────────────────────────────────

#[test]
fn deployments_persist_across_a_simulated_restart() {
    let deployments_path = unique_temp_path("deployments");
    let store_dir = unique_temp_path("store");

    // A real on-disk store, not MemStore — the deployment definition surviving only means
    // something if the unit it points at is still there too.
    let backend: DynStore = Box::new(FileStore::new(&store_dir).unwrap());
    let store = SharedStore(Arc::new(Mutex::new(backend)));

    let ingress = test_ingress();
    let ingress_hash = hash_unit(&ingress).unwrap();
    let mut writer = store.clone();
    writer.put(&ingress).unwrap();
    writer.put(&test_sink()).unwrap();
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_ingress_flow(ingress_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    writer.put(&flow).unwrap();

    // "First run": nothing to load yet, deploy, which persists write-through.
    {
        let deployments = Arc::new(Mutex::new(load_deployments(&deployments_path, &store).unwrap()));
        assert_eq!(deployments.lock().unwrap().len(), 0);

        let state = AppState {
            store: store.clone(),
            registry: Mutex::new(Registry::new(store.clone())),
            cache: Mutex::new(SharedCache::new()),
            deployments,
            deployments_path: deployments_path.clone(),
            flows: Mutex::new(BTreeMap::new()),
            flows_path: ":memory:".to_string(),
            port: TEST_PORT,
            listeners: None,
        };
        let body = serde_json::to_vec(&serde_json::json!({
            "slug": "echo",
            "unit": to_hex(&flow_hash),
            "acknowledge": ["NetworkIngress", "NetworkEgress"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
    }

    // "Restart": a fresh load from the same path/store reconstructs the deployment, with
    // triggers re-derived (not read back from disk), and it actually works.
    let reloaded = load_deployments(&deployments_path, &store).unwrap();
    assert_eq!(reloaded.len(), 1);
    let dep = reloaded.get("echo").unwrap();
    assert_eq!(dep.unit, flow_hash);
    assert_eq!(dep.triggers, vec!["Ingress".to_string()]);

    let state2 = AppState {
        store: store.clone(),
        registry: Mutex::new(Registry::new(store.clone())),
        cache: Mutex::new(SharedCache::new()),
        deployments: Arc::new(Mutex::new(reloaded)),
        deployments_path: deployments_path.clone(),
        flows: Mutex::new(BTreeMap::new()),
        flows_path: ":memory:".to_string(),
        port: TEST_PORT,
        listeners: None,
    };
    let triggered = route(&state2, &req("POST", "/trigger/echo", b"restarted".to_vec()));
    assert_eq!(triggered.status_code, 200);
    assert_eq!(body_json(triggered)["performed"][0]["payload"]["value"], "restarted");

    let _ = std::fs::remove_file(&deployments_path);
    let _ = std::fs::remove_dir_all(&store_dir);
}

// ── multi-flow overlap ───────────────────────────────────────────────────────────────

#[test]
fn multiple_due_clock_deployments_all_fire_in_one_pass() {
    let state = test_state();
    let clock = test_clock();
    let clock_hash = hash_unit(&clock).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&clock).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_clock_flow(clock_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));

    // The same unit, deployed twice under different slugs — a legitimate real scenario,
    // and enough to exercise the per-tick fan-out (two due deployments in one pass).
    for slug in ["tick-a", "tick-b"] {
        let body = serde_json::to_vec(&serde_json::json!({
            "slug": slug,
            "unit": to_hex(&flow_hash),
            "interval_seconds": 5,
            "acknowledge": ["Clock", "NetworkEgress"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
    }

    run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, SystemTime::now());

    for slug in ["tick-a", "tick-b"] {
        let h = route(&state, &req("GET", &format!("/deployments/{slug}/history"), vec![]));
        let ticks = body_json(h)["ticks"].as_array().unwrap().clone();
        assert_eq!(ticks.len(), 1, "slug {slug} should have exactly one tick recorded");
        assert_eq!(ticks[0]["performed"][0]["capability"], "NetworkEgress");
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
        state.flows_path = flows_path.clone();
        let save =
            serde_json::to_vec(&serde_json::json!({ "name": "keeper", "document": doc })).unwrap();
        assert_eq!(route(&state, &req("POST", "/flows", save)).status_code, 200);
    }

    // "Restart": a fresh state loads the table the way main() does, from the same path.
    let mut state2 = test_state();
    state2.flows = Mutex::new(load_flows(&flows_path).unwrap());
    state2.flows_path = flows_path.clone();
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
        let e = load_deployments(&path, &state.store).err().expect("must refuse, not skip");
        assert!(e.contains("deployment"), "{e}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents, "file left as it was");
    }
    std::fs::write(&path, r#"[{"name":"../evil","document":{},"saved_at_epoch_seconds":0}]"#).unwrap();
    assert!(load_flows(&path).is_err());

    // Regression 2: a redeploy keeps a slug's state across a stateless version, but loading
    // only restored state for units deriving a state capability, so a restart dropped it.
    let mut state = test_state();
    state.deployments_path = path.clone();
    let deploy = |unit: Hash, ack: serde_json::Value| {
        let body = serde_json::to_vec(&serde_json::json!({ "slug": "writer", "unit": to_hex(&unit), "acknowledge": ack }))
            .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
    };
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    deploy(writer, serde_json::json!(["NetworkIngress", "StateWrite(\"k\")"]));
    assert_eq!(route(&state, &req("POST", "/trigger/writer", b"kept".to_vec())).status_code, 200);
    let stateless = stage_ingress_plus_driver(&state, test_random());
    deploy(stateless, serde_json::json!(["NetworkIngress", "Random"]));

    let reloaded = load_deployments(&path, &state.store).unwrap();
    let held = reloaded["writer"].state.as_ref().expect("state must survive the restart").lock().unwrap().clone();
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
    state.deployments_path = path.clone();
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let body = serde_json::to_vec(&serde_json::json!({
        "slug": "writer", "unit": to_hex(&writer), "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", body)).status_code, 200);
    let write = |text: &str| assert_eq!(route(&state, &req("POST", "/trigger/writer", text.as_bytes().to_vec())).status_code, 200);
    let current = || body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    let act = |action: &str, body: &str| {
        assert_eq!(route(&state, &req("POST", &format!("/deployments/writer/{action}"), body.as_bytes().to_vec())).status_code, 200)
    };
    write("kept");
    let first = current();
    assert_eq!(first["state"]["k"]["value"], "kept");
    let root = first["root"].as_str().unwrap().to_string();
    assert_eq!(root.len(), 64);

    // The file holds references only, and a restart gets the same root back.
    let file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(file[0].get("state").is_none(), "no inline values: {}", file[0]);
    let hash = hash_value(&Value::Text("kept".into())).unwrap();
    assert_eq!(file[0]["state_refs"]["k"], to_hex(&hash));
    let reloaded = load_deployments(&path, &state.store).unwrap();
    assert_eq!(to_hex(&reloaded["writer"].state.as_ref().unwrap().lock().unwrap().root()), root);

    // Stop (session 1 keeps "kept"), start fresh and write something else (session 2), then
    // restore session 1: the root is the one it had, and its snapshot shares the value's hash.
    act("stop", "");
    act("run", r#"{"from":"fresh"}"#);
    write("other");
    assert_ne!(current()["root"], root.as_str());
    act("stop", "");
    act("run", r#"{"from":1}"#);
    assert_eq!(current()["root"], root.as_str(), "session 1 restored exactly");
    act("stop", "");
    let sessions = body_json(route(&state, &req("GET", "/deployments/writer/sessions", vec![])));
    let s = |i: usize| &sessions["sessions"][i];
    assert_eq!(s(0)["state"]["k"]["value"], "kept", "the sessions API still answers with values");
    assert_eq!(s(0)["root"], root.as_str());
    assert_eq!(s(1)["state"]["k"]["value"], "other");
    assert_eq!(s(0)["state_refs"]["k"], s(2)["state_refs"]["k"], "one value, one hash, stored once");
    let _ = std::fs::remove_file(&path);
}

/// A deployments file from before references (values inline, in the state and in session
/// snapshots) still loads: its values move into the store, and the next save writes references.
/// A reference whose value isn't in the store refuses to load rather than losing the state.
#[test]
fn inline_state_from_older_files_moves_into_the_store() {
    let mut state = test_state();
    let path = unique_temp_path("migrate");
    state.deployments_path = path.clone();
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let text = |s: &str| serde_json::json!({ "type": "Text", "value": s });
    let old = serde_json::json!([{
        "slug": "writer", "unit": to_hex(&writer), "trigger_count": 2, "error_count": 0,
        "state": { "k": text("live") },
        "status": "running",
        "sessions": [
            { "number": 1, "started_at": 0, "ended_at": 5, "from": "deploy", "state": { "k": text("first") } },
            { "number": 2, "started_at": 6, "ended_at": null, "from": "latest" },
        ],
    }]);
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();

    let loaded = load_deployments(&path, &state.store).unwrap();
    assert_eq!(loaded["writer"].state.as_ref().unwrap().lock().unwrap().get("k"), Some(&Value::Text("live".into())));
    let first = loaded["writer"].life.snapshot(1).expect("the inline snapshot became references");
    assert_eq!(state.store.get_value(&first["k"]).unwrap(), Some(Value::Text("first".into())));

    // Saved again, the file holds references only, and loads the same.
    *state.deployments.lock().unwrap() = loaded;
    persist_deployments(&path, &state.deployments, &state.store);
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(!file.contains("\"live\"") && !file.contains("\"first\""), "no inline values left: {file}");
    let again = load_deployments(&path, &state.store).unwrap();
    assert_eq!(again["writer"].life.snapshot(1), Some(first));

    // A reference to a value the store doesn't have: refuse, and leave the file alone.
    let dangling = file.replace(&to_hex(&hash_value(&Value::Text("live".into())).unwrap()), &"cd".repeat(32));
    std::fs::write(&path, &dangling).unwrap();
    let e = load_deployments(&path, &state.store).err().expect("must refuse");
    assert!(e.contains("can't be loaded"), "{e}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), dangling);
    let _ = std::fs::remove_file(&path);
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
        assert!(load_deployments(&path, &state.store).is_err(), "{label} deployments file");
        assert!(load_flows(&path).is_err(), "{label} flows file");
        assert_eq!(std::fs::read(&path).unwrap(), contents, "the bad file must be left as it was");
        let _ = std::fs::remove_file(&path);
    }
    // A missing file is still a normal first run, not an error.
    let missing = unique_temp_path("missing");
    assert!(load_deployments(&missing, &state.store).unwrap().is_empty());
    assert!(load_flows(&missing).unwrap().is_empty());
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
            FlowRecord { name: name.to_string(), saved_at_epoch_seconds: 0, document: serde_json::json!({}) },
        );
        persist_flows(&path, &flows);
    }
    flows.remove("a");
    persist_flows(&path, &flows); // shrinking: no stale tail from the longer write
    assert_eq!(load_flows(&path).unwrap().keys().collect::<Vec<_>>(), vec!["b"]);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── every honestly-answerable driver is actually sourced, regardless of which one
// triggered this fire (the Random / Clock-via-/trigger gap) ────────────────────────────

#[test]
fn trigger_sources_random_even_though_ingress_is_what_actually_fired_it() {
    // Real bug, not a hypothetical: Random was never supplied by any fire path at all — a
    // random leaf always evaluated to nothing, silently, with no error anywhere in the
    // chain (an absent slot is dropped before compute runs, so a downstream primitive like
    // concat just quietly produces shorter text, never a crash). This is the direct
    // regression test for that.
    let state = test_state();
    let composite_hash = stage_ingress_plus_driver(&state, test_random());
    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "rnd",
        "unit": to_hex(&composite_hash),
        "acknowledge": ["NetworkIngress", "Random"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

    let r = route(&state, &req("POST", "/trigger/rnd", b"x".to_vec()));
    assert_eq!(r.status_code, 200);
    let v = body_json(r);
    assert_eq!(v["outputs"][0]["type"], "Number"); // a real number, not null
}

#[test]
fn trigger_sources_clock_even_though_ingress_is_what_actually_fired_it() {
    // The same gap, found by auditing rather than reported: Clock was only ever sourced on
    // the scheduler's own tick path, never on a manual /trigger of an Ingress+Clock unit.
    let state = test_state();
    let composite_hash = stage_ingress_plus_driver(&state, test_clock());
    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "clk",
        "unit": to_hex(&composite_hash),
        "acknowledge": ["NetworkIngress", "Clock"],
        "interval_seconds": 60,
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

    let r = route(&state, &req("POST", "/trigger/clk", b"x".to_vec()));
    assert_eq!(r.status_code, 200);
    let v = body_json(r);
    assert_eq!(v["outputs"][0]["type"], "Number");
}

// ── per-deployment state ─────────────────────────────────────────────────────────────

#[test]
fn trigger_with_state_persists_across_calls() {
    let state = test_state();
    let composite_hash = stage_stateful_writer(&state, test_ingress(), "counter");
    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "writer",
        "unit": to_hex(&composite_hash),
        "acknowledge": ["NetworkIngress", "StateWrite(\"counter\")"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

    // First fire: writes "hello".
    let r1 = route(&state, &req("POST", "/trigger/writer", b"hello".to_vec()));
    assert_eq!(r1.status_code, 200);
    let v1 = body_json(r1);
    assert_eq!(v1["performed"][0]["capability"], "StateWrite(\"counter\")");
    assert_eq!(v1["performed"][0]["payload"]["value"], "hello");

    let s1 = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    assert_eq!(s1["state"]["counter"]["value"], "hello");

    // Second fire, a real *different* call: overwrites — proving this is the same state
    // slot persisting across two separate fires, not something reset or freshly allocated
    // each time.
    let r2 = route(&state, &req("POST", "/trigger/writer", b"world".to_vec()));
    assert_eq!(r2.status_code, 200);
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

    let v1 = stage_stateful_writer(&state, test_ingress(), "counter");
    deploy(v1, serde_json::json!(["NetworkIngress", "StateWrite(\"counter\")"]));
    assert_eq!(route(&state, &req("POST", "/trigger/writer", b"hello".to_vec())).status_code, 200);
    let before = state_arc();

    // Same unit redeployed: state survives, and it is the very same map, not a copy.
    deploy(v1, serde_json::json!(["NetworkIngress", "StateWrite(\"counter\")"]));
    assert!(Arc::ptr_eq(&before, &state_arc()));
    let s = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    assert_eq!(s["state"]["counter"]["value"], "hello");

    // A different stateful version writing another key: the old key is still there.
    let v2 = stage_stateful_writer(&state, test_ingress(), "other");
    deploy(v2, serde_json::json!(["NetworkIngress", "StateWrite(\"other\")"]));
    assert_eq!(route(&state, &req("POST", "/trigger/writer", b"world".to_vec())).status_code, 200);
    let s = body_json(route(&state, &req("GET", "/deployments/writer/state", vec![])));
    assert_eq!(s["state"]["counter"]["value"], "hello");
    assert_eq!(s["state"]["other"]["value"], "world");

    // A stateless version in between doesn't discard it either.
    let stateless = stage_ingress_plus_driver(&state, test_random());
    deploy(stateless, serde_json::json!(["NetworkIngress", "Random"]));
    deploy(v1, serde_json::json!(["NetworkIngress", "StateWrite(\"counter\")"]));
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
            // The respond template (editor/src/templates.ts) at status 200, missing 404.
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
    state.deployments_path = path.clone();
    let api = todo_server(&state, "18080");
    let deployed = deploy_server(&state, "todo-api", api);
    let code = deployed.status_code;
    let shown = body_json(deployed);
    assert_eq!(code, 200, "{shown}");
    assert_eq!(shown["triggers"], serde_json::json!(["Http :18080"]));

    let call = |method: &str, url: &str, body: &str| {
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, "todo-api",
            &Request::fake_http(method, url, vec![], body.as_bytes().to_vec()));
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
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

    // The listing shows the port and the request count; /trigger is refused for a server.
    let listed = body_json(route(&state, &req("GET", "/deployments", vec![])));
    assert_eq!(listed["deployments"][0]["port"], 18080);
    assert_eq!(listed["deployments"][0]["trigger_count"], 13);
    assert_eq!(route(&state, &req("POST", "/trigger/todo-api", b"x".to_vec())).status_code, 400);

    // A second server on the same port is refused; a broken declaration is a 400.
    let other = todo_server(&state, "18080");
    assert_eq!(deploy_server(&state, "other", other).status_code, 409);
    let bad = todo_server(&state, "0");
    assert_eq!(deploy_server(&state, "bad", bad).status_code, 400);

    // A restart reloads it as a server, with its items.
    let reloaded = load_deployments(&path, &state.store).unwrap();
    assert_eq!(reloaded["todo-api"].server.as_ref().map(|s| s.port), Some(18080));
    assert_eq!(reloaded["todo-api"].triggers, vec!["Http :18080"], "labelled as when it was deployed");
    let todos = reloaded["todo-api"].state.as_ref().unwrap().lock().unwrap().get("todos").unwrap().clone();
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
            &Request::fake_http(method, url, vec![], body.as_bytes().to_vec()));
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
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
            &Request::fake_http(method, url, vec![], body.as_bytes().to_vec()))
        .status_code
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
    let resolver = materialize_closure(&state.store, api).unwrap();
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
    let mut host = RecordingHost { performed: Vec::new() };
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
    state.deployments_path = path.clone();
    let api = todo_server(&state, "18082");
    assert_eq!(deploy_server(&state, "todo", api).status_code, 200);
    let call = |method: &str, body: &str| {
        let r = serve_data(&state.store, &state.deployments, &state.deployments_path, "todo",
            &Request::fake_http(method, "/todos", vec![], body.as_bytes().to_vec()));
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
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
    let reloaded = load_deployments(&path, &state.store).unwrap();
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
    state.deployments_path = path.clone();
    let deploy = |state: &AppState, slug: &str, unit: Hash, ack: serde_json::Value| {
        let body = serde_json::to_vec(&serde_json::json!({ "slug": slug, "unit": to_hex(&unit), "acknowledge": ack }))
            .unwrap();
        route(state, &req("POST", "/deploy", body)).status_code
    };
    let a = stage_stateful_writer(&state, test_ingress(), "k");
    assert_eq!(deploy(&state, "a", a, serde_json::json!(["NetworkIngress", "StateWrite(\"k\")"])), 200);

    // Simulate "a" mid-fire: hold its state lock, as fire_with_state does for a whole round.
    let a_state = state.deployments.lock().unwrap()["a"].state.clone().unwrap();
    let mid_fire = a_state.lock().unwrap();

    std::thread::scope(|s| {
        // Deploying "b" persists, and persisting needs "a"'s state, so this waits for "a".
        let deploying = s.spawn(|| deploy(&state, "b", a, serde_json::json!(["NetworkIngress", "StateWrite(\"k\")"])));
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

    let reloaded = load_deployments(&path, &state.store).unwrap();
    assert_eq!(reloaded.keys().collect::<Vec<_>>(), vec!["a", "b"]);
    let _ = std::fs::remove_file(&path);
}

/// Regression: `ingress → egress → store_write` stored the `Effect` itself, the next fire
/// panicked in holon-host's `constant_unit` while holding the state lock, and the poisoned
/// mutex then took down every later `/deploy`, `/deployments` and `/trigger`.
#[test]
fn effect_written_to_state_does_not_take_the_server_down() {
    let state = test_state();
    let ingress = test_ingress();
    let sink = test_sink();
    let write = test_store_write("cell");
    let read = test_store_read("cell");
    let hashes: Vec<Hash> = [&ingress, &sink, &write, &read]
        .iter()
        .map(|u| {
            route(&state, &req("POST", "/unit", encode_preimage(u).unwrap()));
            hash_unit(u).unwrap()
        })
        .collect();
    let composite = Unit {
        body: UnitBody::Composite {
            children: hashes,
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }, // ingress → egress
                Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 }, // egress → store_write
                Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                Edge { from_child: 3, from_port: 0, to_child: BOUNDARY, to_port: 1 }, // store_read
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![
            Port { name: "write".into(), ty: Type::Effect },
            Port { name: "read".into(), ty: Type::Any },
        ],
    };
    let composite_hash = hash_unit(&composite).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&composite).unwrap()));
    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "bad-writer",
        "unit": to_hex(&composite_hash),
        "acknowledge": ["NetworkIngress", "NetworkEgress", "StateRead(\"cell\")", "StateWrite(\"cell\")"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

    for _ in 0..2 {
        assert_eq!(route(&state, &req("POST", "/trigger/bad-writer", b"x".to_vec())).status_code, 200);
    }
    let s = body_json(route(&state, &req("GET", "/deployments/bad-writer/state", vec![])));
    assert!(s["state"].get("cell").is_none(), "an effect request is not data: {s}");
    assert_eq!(route(&state, &req("GET", "/deployments", vec![])).status_code, 200);
}

#[test]
fn clock_tick_with_state_persists_across_ticks() {
    let state = test_state();
    let composite_hash = stage_stateful_writer(&state, test_clock(), "last_tick");
    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "clock-writer",
        "unit": to_hex(&composite_hash),
        "interval_seconds": 5,
        "acknowledge": ["Clock", "StateWrite(\"last_tick\")"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

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

#[test]
fn reading_a_never_written_key_evaluates_to_nothing() {
    // The honest-absence side, proven end to end: a fresh deployment's key that's never
    // been written makes the whole single-output settle fail — the same rule
    // `holon-core`'s own tests already prove in isolation, now confirmed through the real
    // trigger path.
    let state = test_state();
    let composite_hash = stage_stateful_reader(&state, "never_written");
    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "reader",
        "unit": to_hex(&composite_hash),
        "acknowledge": ["NetworkIngress", "StateRead(\"never_written\")", "NetworkEgress"],
    }))
    .unwrap();
    assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);

    let r = route(&state, &req("POST", "/trigger/reader", b"x".to_vec()));
    assert_eq!(r.status_code, 200);
    let v = body_json(r);
    assert_eq!(v["outputs"][0], serde_json::Value::Null);
    assert_eq!(v["performed"].as_array().unwrap().len(), 0);
}

#[test]
fn deployments_listing_shows_state_and_counters() {
    let state = test_state();
    let stateful_hash = stage_stateful_writer(&state, test_ingress(), "k");
    let deploy_stateful = serde_json::to_vec(&serde_json::json!({
        "slug": "stateful",
        "unit": to_hex(&stateful_hash),
        "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"],
    }))
    .unwrap();
    route(&state, &req("POST", "/deploy", deploy_stateful));

    let ingress = test_ingress();
    let ingress_hash = hash_unit(&ingress).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_ingress_flow(ingress_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));
    let deploy_stateless = serde_json::to_vec(&serde_json::json!({
        "slug": "stateless",
        "unit": to_hex(&flow_hash),
        "acknowledge": ["NetworkIngress", "NetworkEgress"],
    }))
    .unwrap();
    route(&state, &req("POST", "/deploy", deploy_stateless));

    route(&state, &req("POST", "/trigger/stateful", b"x".to_vec()));
    route(&state, &req("POST", "/trigger/stateful", b"y".to_vec()));

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

#[test]
fn get_state_404s_for_unknown_slug_and_400s_for_stateless_deployment() {
    let state = test_state();
    assert_eq!(route(&state, &req("GET", "/deployments/nope/state", vec![])).status_code, 404);

    let ingress = test_ingress();
    let ingress_hash = hash_unit(&ingress).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let egress_hash = hash_unit(&test_sink()).unwrap();
    let flow = test_ingress_flow(ingress_hash, egress_hash);
    let flow_hash = hash_unit(&flow).unwrap();
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));
    let deploy_body = serde_json::to_vec(&serde_json::json!({
        "slug": "stateless",
        "unit": to_hex(&flow_hash),
        "acknowledge": ["NetworkIngress", "NetworkEgress"],
    }))
    .unwrap();
    route(&state, &req("POST", "/deploy", deploy_body));

    assert_eq!(route(&state, &req("GET", "/deployments/stateless/state", vec![])).status_code, 400);
}

#[test]
fn state_persists_across_a_simulated_restart() {
    let deployments_path = unique_temp_path("state-deployments");
    let store_dir = unique_temp_path("state-store");
    let backend: DynStore = Box::new(FileStore::new(&store_dir).unwrap());
    let store = SharedStore(Arc::new(Mutex::new(backend)));

    let ingress = test_ingress();
    let ingress_hash = hash_unit(&ingress).unwrap();
    let mut writer = store.clone();
    writer.put(&ingress).unwrap();
    let write_leaf = test_store_write("counter");
    let write_hash = hash_unit(&write_leaf).unwrap();
    writer.put(&write_leaf).unwrap();
    let composite = test_stateful_writer(ingress_hash, write_hash);
    let composite_hash = hash_unit(&composite).unwrap();
    writer.put(&composite).unwrap();

    // "First run": deploy and trigger once — writes "before-restart".
    {
        let deployments = Arc::new(Mutex::new(load_deployments(&deployments_path, &store).unwrap()));
        let state = AppState {
            store: store.clone(),
            registry: Mutex::new(Registry::new(store.clone())),
            cache: Mutex::new(SharedCache::new()),
            deployments,
            deployments_path: deployments_path.clone(),
            flows: Mutex::new(BTreeMap::new()),
            flows_path: ":memory:".to_string(),
            port: TEST_PORT,
            listeners: None,
        };
        let deploy_body = serde_json::to_vec(&serde_json::json!({
            "slug": "writer",
            "unit": to_hex(&composite_hash),
            "acknowledge": ["NetworkIngress", "StateWrite(\"counter\")"],
        }))
        .unwrap();
        assert_eq!(route(&state, &req("POST", "/deploy", deploy_body)).status_code, 200);
        let triggered = route(&state, &req("POST", "/trigger/writer", b"before-restart".to_vec()));
        assert_eq!(triggered.status_code, 200);
    }

    // "Restart": a fresh load from the same path/store — the state survives, and the
    // deployment can still be triggered again afterward.
    let reloaded = load_deployments(&deployments_path, &store).unwrap();
    let dep = reloaded.get("writer").unwrap();
    let restored_state = dep.state.as_ref().unwrap().lock().unwrap();
    assert_eq!(restored_state.get("counter"), Some(&Value::Text("before-restart".to_string())));
    drop(restored_state);

    let state2 = AppState {
        store: store.clone(),
        registry: Mutex::new(Registry::new(store.clone())),
        cache: Mutex::new(SharedCache::new()),
        deployments: Arc::new(Mutex::new(reloaded)),
        deployments_path: deployments_path.clone(),
        flows: Mutex::new(BTreeMap::new()),
        flows_path: ":memory:".to_string(),
        port: TEST_PORT,
        listeners: None,
    };
    let triggered2 = route(&state2, &req("POST", "/trigger/writer", b"after-restart".to_vec()));
    assert_eq!(triggered2.status_code, 200);
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
    let r1 = with_cors(&allowed, route(&state, &allowed));
    assert!(r1
        .headers
        .iter()
        .any(|(k, v)| k.as_ref() == "Access-Control-Allow-Origin" && v.as_ref() == "http://localhost:5173"));

    let disallowed =
        Request::fake_http("GET", "/health", vec![host_header(), ("Origin".to_string(), "http://evil.example".to_string())], vec![]);
    let r2 = with_cors(&disallowed, route(&state, &disallowed));
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
    // what it actually says. This checks the one thing that matters: every method a real
    // endpoint in this file uses (GET, POST, DELETE) is actually named in the value.
    let state = test_state();
    let req = Request::fake_http(
        "OPTIONS",
        "/deployments/whatever",
        vec![host_header(), ("Origin".to_string(), "http://localhost:5173".to_string())],
        vec![],
    );
    let res = with_cors(&req, route(&state, &req));
    let methods = res
        .headers
        .iter()
        .find(|(k, _)| k.as_ref() == "Access-Control-Allow-Methods")
        .map(|(_, v)| v.as_ref().to_string())
        .unwrap_or_default();
    for method in ["GET", "POST", "DELETE"] {
        assert!(methods.contains(method), "Access-Control-Allow-Methods {methods:?} is missing {method}");
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
    let attacks: [(&str, Vec<(&str, &str)>, u16); 7] = [
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
