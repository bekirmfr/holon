//! A deployment's flow log: one entry per run (a request on its port, a `/trigger`, a clock tick)
//! holding everything that run depended on and everything it produced.
//!
//! Evaluation is pure and deterministic, so an entry is enough to run it again exactly: the
//! request record, the state the run read (`state_before`), and the sourced values (clock,
//! random, ingress) are its whole input. The editor's view mode replays entries node by node
//! from just this, to show what every node computed and to step through a past run.
//!
//! The newest entries per deployment (the host's `--keep-runs`, [`DEFAULT_KEEP`] without it) are
//! kept in memory, and in a file of the deployment's own beside the deployments file
//! (`<deployments>.logs/<slug>.jsonl`, a JSON line per entry, owner-only), so a restart reads
//! them back and replay still has them. An entry is written as the control plane serves it:
//! secrets and tokens already redacted. The file is cut back to the newest it keeps once it holds
//! twice that; a line half-written by a crash is skipped.
//!
//! The file is a hash chain: each entry's `prev` is the BLAKE3 hash of the line before it, exactly
//! as written. Cutting the file back keeps the newest lines byte for byte, so the chain still
//! checks from the oldest line kept (whose `prev` names one no longer there). Opening the file
//! checks it: an entry whose `prev` isn't the line before it (one changed, removed or inserted
//! outside holon) is a break, reported by its seq. Entries from before the chain (no `prev`)
//! check only once one with a `prev` has been seen.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{PerformedEntry, SValue};

/// How many entries a deployment keeps when the host doesn't say; older ones drop off the front.
pub const DEFAULT_KEEP: usize = 500;

static KEEP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(DEFAULT_KEEP);

/// How many entries each deployment's log keeps from now on (the server sets it once, at start;
/// at least 1).
pub fn set_keep(n: usize) {
    KEEP.store(n.max(1), std::sync::atomic::Ordering::Relaxed);
}

/// How many entries each deployment's log keeps.
pub fn keep() -> usize {
    KEEP.load(std::sync::atomic::Ordering::Relaxed)
}

/// A line's hash in the chain, hex.
pub fn line_hash(line: &str) -> String {
    blake3::hash(line.as_bytes()).to_hex().to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
// A field left out when empty reads back empty.
#[serde(default)]
pub struct LogEntry {
    /// Increases by one per entry, across sessions; `GET …/log?after=` reads past it.
    pub seq: u64,
    /// The hash of the line before it in the log file (hex; see the module docs). Empty for the
    /// first entry, and for entries written before the chain.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prev: String,
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
    /// The keys it claimed before calling out (`docs/L4s/key-claims-L4.md`): how long each waited
    /// for another run, and whether holding it showed what it read had changed (then it ran again).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claims: Vec<LoggedClaim>,
    /// Who sent the request over the control plane (`holon mcp`'s `call`, `token:claude`, or
    /// `local:ada (agent)`; the editor's request tester), when it came that way: a request on the
    /// deployment's own port names nobody.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// One key a run claimed before calling out.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoggedClaim {
    /// `charges[k2]`, or `charges[*]` for a listing.
    pub key: String,
    pub waited_ms: f64,
    pub overtaken: bool,
    /// How long it was held, from taken to let go; none for a key the run never held (busy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_ms: Option<f64>,
}

/// One collection read in a run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoggedRead {
    pub path: Vec<u32>,
    pub request: SValue,
    /// What it answered; for a file read kept by hash (`file`), none: its text is elsewhere.
    pub answer: Option<SValue>,
    /// For a file read (folders.rs): the file's hash and size, and its text's place in the content
    /// store when it isn't in `answer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<LoggedFile>,
}

/// A file a run read, as its log entry keeps it: what `holon verify` checks a supplied file by.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoggedFile {
    /// BLAKE3 of its bytes (hex).
    pub hash: String,
    pub size: u64,
    /// Its text's value hash in the content store (hex), when the folder keeps bytes and the text
    /// isn't in the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
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
    /// Why a module at this node gave nothing (the engine's words: out of fuel, a trap, a js
    /// body's exception), when it did: what `explain` says beside the statement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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
    /// Why the node being left gave nothing, when a module said so.
    reason: Option<String>,
}

impl Timer {
    pub const DEPTH: usize = 3;
    /// The largest output kept per node, as JSON: enough for records, messages and signatures;
    /// a large body is in the request or response already.
    pub const MAX_RECORDED_BYTES: usize = 16 * 1024;

