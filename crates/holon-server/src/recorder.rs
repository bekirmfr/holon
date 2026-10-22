//! A deployment's flow log: one entry per run (a request on its port, a `/trigger`, a clock tick)
//! holding everything that run depended on and everything it produced.
//!
//! Evaluation is pure and deterministic, so an entry is enough to run it again exactly: the
//! request record, the state the run read (`state_before`), and the sourced values (clock,
//! random, ingress) are its whole input. The editor's view mode replays entries node by node
//! from just this, to show what every node computed and to step through a past run.
//!
//! Kept in memory, the newest [`CAPACITY`] per deployment; a restart starts a fresh log.

use std::collections::{BTreeMap, VecDeque};

use serde::Serialize;

use crate::{PerformedEntry, SValue};

/// How many entries a deployment keeps; older ones drop off the front.
pub const CAPACITY: usize = 500;

#[derive(Serialize, Clone, Debug)]
pub struct LogEntry {
    /// Increases by one per entry, across sessions; `GET …/log?after=` reads past it.
    pub seq: u64,
    /// The run session it happened in.
    pub session: u32,
    /// When it started, milliseconds since the epoch.
    pub at_ms: u64,
    pub duration_ms: f64,
    /// `"request"` (on the deployment's port), `"trigger"` (`POST /trigger`) or `"tick"`.
    pub kind: &'static str,
    /// The matched route, e.g. `"GET /todos/:id"`; `None` for ticks, triggers, unrouted requests.
    pub route: Option<String>,
    /// The request's method and URL, for a request.
    pub method: Option<String>,
    pub url: Option<String>,
    /// The HTTP status answered, for a request.
    pub status: Option<u16>,
    /// The request record the endpoint received.
    pub request: Option<SValue>,
    /// The sourced values the run saw, by capability (`"Clock"`, `"Random"`, `"NetworkIngress"`).
    pub sourced: Vec<(String, SValue)>,
    /// The deployment's state when the run began: what every `store_read` saw.
    pub state_before: BTreeMap<String, SValue>,
    /// The unit's settled outputs (an endpoint's: response first, then effects).
    pub outputs: Vec<Option<SValue>>,
    /// The effects the host performed.
    pub performed: Vec<PerformedEntry>,
    /// The response body sent, for a request (as text, cut at 64 KiB).
    pub response_body: Option<String>,
    /// Why the run failed before it could settle, if it did.
    pub error: Option<String>,
    /// When each step ran, relative to the run's start (see [`Timer`]).
    #[serde(default)]
    pub timeline: Vec<NodeTiming>,
    /// The outbound requests the run made, round by round (`http_request`, §4.1): replay answers
    /// from these instead of the network. Absent when it made none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rounds: Vec<LoggedRound>,
}

/// The requests performed after one round, to answer the next.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LoggedRound {
    pub exchanges: Vec<LoggedExchange>,
}

/// One outbound request: the node that made it (its path in the run's root unit), the request,
/// the response, and when, in microseconds since the run began.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LoggedExchange {
    pub path: Vec<u32>,
    pub request: SValue,
    pub response: SValue,
    pub start_us: f64,
    pub end_us: f64,
}

impl LoggedRound {
    pub fn from_host(round: &holon_host::Round) -> LoggedRound {
        LoggedRound {
            exchanges: round
                .exchanges
                .iter()
                .map(|x| LoggedExchange {
                    path: x.path.clone(),
                    request: SValue::from_core(&x.request),
                    response: SValue::from_core(&x.response),
                    start_us: x.start_us,
                    end_us: x.end_us,
                })
                .collect(),
        }
    }
}

/// One composite child's evaluation: its path of child indices from the run's root unit, and
/// when it started and finished, in microseconds since the run began.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct NodeTiming {
    pub path: Vec<u32>,
    pub start_us: f64,
    pub end_us: f64,
    /// Which round of the run (0 for a run without outbound requests).
    pub round: u32,
}

/// Times each composite child of a run as core evaluates it (core's `Observer`), down to
/// [`Timer::DEPTH`] levels: for a request, the endpoint's handler and the handler's nodes (the
/// canvas nodes); for a whole-flow unit, its nodes. Deeper levels (a template's insides) are left
/// out to keep entries small.
pub struct Timer {
    start: std::time::Instant,
    open: Vec<f64>,
    round: u32,
    pub timeline: Vec<NodeTiming>,
}

