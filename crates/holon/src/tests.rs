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
        data_limits: dataplane::DataLimits::default(),
        policy: std::sync::RwLock::new(Arc::new(Policy::default())),
        policy_path: ":none:".to_string(),
        secrets: Arc::new(SecretStore::memory()),
        access: access::Access::default(),
        activity: activity::Activity::memory(),
        editor: None,
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

/// A data-plane request with no headers.
fn fake_data(method: &str, url: &str, body: Vec<u8>) -> data::DataRequest {
    data::DataRequest { method: method.into(), url: url.into(), headers: vec![], body }
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

/// An `http_request` puts its host in the flow's manifest: deploy needs that host acknowledged,
/// by name, and "the network" in general isn't enough.
#[test]
fn deploy_needs_each_requested_host_acknowledged_by_name() {
    let state = test_state();
    let ingress = test_ingress();
    let request = Unit {
        params: vec![Param::frozen("method", "GET"), Param::frozen("host", "api.example.com")],
        ..holon_core::interface_of("http_request").unwrap()
    };
    let flow = Unit {
        body: UnitBody::Composite {
            children: vec![stage(&state, &ingress), stage(&state, &request)],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }, // ingress → path
                Edge { from_child: 1, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "response".into(), ty: Type::Record }],
    };
    let flow_hash = stage(&state, &flow);
    let deploy = |ack: serde_json::Value| {
        let body = serde_json::json!({ "slug": "caller", "unit": to_hex(&flow_hash), "acknowledge": ack });
        route(&state, &req("POST", "/deploy", serde_json::to_vec(&body).unwrap()))
    };
    let refused = deploy(serde_json::json!(["NetworkIngress", "NetworkEgress"]));
    assert_eq!(refused.status_code, 400);
    assert!(body_json(refused)["error"].as_str().unwrap().contains("HttpRequest(\"api.example.com\")"));
    assert_eq!(deploy(serde_json::json!(["NetworkIngress", "HttpRequest(\"api.example.com\")"])).status_code, 200);
}

/// A deployed flow's `http_request` goes through the round loop and the outbound rules: here the
/// host is this machine (127.0.0.1), which no deployment may reach without an explicit grant, so
/// the request is refused at perform time. The refusal is the node's response, flows on to the
/// egress, and the flow log records the round: the request, the response, and its node.
#[test]
fn a_deployed_request_is_performed_between_rounds_and_logged() {
    let state = test_state();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let request = Unit {
        params: vec![Param::frozen("method", "GET"), Param::frozen("host", &host)],
        ..holon_core::interface_of("http_request").unwrap()
    };
    let flow = Unit {
        body: UnitBody::Composite {
            children: vec![stage(&state, &test_ingress()), stage(&state, &request), stage(&state, &test_sink())],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 }, // ingress → path
                Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 }, // response → egress
                Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    };
    let flow_hash = stage(&state, &flow);
    let body = serde_json::json!({
        "slug": "caller", "unit": to_hex(&flow_hash),
        "acknowledge": ["NetworkIngress", "NetworkEgress", format!("HttpRequest({host:?})")],
    });
    assert_eq!(route(&state, &req("POST", "/deploy", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let fired = route(&state, &req("POST", "/trigger/caller", b"/status".to_vec()));
    assert_eq!(fired.status_code, 200);

    let log = body_json(route(&state, &req("GET", "/deployments/caller/log", vec![])));
    let entry = &log["entries"][0];
    let exchange = &entry["rounds"][0]["exchanges"][0];
    // The node's path is its index in the unit as stored: children in canonical (hash) order.
    let stored = state.store.get(&flow_hash).unwrap().unwrap();
    let UnitBody::Composite { children, .. } = &stored.body else { panic!() };
    let index = children.iter().position(|h| *h == hash_unit(&request).unwrap()).unwrap();
    assert_eq!(exchange["path"], serde_json::json!([index]), "the request node, by path");
    let url = |v: &serde_json::Value| v["value"].as_array().unwrap().iter().find(|kv| kv[0] == "url").unwrap()[1]["value"].clone();
    assert_eq!(url(&exchange["request"]), format!("https://{host}/status"));
    let error = exchange["response"]["value"].as_array().unwrap().iter().find(|kv| kv[0] == "error").unwrap()[1]["value"].clone();
    assert_eq!(error, "blocked_address", "this machine is a private address");
    assert_eq!(entry["performed"][0]["capability"], "NetworkEgress", "the refusal flowed on to the egress");
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
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    // StateWrite is granted by policy; NetworkIngress isn't.
    let refused = deploy_json(&state, "w", writer, serde_json::json!({}));
    assert_eq!(refused.status_code, 400);
    let message = body_json(refused)["error"].as_str().unwrap().to_string();
    assert!(message.contains("NetworkIngress") && !message.contains("StateWrite"), "{message}");
    assert_eq!(deploy_json(&state, "w", writer, serde_json::json!({ "acknowledge": ["NetworkIngress"] })).status_code, 200);
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
    assert!(p["already_granted"].as_array().unwrap().contains(&serde_json::json!("NetworkIngress")));
    assert!(p["refused"].is_null());
    let p = preview(serde_json::json!({ "requirements": { "private_addresses": true } }));
    assert!(p["refused"].as_str().unwrap().contains("private addresses"));
    // The policy and the deployment's manifest are readable.
    let policy = body_json(route(&state, &req("GET", "/permissions", vec![])));
    assert_eq!(policy["defaults"]["max_requests"], 50);
    let manifest = body_json(route(&state, &req("GET", "/deployments/w/manifest", vec![])));
    assert_eq!(manifest["not_granted"], serde_json::json!([]));
}

/// With the exceptions granted (a host whose policy allows them, a deployment that asks), an
/// outbound request reaches a server on this machine over plain HTTP and its answer flows on.
#[test]
fn granted_exceptions_let_a_request_reach_a_local_server() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(1) {
            let mut stream = stream.unwrap();
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        }
    });
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true").unwrap());
    let request = Unit {
        params: vec![Param::frozen("method", "GET"), Param::frozen("host", &host)],
        ..holon_core::interface_of("http_request").unwrap()
    };
    let field = Unit {
        params: vec![Param::frozen("name", "body")],
        ..holon_core::interface_of("field").unwrap()
    };
    let flow = Unit {
        body: UnitBody::Composite {
            children: vec![stage(&state, &test_ingress()), stage(&state, &request), stage(&state, &field), stage(&state, &test_sink())],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 },
                Edge { from_child: 2, from_port: 0, to_child: 3, to_port: 0 },
                Edge { from_child: 3, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    };
    let flow_hash = stage(&state, &flow);
    let ack = serde_json::json!(["NetworkIngress", "NetworkEgress", format!("HttpRequest({host:?})"), format!("PlainHttp({host:?})"), "PrivateAddresses"]);
    let requirements = serde_json::json!({ "plain_http": [host], "private_addresses": true });
    let deployed = deploy_json(&state, "local", flow_hash, serde_json::json!({ "acknowledge": ack, "requirements": requirements }));
    assert_eq!(deployed.status_code, 200);
    let fired = body_json(route(&state, &req("POST", "/trigger/local", b"/hi".to_vec())));
    assert_eq!(fired["performed"][0]["payload"]["value"], "hello", "the answer's body reached the egress");
}