    pub fn new() -> Timer {
        Timer { start: std::time::Instant::now(), open: Vec::new(), round: 0, produced: None, timeline: Vec::new(), reason: None }
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
    fn produced(&mut self, path: &[u32], outputs: &[holon_core::Slot]) {
        if path.len() <= Self::DEPTH {
            // A pending output (a round still waiting on an answer) is logged as nothing yet.
            let outputs: Vec<Option<SValue>> = outputs.iter().map(|o| o.value().map(SValue::from_core)).collect();
            let size = serde_json::to_vec(&outputs).map_or(usize::MAX, |b| b.len());
            self.produced = (size <= Self::MAX_RECORDED_BYTES).then_some(outputs);
        }
    }
    fn leave(&mut self, path: &[u32]) {
        if path.len() <= Self::DEPTH {
            let start_us = self.open.pop().unwrap_or(0.0);
            let end_us = self.now_us();
            let outputs = self.produced.take();
            let reason = self.reason.take();
            self.timeline.push(NodeTiming { path: path.to_vec(), start_us, end_us, round: self.round, outputs, reason });
        }
    }
    fn round(&mut self, n: usize) {
        self.round = n as u32;
    }
    fn failed(&mut self, path: &[u32], reason: &str) {
        if path.len() <= Self::DEPTH {
            self.reason = Some(reason.to_string());
        }
    }
}

#[derive(Debug)]
pub struct Log {
    next_seq: u64,
    entries: VecDeque<LogEntry>,
    /// How many entries it keeps.
    keep: usize,
    /// Where it is kept beside memory (none: memory only, a `:memory:` deployments file).
    file: Option<PathBuf>,
    /// How many lines the file holds.
    written: usize,
    /// Whether the file ends in a line a crash cut short, which the next line mustn't join.
    cut: bool,
    /// The hash of the newest line: the next entry's `prev`.
    head: String,
    /// The seqs of the entries that don't follow the line before them (see the module docs).
    breaks: Vec<u64>,
    /// The deployment it is the log of, whose heads go to the witness, if one is set (heads.rs).
    slug: Option<String>,
}

impl Default for Log {
    fn default() -> Log {
        Log::open(None)
    }
}

/// Where `slug`'s log is kept for the deployments file at `deployments_path`, if anywhere.
pub fn file_for(deployments_path: &str, slug: &str) -> Option<PathBuf> {
    (deployments_path != ":memory:").then(|| Path::new(deployments_path).with_extension("logs").join(format!("{slug}.jsonl")))
}

impl Log {
    /// The log kept at `file` (none: in memory only), keeping as many entries as the host set,
    /// with the newest it holds read back and its chain checked.
    pub fn open(file: Option<PathBuf>) -> Log {
        Log::open_keeping(file, keep())
    }

    /// The log of deployment `slug`, kept beside the deployments file at `deployments_path`: its
    /// heads go to the witness, if one is set.
    pub fn for_deployment(deployments_path: &str, slug: &str) -> Log {
        let mut log = Log::open(file_for(deployments_path, slug));
        log.slug = Some(slug.to_string());
        log
    }

    /// [`Log::open`], keeping `keep` entries.
    pub fn open_keeping(file: Option<PathBuf>, keep: usize) -> Log {
        let mut log = Log {
            next_seq: 0,
            entries: VecDeque::new(),
            keep: keep.max(1),
            file,
            written: 0,
            cut: false,
            head: String::new(),
            breaks: Vec::new(),
            slug: None,
        };
        let Some(path) = &log.file else { return log };
        let Ok(text) = std::fs::read_to_string(path) else { return log };
        log.cut = !text.is_empty() && !text.ends_with('\n');
        let mut chained = false;
        for line in text.lines() {
            log.written += 1;
            // A line a crash cut short is skipped, not fatal: the others still replay, and the
            // chain goes on from the line before it.
            if let Ok(entry) = serde_json::from_str::<LogEntry>(line) {
                if (chained || !entry.prev.is_empty()) && !log.head.is_empty() && entry.prev != log.head {
                    log.breaks.push(entry.seq);
                }
                chained |= !entry.prev.is_empty();
                log.head = line_hash(line);
                log.next_seq = log.next_seq.max(entry.seq + 1);
                log.entries.push_back(entry);
            }
        }
        while log.entries.len() > log.keep {
            log.entries.pop_front();
        }
        // Only the entries kept count (the file's first line has nothing before it to check).
        log.breaks.retain(|seq| log.entries.front().is_some_and(|oldest| *seq >= oldest.seq));
        log
    }