impl Timer {
    pub const DEPTH: usize = 2;

    pub fn new() -> Timer {
        Timer { start: std::time::Instant::now(), open: Vec::new(), round: 0, timeline: Vec::new() }
    }

    fn now_us(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1e6
    }
}

impl holon_core::Observer for Timer {
    fn enter(&mut self, path: &[u32]) {
        if path.len() <= Self::DEPTH {
            let t = self.now_us();
            self.open.push(t);
        }
    }
    fn leave(&mut self, path: &[u32]) {
        if path.len() <= Self::DEPTH {
            let start_us = self.open.pop().unwrap_or(0.0);
            let end_us = self.now_us();
            self.timeline.push(NodeTiming { path: path.to_vec(), start_us, end_us, round: self.round });
        }
    }
    fn round(&mut self, n: usize) {
        self.round = n as u32;
    }
}

#[derive(Default, Debug)]
pub struct Log {
    next_seq: u64,
    entries: VecDeque<LogEntry>,
}

impl Log {
    /// Append `entry`, numbering it, and drop the oldest past [`CAPACITY`].
    pub fn push(&mut self, mut entry: LogEntry) {
        entry.seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push_back(entry);
        while self.entries.len() > CAPACITY {
            self.entries.pop_front();
        }
    }

    /// The entries after `after` (all of them for `None`), oldest first, at most `limit`
    /// (the newest `limit`, when there are more).
    pub fn after(&self, after: Option<u64>, limit: usize) -> Vec<LogEntry> {
        let newer: Vec<&LogEntry> = self.entries.iter().filter(|e| after.is_none_or(|a| e.seq > a)).collect();
        newer[newer.len().saturating_sub(limit)..].iter().map(|e| (*e).clone()).collect()
    }

    /// The seq the next entry will get.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }
}

/// A response body as log text: lossy UTF-8, cut at 64 KiB.
pub fn body_text(body: &[u8]) -> String {
    const MAX: usize = 64 * 1024;
    let mut s = String::from_utf8_lossy(&body[..body.len().min(MAX)]).into_owned();
    if body.len() > MAX {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> LogEntry {
        LogEntry {
            seq: 0,
            session: 1,
            at_ms: 0,
            duration_ms: 0.0,
            kind: "tick",
            route: None,
            method: None,
            url: None,
            status: None,
            request: None,
            sourced: vec![],
            state_before: BTreeMap::new(),
            outputs: vec![],
            performed: vec![],
            response_body: None,
            error: None,
            timeline: vec![],
            rounds: vec![],
        }
    }

    #[test]
    fn numbers_entries_reads_past_a_seq_and_keeps_the_newest() {
        let mut log = Log::default();
        for _ in 0..(CAPACITY + 5) {
            log.push(entry());
        }
        assert_eq!(log.next_seq(), (CAPACITY + 5) as u64);
        let all = log.after(None, usize::MAX);
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].seq, 5, "the five oldest dropped off");
        let newer = log.after(Some((CAPACITY + 2) as u64), 10);
        assert_eq!(newer.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![(CAPACITY + 3) as u64, (CAPACITY + 4) as u64]);
        assert_eq!(log.after(None, 3).iter().map(|e| e.seq).collect::<Vec<_>>(), vec![502, 503, 504], "the newest 3");
    }

    #[test]
    fn the_timer_records_each_step_up_to_its_depth_in_order_of_finishing() {
        use holon_core::Observer;
        let mut t = Timer::new();
        for path in [&[0u32][..], &[0, 0], &[0, 0, 0]] {
            t.enter(path);
        }
        for path in [&[0u32, 0, 0][..], &[0, 0], &[0]] {
            t.leave(path);
        }
        let paths: Vec<Vec<u32>> = t.timeline.iter().map(|n| n.path.clone()).collect();
        assert_eq!(paths, vec![vec![0, 0], vec![0]], "depth 3 left out; inner steps finish first");
        assert!(t.timeline.iter().all(|n| n.end_us >= n.start_us));
        assert!(t.timeline[1].start_us <= t.timeline[0].start_us, "the outer step started first");
    }

    #[test]
    fn body_text_is_lossy_and_bounded() {
        assert_eq!(body_text(b"ok"), "ok");
        assert_eq!(body_text(&[0xff]), "\u{fffd}");
        assert!(body_text(&vec![b'a'; 70_000]).ends_with('…'));
    }
}