/// The grant is the boundary at run time: an effect it doesn't cover isn't performed, whatever
/// the graph says. (A grant can only lose an item by being changed under a running deployment;
/// this sets that up directly.)
#[test]
fn an_effect_outside_the_grant_is_refused_at_perform_time() {
    let state = test_state();
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, ack).status_code, 200);
    {
        let mut table = state.deployments.lock().unwrap();
        let dep = table.get_mut("w").unwrap();
        let mut narrowed = dep.grant.clone();
        narrowed.items.remove("StateWrite(\"k\")");
        dep.permit = Arc::new(Permit::new(dep.unit, &narrowed, &state.policy()));
    }
    let fired = body_json(route(&state, &req("POST", "/trigger/w", b"value".to_vec())));
    assert_eq!(fired["performed"], serde_json::json!([]), "the write was refused");
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
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, ack).status_code, 200);

    let revoked = change_grant(&state, "w", serde_json::json!({ "revoke": ["StateWrite(\"k\")"] }));
    assert_eq!(revoked.status_code, 200);
    let view = body_json(revoked);
    assert_eq!(view["status"], "paused");
    assert!(view["attention"].as_str().unwrap().contains("StateWrite(\"k\")"), "{view}");
    assert_eq!(view["grant"]["revoked"], serde_json::json!(["StateWrite(\"k\")"]));
    assert_eq!(route(&state, &req("POST", "/trigger/w", b"v".to_vec())).status_code, 409, "paused");
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
    assert_eq!(route(&state, &req("POST", "/trigger/w", b"v".to_vec())).status_code, 200);
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
    let request = Unit {
        params: vec![Param::frozen("method", "GET"), Param::frozen("host", host)],
        ..holon_core::interface_of("http_request").unwrap()
    };
    let flow = Unit {
        body: UnitBody::Composite {
            children: vec![stage(&state, &test_ingress()), stage(&state, &request), stage(&state, &test_sink())],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 1, to_port: 0 },
                Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 0 },
                Edge { from_child: 2, from_port: 0, to_child: BOUNDARY, to_port: 0 },
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "out".into(), ty: Type::Effect }],
    };
    let flow_hash = stage(&state, &flow);
    let ack = serde_json::json!(["NetworkIngress", "NetworkEgress", format!("HttpRequest({host:?})")]);
    assert_eq!(deploy_json(&state, "caller", flow_hash, serde_json::json!({ "acknowledge": ack })).status_code, 200);

    let change = serde_json::json!({ "revoke": [format!("HttpRequest({host:?})")], "keep_running": true });
    let view = body_json(change_grant(&state, "caller", change));
    assert_eq!(view["status"], "running");
    assert!(view["attention"].is_null());
    assert_eq!(view["grant"]["keep_running"], true);

    assert_eq!(route(&state, &req("POST", "/trigger/caller", b"/status".to_vec())).status_code, 200);
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
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let requirements = serde_json::json!({ "limits": { "request_timeout_ms": 60000 } });
    let ack = serde_json::json!(["NetworkIngress", "StateWrite(\"k\")", "Limit(request_timeout_ms=60000)"]);
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
}

