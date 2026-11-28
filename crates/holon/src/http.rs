//! The control plane's HTTP edge: who may call it (Host, Origin, Content-Type), the body cap,
//! CORS, and `route`, which dispatches every request to its handler.

use crate::*;
use crate::access::{Access, Caller};

pub(crate) fn err(status: u16, message: impl Into<String>) -> Response {
    Response::json(&ErrorBody { error: message.into() }).with_status_code(status)
}

/// Whether `origin` is a localhost origin this server will answer CORS for. Scoped
/// deliberately narrow — see the L4's Fork 1: this server is not meant to be reached from
/// anywhere but the editor's own dev server on the same machine.
pub(crate) fn is_allowed_origin(origin: &str) -> bool {
    ["http://localhost", "http://127.0.0.1"].iter().any(|base| match origin.strip_prefix(base) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix(':')
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    })
}

/// The largest request body any endpoint accepts. Units and flow texts are kilobytes, a big
/// flow's text under a megabyte (a deploy sends the text, docs/L4s/text-deploy-plan.md); this only
/// exists so one oversized POST can't exhaust memory.
pub(crate) const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// The control plane's request gate, checked before any routing. CORS headers only decide
/// whether a page may *read* a response, never whether the request runs, so without this any
/// web page the user visits could drive the API:
///
/// - **Host** must name this server on loopback (`localhost`, `127.0.0.1` or `[::1]` at the
///   listening port). A DNS-rebinding page reaches 127.0.0.1 under its own hostname, and would
///   otherwise be same-origin with full access.
/// - **Origin**, when a browser sends one, must be an allowed localhost origin. Browsers attach
///   it to every cross-origin POST/DELETE, including "simple" requests that skip preflight.
/// - **Content-Type** of a POST must be `application/json` or `application/octet-stream`,
///   neither of which is CORS-safelisted, so a cross-origin POST always needs a preflight this
///   server never grants. Defense in depth behind the Origin check.
///
/// Clients without a browser (curl, tests) send no Origin and pass by setting the two headers.
/// Then the token, when one is configured (access.rs). A host or origin named with
/// `--allow-host` / `--allow-origin` passes too: a server reached beyond loopback, behind a token.
/// Answers who the request comes from (access.rs), when it may go on.
pub(crate) fn reject_untrusted(request: &Request, port: u16, access: &Access) -> Result<Caller, Response> {
    let host = request.header("Host").unwrap_or("");
    if !access.allows_host(host, port) {
        return Err(err(403, format!("Host {host:?} is not this server on loopback")));
    }
    if let Some(origin) = request.header("Origin")
        && !access.allows_origin(origin)
    {
        return Err(err(403, format!("Origin {origin:?} is not allowed")));
    }
    if matches!(request.method(), "POST" | "PUT" | "PATCH") {
        let media_type = request.header("Content-Type").unwrap_or("").split(';').next().unwrap_or("").trim();
        if !["application/json", "application/octet-stream"].iter().any(|t| media_type.eq_ignore_ascii_case(t)) {
            return Err(err(415, format!("{} bodies must be sent as application/json or application/octet-stream", request.method())));
        }
    }
    access.admit(request)
}

/// Add CORS headers to every response, in one place — so a new endpoint can never forget them
/// (the class of bug the `pub use` re-export list and the `WEncapError` variant drift are both
/// named for elsewhere in this project: a thing that must be repeated per call site eventually
/// isn't repeated somewhere).
pub(crate) fn with_cors(access: &Access, request: &Request, mut response: Response) -> Response {
    if let Some(origin) = request.header("Origin")
        && access.allows_origin(origin) {
            response = response
                .with_additional_header("Access-Control-Allow-Origin", origin.to_string())
                .with_additional_header("Access-Control-Allow-Methods", "GET, POST, PUT, PATCH, DELETE, OPTIONS")
                .with_additional_header("Access-Control-Allow-Headers", "Content-Type, Content-Encoding, Authorization")
                // A preflight's answer kept ten minutes (unstated, five seconds): a page on
                // another origin (the editor in development, an --allow-origin console) opens
                // fewer new connections, each one the server's HTTP library may leave unread
                // on a busy machine until another closes (the editor gives a read up and asks
                // again on another: server.ts's ANSWER_WITHIN). Kept per address, as browsers
                // key it, so a poll whose query moves still asks each time.
                .with_additional_header("Access-Control-Max-Age", "600")
                .with_additional_header("Vary", "Origin");
        }
    response
}

