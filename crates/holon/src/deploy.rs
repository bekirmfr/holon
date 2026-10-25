//! Deployments: deploy, list, state, history, delete, `/trigger`, run / pause / stop and sessions,
//! the flow log and the canvas a deployment came from.

use crate::*;

/// Whether a unit's derived capabilities include either state axis — the gate for whether a
/// deployment gets its own `state` slot at all. Takes the iterator `Capabilities::iter()`
/// already gives, rather than the `Capabilities` type itself, so this needs no new import.
pub(crate) fn derives_state(mut caps: impl Iterator<Item = Capability>) -> bool {
    caps.any(|c| {
        // A folder's reads are checked, and its writes committed, as a collection's are.
        matches!(
            c,
            Capability::StateRead(_)
                | Capability::StateWrite(_)
                | Capability::CollectionRead(_)
                | Capability::CollectionWrite(_)
                | Capability::FileRead(_)
                | Capability::FileWrite(_)
        )
    })
}

/// What a deployment shows as its triggers: a server's port, or plain `Ingress` for a unit that
/// takes requests some other way, plus `Clock` for a clock-driven one (`clock` is false for a
/// server, which the scheduler never fires). Deploy and reload both derive it here.
pub(crate) fn trigger_labels(server: Option<&Server>, ingress: bool, clock: bool) -> Vec<String> {
    let mut triggers = Vec::new();
    if let Some(s) = server {
        triggers.push(format!("Http :{}", s.port));
    } else if ingress {
        triggers.push("Ingress".to_string());
    }
    if clock {
        triggers.push("Clock".to_string());
    }
    triggers
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_post_deploy(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    policy: &Policy,
    secrets: &SecretStore,
    folders: &Arc<folders::Folders>,
    request: &Request,
    // Who deployed it (access.rs), kept with the deployment.
    by: Option<&str>,
    // Whether an agent deploys it (access.rs): it never approves.
    agent: bool,
    activity: &crate::activity::Activity,
) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body: DeployRequestBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("not a valid deploy request JSON: {e}")),
    };
    if agent && !body.acknowledge.is_empty() {
        return err(403, "an agent doesn't approve what a flow may do: deploy without \"acknowledge\"; what needs approval, a person approves, deploying it from the editor");
    }
    if !is_valid_slug(&body.slug) {
        return err(400, "slug must be non-empty ASCII alphanumeric, '-', or '_' only");
    }
    if let Err(e) = crate::flows::check_uses(body.uses.as_deref()) {
        return err(400, e);
    }
    if let Some(bad) = body.browsers.iter().find(|o| !crate::listeners::is_origin_or_any(o)) {
        return err(400, format!("browsers: {bad:?} isn't an origin (https://example.com) or *"));
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
    let arrivals = Arc::new(arrivals::Watch::open(deployments_path, &body.slug, &caps));
    if !is_ingress && !is_clock && !arrivals.watches() {
        return err(
            400,
            "this unit declares no driver capability (NetworkIngress, Clock or FileArrival) — nothing would ever trigger it",
        );
    }

    // Deploy is a request (permissions.rs): the manifest is what the unit's graph derives plus
    // what its author declares; policy refuses what it never grants, grants what it grants
    // without asking, and everything else must be acknowledged. A redeploy asks only for what
    // the previous grant didn't already cover.
    let manifest = Manifest::new(caps.iter(), body.requirements.clone().with_module_limits(root_unit, &resolver));
    let seen = Arc::new(folders::Scoped::new(Arc::clone(folders), &body.slug));
    if let Some(why) = policy.refuses(&manifest).or_else(|| seen.refuses(&manifest)) {
        return err(400, format!("this host can't grant that: {why}"));
    }
    let previous = deployments.lock().unwrap().get(&body.slug).map(|d| d.grant.clone());
    let missing: Vec<String> = policy
        .needs_approval(&manifest, previous.as_ref())
        .into_iter()
        .filter(|item| !body.acknowledge.contains(item))
        .collect();
    if agent && !missing.is_empty() {
        return err(
            403,
            format!(
                "this needs a person's approval ({}): save it as a flow, and a person approves it by deploying it from the editor",
                missing.join(", ")
            ),
        );
    }
    if !missing.is_empty() {
        return err(
            400,
            format!(
                "this unit's manifest includes {} — list them in \"acknowledge\" to confirm you've reviewed what it may do",
                missing.join(", ")
            ),
        );
    }
    let grant = Grant::for_manifest(&manifest, epoch_now(), "deploy");
    let permit = Arc::new(Permit::new(unit_hash, &grant, policy));
    if let Some(why) = durable::refuses_waits(root_unit, &resolver, &permit) {
        return err(400, format!("this host can't grant that: {why}"));
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

    let mut triggers = trigger_labels(server.as_ref(), is_ingress, is_clock);
    triggers.extend(arrivals.labels());
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
        let (log, old_document, mut versions, old_unit) = match deployments.get_mut(&body.slug) {
            Some(old) => (std::mem::take(&mut old.log), old.document.take(), std::mem::take(&mut old.versions), Some(old.unit)),
            None => (Log::for_deployment(deployments_path, &body.slug), None, Vec::new(), None),
        };
        // The version it replaces keeps its canvas, so the runs it logged still show on it.
        if let (Some(unit), Some(doc)) = (old_unit, &old_document)
            && unit != unit_hash
        {
            keep_version(&mut versions, to_hex(&unit), doc.clone());
        }
        let document = body.document.clone().or(old_document);
        let carried = deployments.get(&body.slug).and_then(|old| old.state.clone());
        // Runs waiting keep waiting, each on the unit it started on.
        let parked = deployments.get(&body.slug).map(|old| old.parked.clone()).unwrap_or_default();
        let state = match carried {
            Some(arc) => Some(arc),
            None if is_state => Some(Arc::new(claims::StateCell::new(State::new()))),
            None => None,
        };
        arrivals.baseline(&seen);
        deployments.insert(
            body.slug.clone(),
            Deployment {
                grant,
                permit,
                attention: None,
                listen_error: None,
                secrets: secrets.vault(&body.slug),
                folders: seen,
                arrivals,
                parked,
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
                deployed_by: by.map(String::from),
                uses: body.uses.clone(),
                browsers: body.browsers.clone(),
                life,
                log,
                document,
                versions,
            },
        );
    }
    persist_deployments(deployments_path, deployments, store);
    activity.record("deployed", &body.slug, by);

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
        deployed_by: dep.deployed_by.clone(),
        uses: dep.uses.clone(),
        browsers: dep.browsers.clone(),
        attention: dep.attention.clone(),
        listen_error: dep.listen_error.clone(),
        waiting: dep.parked.len(),
    }
}