    /// Append `entry`, numbering it and chaining it to the one before, and drop the oldest past
    /// what it keeps; keep it in the file.
    pub fn push(&mut self, mut entry: LogEntry) {
        entry.seq = self.next_seq;
        self.next_seq += 1;
        entry.prev = self.head.clone();
        let line = match serde_json::to_string(&entry) {
            Ok(line) => line,
            Err(e) => {
                eprintln!("holon: could not write a flow log entry: {e}");
                return;
            }
        };
        self.head = line_hash(&line);
        self.export("run", entry.seq, &self.head, entry.at_ms);
        self.entries.push_back(entry);
        while self.entries.len() > self.keep {
            self.entries.pop_front();
        }
        if let Err(e) = self.keep_line(&line) {
            eprintln!("holon: could not keep the flow log in {:?}: {e}", self.file);
        }
    }

    /// Write the newest entry's `line` to the file: appended, or, once the file would hold twice
    /// what the log keeps, the file cut back to the newest it keeps, those lines as they were.
    fn keep_line(&mut self, line: &str) -> std::io::Result<()> {
        let Some(path) = self.file.clone() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if self.written + 1 > 2 * self.keep {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<&str> = text.lines().filter(|l| serde_json::from_str::<LogEntry>(l).is_ok()).collect();
            let kept: Vec<&str> = lines[lines.len().saturating_sub(self.keep - 1)..].iter().copied().chain([line]).collect();
            let mut all = String::new();
            for l in &kept {
                all.push_str(l);
                all.push('\n');
            }
            // The witness hears that the runs before the oldest kept are gone by the host's choice.
            if let Ok(oldest) = serde_json::from_str::<LogEntry>(kept[0])
                && !oldest.prev.is_empty()
            {
                let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                self.export("trim", oldest.seq, &oldest.prev, now_ms);
            }
            crate::sealing::write_private(&path, all.as_bytes())?;
            self.written = all.lines().count();
            self.cut = false;
            return Ok(());
        }
        use std::io::Write;
        let text = if std::mem::take(&mut self.cut) { format!("\n{line}\n") } else { format!("{line}\n") };
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
        file.write_all(text.as_bytes())?;
        self.written += 1;
        Ok(())
    }

    /// Queue a head for the witness (heads.rs), when this is a deployment's log.
    fn export(&self, kind: &str, seq: u64, head: &str, at_ms: u64) {
        if let Some(slug) = &self.slug {
            crate::heads::note(crate::heads::Head { deployment: slug.clone(), kind: kind.into(), seq, head: head.into(), at_ms });
        }
    }

    /// The hash of the newest entry's line: what the next entry's `prev` will be, and what a copy
    /// of the log can be checked up to.
    pub fn head(&self) -> &str {
        &self.head
    }

    /// The log's lines exactly as written, oldest first, so a copy checks against the chain: the
    /// file's (every line it holds, a cut one too), or, kept in memory only, each entry as it was
    /// written.
    pub fn lines(&self) -> Vec<String> {
        if let Some(path) = &self.file {
            return std::fs::read_to_string(path).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default();
        }
        self.entries.iter().filter_map(|e| serde_json::to_string(e).ok()).collect()
    }

    /// The seqs of the entries kept that don't follow the line before them: the log was changed
    /// outside holon there.
    pub fn breaks(&self) -> &[u64] {
        &self.breaks
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
            prev: String::new(),
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
            claims: vec![],
            by: None,
        }
    }

    #[test]
    fn numbers_entries_reads_past_a_seq_and_keeps_the_newest() {
        let keep = 20;
        let mut log = Log::open_keeping(None, keep);
        for _ in 0..(keep + 5) {
            log.push(entry());
        }
        assert_eq!(log.next_seq(), (keep + 5) as u64);
        let all = log.after(None, usize::MAX);
        assert_eq!(all.len(), keep);
        assert_eq!(all[0].seq, 5, "the five oldest dropped off");
        let newer = log.after(Some((keep + 2) as u64), 10);
        assert_eq!(newer.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![(keep + 3) as u64, (keep + 4) as u64]);
        assert_eq!(log.after(None, 3).iter().map(|e| e.seq).collect::<Vec<_>>(), vec![22, 23, 24], "the newest 3");
    }

