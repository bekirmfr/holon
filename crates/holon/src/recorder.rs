//! A deployment's flow log: one entry per run (a request on its port, a `/trigger`, a clock tick)
//! holding everything that run depended on and everything it produced.
//!
//! Evaluation is pure and deterministic, so an entry is enough to run it again exactly: the
//! request record, the state the run read (`state_before`), and the sourced values (clock,
//! random, ingress) are its whole input. The editor's view mode replays entries node by node
//! from just this, to show what every node computed and to step through a past run.
//!
//! The newest [`CAPACITY`] per deployment are kept in memory, and in a file of the deployment's
//! own beside the deployments file (`<deployments>.logs/<slug>.jsonl`, a JSON line per entry,
//! owner-only), so a restart reads them back and replay still has them. An entry is written as
//! the control plane serves it: secrets and tokens already redacted. The file is rewritten to the
//! newest [`CAPACITY`] once it holds twice that; a line half-written by a crash is skipped.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{PerformedEntry, SValue};

/// How many entries a deployment keeps; older ones drop off the front.
pub const CAPACITY: usize = 500;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
// A field left out when empty reads back empty.
#[serde(default)]
pub struct LogEntry {
    /// Increases by one per entry, across sessions; `GET …/log?after=` reads past it.
    pub seq: u64,
    /// The run session it happened in.
    pub session: u32,
    /// When it started, milliseconds since the epoch.
    pub at_ms: u64,
    pub duration_ms: f64,
    /// `"request"` (on the deployment's port), `"trigger"` (`POST /trigger`) or `"tick"`.
    pub kind: String,
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
    /// The run's id (hex; §4.4), recomputable from this entry with `nonce`. Empty for a request
    /// that matched no route (no run).
    pub run_id: String,
    /// The deployment version it ran on (the deployment's unit then, hex; for a resumed part, the
    /// version its run started on). View mode shows that version's canvas
    /// (`GET …/document?unit=`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    pub nonce: String,
    /// The grant (hash, hex) the run was held to.
    pub grant: String,
    /// The waiting points it settled waiting at (a parked run's part), if any.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parked: Vec<String>,
    /// For a resumed part: the events delivered to its waiting points so far.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub events: BTreeMap<String, SValue>,
    /// The collection reads it made, by node path, and what each answered: replay answers them
    /// from here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<LoggedRead>,
    /// How many times it ran again because what it read changed before it could commit.
    #[serde(skip_serializing_if = "is_zero")]
    pub retries: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// One collection read in a run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoggedRead {
    pub path: Vec<u32>,
    pub request: SValue,
    pub answer: Option<SValue>,
}

/// The requests performed after one round, to answer the next.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoggedRound {
    pub exchanges: Vec<LoggedExchange>,
}

/// One outbound request: the node that made it (its path in the run's root unit), the request,
/// the response, and when, in microseconds since the run began.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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

/// One composite child's evaluation: its path of child indices from the run's root unit, when it
/// started and finished (microseconds since the run began), and what it produced.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NodeTiming {
    pub path: Vec<u32>,
    pub start_us: f64,
    pub end_us: f64,
    /// Which round of the run (0 for a run without outbound requests).
    pub round: u32,
    /// Its outputs, as the run produced them: what a replay shows for a node it can't recompute
    /// (one fed a secret, which is never logged). Redacted like the rest of the entry; left out
    /// when any of them is larger than [`Timer::MAX_RECORDED_BYTES`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<Option<SValue>>>,
}

/// Times each composite child of a run as core evaluates it (core's `Observer`), down to
/// [`Timer::DEPTH`] levels: for a request, the endpoint's handler, the handler's nodes (the canvas
/// nodes), and one level inside each: a holon's own nodes (a placed composite), shown in view
/// mode's component tab, and a template's. Deeper levels are left out to keep entries small.
pub struct Timer {
    start: std::time::Instant,
    open: Vec<f64>,
    round: u32,
    /// The outputs of the node about to be left (core reports them just before `leave`).
    produced: Option<Vec<Option<SValue>>>,
    pub timeline: Vec<NodeTiming>,
}

impl Timer {
    pub const DEPTH: usize = 3;
    /// The largest output kept per node, as JSON: enough for records, messages and signatures;
    /// a large body is in the request or response already.
    pub const MAX_RECORDED_BYTES: usize = 16 * 1024;

