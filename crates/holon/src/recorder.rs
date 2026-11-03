//! A deployment's flow log: one entry per run (a request on its port, a `/trigger`, a clock tick)
//! holding everything that run depended on and everything it produced.
//!
//! Evaluation is pure and deterministic, so an entry is enough to run it again exactly: the
//! request record, the state the run read (`state_before`), and the sourced values (clock,
//! random, ingress) are its whole input. The editor's view mode replays entries node by node
//! from just this, to show what every node computed and to step through a past run.
//!
//! The newest entries per deployment are kept in memory, and in a file of the deployment's own
//! beside the deployments file (`<deployments>.logs/<slug>.jsonl`, a JSON line per entry,
//! owner-only), so a restart reads them back and replay still has them. How many, and for how
//! long, is the deployment's retention (its `keep_runs` and `keep_run_days` limits,
//! permissions.rs; the host's `--keep-runs`, [`DEFAULT_KEEP`] without it, at any age). An entry is
//! written as the control plane serves it: secrets and tokens already redacted. The file is cut
//! back to the newest it keeps once it holds twice that, or once its oldest line is a day past the
//! age it keeps; it always keeps its newest line, which carries the numbering and the chain on. A
//! line half-written by a crash is skipped.
//!
//! The file is a hash chain: each entry's `prev` is the BLAKE3 hash of the line before it, exactly
//! as written. Cutting the file back keeps the newest lines byte for byte, so the chain still
//! checks from the oldest line kept (whose `prev` names one no longer there). Opening the file
//! checks it: an entry whose `prev` isn't the line before it (one changed, removed or inserted
//! outside holon) is a break, reported by its seq. Entries from before the chain (no `prev`)
//! check only once one with a `prev` has been seen.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use holon_core::Value;

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
    /// The state it left, once it committed: the hash of a snapshot in the content store
    /// (`restore::save_snapshot`), what `holon restore --to-run` puts back. Absent for a
    /// deployment without state, a run that didn't commit, and entries logged before rc.30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_after: Option<String>,
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
    /// For a part that settled waiting: the packs it had settled, the sends that didn't wait,
    /// which go out as it parks (channels.rs). A later part sends only the ones these didn't.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub progress: Vec<PerformedEntry>,
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
    /// The secrets the run was given, by name: the version and fingerprint its marks name
    /// (`v3 #a1b2c3d4`, `‹secret:name v3 #a1b2c3d4›`), so which value it had shows even where it
    /// logged none, and `holon verify` stands in the same marks. Absent in entries logged before.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, String>,
    /// Whether text the run had looked like a mark the log writes in place of a value it never
    /// holds (`‹secret:name›`, `‹token:name›`), and was escaped: each `‹` of its own before one
    /// written twice, so that a mark is only ever one the log wrote (`secrets::write_marked`).
    /// Absent when nothing was, and in entries logged before look-alikes were escaped.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub escaped: bool,
    /// The credentials its calls answered, until the entry is logged (`log_run`): not part of it.
    #[serde(skip)]
    pub answered: AnswerTokens,
}

/// The credentials a run's calls answered, name and value, on their way to the redactor that
/// keeps them out of the log (`secrets::answered_tokens`): never serialized, never printed.
#[derive(Clone, Default)]
pub struct AnswerTokens(pub Vec<(String, String)>);

impl std::fmt::Debug for AnswerTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AnswerTokens({} kept out of sight)", self.0.len())
    }
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
    /// What names its text in the blob store (blobs.rs: the BLAKE3 of the text, hex; in a log from
    /// before it, the text's value hash in the content store), when the folder keeps bytes and the
    /// text isn't in the entry: the whole file's, or the range's. A sweep may remove the text once
    /// no kept run refers to it within the folder's window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
    /// For part of a file (`lines`, `slice`): where it is and what proves it part of the file. The
    /// read's answer is then not in the entry: it is the range's text, or its lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<LoggedRange>,
}

/// Part of a file a run read (ranges.rs): bytes `start..end` of the file whose hash is
/// [`LoggedFile::hash`]. When its folder keeps bytes, the bytes around it to whole BLAKE3 chunks
/// (hex) and the chaining values of every other subtree of the file (hex) tie it to that hash, so
/// `holon verify` checks it without the file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoggedRange {
    pub start: u64,
    pub end: u64,
    /// Its text, when the entry holds it (else by [`LoggedFile::blob`], or not at all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub before: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub after: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof: Vec<String>,
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
    /// Whether it keeps anything: off for a run that won't be logged (an unrecorded handler's,
    /// docs/L4s/request-path-L4.md §4.5 a), which then times and copies nothing.
    on: bool,
}

impl Timer {
    pub const DEPTH: usize = 3;
    /// The largest output kept per node, as JSON: enough for records, messages and signatures;
    /// a large body is in the request or response already.
    pub const MAX_RECORDED_BYTES: usize = 16 * 1024;

    pub fn new() -> Timer {
        Timer { start: std::time::Instant::now(), open: Vec::new(), round: 0, produced: None, timeline: Vec::new(), reason: None, on: true }
    }

    /// One that keeps nothing: its timeline stays empty.
    pub fn off() -> Timer {
        Timer { on: false, ..Timer::new() }
    }

    fn now_us(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1e6
    }
}

/// At least as many bytes as `outputs`' JSON (`Vec<Option<SValue>>`) takes, or none past
/// [`Timer::MAX_RECORDED_BYTES`] or with a number that isn't finite (written as `null`): then only
/// counting the JSON says.
fn outputs_bound(outputs: &[holon_core::Slot]) -> Option<usize> {
    let mut n = 2;
    for o in outputs {
        n += 1 + match o.value() {
            Some(v) => value_bound(v, Timer::MAX_RECORDED_BYTES.checked_sub(n)?)?,
            None => 4,
        };
    }
    (n <= Timer::MAX_RECORDED_BYTES).then_some(n)
}

