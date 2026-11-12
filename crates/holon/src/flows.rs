//! Saved flows: the editor's canvas documents, by name.

use crate::*;
use crate::access::Caller;
use crate::can::{can, Action, Resource};

/// `POST /flows` — save (upsert) a flow document by name. The name follows the same rule as
/// deployment slugs (`is_valid_slug`) — one naming convention across the API, not two. The
/// document is stored as-is; the server never inspects inside it (see `AppState::flows`).
pub(crate) fn handle_post_flows(
    state: &AppState,
    request: &Request,
    // Who saves it: an agent replaces only its own.
    who: &Caller,
) -> Response {
    let bytes = match read_body(request) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let body: SaveFlowBody = match serde_json::from_slice(&bytes) {
        Ok(b) => b,
        Err(e) => return err(400, format!("invalid save-flow body: {e}")),
    };
    if !is_valid_slug(&body.name) {
        return err(400, "flow name must be non-empty ASCII alphanumeric/'-'/'_' only");
    }
    if let Err(e) = check_uses(body.uses.as_deref()) {
        return err(400, e);
    }
    let name = body.name.clone();
    match save(state, &body.name, body.document, body.uses.clone(), who) {
        Ok(saved_at_epoch_seconds) => Response::json(&FlowListEntry { name, saved_at_epoch_seconds, saved_by: who.name.clone(), uses: body.uses }),
        Err(refused) => refused,
    }
}

/// Whether `who` may do `action` to the saved flow `name`, saved by `saved_by`: as what is bound
/// to them on it says; or, for their draft (one they saved that no one has deployed), wherever
/// they hold the action, as the agent role's own flows are (the plan's D18). Editing asks about
/// whose the flow is: an agent replaces only its own.
fn may(who: &Caller, action: Action, name: &str, saved_by: Option<&str>, deployed: bool) -> Result<(), crate::can::Refusal> {
    let on = if action == Action::FlowEdit { Resource::Saved { name, by: saved_by } } else { Resource::Flow(name) };
    let refused = match can(who, action, &on) {
        Ok(()) => return Ok(()),
        Err(refusal) => refusal,
    };
    let draft = !deployed && saved_by.is_some() && saved_by == who.name.as_deref();
    if draft && can(who, action, &Resource::Flows).is_ok() { Ok(()) } else { Err(refused) }
}

/// Whether a flow named `name` is deployed.
fn deployed(state: &AppState, name: &str) -> bool {
    state.deployments.lock().unwrap().contains_key(name)
}

/// Save `document` as the flow `name`, by `who`, and answer the save's stamp; saving it is
/// `can`'s to let, a new one as `who`'s own, and an agent replaces only a flow it saved itself
/// (403 otherwise). An agent's proposal saves its flow this way too (proposals.rs).
pub(crate) fn save(state: &AppState, name: &str, document: serde_json::Value, uses: Option<Vec<HolonUse>>, who: &Caller) -> Result<u64, Response> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let by = who.name.as_deref();
    let deployed = deployed(state, name);
    let mut table = state.flows.lock().unwrap();
    let saved_by = table.get(name).map_or(by, |saved| saved.saved_by.as_deref());
    may(who, Action::FlowEdit, name, saved_by, deployed).map_err(|refusal| refusal.response())?;
    // Each save of a flow is stamped later than the one before, even within the same second: the
    // stamp is how a flow placed from it tells an older save from the newest (holonUsage.ts).
    let saved_at_epoch_seconds = table.get(name).map_or(now, |p| now.max(p.saved_at_epoch_seconds + 1));
    table.insert(name.to_string(), FlowRecord { name: name.to_string(), document, saved_at_epoch_seconds, saved_by: by.map(String::from), uses });
    persist_flows(&state.flows_path, &table);
    state.activity.record("saved", name, by);
    Ok(saved_at_epoch_seconds)
}

/// `GET /flows` — every saved flow `who` may view: its name + freshness, never the documents
/// themselves.
pub(crate) fn handle_get_flows(state: &AppState, who: &Caller) -> Response {
    let deployed: BTreeSet<String> = state.deployments.lock().unwrap().keys().cloned().collect();
    let table = state.flows.lock().unwrap();
    let entries: Vec<FlowListEntry> = table
        .values()
        .filter(|f| may(who, Action::FlowView, &f.name, f.saved_by.as_deref(), deployed.contains(&f.name)).is_ok())
        .map(|f| FlowListEntry { name: f.name.clone(), saved_at_epoch_seconds: f.saved_at_epoch_seconds, saved_by: f.saved_by.clone(), uses: f.uses.clone() })
        .collect();
    Response::json(&entries)
}

/// `GET /flows/<name>` — the full saved record, document included. 404 for an unknown name,
/// same convention as every other by-name lookup on this API; 403 for one `who` may not view.
pub(crate) fn handle_get_flow(state: &AppState, name: &str, who: &Caller) -> Response {
    let deployed = deployed(state, name);
    let table = state.flows.lock().unwrap();
    match table.get(name) {
        Some(record) => match may(who, Action::FlowView, name, record.saved_by.as_deref(), deployed) {
            Ok(()) => Response::json(record),
            Err(refusal) => refusal.response(),
        },
        None => can(who, Action::FlowView, &Resource::Flow(name)).map_or_else(|r| r.response(), |_| err(404, "no flow saved under that name")),
    }
}

/// `DELETE /flows/<name>` — remove a saved flow. 404 if it was never there; 403 for one `who` may
/// not delete.
pub(crate) fn handle_delete_flow(state: &AppState, name: &str, who: &Caller) -> Response {
    let deployed = deployed(state, name);
    let mut table = state.flows.lock().unwrap();
    let Some(record) = table.get(name) else {
        return can(who, Action::FlowDelete, &Resource::Flow(name)).map_or_else(|r| r.response(), |_| err(404, "no flow saved under that name"));
    };
    if let Err(refusal) = may(who, Action::FlowDelete, name, record.saved_by.as_deref(), deployed) {
        return refusal.response();
    }
    table.remove(name);
    persist_flows(&state.flows_path, &table);
    Response::json(&serde_json::json!({ "deleted": name }))
}

/// The most holons one flow may say it places.
const MAX_USES: usize = 1000;

/// Whether `uses` (a save's or a deploy's) is well formed: flow names as names go, not too many.
pub(crate) fn check_uses(uses: Option<&[HolonUse]>) -> Result<(), String> {
    let uses = uses.unwrap_or_default();
    if uses.len() > MAX_USES {
        return Err(format!("uses: at most {MAX_USES} placed flows"));
    }
    match uses.iter().find(|u| !is_valid_slug(&u.flow)) {
        Some(u) => Err(format!("uses: {:?} isn't a flow name", u.flow)),
        None => Ok(()),
    }
}
