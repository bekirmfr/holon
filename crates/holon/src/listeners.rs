//! The data plane: answering a request to a deployed server unit, whether it came to the server's
//! own port (dataplane.rs) or from the editor's "send live" over the control plane; and keeping
//! one listener running per server deployment.

use crate::data::{DataRequest, DataResponse};
use crate::*;

/// Answer one data-plane request for the server deployment `slug`: route it, settle the matched
/// endpoint with its request record and the deployment's state, and send its response (see
/// `data.rs`). The deployment is looked up per request, so a redeploy on the same port takes
/// effect for the very next request without restarting the listener.
/// Answer one data-plane request to `slug`, as a browser may read it: a page from another site
/// is answered (CORS) only when the deployment says it answers pages from there (`browsers`),
/// its preflight then answered here, before the flow.
pub(crate) fn serve_data(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &Place,
    slug: &str,
    request: &DataRequest,
) -> DataResponse {
    let allowed = allowed_origin(deployments, slug, &request.headers);
    serve_data_forwarding(store, deployments, deployments_path, slug, request, None, allowed)
}

/// The origin a page sent the request from, when `slug` answers pages from there (`browsers`).
pub(crate) fn allowed_origin(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, headers: &[(String, String)]) -> Option<String> {
    let browsers = deployments.lock().unwrap().get(slug).map(|d| d.browsers.clone()).unwrap_or_default();
    let origin = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("origin")).map(|(_, v)| v.clone());
    origin.filter(|o| browsers.iter().any(|b| b == "*" || b == o))
}

/// [`serve_data`], a streamed call's answer forwarded to the caller as it arrives (`forward`)
/// when the route's run makes one. `allowed` is the request's origin when the deployment answers
/// pages from there (`allowed_origin`), worked out once by the caller, which needs it too.
#[allow(clippy::too_many_arguments)]
pub(crate) fn serve_data_forwarding(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &Place,
    slug: &str,
    request: &DataRequest,
    forward: Option<outbound::Forwarder>,
    allowed: Option<String>,
) -> DataResponse {
    if request.method == "OPTIONS"
        && let Some(origin) = &allowed
    {
        return DataResponse {
            status: 204,
            headers: vec![
                ("access-control-allow-origin".into(), origin.clone()),
                ("access-control-allow-methods".into(), "GET, POST, PUT, PATCH, DELETE".into()),
                ("access-control-allow-headers".into(), "Content-Type, Authorization".into()),
                ("access-control-max-age".into(), "600".into()),
                ("vary".into(), "Origin".into()),
            ],
            body: vec![],
            subscribe: None,
            file: None,
        };
    }
    let mut answer = serve_flow(store, deployments, deployments_path, slug, request, forward);
    if let Some(origin) = allowed {
        answer.headers.push(("access-control-allow-origin".into(), origin));
        answer.headers.push(("vary".into(), "Origin".into()));
    }
    answer
}

/// Whether `o` is `*` or an origin as a browser sends one: `http(s)://host[:port]`, no path.
pub(crate) fn is_origin_or_any(o: &str) -> bool {
    o == "*"
        || o.strip_prefix("https://").or_else(|| o.strip_prefix("http://")).is_some_and(|rest| {
            !rest.is_empty() && !rest.contains('/') && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b))
        })
}

/// The flow's own answer to a data-plane request.
/// A request's handler run, as `serve_flow` keeps it until it is logged or counted.
struct Ran {
    run: FiredRun,
    request: Option<Value>,
    route: String,
    /// What its entry's error is (before a park's): the run's, or a secret it wasn't approved to
    /// answer with. What the answer's checks read.
    error: Option<String>,
    /// Its entry, made as it ran when it is to be logged whatever comes of it (a recorded run).
    entry: Option<LogEntry>,
}

