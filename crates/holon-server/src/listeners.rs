//! The data plane: one listener per deployed server unit, answering on its own port, and the
//! editor's "send live", which reaches the same handler over the control plane.

use crate::*;

/// Answer one data-plane request for the server deployment `slug`: route it, settle the matched
/// endpoint with its request record and the deployment's state, and send its response (see
/// `data.rs`). The deployment is looked up per request, so a redeploy on the same port takes
/// effect for the very next request without restarting the listener.
pub(crate) fn serve_data(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: &str,
    request: &Request,
) -> Response {
    let found = deployments
        .lock()
        .unwrap()
        .get(slug)
        .map(|d| (d.unit, d.state.clone(), d.server.clone(), d.life.status, Arc::clone(&d.permit), Arc::clone(&d.secrets)));
    let Some((unit_hash, state, Some(server), status, permit, secrets)) = found else {
        return err(503, format!("{slug:?} is not deployed as a server"));
    };
    if status != Status::Running {
        return err(503, format!("{slug:?} is {}", status_word(status)));
    }
    let body = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => return err(500, format!("store error while materializing closure: {e:?}")),
    };
    let headers: Vec<(String, String)> = request.headers().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let epoch_seconds = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let sourced = [
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Number(rand::random::<f64>())),
    ];
    let started = started_now();
    let http = data::HttpRequest { method: request.method(), url: request.raw_url(), headers: &headers, body: &body };

    // A resume route needs a run waiting for its token, or it answers nothing at all.
    let raw_url = request.raw_url();
    let (path, query) = raw_url.split_once('?').unwrap_or((raw_url, ""));
    let mut resume: Option<(String, String, Value, String)> = None;
    if let Some((route, params)) = holon_core::match_route(&server, request.method(), path)
        && let Some(name) = &route.resume
    {
        let from_query = query.split('&').find_map(|kv| kv.strip_prefix("token=")).map(data::percent_decode);
        let token = params.get("token").map(|t| data::percent_decode(t)).or(from_query).unwrap_or_default();
        let Some(run_id) = durable::find_by_token(deployments, slug, name, &token) else {
            return err(404, "no run is waiting here for that token (it was used, or has expired)");
        };
        let params = params.into_iter().map(|(k, v)| (k, data::percent_decode(&v))).collect();
        let event = data::request_record(request.method(), path, params, query, &headers, &String::from_utf8_lossy(&body));
        resume = Some((run_id, name.clone(), event, token));
    }

    let mut ran: Option<LogEntry> = None;
    let mut waits = RunWaits::default();
    let mut parked: Result<Vec<String>, String> = Ok(Vec::new());
    let mut answer = data::handle(
        &server,
        &resolver,
        &http,
        |endpoint, route, req, sourced| {
            let run = fire_with_state(endpoint, &[Some(req.clone())], &resolver, state.as_deref(), sourced, &permit, false, &secrets, &mut waits, None);
            let unit = hash_unit(endpoint).unwrap_or([0; 32]);
            parked = durable::park_if_waiting(deployments, store, slug, &unit, "request", Some(route.to_string()), &[Some(req.clone())], sourced, &run, &waits);
            let mut entry = run_entry("request", started, sourced, &run);
            entry.route = Some(route.to_string());
            entry.request = Some(SValue::from_core(&req));
            ran = Some(entry);
            run.outputs
        },
        &sourced,
    );
    if let Err(e) = &parked {
        answer = data::DataResponse::json_error(503, e);
    }
    record_trigger(deployments, slug, answer.status >= 500);
    // Every request is logged, routed or not; an unrouted one (404, 405) never ran a handler.
    let mut entry = ran.unwrap_or_else(|| run_entry("request", started, &sourced, &FiredRun::nothing()));
    entry.duration_ms = started.1.elapsed().as_secs_f64() * 1000.0;
    entry.method = Some(request.method().to_string());
    entry.url = Some(request.raw_url().to_string());
    entry.status = Some(answer.status);
    entry.response_body = Some(recorder::body_text(&answer.body));
    match parked {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    Redactor::tokens(&waits.tokens).entry(&mut entry);
    // A resume route's request carries the token it delivers (in its URL): redacted too.
    if let Some((_, name, _, token)) = &resume {
        Redactor::tokens(&BTreeMap::from([(name.clone(), token.clone())])).entry(&mut entry);
    }
    let parked_any = !entry.parked.is_empty();
    log_run(deployments, slug, entry);
    if state.is_some() || parked_any {
        persist_deployments(deployments_path, deployments, store);
    }
    // The route's own handler has answered; now the waiting run continues.
    if let Some((run_id, name, event, _)) = resume
        && let Err(e) = durable::resume_parked(store, deployments, deployments_path, slug, &run_id, &name, event)
    {
        eprintln!("holon-server: resuming run {run_id} in {slug:?}: {e}");
    }
    Response {
        status_code: answer.status,
        headers: answer.headers.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
        data: rouille::ResponseBody::from_data(answer.body),
        upgrade: None,
    }
}

