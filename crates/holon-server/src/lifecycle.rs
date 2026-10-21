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

use std::collections::BTreeMap;

use holon_core::Value;
use serde::{Deserialize, Serialize};

use crate::SValue;

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
    /// The state when the session ended. `None` while it runs (the live state is the
    /// deployment's own) and for a deployment without state.
    #[serde(default)]
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
}

impl LifecycleError {
    pub fn message(&self) -> String {
        match self {
            LifecycleError::NotRunning => "it isn't running".into(),
            LifecycleError::AlreadyRunning => "it is already running".into(),
            LifecycleError::AlreadyStopped => "it is already stopped".into(),
            LifecycleError::NoSuchSnapshot(n) => format!("session {n} has no saved state"),
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

    /// End the current session, keeping `live` (the deployment's state, if it has one) as its
    /// snapshot.
    pub fn stop(&mut self, live: Option<&BTreeMap<String, Value>>, now: u64) -> Result<(), LifecycleError> {
        if self.status == Status::Stopped {
            return Err(LifecycleError::AlreadyStopped);
        }
        self.status = Status::Stopped;
        if let Some(s) = self.current() {
            s.ended_at = Some(now);
            s.state = live.map(snapshot);
        }
        Ok(())
    }

    /// Resume a paused session, or start a new one after a stop. For a new session, returns the
    /// state it should start with (`None`: keep the live state as it is); `from` is ignored on
    /// resume, which always continues where the session was.
    pub fn run(&mut self, from: RunFrom, now: u64) -> Result<Option<BTreeMap<String, Value>>, LifecycleError> {
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
                        let snap = self
                            .sessions
                            .iter()
                            .find(|s| s.number == n)
                            .and_then(|s| s.state.as_ref())
                            .ok_or(LifecycleError::NoSuchSnapshot(n))?;
                        (Some(restore(snap)), format!("session {n}"))
                    }
                };
                let number = self.sessions.last().map_or(1, |s| s.number + 1);
                self.sessions.push(session(number, now, &label));
                self.status = Status::Running;
                Ok(state)
            }
        }
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
    Session { number, started_at: now, ended_at: None, requests: 0, errors: 0, from: from.into(), state: None }
}

fn snapshot(live: &BTreeMap<String, Value>) -> BTreeMap<String, SValue> {
    live.iter().map(|(k, v)| (k.clone(), SValue::from_core(v))).collect()
}

fn restore(snap: &BTreeMap<String, SValue>) -> BTreeMap<String, Value> {
    snap.iter().map(|(k, v)| (k.clone(), v.clone().into_core())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todos(n: usize) -> BTreeMap<String, Value> {
        BTreeMap::from([("todos".to_string(), Value::List(vec![Value::Number(1.0); n]))])
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
        assert_eq!(s.state.as_ref().unwrap()["todos"], SValue::from_core(&todos(2)["todos"]));
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
    fn run_from_parses_names_and_numbers() {
        assert_eq!(RunFrom::parse("latest"), Some(RunFrom::Latest));
        assert_eq!(RunFrom::parse("fresh"), Some(RunFrom::Fresh));
        assert_eq!(RunFrom::parse("3"), Some(RunFrom::Session(3)));
        assert_eq!(RunFrom::parse("x"), None);
    }
}
