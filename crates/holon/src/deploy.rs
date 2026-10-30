//! Deployments: deploy, list, state, history, delete, `/trigger`, run / pause / stop and sessions,
//! the flow log and the canvas a deployment came from.

use crate::*;

/// Whether a unit's derived capabilities include either state axis — the gate for whether a
/// deployment gets its own `state` slot at all. Takes the iterator `Capabilities::iter()`
/// already gives, rather than the `Capabilities` type itself, so this needs no new import.
pub(crate) fn derives_state(mut caps: impl Iterator<Item = Capability>) -> bool {
    caps.any(|c| {
        // A folder's or a database's reads are checked, and its writes committed, as a collection's are.
        matches!(
            c,
            Capability::StateRead(_)
                | Capability::StateWrite(_)
                | Capability::CollectionRead(_)
                | Capability::CollectionWrite(_)
                | Capability::FileRead(_)
                | Capability::FileWrite(_)
                | Capability::DbRead(_)
                | Capability::DbWrite(_)
        )
    })
}

/// What a deployment shows as its triggers: a server's port, its clocks (`every 2s`), its file
/// handlers (`on file inbox`) and its actions (`action reset`), or plain
/// `Ingress` for a unit that takes requests some other way, plus `Clock` for a flow on an
/// interval (`clock` is false for a server, whose clocks are its own). Deploy and reload both
/// derive it here.
pub(crate) fn trigger_labels(server: Option<&Server>, ingress: bool, clock: bool) -> Vec<String> {
    let mut triggers = Vec::new();
    if let Some(s) = server {
        triggers.extend(s.port.map(|port| format!("Http :{port}")));
        triggers.extend(s.clocks.iter().map(|c| c.label.clone()));
        triggers.extend(s.files.iter().map(|f| f.label.clone()));
        triggers.extend(s.actions.iter().map(|a| a.label.clone()));
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

/// The slug's current grant and unit, if it is deployed.
fn previous_of(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> (Option<Grant>, Option<Hash>, Vec<String>) {
    match deployments.lock().unwrap().get(slug) {
        Some(d) => (Some(d.grant.clone()), Some(d.unit), d.browsers.clone()),
        None => (None, None, Vec::new()),
    }
}

/// The origins `browsers` answers pages from that the slug's deployment doesn't yet, as items a
/// person approves for an agent (`Browser("https://app.example")`): a page from there, in its
/// visitor's browser, then reads what the flow answers.
fn browsers_added(browsers: &[String], previous: &[String]) -> Vec<String> {
    browsers.iter().filter(|o| !previous.iter().any(|p| p == "*" || p == *o)).map(|o| format!("Browser({o:?})")).collect()
}

/// What deploying `unit` needs a person to approve (what policy and the previous grant don't
/// cover), and what it deploys without: an agent's new code never inherits the secrets a person
/// let the old code read, because a secret goes only to code a person has looked at (redaction
/// catches a secret's literal value only, so code that encodes it first would hand the agent the
/// value). When an agent changes the code, a secret it can run without is withheld from the grant,
/// offered back to a person, and one it needs waits for a person's approval.
fn approval_needed(
    policy: &Policy,
    manifest: &Manifest,
    previous: Option<&Grant>,
    previous_unit: Option<Hash>,
    unit: Hash,
    agent: bool,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut needed = policy.needs_approval(manifest, previous);
    let mut withheld = BTreeSet::new();
    if agent && previous_unit != Some(unit) {
        for item in manifest.items().into_iter().filter(|item| item.starts_with("SecretRead(")) {
            if manifest.is_optional(&item) {
                needed.remove(&item);
                withheld.insert(item);
            } else {
                needed.insert(item);
            }
        }
    }
    (needed, withheld)
}

/// `POST /deploy` — register `slug → unit`. The trigger(s) are **derived**, not asserted:
/// `capabilities(unit, resolver)` (the same closure-union already used for capability-consent
/// elsewhere in this project) tells us whether the unit needs `Ingress`, `Clock`, both, or
/// neither — the caller only supplies what genuinely can't be read off the graph
/// (`interval_seconds`: nothing tells you "every 3600 seconds" from structure alone). A unit
/// without routes deployed with an interval runs on the clock, so `Clock` is in its manifest
/// whether or not it reads the time (core's `Capabilities::deployed`). A unit with no trigger is
/// refused — nothing would start a run. A repeat slug is a redeploy, not an error (the L4's Fork 2): the unit, triggers
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
    deploy(store, deployments, deployments_path, policy, secrets, folders, body, by, agent, activity, None)
}

/// Deploy `body` (`POST /deploy`'s, or a restore's: `restored_from` names the version it deploys
/// again). Every deploy is a new version, kept for good.
#[allow(clippy::too_many_arguments)]
fn deploy(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    policy: &Policy,
    secrets: &SecretStore,
    folders: &Arc<folders::Folders>,
    body: DeployRequestBody,
    by: Option<&str>,
    agent: bool,
    activity: &crate::activity::Activity,
    restored_from: Option<u32>,
) -> Response {
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
        Err(e) => return e.response(),
    };
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        return err(404, "no unit stored at that hash — stage or publish it first");
    };

    // A server declaration is answered on its own port (the data plane), so it needs neither
    // `/trigger` nor a schedule. Its clock/random leaves are sourced per request instead.
    let server = match declaration_of(root_unit) {
        Some("server") => match read_server(root_unit, &resolver) {
            Ok(s) => Some(s),
            Err(e) => return err(400, format!("not a valid server declaration: {e:?}")),
        },
        _ => None,
    };
    // A flow deployed on a schedule runs on the clock, whether or not it reads the time.
    let caps = capabilities(root_unit, &resolver).deployed(body.interval_seconds.is_some() && server.is_none());
    let is_ingress = caps.contains(Capability::NetworkIngress);
    let is_clock = caps.contains(Capability::Clock) && server.is_none();
    let has_clocks = server.as_ref().is_some_and(|s| !s.clocks.is_empty());
    let has_actions = server.as_ref().is_some_and(|s| !s.actions.is_empty());
    let arrivals = Arc::new(arrivals::Watch::open(deployments_path, &body.slug, &caps, holon_core::file_triggers(root_unit, &resolver)));
    if !is_ingress && !is_clock && !has_clocks && !has_actions && !arrivals.watches() {
        return err(
            400,
            "nothing would start a run: it has no route, clock, file handler or action. To run it by hand, put it in an `action run (input) { … }`",
        );
    }
    if has_clocks && body.interval_seconds.is_some() {
        return err(400, "this flow's clocks are in its text (its `every` blocks): it takes no interval_seconds");
    }
    // What starts its clocks' runs is never optional, beside routes too (the manifest names one
    // trigger, `NetworkIngress`, then).
    if has_clocks && body.requirements.optional.contains("Clock") {
        return err(400, "Clock starts this flow's clocks (its `every` blocks): it can't be optional");
    }
    // Nor is what starts its file handlers' runs.
    for f in server.iter().flat_map(|s| &s.files) {
        let named = format!("FileArrival({:?})", f.trigger.folder);
        if body.requirements.optional.contains(&named) {
            return err(400, format!("{named} starts this flow's {}: it can't be optional", f.label));
        }
    }
    // Agents may run only actions it has.
    if let Some(name) = body.requirements.agents.iter().find(|n| server.as_ref().is_none_or(|s| s.action(n).is_none())) {
        return err(400, format!("agents may run {name:?}, but it has no action {name:?}"));
    }
    // Nor its actions'.
    for a in server.iter().flat_map(|s| &s.actions) {
        let named = format!("Action({:?})", a.name);
        if body.requirements.optional.contains(&named) {
            return err(400, format!("{named} starts this flow's {}: it can't be optional", a.label));
        }
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
    let (previous, previous_unit, previous_browsers) = previous_of(deployments, &body.slug);
    let (mut needed, withheld) = approval_needed(policy, &manifest, previous.as_ref(), previous_unit, unit_hash, agent);
    if agent {
        needed.extend(browsers_added(&body.browsers, &previous_browsers));
    }
    let missing: Vec<String> = needed.into_iter().filter(|item| !body.acknowledge.contains(item)).collect();
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
    let mut grant = Grant::for_manifest(&manifest, epoch_now(), "deploy");
    if !withheld.is_empty() {
        grant.items.retain(|item| !withheld.contains(item));
        let named = withheld.iter().cloned().collect::<Vec<_>>().join(", ");
        grant.changes.push(crate::permissions::GrantChange { at: grant.granted_at, change: format!("withheld {named}: an agent changed the code, so a person grants it back") });
        grant.revoked = withheld;
    }
    // Limits the host lowered stay lowered: a new version doesn't undo the host's choice, unless
    // the deploy asks for that limit anew and a person approves it.
    if let Some(previous) = &previous {
        grant.lowered = previous
            .lowered
            .iter()
            .filter(|(name, _)| !body.acknowledge.iter().any(|item| item.starts_with(&format!("Limit({name}="))))
            .map(|(name, n)| (name.clone(), *n))
            .collect();
    }
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
    // A server's file handlers are named by their lines, among its triggers already.
    triggers.extend(arrivals.labels().filter(|_| server.is_none()));
    let is_state = derives_state(caps.iter());
    // Its canvas goes into the content store before anything changes, so a store that can't take
    // it refuses the deploy rather than losing the version's canvas.
    let document_ref = match &body.document {
        Some(doc) => match store_document(store, doc) {
            Ok(r) => Some(r),
            Err(e) => return err(500, format!("its canvas couldn't be kept: {e}")),
        },
        None => None,
    };
    let version;

    {
        let mut deployments = deployments.lock().unwrap();
        if let Some(s) = &server
            && let Some((other, _)) = deployments
                .iter()
                .find(|(other, d)| **other != body.slug && s.port.is_some() && d.server.as_ref().is_some_and(|o| o.port == s.port))
        {
            return err(409, format!("port {} is already served by deployment {other:?}", s.port.unwrap_or_default()));
        }
        // A redeploy keeps the slug's state: shipping a fix must never wipe the data the
        // previous version wrote. The *same* `Arc` is carried over (not a copy), so a fire still
        // in flight against the old version commits into the map the new version reads. Kept
        // even when the new unit is stateless, so redeploying a stateful version later finds
        // its data again; `DELETE /deployments/<slug>` is the one way to discard it.
        let now_secs = epoch_now();
        // A redeploy keeps the slug's sessions too, and runs it: a stopped deployment starts a new
        // session from its latest state, a paused one resumes. Starting is a person's, so an agent
        // replaces only a running deployment.
        let life = match deployments.get(&body.slug) {
            Some(old) if agent && old.life.status != crate::lifecycle::Status::Running => {
                let status = if old.life.status == crate::lifecycle::Status::Paused { "paused" } else { "stopped" };
                return err(
                    403,
                    format!("deployment {:?} is {status}, and an agent doesn't start one: a person runs it first, then deploy again", body.slug),
                );
            }
            Some(old) => {
                let mut life = old.life.clone();
                let _ = life.run(RunFrom::Latest, now_secs);
                life
            }
            None => Lifecycle::deployed(now_secs),
        };
        let (log, old_document, mut versions, mut events) = match deployments.get_mut(&body.slug) {
            Some(old) => (std::mem::take(&mut old.log), old.document.take(), std::mem::take(&mut old.versions), std::mem::take(&mut old.events)),
            None => (Log::for_deployment(deployments_path, &body.slug), None, Vec::new(), crate::events::Events::for_deployment(deployments_path, &body.slug)),
        };
        // Every deploy is a version, kept for good with what it asked, so it can be deployed
        // again; one deployed without a canvas keeps the canvas it had.
        version = versions.last().map_or(1, |v| v.number + 1);
        let document_ref = document_ref.or_else(|| versions.last().and_then(|v| v.document_ref.clone()));
        let carried = deployments.get(&body.slug).and_then(|old| old.state.clone());
        // And the state it carries, as a snapshot: the state the version before it left.
        let state_at_deploy = carried.as_ref().and_then(|arc| {
            crate::restore::save_snapshot(&arc.state.lock().unwrap(), None, store)
                .map_err(|e| eprintln!("holon: {}: the state version {version} carried wasn't kept: {e}", body.slug))
                .ok()
        });
        versions.push(Version {
            number: version,
            unit: to_hex(&unit_hash),
            deployed_at: Some(now_secs),
            by: by.map(String::from),
            restored_from,
            request: Some(VersionRequest {
                interval_seconds: body.interval_seconds,
                requirements: body.requirements.clone(),
                uses: body.uses.clone(),
                browsers: body.browsers.clone(),
            }),
            document_ref,
            state_at_deploy,
            first_seq: Some(log.next_seq()),
            document: None,
        });
        let what = match restored_from {
            Some(n) => format!("restored version {n}'s code, as version {version}"),
            None => format!("deployed version {version}"),
        };
        events.push("deploy", what, by, log.next_seq(), crate::events::Detail { version: Some(version), ..Default::default() });
        let document = body.document.clone().or(old_document);
        // Its subscribers stay subscribed across a redeploy, and its kept packs stay kept.
        let channels = deployments.get(&body.slug).map(|old| Arc::clone(&old.channels)).unwrap_or_default();
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
                clock_due: server.as_ref().map(|s| vec![SystemTime::now(); s.clocks.len()]).unwrap_or_default(),
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
                channels,
                document,
                versions,
                events,
            },
        );
    }
    persist_deployments(deployments_path, deployments, store);
    activity.record(if restored_from.is_some() { "restored" } else { "deployed" }, &body.slug, by);

    Response::json(&DeployResponseBody { slug: body.slug, unit: to_hex(&unit_hash), triggers, interval_seconds, version })
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
        port: dep.server.as_ref().and_then(|s| s.port),
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
    // Whether an agent asks (access.rs): what it needs approved is what its deploy would.
    agent: bool,
) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body: DeployRequestBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("not a valid deploy request JSON: {e}")),
    };
    preview(store, deployments, policy, secrets, folders, &body, agent)
}

