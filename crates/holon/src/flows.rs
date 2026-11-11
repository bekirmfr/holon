//! Saved flows: the editor's canvas documents, by name.

use crate::*;
use crate::access::Caller;
use crate::can::{can, Action, Resource};

/// `POST /flows` — save (upsert) a flow document by name. The name follows the same rule as
/// deployment slugs (`is_valid_slug`) — one naming convention across the API, not two. The
/// document is stored as-is; the server never inspects inside it (see `AppState::flows`).
pub(crate) fn handle_post_flows(
    flows: &Mutex<BTreeMap<String, FlowRecord>>,
    flows_path: &Place,
    request: &Request,
    // Who saves it: an agent replaces only its own.
    who: &Caller,
    activity: &crate::activity::Activity,
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
    match save(flows, flows_path, &body.name, body.document, body.uses.clone(), who, activity) {
        Ok(saved_at_epoch_seconds) => Response::json(&FlowListEntry { name, saved_at_epoch_seconds, saved_by: who.name.clone(), uses: body.uses }),
        Err(refused) => refused,
    }
}

/// Save `document` as the flow `name`, by `who`, and answer the save's stamp; replacing a flow is
/// `can`'s to let, and an agent replaces only a flow it saved itself (403 otherwise). An agent's
/// proposal saves its flow this way too (proposals.rs).
pub(crate) fn save(
    flows: &Mutex<BTreeMap<String, FlowRecord>>,
    flows_path: &Place,
    name: &str,
    document: serde_json::Value,
    uses: Option<Vec<HolonUse>>,
    who: &Caller,
    activity: &crate::activity::Activity,
) -> Result<u64, Response> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let by = who.name.as_deref();
    let mut table = flows.lock().unwrap();
    if let Some(saved) = table.get(name) {
        can(who, Action::FlowEdit, &Resource::Saved { name, by: saved.saved_by.as_deref() }).map_err(|refusal| refusal.response())?;
    }
    // Each save of a flow is stamped later than the one before, even within the same second: the
    // stamp is how a flow placed from it tells an older save from the newest (holonUsage.ts).
    let saved_at_epoch_seconds = table.get(name).map_or(now, |p| now.max(p.saved_at_epoch_seconds + 1));
    table.insert(name.to_string(), FlowRecord { name: name.to_string(), document, saved_at_epoch_seconds, saved_by: by.map(String::from), uses });
    persist_flows(flows_path, &table);
    activity.record("saved", name, by);
    Ok(saved_at_epoch_seconds)
}

/// `GET /flows` — every saved flow's name + freshness, never the documents themselves.
pub(crate) fn handle_get_flows(flows: &Mutex<BTreeMap<String, FlowRecord>>) -> Response {
    let table = flows.lock().unwrap();
    let entries: Vec<FlowListEntry> = table
        .values()
        .map(|f| FlowListEntry { name: f.name.clone(), saved_at_epoch_seconds: f.saved_at_epoch_seconds, saved_by: f.saved_by.clone(), uses: f.uses.clone() })
        .collect();
    Response::json(&entries)
}

/// `GET /flows/<name>` — the full saved record, document included. 404 for an unknown name,
/// same convention as every other by-name lookup on this API.
pub(crate) fn handle_get_flow(flows: &Mutex<BTreeMap<String, FlowRecord>>, name: &str) -> Response {
    let table = flows.lock().unwrap();
    match table.get(name) {
        Some(record) => Response::json(record),
        None => err(404, "no flow saved under that name"),
    }
}

/// `DELETE /flows/<name>` — remove a saved flow. 404 if it was never there.
pub(crate) fn handle_delete_flow(
    flows: &Mutex<BTreeMap<String, FlowRecord>>,
    flows_path: &Place,
    name: &str,
) -> Response {
    let mut table = flows.lock().unwrap();
    if table.remove(name).is_none() {
        return err(404, "no flow saved under that name");
    }
    persist_flows(flows_path, &table);
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
