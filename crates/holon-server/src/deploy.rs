//! Deployments: deploy, list, state, history, delete, `/trigger`, run / pause / stop and sessions,
//! the flow log and the canvas a deployment came from.

use crate::*;

/// Whether a unit's derived capabilities include either state axis — the gate for whether a
/// deployment gets its own `state` slot at all. Takes the iterator `Capabilities::iter()`
/// already gives, rather than the `Capabilities` type itself, so this needs no new import.
pub(crate) fn derives_state(mut caps: impl Iterator<Item = Capability>) -> bool {
    caps.any(|c| matches!(c, Capability::StateRead(_) | Capability::StateWrite(_)))
}

pub(crate) fn is_valid_slug(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `POST /deploy` — register `slug → unit`. The trigger(s) are **derived**, not asserted:
/// `capabilities(unit, resolver)` (the same closure-union already used for capability-consent
/// elsewhere in this project) tells us whether the unit needs `Ingress`, `Clock`, both, or
/// neither — the caller only supplies what genuinely can't be read off the graph
/// (`interval_seconds`, only when `Clock` is present; nothing tells you "every 3600 seconds"
/// from structure alone). A unit deriving neither capability is refused — nothing would ever
/// trigger it. A repeat slug is a redeploy, not an error (the L4's Fork 2): the unit, triggers
/// and schedule are replaced, but the slug's **state is kept** (see the insert below).
pub(crate) fn handle_post_deploy(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    request: &Request,
) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body: DeployRequestBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("not a valid deploy request JSON: {e}")),
    };
    if !is_valid_slug(&body.slug) {
        return err(400, "slug must be non-empty ASCII alphanumeric, '-', or '_' only");
    }
    let Some(unit_hash) = parse_hash(&body.unit) else {
        return err(400, "unit is not a 64-char hex hash");
    };

    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => return err(500, format!("store error while materializing closure: {e:?}")),
    };
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        return err(404, "no unit stored at that hash — stage or publish it first");
    };

    let caps = capabilities(root_unit, &resolver);
    // A server declaration is answered on its own port (the data plane), so it needs neither
    // `/trigger` nor a schedule. Its clock/random leaves are sourced per request instead.
    let server = match declaration_of(root_unit) {
        Some("server") => match read_server(root_unit, &resolver) {
            Ok(s) => Some(s),
            Err(e) => return err(400, format!("not a valid server declaration: {e:?}")),
        },
        _ => None,
    };
    let is_ingress = caps.contains(Capability::NetworkIngress);
    let is_clock = caps.contains(Capability::Clock) && server.is_none();
    if !is_ingress && !is_clock {
        return err(
            400,
            "this unit declares no driver capability (NetworkIngress or Clock) — nothing would ever trigger it",
        );
    }

    // Capability-consent, before this unit ever gets to run on a schedule or answer a request.
    // The set itself is still fully derived (`caps`, above) — only "has the caller actually
    // reviewed it" is asserted, which is the one part that genuinely can't come from the graph.
    let missing: Vec<String> = caps
        .iter()
        .map(|c| format!("{c:?}"))
        .filter(|name| !body.acknowledge.contains(name))
        .collect();
    if !missing.is_empty() {
        return err(
            400,
            format!(
                "this unit's derived capabilities include {} — list them in \"acknowledge\" to confirm you've reviewed what it may do",
                missing.join(", ")
            ),
        );
    }

    let interval_seconds = if is_clock {
        match body.interval_seconds {
            Some(n) if n >= 1 => Some(n),
            Some(_) => return err(400, "interval_seconds must be at least 1 (the L4's Fork 3 floor)"),
            None => {
                return err(400, "this unit's derived capabilities include Clock — interval_seconds is required")
            }
        }
    } else {
        None
    };
    // Fires on the scheduler's very next pass rather than waiting a full interval the first
    // time — a person deploying a clock flow wants to see it work now, not in an hour.
    let next_due = interval_seconds.map(|_| SystemTime::now());

    let mut triggers = Vec::new();
    if let Some(s) = &server {
        triggers.push(format!("Http :{}", s.port));
    } else if is_ingress {
        triggers.push("Ingress".to_string());
    }
    if is_clock {
        triggers.push("Clock".to_string());
    }
    let is_state = derives_state(caps.iter());

    {
        let mut deployments = deployments.lock().unwrap();
        if let Some(s) = &server
            && let Some((other, _)) = deployments
                .iter()
                .find(|(other, d)| **other != body.slug && d.server.as_ref().is_some_and(|o| o.port == s.port))
        {
            return err(409, format!("port {} is already served by deployment {other:?}", s.port));
        }
        // A redeploy keeps the slug's state: shipping a fix must never wipe the data the
        // previous version wrote. The *same* `Arc` is carried over (not a copy), so a fire still
        // in flight against the old version commits into the map the new version reads. Kept
        // even when the new unit is stateless, so redeploying a stateful version later finds
        // its data again; `DELETE /deployments/<slug>` is the one way to discard it.
        let now_secs = epoch_now();
        // A redeploy keeps the slug's sessions too, and runs it: a stopped deployment starts a new
        // session from its latest state, a paused one resumes.
        let life = match deployments.get(&body.slug) {
            Some(old) => {
                let mut life = old.life.clone();
                let _ = life.run(RunFrom::Latest, now_secs);
                life
            }
            None => Lifecycle::deployed(now_secs),
        };
        let (log, old_document) = match deployments.get_mut(&body.slug) {
            Some(old) => (std::mem::take(&mut old.log), old.document.take()),
            None => (Log::default(), None),
        };
        let document = body.document.clone().or(old_document);
        let carried = deployments.get(&body.slug).and_then(|old| old.state.clone());
        let state = match carried {
            Some(arc) => Some(arc),
            None if is_state => Some(Arc::new(Mutex::new(BTreeMap::new()))),
            None => None,
        };
        deployments.insert(
            body.slug.clone(),
            Deployment {
                unit: unit_hash,
                triggers: triggers.clone(),
                interval_seconds,
                next_due,
                history: Vec::new(),
                state,
                server,
                trigger_count: 0,
                error_count: 0,
                deployed_at: SystemTime::now(),
                life,
                log,
                document,
            },
        );
    }
    persist_deployments(deployments_path, deployments);

    Response::json(&DeployResponseBody { slug: body.slug, unit: to_hex(&unit_hash), triggers, interval_seconds })
}

