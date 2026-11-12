//! What was done on the control plane, by whom: each save, deploy, run, pause, stop, grant and
//! delete, appended to `<deployments>.activity.jsonl` beside the deployments file (in memory for
//! an in-memory server). A deployment or flow that is deleted takes its own records with it;
//! this log keeps that it was, and who did it. Secrets have their own audit (secrets.rs).
//! `GET /activity?limit=n` answers the newest first: all of it to a caller who views every flow,
//! and only what was done to the flows they view to one bound at some flows.

use std::path::PathBuf;
use crate::sync::Mutex;

use rouille::{Request, Response};

use crate::access::Caller;
use crate::can::{can, everywhere, Action, Resource};
use crate::paths::Place;
use serde::{Deserialize, Serialize};

/// One thing done.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub(crate) struct ActivityEntry {
    pub(crate) at: u64,
    /// `saved`, `deleted flow`, `deployed`, `run`, `paused`, `stopped`, `granted`, `deleted`.
    pub(crate) action: String,
    /// The flow's or deployment's name.
    pub(crate) target: String,
    /// Who did it (access.rs's caller); absent on an open loopback server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) by: Option<String>,
    /// What they said of it: a deploy's note, kept with its grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

pub(crate) struct Activity {
    path: Option<PathBuf>,
    kept: Mutex<Vec<ActivityEntry>>,
}

/// The most entries `GET /activity` answers.
const MAX_READ: usize = 500;

impl Activity {
    /// Beside the deployments file, or in memory if that is kept nowhere.
    pub(crate) fn beside(deployments_path: &Place) -> Activity {
        let path = deployments_path.beside("activity.jsonl").map(PathBuf::from);
        Activity { path, kept: Mutex::new(Vec::new()) }
    }

    #[cfg(test)]
    pub(crate) fn memory() -> Activity {
        Activity::beside(&Place::Nowhere)
    }

    pub(crate) fn record(&self, action: &str, target: &str, by: Option<&str>) {
        self.record_noted(action, target, by, None);
    }

    /// Record it with what its doer said of it.
    pub(crate) fn record_noted(&self, action: &str, target: &str, by: Option<&str>, note: Option<&str>) {
        let entry = ActivityEntry { at: crate::deploy::epoch_now(), action: action.into(), target: target.into(), by: by.map(String::from), note: note.map(String::from) };
        let Some(path) = &self.path else {
            self.kept.lock().unwrap().push(entry);
            return;
        };
        let _one_at_a_time = self.kept.lock().unwrap();
        let appended = (|| -> std::io::Result<()> {
            use std::io::Write;
            let line = serde_json::to_string(&entry).map_err(std::io::Error::other)? + "\n";
            crate::sealing::append_private(path)?.write_all(line.as_bytes())
        })();
        if let Err(e) = appended {
            eprintln!("holon: could not append to the activity log {path:?}: {e}");
        }
    }

    /// The newest `limit` entries that `keep` keeps, newest first.
    pub(crate) fn newest(&self, limit: usize, keep: impl Fn(&ActivityEntry) -> bool) -> Vec<ActivityEntry> {
        let all: Vec<ActivityEntry> = match &self.path {
            None => self.kept.lock().unwrap().clone(),
            Some(path) => std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect(),
        };
        all.into_iter().rev().filter(keep).take(limit.min(MAX_READ)).collect()
    }
}

/// What is done to a flow, by the words its entry is kept in (flows.rs, deploy.rs, proposals.rs,
/// durable.rs, http.rs). What isn't here is shown only to whoever views every flow.
const DONE_TO_A_FLOW: &[&str] =
    &["saved", "deleted flow", "deployed", "restored", "restored state", "asked to restore", "proposed", "declined", "approved", "granted", "run", "paused", "stopped", "deleted"];

/// The flow `e` was done to, if it was: its target, or a decision's run's ("payout run 7 at "ok"").
fn flow_of(e: &ActivityEntry) -> Option<&str> {
    DONE_TO_A_FLOW.contains(&e.action.as_str()).then(|| e.target.split(" run ").next().unwrap_or_default())
}

/// `GET /activity?limit=n` (default 50): what was done, newest first, as far as `who` may see it.
pub(crate) fn handle_get_activity(activity: &Activity, request: &Request, who: &Caller) -> Response {
    let limit = request.get_param("limit").and_then(|l| l.parse().ok()).unwrap_or(50);
    let all = everywhere(who, Action::FlowView);
    let keep = |e: &ActivityEntry| all || flow_of(e).is_some_and(|slug| can(who, Action::FlowView, &Resource::Flow(slug)).is_ok());
    Response::json(&serde_json::json!({ "entries": activity.newest(limit, keep) }))
}
