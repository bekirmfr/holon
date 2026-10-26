//! Changing what a deployment is granted while it is deployed (§4.2.1, lifecycle point 4): the
//! host revokes an item, grants back one the manifest asks for, or lowers a limit, and can reload
//! its policy so a lowered ceiling or a withdrawn exception applies to every deployment, all
//! without a redeploy.
//!
//! After any of these, the deployment is re-checked like at load: a grant that no longer covers
//! what it needs (or that the policy no longer allows) pauses it, flagged, unless the host chose to
//! keep it running; a grant that covers it again resumes it, if the grant was what paused it. Its
//! permit is rebuilt, so the next run is held to the new grant, waiting runs included.

use holon_core::Capabilities;

use crate::permissions::ChangeRequest;
use crate::*;

/// What `unit` needs as its graph derives it, read from the store with no table lock held;
/// `scheduled` (deployed with an interval: a clock flow), with the clock it runs on.
pub(crate) fn derived_capabilities(store: &SharedStore, unit: Hash, scheduled: bool) -> Result<Capabilities, String> {
    let resolver = materialize_closure(store, unit).map_err(|e| format!("store error while materializing closure: {e:?}"))?;
    let root = resolver.resolve(&unit).ok_or("the deployed unit no longer resolves in the store")?;
    Ok(capabilities(root, &resolver).deployed(scheduled))
}

/// Re-check `dep` against its grant and `policy`: its attention, its status where the grant
/// decides it, and its permit.
pub(crate) fn recheck(dep: &mut Deployment, caps: &Capabilities, policy: &Policy, now: u64) {
    let manifest = dep.grant.manifest_of(caps.iter());
    dep.attention = dep.grant.attention(&manifest, policy);
    match (&dep.attention, dep.life.status) {
        (Some(why), Status::Running) => {
            let _ = dep.life.pause();
            dep.channels.close();
            dep.grant.paused = true;
            eprintln!("holon: paused: {why}");
        }
        (None, Status::Paused) if dep.grant.paused => {
            let _ = dep.life.run(RunFrom::Latest, now);
            dep.grant.paused = false;
            if dep.interval_seconds.is_some() {
                dep.next_due = Some(SystemTime::now());
            }
        }
        _ => {}
    }
    dep.permit = Arc::new(Permit::new(dep.unit, &dep.grant, policy));
}

/// What the grant view says about a deployment: its grant, the grant's hash, and why it can't
/// run, if it can't.
fn grant_view(dep: &Deployment) -> serde_json::Value {
    serde_json::json!({
        "grant": dep.grant,
        "hash": to_hex(&dep.grant.hash()),
        "attention": dep.attention,
        "status": dep.life.status,
    })
}

/// `GET /admin/grants` — who holds what: each deployment's granted items and those revoked since,
/// with its status and why it can't run, if it can't. The admin page's permissions turn it round,
/// by resource (`docs/L4s/databases-L4.md` §9.1); a grant changes only on its deployment's card
/// (`POST /deployments/<slug>/grant`).
pub(crate) fn handle_get_admin_grants(state: &AppState) -> Response {
    let table = state.deployments.lock().unwrap();
    let held: Vec<serde_json::Value> = table
        .iter()
        .map(|(slug, dep)| {
            serde_json::json!({
                "slug": slug,
                "status": dep.life.status,
                "attention": dep.attention,
                "items": dep.grant.items,
                "revoked": dep.grant.revoked,
            })
        })
        .collect();
    Response::json(&held)
}

/// `POST /deployments/<slug>/grant` — change what it is granted:
/// `{"revoke": [item], "grant": [item], "lower": {limit: n}, "keep_running": bool}`, every part
/// optional. Applied whole or not at all; answers with the grant as it now is.
pub(crate) fn handle_post_grant(state: &AppState, slug: &str, request: &Request) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let change: ChangeRequest = match serde_json::from_slice(&bytes) {
        Ok(c) => c,
        Err(e) => return err(400, format!("not a valid grant change: {e}")),
    };
    let Some((unit, scheduled)) = state.deployments.lock().unwrap().get(slug).map(|d| (d.unit, d.interval_seconds.is_some())) else {
        return err(404, format!("no deployment {slug:?}"));
    };
    let caps = match derived_capabilities(&state.store, unit, scheduled) {
        Ok(c) => c,
        Err(e) => return err(500, e),
    };
    let policy = state.policy();
    let reply = {
        let mut table = state.deployments.lock().unwrap();
        let Some(dep) = table.get_mut(slug) else {
            return err(404, format!("no deployment {slug:?}"));
        };
        if dep.unit != unit {
            return err(409, format!("{slug:?} was redeployed meanwhile; look at its grant again"));
        }
        let manifest = dep.grant.manifest_of(caps.iter());
        let now = epoch_now();
        match dep.grant.change(&manifest, &policy, &change, now) {
            Ok(grant) => {
                for c in grant.changes.iter().skip(dep.grant.changes.len()) {
                    eprintln!("holon: grant of {slug:?}: {}", c.change);
                }
                dep.grant = grant;
            }
            Err(e) => return err(400, e),
        }
        recheck(dep, &caps, &policy, now);
        grant_view(dep)
    };
    persist_deployments(&state.deployments_path, &state.deployments, &state.store);
    sync_listeners(state);
    Response::json(&reply)
}

/// `POST /permissions/reload` — read the policy file again and hold every deployment to it: a
/// lowered ceiling or a withdrawn exception applies at once. A file that doesn't parse changes
/// nothing. Answers with the policy and each deployment's attention.
pub(crate) fn handle_post_permissions_reload(state: &AppState) -> Response {
    let policy = match Policy::load(&state.policy_path) {
        Ok(p) => Arc::new(p),
        Err(e) => return err(400, format!("the policy wasn't reloaded: {e}")),
    };
    let units: Vec<(String, Hash, bool)> = state.deployments.lock().unwrap().iter().map(|(s, d)| (s.clone(), d.unit, d.interval_seconds.is_some())).collect();
    let mut derived = BTreeMap::new();
    for (slug, unit, scheduled) in units {
        match derived_capabilities(&state.store, unit, scheduled) {
            Ok(caps) => {
                derived.insert(slug, (unit, caps));
            }
            Err(e) => return err(500, format!("{slug:?}: {e}")),
        }
    }
    let attention: BTreeMap<String, Option<String>> = {
        let mut table = state.deployments.lock().unwrap();
        *state.policy.write().unwrap() = Arc::clone(&policy);
        let now = epoch_now();
        table
            .iter_mut()
            .map(|(slug, dep)| {
                // Redeployed meanwhile: that deploy already checked it against the policy it saw.
                if let Some((unit, caps)) = derived.get(slug)
                    && *unit == dep.unit
                {
                    recheck(dep, caps, &policy, now);
                }
                (slug.clone(), dep.attention.clone())
            })
            .collect()
    };
    eprintln!("holon: policy reloaded from {:?}", state.policy_path);
    persist_deployments(&state.deployments_path, &state.deployments, &state.store);
    sync_listeners(state);
    let mut reply = permissions_view(&policy);
    reply["attention"] = serde_json::to_value(attention).unwrap_or_default();
    Response::json(&reply)
}