/// A request's entry, from its run: as the run made it, with its route, its request, and its
/// error.
/// Its timeline is moved out of the run: nothing reads the run's after.
fn ran_entry(started: (u64, std::time::Instant), sourced: &[(Capability, Value)], run: &mut FiredRun, route: &str, request: &Option<Value>, error: Option<String>) -> LogEntry {
    let mut entry = run_entry_taking("request", started, sourced, run);
    entry.error = error;
    entry.route = Some(route.to_string());
    entry.request = request.as_ref().map(SValue::from_core);
    entry
}

fn serve_flow(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &Place,
    slug: &str,
    request: &DataRequest,
    forward: Option<outbound::Forwarder>,
) -> DataResponse {
    #[cfg(test)]
    panic_for_tests::check(&request.url);
    let found = deployments
        .lock()
        .unwrap()
        .get(slug)
        .map(|d| (d.unit, d.state.clone(), d.server.clone(), d.life.status, Arc::clone(&d.permit), Arc::clone(&d.secrets), (Arc::clone(&d.folders), Arc::clone(&d.channels))));
    let Some((unit_hash, state, Some(server), status, permit, secrets, (folders, channels))) = found else {
        return DataResponse::json_error(503, &format!("{slug:?} is not deployed as a server"));
    };
    if status != Status::Running {
        return DataResponse::json_error(503, &format!("{slug:?} is {}", status_word(status)));
    }
    let resolver = match closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => return DataResponse::json_error(500, &format!("store error while materializing closure: {e:?}")),
    };
    let (method, raw_url, headers, body) = (request.method.as_str(), request.url.as_str(), &request.headers, &request.body);
    let epoch_seconds = clock_seconds();
    let sourced = [
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, random_bytes()),
    ];
    let started = started_now();
    let http = data::HttpRequest { method, url: raw_url, headers, body };

    // A resume route needs a run waiting for its token, or it answers nothing at all.
    let (path, query) = raw_url.split_once('?').unwrap_or((raw_url, ""));
    let mut resume: Option<(durable::Claimed, String, Value, String)> = None;
    // Only a flow with a resume route looks for one.
    if server.routes.iter().any(|r| r.resume.is_some())
        && let Some((route, params)) = holon_core::match_route(&server, method, path)
        && let Some(name) = &route.resume
    {
        let from_query = query.split('&').find_map(|kv| kv.strip_prefix("token=")).map(data::percent_decode);
        let token = params.get("token").map(|t| data::percent_decode(t)).or(from_query).unwrap_or_default();
        // Claimed now, so a second request carrying the same token while this one runs finds
        // nothing waiting: a token resumes its run once.
        let Some(claimed) = durable::claim_by_token(deployments, slug, name, &token) else {
            return DataResponse::json_error(404, "no run is waiting here for that token (it was used, or has expired)");
        };
        let params = params.into_iter().map(|(k, v)| (k, data::percent_decode(&v))).collect();
        let event = data::request_record(method, path, params, query, headers, &String::from_utf8_lossy(body));
        resume = Some((claimed, name.clone(), event, token));
    }

    // The handler's run, once it has run: its entry is made only if it is logged (an unrecorded
    // run that didn't fail is only counted, docs/L4s/interpreter-path-L4.md, the per-request
    // bookkeeping), and what the answer's checks read of it.
    let mut ran: Option<Ran> = None;
    // How its run is recorded: its route's level, or, a request no route took, lazy (C5).
    let mut level = holon_core::LogLevel::Lazy;
    // Whether its answer may be computed from a secret: the log keeps it withheld then.
    let mut answer_derived = false;
    // A secret its answer carries that it wasn't approved to answer with: the caller gets a 500.
    let mut unapproved: Option<String> = None;
    let mut waits = RunWaits { folders: Some(folders), forward, channels: Some(channels), slug: Some(slug.to_string()), ..Default::default() };
    let mut parked: Result<Vec<String>, String> = Ok(Vec::new());
    // A module in the flow runs through the engine; everything else resolves as before.
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let mut answer = data::handle(
        &server,
        &resolver,
        &http,
        |endpoint, matched, req, sourced| {
            // The route's label, its level and its endpoint's hash, as the server read them.
            let label = format!("{} {}", matched.method, matched.path);
            let route = label.as_str();
            level = matched.log;
            // A run that won't be logged keeps no timeline (§4.5 a), and is never logged, failed or
            // not: its tally keeps the last failure (resilience-audit-L4.md, §2.8).
            waits.unrecorded = permit.log_level(Some(route), level) == holon_core::LogLevel::Unrecorded;
            // One copy of the request, lent to the run, the check for a wait and the entry.
            let inputs = [Some(req)];
            let mut run = fire_with_state(endpoint, &inputs, &resolver, store, state.as_ref(), sourced, &permit, false, &secrets, &mut waits, None);
            parked = durable::park_if_waiting(deployments, store, slug, &matched.endpoint, "request", Some(route.to_string()), level, &inputs, sourced, &run, &waits);
            answer_derived = run.secret_fed.derives_output(0);
            // An answer carrying a secret goes to the caller only where a person approved it
            // (`SecretAnswer("name")`): one the graph says may carry it, or one that holds its
            // value as it is (a far side echoing a header back, which the graph can't see), in a
            // run that computed with a secret at all.
            let answered = run.outputs.first().and_then(Option::as_ref).filter(|_| !run.secret_fed.is_empty());
            let mut carried = answered.map(|a| secrets::secrets_in(a, &secrets)).unwrap_or_default();
            if run.secret_fed.reaches_output(0) {
                carried.extend(holon_core::answered_secrets(endpoint, &resolver));
            }
            unapproved = carried.into_iter().find(|name| !permit.allows(&Capability::SecretAnswer(name.clone())));
            let error = match &unapproved {
                Some(name) => Some(format!("refused to answer with the secret {name:?}: this deployment wasn't approved to answer its callers with it")),
                None => run.error.clone(),
            };
            let [request] = inputs;
            // The answer, and the run kept to make its entry from if it is logged: a recorded one's
            // entry is made now, and it gives its outputs up to the answer; an unrecorded one keeps
            // them, its answer a copy, in case it is logged after all.
            let entry = (!waits.unrecorded).then(|| ran_entry(started, sourced, &mut run, route, &request, error.clone()));
            let outputs = if waits.unrecorded { run.outputs.clone() } else { std::mem::take(&mut run.outputs) };
            ran = Some(Ran { run, request, route: route.to_string(), error, entry });
            outputs
        },
        &sourced,
    );
    if let Err(e) = &parked {
        answer = data::DataResponse::json_error(503, e);
    }
    if let Some(name) = &unapproved {
        answer = data::DataResponse::json_error(500, &format!("this deployment may not answer its callers with the secret {name:?}"));
    }
    // A subscription to a channel the deployment wasn't granted to let callers subscribe to.
    if let Some(s) = &answer.subscribe
        && !permit.allows(&Capability::ChannelRead(s.channel.clone()))
    {
        answer = data::DataResponse::json_error(500, &format!("this deployment may not let callers subscribe to the channel {:?}", s.channel));
    }
    // A run that waited as long as it may for a key another run holds is turned away, not
    // answered from nothing: its client may try again.
    if let Some(busy) = ran.as_ref().and_then(|r| r.error.as_deref()).filter(|e| e.starts_with("busy:")) {
        answer = data::DataResponse::json_error(503, busy);
    }
    // One whose file read failed (a name that leaves its folder, a file too big or not text) did
    // nothing: it says so, not "not found". Why is in the run log, not told to the caller.
    if ran.as_ref().and_then(|r| r.error.as_deref()).is_some_and(|e| e.starts_with(folders::READ_FAILED)) {
        answer = data::DataResponse::json_error(500, "a file this flow reads couldn't be read: its run log says why");
    }
    // One whose database read failed, or whose writes there didn't land, likewise; one whose
    // statement didn't change the rows it expects is the data saying no: nothing it did was written.
    let error = ran.as_ref().and_then(|r| r.error.as_deref());
    if error.is_some_and(|e| e.starts_with(databases::READ_FAILED)) {
        answer = data::DataResponse::json_error(500, "a database this flow reads couldn't be read: its run log says why");
    }
    if error.is_some_and(|e| e.starts_with(databases::WRITE_FAILED)) {
        answer = data::DataResponse::json_error(500, "a database this flow writes couldn't be written: its run log says why");
    }
    if let Some(unmet) = error.filter(|e| e.starts_with(databases::UNMET)) {
        answer = data::DataResponse::json_error(409, unmet);
    }
    // One whose file write didn't land (a folder past its cap, say) didn't do what it answered.
    if ran.as_ref().and_then(|r| r.error.as_deref()).is_some_and(|e| e.starts_with(folders::WRITE_FAILED)) {
        answer = data::DataResponse::json_error(500, "a file this flow writes couldn't be written: its run log says why");
    }
    // One whose packs were past its limits did nothing.
    if ran.as_ref().and_then(|r| r.error.as_deref()).is_some_and(|e| e.starts_with(channels::SEND_FAILED)) {
        answer = data::DataResponse::json_error(500, "this flow's packs were past its channel limits: its run log says why");
    }
    // A file it answered with, checked before its record is written, so the record says what was
    // sent: what went wrong is its entry's error.
    let mut file_error = None;
    if answer.file.is_some() {
        (answer, file_error) = serve_file(answer, &permit, waits.folders.as_deref(), &request.headers);
    }
    record_trigger(deployments, slug, answer.status >= 500);
    // Every request is logged, routed or not; an unrouted one (404, 405) never ran a handler. Its
    // entry is made only where it is logged: an unrecorded run that didn't fail is counted.
    let route = ran.as_ref().map(|r| r.route.clone());
    let error = match &parked {
        Err(e) => Some(e.clone()),
        Ok(_) => file_error.clone().or_else(|| ran.as_ref().and_then(|r| r.error.clone())),
    };
    let parked_any = matches!(&parked, Ok(names) if !names.is_empty());
    // Whether an unrecorded run sent packs or mail, which go out even though it isn't logged.
    let sends = level == holon_core::LogLevel::Unrecorded && ran.as_ref().is_some_and(|r| r.run.performed.iter().chain(&r.run.progress).any(|(c, _)| matches!(c, Capability::ChannelSend(_) | Capability::MailSend(_))));
    // The tokens its starts were handed for the runs they started (calls.rs): bearer credentials
    // as its own are, kept out of its log though its caller is answered with them.
    let handed = ran.as_ref().map_or_else(Vec::new, |r| calls::handed(&r.run.rounds));
    let entry = || {
        let mut entry = match ran {
            Some(Ran { entry: Some(entry), .. }) => entry,
            Some(Ran { mut run, request, route, error, .. }) => ran_entry(started, &sourced, &mut run, &route, &request, error),
            None => run_entry("request", started, &sourced, &FiredRun::nothing()),
        };
        entry.duration_ms = started.1.elapsed().as_secs_f64() * 1000.0;
        entry.method = Some(method.to_string());
        entry.url = Some(raw_url.to_string());
        entry.status = Some(answer.status);
        entry.response_body = Some(match &answer.file {
            _ if answer_derived => crate::taint::WITHHELD.to_string(),
            Some(file) => file.line(),
            None => recorder::body_text(&answer.body),
        });
        if file_error.is_some() {
            entry.error = file_error;
        }
        entry.by = request.by.clone();
        match parked {
            Ok(names) => entry.parked = names,
            Err(e) => entry.error = Some(e),
        }
        entry
    };
    let tokens = || {
        let tokens = Redactor::tokens(&waits.tokens).and(Redactor::handed(&handed));
        // A resume route's request carries the token it delivers (in its URL): redacted too.
        match &resume {
            Some((_, name, _, token)) => tokens.and(Redactor::tokens(&BTreeMap::from([(name.clone(), token.clone())]))),
            None => tokens,
        }
    };
    // A strict run answers once its record is on disk; one whose record may not be isn't
    // answered as done (C2 e).
    let counted = Counted { route: route.as_deref(), at_ms: started.0, duration_ms: started.1.elapsed().as_secs_f64() * 1000.0, status: answer.status, error: error.as_deref() };
    if let Some(Logged { kept: Err(e), .. }) = log_or_count(deployments, slug, counted, entry, tokens, level, sends) {
        answer = data::DataResponse::json_error(500, &format!("the request ran, but {e}"));
    }
    // The state is kept by its head, written as the change landed (claims.rs); the deployments
    // file changes only with the deployment's definition, here its waiting runs.
    if parked_any {
        persist_deployments(deployments_path, deployments);
    }
    // The route's own handler has answered; now the waiting run continues.
    if let Some((claimed, name, event, _)) = resume {
        let run_id = claimed.run_id().to_string();
        if let Err(e) = durable::resume_parked(store, deployments, deployments_path, slug, claimed, &name, event) {
            eprintln!("holon: resuming run {run_id} in {slug:?}: {e}");
        }
    }
    answer
}