/// The policy file read again holds every deployment to it at once: a ceiling lowered below what a
/// deployment was granted pauses it, and raising it back resumes it. A file that doesn't parse
/// changes nothing.
#[test]
fn reloading_the_policy_holds_every_deployment_to_it() {
    let mut state = test_state();
    let path = unique_temp_path("policy");
    state.policy_path = path.clone();
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let requirements = serde_json::json!({ "limits": { "request_timeout_ms": 60000 } });
    let ack = serde_json::json!(["NetworkIngress", "StateWrite(\"k\")", "Limit(request_timeout_ms=60000)"]);
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
    state.deployments_path = path.clone();
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, ack.clone()).status_code, 200);

    // As if granted less than it needs: drop an item from the stored grant.
    let mut file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file[0]["grant"]["items"] = serde_json::json!(["NetworkIngress"]);
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let reloaded = load_deployments(&path, &state.store, &state.policy()).unwrap();
    let w = &reloaded["w"];
    assert_eq!(w.life.status, Status::Paused);
    assert!(w.attention.as_deref().unwrap().contains("StateWrite(\"k\")"), "{:?}", w.attention);
    *state.deployments.lock().unwrap() = reloaded;
    let listed = body_json(route(&state, &req("GET", "/deployments", vec![])));
    assert!(listed["deployments"][0]["attention"].is_string());
    assert_eq!(route(&state, &req("POST", "/deployments/w/run", vec![])).status_code, 409);
    // Deploying again asks for the missing item, and then it runs.
    assert_eq!(deploy_json(&state, "w", writer, serde_json::json!({ "acknowledge": ["NetworkIngress"] })).status_code, 400);
    assert_eq!(deploy_json(&state, "w", writer, ack).status_code, 200);
    assert_eq!(route(&state, &req("POST", "/trigger/w", b"ok".to_vec())).status_code, 200);

    // A record without a grant: granted its derived capabilities, by migration.
    let mut file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    file[0].as_object_mut().unwrap().remove("grant");
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let migrated = load_deployments(&path, &state.store, &state.policy()).unwrap();
    assert_eq!(migrated["w"].grant.by, "migrated");
    assert!(migrated["w"].attention.is_none() && migrated["w"].grant.items.contains("StateWrite(\"k\")"));
    let _ = std::fs::remove_file(&path);
}

/// Every logged run carries its id, the nonce in it, and the grant it was held to; the id can be
/// recomputed from the entry alone, and two runs of the same input still differ.
#[test]
fn a_run_id_is_recomputable_from_its_log_entry() {
    let state = test_state();
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "w", writer, ack).status_code, 200);
    route(&state, &req("POST", "/trigger/w", b"same".to_vec()));
    route(&state, &req("POST", "/trigger/w", b"same".to_vec()));
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
        .map(|p| {
            let cap = match p[0].as_str().unwrap() {
                "Clock" => Capability::Clock,
                "Random" => Capability::Random,
                _ => Capability::NetworkIngress,
            };
            (cap, sv(&p[1]))
        })
        .collect();
    let hex = e["nonce"].as_str().unwrap();
    let nonce: [u8; 16] = (0..16).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect::<Vec<_>>().try_into().unwrap();
    assert_eq!(hex.len(), 32, "128 bits");
    let recomputed = run_id_of(writer, &[], Some(&state_before), &sourced, &nonce);
    assert_eq!(e["run_id"], to_hex(&recomputed));
}