/// `POST /deploy/preview` — what deploying would ask for, without deploying: the manifest's items,
/// which of them need approval (what `/deploy` will want acknowledged), which are already
/// granted (by host policy or the slug's current grant), and why policy would refuse it, if it
/// would. The body is a deploy request's (`acknowledge` is ignored). This is the consent screen's
/// source: it shows asked against granted before anything changes.
pub(crate) fn handle_post_deploy_preview(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    policy: &Policy,
    secrets: &SecretStore,
    folders: &Arc<folders::Folders>,
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
    let manifest = Manifest::new(capabilities(root_unit, &resolver).iter(), body.requirements.with_module_limits(root_unit, &resolver));
    let previous = deployments.lock().unwrap().get(&body.slug).map(|d| d.grant.clone());
    let needs = policy.needs_approval(&manifest, previous.as_ref());
    let items = manifest.items();
    let granted: Vec<&String> = items.iter().filter(|i| !needs.contains(*i)).collect();
    Response::json(&serde_json::json!({
        "manifest": to_hex(&manifest.hash()),
        "items": items,
        "needs_approval": needs,
        "already_granted": granted,
        "refused": policy.refuses(&manifest).or_else(|| {
            let permit = Permit::new(unit_hash, &Grant::for_manifest(&manifest, 0, "preview"), policy);
            durable::refuses_waits(root_unit, &resolver, &permit)
        }).or_else(|| folders::Scoped::new(Arc::clone(folders), &body.slug).refuses(&manifest)),
        // Secrets it reads that aren't set yet: it deploys, but those nodes stay empty until set.
        "missing_secrets": manifest
            .capabilities
            .iter()
            .filter_map(|c| c.strip_prefix("SecretRead(\"")?.strip_suffix("\")"))
            .filter(|name| !secrets.names(&body.slug).iter().any(|n| n == name))
            .collect::<Vec<_>>(),
    }))
}