/// Build the wire-facing summary of one deployment — shared by `GET /deployments` and
/// `DELETE /deployments/<slug>`'s response, so the two never drift apart on what a
/// deployment "is" from the outside.
pub(crate) fn deployment_entry(slug: &str, dep: &Deployment) -> DeploymentEntry {
    DeploymentEntry {
        slug: slug.to_string(),
        unit: to_hex(&dep.unit),
        triggers: dep.triggers.clone(),
        interval_seconds: dep.interval_seconds,
        has_state: dep.state.is_some(),
        port: dep.server.as_ref().map(|s| s.port),
        status: dep.life.status,
        session: dep.life.sessions.last().map_or(0, |s| s.number),
        sessions: dep.life.sessions.len(),
        trigger_count: dep.trigger_count,
        error_count: dep.error_count,
        deployed_at_epoch_seconds: dep.deployed_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
    }
}

/// `GET /deployments` — every deployment, its derived triggers, and its interval if clock-bound.
pub(crate) fn handle_get_deployments(deployments: &Mutex<BTreeMap<String, Deployment>>) -> Response {
    let deployments = deployments.lock().unwrap();
    let list = deployments.iter().map(|(slug, dep)| deployment_entry(slug, dep)).collect();
    Response::json(&DeploymentsBody { deployments: list })
}