/// What deploying `body` would ask for (`POST /deploy/preview`'s answer, and a restore's dry run).
fn preview(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    policy: &Policy,
    secrets: &SecretStore,
    folders: &Arc<folders::Folders>,
    body: &DeployRequestBody,
    agent: bool,
) -> Response {
    match preview_value(store, deployments, policy, secrets, folders, body, agent) {
        Ok(v) => Response::json(&v),
        Err(r) => r,
    }
}

/// What `preview` answers, for a restore to add to.
fn preview_value(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    policy: &Policy,
    secrets: &SecretStore,
    folders: &Arc<folders::Folders>,
    body: &DeployRequestBody,
    agent: bool,
) -> Result<serde_json::Value, Response> {
    let Some(unit_hash) = parse_hash(&body.unit) else {
        return Err(err(400, "unit is not a 64-char hex hash"));
    };
    let resolver = materialize_closure(store, unit_hash).map_err(|e| e.response())?;
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        return Err(err(404, "no unit stored at that hash — stage or publish it first"));
    };
    // As the deploy will derive it: on a schedule, it runs on the clock.
    let scheduled = body.interval_seconds.is_some() && declaration_of(root_unit) != Some("server");
    let manifest = Manifest::new(capabilities(root_unit, &resolver).deployed(scheduled).iter(), body.requirements.clone().with_module_limits(root_unit, &resolver));
    let (previous, previous_unit, previous_browsers) = previous_of(deployments, &body.slug);
    let (mut needs, withheld) = approval_needed(policy, &manifest, previous.as_ref(), previous_unit, unit_hash, agent);
    if agent {
        needs.extend(browsers_added(&body.browsers, &previous_browsers));
    }
    let items = manifest.items();
    let granted: Vec<&String> = items.iter().filter(|i| !needs.contains(*i) && !withheld.contains(*i)).collect();
    Ok(serde_json::json!({
        "manifest": to_hex(&manifest.hash()),
        "items": items,
        "needs_approval": needs,
        "already_granted": granted,
        // What the slug's grant covers now (none for a first deploy): beside `items`, what changes.
        "granted_now": previous.as_ref().map(|g| &g.items),
        // Secrets it can run without that an agent's deploy leaves out, for a person to grant back.
        "withheld": withheld,
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
/// current again, as a new one; with `"dry_run": true` it changes nothing and says what it would.
/// A slug may have secrets before it is first deployed.
/// `by`: who asked (access.rs), named in the audit.
pub(crate) fn handle_secrets(
    secrets: &SecretStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    slug: &str,
    name: Option<&str>,
    method: &str,
    request: &Request,
    by: Option<&str>,
) -> Response {
    // Each change goes on the deployment's timeline: its name, version and source, never a value.
    let note = |name: &str, what: String| {
        crate::events::note(deployments, slug, "secret", what, by, crate::events::Detail { name: Some(name.to_string()), ..Default::default() })
    };
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
                #[serde(default)]
                dry_run: bool,
            }
            match serde_json::from_slice::<Rollback>(&bytes) {
                Ok(body) if body.dry_run => match secrets.rollback_preview(slug, name, body.version) {
                    Ok(preview) => Response::json(&preview),
                    Err(e) => err(400, e),
                },
                Ok(body) => match secrets.rollback(slug, name, body.version, by) {
                    Ok(version) => {
                        note(name, format!("rolled the secret {name} back to version {}, as version {version}", body.version));
                        Response::json(&serde_json::json!({ "version": version }))
                    }
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
            let mut source = "a value";
            let set = match serde_json::from_slice(&bytes) {
                Ok(SetSecret { value: Some(value), from: None }) => secrets.set(slug, name, &value, by),
                Ok(SetSecret { value: None, from: Some(from) }) if from == "local" => Err("a local secret is set with a value".to_string()),
                Ok(SetSecret { value: None, from: Some(from) }) => {
                    source = if from.starts_with("vault:") { "vault" } else { "the environment" };
                    secrets.set_from(slug, name, &from, by)
                }
                Ok(_) => Err("expected {\"value\": \"...\"} or {\"from\": \"env:… | vault:…\"}".to_string()),
                Err(e) => Err(format!("expected {{\"value\": \"...\"}} or {{\"from\": \"...\"}}: {e}")),
            };
            match set {
                Ok(version) => {
                    note(name, format!("set the secret {name}, version {version}, from {source}"));
                    Response::json(&serde_json::json!({ "version": version }))
                }
                Err(e) => err(400, e),
            }
        }
        ("DELETE", Some(name), None) => match secrets.remove(slug, name, by) {
            Ok(true) => {
                note(name, format!("removed the secret {name}"));
                Response::empty_204()
            }
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
    let (unit, grant, attention, scheduled) = match deployments.lock().unwrap().get(slug) {
        Some(d) => (d.unit, d.grant.clone(), d.attention.clone(), d.interval_seconds.is_some()),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    if which == "grant" {
        return Response::json(&serde_json::json!({ "grant": grant, "hash": to_hex(&grant.hash()), "attention": attention }));
    }
    let resolver = match materialize_closure(store, unit) {
        Ok(r) => r,
        Err(e) => return e.response(),
    };
    let Some(root) = resolver.resolve(&unit) else { return err(500, "deployed unit no longer resolves in the store") };
    let manifest = Manifest::new(capabilities(root, &resolver).deployed(scheduled).iter(), grant.requirements.clone().with_module_limits(root, &resolver));
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
        Some(dep) => {
            // Redacted when kept; again here, against the secrets as they are now.
            let mut reply = serde_json::to_value(HistoryBody { slug: slug.to_string(), ticks: dep.history.clone() }).unwrap_or_default();
            Redactor::of(&dep.secrets).json(&mut reply);
            Response::json(&reply)
        }
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
            dep.events.delete_file();
            dep.arrivals.forget();
            dep.channels.close();
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
    let (unit_hash, state, permit, vault, folders, channels) = {
        let deployments = deployments.lock().unwrap();
        match deployments.get(slug) {
            Some(dep) if dep.life.status != Status::Running => {
                return err(409, format!("{slug:?} is {}", status_word(dep.life.status)));
            }
            Some(dep) if let Some(action) = dep.server.as_ref().and_then(|s| s.actions.first()) => {
                return err(400, format!("this deployment's actions run by name: POST /trigger/{slug}/{}", action.name));
            }
            Some(dep) if dep.server.is_some() => {
                return err(400, "this deployment is an HTTP server: send requests to its own port");
            }
            Some(dep) => (dep.unit, dep.state.clone(), Arc::clone(&dep.permit), Arc::clone(&dep.secrets), Arc::clone(&dep.folders), Arc::clone(&dep.channels)),
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
    let mut waits = RunWaits { folders: Some(folders), channels: Some(channels), slug: Some(slug.to_string()), ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root_unit, &[], &resolver, store, state.as_ref(), &extra_sourced, &permit, false, &vault, &mut waits, None);

    record_trigger(deployments, slug, false);
    let mut entry = run_entry("trigger", started, &extra_sourced, &run);
    match durable::park_if_waiting(deployments, store, slug, &unit_hash, "trigger", None, &[], &extra_sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    let parked_any = !entry.parked.is_empty();
    log_run(deployments, slug, entry, Redactor::tokens(&waits.tokens));
    let FiredRun { outputs, performed, answered, .. } = run;
    if state.is_some() || parked_any {
        persist_deployments(deployments_path, deployments, store);
    }

    let redactor = Redactor::of(&vault);
    let answers = Redactor::answers(&answered.0, &vault);
    let mut reply = serde_json::to_value(FireResponseBody {
        outputs: outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect(),
        performed: performed
            .into_iter()
            .map(|(capability, payload)| PerformedEntry { capability: format!("{capability:?}"), payload: SValue::from_core(&payload) })
            .collect(),
    })
    .unwrap_or_default();
    redactor.json(&mut reply);
    answers.json(&mut reply);
    Response::json(&reply)
}

/// `POST /trigger/<slug>/<name>`: run the deployment's action `name` by hand, what was sent the
/// body as JSON (nothing is `{}`). Its handler alone runs, logged as a trigger run under its label
/// (`action reset`) with what was sent and who ran it. The answer is `{answer, seq}`: the
/// respond's body (`null` for an action that only writes) and the run's log entry. A respond's
/// error (its `missing`) answers with its status; a run that failed, with why in its log. An
/// `agent` runs only an action its grant lets agents run (`AgentRuns("reset")`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_post_action(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: &str,
    name: &str,
    request: &Request,
    by: Option<&str>,
    agent: bool,
) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let input = String::from_utf8(bytes).map_err(|_| "what is sent to an action is JSON, and this isn't UTF-8 text".to_string()).and_then(|text| {
        if text.trim().is_empty() {
            Ok(Value::Record(Default::default()))
        } else {
            holon_core::parse_json(&text).ok_or_else(|| "what is sent to an action is JSON, and this isn't".to_string())
        }
    });
    let ran = run_action(store, deployments, deployments_path, slug, name, input, by, agent, &[]);
    if ran.status == 404 && ran.reply.is_null() {
        return Response::empty_404();
    }
    Response::json(&ran.reply).with_status_code(ran.status)
}

/// What running an action came to: the reply a caller over HTTP gets, and, for a flow's call, the
/// answer as a value and the run's number (docs/L4s/triggers-L4.md §9).
pub(crate) struct ActionRan {
    pub(crate) status: u16,
    /// `{answer, seq}`; a run that waits, `{answer: null, seq, waiting: true}`; why it didn't run,
    /// `{error}`; no deployment, `null` with a 404.
    pub(crate) reply: serde_json::Value,
    /// What it answered, as the reply says it (redacted as it is): none for an action that only
    /// writes, or that didn't run.
    pub(crate) answer: Option<Value>,
    /// The run's number in the deployment's log, when it ran.
    pub(crate) seq: Option<u64>,
    /// It parked at an `await`: the run goes on when the event arrives.
    pub(crate) waiting: bool,
    /// Why it didn't answer, in words.
    pub(crate) error: Option<String>,
    /// Which refusal or failure it was, for a flow's call to say as `error`.
    pub(crate) why: Option<&'static str>,
}

impl ActionRan {
    fn refused(status: u16, why: &'static str, message: impl Into<String>) -> ActionRan {
        let message = message.into();
        ActionRan { status, reply: serde_json::json!({ "error": message }), answer: None, seq: None, waiting: false, error: Some(message), why: Some(why) }
    }
}

/// Run the deployment `slug`'s action `name` with `input` (or why that isn't one: a 400), as `by`,
/// and say what came of it. `chain` is the flows whose calls led here, the first being the caller
/// that started it (empty for a person's or an agent's run): the run carries it, so a flow called
/// down a chain can't call one already in it, and the chain can't run past `max_call_depth`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_action(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: &str,
    name: &str,
    input: Result<Value, String>,
    by: Option<&str>,
    agent: bool,
    chain: &[String],
) -> ActionRan {
    let (action, state, permit, vault, folders, channels) = {
        let deployments = deployments.lock().unwrap();
        let Some(dep) = deployments.get(slug) else {
            return ActionRan { status: 404, reply: serde_json::Value::Null, answer: None, seq: None, waiting: false, error: Some(format!("no deployment {slug:?}")), why: Some("not_found") };
        };
        if dep.life.status != Status::Running {
            return ActionRan::refused(409, "not_running", format!("{slug:?} is {}", status_word(dep.life.status)));
        }
        let Some(action) = dep.server.as_ref().and_then(|s| s.action(name)) else {
            return ActionRan::refused(404, "no_action", format!("{slug:?} has no action {name:?}"));
        };
        // An agent runs only what its grant lets agents run, as a person granted it.
        if agent && !dep.grant.items.contains(&format!("AgentRuns({name:?})")) {
            return ActionRan::refused(403, "refused", format!("agents may not run {slug:?}'s action {name:?}: a person lets them, under \"agents may run\" in its deploy settings, and approves it"));
        }
        (action.clone(), dep.state.clone(), Arc::clone(&dep.permit), Arc::clone(&dep.secrets), Arc::clone(&dep.folders), Arc::clone(&dep.channels))
    };
    let input = match input {
        Ok(input) => input,
        Err(why) => return ActionRan::refused(400, "bad_input", why),
    };
    let failed_to_start = |message: String| {
        record_trigger(deployments, slug, true);
        ActionRan::refused(500, "failed", message)
    };
    let resolver = match materialize_closure(store, action.handler) {
        Ok(r) => r,
        Err(e) => return failed_to_start(format!("store error while materializing closure: {e:?}")),
    };
    let Some(handler) = resolver.resolve(&action.handler) else {
        return failed_to_start("the action's handler no longer resolves in the store".into());
    };

    let epoch_seconds = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let sourced = [(Capability::Clock, Value::Number(epoch_seconds as f64)), (Capability::Random, Value::Number(rand::random::<f64>()))];
    let started = started_now();
    let inputs = [Some(input.clone())];
    let mut waits = RunWaits { folders: Some(folders), channels: Some(channels), slug: Some(slug.to_string()), chain: chain.to_vec(), ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(handler, &inputs, &resolver, store, state.as_ref(), &sourced, &permit, false, &vault, &mut waits, None);

    // Output 0 is the respond's envelope, or (an action that only writes) an effect.
    let (status, answer) = match run.outputs.first().cloned().flatten() {
        Some(Value::Record(f)) if matches!(f.get("status"), Some(Value::Number(_))) => {
            let Some(Value::Number(n)) = f.get("status") else { unreachable!() };
            (*n as u16, f.get("body").cloned())
        }
        Some(Value::Effect { .. }) => (200, None),
        Some(other) => (200, Some(other)),
        None => (500, None),
    };
    let failed = run.error.is_some() || status >= 500;
    record_trigger(deployments, slug, failed);
    let mut entry = run_entry("trigger", started, &sourced, &run);
    entry.route = Some(action.label.clone());
    entry.request = Some(SValue::from_core(&input));
    entry.by = by.map(String::from);
    let answer_json = answer.as_ref().map_or(Some("null".to_string()), holon_core::stringify_json);
    entry.response_body = Some(if run.secret_fed.derives_output(0) { crate::taint::WITHHELD.to_string() } else { answer_json.clone().unwrap_or_default() });
    match durable::park_if_waiting(deployments, store, slug, &action.handler, "trigger", Some(action.label.clone()), &inputs, &sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    let (parked_any, error) = (!entry.parked.is_empty(), entry.error.is_some());
    let seq = deployments.lock().unwrap().get(slug).map_or(0, |d| d.log.next_seq());
    log_run(deployments, slug, entry, Redactor::tokens(&waits.tokens));
    if state.is_some() || parked_any {
        persist_deployments(deployments_path, deployments, store);
    }
    let ran = |status: u16, reply: serde_json::Value, answer: Option<Value>, waiting: bool, error: Option<String>, why: Option<&'static str>| ActionRan { status, reply, answer, seq: Some(seq), waiting, error, why };
    if error {
        let message = format!("the action failed: its run log says why (entry {seq})");
        return ran(500, serde_json::json!({ "error": message }), None, false, Some(message), Some("failed"));
    }
    if parked_any {
        return ran(202, serde_json::json!({ "answer": null, "seq": seq, "waiting": true }), None, true, None, Some("waiting"));
    }
    let Some(answer_json) = answer_json else {
        let message = "the action's answer has no JSON form".to_string();
        return ran(500, serde_json::json!({ "error": message }), None, false, Some(message), Some("failed"));
    };
    let answer: serde_json::Value = if run.secret_fed.derives_output(0) {
        serde_json::Value::String(crate::taint::WITHHELD.to_string())
    } else {
        serde_json::from_str(&answer_json).unwrap_or_default()
    };
    let mut reply = if (200..300).contains(&status) { serde_json::json!({ "answer": answer, "seq": seq }) } else { answer };
    Redactor::of(&vault).json(&mut reply);
    Redactor::tokens(&waits.tokens).json(&mut reply);
    // What a flow's call is answered with: the reply's answer, as it was redacted.
    let said = reply.get("answer").filter(|a| !a.is_null()).and_then(|a| holon_core::parse_json(&a.to_string()));
    let message = (!(200..300).contains(&status)).then(|| format!("the action answered {status}"));
    let why = message.as_ref().map(|_| "failed");
    ran(status, reply, said, false, message, why)
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

/// Keep a flow document in the content store, as a text value holding its JSON: its hash (hex).
fn store_document(store: &SharedStore, document: &serde_json::Value) -> Result<String, String> {
    let text = serde_json::to_string(document).map_err(|e| e.to_string())?;
    store.clone().put_value(&Value::Text(text)).map(|h| to_hex(&h)).map_err(|e| format!("{e:?}"))
}

/// A version's flow document, from the content store.
fn version_document(store: &SharedStore, version: &Version) -> Option<serde_json::Value> {
    let hash = parse_hash(version.document_ref.as_deref()?)?;
    match store.get_value(&hash).ok()?? {
        Value::Text(text) => serde_json::from_str(&text).ok(),
        _ => None,
    }
}

/// A deployment's versions as this release keeps them, from what a file of any release held:
/// each numbered, each document in the content store, and `current` last unless it is there
/// already. Files before rc.30 kept only earlier versions' documents (the last 10), without what
/// their deploys asked: those versions open on their canvas, but don't restore.
pub(crate) fn complete_versions(kept: Vec<Version>, current: Version, store: &SharedStore) -> Result<Vec<Version>, String> {
    let mut versions: Vec<Version> = Vec::with_capacity(kept.len() + 1);
    let mut next = 1;
    for mut v in kept.into_iter().chain(std::iter::once(current)) {
        if v.number == 0 {
            // The current version, when the file already ends with it.
            if versions.last().is_some_and(|last| last.unit == v.unit) {
                continue;
            }
            v.number = next;
        }
        next = v.number + 1;
        if let Some(doc) = v.document.take() {
            v.document_ref = Some(store_document(store, &doc).map_err(|e| format!("version {}'s canvas can't be stored: {e}", v.number))?);
        }
        versions.push(v);
    }
    Ok(versions)
}

/// `GET /deployments/<slug>/document` — the flow document it was deployed from, for view mode;
/// with `?unit=<hex>`, the document of that version (the current one, or an earlier one), for a
/// run logged on it.
pub(crate) fn handle_get_document(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, request: &Request) -> Response {
    let deployments = deployments.lock().unwrap();
    let Some(dep) = deployments.get(slug) else { return err(404, format!("no deployment {slug:?}")) };
    let wanted = request.get_param("unit").filter(|u| *u != to_hex(&dep.unit));
    let doc = match &wanted {
        None => dep.document.clone(),
        Some(unit) => dep.versions.iter().rev().find(|v| &v.unit == unit).and_then(|v| version_document(store, v)),
    };
    match (doc, wanted) {
        (Some(doc), _) => Response::json(&doc),
        (None, None) => err(404, format!("{slug:?} was deployed without its canvas; deploy it again from the editor")),
        (None, Some(unit)) => err(404, format!("{slug:?} keeps no canvas of version {}", &unit[..unit.len().min(12)])),
    }
}

/// `GET /deployments/<slug>/versions` — every version deployed, newest first: its number, unit,
/// when and by whom, the version it restored, whether it is the one deployed now, whether it
/// can be restored (what its deploy asked is kept), and the log's seq at its deploy.
pub(crate) fn handle_get_versions(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> Response {
    let deployments = deployments.lock().unwrap();
    let Some(dep) = deployments.get(slug) else { return err(404, format!("no deployment {slug:?}")) };
    let current = dep.versions.last().map(|v| v.number);
    let versions: Vec<serde_json::Value> = dep
        .versions
        .iter()
        .enumerate()
        .rev()
        .map(|(i, v)| {
            serde_json::json!({
                "number": v.number,
                "unit": v.unit,
                "deployed_at": v.deployed_at,
                "by": v.by,
                "restored_from": v.restored_from,
                "current": Some(v.number) == current,
                "restorable": v.request.is_some(),
                "canvas": v.document_ref.is_some(),
                // The state it left is kept: the one the next version carried.
                "state_kept": dep.versions.get(i + 1).is_some_and(|next| next.state_at_deploy.is_some()),
                // Where it starts in the log: the seq its first run got (absent from before rc.30).
                "first_seq": v.first_seq,
            })
        })
        .collect();
    Response::json(&serde_json::json!({ "slug": slug, "versions": versions }))
}

/// `POST /deployments/<slug>/restore` — go back to an earlier point (docs/L4s/history-L4.md §4): a
/// version (`"version": n`) or a run (`"run": seq`), its code (`"code": true`), its state
/// (`"state": true`), or both.
///
/// - **Code** is deployed again: the version's unit (for a run, the version it ran on), with what
///   its deploy asked and its canvas, as a new version that names the one it restores. It is a
///   deploy like any other, checked against today's policy and grant rather than those it had:
///   what it needs that isn't granted now is approved again (`acknowledge`).
/// - **State** is put back as the live state: the state a version left (the one the version after
///   it carried when deployed), or a run left. The session ends, keeping the state it replaces,
///   and a new one starts from the one put back (`Lifecycle::restart`), so a restore can be undone
///   in turn. The deployment must be running or paused.
///
/// Both together: the code first, then the state, and nothing changes if the state can't be read.
/// `"dry_run": true` changes nothing and says what it would: the deploy's preview for code, and for
/// state, the keys it would change and the runs logged since that point, with the outside effects
/// they performed, which a restore doesn't undo.
pub(crate) fn handle_post_restore(state: &AppState, slug: &str, request: &Request, by: Option<&str>, agent: bool) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let restore: RestoreRequestBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("not a valid restore request JSON: {e}")),
    };
    if !restore.code && !restore.state {
        return err(400, "say what to restore: \"code\": true, \"state\": true, or both");
    }
    let point = match (restore.version, restore.run) {
        (Some(n), None) => Point::Version(n),
        (None, Some(seq)) => Point::Run(seq),
        _ => return err(400, "name one point to go back to: \"version\": N or \"run\": SEQ"),
    };
    let Plan { code, state: put_back } = match restore_plan(state, slug, point, restore.code, restore.state) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let code = code.map(|(n, body)| (n, DeployRequestBody { acknowledge: restore.acknowledge, ..body }));
    if restore.dry_run {
        let mut answer = match &code {
            Some((_, body)) => match preview_value(&state.store, &state.deployments, &state.policy(), &state.secrets, &state.folders, body, agent) {
                Ok(v) => v,
                Err(r) => return r,
            },
            None => serde_json::json!({}),
        };
        answer["code"] = serde_json::json!(code.as_ref().map(|(n, _)| n));
        if let Some((snapshot, label, since)) = &put_back {
            answer["state"] = match state_change(state, slug, snapshot, label, since) {
                Ok(v) => v,
                Err(r) => return r,
            };
        }
        return Response::json(&answer);
    }
    // The state is read (and checked) before anything changes.
    let put_back = match put_back.map(|(snapshot, label, _)| crate::restore::load_snapshot(&snapshot, &state.store).map(|s| (s, label.clone())).map_err(|e| (e, label))).transpose() {
        Ok(p) => p,
        Err((e, label)) => return err(500, format!("the state {label} left can't be read: {e}")),
    };
    let mut answer = serde_json::json!({ "slug": slug });
    if let Some((number, body)) = code {
        let deployed =
            deploy(&state.store, &state.deployments, &state.deployments_path, &state.policy(), &state.secrets, &state.folders, body, by, agent, &state.activity, Some(number));
        if !deployed.is_success() {
            return deployed;
        }
        if let Some(v) = state.deployments.lock().unwrap().get(slug).and_then(|d| d.versions.last()) {
            answer["version"] = serde_json::json!(v.number);
            answer["unit"] = serde_json::json!(v.unit);
        }
        answer["code"] = serde_json::json!(number);
    }
    if let Some((loaded, label)) = put_back {
        let session = match put_state_back(state, slug, loaded, &label, by) {
            Ok(n) => n,
            Err(r) => return r,
        };
        persist_deployments(&state.deployments_path, &state.deployments, &state.store);
        state.activity.record("restored state", slug, by);
        answer["state"] = serde_json::json!({ "from": label, "session": session });
    }
    Response::json(&answer)
}

/// The point a restore goes back to.
#[derive(Clone, Copy)]
enum Point {
    Version(u32),
    Run(u64),
}

/// What a restore puts back, worked out before anything changes.
struct Plan {
    /// The version whose code is deployed again, and its deploy as it asked it.
    code: Option<(u32, DeployRequestBody)>,
    /// The snapshot put back as the state, what it is (`"version 3"`, `"run 41"`), and the runs
    /// logged since.
    state: Option<(String, String, Vec<LogEntry>)>,
}

fn restore_plan(state: &AppState, slug: &str, point: Point, code: bool, with_state: bool) -> Result<Plan, Response> {
    let deployments = state.deployments.lock().unwrap();
    let Some(dep) = deployments.get(slug) else { return Err(err(404, format!("no deployment {slug:?}"))) };
    let current = dep.versions.last();
    if with_state {
        if dep.state.is_none() {
            return Err(err(409, format!("{slug:?} keeps no state: there is none to restore")));
        }
        if dep.life.status == Status::Stopped {
            return Err(err(409, format!("can't restore {slug:?}'s state: {}", crate::lifecycle::LifecycleError::Stopped.message())));
        }
    }
    let kept = dep.log.after(None, usize::MAX);
    let (version, put_back) = match point {
        Point::Version(n) => {
            let Some(i) = dep.versions.iter().position(|v| v.number == n) else {
                let newest = current.map_or(0, |v| v.number);
                return Err(err(404, format!("{slug:?} has no version {n}: its versions are 1 to {newest}")));
            };
            let Some(next) = dep.versions.get(i + 1) else {
                let has = if with_state { ", and the state it left is the live one" } else { "" };
                return Err(err(409, format!("version {n} is the one deployed now{has}")));
            };
            let put_back = if with_state {
                let Some(snapshot) = next.state_at_deploy.clone() else {
                    return Err(err(
                        409,
                        format!(
                            "the state version {n} left wasn't kept: version {} was deployed before Holon kept the state a deploy carries (rc.29 or earlier)",
                            next.number
                        ),
                    ));
                };
                let since = next.first_seq.unwrap_or_default();
                Some((snapshot, format!("version {n}"), kept.into_iter().filter(|e| e.seq >= since).collect()))
            } else {
                None
            };
            (code.then(|| dep.versions[i].clone()), put_back)
        }
        Point::Run(seq) => {
            let Some(entry) = kept.iter().find(|e| e.seq == seq) else {
                return Err(err(
                    404,
                    if seq >= dep.log.next_seq() {
                        format!("{slug:?} has no run {seq} yet")
                    } else {
                        format!("run {seq} isn't in {slug:?}'s log any more: it keeps the newest {} runs", crate::recorder::keep())
                    },
                ));
            };
            let put_back = if with_state {
                let Some(snapshot) = entry.state_after.clone() else {
                    return Err(err(
                        409,
                        format!("run {seq} left no state to restore: it didn't commit, or it ran before Holon kept the state runs leave (rc.29 or earlier)"),
                    ));
                };
                Some((snapshot, format!("run {seq}"), kept.iter().filter(|e| e.seq > seq).cloned().collect()))
            } else {
                None
            };
            // Its code: the newest version deployed with the unit it ran on, unless that is the
            // code deployed now.
            let version = if !code {
                None
            } else if current.is_some_and(|c| c.unit == entry.unit) {
                if !with_state {
                    return Err(err(409, format!("run {seq} ran on the code deployed now")));
                }
                None
            } else {
                match dep.versions.iter().rev().find(|v| v.unit == entry.unit) {
                    Some(v) => Some(v.clone()),
                    None => return Err(err(409, format!("the code run {seq} ran on isn't one of {slug:?}'s versions"))),
                }
            };
            (version, put_back)
        }
    };
    let code = match version {
        None => None,
        Some(v) => {
            let Some(asked) = v.request.clone() else {
                return Err(err(
                    409,
                    format!(
                        "version {} was deployed before Holon kept what a deploy asks (rc.29 or earlier), so it can't be restored as it was: open its canvas and deploy it again from the editor",
                        v.number
                    ),
                ));
            };
            let body = DeployRequestBody {
                slug: slug.to_string(),
                unit: v.unit.clone(),
                interval_seconds: asked.interval_seconds,
                acknowledge: Vec::new(),
                document: version_document(&state.store, &v),
                requirements: asked.requirements,
                uses: asked.uses,
                browsers: asked.browsers,
            };
            Some((v.number, body))
        }
    };
    Ok(Plan { code, state: put_back })
}

/// What putting `snapshot` back would change: the keys whose values differ, those it brings back
/// and those it drops (each with its value now and then; a collection, its items that differ),
/// and the runs logged since `label`, with the outside effects they performed (calls, files,
/// database rows, messages), which a restore doesn't undo. Writes to the deployment's own state
/// and collections aren't counted: those are what it puts back.
fn state_change(state: &AppState, slug: &str, snapshot: &str, label: &str, since: &[LogEntry]) -> Result<serde_json::Value, Response> {
    let back = crate::restore::snapshot_refs(snapshot, &state.store).map_err(|e| err(500, format!("the state {label} left can't be read: {e}")))?;
    let (live, redactor, before) = match state.deployments.lock().unwrap().get(slug) {
        Some(d) => {
            // The runs before the point: what a file was there, the last write before it.
            let first = since.first().map_or(u64::MAX, |e| e.seq);
            let before = if since.is_empty() { Vec::new() } else { d.log.after(None, usize::MAX).into_iter().filter(|e| e.seq < first).collect() };
            (d.state.clone(), Some(Redactor::of(&d.secrets)), before)
        }
        None => (None, None, Vec::new()),
    };
    let now = live.as_ref().map(|arc| arc.state.lock().unwrap().refs()).unwrap_or_default();
    let changed: Vec<&String> = back.iter().filter(|(k, h)| now.get(*k).is_some_and(|n| n != *h)).map(|(k, _)| k).collect();
    let added: Vec<&String> = back.keys().filter(|k| !now.contains_key(*k)).collect();
    let removed: Vec<&String> = now.keys().filter(|k| !back.contains_key(*k)).collect();
    // Each key it touches, as it is now and as it would be (null: absent), for a diff; a
    // collection's, the items that differ, its root saying nothing to a person. Past
    // STATE_DIFF_KEYS keys, the names alone say what changes. Redacted as the control plane is.
    let unreadable = |e: StoreError| err(500, format!("the state {label} left can't be read: {e:?}"));
    let mut values = serde_json::Map::new();
    let mut collections = serde_json::Map::new();
    for key in changed.iter().chain(&added).chain(&removed).take(STATE_DIFF_KEYS) {
        let then = back.get(*key).map(|h| state.store.get_value(h)).transpose().map_err(unreadable)?.flatten();
        let now = live.as_ref().and_then(|arc| arc.state.lock().unwrap().get(key).cloned());
        if let Some(name) = key.strip_prefix("#collection:") {
            collections.insert(name.to_string(), collection_change(&state.store, now.as_ref(), then.as_ref()).map_err(|e| err(500, format!("the state {label} left can't be read: {e}")))?);
        } else {
            values.insert((*key).clone(), serde_json::json!({ "now": now.as_ref().map(SValue::from_core), "then": then.as_ref().map(SValue::from_core) }));
        }
    }
    let files = folders::files_since(&folders::Scoped::new(Arc::clone(&state.folders), slug), &before, since);
    let mut values = serde_json::json!({ "values": values, "collections": collections, "files": files });
    if let Some(r) = &redactor {
        r.json(&mut values);
    }
    let mut effects: BTreeMap<&str, u64> = BTreeMap::new();
    for p in since.iter().flat_map(|e| &e.performed) {
        let name = p.capability.split('(').next().unwrap_or_default();
        if name != "StateWrite" && name != "CollectionWrite" && !name.ends_with("Read") {
            *effects.entry(name).or_default() += 1;
        }
    }
    Ok(serde_json::json!({
        "from": label,
        "changed": changed,
        "added": added,
        "removed": removed,
        "values": values["values"],
        "collections": values["collections"],
        // The files the runs since wrote, which a restore doesn't put back, as they are now.
        "files": values["files"]["files"],
        "files_more": values["files"]["more"],
        "runs_since": since.len(),
        "effects_since": effects,
    }))
}

/// How many keys a restore's dry run shows the values of.
const STATE_DIFF_KEYS: usize = 50;
/// How many items of a collection a restore's dry run compares, and shows of those that differ.
const COLLECTION_DIFF_READ: usize = 1000;
const COLLECTION_DIFF_ITEMS: usize = 50;

/// The items that differ between a collection now and as a restore would put it back (each its
/// cell's value, a root; absent: no collection), as `{now, then}` by key; `more` when it compared
/// or shows only some of them.
fn collection_change(store: &SharedStore, now: Option<&Value>, then: Option<&Value>) -> Result<serde_json::Value, String> {
    let read = |cell: Option<&Value>| -> Result<(BTreeMap<String, Value>, bool), String> {
        let Some(root) = cell.and_then(|v| match v {
            Value::Text(hex) => parse_hash(hex),
            _ => None,
        }) else {
            return Ok((BTreeMap::new(), false));
        };
        let items = holon_host::collection::list(store, &root, "", None, COLLECTION_DIFF_READ + 1).map_err(|e| format!("{e:?}"))?;
        let more = items.len() > COLLECTION_DIFF_READ;
        Ok((items.into_iter().take(COLLECTION_DIFF_READ).collect(), more))
    };
    let ((now, more_now), (then, more_then)) = (read(now)?, read(then)?);
    // A side cut short is read up to its last key: past the first such key, neither is compared.
    let bound = [(more_now, &now), (more_then, &then)].into_iter().filter(|(more, _)| *more).filter_map(|(_, side)| side.keys().next_back()).min();
    let keys: BTreeSet<&String> =
        now.keys().chain(then.keys()).filter(|k| bound.is_none_or(|b| *k <= b) && now.get(*k) != then.get(*k)).collect();
    let items: serde_json::Map<String, serde_json::Value> = keys
        .iter()
        .take(COLLECTION_DIFF_ITEMS)
        .map(|k| ((*k).clone(), serde_json::json!({ "now": now.get(*k).map(SValue::from_core), "then": then.get(*k).map(SValue::from_core) })))
        .collect();
    Ok(serde_json::json!({ "items": items, "more": more_now || more_then || keys.len() > COLLECTION_DIFF_ITEMS }))
}

/// Make `loaded` `slug`'s live state, in a new session: the one it replaces is kept as the ending
/// session's, as a stop keeps it. Answers the new session's number.
fn put_state_back(state: &AppState, slug: &str, loaded: State, label: &str, by: Option<&str>) -> Result<u32, Response> {
    let mut table = state.deployments.lock().unwrap();
    let Some(dep) = table.get_mut(slug) else { return Err(err(404, format!("no deployment {slug:?}"))) };
    let Some(arc) = dep.state.clone() else { return Err(err(409, format!("{slug:?} keeps no state: there is none to restore"))) };
    let mut live = arc.state.lock().unwrap();
    let refs = live.save(&mut state.store.clone()).map_err(|e| err(500, format!("the state it has now couldn't be kept: {e:?}")))?;
    dep.life.restart(Some(&refs), label, epoch_now()).map_err(|e| err(409, format!("can't restore {slug:?}'s state: {}", e.message())))?;
    *live = loaded;
    drop(live);
    let session = dep.life.sessions.last().map_or(0, |s| s.number);
    dep.note("restore", format!("put back the state {label} left, in session {session}"), by, crate::events::Detail { session: Some(session), from: Some(label.to_string()), ..Default::default() });
    Ok(session)
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
pub(crate) fn handle_lifecycle(state: &AppState, slug: &str, action: &str, request: &Request, by: Option<&str>) -> Response {
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
        let sessions_before = dep.life.sessions.len();
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
                dep.arm_clocks(SystemTime::now());
            }),
        };
        if let Err(e) = result {
            return err(409, format!("can't {action} {slug:?}: {}", e.message()));
        }
        // What it did: a run after a stop started a session, named after where its state came from.
        let started = dep.life.sessions.last().filter(|_| dep.life.sessions.len() > sessions_before).map(|s| (s.number, s.from.clone()));
        let (what, session) = match (action, started) {
            ("pause", _) => ("paused".to_string(), None),
            ("stop", _) => ("stopped".to_string(), None),
            (_, Some((n, from))) => (format!("ran from {from}, in session {n}"), Some(n)),
            _ => ("resumed".to_string(), None),
        };
        dep.note("lifecycle", what, by, crate::events::Detail { session, ..Default::default() });
        // A paused or stopped deployment answers nobody: its subscribers hear so.
        if dep.life.status != Status::Running {
            dep.channels.close();
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

