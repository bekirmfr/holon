//! Saved flows: the editor's canvas documents, by name.

use crate::*;

/// `POST /flows` — save (upsert) a flow document by name. The name follows the same rule as
/// deployment slugs (`is_valid_slug`) — one naming convention across the API, not two. The
/// document is stored as-is; the server never inspects inside it (see `AppState::flows`).
pub(crate) fn handle_post_flows(
    flows: &Mutex<BTreeMap<String, FlowRecord>>,
    flows_path: &str,
    request: &Request,
    by: Option<&str>,
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
    let saved_at_epoch_seconds =
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let saved_by = by.map(String::from);
    let record = FlowRecord { name: body.name.clone(), document: body.document, saved_at_epoch_seconds, saved_by: saved_by.clone() };

    let mut table = flows.lock().unwrap();
    table.insert(body.name.clone(), record);
    persist_flows(flows_path, &table);

    Response::json(&FlowListEntry { name: body.name, saved_at_epoch_seconds, saved_by })
}

/// `GET /flows` — every saved flow's name + freshness, never the documents themselves.
pub(crate) fn handle_get_flows(flows: &Mutex<BTreeMap<String, FlowRecord>>) -> Response {
    let table = flows.lock().unwrap();
    let entries: Vec<FlowListEntry> = table
        .values()
        .map(|f| FlowListEntry { name: f.name.clone(), saved_at_epoch_seconds: f.saved_at_epoch_seconds, saved_by: f.saved_by.clone() })
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
    flows_path: &str,
    name: &str,
) -> Response {
    let mut table = flows.lock().unwrap();
    if table.remove(name).is_none() {
        return err(404, "no flow saved under that name");
    }
    persist_flows(flows_path, &table);
    Response::json(&serde_json::json!({ "deleted": name }))
}