/// Start the data-plane listener for `slug` on `127.0.0.1:port`, returning its stop signal.
/// Loopback only for now: before a data plane faces the internet it moves to a maintained async
/// stack with timeouts and connection limits (docs/PLATFORM_LAYOUT.md §7b).
pub(crate) fn start_listener(state: &AppState, slug: &str, port: u16) -> Result<std::sync::mpsc::Sender<()>, String> {
    let store = state.store.clone();
    let deployments = Arc::clone(&state.deployments);
    let path = state.deployments_path.clone();
    let owner = slug.to_string();
    let server = rouille::Server::new(("127.0.0.1", port), move |request| {
        serve_data(&store, &deployments, &path, &owner, request)
    })
    .map_err(|e| e.to_string())?;
    let (_thread, stop) = server.stoppable();
    println!("holon-server: {slug:?} answering on http://127.0.0.1:{port}");
    Ok(stop)
}

/// Bring the running listeners in line with the deployments table: stop any whose deployment is
/// gone or moved port, start one for each server deployment without one. Idempotent; called after
/// every deploy and undeploy, and once at startup.
pub(crate) fn sync_listeners(state: &AppState) {
    let Some(listeners) = &state.listeners else { return };
    let wanted: BTreeMap<String, u16> = state
        .deployments
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, d)| d.life.status != Status::Stopped)
        .filter_map(|(slug, d)| d.server.as_ref().map(|s| (slug.clone(), s.port)))
        .collect();
    let mut running = listeners.lock().unwrap();
    running.retain(|slug, (port, stop)| {
        let keep = wanted.get(slug) == Some(port);
        if !keep {
            let _ = stop.send(());
        }
        keep
    });
    for (slug, port) in wanted {
        if running.contains_key(&slug) {
            continue;
        }
        match start_listener(state, &slug, port) {
            Ok(stop) => {
                running.insert(slug, (port, stop));
            }
            Err(e) => eprintln!("holon-server: {slug:?} could not listen on port {port}: {e}"),
        }
    }
}

/// `POST /deployments/<slug>/request`'s body: one HTTP request to send to that server deployment.
#[derive(Deserialize)]
pub(crate) struct SendRequestBody {
    pub(crate) method: String,
    /// Path plus optional query string, e.g. `/todos/3?full=1`.
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) headers: Vec<(String, String)>,
    #[serde(default)]
    pub(crate) body: String,
}

/// The data plane's answer, as `POST /deployments/<slug>/request` returns it.
#[derive(Serialize)]
pub(crate) struct SentResponseBody {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    /// The response body as text (lossily, if it isn't UTF-8).
    pub(crate) body: String,
}

/// `POST /deployments/<slug>/request` — send one request to a deployed server and return its
/// answer: the editor's request tester ("send live"). It goes through `serve_data`, exactly as a
/// request on the server's own port would (same routing, same state, counted the same), but
/// arrives over the control plane, so the browser needs no CORS access to the data plane and the
/// control plane's request gate still applies.
pub(crate) fn handle_post_request(state: &AppState, slug: &str, request: &Request) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let send: SendRequestBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("not a valid request JSON: {e}")),
    };
    if !send.url.starts_with('/') {
        return err(400, "url must start with / (a path, plus an optional ?query)");
    }
    let redactor = match state.deployments.lock().unwrap().get(slug) {
        None => return err(404, format!("no deployment {slug:?}")),
        Some(d) if d.server.is_none() => return err(400, format!("{slug:?} is not an HTTP server deployment")),
        Some(d) => Redactor::of(&d.secrets),
    };
    let inner = Request::fake_http(send.method, send.url, send.headers, send.body.into_bytes());
    let answer = serve_data(&state.store, &state.deployments, &state.deployments_path, slug, &inner);
    let headers = answer.headers.iter().map(|(k, v)| (k.to_string(), redactor.text(v))).collect();
    let status = answer.status_code;
    let (mut reader, _) = answer.data.into_reader_and_size();
    let mut body = Vec::new();
    if reader.read_to_end(&mut body).is_err() {
        return err(500, "could not read the server's response");
    }
    Response::json(&SentResponseBody { status, headers, body: redactor.text(&String::from_utf8_lossy(&body)) })
}
