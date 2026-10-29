//! A deployment's lifecycle: running, paused or stopped, and the run sessions it has been through.
//!
//! - **Running**: answers requests and fires on its schedule. The current session counts them.
//! - **Paused**: keeps its port and its session but answers nothing (503) and skips ticks.
//!   Resuming continues the same session.
//! - **Stopped**: the session ends, the state at that moment is kept as the session's snapshot,
//!   and the port is released. Running again starts a new session, from the latest state, from
//!   nothing, or from any earlier session's snapshot.
//!
//! A deployment can only be deleted once stopped. Until then every session, with its snapshot,
//! is kept.
//!
//! A snapshot is kept by reference: each key's value hash (the host's `State::refs`), with the
//! values in the content store, so sessions share whatever values they have in common. This
//! module only records references; the server stores and loads the values.

use std::collections::BTreeMap;

use holon_core::{parse_hash, Hash};
use serde::{Deserialize, Serialize};

use crate::{to_hex, SValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Paused,
    Stopped,
}

/// One run of a deployment, from run (or deploy) to stop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub number: u32,
    pub started_at: u64,
    pub ended_at: Option<u64>,
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub errors: u64,
    /// What it started from: `"deploy"`, `"latest"`, `"fresh"` or `"session N"`.
    #[serde(default)]
    pub from: String,
    /// The state when the session ended, by reference: each key's value hash (hex), the values
    /// in the content store. `None` while it runs (the live state is the deployment's own) and
    /// for a deployment without state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_refs: Option<BTreeMap<String, String>>,
    /// A snapshot as files before `state_refs` held it, values inline. Read once at load, when
    /// its values move into the store (`persist.rs`); never written.
    #[serde(default, skip_serializing)]
    pub state: Option<BTreeMap<String, SValue>>,
}

/// Where a new session's state comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunFrom {
    /// The state as the last session left it.
    Latest,
    /// Empty.
    Fresh,
    /// A given session's snapshot.
    Session(u32),
}

