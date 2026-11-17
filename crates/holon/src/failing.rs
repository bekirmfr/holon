//! What the server is failing to keep now: the deployments file it couldn't write, the run logs
//! it couldn't sync to disk. Each is a warning to an admin (storage.rs's `warnings`: the admin
//! page, `holon host`, the terminal editor), with why, and makes `GET /health` answer 503
//! "not ok" until the next write of it goes through: a load balancer then sends requests to a
//! server that can keep what they do. The witness not taking heads is a warning only (heads.rs),
//! since one witness down would take every server it witnesses out at once.
//!
//! Kept by the deployments file's place, so two servers in one process (the tests') don't see
//! each other's; a table kept nowhere has nothing to fail.

use std::collections::BTreeMap;
use std::path::PathBuf;
use crate::sync::Mutex;

use crate::paths::Place;

/// A writer whose failure the server can't simply go on from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Writer {
    /// The deployments file (persist.rs), with the waiting runs it keeps: a deploy, a grant or a
    /// pause made while it fails is lost when the server starts again.
    Deployments,
    /// The run logs and event logs synced to disk every second, the witness's queue with them
    /// (main.rs's `sync_to_disk`): while it fails a strict run answers 500, and a lazy run's
    /// line may be lost in a power loss.
    Logs,
}

impl Writer {
    /// What `/health` says of it, to anyone who asks: never a path or an error's text, which
    /// only an admin reads (the warnings).
    pub(crate) fn health(self) -> &'static str {
        match self {
            Writer::Deployments => "the deployments file can't be written",
            Writer::Logs => "the run logs can't be synced to disk",
        }
    }
}

/// What `/health` answers while something fails: this, then what fails.
pub(crate) const NOT_OK: &str = "not ok";

static FAILING: Mutex<BTreeMap<(PathBuf, Writer), String>> = Mutex::new(BTreeMap::new());

/// Note how the last write of `writer` went for the server whose deployments file is `at`.
pub(crate) fn note(at: &Place, writer: Writer, result: &Result<(), String>) {
    let Some(path) = at.at() else { return };
    let key = (PathBuf::from(path.clone()), writer);
    let mut failing = FAILING.lock().unwrap();
    match result {
        Ok(()) => drop(failing.remove(&key)),
        Err(e) => drop(failing.insert(key, e.clone())),
    }
}

/// What fails now for the server whose deployments file is `at`, with why.
pub(crate) fn now(at: &Place) -> Vec<(Writer, String)> {
    let Some(path) = at.at() else { return Vec::new() };
    let path = PathBuf::from(path.clone());
    FAILING.lock().unwrap().iter().filter(|((p, _), _)| *p == path).map(|((_, w), e)| (*w, e.clone())).collect()
}

/// `GET /health`'s answer for the server whose deployments file is `at`: 200 "ok", or 503
/// "not ok: " and what fails.
pub(crate) fn health(at: &Place) -> (u16, String) {
    let failing = now(at);
    if failing.is_empty() {
        return (200, "ok".into());
    }
    let what: Vec<&str> = failing.iter().map(|(w, _)| w.health()).collect();
    (503, format!("{NOT_OK}: {}", what.join("; ")))
}

/// A write or read of the server's own files that failed, as two texts kept apart: what whoever
/// asked is told, and why, which only an admin reads (the warnings, stderr). Why holds where on
/// the server's disk and the error's own text, which can name a path or a user; whoever asked
/// may be a flow's caller, or someone signing in (audit R71). What they are told is a fixed
/// text, with at most a line and a column, or a reason holon gives of what it was sent: no place
/// and no error's text can be put in it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Failed {
    told: String,
    why: String,
}

impl Failed {
    /// `told` to whoever asked, `why` to the admin.
    pub(crate) fn new(told: &'static str, why: impl Into<String>) -> Failed {
        Failed { told: told.into(), why: why.into() }
    }

    /// A file that doesn't parse: `told` and where in it, as a line and a column (a line of it,
    /// or a value its message quotes, can hold what the file names: a secret's source, say).
    pub(crate) fn at(told: &'static str, line: usize, column: usize, why: impl Into<String>) -> Failed {
        Failed { told: format!("{told} (line {line}, column {column})"), why: why.into() }
    }

    /// A reason holon writes itself of what it was given (a limit it doesn't know, say), with
    /// nothing of the server's disk or an error's text in it, told as it is; `why` may add where.
    pub(crate) fn reason(reason: String, why: impl Into<String>) -> Failed {
        Failed { told: reason, why: why.into() }
    }

    /// Why, for the admin.
    pub(crate) fn why(&self) -> &str {
        &self.why
    }

    /// What whoever asked is told, where the admin hears why another way (a writer's warning).
    pub(crate) fn told(self) -> String {
        self.told
    }

    /// What whoever asked is told, once why is on stderr for the admin.
    pub(crate) fn said(self) -> String {
        if self.why != self.told {
            eprintln!("warning: {}: {}", self.told, self.why);
        }
        self.told
    }
}

/// Where in `text` byte `at` is: its line and column, from 1.
pub(crate) fn line_and_column(text: &str, at: usize) -> (usize, usize) {
    let before = String::from_utf8_lossy(&text.as_bytes()[..at.min(text.len())]);
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}