    /// A fresh directory for one test's log files.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn each_entry_names_the_line_before_it_and_the_chain_survives_cutting_back() {
        let dir = temp_dir("chain");
        let path = dir.join("slug.jsonl");
        let mut log = Log::open_keeping(Some(path.clone()), 3);
        for _ in 0..3 {
            log.push(entry());
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let first: LogEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.prev, "", "the first entry has nothing before it");
        for pair in lines.windows(2) {
            let next: LogEntry = serde_json::from_str(pair[1]).unwrap();
            assert_eq!(next.prev, line_hash(pair[0]));
        }
        assert_eq!(log.head(), line_hash(lines[2]));

        // Past twice what it keeps, the file is cut back to the newest lines as they were: the
        // chain still checks, from the oldest kept, and a reopened log goes on from its head.
        for _ in 0..4 {
            log.push(entry());
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 3, "cut back to the newest 3 on the 7th");
        let back = Log::open_keeping(Some(path.clone()), 3);
        assert!(back.breaks().is_empty(), "{:?}", back.breaks());
        assert_eq!(back.head(), log.head());
        assert_eq!(back.after(None, 10).iter().map(|e| e.seq).collect::<Vec<_>>(), [4, 5, 6]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_changed_removed_or_inserted_outside_holon_is_a_break_at_the_entry_after_it() {
        let dir = temp_dir("breaks");
        let path = dir.join("slug.jsonl");
        let mut log = Log::open_keeping(Some(path.clone()), 10);
        for _ in 0..5 {
            log.push(entry());
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let write = |lines: &[String]| std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let breaks = || Log::open_keeping(Some(path.clone()), 10).breaks().to_vec();
        assert!(breaks().is_empty());

        // A run's outcome changed: the entry after it no longer follows.
        let mut changed = lines.clone();
        changed[1] = changed[1].replace("\"tick\"", "\"trigger\"");
        write(&changed);
        assert_eq!(breaks(), [2]);

        // A run removed.
        let mut removed = lines.clone();
        removed.remove(2);
        write(&removed);
        assert_eq!(breaks(), [3]);

        // The newest run removed leaves no break: what proves it is the head, kept elsewhere.
        write(&lines[..4]);
        assert!(breaks().is_empty());
        assert_ne!(Log::open_keeping(Some(path.clone()), 10).head(), log.head());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn entries_from_before_the_chain_read_back_and_the_chain_starts_after_them() {
        let dir = temp_dir("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("slug.jsonl");
        let old: Vec<String> = (0..3)
            .map(|seq| serde_json::to_string(&LogEntry { seq, ..entry() }).unwrap())
            .collect();
        std::fs::write(&path, old.join("\n") + "\n").unwrap();
        let mut log = Log::open_keeping(Some(path.clone()), 10);
        assert!(log.breaks().is_empty());
        log.push(entry());
        let back = Log::open_keeping(Some(path.clone()), 10);
        assert!(back.breaks().is_empty());
        assert_eq!(back.after(None, 10).last().unwrap().prev, line_hash(&old[2]));
        let _ = std::fs::remove_dir_all(&dir);
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
        const KEEP: usize = 20;
        let dir = temp_dir("restart");
        let path = dir.join("nested").join("slug.jsonl");
        let open = || Log::open_keeping(Some(path.clone()), KEEP);
        let mut log = open();
        for _ in 0..3 {
            log.push(entry());
        }
        // A restart: the entries come back, and numbering carries on.
        let mut again = open();
        assert_eq!(again.after(None, 10).iter().map(|e| e.seq).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(again.next_seq(), 3);
        again.push(entry());
        assert_eq!(open().next_seq(), 4);

        // A line a crash cut short is skipped; the rest still read, and the next entry starts a
        // line of its own, chained to the last whole one.
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"seq\": 9, \"sess");
        std::fs::write(&path, text).unwrap();
        let mut cut = open();
        assert_eq!(cut.after(None, 10).len(), 4);
        cut.push(entry());
        let back = open();
        assert_eq!(back.after(None, 10).len(), 5);
        assert!(back.breaks().is_empty(), "{:?}", back.breaks());

        // Bounded: past twice what it keeps, the file is cut back to the newest it keeps.
        let mut big = open();
        for _ in 0..(2 * KEEP) {
            big.push(entry());
        }
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert!((KEEP..=2 * KEEP).contains(&lines), "{lines}");
        let back = open();
        assert_eq!(back.after(None, usize::MAX).len(), KEEP);
        assert_eq!(back.next_seq(), big.next_seq());
        assert!(back.breaks().is_empty());
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