/// At least as many bytes as `v`'s JSON as an `SValue` takes, or none past `most`. Each value's
/// frame (`{"type":"Record","value":…}`, a `Sum`'s arm and payload) is under 64 bytes (a `Sum`'s,
/// with its arm's 20 digits, is given 84), a finite number writes at most 24, and an escaped
/// byte at most 6 (`\u00XX`); a record's field (`["k",v]` and a comma) is under 16 beside its key.
fn value_bound(v: &Value, most: usize) -> Option<usize> {
    let n = match v {
        Value::Number(x) if x.is_finite() => 64,
        Value::Number(_) => return None,
        Value::Boolean(_) => 64,
        Value::Text(t) => 64 + 6 * t.len(),
        Value::List(xs) => {
            let mut n = 64;
            for x in xs {
                n += 1 + value_bound(x, most.checked_sub(n)?)?;
            }
            n
        }
        Value::Record(fields) => {
            let mut n = 64;
            for (k, x) in fields {
                n += 16 + 6 * k.len();
                n += value_bound(x, most.checked_sub(n)?)?;
            }
            n
        }
        Value::Effect { sink, payload } => 64 + 6 * sink.len() + value_bound(payload, most)?,
        Value::Sum { payload, .. } => 84 + value_bound(payload, most)?,
    };
    (n <= most).then_some(n)
}

/// Counts what is written to it, and refuses past `most` bytes.
struct Counted {
    n: usize,
    most: usize,
}