/// `GET /deployments/<slug>/history` — the bounded (last 20) clock-tick history for a
/// `Clock`-triggered deployment. Empty for a purely `Ingress`-triggered one — there's nothing
/// to show; each `/trigger/<slug>` call already returns its own result directly.
pub(crate) fn handle_get_history(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> Response {
    let deployments = deployments.lock().unwrap();
    match deployments.get(slug) {
        Some(dep) => Response::json(&HistoryBody { slug: slug.to_string(), ticks: dep.history.clone() }),
        None => Response::empty_404(),
    }
}

/// `GET /deployments/<slug>/state` — a stateful deployment's current keyed state, in full.
/// 404 for an unknown slug (same convention as `/history`); 400 for a slug that exists but
/// isn't stateful (its derived capabilities never included `StateRead`/`StateWrite`) — a
/// different failure from "not found," so a different status, naming what's actually true
/// rather than pretending an empty map would mean the same thing as "this was never a stateful
/// deployment at all."
pub(crate) fn handle_get_state(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> Response {
    let deployments = deployments.lock().unwrap();
    match deployments.get(slug) {
        Some(dep) => match &dep.state {
            Some(arc) => {
                let storage = arc.lock().unwrap();
                let state = storage.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect();
                Response::json(&StateBody { slug: slug.to_string(), state })
            }
            None => err(
                400,
                "this deployment has no state — its derived capabilities don't include StateRead/StateWrite",
            ),
        },
        None => Response::empty_404(),
    }
}

/// `DELETE /deployments/<slug>` — closes the named gap from the clock-deploy L4's Fork 4: the
/// only way to stop a `Clock`-triggered deployment used to be killing the whole server. Removing
/// the slug from the table is sufficient — `run_due_clock_ticks` only ever acts on what's
/// currently in the table, so there's no separate "cancel" signal to build; the deployment
/// simply stops being something the scheduler can find. For an `Ingress`-triggered deployment,
/// `POST /trigger/<slug>` starts 404ing immediately afterward, the same 404 it already gives a
/// slug that was never deployed. Returns the removed deployment's last known info, or 404 if
/// the slug didn't exist.
pub(crate) fn handle_delete_deployment(
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: &str,
) -> Response {
    let removed = {
        let mut table = deployments.lock().unwrap();
        match table.get(slug) {
            Some(dep) if dep.life.status != Status::Stopped => {
                return err(409, format!("stop {slug:?} before deleting it"));
            }
            _ => table.remove(slug),
        }
    };
    match removed {
        Some(dep) => {
            persist_deployments(deployments_path, deployments);
            Response::json(&deployment_entry(slug, &dep))
        }
        None => Response::empty_404(),
    }
}

/// `POST /trigger/<slug>` — the ingress trigger. Looks up the deployed unit, materializes its
/// closure exactly as `/fire` and the cache endpoints do, and settles it via `fire_sourced`
/// with the raw request body injected as `Capability::NetworkIngress` — wrapped as
/// `Value::Text`, the L4's Fork 3 default (`ingress`'s declared output type is `Any`; nothing
/// forces this choice, but Text matches how every other text-shaped payload already crosses
/// this boundary). A flow wanting structured data parses that Text itself, downstream, using
/// this project's own `Sum`/`select` machinery — not a format this endpoint imposes. Works
/// against any deployed unit regardless of its derived triggers (a `Clock`-only unit just won't
/// have an `ingress` leaf for the resolver to substitute, so this is harmless, not unsafe, to
/// allow uniformly rather than gating on which triggers were derived at deploy time).
/// Bump a deployment's trigger/error counters after a fire attempt — a small, shared tail
/// step for both `handle_post_trigger` and `fire_one_clock_tick`. Locks the table only
/// briefly; if the deployment was deleted concurrently between the earlier lookup and this
/// call, this quietly no-ops rather than panicking, the same defensive shape
/// `fire_one_clock_tick`'s own history write-back already uses.
pub(crate) fn record_trigger(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, is_error: bool) {
    if let Some(dep) = deployments.lock().unwrap().get_mut(slug) {
        dep.trigger_count += 1;
        if is_error {
            dep.error_count += 1;
        }
        dep.life.record(is_error);
    }
}

/// `POST /trigger/<slug>` — the ingress trigger. Looks up the deployed unit, materializes its
/// closure exactly as `/fire` and the cache endpoints do, and settles it via `fire_with_state`
/// with the raw request body injected as `Capability::NetworkIngress` — wrapped as
/// `Value::Text`, the L4's Fork 3 default (`ingress`'s declared output type is `Any`; nothing
/// forces this choice, but Text matches how every other text-shaped payload already crosses
/// this boundary). A flow wanting structured data parses that Text itself, downstream, using
/// this project's own `Sum`/`select` machinery — not a format this endpoint imposes. Works
/// against any deployed unit regardless of its derived triggers (a `Clock`-only unit just won't
/// have an `ingress` leaf for the resolver to substitute, so this is harmless, not unsafe, to
/// allow uniformly rather than gating on which triggers were derived at deploy time).
///
/// Only ever holds the deployments-table lock briefly — once to look up the unit and clone out
/// this deployment's own state handle (an `Arc` clone, not the state itself), and once at the
/// end to bump counters (and, for a stateful deployment, persist). The fire itself, and any
/// state locking it needs, happens with the table lock released, so one slow trigger never
/// blocks any other deployment.
pub(crate) fn handle_post_trigger(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: &str,
    request: &Request,
) -> Response {
    let (unit_hash, state) = {
        let deployments = deployments.lock().unwrap();
        match deployments.get(slug) {
            Some(dep) if dep.life.status != Status::Running => {
                return err(409, format!("{slug:?} is {}", status_word(dep.life.status)));
            }
            Some(dep) if dep.server.is_some() => {
                return err(400, "this deployment is an HTTP server: send requests to its own port");
            }
            Some(dep) => (dep.unit, dep.state.clone()),
            None => return Response::empty_404(),
        }
    };
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body_text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return err(400, "request body is not valid UTF-8 text"),
    };
    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => {
            record_trigger(deployments, slug, true);
            return err(500, format!("store error while materializing closure: {e:?}"));
        }
    };
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        record_trigger(deployments, slug, true);
        return err(500, "deployed unit no longer resolves in the store");
    };

    let now = SystemTime::now();
    let epoch_seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let extra_sourced = [
        (Capability::NetworkIngress, Value::Text(body_text)),
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Number(rand::random::<f64>())),
    ];
    let started = started_now();
    let run = fire_with_state(root_unit, &[], &resolver, state.as_deref(), &extra_sourced);

    record_trigger(deployments, slug, false);
    log_run(deployments, slug, run_entry("trigger", started, &extra_sourced, &run));
    let FiredRun { outputs, performed, .. } = run;
    if state.is_some() {
        persist_deployments(deployments_path, deployments);
    }

    Response::json(&FireResponseBody {
        outputs: outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        performed: performed
            .into_iter()
            .map(|(capability, payload)| PerformedEntry { capability: format!("{capability:?}"), payload: SValue::from_core(&payload) })
            .collect(),
    })
}