    pub fn new() -> Timer {
        Timer { start: std::time::Instant::now(), open: Vec::new(), round: 0, produced: None, timeline: Vec::new() }
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
    fn produced(&mut self, path: &[u32], outputs: &[Option<holon_core::Value>]) {
        if path.len() <= Self::DEPTH {
            let outputs: Vec<Option<SValue>> = outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect();
            let size = serde_json::to_vec(&outputs).map_or(usize::MAX, |b| b.len());
            self.produced = (size <= Self::MAX_RECORDED_BYTES).then_some(outputs);
        }
    }
    fn leave(&mut self, path: &[u32]) {
        if path.len() <= Self::DEPTH {
            let start_us = self.open.pop().unwrap_or(0.0);
            let end_us = self.now_us();
            let outputs = self.produced.take();
            self.timeline.push(NodeTiming { path: path.to_vec(), start_us, end_us, round: self.round, outputs });
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
    /// Where it is kept beside memory (none: memory only, a `:memory:` deployments file).
    file: Option<PathBuf>,
    /// How many lines the file holds.
    written: usize,
}

/// Where `slug`'s log is kept for the deployments file at `deployments_path`, if anywhere.
pub fn file_for(deployments_path: &str, slug: &str) -> Option<PathBuf> {
    (deployments_path != ":memory:").then(|| Path::new(deployments_path).with_extension("logs").join(format!("{slug}.jsonl")))
}

impl Log {
    /// The log kept at `file` (none: in memory only), with the newest entries it holds read back.
    pub fn open(file: Option<PathBuf>) -> Log {
        let mut log = Log { file, ..Log::default() };
        let Some(path) = &log.file else { return log };
        let Ok(text) = std::fs::read_to_string(path) else { return log };
        for line in text.lines() {
            log.written += 1;
            // A line a crash cut short is skipped, not fatal: the others still replay.
            if let Ok(entry) = serde_json::from_str::<LogEntry>(line) {
                log.next_seq = log.next_seq.max(entry.seq + 1);
                log.entries.push_back(entry);
            }
        }
        while log.entries.len() > CAPACITY {
            log.entries.pop_front();
        }
        log
    }

    /// Append `entry`, numbering it, and drop the oldest past [`CAPACITY`]; keep it in the file.
    pub fn push(&mut self, mut entry: LogEntry) {
        entry.seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push_back(entry);
        while self.entries.len() > CAPACITY {
            self.entries.pop_front();
        }
        if let Err(e) = self.keep() {
            eprintln!("holon: could not keep the flow log in {:?}: {e}", self.file);
        }
    }

    /// Write the newest entry to the file: appended, or the whole file rewritten to the newest
    /// [`CAPACITY`] once it would hold twice that.
    fn keep(&mut self) -> std::io::Result<()> {
        let Some(path) = self.file.clone() else { return Ok(()) };
        let Some(newest) = self.entries.back() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if self.written + 1 > 2 * CAPACITY {
            let mut all = String::new();
            for e in &self.entries {
                all.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
                all.push('\n');
            }
            crate::sealing::write_private(&path, all.as_bytes())?;
            self.written = self.entries.len();
            return Ok(());
        }
        use std::io::Write;
        let line = serde_json::to_string(newest).map_err(std::io::Error::other)? + "\n";
        let new = !path.exists();
        let mut options = std::fs::OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        if new {
            crate::sealing::restrict_to_owner(&path)?;
        }
        file.write_all(line.as_bytes())?;
        self.written += 1;
        Ok(())
    }

    /// Delete the file: its deployment was deleted.
    pub fn delete_file(&self) {
        if let Some(path) = &self.file {
            let _ = std::fs::remove_file(path);
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

    /// Whether some entry's state before held `text` in cell `key`: a collection root this
    /// deployment's log has seen.
    pub fn shows(&self, key: &str, text: &str) -> bool {
        let want = SValue::from_core(&holon_core::Value::Text(text.to_string()));
        self.entries.iter().any(|e| e.state_before.get(key) == Some(&want))
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
            unit: String::new(),
            seq: 0,
            session: 1,
            at_ms: 0,
            duration_ms: 0.0,
            kind: "tick".into(),
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
            run_id: String::new(),
            nonce: String::new(),
            grant: String::new(),
            parked: vec![],
            events: BTreeMap::new(),
            reads: vec![],
            retries: 0,
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
        for path in [&[0u32][..], &[0, 0], &[0, 0, 1], &[0, 0, 1, 0]] {
            t.enter(path);
        }
        for path in [&[0u32, 0, 1, 0][..], &[0, 0, 1], &[0, 0], &[0]] {
            t.leave(path);
        }
        let paths: Vec<Vec<u32>> = t.timeline.iter().map(|n| n.path.clone()).collect();
        assert_eq!(paths, vec![vec![0, 0, 1], vec![0, 0], vec![0]], "depth 4 left out; inner steps finish first");
        assert!(t.timeline.iter().all(|n| n.end_us >= n.start_us));
        assert!(t.timeline[2].start_us <= t.timeline[0].start_us, "the outer step started first");
    }

    #[test]
    fn a_kept_log_reads_back_after_a_restart_skips_a_cut_line_and_stays_bounded() {
        let dir = std::env::temp_dir().join(format!("holon-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("slug.jsonl");
        let mut log = Log::open(Some(path.clone()));
        for _ in 0..3 {
            log.push(entry());
        }
        // A restart: the entries come back, and numbering carries on.
        let mut again = Log::open(Some(path.clone()));
        assert_eq!(again.after(None, 10).iter().map(|e| e.seq).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(again.next_seq(), 3);
        again.push(entry());
        assert_eq!(Log::open(Some(path.clone())).next_seq(), 4);

        // A line a crash cut short is skipped; the rest still read.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"seq\": 9, \"sess");
        std::fs::write(&path, text).unwrap();
        assert_eq!(Log::open(Some(path.clone())).after(None, 10).len(), 4);

        // Bounded: past twice CAPACITY lines, the file is rewritten to the newest CAPACITY.
        let mut big = Log::open(Some(path.clone()));
        for _ in 0..(2 * CAPACITY) {
            big.push(entry());
        }
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert!((CAPACITY..=2 * CAPACITY).contains(&lines), "{lines}");
        let back = Log::open(Some(path.clone()));
        assert_eq!(back.after(None, usize::MAX).len(), CAPACITY);
        assert_eq!(back.next_seq(), big.next_seq());
        big.delete_file();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn body_text_is_lossy_and_bounded() {
        assert_eq!(body_text(b"ok"), "ok");
        assert_eq!(body_text(&[0xff]), "\u{fffd}");
        assert!(body_text(&vec![b'a'; 70_000]).ends_with('…'));
    }
}
