//! Saved flows, by name: each its text, and where its canvas was drawn by hand when it was
//! (docs/L4s/text-deploy-plan.md).

use crate::*;
use crate::access::Caller;
use crate::can::{can, Action, Resource};

/// `POST /flows` — save (upsert) a flow's text by name, with its hand-drawn layout. The name
/// follows the same rule as deployment slugs (`is_valid_slug`) — one naming convention across the
/// API, not two. The text is kept as it is, compiling or not (a draft); the layout is checked to be
/// one a server keeps (`kept_layout::check`). The body may come gzipped (`read_flow_body`).
/// `base` says which save it was made from (`saved_at_epoch_seconds`; null: a new flow, none saved
/// under the name), and it is saved only while that is the save there (`save`).
pub(crate) fn handle_post_flows(
    state: &AppState,
    request: &Request,
    // Who saves it: an agent replaces only its own.
    who: &Caller,
) -> Response {
    let bytes = match read_flow_body(request) {
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
    match save(state, &body.name, body.text, body.layout, body.uses.clone(), who, body.base) {
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

/// Save `text` (with `layout`) as the flow `name`, by `who`, and answer the save's stamp; saving it is
/// `can`'s to let, a new one as `who`'s own, and an agent replaces only a flow it saved itself
/// (403 otherwise). An agent's proposal saves its flow this way too (proposals.rs).
///
/// `base` is the save it was made from (`Some(None)`: none, a new flow; `None`: not said). One made
/// from another than the save there now (a colleague's or an agent's since, or none since it was
/// removed) is 409, with the save there now: saved, it would quietly put back what they changed.
/// Kept or not as one with the file: a save that isn't written (a full disk) answers 500 and leaves
/// the flow as it was, and one past what this server keeps is refused (`MAX_SAVED`).
pub(crate) fn save(
    state: &AppState,
    name: &str,
    text: String,
    layout: Option<holon_author::kept_layout::Layout>,
    uses: Option<Vec<HolonUse>>,
    who: &Caller,
    base: Option<Option<u64>>,
) -> Result<u64, Response> {
    if let Some(why) = layout.as_ref().and_then(|l| holon_author::kept_layout::check(l).err()) {
        return Err(err(400, why));
    }
    let size = record_size(&text, layout.as_ref());
    if size > MAX_FLOW_BYTES {
        return Err(err(413, format!("a flow is saved with a text and layout of at most {MAX_FLOW_BYTES} bytes, and this one is {size}")));
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let by = who.name.as_deref();
    let deployed = deployed(state, name);
    let mut table = state.flows.lock().unwrap();
    let saved_by = table.get(name).map_or(by, |saved| saved.saved_by.as_deref());
    may(who, Action::FlowEdit, name, saved_by, deployed).map_err(|refusal| refusal.response())?;
    let there = table.get(name).map(|p| p.saved_at_epoch_seconds);
    if let Some(base) = base.filter(|base| *base != there) {
        return Err(stale(name, base, table.get(name)));
    }
    if there.is_none() && table.len() >= MAX_SAVED {
        return Err(err(409, format!("{MAX_SAVED} flows are saved here, the most a server keeps: remove one first")));
    }
    let others: usize = table.values().filter(|f| f.name != name).map(|f| record_size(&f.text, f.layout.as_ref())).sum();
    if others + size > MAX_SAVED_BYTES {
        return Err(err(409, format!("the saved flows would come to {} bytes, past the {MAX_SAVED_BYTES} a server keeps: remove some first", others + size)));
    }
    // Each save of a flow is stamped later than the one before, even within the same second: the
    // stamp is how a flow placed from it tells an older save from the newest (holonUsage.ts).
    let saved_at_epoch_seconds = there.map_or(now, |there| now.max(there + 1));
    let before = table.insert(name.to_string(), FlowRecord { name: name.to_string(), text, layout, saved_at_epoch_seconds, saved_by: by.map(String::from), uses });
    if let Err(e) = persist_flows(&state.flows_path, &table) {
        match before {
            Some(before) => table.insert(name.to_string(), before),
            None => table.remove(name),
        };
        return Err(err(500, format!("{name:?} isn't saved: {e}")));
    }
    state.activity.record("saved", name, by);
    Ok(saved_at_epoch_seconds)
}

/// A save made from `base` (none: a new flow) when the flow `name` is `now`: 409, saying what is
/// there instead, with its stamp and who saved it, for the editor to ask whether to replace it.
fn stale(name: &str, base: Option<u64>, now: Option<&FlowRecord>) -> Response {
    let since = match now {
        Some(now) => {
            let by = now.saved_by.as_deref().map(|by| format!(" by {by}")).unwrap_or_default();
            let at = crate::cmdline::when(Some(now.saved_at_epoch_seconds));
            match base {
                Some(_) => format!("{name:?} was saved again{by} at {at} since you opened it"),
                None => format!("{name:?} is saved already{by} at {at}, and this is saved as a new flow"),
            }
        }
        None => format!("{name:?} was removed since you opened it"),
    };
    let then = match (base, now) {
        (None, Some(_)) => "saving yours would replace it; save it under another name, or with its stamp as base to replace it",
        _ => "saving yours would put back what was changed; open it again, or save yours with its stamp as base to replace it",
    };
    let body = serde_json::json!({
        "error": format!("{since}: {then}"),
        "saved_at_epoch_seconds": now.map(|n| n.saved_at_epoch_seconds),
        "saved_by": now.and_then(|n| n.saved_by.clone()),
    });
    Response::json(&body).with_status_code(409)
}

/// `GET /flows` — every saved flow `who` may view: its name + freshness, never the texts
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

/// `GET /flows/<name>` — the full saved record, text and layout included. 404 for an unknown name,
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
    let removed = table.remove(name).expect("it is there");
    if let Err(e) = persist_flows(&state.flows_path, &table) {
        table.insert(name.to_string(), removed);
        return err(500, format!("{name:?} isn't removed: {e}"));
    }
    Response::json(&serde_json::json!({ "deleted": name }))
}

/// The most flows a server keeps saved, and the most bytes their texts and layouts come to: a save
/// past either is refused (409), so a loop of saves (an agent's) can't fill the disk; each save
/// writes the file whole. And the most one flow is saved with (413).
/// Smaller in the tests, which fill them.
pub(crate) const MAX_SAVED: usize = if cfg!(test) { 40 } else { 1000 };
pub(crate) const MAX_SAVED_BYTES: usize = if cfg!(test) { 4 * 1024 * 1024 } else { 64 * 1024 * 1024 };
pub(crate) const MAX_FLOW_BYTES: usize = if cfg!(test) { 1024 * 1024 } else { 4 * 1024 * 1024 };

/// What a saved flow comes to: its text and its layout, as kept.
fn record_size(text: &str, layout: Option<&holon_author::kept_layout::Layout>) -> usize {
    text.len() + layout.and_then(|l| serde_json::to_vec(l).ok()).map_or(0, |l| l.len())
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
