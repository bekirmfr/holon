//! What was done on the control plane, by whom: each save, deploy, run, pause, stop, grant and
//! delete, appended to `<deployments>.activity.jsonl` beside the deployments file (in memory for
//! an in-memory server). A deployment or flow that is deleted takes its own records with it;
//! this log keeps that it was, and who did it. Secrets have their own audit (secrets.rs).
//! `GET /activity?limit=n` answers the newest first.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rouille::{Request, Response};
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
}

pub(crate) struct Activity {
    path: Option<PathBuf>,
    kept: Mutex<Vec<ActivityEntry>>,
}

/// The most entries `GET /activity` answers.
const MAX_READ: usize = 500;

impl Activity {
    /// Beside the deployments file, or in memory for `":memory:"`.
    pub(crate) fn beside(deployments_path: &str) -> Activity {
        let path = (deployments_path != ":memory:").then(|| Path::new(deployments_path).with_extension("activity.jsonl"));
        Activity { path, kept: Mutex::new(Vec::new()) }
    }

    #[cfg(test)]
    pub(crate) fn memory() -> Activity {
        Activity::beside(":memory:")
    }

    pub(crate) fn record(&self, action: &str, target: &str, by: Option<&str>) {
        let entry = ActivityEntry { at: crate::deploy::epoch_now(), action: action.into(), target: target.into(), by: by.map(String::from) };
        let Some(path) = &self.path else {
            self.kept.lock().unwrap().push(entry);
            return;
        };
        let _one_at_a_time = self.kept.lock().unwrap();
        let appended = (|| -> std::io::Result<()> {
            use std::io::Write;
            let line = serde_json::to_string(&entry).map_err(std::io::Error::other)? + "\n";
            std::fs::OpenOptions::new().append(true).create(true).open(path)?.write_all(line.as_bytes())
        })();
        if let Err(e) = appended {
            eprintln!("holon: could not append to the activity log {path:?}: {e}");
        }
    }

    /// The newest `limit` entries, newest first.
    pub(crate) fn newest(&self, limit: usize) -> Vec<ActivityEntry> {
        let all: Vec<ActivityEntry> = match &self.path {
            None => self.kept.lock().unwrap().clone(),
            Some(path) => std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect(),
        };
        all.into_iter().rev().take(limit.min(MAX_READ)).collect()
    }
}

/// `GET /activity?limit=n` (default 50): what was done, newest first.
pub(crate) fn handle_get_activity(activity: &Activity, request: &Request) -> Response {
    let limit = request.get_param("limit").and_then(|l| l.parse().ok()).unwrap_or(50);
    Response::json(&serde_json::json!({ "entries": activity.newest(limit) }))
}