impl std::io::Write for Counted {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.n += bytes.len();
        if self.n > self.most {
            return Err(std::io::Error::other("past the limit"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl holon_core::Observer for Timer {
    fn enter(&mut self, path: &[u32]) {
        if self.on && path.len() <= Self::DEPTH {
            let t = self.now_us();
            self.open.push(t);
        }
    }
    fn produced(&mut self, path: &[u32], outputs: &[holon_core::Slot]) {
        if self.on && path.len() <= Self::DEPTH {
            // A pending output (a round still waiting on an answer) is logged as nothing yet.
            // Kept when their JSON fits, as the line will write it: an upper bound settles almost
            // every node without writing anything, and only one past it is counted exactly, as
            // JSON written nowhere (interpreter-path-L4.md, step 2).
            let surely = outputs_bound(outputs).is_some_and(|n| n <= Self::MAX_RECORDED_BYTES);
            let outputs: Vec<Option<SValue>> = outputs.iter().map(|o| o.value().map(SValue::from_core)).collect();
            let fits = surely || serde_json::to_writer(Counted { n: 0, most: Self::MAX_RECORDED_BYTES }, &outputs).is_ok();
            self.produced = fits.then_some(outputs);
        }
    }
    fn leave(&mut self, path: &[u32]) {
        if self.on && path.len() <= Self::DEPTH {
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
        if self.on && path.len() <= Self::DEPTH {
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
    /// How old an entry it keeps, in milliseconds (none: any age).
    max_age_ms: Option<u64>,
    /// When the file's oldest line ran (0: no file, or nothing in it).
    file_oldest_ms: u64,
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
    /// The file, open for appending once a line is written, so a line is one write; closed before
    /// a cut-back replaces the file, and before the file is deleted.
    handle: Option<std::fs::File>,
    /// Where each line of the file that parsed is, and what a cut-back needs of it, so a cut-back
    /// copies the lines it keeps instead of parsing them all again (docs/L4s/request-path-L4.md,
    /// C2 c). Oldest first.
    lines: VecDeque<LineAt>,
    /// How long the file is, as this log wrote it: a file of another length was changed outside
    /// holon, and a cut-back reads it afresh.
    file_len: u64,
    /// Whether lines were written since the file was last synced to disk (`sync_logs`).
    dirty: bool,
    /// The seqs of the recent entries whose line couldn't be written to the file, so a strict run
    /// isn't told its record is on disk (`LogCell::sync_through`). The newest few.
    unkept: VecDeque<u64>,
    /// Entries let go of from memory, to be freed once the log's lock is released
    /// (`LogCell::with`): freeing one, its timeline and all, took a quarter of the time a run held
    /// the lock.
    gone: Vec<LogEntry>,
}

/// How often the logs' files are synced to disk (`sync_logs`): what a power loss can lose of them,
/// at most (docs/L4s/request-path-L4.md, C2 d).
pub const SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// Sync `path`'s written data to disk. A file that isn't there any more (its deployment was
/// deleted) has nothing to sync.
pub fn sync_file(path: &Path) -> std::io::Result<()> {
    match std::fs::OpenOptions::new().write(true).open(path) {
        // Any descriptor of a file flushes the file's data; opened for writing, as Windows asks.
        Ok(file) => file.sync_data(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Sync every run log and event log of `deployments` written since it was last synced (C2 d):
/// every `SYNC_EVERY`, and before a witness is sent a head (heads.rs's `before_send`). A head is
/// queued while its line is written, under its log's lock (or the table's, for an event), so once
/// this has taken each lock, the line of every head queued before it is written, and the sync
/// after covers it. No lock is held while a file syncs. A failure puts that log's flag back.
pub(crate) fn sync_logs(deployments: &std::sync::Mutex<BTreeMap<String, crate::Deployment>>) -> Result<(), String> {
    let (logs, events): (Vec<std::sync::Arc<LogCell>>, Vec<(String, PathBuf)>) = {
        let mut table = deployments.lock().unwrap();
        let logs = table.values().map(|d| std::sync::Arc::clone(&d.log)).collect();
        let events = table.iter_mut().filter_map(|(slug, d)| d.events.take_dirty().map(|p| (slug.clone(), p))).collect();
        (logs, events)
    };
    let mut failed: Option<String> = None;
    for log in logs {
        // Through the cell, so a strict run waiting on its sync knows when it is done.
        if let Err(e) = log.sync() {
            failed.get_or_insert(e);
        }
    }
    for (slug, path) in events {
        if let Err(e) = sync_file(&path) {
            if let Some(d) = deployments.lock().unwrap().get_mut(&slug) {
                d.events.still_dirty();
            }
            failed.get_or_insert(format!("could not sync the events {}: {e}", path.display()));
        }
    }
    failed.map_or(Ok(()), Err)
}

/// An entry's line made before its log's lock is taken (docs/L4s/request-path-plan.md, after 12.4):
/// all of it but the two fields only the lock knows, its seq and the hash of the line before
/// (`prev`). The entry's JSON with seq 0 and no prev starts `{"seq":0,`, `seq` being its first
/// field and `prev` (left out when empty) its second; so the line is `{"seq":N`, `,"prev":"H"`
/// when there is one, and the rest as made: exactly what serializing the numbered entry writes,
/// so the chain hashes the same bytes. Whatever doesn't start so is made again under the lock.
pub struct Made(Option<String>);

impl Made {
    const START: &'static str = "{\"seq\":0,";

    /// `entry`'s line, but for its seq and prev: for an entry not yet numbered or chained.
    pub fn of(entry: &LogEntry) -> Made {
        if entry.seq != 0 || !entry.prev.is_empty() {
            return Made(None);
        }
        // Written into room for this thread's last line (and some), instead of a buffer doubled
        // from nothing: the same bytes (docs/L4s/interpreter-path-L4.md, the per-request
        // bookkeeping).
        thread_local! {
            static LAST: std::cell::Cell<usize> = const { std::cell::Cell::new(4096) };
        }
        let mut buf = Vec::with_capacity(LAST.get().min(1 << 20) + 512);
        if serde_json::to_writer(&mut buf, entry).is_err() {
            return Made(None);
        }
        LAST.set(buf.len());
        Made(String::from_utf8(buf).ok().filter(|t| t.starts_with(Made::START)))
    }

    /// Whether its line may hold a `‹`, which redaction would escape where it reads as a mark:
    /// true for a line that couldn't be made.
    pub fn may_hold_a_mark(&self) -> bool {
        self.0.as_deref().is_none_or(|line| line.contains('‹'))
    }

    /// The line numbered `seq`, after the line hashed `prev` (hex, or empty for none).
    pub(crate) fn line(&self, seq: u64, prev: &str) -> Option<String> {
        let made = self.0.as_deref()?;
        let rest = &made[Made::START.len() - 1..];
        let mut line = String::with_capacity(made.len() + prev.len() + 32);
        line.push_str("{\"seq\":");
        line.push_str(&seq.to_string());
        if !prev.is_empty() {
            line.push_str(",\"prev\":\"");
            line.push_str(prev);
            line.push('"');
        }
        line.push_str(rest);
        Some(line)
    }
}

/// A line of the log's file: its bytes (without the line end), and the parts of its entry a
/// cut-back decides by.
#[derive(Debug, Clone)]
struct LineAt {
    offset: u64,
    len: usize,
    at_ms: u64,
    seq: u64,
    prev: String,
}

/// The lines of a log file's `text`, as `open_retaining` reads them: each line (as `str::lines`
/// splits them) with its entry when it parses, and where it is.
fn lines_of(text: &str) -> Vec<(&str, Option<LogEntry>, u64)> {
    let mut offset = 0u64;
    text.split_inclusive('\n')
        .map(|raw| {
            let at = offset;
            offset += raw.len() as u64;
            let line = raw.strip_suffix('\n').map_or(raw, |l| l.strip_suffix('\r').unwrap_or(l));
            (line, serde_json::from_str::<LogEntry>(line).ok(), at)
        })
        .collect()
}

/// One deployment's log behind a lock of its own (docs/L4s/request-path-L4.md, C2): runs of one
/// deployment append one at a time, in one order, and runs of different deployments never wait
/// for each other. The deployments table is never taken while this lock is held. `next_seq` is
/// mirrored outside the lock, for a reader that holds the table and needs only that.
///
/// It syncs its file too (C2 d and e): `synced` is the seq up to which every line is on disk, and
/// a sync holds its lock, so one run's sync covers every line written before it started, and a
/// run that waited for it finds its own line covered and returns without one of its own (group
/// commit). The log's lock is taken only to read what to sync, never across the sync itself, so
/// runs go on writing while one syncs.
#[derive(Debug)]
pub struct LogCell {
    log: Mutex<Log>,
    next_seq: AtomicU64,
    /// Every entry before this seq is on disk.
    synced: Mutex<u64>,
    /// How many times its file was synced, for the tests and the throughput check.
    syncs: AtomicU64,
}

impl LogCell {
    pub fn new(log: Log) -> LogCell {
        let next_seq = AtomicU64::new(log.next_seq);
        let synced = Mutex::new(log.next_seq);
        LogCell { log: Mutex::new(log), next_seq, synced, syncs: AtomicU64::new(0) }
    }

    /// Return once entry `seq` is on disk, with every entry before it: at once if a sync since it
    /// was written covered it, or after one sync, which covers every entry written so far, its
    /// own and others' (a strict run, before its answer). An error if its line was never written,
    /// or the sync failed.
    pub fn sync_through(&self, seq: u64) -> Result<(), String> {
        let mut synced = self.synced.lock().unwrap();
        if *synced > seq {
            return Ok(());
        }
        self.sync_locked(&mut synced)?;
        if self.with(|log| log.unkept.contains(&seq)) {
            return Err(format!("its record (entry {seq}) couldn't be written to the log's file"));
        }
        Ok(())
    }

    /// Sync whatever was written since the last sync (the syncer, each `SYNC_EVERY`).
    pub fn sync(&self) -> Result<(), String> {
        self.sync_locked(&mut self.synced.lock().unwrap())
    }

    fn sync_locked(&self, synced: &mut u64) -> Result<(), String> {
        let (path, upto) = self.with(|log| (log.take_dirty(), log.next_seq));
        if let Some(path) = path {
            self.syncs.fetch_add(1, Ordering::Relaxed);
            if let Err(e) = sync_file(&path) {
                self.with(Log::still_dirty);
                return Err(format!("could not sync the run log {}: {e}", path.display()));
            }
        }
        *synced = upto;
        Ok(())
    }

    /// How many times its file was synced.
    #[cfg(test)]
    pub fn syncs(&self) -> u64 {
        self.syncs.load(Ordering::Relaxed)
    }

    /// The seq the next entry will get, without taking the lock.
    pub fn next_seq(&self) -> u64 {
        self.next_seq.load(Ordering::Acquire)
    }

    /// Run `f` on the log, under its lock.
    pub fn with<T>(&self, f: impl FnOnce(&mut Log) -> T) -> T {
        let mut log = self.log.lock().unwrap();
        let out = f(&mut log);
        self.next_seq.store(log.next_seq, Ordering::Release);
        let gone = std::mem::take(&mut log.gone);
        drop(log);
        // The entries it let go of, freed with the lock released.
        drop(gone);
        out
    }
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
        Log::open_retaining(file, keep, None)
    }

    /// [`Log::open`], keeping `keep` entries, none older than `max_age_ms`.
    pub fn open_retaining(file: Option<PathBuf>, keep: usize, max_age_ms: Option<u64>) -> Log {
        let mut log = Log {
            next_seq: 0,
            entries: VecDeque::new(),
            keep: keep.max(1),
            max_age_ms,
            file_oldest_ms: 0,
            file,
            written: 0,
            cut: false,
            head: String::new(),
            breaks: Vec::new(),
            slug: None,
            handle: None,
            lines: VecDeque::new(),
            file_len: 0,
            dirty: false,
            unkept: VecDeque::new(),
            gone: Vec::new(),
        };
        let Some(path) = &log.file else { return log };
        let Ok(text) = std::fs::read_to_string(path) else { return log };
        log.cut = !text.is_empty() && !text.ends_with('\n');
        log.file_len = text.len() as u64;
        let mut chained = false;
        for (line, entry, offset) in lines_of(&text) {
            log.written += 1;
            // A line a crash cut short is skipped, not fatal: the others still replay, and the
            // chain goes on from the line before it.
            if let Some(entry) = entry {
                log.lines.push_back(LineAt { offset, len: line.len(), at_ms: entry.at_ms, seq: entry.seq, prev: entry.prev.clone() });
                if (chained || !entry.prev.is_empty()) && !log.head.is_empty() && entry.prev != log.head {
                    log.breaks.push(entry.seq);
                }
                chained |= !entry.prev.is_empty();
                if log.entries.is_empty() {
                    log.file_oldest_ms = entry.at_ms;
                }
                log.head = line_hash(line);
                log.next_seq = log.next_seq.max(entry.seq + 1);
                log.entries.push_back(entry);
            }
        }
        log.prune(now_ms());
        log.cut_back_if_aged();
        log
    }

    /// Keep `keep` entries from now on, none older than `max_age_ms` (none: any age): the
    /// deployment's retention, set at deploy and load and when its grant changes. Keeping more
    /// than it did reads back what the file still holds.
    pub fn retain(&mut self, keep: usize, max_age_ms: Option<u64>) {
        let keep = keep.max(1);
        if (keep, max_age_ms) == (self.keep, self.max_age_ms) {
            return;
        }
        if keep > self.keep && self.file.is_some() {
            let slug = self.slug.take();
            *self = Log::open_retaining(self.file.take(), keep, max_age_ms);
            self.slug = slug;
            return;
        }
        (self.keep, self.max_age_ms) = (keep, max_age_ms);
        self.prune(now_ms());
        self.cut_back_if_aged();
    }

    /// Drop the entries past what it keeps: the oldest past `keep`, and any older than its age.
    /// Only the entries kept count for breaks (the oldest has nothing before it to check).
    fn prune(&mut self, now_ms: u64) {
        let cutoff = self.max_age_ms.map(|age| now_ms.saturating_sub(age));
        while self.entries.len() > self.keep || self.entries.front().is_some_and(|e| cutoff.is_some_and(|c| e.at_ms < c)) {
            if let Some(old) = self.entries.pop_front() {
                // Freed after the lock, unless nobody frees them (a log used without its cell).
                if self.gone.len() < 64 {
                    self.gone.push(old);
                }
            }
        }
        self.breaks.retain(|seq| self.entries.front().is_some_and(|oldest| *seq >= oldest.seq));
    }

    /// Whether the file's oldest line is a day past the age it keeps: time to cut it back.
    fn file_aged(&self, now_ms: u64) -> bool {
        const DAY_MS: u64 = 24 * 3600 * 1000;
        self.file.is_some()
            && self.file_oldest_ms != 0
            && self.max_age_ms.is_some_and(|age| self.file_oldest_ms.saturating_add(age).saturating_add(DAY_MS) < now_ms)
    }

    /// Cut the file back when its oldest line is past the age it keeps, a run or not.
    fn cut_back_if_aged(&mut self) {
        if self.file_aged(now_ms())
            && let Err(e) = self.cut_back(None)
        {
            eprintln!("holon: could not cut back the flow log in {:?}: {e}", self.file);
        }
    }

    /// Append `entry`, numbering it and chaining it to the one before, and drop the oldest past
    /// what it keeps; keep it in the file.
    #[cfg(test)]
    pub fn push(&mut self, entry: LogEntry) -> u64 {
        let made = Made::of(&entry);
        self.push_made(entry, made)
    }

    /// [`push`](Log::push), with `entry`'s line made beforehand ([`Made::of`]), before this log's
    /// lock was taken: under it, only its seq and the hash of the line before go in front.
    pub fn push_made(&mut self, mut entry: LogEntry, made: Made) -> u64 {
        let seq = self.next_seq;
        entry.seq = seq;
        self.next_seq += 1;
        entry.prev = self.head.clone();
        let line = match made.line(seq, &entry.prev).map_or_else(|| serde_json::to_string(&entry), Ok) {
            Ok(line) => line,
            Err(e) => {
                eprintln!("holon: could not write a flow log entry: {e}");
                self.unkept(seq);
                return seq;
            }
        };
        self.head = line_hash(&line);
        self.export("run", entry.seq, &self.head, entry.at_ms);
        let at = LineAt { offset: 0, len: line.len(), at_ms: entry.at_ms, seq: entry.seq, prev: entry.prev.clone() };
        self.entries.push_back(entry);
        self.prune(now_ms());
        if let Err(e) = self.keep_line(&line, at) {
            eprintln!("holon: could not keep the flow log in {:?}: {e}", self.file);
            self.unkept(seq);
        }
        seq
    }

    /// Entry `seq`'s line isn't in the file.
    fn unkept(&mut self, seq: u64) {
        const KEPT: usize = 1024;
        if self.unkept.len() == KEPT {
            self.unkept.pop_front();
        }
        self.unkept.push_back(seq);
    }

    /// Write the newest entry's `line` (a run at `at_ms`) to the file: appended, or, once the
    /// file would hold twice what the log keeps or its oldest line is past the age it keeps, the
    /// file cut back to what it keeps.
    fn keep_line(&mut self, line: &str, mut at: LineAt) -> std::io::Result<()> {
        let Some(path) = self.file.clone() else { return Ok(()) };
        if self.handle.is_none()
            && let Some(dir) = path.parent()
        {
            std::fs::create_dir_all(dir)?;
        }
        if self.written + 1 > 2 * self.keep || self.file_aged(now_ms()) {
            return self.cut_back(Some((line, at)));
        }
        if self.written == 0 {
            self.file_oldest_ms = at.at_ms;
        }
        use std::io::Write;
        let after_cut = std::mem::take(&mut self.cut);
        let text = if after_cut { format!("\n{line}\n") } else { format!("{line}\n") };
        if self.handle.is_none() {
            let new = !path.exists();
            let mut options = std::fs::OpenOptions::new();
            options.append(true).create(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(&path)?;
            if new {
                crate::sealing::restrict_to_owner(&path)?;
            }
            self.handle = Some(file);
        }
        if let Err(e) = self.handle.as_mut().expect("opened above").write_all(text.as_bytes()) {
            // Opened again for the next line, in case the file went away under it; a file of
            // another length is read afresh by the next cut-back.
            self.handle = None;
            return Err(e);
        }
        at.offset = self.file_len + u64::from(after_cut);
        self.file_len += text.len() as u64;
        self.lines.push_back(at);
        self.written += 1;
        self.dirty = true;
        Ok(())
    }

    /// Rewrite the file as the lines it keeps, byte for byte, then `line` (the newest entry's,
    /// when there is one): the newest `keep`, none older than its age, but always the newest
    /// line, which carries the numbering and the chain on.
    fn cut_back(&mut self, line: Option<(&str, LineAt)>) -> std::io::Result<()> {
        let Some(path) = self.file.clone() else { return Ok(()) };
        // Closed before the file is replaced: a handle would go on writing to the old one (and
        // Windows won't replace an open file). The next line opens the new one.
        self.handle = None;
        let cutoff = self.max_age_ms.map(|age| now_ms().saturating_sub(age));
        let bytes = std::fs::read(&path).unwrap_or_default();
        // A file this log didn't leave as it is (changed, cut or grown outside holon) is read
        // afresh, as a log opening it would; otherwise the index says where each line is.
        if bytes.len() as u64 != self.file_len {
            let text = String::from_utf8_lossy(&bytes);
            self.lines = lines_of(&text)
                .into_iter()
                .filter_map(|(l, e, offset)| e.map(|e| LineAt { offset, len: l.len(), at_ms: e.at_ms, seq: e.seq, prev: e.prev }))
                .collect();
        }
        // Each line it could keep, oldest first: the file's that parsed, then the newest entry's.
        let in_file = |at: &LineAt| bytes.get(at.offset as usize..at.offset as usize + at.len);
        let mut lines: Vec<(&[u8], &LineAt)> = self.lines.iter().filter_map(|at| in_file(at).map(|b| (b, at))).collect();
        if let Some((line, at)) = &line {
            lines.push((line.as_bytes(), at));
        }
        let newest = lines.len().saturating_sub(1);
        let start = lines.len().saturating_sub(self.keep);
        let kept: Vec<&(&[u8], &LineAt)> = lines
            .iter()
            .enumerate()
            .filter(|(i, (_, at))| *i == newest || (*i >= start && cutoff.is_none_or(|c| at.at_ms >= c)))
            .map(|(_, l)| l)
            .collect();
        if kept.len() == lines.len() && line.is_none() {
            return Ok(());
        }
        let mut all = Vec::with_capacity(kept.iter().map(|(b, _)| b.len() + 1).sum());
        let mut index = VecDeque::with_capacity(kept.len());
        for (b, at) in &kept {
            index.push_back(LineAt { offset: all.len() as u64, ..(*at).clone() });
            all.extend_from_slice(b);
            all.push(b'\n');
        }
        // The witness hears that the runs before the oldest kept are gone by the host's choice.
        if let Some((_, oldest)) = kept.first()
            && !oldest.prev.is_empty()
            && kept.len() < lines.len()
        {
            self.export("trim", oldest.seq, &oldest.prev, now_ms());
        }
        self.file_oldest_ms = kept.first().map_or(0, |(_, at)| at.at_ms);
        // Written whole and synced (write_private), the newest line with it.
        crate::sealing::write_private(&path, &all)?;
        self.written = kept.len();
        self.file_len = all.len() as u64;
        self.lines = index;
        self.cut = false;
        self.dirty = false;
        Ok(())
    }

    /// The file to sync, if lines were written since it last was: the flag is cleared, and put
    /// back with `still_dirty` if the sync fails.
    pub fn take_dirty(&mut self) -> Option<PathBuf> {
        std::mem::take(&mut self.dirty).then(|| self.file.clone()).flatten()
    }

    /// Its file still needs syncing: the last sync of it failed.
    pub fn still_dirty(&mut self) {
        self.dirty = true;
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
    pub fn delete_file(&mut self) {
        self.handle = None;
        self.lines.clear();
        self.file_len = 0;
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

/// Now, in milliseconds since the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
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
            state_after: None,
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
            progress: vec![],
            events: BTreeMap::new(),
            reads: vec![],
            retries: 0,
            claims: vec![],
            by: None,
            secrets: BTreeMap::new(),
            escaped: false,
            answered: AnswerTokens::default(),
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

    /// A line made before the lock is, byte for byte, what serializing the numbered and chained
    /// entry writes, with a prev and without; one already numbered is made under the lock instead;
    /// and a log written so reads back with its chain whole.
    #[test]
    fn a_line_made_before_the_lock_is_the_one_serializing_writes() {
        let full = LogEntry { route: Some("GET /a \"quoted\"".into()), error: Some("ü\n".into()), duration_ms: 1.5, ..entry() };
        let made = Made::of(&full);
        assert!(made.0.is_some());
        for (seq, prev) in [(0u64, ""), (7, "ab12"), (123_456, &"f".repeat(64)[..])] {
            let numbered = LogEntry { seq, prev: prev.to_string(), ..full.clone() };
            assert_eq!(made.line(seq, prev).unwrap(), serde_json::to_string(&numbered).unwrap(), "seq {seq}, prev {prev:?}");
        }
        assert!(Made::of(&LogEntry { seq: 3, ..entry() }).0.is_none(), "already numbered: made under the lock");
        let dir = temp_dir("made");
        let path = dir.join("slug.jsonl");
        let mut log = Log::open_keeping(Some(path.clone()), 10);
        for _ in 0..3 {
            log.push_made(full.clone(), Made::of(&full));
        }
        let back = Log::open_keeping(Some(path), 10);
        assert!(back.breaks().is_empty(), "the chain reads back whole");
        assert_eq!(back.after(None, 10).iter().map(|e| e.seq).collect::<Vec<_>>(), [0, 1, 2]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A timer that is off keeps nothing, at any depth: a run that won't be logged (§4.5 a).
    #[test]
    fn a_timer_that_is_off_keeps_nothing() {
        use holon_core::Observer;
        let mut t = Timer::off();
        t.enter(&[0]);
        t.produced(&[0], &[]);
        t.failed(&[0], "why");
        t.leave(&[0]);
        assert!(t.timeline.is_empty());
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

    /// A deployment's retention by age: runs older than it keeps drop from memory, the file is
    /// cut back once its oldest line is a day past it (the newest line always kept, so numbering
    /// and the chain go on), and keeping more again reads back what the file still holds.
    #[test]
    fn a_log_keeps_runs_by_age_and_reads_back_more_when_it_keeps_more() {
        const DAY: u64 = 24 * 3600 * 1000;
        let dir = temp_dir("age");
        let path = dir.join("slug.jsonl");
        let now = now_ms();
        let mut log = Log::open_keeping(Some(path.clone()), 10);
        for days_ago in [40, 35, 3, 1] {
            log.push(LogEntry { at_ms: now - days_ago * DAY, ..entry() });
        }
        let seqs = |log: &Log| log.after(None, 100).iter().map(|e| e.seq).collect::<Vec<_>>();
        let lines = || std::fs::read_to_string(&path).unwrap().lines().count();

        log.retain(10, Some(30 * DAY));
        assert_eq!(seqs(&log), [2, 3], "the two older than 30 days dropped");
        assert_eq!(lines(), 2, "and the file was cut back to the two it keeps");
        let back = Log::open_retaining(Some(path.clone()), 10, Some(30 * DAY));
        assert!(back.breaks().is_empty(), "the chain checks from the oldest kept: {:?}", back.breaks());
        assert_eq!(back.head(), log.head());

        // All of them past the age: the newest line stays in the file, so the next run goes on
        // from it.
        log.retain(10, Some(DAY / 2));
        assert_eq!(seqs(&log), Vec::<u64>::new());
        assert_eq!(lines(), 1);
        log.push(LogEntry { at_ms: now, ..entry() });
        assert_eq!(seqs(&log), [4]);
        let back = Log::open_retaining(Some(path.clone()), 10, Some(DAY / 2));
        assert_eq!((back.next_seq(), back.head()), (5, log.head()));

        // Fewer by count, then more again: what the file holds comes back.
        let mut log = Log::open_keeping(Some(path.clone()), 10);
        for _ in 0..4 {
            log.push(LogEntry { at_ms: now, ..entry() });
        }
        log.retain(2, None);
        assert_eq!(seqs(&log), [7, 8]);
        log.retain(10, None);
        assert_eq!(seqs(&log), [3, 4, 5, 6, 7, 8], "read back from the file");
        let _ = std::fs::remove_dir_all(&dir);
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

    /// What a cut-back wrote before it kept an index (step 4 of docs/L4s/request-path-plan.md):
    /// every line of `text` parsed, those that parse kept by the rule, then `line`. The index must
    /// write exactly this.
    fn cut_back_as_before(text: &str, keep: usize, line: &str) -> String {
        let mut lines: Vec<&str> = text.lines().filter(|l| serde_json::from_str::<LogEntry>(l).is_ok()).collect();
        lines.push(line);
        let (newest, start) = (lines.len() - 1, lines.len().saturating_sub(keep));
        lines.iter().enumerate().filter(|(i, _)| *i == newest || *i >= start).map(|(_, l)| format!("{l}\n")).collect()
    }

    #[test]
    fn a_cut_back_keeps_its_lines_byte_for_byte_without_parsing_them() {
        use std::io::Write;
        let dir = temp_dir("cut-bytes");
        let path = dir.join("slug.jsonl");
        {
            let mut log = Log::open_keeping(Some(path.clone()), 100);
            for _ in 0..3 {
                log.push(entry());
            }
        }
        // A line with a field this build doesn't know, then one a crash cut short.
        let unknown = serde_json::to_string(&LogEntry { seq: 3, ..entry() }).unwrap().replacen('{', "{\"later\":1,", 1);
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(format!("{unknown}\n{{\"seq\":4,\"pre").as_bytes()).unwrap();
        let mut log = Log::open_keeping(Some(path.clone()), 2);
        let before = std::fs::read_to_string(&path).unwrap();
        log.push(entry()); // past twice what it keeps: cut back
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, cut_back_as_before(&before, 2, after.lines().last().unwrap()));

        // Lines appended after a cut-back land in the new file at the offsets the index says, so
        // cut-back after cut-back the chain checks, and a reopened log goes on from its head.
        for _ in 0..9 {
            log.push(entry());
        }
        let back = Log::open_keeping(Some(path.clone()), 2);
        assert!(back.breaks().is_empty(), "{:?}", back.breaks());
        assert_eq!(back.head(), log.head());
        let mut back = back;
        for _ in 0..5 {
            back.push(entry());
        }
        let again = Log::open_keeping(Some(path.clone()), 2);
        assert!(again.breaks().is_empty(), "{:?}", again.breaks());
        assert_eq!(again.after(None, 10).iter().map(|e| e.seq).collect::<Vec<_>>(), [17, 18]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_changed_outside_holon_is_read_afresh_by_its_cut_back() {
        use std::io::Write;
        let dir = temp_dir("cut-changed");
        let path = dir.join("slug.jsonl");
        let mut log = Log::open_keeping(Some(path.clone()), 2);
        for _ in 0..3 {
            log.push(entry());
        }
        // A line added by hand: the file is no longer the length this log left it.
        let added = serde_json::to_string(&LogEntry { seq: 40, ..entry() }).unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(format!("{added}\n").as_bytes()).unwrap();
        log.push(entry());
        let before = std::fs::read_to_string(&path).unwrap();
        log.push(entry()); // the fifth line this log wrote: cut back
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, cut_back_as_before(&before, 2, after.lines().last().unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_is_dirty_from_a_write_until_it_is_synced_and_a_cut_back_syncs_it() {
        let dir = temp_dir("dirty");
        let path = dir.join("slug.jsonl");
        let mut log = Log::open_keeping(Some(path.clone()), 2);
        assert_eq!(log.take_dirty(), None, "nothing written yet");
        log.push(entry());
        assert_eq!(log.take_dirty(), Some(path.clone()));
        assert_eq!(log.take_dirty(), None, "taken: clean until the next line");
        log.push(entry());
        log.still_dirty();
        assert_eq!(log.take_dirty(), Some(path.clone()), "a failed sync puts it back");
        for _ in 0..2 {
            log.push(entry());
        }
        assert!(log.take_dirty().is_some());
        log.push(entry()); // the fifth line: cut back, written whole and synced
        assert_eq!(log.take_dirty(), None);
        assert!(sync_file(&dir.join("gone.jsonl")).is_ok(), "a file deleted with its deployment has nothing to sync");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One sync covers every entry written before it (C2 e, group commit): a run whose entry an
    /// earlier sync covered returns without one of its own, and a sync with nothing new written
    /// syncs nothing. An entry whose line couldn't be written isn't said to be kept, even once a
    /// sync after it succeeds.
    #[test]
    fn one_sync_covers_every_entry_written_before_it() {
        let dir = temp_dir("group-commit");
        let cell = LogCell::new(Log::open_keeping(Some(dir.join("slug.jsonl")), 100));
        let seqs: Vec<u64> = (0..5).map(|_| cell.with(|log| log.push(entry()))).collect();
        assert_eq!(cell.syncs(), 0, "writing syncs nothing");
        assert_eq!(cell.sync_through(seqs[4]), Ok(()));
        assert_eq!(cell.syncs(), 1);
        for &seq in &seqs {
            assert_eq!(cell.sync_through(seq), Ok(()));
        }
        assert_eq!(cell.syncs(), 1, "the four before it were covered by the one sync");
        assert_eq!(cell.sync(), Ok(()));
        assert_eq!(cell.syncs(), 1, "nothing new written: nothing to sync");
        let next = cell.with(|log| log.push(entry()));
        assert_eq!(cell.sync_through(next), Ok(()));
        assert_eq!(cell.syncs(), 2, "a new entry, a new sync");

        // Many runs at once: each returns with its entry on disk, and none syncs more than once.
        std::thread::scope(|s| {
            for _ in 0..32 {
                s.spawn(|| {
                    let seq = cell.with(|log| log.push(entry()));
                    assert_eq!(cell.sync_through(seq), Ok(()));
                });
            }
        });
        assert!(cell.syncs() <= 2 + 32, "{}", cell.syncs());
        assert_eq!(cell.with(Log::take_dirty), None, "every entry is on disk");

        // A line that couldn't be written: a folder where the file was.
        let file = dir.join("slug.jsonl");
        cell.with(|log| log.handle = None);
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let lost = cell.with(|log| log.push(entry()));
        std::fs::remove_dir(&file).unwrap();
        let kept = cell.with(|log| log.push(entry()));
        let said = cell.sync_through(lost).unwrap_err();
        assert!(said.contains(&format!("entry {lost}")), "{said}");
        assert_eq!(cell.sync_through(kept), Ok(()), "the next one, written, is kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The timer keeps a node's outputs exactly when their JSON is at most `MAX_RECORDED_BYTES`,
/// measured without writing it (interpreter-path-L4.md, step 2), as writing it out measured it.
#[cfg(test)]
mod measured {
    use super::*;

    /// The bound is never below what the JSON takes, for values of every kind, the worst
    /// escapes and the longest numbers included.
    #[test]
    fn the_bound_is_never_below_the_json() {
        use holon_core::Slot;
        let text = |s: &str| Value::Text(s.into());
        let worst = "\u{1}\u{1f}\"\\\n\t é ‹secret:x› 😀".repeat(3);
        let leaves = vec![
            Value::Number(-2.2250738585072014e-308),
            Value::Number(1.7976931348623157e308),
            Value::Number(0.1),
            Value::Number(-0.0),
            Value::Boolean(true),
            text(""),
            text(&worst),
        ];
        let mut values = leaves.clone();
        values.push(Value::List(leaves.clone()));
        values.push(Value::List(vec![]));
        values.push(Value::Record(leaves.iter().enumerate().map(|(i, v)| (format!("{worst}{i}"), v.clone())).collect()));
        values.push(Value::Record(BTreeMap::new()));
        values.push(Value::Effect { sink: worst.clone(), payload: Box::new(Value::List(leaves.clone())) });
        values.push(Value::Sum { arm: usize::MAX, payload: Box::new(text(&worst)) });
        let nested = Value::List(values.clone());
        values.push(Value::Record(BTreeMap::from([(worst.clone(), nested.clone())])));
        values.push(Value::Sum { arm: 0, payload: Box::new(nested) });
        for v in &values {
            let slots = [Slot::Value(v.clone()), Slot::Absent, Slot::Pending];
            let json = serde_json::to_vec(&slots.iter().map(|o| o.value().map(SValue::from_core)).collect::<Vec<_>>()).unwrap().len();
            match outputs_bound(&slots) {
                Some(bound) => assert!(bound >= json, "{bound} < {json} for {v:?}"),
                None => assert!(json > Timer::MAX_RECORDED_BYTES / 7, "given up on {json} bytes"),
            }
        }
        // A number that isn't finite: no bound, so counting its JSON decides.
        assert_eq!(outputs_bound(&[Slot::Value(Value::Number(f64::INFINITY))]), None);
    }

    /// Kept exactly when the JSON fits, through the timer itself, across the limit.
    #[test]
    fn the_timer_keeps_outputs_exactly_when_their_json_fits() {
        use holon_core::{Observer, Slot};
        let most = Timer::MAX_RECORDED_BYTES;
        for n in (most - 60..most + 10).chain([0, 1, 100, most / 6, most / 5, 3 * most]) {
            for s in ["x".repeat(n), "\n".repeat(n / 2)] {
                let slots = [Slot::Value(Value::Text(s.clone()))];
                let mut timer = Timer::new();
                timer.enter(&[0]);
                timer.produced(&[0], &slots);
                timer.leave(&[0]);
                let json = serde_json::to_vec(&vec![Some(SValue::Text(s))]).unwrap().len();
                assert_eq!(timer.timeline[0].outputs.is_some(), json <= most, "{json} bytes");
            }
        }
        // A number that isn't finite is written as `null`, and kept, as writing it out found.
        let mut timer = Timer::new();
        timer.enter(&[0]);
        timer.produced(&[0], &[Slot::Value(Value::Number(f64::NAN))]);
        timer.leave(&[0]);
        let json = serde_json::to_vec(&vec![Some(SValue::Number(f64::NAN))]).unwrap().len();
        assert_eq!(timer.timeline[0].outputs.is_some(), json <= most);
    }

    #[test]
    fn outputs_are_kept_exactly_when_their_json_fits() {
        let most = Timer::MAX_RECORDED_BYTES;
        for n in (most - 40..most + 40).chain([0, 1, 100, 3 * most]) {
            let outputs = vec![Some(SValue::Text("x".repeat(n))), None];
            let fits = serde_json::to_writer(Counted { n: 0, most }, &outputs).is_ok();
            assert_eq!(fits, serde_json::to_vec(&outputs).unwrap().len() <= most, "{n} bytes of text");
        }
    }
}