/// A file a route answered with, sent (host-capabilities L4 §5.11.14): the deployment granted
/// `FileServe` of its folder, the file within `max_served_bytes`, a 304 for a cached copy that is
/// still the file's (`If-None-Match`), and the file still the handle's, or a 503 the caller may
/// try again. A small file's bytes are read and checked here; a big one is opened and checked by
/// size and time, and the transport streams it, checking its hash as it goes. With the error its
/// entry says, if any.
pub(crate) fn serve_file(mut answer: DataResponse, permit: &Permit, folders: Option<&folders::Scoped>, headers: &[(String, String)]) -> (DataResponse, Option<String>) {
    let Some(file) = answer.file.take() else { return (answer, None) };
    let path = format!("{}/{}", file.folder, file.name);
    if !permit.allows(&Capability::FileServe(file.folder.clone())) {
        let why = format!("this deployment may not answer its callers with files from the folder {:?}", file.folder);
        return (data::DataResponse::json_error(500, &why), Some(format!("refused: {why}")));
    }
    let max = permit.files.max_served_bytes;
    if file.size > max {
        let why = format!("{path} is {} bytes, more than the {max} a route answers with (max_served_bytes)", file.size);
        return (data::DataResponse::json_error(500, &why), Some(format!("refused: {why}")));
    }
    let if_none_match = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("if-none-match")).map(|(_, v)| v.as_str());
    if served_status(answer.status, &file, if_none_match, None) == 304 {
        answer.status = 304;
        answer.file = Some(file);
        return (answer, None);
    }
    let changed = || {
        let why = format!("{path} changed since the run read it");
        let mut r = data::DataResponse::json_error(503, &why);
        r.headers.push(("retry-after".into(), "1".into()));
        (r, Some(format!("changed: {why}")))
    };
    let Some(Ok(Some((f, meta)))) = folders.map(|s| s.open_file(&file.folder, &file.name)) else { return changed() };
    if meta.len() != file.size || folders::modified(&meta) != file.modified {
        return changed();
    }
    if file.size <= permit.files.inline_file_bytes {
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut std::io::Read::take(&f, file.size + 1), &mut bytes).is_err() || blake3::hash(&bytes).to_hex().as_str() != file.hash {
            return changed();
        }
        answer.body = bytes;
        answer.file = Some(file);
    } else {
        answer.file = Some(data::Served { opened: Some(Arc::new(f)), ..file });
    }
    (answer, None)
}

