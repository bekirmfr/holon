//! The control plane's HTTP edge: who may call it (Host, Origin, Content-Type), the body cap,
//! CORS, and `route`, which dispatches every request to its handler.

use crate::*;
use crate::access::{Access, Caller, Role};

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

/// The largest request body any endpoint accepts. Units and flow documents are kilobytes; this
/// only exists so one oversized POST can't exhaust memory.
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
    if request.method() == "POST" {
        let media_type = request.header("Content-Type").unwrap_or("").split(';').next().unwrap_or("").trim();
        if !["application/json", "application/octet-stream"].iter().any(|t| media_type.eq_ignore_ascii_case(t)) {
            return Err(err(415, "POST bodies must be sent as application/json or application/octet-stream"));
        }
    }
    access.check(request)
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
                .with_additional_header("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS")
                .with_additional_header("Access-Control-Allow-Headers", "Content-Type, Authorization")
                .with_additional_header("Vary", "Origin");
        }
    response
}

pub(crate) fn read_body(request: &Request) -> Result<Vec<u8>, Response> {
    let Some(body) = request.data() else {
        return Err(err(400, "body already consumed"));
    };
    let mut bytes = Vec::new();
    // One byte past the cap is enough to tell "too large" apart from "exactly at the cap".
    if body.take(MAX_BODY_BYTES + 1).read_to_end(&mut bytes).is_err() {
        return Err(err(400, "failed to read request body"));
    }
    if bytes.len() as u64 > MAX_BODY_BYTES {
        return Err(err(413, format!("request body exceeds {MAX_BODY_BYTES} bytes")));
    }
    Ok(bytes)
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
    let agent = caller.role == Some(Role::Agent);

    // Preflight — answered before touching any endpoint logic.
    if method == "OPTIONS" {
        return Response::empty_204();
    }

    // Signing in, and the admin panel (auth.rs).
    if let Some(response) = crate::auth::route(state, request, &caller) {
        return response;
    }

    if method == "GET" && url == "/health" {
        return Response::text("ok");
    }

    if let Some(hash_hex) = url.strip_prefix("/unit/")
        && method == "GET" {
            return handle_get_unit(&state.store, hash_hex);
        }

    if method == "POST" && url == "/unit" {
        return handle_post_unit(&state.store, request);
    }

    if method == "POST" && url == "/registry/publish" {
        return handle_post_publish(&state.registry, request);
    }

    if method == "GET" && url == "/registry" {
        return handle_get_registry(&state.registry);
    }

    if method == "POST" && url == "/registry/fork" {
        return handle_post_fork(&state.registry, request);
    }

    if method == "POST" && url == "/cache/ingest" {
        return handle_post_cache_ingest(&state.cache, request);
    }

    if let Some(key_hex) = url.strip_prefix("/cache/")
        && method == "GET" {
            return handle_get_cache(&state.cache, &state.store, key_hex);
        }

    if method == "POST" && url == "/fire" {
        return handle_post_fire(&state.store, request);
    }

    if method == "POST" && url == "/deploy" {
        let response = handle_post_deploy(&state.store, &state.deployments, &state.deployments_path, &state.policy(), &state.secrets, &state.folders, request, by, agent, &state.activity);
        sync_listeners(state);
        return response;
    }

    if url == "/flows" {
        if method == "POST" {
            return handle_post_flows(&state.flows, &state.flows_path, request, by, agent, &state.activity);
        }
        if method == "GET" {
            return handle_get_flows(&state.flows);
        }
    }

    if let Some(name) = url.strip_prefix("/flows/") {
        if method == "GET" {
            return handle_get_flow(&state.flows, name);
        }
        if method == "DELETE" {
            return recorded(state, handle_delete_flow(&state.flows, &state.flows_path, name), "deleted flow", name, by);
        }
    }

    if method == "GET" && url.split('?').next() == Some("/activity") {
        return crate::activity::handle_get_activity(&state.activity, request);
    }

    if method == "POST" && url == "/deploy/preview" {
        return handle_post_deploy_preview(&state.store, &state.deployments, &state.policy(), &state.secrets, &state.folders, request, agent);
    }

    if method == "GET" && url == "/permissions" {
        return handle_get_permissions(&state.policy());
    }

    if method == "POST" && url == "/permissions/reload" {
        return grants::handle_post_permissions_reload(state);
    }

    if method == "GET" && url == "/deployments" {
        return handle_get_deployments(&state.deployments);
    }

    if let Some(rest) = url.strip_prefix("/deployments/") {
        if let Some(slug) = rest.strip_suffix("/secrets") {
            return handle_secrets(&state.secrets, slug, None, method, request, by);
        }
        if let Some((slug, name)) = rest.split_once("/secrets/") {
            return handle_secrets(&state.secrets, slug, Some(name), method, request, by);
        }
        if let Some((slug, name)) = rest.split_once("/collections/")
            && method == "GET"
        {
            return collections::handle_get_collection(state, slug, name, request);
        }
        if let Some(slug) = rest.strip_suffix("/runs")
            && method == "GET"
        {
            return durable::handle_get_runs(&state.deployments, slug);
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
        } else if let Some(slug) = rest.strip_suffix("/document") {
            if method == "GET" {
                return handle_get_document(&state.deployments, slug, request);
            }
        } else if let Some((slug, which)) = rest.rsplit_once('/')
            && matches!(which, "manifest" | "grant")
        {
            if method == "GET" {
                return handle_get_permission_view(&state.store, &state.deployments, slug, which);
            }
            if method == "POST" && which == "grant" {
                return recorded(state, grants::handle_post_grant(state, slug, request), "granted", slug, by);
            }
        } else if let Some(slug) = rest.strip_suffix("/sessions") {
            if method == "GET" {
                return handle_get_sessions(&state.deployments, &state.store, slug);
            }
        } else if let Some((slug, action)) = rest.rsplit_once('/')
            && matches!(action, "run" | "pause" | "stop")
        {
            if method == "POST" {
                let response = handle_lifecycle(state, slug, action, request);
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
        } else if let Some(slug) = rest.strip_suffix("/tick") {
            if method == "POST" {
                return scheduler::handle_post_tick(state, slug, request);
            }
        } else if method == "DELETE" {
            let response = handle_delete_deployment(&state.store, &state.deployments, &state.deployments_path, rest);
            if response.status_code == 200
                && let Err(e) = state.secrets.forget(rest, by)
            {
                eprintln!("warning: {rest:?}'s secrets weren't removed: {e}");
            }
            sync_listeners(state);
            return recorded(state, response, "deleted", rest, by);
        }
    }

    if let Some(slug) = url.strip_prefix("/trigger/")
        && method == "POST" {
            // `/trigger/<slug>/<name>` runs one of its actions by hand.
            if let Some((slug, name)) = slug.split_once('/') {
                return handle_post_action(&state.store, &state.deployments, &state.deployments_path, slug, name, request, by);
            }
            return handle_post_trigger(&state.store, &state.deployments, &state.deployments_path, slug, request);
        }

    Response::empty_404()
}