impl RunFrom {
    /// `"latest"`, `"fresh"`, or a session number.
    pub fn parse(s: &str) -> Option<RunFrom> {
        match s {
            "latest" => Some(RunFrom::Latest),
            "fresh" => Some(RunFrom::Fresh),
            n => n.parse().ok().map(RunFrom::Session),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum LifecycleError {
    NotRunning,
    AlreadyRunning,
    AlreadyStopped,
    NoSuchSnapshot(u32),
    /// A state restore needs a session to replace the state of.
    Stopped,
}

impl LifecycleError {
    pub fn message(&self) -> String {
        match self {
            LifecycleError::NotRunning => "it isn't running".into(),
            LifecycleError::AlreadyRunning => "it is already running".into(),
            LifecycleError::AlreadyStopped => "it is already stopped".into(),
            LifecycleError::NoSuchSnapshot(n) => format!("session {n} has no saved state"),
            LifecycleError::Stopped => {
                "it is stopped: run it (from any session's state), or run it, then restore a state".into()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lifecycle {
    pub status: Status,
    pub sessions: Vec<Session>,
}

impl Lifecycle {
    /// A fresh deployment: running, in its first session.
    pub fn deployed(now: u64) -> Lifecycle {
        Lifecycle { status: Status::Running, sessions: vec![session(1, now, "deploy")] }
    }

    /// The session in progress (running or paused), if any.
    pub fn current(&mut self) -> Option<&mut Session> {
        self.sessions.last_mut().filter(|s| s.ended_at.is_none())
    }

    pub fn pause(&mut self) -> Result<(), LifecycleError> {
        if self.status != Status::Running {
            return Err(LifecycleError::NotRunning);
        }
        self.status = Status::Paused;
        Ok(())
    }

    /// End the current session, keeping `live` (the deployment's state's references, if it has
    /// state; its values must already be in the store) as its snapshot.
    pub fn stop(&mut self, live: Option<&BTreeMap<String, Hash>>, now: u64) -> Result<(), LifecycleError> {
        if self.status == Status::Stopped {
            return Err(LifecycleError::AlreadyStopped);
        }
        self.status = Status::Stopped;
        if let Some(s) = self.current() {
            s.ended_at = Some(now);
            s.state_refs = live.map(|refs| refs.iter().map(|(k, h)| (k.clone(), to_hex(h))).collect());
        }
        Ok(())
    }

    /// Resume a paused session, or start a new one after a stop. For a new session, returns the
    /// references of the state it should start with (`None`: keep the live state as it is;
    /// empty: start empty); `from` is ignored on resume, which always continues where the
    /// session was.
    pub fn run(&mut self, from: RunFrom, now: u64) -> Result<Option<BTreeMap<String, Hash>>, LifecycleError> {
        match self.status {
            Status::Running => Err(LifecycleError::AlreadyRunning),
            Status::Paused => {
                self.status = Status::Running;
                Ok(None)
            }
            Status::Stopped => {
                let (state, label) = match from {
                    RunFrom::Latest => (None, "latest".to_string()),
                    RunFrom::Fresh => (Some(BTreeMap::new()), "fresh".to_string()),
                    RunFrom::Session(n) => {
                        let snap = self.snapshot(n).ok_or(LifecycleError::NoSuchSnapshot(n))?;
                        (Some(snap), format!("session {n}"))
                    }
                };
                let number = self.sessions.last().map_or(1, |s| s.number + 1);
                self.sessions.push(session(number, now, &label));
                self.status = Status::Running;
                Ok(state)
            }
        }
    }

    /// A state put back while it runs or is paused (`holon restore --state`): the current session
    /// ends, keeping `live` as its snapshot, as a stop would, and a new one starts from `from`
    /// (`"version 3"`, `"run 41"`), in the same status. The state it replaced stays restorable as
    /// that session's.
    pub fn restart(&mut self, live: Option<&BTreeMap<String, Hash>>, from: &str, now: u64) -> Result<(), LifecycleError> {
        let status = self.status;
        if status == Status::Stopped {
            return Err(LifecycleError::Stopped);
        }
        self.stop(live, now)?;
        let number = self.sessions.last().map_or(1, |s| s.number + 1);
        self.sessions.push(session(number, now, from));
        self.status = status;
        Ok(())
    }

    /// Session `n`'s snapshot references, if it has a readable one.
    pub fn snapshot(&self, n: u32) -> Option<BTreeMap<String, Hash>> {
        let refs = self.sessions.iter().find(|s| s.number == n)?.state_refs.as_ref()?;
        refs.iter().map(|(k, h)| Some((k.clone(), parse_hash(h)?))).collect()
    }

    /// Count one request (or tick) against the current session.
    pub fn record(&mut self, is_error: bool) {
        if let Some(s) = self.current() {
            s.requests += 1;
            if is_error {
                s.errors += 1;
            }
        }
    }
}

fn session(number: u32, now: u64, from: &str) -> Session {
    Session {
        number,
        started_at: now,
        ended_at: None,
        requests: 0,
        errors: 0,
        from: from.into(),
        state_refs: None,
        state: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in snapshot: the references of a state holding `n` todos.
    fn todos(n: usize) -> BTreeMap<String, Hash> {
        BTreeMap::from([("todos".to_string(), [n as u8; 32])])
    }

    #[test]
    fn a_deployment_runs_pauses_resumes_and_stops_with_one_session() {
        let mut l = Lifecycle::deployed(100);
        l.record(false);
        l.pause().unwrap();
        assert_eq!(l.pause(), Err(LifecycleError::NotRunning));
        assert_eq!(l.run(RunFrom::Fresh, 110), Ok(None), "resume ignores `from` and keeps the session");
        l.record(true);
        l.stop(Some(&todos(2)), 120).unwrap();
        assert_eq!(l.stop(None, 121), Err(LifecycleError::AlreadyStopped));
        assert_eq!(l.status, Status::Stopped);
        let s = &l.sessions[0];
        assert_eq!((s.number, s.started_at, s.ended_at, s.requests, s.errors), (1, 100, Some(120), 2, 1));
        assert_eq!(s.state_refs.as_ref().unwrap()["todos"], to_hex(&[2u8; 32]), "the snapshot, by reference");
        assert_eq!(l.snapshot(1), Some(todos(2)));
    }

    #[test]
    fn each_run_after_a_stop_is_a_new_session_from_the_chosen_state() {
        let mut l = Lifecycle::deployed(0);
        l.stop(Some(&todos(1)), 10).unwrap();
        assert_eq!(l.run(RunFrom::Latest, 20), Ok(None), "latest keeps the live state");
        assert_eq!(l.run(RunFrom::Latest, 21), Err(LifecycleError::AlreadyRunning));
        l.stop(Some(&todos(3)), 30).unwrap();
        assert_eq!(l.run(RunFrom::Fresh, 40), Ok(Some(BTreeMap::new())));
        l.stop(Some(&BTreeMap::new()), 50).unwrap();
        assert_eq!(l.run(RunFrom::Session(1), 60), Ok(Some(todos(1))), "restore session 1's snapshot");
        l.stop(None, 70).unwrap();
        assert_eq!(l.run(RunFrom::Session(9), 80), Err(LifecycleError::NoSuchSnapshot(9)));
        assert_eq!(l.run(RunFrom::Session(4), 80), Err(LifecycleError::NoSuchSnapshot(4)), "a stateless snapshot");
        let summary: Vec<(u32, &str, Option<u64>)> =
            l.sessions.iter().map(|s| (s.number, s.from.as_str(), s.ended_at)).collect();
        assert_eq!(
            summary,
            vec![(1, "deploy", Some(10)), (2, "latest", Some(30)), (3, "fresh", Some(50)), (4, "session 1", Some(70))]
        );
    }

    #[test]
    fn a_restored_state_starts_a_session_and_keeps_the_one_it_replaced() {
        let mut l = Lifecycle::deployed(0);
        l.pause().unwrap();
        l.restart(Some(&todos(5)), "run 41", 10).unwrap();
        assert_eq!(l.status, Status::Paused, "a paused deployment stays paused");
        assert_eq!(l.snapshot(1), Some(todos(5)), "the state it replaced is session 1's");
        let s = &l.sessions[1];
        assert_eq!((s.number, s.from.as_str(), s.started_at, s.ended_at), (2, "run 41", 10, None));
        l.run(RunFrom::Latest, 11).unwrap();
        l.restart(None, "version 1", 20).unwrap();
        assert_eq!(l.status, Status::Running);
        assert_eq!(l.sessions[2].from, "version 1");
        l.stop(None, 30).unwrap();
        assert_eq!(l.restart(None, "run 1", 40), Err(LifecycleError::Stopped));
        assert_eq!(l.sessions.len(), 3, "a refused restart changes nothing");
    }

    #[test]
    fn run_from_parses_names_and_numbers() {
        assert_eq!(RunFrom::parse("latest"), Some(RunFrom::Latest));
        assert_eq!(RunFrom::parse("fresh"), Some(RunFrom::Fresh));
        assert_eq!(RunFrom::parse("3"), Some(RunFrom::Session(3)));
        assert_eq!(RunFrom::parse("x"), None);
    }
}