/// What every control-plane response is sent with: no other page may show it in a frame (the
/// editor, framed by a hostile page while signed in, could be lured into a click on Approve), with
/// `X-Frame-Options` for browsers that don't read `frame-ancestors`; its type isn't guessed past
/// the one it is sent as; and no address of it goes on to where its links lead.
pub(crate) const GUARDS: [(&str, &str); 4] = [
    ("X-Frame-Options", "DENY"),
    ("Content-Security-Policy", "frame-ancestors 'none'"),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
];

/// `response` with the `GUARDS`, added in one place as CORS's headers are (`with_cors`), so a new
/// endpoint or an early refusal can't go out without them. One it already names is kept: the
/// editor's page has a policy of its own (editor.rs `PAGE_POLICY`), which forbids frames too.
pub(crate) fn guarded(response: Response) -> Response {
    GUARDS.iter().fold(response, |response, &(name, value)| {
        if response.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name)) {
            response
        } else {
            response.with_additional_header(name, value)
        }
    })
}

/// How long a request's body may take to arrive, all of it. Past it the client is told 408 and
/// the request lets go of its admission (`Slots`, main.rs), so bodies that trickle in or stop
/// can't hold every slot and lock everyone out, signing in included. The data plane's is its
/// `body_timeout`, the same 30 seconds. The tests hold it too: under a short one, any test that
/// sends a body failed with 408 on a loaded machine (102 of them in one run, audit R50); the tests
/// of the deadline itself set one of their own ([`in_test::with_body_deadline`]).
pub(crate) const BODY_DEADLINE: Duration = Duration::from_secs(30);

/// The deadline a body is read by on this thread: [`BODY_DEADLINE`], or a test's own.
fn body_deadline() -> Duration {
    #[cfg(test)]
    if let Some(deadline) = in_test::DEADLINE.with(std::cell::Cell::get) {
        return deadline;
    }
    BODY_DEADLINE
}

/// Every body the control plane reads, read by the deadline and the cap. Sent as it is: a body sent
/// compressed (`Content-Encoding`) is refused here; only the routes that take a flow read one
/// ([`read_flow_body`]).
pub(crate) fn read_body(request: &Request) -> Result<Vec<u8>, Response> {
    if encoding_of(request).is_some() {
        return Err(err(415, "this route takes its body as it is, not encoded (Content-Encoding)"));
    }
    read_plain(request)
}

/// The body's `Content-Encoding`, when it names one (`identity` names none).
fn encoding_of(request: &Request) -> Option<String> {
    request.header("Content-Encoding").map(|e| e.trim().to_ascii_lowercase()).filter(|e| !e.is_empty() && e != "identity")
}

/// The body of a request that sends a flow (a deploy, its preview, a save), which may come gzipped
/// (`Content-Encoding: gzip`, docs/L4s/text-deploy-plan.md): sent or unpacked, it is held to
/// [`MAX_BODY_BYTES`], unpacking stopped as it passes that, so no body is ever larger here than a
/// plain one may be. Another encoding, a gzip that doesn't unpack and one past the cap are each
/// refused in their own words.
pub(crate) fn read_flow_body(request: &Request) -> Result<Vec<u8>, Response> {
    match encoding_of(request).as_deref() {
        None => read_plain(request),
        Some("gzip") => {
            let packed = read_plain(request)?;
            holon_core::gunzip_bounded(&packed, MAX_BODY_BYTES as usize).map_err(|why| match why {
                holon_core::Unpacked::Broken => err(400, "the body says it is gzipped, and it doesn't unpack as gzip"),
                holon_core::Unpacked::TooLarge => err(413, format!("request body exceeds {MAX_BODY_BYTES} bytes, unpacked")),
            })
        }
        Some(_) => Err(err(415, "a flow's body is sent as it is or gzipped (Content-Encoding: gzip), not otherwise encoded")),
    }
}

/// [`read_body`], whatever the body says of its encoding.
fn read_plain(request: &Request) -> Result<Vec<u8>, Response> {
    // Read on a thread of its own, from a copy of the request that shares its body: one that
    // trickles or stops is left to that thread and its connection, and this request answers at
    // the deadline. Given up on, the reader stops at its next read, so the 408 can be written
    // (rouille holds the connection for each read).
    let owned = request.remove_prefix("").expect("every url starts with the empty text");
    let given_up = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = Arc::clone(&given_up);
    let (sent, got) = std::sync::mpsc::sync_channel(1);
    let reader = thread::Builder::new().name("body".into()).spawn(move || {
        let read = owned.data().map(|body| {
            let mut bytes = Vec::new();
            // One byte past the cap is enough to tell "too large" apart from "exactly at the cap".
            Until { inner: body, stop }.take(MAX_BODY_BYTES + 1).read_to_end(&mut bytes).map(|_| bytes)
        });
        let _ = sent.send(read);
    });
    if reader.is_err() {
        return Err(err(503, "the server can't read another request's body now: try again in a moment"));
    }
    let deadline = body_deadline();
    match got.recv_timeout(deadline) {
        Ok(None) => Err(err(400, "body already consumed")),
        Ok(Some(Ok(bytes))) if bytes.len() as u64 > MAX_BODY_BYTES => Err(err(413, format!("request body exceeds {MAX_BODY_BYTES} bytes"))),
        Ok(Some(Ok(bytes))) => Ok(bytes),
        Ok(Some(Err(_))) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(err(400, "failed to read request body")),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            given_up.store(true, std::sync::atomic::Ordering::Release);
            Err(err(408, format!("the request's body took more than {} seconds to arrive", deadline.as_secs_f64())).with_additional_header("Connection", "close"))
        }
    }
}