/// A secret is set over the control plane and never returned by it; a run granted it reads it;
/// and no log entry or reply carries its value, which reads `‹secret:name›` wherever it would
/// have appeared.
#[test]
fn a_secret_reaches_its_runs_and_nothing_else() {
    let state = test_state();
    let secret = Unit { params: vec![Param::frozen("name", "api_key")], ..holon_core::interface_of("secret").unwrap() };
    let equals = holon_core::interface_of("equals").unwrap();
    let flow = Unit {
        body: UnitBody::Composite {
            children: vec![
                stage(&state, &secret),
                stage(&state, &test_ingress()),
                stage(&state, &equals),
                stage(&state, &test_sink()),
                stage(&state, &test_store_write("k")),
            ],
            edges: vec![
                Edge { from_child: 0, from_port: 0, to_child: 2, to_port: 0 }, // secret → equals.a
                Edge { from_child: 1, from_port: 0, to_child: 2, to_port: 1 }, // ingress → equals.b
                Edge { from_child: 2, from_port: 0, to_child: 3, to_port: 0 }, // → egress
                Edge { from_child: 0, from_port: 0, to_child: 4, to_port: 0 }, // secret → state k
                Edge { from_child: 3, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                Edge { from_child: 4, from_port: 0, to_child: BOUNDARY, to_port: 1 },
            ],
        },
        params: vec![],
        inputs: vec![],
        outputs: vec![Port { name: "said".into(), ty: Type::Effect }, Port { name: "kept".into(), ty: Type::Effect }],
    };
    let flow_hash = stage(&state, &flow);
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
    assert_eq!(String::from_utf8(body_bytes(rollback(1))).unwrap(), r#"{"version":3}"#);
    assert_eq!(*state.secrets.vault("s").lock().unwrap().current["api_key"], value);

    let ack = serde_json::json!(["NetworkIngress", "NetworkEgress", "SecretRead(\"api_key\")", "StateWrite(\"k\")"]);
    assert_eq!(deploy_json(&state, "s", flow_hash, serde_json::json!({ "acknowledge": ack })).status_code, 200);
    let fire = |body: &str| String::from_utf8(body_bytes(route(&state, &req("POST", "/trigger/s", body.as_bytes().to_vec())))).unwrap();
    let matched = fire(value);
    assert!(matched.contains(r#""value":true"#), "the run read the secret: {matched}");
    assert!(fire("something else").contains(r#""value":false"#));

    // The log keeps what each node produced, so a replay can show a node it can't recompute:
    // `equals` said true, and the secret node's own output is its placeholder.
    let log = body_json(route(&state, &req("GET", "/deployments/s/log", vec![])));
    let produced: Vec<serde_json::Value> = log["entries"][0]["timeline"].as_array().unwrap().iter().filter_map(|t| t.get("outputs").cloned()).collect();
    assert!(produced.contains(&serde_json::json!([{ "type": "Boolean", "value": true }])), "{produced:?}");
    assert!(produced.contains(&serde_json::json!([{ "type": "Text", "value": "‹secret:api_key›" }])), "{produced:?}");

    // Nowhere in what the control plane says about it.
    for url in ["/deployments/s/log", "/deployments/s/state", "/deployments/s/sessions", "/deployments/s/secrets", "/deployments/s/grant"] {
        let text = String::from_utf8(body_bytes(route(&state, &req("GET", url, vec![])))).unwrap();
        assert!(!text.contains(value), "{url} leaks it: {text}");
        if url.ends_with("/state") {
            assert!(!text.contains("\"k\""), "a secret is never stored in state: {text}");
        }
    }
    // Not in the value store on disk either: nothing that contains it was ever written.
    assert!(state.deployments.lock().unwrap()["s"].state.as_ref().unwrap().lock().unwrap().get("k").is_none());
    assert!(!matched.contains(value), "the trigger's reply doesn't either");

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
    state.deployments_path = path.clone();
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
    let written = || state.deployments.lock().unwrap()["shop"].state.as_ref().unwrap().lock().unwrap().get("paid").cloned();
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
    assert_eq!(load_deployments(&path, &state.store, &state.policy()).unwrap()["shop"].parked.len(), 1);
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(!on_disk.contains(&token) && !on_disk.contains("\"tokens\""), "a run record holds no token");

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
    *state.deployments.lock().unwrap().get_mut("shop").unwrap().state.as_ref().unwrap().lock().unwrap() = State::new();
    call("POST", "/orders");
    assert_eq!(runs().len(), 1);
    durable::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 4000);
    assert!(runs().is_empty(), "expired and finished");
    assert_eq!(written(), None);
    let entries = body_json(route(&state, &req("GET", "/deployments/shop/log", vec![])))["entries"].as_array().unwrap().clone();
    assert_eq!(entries.last().unwrap()["events"]["paid"]["value"][0][0], "expired");
    let _ = std::fs::remove_file(&path);
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
        let deployments = Arc::new(Mutex::new(load_deployments(&deployments_path, &store, &Policy::default()).unwrap()));
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
            data_limits: dataplane::DataLimits::default(),
            policy: std::sync::RwLock::new(Arc::new(Policy::default())),
        policy_path: ":none:".to_string(),
            secrets: Arc::new(SecretStore::memory()),
        access: access::Access::default(),
        activity: activity::Activity::memory(),
        editor: None,
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
    let reloaded = load_deployments(&deployments_path, &store, &Policy::default()).unwrap();
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
        data_limits: dataplane::DataLimits::default(),
        policy: std::sync::RwLock::new(Arc::new(Policy::default())),
        policy_path: ":none:".to_string(),
        secrets: Arc::new(SecretStore::memory()),
        access: access::Access::default(),
        activity: activity::Activity::memory(),
        editor: None,
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
        let e = load_deployments(&path, &state.store, &state.policy()).err().expect("must refuse, not skip");
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

    let reloaded = load_deployments(&path, &state.store, &state.policy()).unwrap();
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
    let reloaded = load_deployments(&path, &state.store, &state.policy()).unwrap();
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

    let loaded = load_deployments(&path, &state.store, &state.policy()).unwrap();
    assert_eq!(loaded["writer"].state.as_ref().unwrap().lock().unwrap().get("k"), Some(&Value::Text("live".into())));
    let first = loaded["writer"].life.snapshot(1).expect("the inline snapshot became references");
    assert_eq!(state.store.get_value(&first["k"]).unwrap(), Some(Value::Text("first".into())));

    // Saved again, the file holds references only, and loads the same.
    *state.deployments.lock().unwrap() = loaded;
    persist_deployments(&path, &state.deployments, &state.store);
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(!file.contains("\"live\"") && !file.contains("\"first\""), "no inline values left: {file}");
    let again = load_deployments(&path, &state.store, &state.policy()).unwrap();
    assert_eq!(again["writer"].life.snapshot(1), Some(first));

    // A reference to a value the store doesn't have: refuse, and leave the file alone.
    let dangling = file.replace(&to_hex(&hash_value(&Value::Text("live".into())).unwrap()), &"cd".repeat(32));
    std::fs::write(&path, &dangling).unwrap();
    let e = load_deployments(&path, &state.store, &state.policy()).err().expect("must refuse");
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
        assert!(load_deployments(&path, &state.store, &state.policy()).is_err(), "{label} deployments file");
        assert!(load_flows(&path).is_err(), "{label} flows file");
        assert_eq!(std::fs::read(&path).unwrap(), contents, "the bad file must be left as it was");
        let _ = std::fs::remove_file(&path);
    }
    // A missing file is still a normal first run, not an error.
    let missing = unique_temp_path("missing");
    assert!(load_deployments(&missing, &state.store, &Policy::default()).unwrap().is_empty());
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
            FlowRecord { name: name.to_string(), saved_at_epoch_seconds: 0, document: serde_json::json!({}), saved_by: None, uses: None },
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
    state.deployments_path = path.clone();
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
    *state.deployments.lock().unwrap() = load_deployments(&path, &state.store, &state.policy()).unwrap();
    assert_eq!(call(&state, "GET", "/items/3", ""), (200, r#"{"n":3}"#.to_string()));
    let _ = std::fs::remove_file(&path);
}

/// `ingress → (ignored)`, and `store_read(count) → default 0 → +1 → store_write(count)`, with
/// `extra` children wired after: a counter every trigger bumps.
fn counter(state: &AppState, extra: Vec<Unit>, extra_edges: Vec<Edge>) -> Hash {
    let iface = |name: &str, params: Vec<Param>| Unit { params, ..holon_core::interface_of(name).unwrap() };
    let frozen = |k: &str, v: &str| Param::frozen(k, v);
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    let mut children = vec![
        test_ingress(),
        iface("store_read", vec![frozen("key", "count")]),
        iface("default", vec![frozen("value", "0"), frozen("type", "Number")]),
        iface("const", vec![frozen("value", "1"), frozen("type", "Number")]),
        iface("add", vec![]),
        test_store_write("count"),
    ];
    let outputs = 1 + extra_edges.iter().filter(|x| x.to_child == BOUNDARY).count();
    children.extend(extra);
    let mut edges = vec![e(1, 0, 2, 0), e(2, 0, 4, 0), e(3, 0, 4, 1), e(4, 0, 5, 0), e(5, 0, BOUNDARY, 0)];
    edges.extend(extra_edges);
    let unit = Unit {
        body: UnitBody::Composite { children: children.iter().map(|u| stage(state, u)).collect(), edges },
        params: vec![],
        inputs: vec![],
        outputs: (0..outputs).map(|i| Port { name: format!("out{i}"), ty: Type::Effect }).collect(),
    };
    stage(state, &unit)
}

/// Runs of one deployment no longer queue on its state: they run side by side and commit only
/// if what they read is unchanged, running again otherwise. Many at once lose no update.
#[test]
fn concurrent_runs_lose_no_update() {
    let state = test_state();
    let unit = counter(&state, vec![], vec![]);
    let ack = serde_json::json!(["NetworkIngress", "StateRead(\"count\")", "StateWrite(\"count\")"]);
    assert_eq!(deploy_json(&state, "c", unit, serde_json::json!({ "acknowledge": ack })).status_code, 200);
    std::thread::scope(|s| {
        for _ in 0..24 {
            s.spawn(|| assert_eq!(route(&state, &req("POST", "/trigger/c", b"go".to_vec())).status_code, 200));
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
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming().take(2) {
            let mut stream = stream.unwrap();
            let counted = std::sync::Arc::clone(&counted);
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                counted.fetch_add(1, Ordering::SeqCst);
                if String::from_utf8_lossy(&buf[..n]).starts_with("GET /slow") {
                    std::thread::sleep(Duration::from_millis(400));
                }
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            });
        }
    });
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true").unwrap());
    let call = Unit {
        params: vec![Param::frozen("method", "GET"), Param::frozen("host", &host)],
        ..holon_core::interface_of("http_request").unwrap()
    };
    let e = |fc, fp, tc, tp| Edge { from_child: fc, from_port: fp, to_child: tc, to_port: tp };
    // The ingress body is the path it calls; the call's answer is egressed.
    let unit = counter(&state, vec![call, test_sink()], vec![e(0, 0, 6, 0), e(6, 0, 7, 0), e(7, 0, BOUNDARY, 1)]);
    let ack = serde_json::json!([
        "NetworkIngress", "NetworkEgress", "StateRead(\"count\")", "StateWrite(\"count\")",
        format!("HttpRequest({host:?})"), format!("PlainHttp({host:?})"), "PrivateAddresses",
    ]);
    let requirements = serde_json::json!({ "plain_http": [host], "private_addresses": true });
    assert_eq!(deploy_json(&state, "slow", unit, serde_json::json!({ "acknowledge": ack, "requirements": requirements })).status_code, 200);

    std::thread::scope(|s| {
        let first = s.spawn(|| route(&state, &req("POST", "/trigger/slow", b"/slow".to_vec())).status_code);
        std::thread::sleep(Duration::from_millis(100));
        let started = std::time::Instant::now();
        assert_eq!(route(&state, &req("POST", "/trigger/slow", b"/fast".to_vec())).status_code, 200);
        assert!(started.elapsed() < Duration::from_millis(250), "held up {:?} by the slow run", started.elapsed());
        assert_eq!(first.join().unwrap(), 200);
    });
    let shown = body_json(route(&state, &req("GET", "/deployments/slow/state", vec![])));
    assert_eq!(shown["state"]["count"]["value"], 2.0, "both counted: {shown}");
    assert_eq!(hits.load(Ordering::SeqCst), 2, "the slow call went out once");
    let log = body_json(route(&state, &req("GET", "/deployments/slow/log", vec![])));
    let retried: Vec<u64> = log["entries"].as_array().unwrap().iter().map(|x| x["retries"].as_u64().unwrap_or(0)).collect();
    assert_eq!(retried.iter().sum::<u64>(), 1, "the slow run ran once more: {retried:?}");
}

/// A port nothing listens on just now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
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
    let posted = raw_http(port, format!("POST /todos HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
    assert!(posted.starts_with("HTTP/1.1 200"), "{posted}");
    let listed = raw_http(port, b"GET /todos HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert!(listed.starts_with("HTTP/1.1 200") && listed.contains("buy milk"), "{listed}");
    assert!(raw_http(port, b"GET /nope HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").starts_with("HTTP/1.1 404"));

    // Over the body limit: refused on its Content-Length, nothing read.
    let large = raw_http(port, b"POST /todos HTTP/1.1\r\nHost: x\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n");
    assert!(large.starts_with("HTTP/1.1 413"), "{large}");

    // Headers that never finish: the connection is closed, not held.
    let started = std::time::Instant::now();
    let slow = raw_http(port, b"GET /todos HTTP/1.1\r\nHost: x\r\n");
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
    let listed = raw_http(port, b"GET /todos HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
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
    state.deployments_path = path.clone();
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
    let reloaded = load_deployments(&path, &state.store, &state.policy()).unwrap();
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
    let reloaded = load_deployments(&path, &state.store, &state.policy()).unwrap();
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

    let reloaded = load_deployments(&path, &state.store, &state.policy()).unwrap();
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
        let deployments = Arc::new(Mutex::new(load_deployments(&deployments_path, &store, &Policy::default()).unwrap()));
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
            data_limits: dataplane::DataLimits::default(),
            policy: std::sync::RwLock::new(Arc::new(Policy::default())),
        policy_path: ":none:".to_string(),
            secrets: Arc::new(SecretStore::memory()),
        access: access::Access::default(),
        activity: activity::Activity::memory(),
        editor: None,
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
    let reloaded = load_deployments(&deployments_path, &store, &Policy::default()).unwrap();
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
        data_limits: dataplane::DataLimits::default(),
        policy: std::sync::RwLock::new(Arc::new(Policy::default())),
        policy_path: ":none:".to_string(),
        secrets: Arc::new(SecretStore::memory()),
        access: access::Access::default(),
        activity: activity::Activity::memory(),
        editor: None,
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
    // what it actually says. This checks the one thing that matters: every method a real
    // endpoint in this file uses (GET, POST, DELETE) is actually named in the value.
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
    let v1 = stage_stateful_writer(&state, test_ingress(), "a");
    let v2 = stage_stateful_writer(&state, test_ingress(), "b");
    let deploy = |unit: Hash, key: &str, rev: u32| {
        let ack = serde_json::json!(["NetworkIngress", format!("StateWrite(\"{key}\")")]);
        deploy_json(&state, "v", unit, serde_json::json!({ "acknowledge": ack, "document": { "rev": rev } })).status_code
    };
    let fire = || route(&state, &req("POST", "/trigger/v", b"hello".to_vec())).status_code;
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
    assert!(String::from_utf8(body_bytes(gone)).unwrap().contains("no longer keeps"));

    // Kept: the last KEPT_VERSIONS, newest last, a version deployed again not kept twice.
    let mut versions = Vec::new();
    for n in 0..12 {
        crate::deploy::keep_version(&mut versions, format!("u{n}"), serde_json::json!(n));
    }
    crate::deploy::keep_version(&mut versions, "u5".into(), serde_json::json!("again"));
    let kept: Vec<&str> = versions.iter().map(|v| v.unit.as_str()).collect();
    assert_eq!(kept, ["u2", "u3", "u4", "u6", "u7", "u8", "u9", "u10", "u11", "u5"]);
}

/// A deployment's flow log is kept beside the deployments file: a restart reads it back, so a
/// past run can still be replayed, and deleting the deployment deletes it.
#[test]
fn a_flow_log_survives_a_restart_and_goes_with_its_deployment() {
    let mut state = test_state();
    let path = unique_temp_path("kept-log");
    state.deployments_path = path.clone();
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "kept", writer, ack).status_code, 200);
    for _ in 0..2 {
        assert_eq!(route(&state, &req("POST", "/trigger/kept", b"hello".to_vec())).status_code, 200);
    }
    let file = recorder::file_for(&path, "kept").unwrap();
    assert!(file.exists());

    let reloaded = load_deployments(&path, &state.store, &state.policy()).unwrap();
    let entries = reloaded["kept"].log.after(None, 10);
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
    state.deployments_path = path.clone();
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "chained", writer, ack).status_code, 200);
    for _ in 0..3 {
        assert_eq!(route(&state, &req("POST", "/trigger/chained", b"hello".to_vec())).status_code, 200);
    }
    let log = body_json(route(&state, &req("GET", "/deployments/chained/log", vec![])));
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries[0].get("prev"), None, "the first run has nothing before it");
    let file = recorder::file_for(&path, "chained").unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(entries[1]["prev"], recorder::line_hash(lines[0]));
    assert_eq!(log["head"], recorder::line_hash(lines[2]));
    assert_eq!(log["breaks"], serde_json::json!([]));
    assert!(load_deployments(&path, &state.store, &state.policy()).unwrap()["chained"].log.breaks().is_empty());

    // The first run's body changed in the file: the second no longer follows it.
    let changed = text.replacen("hello", "jello", 1);
    assert_ne!(changed, text, "the run's request is in the file");
    std::fs::write(&file, changed).unwrap();
    let reloaded = load_deployments(&path, &state.store, &state.policy()).unwrap();
    assert_eq!(reloaded["chained"].log.breaks(), [1]);

    route(&state, &req("POST", "/deployments/chained/stop", vec![]));
    route(&state, &req("DELETE", "/deployments/chained", vec![]));
    let _ = std::fs::remove_file(&path);
}

/// A server with accounts, through its routes: set up with the printed code, then signed in,
/// an invite accepted, each role doing what it may, and an admin's API token.
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
    let invite = body_json(call("POST", "/admin/users", serde_json::json!({ "name": "cy", "role": "viewer" }), Some(&ada)))["invite"].as_str().unwrap().to_string();
    assert_eq!(body_json(call("GET", &format!("/auth/invite?token={invite}"), null.clone(), None))["name"], "cy");
    let cy = body_json(call("POST", "/auth/invite", serde_json::json!({ "token": invite, "password": pw }), None))["token"].as_str().unwrap().to_string();
    assert_eq!(body_json(call("GET", "/auth/me", null.clone(), Some(&cy)))["role"], "viewer");
    assert_eq!(call("POST", "/flows", serde_json::json!({ "name": "x", "document": {} }), Some(&cy)).status_code, 403);
    assert_eq!(call("GET", "/admin/users", null.clone(), Some(&cy)).status_code, 403);
    // Signing in again, with the password cy set; then out.
    let again = body_json(call("POST", "/auth/sign-in", serde_json::json!({ "name": "cy", "password": pw }), None))["token"].as_str().unwrap().to_string();
    assert_eq!(call("POST", "/auth/sign-out", null.clone(), Some(&again)).status_code, 204);
    assert_eq!(call("GET", "/flows", null.clone(), Some(&again)).status_code, 401);

    // An admin makes cy an editor; an API token for CI writes, never the admin panel.
    assert_eq!(body_json(call("POST", "/admin/users/cy", serde_json::json!({ "role": "editor" }), Some(&ada)))["role"], "editor");
    assert_eq!(call("POST", "/flows", serde_json::json!({ "name": "x", "document": {} }), Some(&cy)).status_code, 200);
    let ci = body_json(call("POST", "/admin/tokens", serde_json::json!({ "label": "ci", "role": "editor" }), Some(&ada)))["token"].as_str().unwrap().to_string();
    assert_eq!(call("POST", "/flows", serde_json::json!({ "name": "y", "document": {} }), Some(&ci)).status_code, 200);
    assert_eq!(call("GET", "/admin/users", null.clone(), Some(&ci)).status_code, 403);
    let activity = body_json(call("GET", "/activity", null.clone(), Some(&ada)));
    let saved_by_ci = activity["entries"].as_array().unwrap().iter().any(|e| e["action"] == "saved" && e["by"] == "token:ci");
    assert!(saved_by_ci, "{activity}");
}

/// With accounts, a save and a deploy remember who made them, and every action is in the
/// activity log with who did it, a deleted deployment's included.
#[test]
fn what_is_done_names_who_did_it() {
    let mut state = test_state();
    let accounts = std::sync::Arc::new(crate::accounts::Accounts::memory());
    accounts.setup(&accounts.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
    let bob = accounts.accept(&accounts.invite("bob", access::Role::Editor, None, "ada").unwrap(), "correct horse battery").unwrap().0;
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

    let ingress = test_ingress();
    as_bob("POST", "/unit", encode_preimage(&ingress).unwrap());
    as_bob("POST", "/unit", encode_preimage(&test_sink()).unwrap());
    let flow = test_ingress_flow(hash_unit(&ingress).unwrap(), hash_unit(&test_sink()).unwrap());
    as_bob("POST", "/unit", encode_preimage(&flow).unwrap());
    let body = serde_json::json!({ "slug": "echo", "unit": to_hex(&hash_unit(&flow).unwrap()), "acknowledge": ["NetworkIngress", "NetworkEgress"] });
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

    let ingress = test_ingress();
    route(&state, &req("POST", "/unit", encode_preimage(&ingress).unwrap()));
    route(&state, &req("POST", "/unit", encode_preimage(&test_sink()).unwrap()));
    let flow = test_ingress_flow(hash_unit(&ingress).unwrap(), hash_unit(&test_sink()).unwrap());
    route(&state, &req("POST", "/unit", encode_preimage(&flow).unwrap()));
    let body = serde_json::json!({ "slug": "blog", "unit": to_hex(&hash_unit(&flow).unwrap()), "acknowledge": ["NetworkIngress", "NetworkEgress"], "uses": [{ "flow": "slugify", "saved_at": 7 }] });
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
        let request = data::DataRequest { method: method.into(), url: "/todos".into(), headers: vec![("Origin".into(), origin.into())], body: vec![] };
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

#[test]
fn at_takes_an_address_or_a_port_alone() {
    assert_eq!(parse_at("8080").unwrap().to_string(), "127.0.0.1:8080");
    assert_eq!(parse_at("0.0.0.0:7420").unwrap().to_string(), "0.0.0.0:7420");
    assert_eq!(parse_at("[::1]:9000").unwrap().to_string(), "[::1]:9000");
    for bad in ["", "localhost:8080", "8080:", "127.0.0.1", "70000"] {
        assert!(parse_at(bad).is_err(), "{bad:?}");
    }
}

/// A deployment's runs, exported and checked offline: every run runs again to what its entry
/// says. A copy re-chained after changing what a run performed still links, but that run no longer
/// re-runs to its entry; a witnessed head is found only in the copy it came from.
#[test]
fn exported_runs_verify_and_a_rewritten_log_doesnt() {
    let mut state = test_state();
    let path = unique_temp_path("verified-log");
    state.deployments_path = path.clone();
    let writer = stage_stateful_writer(&state, test_ingress(), "k");
    let ack = serde_json::json!({ "acknowledge": ["NetworkIngress", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, "verified", writer, ack).status_code, 200);
    for body in ["hello", "there", "again"] {
        assert_eq!(route(&state, &req("POST", "/trigger/verified", body.as_bytes().to_vec())).status_code, 200);
    }
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", "/deployments/verified/export", vec![])))).unwrap();
    assert_eq!(export.lines.len(), 3);
    let report = verify::check(&export, Some(&export.head));
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
    let report = verify::check(&forged, Some(&export.head));
    assert!(report.breaks.is_empty(), "re-chained, it links");
    assert_eq!(report.reruns[1], (1, verify::Rerun::Differs("the effects it performed".into())));
    assert_eq!(report.witnessed, Some(None), "the witnessed head isn't in the rewritten log");
    assert!(!report.ok());

    route(&state, &req("POST", "/deployments/verified/stop", vec![]));
    route(&state, &req("DELETE", "/deployments/verified", vec![]));
    let _ = std::fs::remove_file(&path);
}