/// `GET /deployments/<slug>/log?after=<seq>&limit=<n>` — the flow log past `after` (all of it
/// without), oldest first, at most `limit` (default 200) newest. `next` is the seq the next
/// entry will get, so a poller can ask for `after=next-1`.
pub(crate) fn handle_get_log(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, request: &Request) -> Response {
    let after = request.get_param("after").and_then(|a| a.parse::<u64>().ok());
    let limit = request.get_param("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(200);
    match deployments.lock().unwrap().get(slug) {
        Some(dep) => Response::json(&serde_json::json!({ "entries": dep.log.after(after, limit), "next": dep.log.next_seq() })),
        None => err(404, format!("no deployment {slug:?}")),
    }
}

/// `GET /deployments/<slug>/document` — the flow document it was deployed from, for view mode.
pub(crate) fn handle_get_document(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> Response {
    match deployments.lock().unwrap().get(slug) {
        Some(dep) => match &dep.document {
            Some(doc) => Response::json(doc),
            None => err(404, format!("{slug:?} was deployed without its canvas; deploy it again from the editor")),
        },
        None => err(404, format!("no deployment {slug:?}")),
    }
}

pub(crate) fn epoch_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

pub(crate) fn status_word(status: Status) -> &'static str {
    match status {
        Status::Running => "running",
        Status::Paused => "paused",
        Status::Stopped => "stopped",
    }
}

/// `POST /deployments/<slug>/run|pause|stop` — move a deployment through its lifecycle
/// (lifecycle.rs). `run` takes an optional `{"from": "latest" | "fresh" | <session number>}`,
/// used only when starting after a stop. Answers with the deployment's entry; 409 when the
/// move doesn't apply (pausing a stopped deployment, running a running one, a session with no
/// saved state).
pub(crate) fn handle_lifecycle(state: &AppState, slug: &str, action: &str, request: &Request) -> Response {
    let from = if action == "run" {
        let bytes = match read_body(request) {
            Ok(b) => b,
            Err(r) => return r,
        };
        if bytes.is_empty() {
            RunFrom::Latest
        } else {
            let v: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(e) => return err(400, format!("not a valid run request JSON: {e}")),
            };
            let from = match &v["from"] {
                serde_json::Value::Null => Some(RunFrom::Latest),
                serde_json::Value::String(s) => RunFrom::parse(s),
                serde_json::Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()).map(RunFrom::Session),
                _ => None,
            };
            match from {
                Some(f) => f,
                None => return err(400, "from must be \"latest\", \"fresh\" or a session number"),
            }
        }
    } else {
        RunFrom::Latest
    };
    let entry = {
        let mut table = state.deployments.lock().unwrap();
        let Some(dep) = table.get_mut(slug) else {
            return err(404, format!("no deployment {slug:?}"));
        };
        let now = epoch_now();
        let live = dep.state.clone();
        let result = match action {
            "pause" => dep.life.pause(),
            "stop" => {
                let held = live.as_ref().map(|arc| arc.lock().unwrap().clone());
                dep.life.stop(held.as_ref(), now)
            }
            _ => dep.life.run(from, now).map(|start_from| {
                if let (Some(arc), Some(map)) = (&live, start_from) {
                    *arc.lock().unwrap() = map;
                }
                // A clock deployment fires on its next pass once running again.
                if dep.interval_seconds.is_some() {
                    dep.next_due = Some(SystemTime::now());
                }
            }),
        };
        if let Err(e) = result {
            return err(409, format!("can't {action} {slug:?}: {}", e.message()));
        }
        deployment_entry(slug, dep)
    };
    persist_deployments(&state.deployments_path, &state.deployments);
    Response::json(&entry)
}

/// `GET /deployments/<slug>/sessions` — every run session, oldest first, each with its
/// end-of-session state snapshot (`null` while it runs, or for a deployment without state).
pub(crate) fn handle_get_sessions(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> Response {
    match deployments.lock().unwrap().get(slug) {
        Some(dep) => Response::json(&serde_json::json!({ "status": dep.life.status, "sessions": dep.life.sessions })),
        None => err(404, format!("no deployment {slug:?}")),
    }
}