/// The status a file is answered with ([`serve_file`]), from the flow's, the request's
/// `If-None-Match` and the error its entry says the checks found: a 503 for a file that changed, a
/// 500 for one refused, a 304 for a 200 whose cached copy is still the file's (the header names
/// its hash, or any, `*`). What `holon verify` expects of a run that answered with a file.
pub(crate) fn served_status(status: u16, file: &data::Served, if_none_match: Option<&str>, error: Option<&str>) -> u16 {
    let cached = || {
        let mut tags = if_none_match.unwrap_or_default().split(',').map(|t| t.trim().trim_start_matches("W/"));
        tags.any(|t| t == "*" || t.strip_prefix('"').and_then(|t| t.strip_suffix('"')) == Some(file.hash.as_str()))
    };
    match error {
        Some(e) if e.starts_with("changed: ") => 503,
        Some(e) if e.starts_with("refused: ") => 500,
        _ if status == 200 && cached() => 304,
        _ => status,
    }
}

/// Bring the running listeners in line with the deployments table: stop any whose deployment is
/// gone or moved port, start one for each server deployment without one. Idempotent; called after
/// every deploy and undeploy, and once at startup. One that can't listen (its port is taken) is
/// marked with why (`listen_error`), so the editor shows it isn't answering; it is tried again
/// on the next call.
pub(crate) fn sync_listeners(state: &AppState) {
    let Some(listeners) = &state.listeners else { return };
    let wanted: BTreeMap<String, u16> = state
        .deployments
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, d)| d.life.status != Status::Stopped)
        .filter_map(|(slug, d)| d.server.as_ref().and_then(|s| s.port).map(|port| (slug.clone(), port)))
        .collect();
    let mut running = listeners.lock().unwrap();
    running.retain(|slug, (port, stop)| {
        let keep = wanted.get(slug) == Some(port);
        if !keep {
            let _ = stop.send(true);
        }
        keep
    });
    let mut failed = BTreeMap::new();
    for (slug, port) in wanted {
        if running.contains_key(&slug) {
            continue;
        }
        match dataplane::start_listener(state, &slug, port) {
            Ok(stop) => {
                running.insert(slug, (port, stop));
            }
            Err(e) => {
                let why = format!("could not listen on port {port}: {e}");
                eprintln!("holon: {slug:?} {why}");
                failed.insert(slug, why);
            }
        }
    }
    drop(running);
    for (slug, dep) in state.deployments.lock().unwrap().iter_mut() {
        dep.listen_error = failed.remove(slug);
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
pub(crate) fn handle_post_request(state: &AppState, slug: &str, request: &Request, by: Option<&str>) -> Response {
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
    let inner = DataRequest { method: send.method, url: send.url, headers: send.headers, body: send.body.into_bytes(), by: by.map(str::to_string) };
    let mut answer = serve_data(&state.store, &state.deployments, &state.deployments_path, slug, &inner);
    // A subscription streams on the deployment's own port; sent from here, it answers with the
    // packs a subscriber would get first.
    if let Some(s) = &answer.subscribe
        && let Some(d) = state.deployments.lock().unwrap().get(slug)
    {
        answer.body = d.channels.kept(&s.channel, s.kept).iter().map(|p| channels::event_text(p)).collect::<String>().into_bytes();
    }
    // A big file streams on the deployment's own port; sent from here, it answers with its line.
    if let Some(file) = answer.file.as_ref().filter(|f| f.opened.is_some()) {
        answer.body = file.line().into_bytes();
    }
    let headers = answer.headers.iter().map(|(k, v)| (k.to_string(), redactor.text(v))).collect();
    let body = redactor.text(&String::from_utf8_lossy(&answer.body));
    Response::json(&SentResponseBody { status: answer.status, headers, body })
}

/// A panic a test asks for, on requests whose path starts with one it names (R1's test).
#[cfg(test)]
pub(crate) mod panic_for_tests {
    use std::sync::Mutex;

    static PATHS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    pub(crate) fn on(path: &str) {
        PATHS.lock().unwrap().push(path.to_string());
    }

    pub(crate) fn off() {
        PATHS.lock().unwrap().clear();
    }

    pub(super) fn check(url: &str) {
        let hit = PATHS.lock().unwrap().iter().any(|p| url.starts_with(p.as_str()));
        if hit {
            panic!("a panic a test asked for");
        }
    }
}