/// `GET /deployments/<slug>/secrets` — its secrets' names, versions and sources, never values.
/// `POST /deployments/<slug>/secrets/<name>` (`{"value": "..."}`, or `{"from": "env:…"}` /
/// `{"from": "vault:…"}` for one kept elsewhere: secret_sources.rs) sets one, as a new version;
/// `DELETE` removes it, every version. `GET /deployments/<slug>/secrets/<name>` — its kept
/// versions and audit events; `POST …/<name>/rollback` (`{"version": n}`) makes an older version
/// current again, as a new one.
/// A slug may have secrets before it is first deployed.
/// `by`: who asked (access.rs), named in the audit.
pub(crate) fn handle_secrets(secrets: &SecretStore, slug: &str, name: Option<&str>, method: &str, request: &Request, by: Option<&str>) -> Response {
    if !is_valid_slug(slug) {
        return err(400, "slug must be non-empty ASCII alphanumeric, '-', or '_' only");
    }
    // `<name>/rollback`: a secret name never holds a '/'.
    let (name, action) = match name.map(|n| n.split_once('/')) {
        Some(Some((name, action))) => (Some(name), Some(action)),
        _ => (name, None),
    };
    match (method, name, action) {
        ("GET", None, None) => Response::json(&serde_json::json!({ "names": secrets.names(slug), "secrets": secrets.list(slug) })),
        ("GET", Some(name), None) => match secrets.history(slug, name) {
            Some((versions, events)) => Response::json(&serde_json::json!({ "name": name, "versions": versions, "events": events })),
            None => err(404, format!("{slug:?} has no secret {name:?}")),
        },
        ("POST", Some(name), Some("rollback")) => {
            let bytes = match read_body(request) {
                Ok(b) => b,
                Err(r) => return r,
            };
            #[derive(Deserialize)]
            struct Rollback {
                version: u32,
            }
            match serde_json::from_slice::<Rollback>(&bytes) {
                Ok(body) => match secrets.rollback(slug, name, body.version, by) {
                    Ok(version) => Response::json(&serde_json::json!({ "version": version })),
                    Err(e) => err(400, e),
                },
                Err(e) => err(400, format!("expected {{\"version\": n}}: {e}")),
            }
        }
        ("POST", Some(name), None) => {
            let bytes = match read_body(request) {
                Ok(b) => b,
                Err(r) => return r,
            };
            #[derive(Deserialize)]
            struct SetSecret {
                value: Option<String>,
                from: Option<String>,
            }
            let set = match serde_json::from_slice(&bytes) {
                Ok(SetSecret { value: Some(value), from: None }) => secrets.set(slug, name, &value, by),
                Ok(SetSecret { value: None, from: Some(from) }) if from == "local" => Err("a local secret is set with a value".to_string()),
                Ok(SetSecret { value: None, from: Some(from) }) => secrets.set_from(slug, name, &from, by),
                Ok(_) => Err("expected {\"value\": \"...\"} or {\"from\": \"env:… | vault:…\"}".to_string()),
                Err(e) => Err(format!("expected {{\"value\": \"...\"}} or {{\"from\": \"...\"}}: {e}")),
            };
            match set {
                Ok(version) => Response::json(&serde_json::json!({ "version": version })),
                Err(e) => err(400, e),
            }
        }
        ("DELETE", Some(name), None) => match secrets.remove(slug, name, by) {
            Ok(true) => Response::empty_204(),
            Ok(false) => err(404, format!("{slug:?} has no secret {name:?}")),
            Err(e) => err(500, e),
        },
        _ => Response::empty_404(),
    }
}

/// `GET /permissions` — this host's policy: each limit's default and ceiling, what it grants
/// without asking, and which exceptions it ever grants.
pub(crate) fn handle_get_permissions(policy: &Policy) -> Response {
    Response::json(&permissions_view(policy))
}

pub(crate) fn permissions_view(policy: &Policy) -> serde_json::Value {
    serde_json::json!({
        "defaults": policy.defaults(),
        "ceilings": policy.ceilings,
        "auto": policy.auto,
        "allow_plain_http": policy.allow_plain_http,
        "allow_private_addresses": policy.allow_private_addresses,
    })
}