/// A body read until it is given up on: then each read fails, the one under way once it returns.
struct Until<R> {
    inner: R,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl<R: Read> Read for Until<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.stop.load(std::sync::atomic::Ordering::Acquire) {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.inner.read(buf)
    }
}

/// `response`, with what it did recorded in the activity log when it succeeded.
fn recorded(state: &AppState, response: Response, action: &str, target: &str, by: Option<&str>) -> Response {
    if response.is_success() {
        state.activity.record(action, target, by);
    }
    response
}

pub(crate) fn route(state: &AppState, request: &Request) -> Response {
    // `request.url()` returns an owned `String` (not a borrow of `request`) — bound once here
    // rather than called again per branch, both so `strip_prefix` below has something with a
    // long enough lifetime to borrow from, and so the routing table has one clear read of "what
    // path is this."
    let url = request.url();
    let method = request.method();

    // The editor's own files (editor.rs), when it's served here: open, as any web page is.
    if method == "GET"
        && let Some(source) = &state.editor
        && let Some(page) = crate::editor::serve(source, request.raw_url())
    {
        return page;
    }

    let caller = match reject_untrusted(request, state.port, &state.access) {
        Ok(caller) => caller,
        Err(rejection) => return rejection,
    };
    let by = caller.name.as_deref();
    // What it records says it came through the run file, when it did (D45).
    let _via = crate::activity::Via::set(caller.local.as_deref());

    // Preflight — answered before touching any endpoint logic.
    if method == "OPTIONS" {
        return Response::empty_204();
    }

    // Roles, groups and what is bound to whom, with its versions, and what the caller may do
    // (bindings.rs): before auth.rs, which answers every other `/auth/…` path.
    if let Some(response) = crate::bindings::route(state, request, &caller) {
        return response;
    }

    // Signing in, and the admin panel (auth.rs).
    if let Some(response) = crate::auth::route(state, request, &caller) {
        return response;
    }

    // "ok", or 503 "not ok: " and what fails, without an error's text, to anyone (failing.rs).
    if method == "GET" && url == "/health" {
        let (status, says) = crate::failing::health(&state.deployments_path);
        return Response::text(says).with_status_code(status);
    }

    // The version this host runs, for the console's host card.
    if method == "GET" && url == "/version" {
        return Response::text(env!("CARGO_PKG_VERSION"));
    }

    if let Some(hash_hex) = url.strip_prefix("/unit/")
        && method == "GET" {
            return handle_get_unit(state, &caller, hash_hex);
        }

    if method == "POST" && url == "/unit" {
        return handle_post_unit(&state.store, request);
    }

    if method == "POST" && url == "/deploy" {
        let response = handle_post_deploy(state, request, by, &caller);
        sync_listeners(state);
        return response;
    }

    if url == "/flows" {
        if method == "POST" {
            return handle_post_flows(state, request, &caller);
        }
        if method == "GET" {
            return handle_get_flows(state, &caller);
        }
    }

    if let Some(name) = url.strip_prefix("/flows/") {
        if method == "GET" {
            return handle_get_flow(state, name, &caller);
        }
        if method == "DELETE" {
            return recorded(state, handle_delete_flow(state, name, &caller), "deleted flow", name, by);
        }
    }

    // Versions waiting for a person, and what became of the others (proposals.rs).
    if method == "GET" && url == "/proposals" {
        return crate::proposals::handle_get_proposals(state, &caller);
    }
    if let Some(rest) = url.strip_prefix("/proposals/") {
        if let Some(id) = rest.strip_suffix("/decline")
            && method == "POST"
        {
            return crate::proposals::handle_post_decline(state, id, request, &caller);
        }
        if method == "GET" && !rest.contains('/') {
            return crate::proposals::handle_get_proposal(state, rest, &caller);
        }
    }

    // The host at a glance (the console's Overview), and its run logs checked again.
    if method == "GET" && url.split('?').next() == Some("/summary") {
        return crate::summary::handle_get_summary(state, request, &caller);
    }
    if method == "POST" && url == "/summary/check" {
        return crate::summary::handle_post_check(state, &caller);
    }

    if method == "GET" && url.split('?').next() == Some("/activity") {
        return crate::activity::handle_get_activity(&state.activity, request, &caller);
    }

    if method == "POST" && url == "/deploy/preview" {
        return handle_post_deploy_preview(state, request, &caller);
    }

    if method == "GET" && url == "/permissions" {
        return handle_get_permissions(&state.policy(), &caller);
    }

    if method == "POST" && url == "/permissions/reload" {
        return grants::handle_post_permissions_reload(state, &caller);
    }

    if method == "GET" && url == "/deployments" {
        return handle_get_deployments(&state.deployments, &caller);
    }

    if method == "GET" && url == "/runs/waiting" {
        return durable::handle_get_waiting(&state.deployments, &caller, &durable::Rules::of(state));
    }

    if let Some(rest) = url.strip_prefix("/deployments/") {
        if let Some(slug) = rest.strip_suffix("/secrets") {
            return handle_secrets(&state.secrets, &state.deployments, slug, None, method, request, by);
        }
        if let Some((slug, name)) = rest.split_once("/secrets/") {
            return handle_secrets(&state.secrets, &state.deployments, slug, Some(name), method, request, by);
        }
        if let Some((slug, name)) = rest.split_once("/collections/")
            && method == "GET"
        {
            return collections::handle_get_collection(state, slug, name, request);
        }
        if let Some(slug) = rest.strip_suffix("/runs")
            && method == "GET"
        {
            return durable::handle_get_runs(&state.deployments, slug, &caller, &durable::Rules::of(state));
        }
        if let Some(slug) = rest.strip_suffix("/logs")
            && method == "GET"
        {
            return logs::handle_get_logs(state, slug, &caller, request);
        }
        if let Some(slug) = rest.strip_suffix("/feeds")
            && method == "GET"
        {
            return feeds::handle_get_feeds(state, slug, crate::can::sees_where(&caller));
        }
        if let Some(slug) = rest.strip_suffix("/queues")
            && method == "GET"
        {
            return queues::handle_get_queues(state, slug, crate::can::sees_where(&caller));
        }
        if let Some(slug) = rest.strip_suffix("/channels")
            && method == "GET"
        {
            return channels::handle_get_channels(state, slug);
        }
        if let Some((slug, run_id)) = rest.strip_suffix("/decide").and_then(|r| r.split_once("/runs/"))
            && method == "POST"
        {
            return durable::handle_post_decide(state, slug, run_id, &caller, request);
        }
        if let Some(slug) = rest.strip_suffix("/changes")
            && method == "GET"
        {
            return changes::handle_get_changes(state, slug, request);
        }
        if let Some(slug) = rest.strip_suffix("/log/heads")
            && method == "GET"
        {
            return handle_get_log_heads(&state.deployments, slug, request);
        }
        if let Some(slug) = rest.strip_suffix("/history") {
            if method == "GET" {
                return handle_get_history(&state.deployments, slug);
            }
        } else if let Some(slug) = rest.strip_suffix("/state") {
            if method == "GET" {
                return handle_get_state(&state.deployments, slug);
            }
        } else if let Some(slug) = rest.strip_suffix("/log") {
            if method == "GET" {
                return handle_get_log(&state.deployments, slug, request);
            }
        } else if let Some(slug) = rest.strip_suffix("/export") {
            if method == "GET" {
                return verify::handle_get_export(&state.store, &state.folders, &state.deployments, slug, request);
            }
        } else if let Some(slug) = rest.strip_suffix("/replay") {
            if method == "GET" {
                return verify::handle_get_replay(&state.store, &state.folders, &state.deployments, &state.replays, slug, request);
            }
        } else if let Some(slug) = rest.strip_suffix("/text") {
            if method == "GET" {
                return handle_get_text(&state.store, &state.deployments, slug, request);
            }
        } else if let Some(slug) = rest.strip_suffix("/events") {
            if method == "GET" {
                return crate::events::handle_get_events(&state.deployments, slug);
            }
        } else if let Some(slug) = rest.strip_suffix("/versions") {
            if method == "GET" {
                return handle_get_versions(&state.deployments, slug);
            }
        } else if let Some(slug) = rest.strip_suffix("/holds") {
            if method == "GET" {
                return crate::storage::handle_get_holds(state, slug);
            }
        } else if let Some(slug) = rest.strip_suffix("/restore") {
            if method == "POST" {
                let response = handle_post_restore(state, slug, request, by, &caller);
                sync_listeners(state);
                return response;
            }
        } else if let Some((slug, which)) = rest.rsplit_once('/')
            && matches!(which, "manifest" | "grant")
        {
            if method == "GET" {
                return handle_get_permission_view(&state.store, &state.deployments, slug, which);
            }
            if method == "POST" && which == "grant" {
                return recorded(state, grants::handle_post_grant(state, slug, request, &caller), "granted", slug, by);
            }
        } else if let Some(slug) = rest.strip_suffix("/sessions") {
            if method == "GET" {
                return handle_get_sessions(&state.deployments, &state.store, slug);
            }
        } else if let Some((slug, action)) = rest.rsplit_once('/')
            && matches!(action, "run" | "pause" | "stop")
        {
            if method == "POST" {
                let response = handle_lifecycle(state, slug, action, request, by);
                sync_listeners(state);
                let done = match action {
                    "run" => "run",
                    "pause" => "paused",
                    _ => "stopped",
                };
                return recorded(state, response, done, slug, by);
            }
        } else if let Some(slug) = rest.strip_suffix("/request") {
            if method == "POST" {
                return handle_post_request(state, slug, request, by);
            }
        } else if let Some(slug) = rest.strip_suffix("/settings") {
            if method == "POST" {
                return crate::proposals::handle_post_settings(state, slug, request, &caller);
            }
        } else if let Some(slug) = rest.strip_suffix("/tick") {
            if method == "POST" {
                return scheduler::handle_post_tick(state, slug, request, &caller);
            }
        } else if method == "DELETE" {
            // Runs given up removing it (`?give_up=`: what of its log can't be read) are an
            // admin's word, beside flow.delete: it is the one way, but retention, that run
            // records go (release audit R110). An agent holds neither (can.rs `fixed`).
            let give_up = match request.get_param("give_up").map(|text| crate::deploy::parse_give_up(&text)) {
                None => Vec::new(),
                Some(Ok(spans)) => spans,
                Some(Err(e)) => return err(400, e),
            };
            if !give_up.is_empty()
                && let Err(refusal) = crate::can::can(&caller, crate::can::Action::HostOperate, &crate::can::Resource::Host)
            {
                return refusal.response();
            }
            let mut given = Vec::new();
            let response = handle_delete_deployment(
                &state.store,
                &state.folders,
                &state.deployments,
                &state.deployments_path,
                rest,
                crate::can::sees_data(&caller),
                &give_up,
                by,
                &mut given,
            );
            // Deleted, it is on no tag.
            if response.status_code == 200
                && let Some(accounts) = state.access.accounts()
            {
                accounts.set_tags(rest, &[]);
            }
            if response.status_code == 200
                && let Err(e) = state.secrets.forget(rest, by)
            {
                eprintln!("warning: {rest:?}'s secrets weren't removed: {e}");
            }
            sync_listeners(state);
            if response.is_success() && !given.is_empty() {
                // By number alone: the activity is read by whoever views the flow.
                let they = if matches!(given.as_slice(), [(a, b)] if a == b) { "it" } else { "they" };
                let note = format!("gave up {}: {they} couldn't be read", crate::deploy::runs_said(&given));
                state.activity.record_noted("deleted", rest, by, Some(&note));
                return response;
            }
            return recorded(state, response, "deleted", rest, by);
        }
    }

    // `/trigger/<slug>/<name>` runs one of its actions by hand.
    if let Some(rest) = url.strip_prefix("/trigger/")
        && method == "POST"
    {
        return match rest.split_once('/') {
            Some((slug, name)) => handle_post_action(&state.store, &state.deployments, &state.deployments_path, slug, name, request, by, &caller),
            None => unnamed_action(&state.deployments, rest),
        };
    }

    Response::empty_404()
}

#[cfg(test)]
pub(crate) mod in_test {
    use std::cell::Cell;
    use std::time::Duration;

    thread_local!(pub(super) static DEADLINE: Cell<Option<Duration>> = const { Cell::new(None) });

    /// `f`, its bodies read by `deadline` (on this thread, the one a request is answered on), not
    /// [`super::BODY_DEADLINE`]: a test of the deadline's own, without every other test living
    /// under a short one.
    pub(crate) fn with_body_deadline<T>(deadline: Duration, f: impl FnOnce() -> T) -> T {
        let before = DEADLINE.with(|d| d.replace(Some(deadline)));
        let done = f();
        DEADLINE.with(|d| d.set(before));
        done
    }
}