/// `GET /deployments/<slug>/manifest` — what it needs, re-derived from its unit; and
/// `GET /deployments/<slug>/grant` — what the host granted it, with the grant's hash.
pub(crate) fn handle_get_permission_view(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    slug: &str,
    which: &str,
) -> Response {
    let (unit, grant, attention) = match deployments.lock().unwrap().get(slug) {
        Some(d) => (d.unit, d.grant.clone(), d.attention.clone()),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    if which == "grant" {
        return Response::json(&serde_json::json!({ "grant": grant, "hash": to_hex(&grant.hash()), "attention": attention }));
    }
    let resolver = match materialize_closure(store, unit) {
        Ok(r) => r,
        Err(e) => return err(500, format!("store error while materializing closure: {e:?}")),
    };
    let Some(root) = resolver.resolve(&unit) else { return err(500, "deployed unit no longer resolves in the store") };
    let manifest = Manifest::new(capabilities(root, &resolver).iter(), grant.requirements.clone().with_module_limits(root, &resolver));
    Response::json(&serde_json::json!({
        "hash": to_hex(&manifest.hash()),
        "capabilities": manifest.capabilities,
        "requirements": manifest.requirements,
        "items": manifest.items(),
        "optional": manifest.requirements.optional,
        "trigger": manifest.trigger(),
        "not_granted": grant.missing(&manifest),
    }))
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
                let storage = arc.state.lock().unwrap();
                let state = storage.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect();
                let mut reply = serde_json::to_value(StateBody { slug: slug.to_string(), state, root: to_hex(&storage.root()) })
                    .unwrap_or_default();
                Redactor::of(&dep.secrets).json(&mut reply);
                Response::json(&reply)
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
    store: &SharedStore,
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
            dep.log.delete_file();
            dep.arrivals.forget();
            persist_deployments(deployments_path, deployments, store);
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
    let (unit_hash, state, permit, vault, folders) = {
        let deployments = deployments.lock().unwrap();
        match deployments.get(slug) {
            Some(dep) if dep.life.status != Status::Running => {
                return err(409, format!("{slug:?} is {}", status_word(dep.life.status)));
            }
            Some(dep) if dep.server.is_some() => {
                return err(400, "this deployment is an HTTP server: send requests to its own port");
            }
            Some(dep) => (dep.unit, dep.state.clone(), Arc::clone(&dep.permit), Arc::clone(&dep.secrets), Arc::clone(&dep.folders)),
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
    let mut waits = RunWaits { folders: Some(folders), ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root_unit, &[], &resolver, store, state.as_ref(), &extra_sourced, &permit, false, &vault, &mut waits, None);

    record_trigger(deployments, slug, false);
    let mut entry = run_entry("trigger", started, &extra_sourced, &run);
    match durable::park_if_waiting(deployments, store, slug, &unit_hash, "trigger", None, &[], &extra_sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    Redactor::tokens(&waits.tokens).entry(&mut entry);
    let parked_any = !entry.parked.is_empty();
    log_run(deployments, slug, entry);
    let FiredRun { outputs, performed, .. } = run;
    if state.is_some() || parked_any {
        persist_deployments(deployments_path, deployments, store);
    }

    let redactor = Redactor::of(&vault);
    let mut reply = serde_json::to_value(FireResponseBody {
        outputs: outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        performed: performed
            .into_iter()
            .map(|(capability, payload)| PerformedEntry { capability: format!("{capability:?}"), payload: SValue::from_core(&payload) })
            .collect(),
    })
    .unwrap_or_default();
    redactor.json(&mut reply);
    Response::json(&reply)
}

/// `GET /deployments/<slug>/log?after=<seq>&limit=<n>` — the flow log past `after` (all of it
/// without), oldest first, at most `limit` (default 200) newest. `next` is the seq the next
/// entry will get, so a poller can ask for `after=next-1`. `head` is the hash of the newest
/// entry's line, and `breaks` the seqs of entries that don't follow the one before them: the log
/// file was changed outside holon there (recorder.rs).
pub(crate) fn handle_get_log(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, request: &Request) -> Response {
    let after = request.get_param("after").and_then(|a| a.parse::<u64>().ok());
    let limit = request.get_param("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(200);
    match deployments.lock().unwrap().get(slug) {
        Some(dep) => Response::json(&serde_json::json!({
            "entries": dep.log.after(after, limit),
            "next": dep.log.next_seq(),
            "head": dep.log.head(),
            "breaks": dep.log.breaks(),
        })),
        None => err(404, format!("no deployment {slug:?}")),
    }
}

/// How many earlier versions' documents a deployment keeps.
pub(crate) const KEPT_VERSIONS: usize = 10;

/// Keep `document` as version `unit`'s, newest last, the last [`KEPT_VERSIONS`] only. A version
/// deployed again moves to the end rather than being kept twice.
pub(crate) fn keep_version(versions: &mut Vec<KeptDocument>, unit: String, document: serde_json::Value) {
    versions.retain(|v| v.unit != unit);
    versions.push(KeptDocument { unit, document });
    if versions.len() > KEPT_VERSIONS {
        versions.drain(..versions.len() - KEPT_VERSIONS);
    }
}

/// `GET /deployments/<slug>/document` — the flow document it was deployed from, for view mode;
/// with `?unit=<hex>`, the document of that version (the current one, or one kept), for a run
/// logged on it.
pub(crate) fn handle_get_document(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, request: &Request) -> Response {
    let deployments = deployments.lock().unwrap();
    let Some(dep) = deployments.get(slug) else { return err(404, format!("no deployment {slug:?}")) };
    let wanted = request.get_param("unit").filter(|u| *u != to_hex(&dep.unit));
    let doc = match &wanted {
        None => dep.document.as_ref(),
        Some(unit) => dep.versions.iter().find(|v| &v.unit == unit).map(|v| &v.document),
    };
    match (doc, wanted) {
        (Some(doc), _) => Response::json(doc),
        (None, None) => err(404, format!("{slug:?} was deployed without its canvas; deploy it again from the editor")),
        (None, Some(unit)) => err(404, format!("{slug:?} no longer keeps the canvas of version {}", &unit[..unit.len().min(12)])),
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
        if action == "run"
            && let Some(why) = &dep.attention
        {
            return err(409, format!("can't run {slug:?}: {why}"));
        }
        let live = dep.state.clone();
        let mut values = state.store.clone();
        // A session to restore is loaded (and verified) before anything changes, so a missing
        // value leaves the deployment as it was.
        let restored = match (action, from) {
            ("run", RunFrom::Session(n)) if dep.life.status == Status::Stopped => match dep.life.snapshot(n) {
                Some(refs) => match State::load(&refs, &values) {
                    Ok(s) => Some(s),
                    Err(e) => return err(500, format!("session {n}'s state can't be loaded: {e:?}")),
                },
                None => None, // `run` reports it
            },
            _ => None,
        };
        let result = match action {
            "pause" => dep.life.pause(),
            "stop" => {
                // The snapshot keeps references, so its values go into the store now.
                let refs = match live.as_ref().map(|arc| arc.state.lock().unwrap().save(&mut values)).transpose() {
                    Ok(refs) => refs,
                    Err(e) => return err(500, format!("the state couldn't be stored: {e:?}")),
                };
                dep.life.stop(refs.as_ref(), now)
            }
            _ => dep.life.run(from, now).map(|start_from| {
                if let (Some(arc), Some(_)) = (&live, start_from) {
                    *arc.state.lock().unwrap() = restored.unwrap_or_default();
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
        // From here on the status is the person's choice, not the grant's.
        dep.grant.paused = false;
        deployment_entry(slug, dep)
    };
    persist_deployments(&state.deployments_path, &state.deployments, &state.store);
    Response::json(&entry)
}

/// `GET /deployments/<slug>/sessions` — every run session, oldest first, each with its
/// end-of-session state snapshot (`null` while it runs, or for a deployment without state).
pub(crate) fn handle_get_sessions(
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    store: &SharedStore,
    slug: &str,
) -> Response {
    let (status, sessions, redactor) = match deployments.lock().unwrap().get(slug) {
        Some(dep) => (dep.life.status, dep.life.sessions.clone(), Redactor::of(&dep.secrets)),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    // Each snapshot with its values (from the store) and its root.
    let sessions: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            let mut json = serde_json::to_value(s).unwrap_or_default();
            let refs = s.state_refs.as_ref().map(|r| {
                r.iter().filter_map(|(k, h)| Some((k.clone(), parse_hash(h)?))).collect::<BTreeMap<String, Hash>>()
            });
            if let Some(refs) = refs {
                match State::load(&refs, store) {
                    Ok(state) => {
                        let values: BTreeMap<String, SValue> =
                            state.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect();
                        json["state"] = serde_json::to_value(values).unwrap_or_default();
                        json["root"] = serde_json::Value::String(to_hex(&state.root()));
                    }
                    Err(e) => json["state_error"] = serde_json::Value::String(format!("{e:?}")),
                }
            }
            json
        })
        .collect();
    let mut reply = serde_json::json!({ "status": status, "sessions": sessions });
    redactor.json(&mut reply);
    Response::json(&reply)
}

