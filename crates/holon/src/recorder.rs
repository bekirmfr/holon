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
    /// Every run it keeps, oldest first, by its seq and when it ran: in memory are only the newest
    /// `IN_MEMORY` of them (`entries`) when it has a file; the rest are read from it when asked.
    kept: VecDeque<(u64, u64)>,
    /// The length of the line each entry in memory was written as, in step with `entries`: what
    /// the host's view counts a log's memory by (storage.rs; an entry takes 1.1 to 3.6 times its
    /// line in memory, measured by `measure_log_memory`).
    entry_lines: VecDeque<usize>,
    /// Their sum.
    entry_bytes: u64,
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
    /// Whether a cut-back is done outside the log's lock (`LogCell`, docs/L4s/resilience-audit-L4.md
    /// R4): a run that finds the file past what it keeps appends its line and leaves the cut to
    /// be made with the lock released. Off for a log used on its own.
    defer_cuts: bool,
    /// A run found the file past what it keeps: a cut-back is to be made (`LogCell::cut_if_planned`).
    cut_wanted: bool,
    /// A cut-back is being made outside the lock: no other is begun meanwhile.
    cutting: bool,
    /// Counts the times the file was rewritten here (cut back holding the lock): a cut planned
    /// before one is stale.
    generation: u64,
    /// Where the runs a cut-back lets go of are written first (`[storage] backup`, storage.rs),
    /// and whether a cut may go on without (a full disk rolling: the admin chose room over
    /// history). None: they are let go of.
    backup: Option<(PathBuf, bool)>,
    /// Why the last cut-back couldn't write them there (and so didn't cut), if it couldn't.
    backup_error: Option<String>,
    /// Its sealed segments, oldest first: the runs it keeps older than the newest `hot`.
    sealed: Vec<Sealed>,
    /// How many of its newest runs it keeps in its file as written (`HOT`).
    hot: usize,
}

/// A cut-back to be made with the log's lock released: the file as it was when it was planned
/// (its length, and where each line is), and what the log keeps.
#[derive(Debug)]
struct CutPlan {
    path: PathBuf,
    backup: Option<(PathBuf, bool)>,
    sealed: Vec<Sealed>,
    hot: usize,
    generation: u64,
    file_len: u64,
    lines: VecDeque<LineAt>,
    keep: usize,
    cutoff: Option<u64>,
}

/// A cut-back made but not yet swapped in: the lines kept, written and synced to `tmp`, with their
/// index, as of the file's first `file_len` bytes.
struct CutMade {
    path: PathBuf,
    generation: u64,
    tmp: PathBuf,
    /// The file's length when the cut was planned: its lines from here on follow the kept ones.
    file_len: u64,
    /// How much of the file is in `tmp` already: the kept lines, and what was caught up since.
    copied: u64,
    len: u64,
    index: VecDeque<LineAt>,
    /// The oldest line kept, when lines before it were let go (the witness hears of it).
    trimmed: Option<(u64, String)>,
    oldest_ms: u64,
    /// The runs it seals: a segment written aside, to be renamed in before the file is replaced.
    seal: Option<(PathBuf, PathBuf, Sealed)>,
    /// The segments none of whose runs it keeps, to go once the file is replaced.
    drop: Vec<PathBuf>,
}

impl CutMade {
    /// Copy the file's bytes from what is in `tmp` already up to `upto` (lines the log wrote since,
    /// which nothing changes once written) after the kept ones, and sync them: with the lock
    /// released, so what is left for the swap is small.
    fn catch_up(&mut self, upto: u64) -> std::io::Result<()> {
        use std::io::{Read, Seek, Write};
        if upto <= self.copied {
            return Ok(());
        }
        let mut file = std::fs::File::open(&self.path)?;
        file.seek(std::io::SeekFrom::Start(self.copied))?;
        let mut tail = Vec::with_capacity((upto - self.copied) as usize);
        file.take(upto - self.copied).read_to_end(&mut tail)?;
        if tail.len() as u64 != upto - self.copied {
            return Err(std::io::Error::other("the file is shorter than its log wrote it"));
        }
        let mut tmp = std::fs::OpenOptions::new().append(true).open(&self.tmp)?;
        tmp.write_all(&tail)?;
        tmp.sync_data()?;
        self.copied = upto;
        Ok(())
    }
}

impl CutPlan {
    /// The kept lines written beside the file and synced, the lock released: what `cut_back`
    /// keeps (the newest `keep`, none older than the cutoff, always the newest line), byte for
    /// byte. None when there is nothing to cut; an error when the file isn't as planned or can't
    /// be written.
    fn make(self) -> std::io::Result<Option<CutMade>> {
        use std::io::Read;
        let mut bytes = Vec::with_capacity(self.file_len as usize);
        std::fs::File::open(&self.path)?.take(self.file_len).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != self.file_len {
            return Err(std::io::Error::other("the file is shorter than when its cut was planned"));
        }
        let in_file = |at: &LineAt| bytes.get(at.offset as usize..at.offset as usize + at.len);
        let lines: Vec<(&[u8], &LineAt)> = self.lines.iter().filter_map(|at| in_file(at).map(|b| (b, at))).collect();
        let hot_keep = self.keep.min(self.hot);
        let split = split(&self.sealed, &lines, self.keep, hot_keep, self.cutoff);
        if split.seal.is_empty() && split.gone.is_empty() && split.drop.is_empty() {
            return Ok(None);
        }
        // What it lets go of, written to the backup first: if it can't be, the cut fails, and the
        // cut made holding the lock instead finds the same and doesn't cut (`cut_back`).
        let gone: Vec<&(&[u8], &LineAt)> = split.gone.iter().map(|i| &lines[*i]).collect();
        keep_aside(self.backup.as_ref(), &self.path, &gone)?;
        let dropped: Vec<&Sealed> = split.drop.iter().map(|j| &self.sealed[*j]).collect();
        keep_segments_aside(self.backup.as_ref(), &self.path, &dropped)?;
        // What it keeps older than the newest `hot`, sealed beside the file.
        let to_seal: Vec<&(&[u8], &LineAt)> = split.seal.iter().map(|i| &lines[*i]).collect();
        let seal = if to_seal.is_empty() { None } else { Some(write_segment(&self.path, &to_seal)?) };
        let kept: Vec<&(&[u8], &LineAt)> = split.stay.iter().map(|i| &lines[*i]).collect();
        let mut all = Vec::with_capacity(kept.iter().map(|(b, _)| b.len() + 1).sum());
        let mut index = VecDeque::with_capacity(kept.len());
        for (b, at) in &kept {
            index.push_back(LineAt { offset: all.len() as u64, ..(*at).clone() });
            all.extend_from_slice(b);
            all.push(b'\n');
        }
        let (trimmed, oldest_ms) = oldest_kept(&self.sealed, &split, seal.as_ref().map(|s| &s.2), &kept, !split.gone.is_empty() || !split.drop.is_empty());
        let tmp = crate::sealing::write_aside(&self.path, &all)?;
        let drop = dropped.iter().map(|s| s.path.clone()).collect();
        Ok(Some(CutMade { path: self.path, generation: self.generation, tmp, file_len: self.file_len, copied: self.file_len, len: all.len() as u64, index, trimmed, oldest_ms, seal, drop }))
    }
}

/// Write the lines a cut-back lets go of (`gone`, the file at `path`'s) to `backup`, when there is
/// one: `<backup>/<slug>/holon-archive-<first seq>-<last seq>.jsonl`, written whole then renamed in, so the same
/// cut made again writes the same file; the housekeeping turns it into an export `holon verify`
/// checks, gzipped (storage.rs). An error if it can't be written, unless the cut may go on
/// without (`backup`'s flag).
fn keep_aside(backup: Option<&(PathBuf, bool)>, path: &Path, gone: &[&(&[u8], &LineAt)]) -> std::io::Result<()> {
    let (Some((dir, may_drop)), Some((_, first)), Some((_, last))) = (backup, gone.first(), gone.last()) else { return Ok(()) };
    let written = (|| {
        let slug = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| std::io::Error::other("a log file without a name"))?;
        let dir = dir.join(slug);
        std::fs::create_dir_all(&dir)?;
        let mut bytes = Vec::with_capacity(gone.iter().map(|(b, _)| b.len() + 1).sum());
        for (b, _) in gone {
            bytes.extend_from_slice(b);
            bytes.push(b'\n');
        }
        holon_host::write_atomically(&dir.join(crate::storage::batch_name(first.seq, last.seq)), &bytes)
    })();
    match written {
        Err(e) if !*may_drop => Err(std::io::Error::other(format!("the runs it lets go of couldn't be written to the backup {}: {e}", dir.display()))),
        _ => Ok(()),
    }
}

/// After a cut that `split` made (of `sealed` then the file's lines, `seal` the segment it seals,
/// `stay` the lines left in the file): the oldest run it keeps, when runs before it went (`went`)
/// and it names one before it (the witness hears of it, as a `trim`), and when it ran.
fn oldest_kept(sealed: &[Sealed], split: &Split, seal: Option<&Sealed>, stay: &[&(&[u8], &LineAt)], went: bool) -> (Option<(u64, String)>, u64) {
    let first = sealed
        .iter()
        .enumerate()
        .find(|(j, _)| !split.drop.contains(j))
        .map(|(_, s)| (s.first(), s.first_prev.clone(), s.runs.first().map_or(0, |r| r.1)))
        .or_else(|| seal.map(|s| (s.first(), s.first_prev.clone(), s.runs.first().map_or(0, |r| r.1))))
        .or_else(|| stay.first().map(|(_, at)| (at.seq, at.prev.clone(), at.at_ms)));
    match first {
        Some((seq, prev, at_ms)) => ((went && !prev.is_empty()).then_some((seq, prev)), at_ms),
        None => (None, 0),
    }
}

/// `keep_aside` for whole segments: their lines, read back, written to the backup first.
fn keep_segments_aside(backup: Option<&(PathBuf, bool)>, path: &Path, segments: &[&Sealed]) -> std::io::Result<()> {
    if backup.is_none() {
        return Ok(());
    }
    for s in segments {
        let text = match read_segment(&s.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        let ats: Vec<LineAt> = s.runs.iter().map(|(seq, at_ms)| LineAt { offset: 0, len: 0, at_ms: *at_ms, seq: *seq, prev: String::new() }).collect();
        let lines: Vec<(&[u8], &LineAt)> = text.lines().map(str::as_bytes).zip(&ats).collect();
        let refs: Vec<&(&[u8], &LineAt)> = lines.iter().collect();
        keep_aside(backup, path, &refs)?;
    }
    Ok(())
}

/// How often the logs' files are synced to disk (`sync_logs`): what a power loss can lose of them,
/// at most (docs/L4s/request-path-L4.md, C2 d).
pub const SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// Sync the directory holding `path`, so a file just renamed into it stays renamed after a crash
/// (where the system has directories to sync; a failure is only said: the file itself is synced).
fn sync_dir_of(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::File::open(dir).and_then(|d| d.sync_all())
    {
        eprintln!("holon: could not sync {}: {e}", dir.display());
    }
    #[cfg(not(unix))]
    let _ = path;
}

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
        // And its file cut back, if a run found it past what it keeps.
        log.cut_if_planned();
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
fn lines_of(text: &str) -> impl Iterator<Item = (&str, Option<LogEntry>, u64)> {
    let mut offset = 0u64;
    // One line parsed at a time: a log opened holds one entry more than it keeps, not its file's.
    text.split_inclusive('\n').map(move |raw| {
        let at = offset;
        offset += raw.len() as u64;
        let line = raw.strip_suffix('\n').map_or(raw, |l| l.strip_suffix('\r').unwrap_or(l));
        (line, serde_json::from_str::<LogEntry>(line).ok(), at)
    })
}

/// How many of its newest runs a log keeps in its file as they were written; the older ones it
/// keeps are sealed, gzipped, in segments beside it (`<slug>.sealed/<first>-<last>.jsonl.gz`),
/// each cut-back sealing what has left the newest (docs/L4s/storage-L4.md, step 9). A log keeping
/// no more than this seals nothing.
pub(crate) const HOT: usize = 128;

/// One sealed segment of a log: the runs in it, in order (seq, when it ran), the hash its first
/// line names as the one before it (what a witness hears when the runs before it go), its size.
#[derive(Debug, Clone)]
struct Sealed {
    path: PathBuf,
    runs: Vec<(u64, u64)>,
    first_prev: String,
    size: u64,
}

impl Sealed {
    fn first(&self) -> u64 {
        self.runs.first().map_or(0, |r| r.0)
    }
    fn last(&self) -> u64 {
        self.runs.last().map_or(0, |r| r.0)
    }
}

/// Where a log's sealed segments are: beside its file, `<slug>.sealed/`.
fn sealed_dir(file: &Path) -> PathBuf {
    file.with_extension("sealed")
}

/// A sealed segment's lines, as written.
fn read_segment(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&bytes[..]), &mut text)?;
    Ok(text)
}

/// `lines` gzipped into a segment beside the file at `file`, written aside and synced (renamed in
/// by whoever made it, `seal_in`): where it is to go, where it is now, and what it holds.
fn write_segment(file: &Path, lines: &[&(&[u8], &LineAt)]) -> std::io::Result<(PathBuf, PathBuf, Sealed)> {
    let (Some((_, first)), Some((_, last))) = (lines.first(), lines.last()) else { return Err(std::io::Error::other("an empty segment")) };
    let dir = sealed_dir(file);
    std::fs::create_dir_all(&dir)?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    for (b, _) in lines {
        std::io::Write::write_all(&mut gz, b)?;
        std::io::Write::write_all(&mut gz, b"\n")?;
    }
    let bytes = gz.finish()?;
    let path = dir.join(format!("{:020}-{:020}.jsonl.gz", first.seq, last.seq));
    let aside = crate::sealing::write_aside(&path, &bytes)?;
    let sealed = Sealed { path: path.clone(), runs: lines.iter().map(|(_, at)| (at.seq, at.at_ms)).collect(), first_prev: first.prev.clone(), size: bytes.len() as u64 };
    Ok((path, aside, sealed))
}

/// How a cut-back splits a log, its sealed segments (`sealed`) then its file's lines (`hot`), by
/// what it keeps (the newest `keep`, none older than `cutoff`, always the newest line): the file's
/// lines that stay in it (its newest `hot_keep` it keeps), those to seal (the rest it keeps), those
/// that go; and the segments that go (none of whose runs it keeps). Indexes into each.
#[derive(Debug, Default, PartialEq)]
struct Split {
    stay: Vec<usize>,
    seal: Vec<usize>,
    gone: Vec<usize>,
    drop: Vec<usize>,
}

fn split(sealed: &[Sealed], hot: &[(&[u8], &LineAt)], keep: usize, hot_keep: usize, cutoff: Option<u64>) -> Split {
    let in_sealed: usize = sealed.iter().map(|s| s.runs.len()).sum();
    let total = in_sealed + hot.len();
    let (start, newest) = (total.saturating_sub(keep), total.saturating_sub(1));
    let kept = |i: usize, at_ms: u64| i == newest || (i >= start && cutoff.is_none_or(|c| at_ms >= c));
    let mut out = Split::default();
    let mut base = 0;
    for (j, s) in sealed.iter().enumerate() {
        if !s.runs.iter().enumerate().any(|(k, (_, at))| kept(base + k, *at)) {
            out.drop.push(j);
        }
        base += s.runs.len();
    }
    let kept_hot: Vec<usize> = (0..hot.len()).filter(|i| kept(in_sealed + i, hot[*i].1.at_ms)).collect();
    out.gone = (0..hot.len()).filter(|i| !kept_hot.contains(i)).collect();
    let stays_from = kept_hot.len().saturating_sub(hot_keep);
    out.seal = kept_hot[..stays_from].to_vec();
    out.stay = kept_hot[stays_from..].to_vec();
    out
}

/// How many of its newest runs a log on file keeps in memory, whole: what a runs list shows first
/// and a view polling for new runs asks for. The others it keeps are read from its file when asked
/// (docs/L4s/resilience-audit-L4.md, R8: kept whole, a log's runs took 1.1 to 3.6 times their
/// lines in memory, bounded by no cache's size).
pub(crate) const IN_MEMORY: usize = 32;

/// What a read of a log's runs needs, planned under its lock (`Log::plan`) and read with it
/// released (`Wanted::read`): the runs it keeps in memory, cloned, and where in its file the
/// older ones are, the file opened. A file is only ever replaced whole, renamed over, so the one
/// opened holds those lines as they were whatever is written or cut meanwhile.
pub struct Wanted {
    memory: Vec<LogEntry>,
    file: Option<(std::fs::File, Vec<At>)>,
    /// The older runs in sealed segments: each segment, and each run's place in it and seq.
    sealed: Vec<(PathBuf, Vec<(usize, u64)>)>,
}

/// Where a run's line is in a log's file: its offset, its length, and the run's seq.
type At = (u64, usize, u64);

impl Wanted {
    /// The runs, oldest first: those read from the file (each checked to be the run it should be:
    /// a line that isn't is left out), then those from memory.
    pub fn read(self) -> Vec<LogEntry> {
        self.read_if(|_| true)
    }

    /// As `read`, parsing only the file's lines `keep` says may hold what is looked for.
    fn read_if(self, keep: impl Fn(&str) -> bool) -> Vec<LogEntry> {
        use std::io::{Read, Seek, SeekFrom};
        let mut out = Vec::new();
        for (path, picks) in self.sealed {
            // A segment let go of since: its runs are no longer kept.
            let Ok(text) = read_segment(&path) else { continue };
            let lines: Vec<&str> = text.lines().collect();
            for (i, seq) in picks {
                let Some(line) = lines.get(i) else { continue };
                if !keep(line) {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<LogEntry>(line)
                    && entry.seq == seq
                {
                    out.push(entry);
                }
            }
        }
        if let Some((mut file, at)) = self.file {
            let mut buf = Vec::new();
            for (offset, len, seq) in at {
                buf.resize(len, 0);
                if file.seek(SeekFrom::Start(offset)).is_err() || file.read_exact(&mut buf).is_err() {
                    continue;
                }
                let Ok(line) = std::str::from_utf8(&buf) else { continue };
                if !keep(line) {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<LogEntry>(line)
                    && entry.seq == seq
                {
                    out.push(entry);
                }
            }
        }
        out.extend(self.memory);
        out
    }
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
    pub fn new(mut log: Log) -> LogCell {
        log.defer_cuts = true;
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

    /// Each of the log's lines, oldest first, as `Log::lines` has them, given to `f`: a file's read
    /// with the lock released, a line at a time, so no run waits for it and no more than a line is
    /// held (docs/L4s/resilience-audit-L4.md, R5). A file is only ever replaced whole (renamed
    /// over, by a cut-back), so what is read is one file's lines; the newest may be one being
    /// written, cut short, which `f` finds isn't whole. A log kept in memory only is read under
    /// the lock.
    pub fn each_line(&self, mut f: impl FnMut(&str)) {
        use std::io::BufRead;
        // Its file opened and its segments named under the lock, so they agree: a cut sealing lines
        // after this leaves them in the file opened, and one letting a segment go drops runs no
        // longer kept.
        let (file, segments) = self.with(|log| (log.file.as_ref().map(std::fs::File::open), log.sealed.iter().map(|s| s.path.clone()).collect::<Vec<_>>()));
        let Some(file) = file else {
            let lines: Vec<String> = self.with(|log| log.entries.iter().filter_map(|e| serde_json::to_string(e).ok()).collect());
            return lines.iter().for_each(|l| f(l));
        };
        for p in segments {
            if let Ok(text) = read_segment(&p) {
                text.lines().for_each(&mut f);
            }
        }
        let Ok(file) = file else { return };
        let mut reader = std::io::BufReader::with_capacity(64 << 10, file);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let bytes = line.strip_suffix(b"\n").unwrap_or(&line);
            if let Ok(text) = std::str::from_utf8(bytes.strip_suffix(b"\r").unwrap_or(bytes)) {
                f(text);
            }
        }
    }

    /// The runs it keeps after `after`, the newest `limit` of them, oldest first (`Log::after`):
    /// planned under the lock, read from the file with it released, so a page reading old runs
    /// holds up none of the deployment's runs.
    pub fn after(&self, after: Option<u64>, limit: usize) -> Vec<LogEntry> {
        self.with(|log| log.plan(after, limit)).read()
    }

    /// Whether some run it keeps had `text` in cell `key` before it ran: a collection root this
    /// deployment's log has seen. Its file read with the lock released.
    pub fn shows(&self, key: &str, text: &str) -> bool {
        shows_in(self.with(|log| log.plan(None, usize::MAX)), key, text)
    }

    /// The log's lines that hold `needle`, oldest first (`each_line`).
    pub fn lines_holding(&self, needle: &str) -> Vec<String> {
        let mut held = Vec::new();
        self.each_line(|l| {
            if l.contains(needle) {
                held.push(l.to_string());
            }
        });
        held
    }

    /// Make the cut-back a run left, if one did: with the lock released, then swapped in under it
    /// (docs/L4s/resilience-audit-L4.md, R4). Called by the logs' periodic sync (`sync_logs`), so
    /// no run waits for it, its own included: until then the file holds past twice what it keeps,
    /// by what a second brings.
    pub fn cut_if_planned(&self) {
        let Some(plan) = self.log.lock().unwrap().plan_cut() else { return };
        let generation = plan.generation;
        let mut made = plan.make();
        // Caught up with what runs write meanwhile, with the lock released, until what is left for
        // the swap is small (or the file changed under the cut, which the swap then finds).
        if let Ok(Some(cut)) = &mut made {
            for _ in 0..8 {
                let Some(upto) = self.log.lock().unwrap().cut_still_good(generation) else { break };
                if upto.saturating_sub(cut.copied) <= 64 << 10 {
                    break;
                }
                if let Err(e) = cut.catch_up(upto) {
                    made = Err(e);
                    break;
                }
            }
        }
        self.log.lock().unwrap().finish_cut(made);
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
            kept: VecDeque::new(),
            entry_lines: VecDeque::new(),
            entry_bytes: 0,
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
            defer_cuts: false,
            cut_wanted: false,
            cutting: false,
            generation: 0,
            backup: None,
            backup_error: None,
            sealed: Vec::new(),
            hot: HOT,
        };
        let Some(path) = log.file.clone() else { return log };
        let mut chained = false;
        // Its sealed segments first, oldest first, each read as the file's lines are; what a cut
        // that crashed left aside, removed.
        let dir = sealed_dir(&path);
        let mut segments: Vec<PathBuf> = Vec::new();
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".jsonl.gz") {
                segments.push(p);
            } else if name.contains(".cut-") {
                let _ = std::fs::remove_file(&p);
            }
        }
        segments.sort();
        for p in segments {
            let text = match read_segment(&p) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("holon: the sealed segment {} can't be read, its runs are missing from the log: {e}", p.display());
                    continue;
                }
            };
            let size = std::fs::metadata(&p).map_or(0, |m| m.len());
            let mut runs = Vec::new();
            let mut first_prev = None;
            for (line, entry, _) in lines_of(&text) {
                if let Some(entry) = entry {
                    if entry.seq < log.next_seq {
                        continue;
                    }
                    first_prev.get_or_insert_with(|| entry.prev.clone());
                    runs.push((entry.seq, entry.at_ms));
                    log.read_line(line, entry, &mut chained);
                }
            }
            if !runs.is_empty() {
                log.sealed.push(Sealed { path: p, runs, first_prev: first_prev.unwrap_or_default(), size });
            }
        }
        let Ok(text) = std::fs::read_to_string(&path) else { return log.opened() };
        log.cut = !text.is_empty() && !text.ends_with('\n');
        log.file_len = text.len() as u64;
        let sealed_up_to = log.next_seq;
        let mut twice = false;
        for (line, entry, offset) in lines_of(&text) {
            log.written += 1;
            // A line a crash cut short is skipped, not fatal: the others still replay, and the
            // chain goes on from the line before it.
            if let Some(entry) = entry {
                // Sealed already: a cut that crashed between sealing it and replacing the file.
                if entry.seq < sealed_up_to {
                    twice = true;
                    continue;
                }
                log.lines.push_back(LineAt { offset, len: line.len(), at_ms: entry.at_ms, seq: entry.seq, prev: entry.prev.clone() });
                if (chained || !entry.prev.is_empty()) && !log.head.is_empty() && entry.prev != log.head {
                    log.breaks.push(entry.seq);
                }
                chained |= !entry.prev.is_empty();
                if log.kept.is_empty() {
                    log.file_oldest_ms = entry.at_ms;
                }
                log.head = line_hash(line);
                log.next_seq = log.next_seq.max(entry.seq + 1);
                log.kept.push_back((entry.seq, entry.at_ms));
                log.entries.push_back(entry);
                log.entry_lines.push_back(line.len());
                log.entry_bytes += line.len() as u64;
                // Only the newest few stay in memory as it reads them.
                if log.entries.len() > IN_MEMORY {
                    log.entries.pop_front();
                    log.entry_bytes -= log.entry_lines.pop_front().unwrap_or(0) as u64;
                }
            }
        }
        if twice {
            // Its file written again without the lines sealed already.
            log.cut_wanted = false;
            if let Err(e) = log.rewrite_file() {
                eprintln!("holon: could not write the flow log {path:?} again without the runs sealed already: {e}");
            }
        }
        log.opened()
    }

    /// The last of opening it: what it keeps pruned, the file cut back when it is past its age; and
    /// a file holding more than a cut-back leaves it (one written before logs sealed their older
    /// runs, or a log not written to since) cut back by the next sync, so an idle log seals too.
    fn opened(mut self) -> Log {
        self.prune(now_ms());
        self.cut_back_if_aged();
        if self.written > 2 * self.keep.min(self.hot) {
            self.want_cut();
        }
        self
    }

    /// Take in one of its lines read as it opens (a sealed segment's): the chain checked, the run
    /// numbered and kept, the newest few kept in memory.
    fn read_line(&mut self, line: &str, entry: LogEntry, chained: &mut bool) {
        if (*chained || !entry.prev.is_empty()) && !self.head.is_empty() && entry.prev != self.head {
            self.breaks.push(entry.seq);
        }
        *chained |= !entry.prev.is_empty();
        if self.kept.is_empty() {
            self.file_oldest_ms = entry.at_ms;
        }
        self.head = line_hash(line);
        self.next_seq = self.next_seq.max(entry.seq + 1);
        self.kept.push_back((entry.seq, entry.at_ms));
        self.entries.push_back(entry);
        self.entry_lines.push_back(line.len());
        self.entry_bytes += line.len() as u64;
        if self.entries.len() > IN_MEMORY {
            self.entries.pop_front();
            self.entry_bytes -= self.entry_lines.pop_front().unwrap_or(0) as u64;
        }
    }

    /// Write its file again as its index says it is (without lines it skipped reading).
    fn rewrite_file(&mut self) -> std::io::Result<()> {
        let Some(path) = self.file.clone() else { return Ok(()) };
        let bytes = std::fs::read(&path)?;
        let mut all = Vec::with_capacity(bytes.len());
        let mut index = VecDeque::with_capacity(self.lines.len());
        for at in &self.lines {
            let Some(b) = bytes.get(at.offset as usize..at.offset as usize + at.len) else { continue };
            index.push_back(LineAt { offset: all.len() as u64, ..at.clone() });
            all.extend_from_slice(b);
            all.push(b'\n');
        }
        self.handle = None;
        crate::sealing::write_private(&path, &all)?;
        sync_dir_of(&path);
        self.written = index.len();
        self.file_len = all.len() as u64;
        self.lines = index;
        self.cut = false;
        self.generation += 1;
        Ok(())
    }

    /// Keep `hot` of its newest runs in its file as written (`HOT`), for a test.
    #[cfg(test)]
    pub fn set_hot(&mut self, hot: usize) {
        self.hot = hot.max(1);
    }

    /// Its sealed segments: how many, how many runs in them, their bytes on disk.
    pub fn sealed_holds(&self) -> (usize, usize, u64) {
        (self.sealed.len(), self.sealed.iter().map(|s| s.runs.len()).sum(), self.sealed.iter().map(|s| s.size).sum())
    }

    /// Where the runs it lets go of are written first, and whether a cut may go on without
    /// (storage.rs keeps it as `[storage] backup` says).
    pub fn set_backup(&mut self, backup: Option<(PathBuf, bool)>) {
        self.backup = backup;
    }

    /// Why the last cut-back couldn't write what it let go of to the backup, if it couldn't.
    pub fn backup_error(&self) -> Option<&str> {
        self.backup_error.as_deref()
    }

    /// How old a run it keeps, at most, in milliseconds (none: any age).
    pub fn max_age_ms(&self) -> Option<u64> {
        self.max_age_ms
    }

    /// Ask for its file to be cut back to what it keeps, as a run finding it full does: by the
    /// periodic sync, with the lock released (R4). The host, freeing disk (storage.rs).
    pub fn want_cut(&mut self) {
        if self.file.is_some() {
            self.cut_wanted = true;
        }
    }

    /// Keep `keep` entries from now on, none older than `max_age_ms` (none: any age): the
    /// deployment's retention, set at deploy and load and when its grant changes, and by the host
    /// rolling a full disk. Keeping more than it did keeps again what its segments and file still
    /// hold.
    pub fn retain(&mut self, keep: usize, max_age_ms: Option<u64>) {
        let keep = keep.max(1);
        if (keep, max_age_ms) == (self.keep, self.max_age_ms) {
            return;
        }
        let older = match (max_age_ms, self.max_age_ms) {
            (None, Some(_)) => true,
            (Some(new), Some(old)) => new > old,
            _ => false,
        };
        if (keep > self.keep || older) && self.file.is_some() {
            // Keeping more: the runs its segments and file still hold, kept again, from the index
            // it has of them (each run's seq and when it ran), with the file and every flag as
            // they are: nothing read, and nothing it was doing forgotten (whether its newest lines
            // are synced, those that couldn't be written, its backup).
            let mut runs: BTreeMap<u64, u64> = self.sealed.iter().flat_map(|s| s.runs.iter().copied()).collect();
            runs.extend(self.lines.iter().map(|l| (l.seq, l.at_ms)));
            runs.extend(self.kept.iter().copied());
            self.kept = runs.into_iter().collect();
        }
        (self.keep, self.max_age_ms) = (keep, max_age_ms);
        self.prune(now_ms());
        self.cut_back_if_aged();
    }

    /// Drop the entries past what it keeps: the oldest past `keep`, and any older than its age.
    /// Only the entries kept count for breaks (the oldest has nothing before it to check).
    fn prune(&mut self, now_ms: u64) {
        let cutoff = self.max_age_ms.map(|age| now_ms.saturating_sub(age));
        while self.kept.len() > self.keep || self.kept.front().is_some_and(|(_, at)| cutoff.is_some_and(|c| *at < c)) {
            self.kept.pop_front();
        }
        // In memory: what it keeps, and with a file only the newest few of it.
        let oldest = self.kept.front().map_or(u64::MAX, |(seq, _)| *seq);
        let most = if self.file.is_some() { IN_MEMORY } else { usize::MAX };
        while self.entries.front().is_some_and(|e| e.seq < oldest) || self.entries.len() > most {
            if let Some(n) = self.entry_lines.pop_front() {
                self.entry_bytes -= n as u64;
            }
            if let Some(old) = self.entries.pop_front() {
                // Freed after the lock, unless nobody frees them (a log used without its cell).
                if self.gone.len() < 64 {
                    self.gone.push(old);
                }
            }
        }
        // The breaks of runs still in its segments or file are kept, for keeping more again.
        let on_disk = self.sealed.first().map(Sealed::first).or_else(|| self.lines.front().map(|l| l.seq)).map_or(oldest, |s| s.min(oldest));
        self.breaks.retain(|seq| *seq >= on_disk);
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
        self.kept.push_back((entry.seq, entry.at_ms));
        self.entries.push_back(entry);
        self.entry_lines.push_back(line.len());
        self.entry_bytes += line.len() as u64;
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
        let past = self.written + 1 > 2 * self.keep.min(self.hot) || self.file_aged(now_ms());
        // Cut holding the lock (a log used on its own); one that couldn't keep what it lets go of
        // aside didn't cut, and the line is appended as any is.
        if past && !self.defer_cuts && self.cut_back(Some((line, at.clone())))? {
            return Ok(());
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
        // Past what it keeps: a cut is wanted, to be made with the lock released, the line already
        // in the file (R4).
        if past {
            self.cut_wanted = true;
        }
        Ok(())
    }

    /// The cut-back wanted, as the file is now, to be made with the lock released: none if none is
    /// wanted, one is being made, or the log keeps no file.
    fn plan_cut(&mut self) -> Option<CutPlan> {
        if !self.cut_wanted || self.cutting {
            return None;
        }
        let path = self.file.clone()?;
        self.cut_wanted = false;
        self.cutting = true;
        let cutoff = self.max_age_ms.map(|age| now_ms().saturating_sub(age));
        Some(CutPlan { path, backup: self.backup.clone(), sealed: self.sealed.clone(), hot: self.hot, generation: self.generation, file_len: self.file_len, lines: self.lines.clone(), keep: self.keep, cutoff })
    }

    /// How long the file is, as this log has written it, while the cut planned at `generation` is
    /// still good for it: what a cut-back catching up copies to.
    fn cut_still_good(&self, generation: u64) -> Option<u64> {
        (self.generation == generation && self.cutting).then_some(self.file_len)
    }

    /// Swap in a cut-back made with the lock released (`CutPlan::make`): the lines written since
    /// it was planned copied after the kept ones and synced, then the file replaced. One the file
    /// moved under (changed outside holon), or one that failed, is made here instead, as it always
    /// was.
    fn finish_cut(&mut self, made: std::io::Result<Option<CutMade>>) {
        self.cutting = false;
        let mut made = match made {
            Ok(Some(made)) => made,
            Ok(None) => return,
            Err(e) => {
                eprintln!("holon: the flow log {:?} is cut back holding its lock: {e}", self.file);
                if let Err(e) = self.cut_back(None) {
                    eprintln!("holon: could not cut back the flow log in {:?}: {e}", self.file);
                }
                return;
            }
        };
        // A log no longer kept in that file (deleted, or moved) has nothing to cut.
        if self.file.as_deref() != Some(made.path.as_path()) || !made.path.exists() {
            let _ = std::fs::remove_file(&made.tmp);
            if let Some((_, aside, _)) = &made.seal {
                let _ = std::fs::remove_file(aside);
            }
            return;
        }
        let swapped = (|| -> std::io::Result<bool> {
            // The file as this log left it, or it is read afresh by the cut made here instead.
            if made.generation != self.generation || std::fs::metadata(&made.path)?.len() != self.file_len || self.file_len < made.file_len {
                return Ok(false);
            }
            // What the log wrote since the cut last caught up: small, the rest copied already.
            made.catch_up(self.file_len)?;
            // The runs it seals, in beside the file before the file is replaced: a crash between
            // leaves them in both, which opening the log reads once (`open_retaining`).
            if let Some((path, aside, _)) = &made.seal {
                std::fs::rename(aside, path)?;
                sync_dir_of(path);
            }
            // Closed before the file is replaced: the next line opens the new one.
            self.handle = None;
            std::fs::rename(&made.tmp, &made.path)?;
            // The rename on disk too, so a crash can't bring back the file it replaced, whose
            // newest lines may not have been synced.
            sync_dir_of(&made.path);
            if let Some((_, _, sealed)) = made.seal.take() {
                self.sealed.push(sealed);
            }
            self.drop_segments(&made.drop);
            let shift = made.len as i64 - made.file_len as i64;
            let moved: Vec<LineAt> = self.lines.iter().filter(|at| at.offset >= made.file_len).map(|at| LineAt { offset: (at.offset as i64 + shift) as u64, ..at.clone() }).collect();
            let mut index = std::mem::take(&mut made.index);
            index.extend(moved);
            self.written = index.len();
            self.file_len = made.len + (made.copied - made.file_len);
            self.lines = index;
            self.file_oldest_ms = made.oldest_ms;
            // Everything in it is on disk: the kept lines synced as they were written, the rest now.
            self.dirty = false;
            Ok(true)
        })();
        match swapped {
            Ok(true) => {
                // What it let go of was written to the backup (`CutPlan::make`): it can be again.
                self.backup_error = None;
                if let Some((seq, prev)) = &made.trimmed {
                    self.export("trim", *seq, prev, now_ms());
                }
            }
            other => {
                let _ = std::fs::remove_file(&made.tmp);
                // The segment it sealed, aside or renamed in already: its runs are in the file
                // still, and the cut made here seals them as it does.
                if let Some((path, aside, _)) = &made.seal {
                    let _ = std::fs::remove_file(aside);
                    if !self.sealed.iter().any(|s| s.path == *path) {
                        let _ = std::fs::remove_file(path);
                    }
                }
                if let Err(e) = other {
                    eprintln!("holon: the flow log {:?} is cut back holding its lock: {e}", self.file);
                }
                if let Err(e) = self.cut_back(None) {
                    eprintln!("holon: could not cut back the flow log in {:?}: {e}", self.file);
                }
            }
        }
    }

    /// Rewrite the file as the lines it keeps, byte for byte, then `line` (the newest entry's,
    /// when there is one): the newest `keep`, none older than its age, but always the newest
    /// line, which carries the numbering and the chain on.
    fn cut_back(&mut self, line: Option<(&str, LineAt)>) -> std::io::Result<bool> {
        let Some(path) = self.file.clone() else { return Ok(true) };
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
                .filter_map(|(l, e, offset)| e.map(|e| LineAt { offset, len: l.len(), at_ms: e.at_ms, seq: e.seq, prev: e.prev }))
                .collect();
        }
        // Each line it could keep, oldest first: the file's that parsed, then the newest entry's.
        let in_file = |at: &LineAt| bytes.get(at.offset as usize..at.offset as usize + at.len);
        let mut lines: Vec<(&[u8], &LineAt)> = self.lines.iter().filter_map(|at| in_file(at).map(|b| (b, at))).collect();
        if let Some((line, at)) = &line {
            lines.push((line.as_bytes(), at));
        }
        let split = split(&self.sealed, &lines, self.keep, self.keep.min(self.hot), cutoff);
        if split.seal.is_empty() && split.gone.is_empty() && split.drop.is_empty() && line.is_none() {
            return Ok(true);
        }
        // What it lets go of, written to the backup first; if it can't be, no cut: the file
        // holds them until it can, and the host warns (storage.rs).
        let gone: Vec<&(&[u8], &LineAt)> = split.gone.iter().map(|i| &lines[*i]).collect();
        let dropped: Vec<Sealed> = split.drop.iter().map(|j| self.sealed[*j].clone()).collect();
        let aside = keep_aside(self.backup.as_ref(), &path, &gone).and_then(|()| keep_segments_aside(self.backup.as_ref(), &path, &dropped.iter().collect::<Vec<_>>()));
        match aside {
            Ok(()) => self.backup_error = None,
            Err(e) => {
                // Not cut: the newest line, if one waits, is appended as any line is.
                self.backup_error = Some(e.to_string());
                return Ok(false);
            }
        }
        // What it keeps older than the newest `hot`, sealed beside the file, in before the file
        // is replaced.
        let to_seal: Vec<&(&[u8], &LineAt)> = split.seal.iter().map(|i| &lines[*i]).collect();
        let seal = if to_seal.is_empty() { None } else { Some(write_segment(&path, &to_seal)?) };
        if let Some((final_path, aside, _)) = &seal {
            std::fs::rename(aside, final_path)?;
            sync_dir_of(final_path);
        }
        let kept: Vec<&(&[u8], &LineAt)> = split.stay.iter().map(|i| &lines[*i]).collect();
        let mut all = Vec::with_capacity(kept.iter().map(|(b, _)| b.len() + 1).sum());
        let mut index = VecDeque::with_capacity(kept.len());
        for (b, at) in &kept {
            index.push_back(LineAt { offset: all.len() as u64, ..(*at).clone() });
            all.extend_from_slice(b);
            all.push(b'\n');
        }
        // The witness hears that the runs before the oldest kept are gone by the host's choice.
        let (trimmed, oldest_ms) = oldest_kept(&self.sealed, &split, seal.as_ref().map(|s| &s.2), &kept, !split.gone.is_empty() || !split.drop.is_empty());
        if let Some((seq, prev)) = trimmed {
            self.export("trim", seq, &prev, now_ms());
        }
        self.file_oldest_ms = oldest_ms;
        // Written whole and synced (write_private), the newest line with it. If it can't be, the
        // segment sealed goes again: its runs are in the file still.
        if let Err(e) = crate::sealing::write_private(&path, &all) {
            if let Some((final_path, _, _)) = &seal
                && !self.sealed.iter().any(|s| s.path == *final_path)
            {
                let _ = std::fs::remove_file(final_path);
            }
            return Err(e);
        }
        sync_dir_of(&path);
        self.generation += 1;
        self.written = kept.len();
        self.file_len = all.len() as u64;
        self.lines = index;
        self.cut = false;
        self.dirty = false;
        if let Some((_, _, sealed)) = seal {
            self.sealed.push(sealed);
        }
        self.drop_segments(&dropped.iter().map(|s| s.path.clone()).collect::<Vec<_>>());
        Ok(true)
    }

    /// Let go of the sealed segments at `paths`: none of their runs is kept any more.
    fn drop_segments(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        self.sealed.retain(|s| !paths.contains(&s.path));
        for p in paths {
            let _ = std::fs::remove_file(p);
        }
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
            // Its sealed segments' first, oldest first, then its file's.
            let mut lines: Vec<String> = self.sealed.iter().filter_map(|s| read_segment(&s.path).ok()).flat_map(|t| t.lines().map(str::to_string).collect::<Vec<_>>()).collect();
            lines.extend(std::fs::read_to_string(path).map(|t| t.lines().map(str::to_string).collect::<Vec<_>>()).unwrap_or_default());
            return lines;
        }
        self.entries.iter().filter_map(|e| serde_json::to_string(e).ok()).collect()
    }

    /// What it holds, for the host's view: the runs it keeps, those of them in memory and the
    /// bytes of the lines they were written as; the lines its file holds.
    pub fn holds(&self) -> (usize, usize, u64, usize) {
        (self.kept.len(), self.entries.len(), self.entry_bytes, self.written)
    }

    /// The seqs of the entries kept that don't follow the line before them: the log was changed
    /// outside holon there.
    pub fn breaks(&self) -> &[u64] {
        let oldest = self.kept.front().map_or(u64::MAX, |(seq, _)| *seq);
        &self.breaks[self.breaks.partition_point(|seq| *seq < oldest)..]
    }

    /// Delete the file: its deployment was deleted.
    pub fn delete_file(&mut self) {
        self.handle = None;
        self.lines.clear();
        self.file_len = 0;
        self.generation += 1;
        self.sealed.clear();
        if let Some(path) = &self.file {
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_dir_all(sealed_dir(path));
        }
    }

    /// The runs it keeps after `after` (all of them for `None`), oldest first, at most `limit`
    /// (the newest `limit`, when there are more): those in memory, and the older ones read from
    /// its file. Read here, with whatever lock it is under held; a `LogCell`'s `after` reads them
    /// with its lock released.
    pub fn after(&self, after: Option<u64>, limit: usize) -> Vec<LogEntry> {
        self.plan(after, limit).read()
    }

    /// What reading the runs it keeps after `after`, the newest `limit` of them, needs (`Wanted`).
    pub fn plan(&self, after: Option<u64>, limit: usize) -> Wanted {
        let newer = self.kept.iter().filter(|(seq, _)| after.is_none_or(|a| *seq > a));
        let n = newer.clone().count();
        let from = newer.clone().nth(n.saturating_sub(limit)).map_or(u64::MAX, |(seq, _)| *seq);
        let memory: Vec<LogEntry> = self.entries.iter().filter(|e| e.seq >= from && after.is_none_or(|a| e.seq > a)).cloned().collect();
        let in_memory_from = self.entries.front().map_or(u64::MAX, |e| e.seq);
        let mut older: Vec<u64> = newer.map(|(seq, _)| *seq).filter(|seq| *seq >= from && *seq < in_memory_from).collect();
        // Those sealed, by segment: each its place in it.
        let mut sealed: Vec<(PathBuf, Vec<(usize, u64)>)> = Vec::new();
        older.retain(|seq| {
            let Some(s) = self.sealed.iter().find(|s| s.first() <= *seq && *seq <= s.last()) else { return true };
            if let Ok(i) = s.runs.binary_search_by_key(seq, |r| r.0) {
                match sealed.last_mut() {
                    Some((p, picks)) if *p == s.path => picks.push((i, *seq)),
                    _ => sealed.push((s.path.clone(), vec![(i, *seq)])),
                }
            }
            false
        });
        let file = match (&self.file, older.is_empty()) {
            (Some(path), false) => std::fs::File::open(path).ok().map(|f| {
                let at = older
                    .iter()
                    .filter_map(|seq| {
                        let i = self.lines.binary_search_by_key(seq, |l| l.seq).ok().or_else(|| self.lines.iter().position(|l| l.seq == *seq))?;
                        let l = &self.lines[i];
                        Some((l.offset, l.len, l.seq))
                    })
                    .collect();
                (f, at)
            }),
            _ => None,
        };
        Wanted { memory, file, sealed }
    }

    /// The seq the next entry will get.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

}

/// Whether a run `wanted` reads had `text` in cell `key` before it ran: only the file's lines
/// holding `text` are parsed.
fn shows_in(wanted: Wanted, key: &str, text: &str) -> bool {
    let want = SValue::from_core(&holon_core::Value::Text(text.to_string()));
    wanted.read_if(|line| line.contains(text)).iter().any(|e| e.state_before.get(key) == Some(&want))
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

    /// A cut made with the lock released whose swap fails leaves no segment the log doesn't
    /// know of: the one it sealed goes, and the cut made holding the lock instead seals the runs.
    #[test]
    fn a_cut_whose_swap_fails_leaves_no_segment_behind() {
        let dir = temp_dir("swap-fails");
        let path = dir.join("slug.jsonl");
        let mut log = Log::open_keeping(Some(path.clone()), 10);
        log.set_hot(2);
        log.defer_cuts = true;
        for _ in 0..6 {
            log.push(entry());
        }
        let made = log.plan_cut().expect("a cut wanted").make().unwrap().expect("something to cut");
        assert!(made.seal.is_some());
        // Its file gone from under it, the swap fails once its segment is renamed in; and the log
        // keeping fewer meanwhile, the cut made instead seals other runs.
        std::fs::remove_file(&made.tmp).unwrap();
        log.retain(3, None);
        log.finish_cut(Ok(Some(made)));
        let on_disk: Vec<PathBuf> = {
            let mut p: Vec<PathBuf> = std::fs::read_dir(sealed_dir(&path)).unwrap().flatten().map(|e| e.path()).collect();
            p.sort();
            p
        };
        let known: Vec<PathBuf> = log.sealed.iter().map(|s| s.path.clone()).collect();
        assert_eq!(on_disk, known, "every segment on disk is one the log knows");
        let back = Log::open_keeping(Some(path.clone()), 10);
        assert_eq!(back.after(None, 100).iter().map(|e| e.seq).collect::<Vec<_>>(), [3, 4, 5]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A backup that couldn't be written to is said until one can be: by a cut made with the
    /// lock released as by one made holding it.
    #[test]
    fn a_backup_written_to_again_is_no_longer_an_error() {
        let dir = temp_dir("backup-again");
        let path = dir.join("slug.jsonl");
        let blocked = dir.join("blocked");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&blocked, "a file where the folder would be").unwrap();
        let cell = LogCell::new(Log::open_keeping(Some(path.clone()), 3));
        cell.with(|l| l.set_backup(Some((blocked.clone(), false))));
        let push = |n: usize| {
            for _ in 0..n {
                cell.with(|l| l.push(entry()));
            }
            cell.cut_if_planned();
        };
        push(8);
        assert!(cell.with(|l| l.backup_error().map(str::to_string)).is_some_and(|e| e.contains("backup")), "can't be written to");
        assert!(cell.with(|l| l.holds().3) > 6, "and nothing cut");
        let backup = dir.join("backup");
        cell.with(|l| l.set_backup(Some((backup.clone(), false))));
        push(1);
        assert_eq!(cell.with(|l| l.backup_error().map(str::to_string)), None, "written to again");
        assert_eq!(cell.with(|l| l.holds().3), 3, "cut back");
        assert!(std::fs::read_dir(backup.join("slug")).unwrap().count() > 0, "what it let go of is there");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Keeping fewer runs then more again keeps again what the file holds, and forgets nothing
    /// the log was doing: that its newest line isn't synced yet, its backup, the breaks in what
    /// it keeps again (docs/L4s/storage-L4.md, the review of steps 1 to 10).
    #[test]
    fn keeping_more_again_forgets_nothing_the_log_was_doing() {
        let dir = temp_dir("keep-more");
        let path = dir.join("slug.jsonl");
        let mut log = Log::open_keeping(Some(path.clone()), 10);
        for _ in 0..5 {
            log.push(entry());
        }
        let mut lines: Vec<String> = std::fs::read_to_string(&path).unwrap().lines().map(str::to_string).collect();
        lines[1] = lines[1].replace("\"tick\"", "\"trigger\"");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let mut log = Log::open_keeping(Some(path.clone()), 10);
        assert_eq!(log.breaks(), [2]);
        log.push(entry());
        let backup = Some((dir.join("backup"), false));
        log.set_backup(backup.clone());
        log.retain(2, None);
        assert_eq!(log.breaks(), &[] as &[u64], "the break is before what it keeps");
        log.retain(10, None);
        assert_eq!(log.after(None, 100).iter().map(|e| e.seq).collect::<Vec<_>>(), [0, 1, 2, 3, 4, 5], "kept again from the file");
        assert_eq!(log.breaks(), [2], "and its break with it");
        assert_eq!(log.backup, backup, "its backup kept");
        assert_eq!(log.take_dirty(), Some(path.clone()), "its newest line still to be synced");
        // And by age: none kept past a millisecond, then every one again once any age may be.
        log.retain(10, Some(1));
        assert_eq!(log.after(None, 100).len(), 0);
        log.retain(10, None);
        assert_eq!(log.after(None, 100).len(), 6);
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

    /// A cut-back made with the lock released (R4) leaves the file a cut made holding it leaves,
    /// byte for byte, cut after cut, and a log opened on it reads the chain unbroken.
    #[test]
    fn a_cut_made_outside_the_lock_leaves_the_file_one_made_holding_it_leaves() {
        let dir = temp_dir("cut-outside");
        let (held, cell) = (dir.join("held.jsonl"), dir.join("cell.jsonl"));
        let mut alone = Log::open_keeping(Some(held.clone()), 3);
        let celled = LogCell::new(Log::open_keeping(Some(cell.clone()), 3));
        for i in 0..20 {
            let e = LogEntry { at_ms: 1_000 + i, ..entry() };
            alone.push(e.clone());
            celled.with(|log| log.push(e));
            celled.cut_if_planned();
            assert_eq!(std::fs::read(&cell).unwrap(), std::fs::read(&held).unwrap(), "after entry {i}");
        }
        let back = Log::open_keeping(Some(cell.clone()), 3);
        assert!(back.breaks().is_empty(), "the chain unbroken");
        assert_eq!(back.next_seq(), 20);
        assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains(".cut-")), "no cut left beside it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs logging from several threads while their log's file is cut back outside the lock: every
    /// entry is numbered once, the file holds the newest ones in order, chained, and stays within
    /// twice what the log keeps and the few that came during a cut.
    #[test]
    fn lines_written_while_a_cut_is_made_outside_the_lock_are_all_kept_in_order() {
        let dir = temp_dir("cut-threads");
        let path = dir.join("slug.jsonl");
        let cell = std::sync::Arc::new(LogCell::new(Log::open_keeping(Some(path.clone()), 50)));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let cell = std::sync::Arc::clone(&cell);
                std::thread::spawn(move || {
                    for i in 0..250 {
                        let e = entry();
                        let made = Made::of(&e);
                        cell.with(|log| log.push_made(e, made));
                        // The periodic sync's part, from one of the threads now and then.
                        if i % 25 == 0 {
                            cell.cut_if_planned();
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        cell.cut_if_planned();
        let text = std::fs::read_to_string(&path).unwrap();
        let seqs: Vec<u64> = text.lines().filter_map(|l| serde_json::from_str::<LogEntry>(l).ok()).map(|e| e.seq).collect();
        assert_eq!(seqs.last(), Some(&1999), "the newest entry is the last line");
        assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "in order, none missing between the oldest kept and the newest");
        assert!(seqs.len() >= 50 && seqs.len() <= 2 * 50 + 8 * 25, "{} lines kept", seqs.len());
        let back = Log::open_keeping(Some(path.clone()), 50);
        assert!(back.breaks().is_empty(), "the chain unbroken");
        assert_eq!(back.next_seq(), 2000);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A log on file keeps only its newest runs in memory (R8), and reads the others it keeps from
    /// its file: every read gives what a log keeping them all in memory gives, run for run, as it
    /// cuts back and as the keep changes; what it holds in memory stays bounded.
    #[test]
    fn a_log_on_file_keeps_its_newest_runs_in_memory_and_reads_the_rest_from_its_file() {
        let dir = temp_dir("in-memory");
        let path = dir.join("slug.jsonl");
        let mut on_file = Log::open_keeping(Some(path.clone()), 100);
        let mut in_memory = Log::open_keeping(None, 100);
        let numbered = |i: u64| LogEntry { at_ms: 1_000 + i, state_before: BTreeMap::from([("n".to_string(), SValue::Text(format!("root-{i}")))]), ..entry() };
        // Run for run: each as its line says it.
        let lines = |runs: Vec<LogEntry>| runs.iter().map(|e| serde_json::to_string(e).unwrap()).collect::<Vec<_>>();
        for i in 0..250 {
            on_file.push(numbered(i));
            in_memory.push(numbered(i));
            if i % 37 == 0 {
                for (after, limit) in [(None, usize::MAX), (None, 5), (Some(i.saturating_sub(60)), 10), (Some(i), 3)] {
                    assert_eq!(lines(on_file.after(after, limit)), lines(in_memory.after(after, limit)), "after {after:?}, the newest {limit}, at run {i}");
                }
            }
        }
        let (kept, held, bytes, _) = on_file.holds();
        assert_eq!((kept, held), (100, IN_MEMORY), "it keeps 100, the newest {IN_MEMORY} in memory");
        assert!(bytes < 32 * 2048, "{bytes} bytes of lines in memory");
        assert_eq!(in_memory.holds().1, 100, "a log in memory only keeps them all there");
        assert_eq!(on_file.after(None, usize::MAX).iter().map(|e| e.seq).collect::<Vec<_>>(), (150..250).collect::<Vec<_>>());
        // Opened again, it reads them as it did; told to keep more, it reads more back from its file.
        let mut back = Log::open_keeping(Some(path.clone()), 100);
        assert_eq!(lines(back.after(None, usize::MAX)), lines(in_memory.after(None, usize::MAX)));
        assert_eq!(back.holds().1, IN_MEMORY);
        back.retain(150, None);
        assert!(back.after(None, usize::MAX).len() > 100, "its file holds past what it kept");
        // A state only an old run's line still shows is found in the file.
        let cell = LogCell::new(Log::open_keeping(Some(path.clone()), 100));
        assert!(cell.shows("n", "root-160"), "an old run's state, from the file");
        assert!(cell.shows("n", "root-249"), "the newest, from memory");
        assert!(!cell.shows("n", "root-10"), "one it no longer keeps");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A log keeping more than `hot` runs seals the older ones it keeps, gzipped, beside its file
    /// (storage step 9): it gives what a log keeping them all in memory gives, run for run and line
    /// for line, as it cuts back (with the lock released, or holding it, the same), as segments go
    /// with retention (to the backup first), and opened again.
    #[test]
    fn a_log_seals_its_older_runs_and_reads_them_back_as_it_wrote_them() {
        let dir = temp_dir("sealed");
        let (cell_path, held_path) = (dir.join("cell.jsonl"), dir.join("held.jsonl"));
        let backup = dir.join("backup");
        let cell = LogCell::new(Log::open_keeping(Some(cell_path.clone()), 300));
        cell.with(|l| {
            l.set_hot(20);
            l.set_backup(Some((backup.clone(), false)));
        });
        let mut held = Log::open_keeping(Some(held_path.clone()), 300);
        held.set_hot(20);
        let mut in_memory = Log::open_keeping(None, 300);
        let numbered = |i: u64| LogEntry { at_ms: 1_000 + i, state_before: BTreeMap::from([("n".to_string(), SValue::Text(format!("root-{i}")))]), ..entry() };
        let lines = |runs: Vec<LogEntry>| runs.iter().map(|e| serde_json::to_string(e).unwrap()).collect::<Vec<_>>();
        for i in 0..1000 {
            cell.with(|l| l.push(numbered(i)));
            cell.cut_if_planned();
            held.push(numbered(i));
            in_memory.push(numbered(i));
            if i % 97 == 0 {
                for (after, limit) in [(None, usize::MAX), (None, 5), (Some(i.saturating_sub(250)), 40), (Some(i), 3)] {
                    let want = lines(in_memory.after(after, limit));
                    assert_eq!(lines(cell.after(after, limit)), want, "the cell, after {after:?}, the newest {limit}, at run {i}");
                    assert_eq!(lines(held.after(after, limit)), want, "held, after {after:?}, the newest {limit}, at run {i}");
                }
            }
        }
        let (segments, sealed_runs, sealed_bytes) = cell.with(|l| l.sealed_holds());
        assert!(segments > 1 && sealed_runs >= 300 - 40, "{segments} segments, {sealed_runs} runs");
        let raw: usize = cell.with(|l| l.lines()).iter().take(sealed_runs).map(|l| l.len() + 1).sum();
        assert!((sealed_bytes as usize) * 5 < raw, "gzipped: {sealed_bytes} bytes for {raw}");
        // Every line, as written: the export's and the sweeps' (each_line), the same, and the
        // cut made holding the lock leaves what the one made without it leaves.
        let all = cell.with(|l| l.lines());
        assert_eq!(all, held.lines(), "sealed alike, with the lock released or held");
        let mut seen = Vec::new();
        cell.each_line(|l| seen.push(l.to_string()));
        assert_eq!(seen, all);
        let seqs: Vec<u64> = all.iter().map(|l| serde_json::from_str::<LogEntry>(l).unwrap().seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1) && seqs.last() == Some(&999), "in order, none twice");
        assert!(cell.shows("n", "root-750"), "a sealed run's state, found");
        // What retention let go of went to the backup first, segments too.
        let backed: usize = std::fs::read_dir(backup.join("cell")).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap().lines().count()).sum();
        assert_eq!(backed + seqs.len(), 1000, "every run backed up or still in the log, once");
        // Opened again: the same runs, the chain unbroken.
        let back = Log::open_keeping(Some(cell_path.clone()), 300);
        assert!(back.breaks().is_empty(), "{:?}", back.breaks());
        assert_eq!(back.next_seq(), 1000);
        assert_eq!(lines(back.after(None, usize::MAX)), lines(in_memory.after(None, usize::MAX)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cut that crashed between sealing runs and replacing the file leaves them in both: opening
    /// the log reads each once, and writes its file again without them.
    #[test]
    fn a_run_sealed_and_still_in_the_file_after_a_crash_is_read_once() {
        let dir = temp_dir("sealed-crash");
        let path = dir.join("slug.jsonl");
        let mut log = Log::open_keeping(Some(path.clone()), 100);
        log.set_hot(1000);
        for i in 0..30 {
            log.push(LogEntry { at_ms: 1_000 + i, ..entry() });
        }
        // The crash: the oldest 20 sealed, the file not yet replaced.
        let bytes = std::fs::read(&path).unwrap();
        let first: Vec<(&[u8], LineAt)> = log.lines.iter().take(20).map(|at| (&bytes[at.offset as usize..at.offset as usize + at.len], at.clone())).collect();
        let refs: Vec<(&[u8], &LineAt)> = first.iter().map(|(b, at)| (*b, at)).collect();
        let (to, aside, _) = write_segment(&path, &refs.iter().collect::<Vec<_>>()).unwrap();
        std::fs::rename(aside, to).unwrap();
        let back = Log::open_keeping(Some(path.clone()), 100);
        let seqs: Vec<u64> = back.after(None, usize::MAX).iter().map(|e| e.seq).collect();
        assert_eq!(seqs, (0..30).collect::<Vec<_>>(), "each once");
        assert!(back.breaks().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 10, "the file written again without the 20 sealed");
        assert_eq!(back.lines().len(), 30);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A read is planned with the lock held and read with it released: a cut-back and more lines
    /// coming between them change nothing it reads (the file opened is the one planned on).
    #[test]
    fn a_read_planned_before_a_cut_back_reads_the_runs_it_planned() {
        let dir = temp_dir("planned-read");
        let path = dir.join("slug.jsonl");
        let cell = LogCell::new(Log::open_keeping(Some(path.clone()), 40));
        for i in 0..60 {
            cell.with(|log| log.push(LogEntry { at_ms: 1_000 + i, ..entry() }));
        }
        let planned = cell.with(|log| log.plan(None, usize::MAX));
        // Past twice what it keeps, then cut back, with the plan still unread.
        for i in 60..200 {
            cell.with(|log| log.push(LogEntry { at_ms: 1_000 + i, ..entry() }));
            cell.cut_if_planned();
        }
        let read: Vec<u64> = planned.read().iter().map(|e| e.seq).collect();
        assert_eq!(read, (20..60).collect::<Vec<_>>(), "the 40 it kept when planned, from the file it planned on");
        assert_eq!(cell.after(None, usize::MAX).iter().map(|e| e.seq).collect::<Vec<_>>(), (160..200).collect::<Vec<_>>());
        // Read while four threads log and cut back: each read the runs it keeps, in order.
        let cell = std::sync::Arc::new(cell);
        let writers: Vec<_> = (0..4)
            .map(|_| {
                let cell = std::sync::Arc::clone(&cell);
                std::thread::spawn(move || {
                    for i in 0..300 {
                        cell.with(|log| log.push(entry()));
                        if i % 25 == 0 {
                            cell.cut_if_planned();
                        }
                    }
                })
            })
            .collect();
        let mut reads = 0;
        while !writers.iter().all(|w| w.is_finished()) || reads == 0 {
            let seqs: Vec<u64> = cell.after(None, usize::MAX).iter().map(|e| e.seq).collect();
            assert!(seqs.len() >= 39 && seqs.len() <= 40, "{} runs read", seqs.len());
            assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "in order, none missing: {seqs:?}");
            reads += 1;
        }
        for w in writers {
            w.join().unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A log read a line at a time (the sweeps', R5) is read with its lock let go, so a run can log
    /// meanwhile, its own reader's included; it gives the lines `lines` does; and read while runs
    /// log and its file is cut back, what it gives is one file's lines, in order, none missing,
    /// only the newest maybe cut short.
    #[test]
    fn a_log_read_a_line_at_a_time_holds_no_lock_and_sees_one_file_whole() {
        let dir = temp_dir("each-line");
        let path = dir.join("slug.jsonl");
        for file in [Some(path.clone()), None] {
            let cell = LogCell::new(Log::open_keeping(file, 1_000));
            for i in 0..10 {
                cell.with(|log| log.push(LogEntry { at_ms: 1_000 + i, ..entry() }));
            }
            let mut read = Vec::new();
            cell.each_line(|l| {
                // The lock is free: taking it again here would never return otherwise.
                cell.with(|log| log.next_seq());
                read.push(l.to_string());
            });
            assert_eq!(read, cell.with(|log| log.lines()));
            assert_eq!(cell.lines_holding("\"at_ms\":1003").len(), 1);
        }
        let _ = std::fs::remove_file(&path);
        let cell = std::sync::Arc::new(LogCell::new(Log::open_keeping(Some(path.clone()), 40)));
        let writers: Vec<_> = (0..4)
            .map(|_| {
                let cell = std::sync::Arc::clone(&cell);
                std::thread::spawn(move || {
                    for i in 0..300 {
                        cell.with(|log| log.push(entry()));
                        if i % 20 == 0 {
                            cell.cut_if_planned();
                        }
                    }
                })
            })
            .collect();
        let mut reads = 0;
        while !writers.iter().all(|w| w.is_finished()) || reads == 0 {
            let mut lines = Vec::new();
            cell.each_line(|l| lines.push(l.to_string()));
            let whole = lines.len().saturating_sub(1);
            let seqs: Vec<u64> = lines[..whole].iter().map(|l| serde_json::from_str::<LogEntry>(l).expect("a line before the last is whole").seq).collect();
            assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "in order, none missing: {seqs:?}");
            reads += 1;
        }
        for w in writers {
            w.join().unwrap();
        }
        assert!(reads > 1, "read while the runs logged");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cut planned, then the file rewritten under the lock before it is swapped in (the log
    /// told to keep fewer): the stale cut is dropped and made afresh; and one whose log was deleted
    /// meanwhile brings no file back.
    #[test]
    fn a_cut_overtaken_by_the_file_changing_is_made_afresh_and_a_deleted_log_stays_deleted() {
        let dir = temp_dir("cut-stale");
        let path = dir.join("slug.jsonl");
        let mut log = Log::open_keeping(Some(path.clone()), 3);
        log.defer_cuts = true;
        for _ in 0..7 {
            log.push(entry());
        }
        let plan = log.plan_cut().expect("past twice what it keeps: a cut planned");
        log.retain(2, None);
        log.push(entry());
        let made = plan.make();
        log.finish_cut(made);
        let back = Log::open_keeping(Some(path.clone()), 2);
        assert!(back.breaks().is_empty(), "the chain unbroken");
        assert_eq!(back.next_seq(), 8);
        // A cut planned, then the log deleted: the file stays gone.
        let mut log = Log::open_keeping(Some(path.clone()), 3);
        log.defer_cuts = true;
        for _ in 0..7 {
            log.push(entry());
        }
        let plan = log.plan_cut().expect("a cut planned");
        log.delete_file();
        log.finish_cut(plan.make());
        assert!(!path.exists(), "a deleted log isn't brought back");
        assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains(".cut-")), "no cut left beside it");
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

    /// The bound holds over random values too (D4): 20,000 of every kind, nested, from tiny to well
    /// past the limit (long texts of awkward characters, numbers of every size and none, records
    /// with awkward keys): never below the JSON, and the timer keeps exactly what writing the JSON
    /// would have kept.
    #[test]
    fn the_bound_holds_over_random_values_of_every_kind() {
        use holon_core::{Observer, Slot};
        let mut seed = 0x0dd_ba11_cafe_f00du64;
        let mut next = move |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % m.max(1)
        };
        fn text(next: &mut impl FnMut(u64) -> u64, most: u64) -> String {
            const PIECES: [&str; 12] = ["a", "\"", "\\", "\n", "\u{1}", "\u{7f}", "é", "😀", "‹", "/", " ", "0"];
            let n = next(most);
            (0..n).map(|_| PIECES[next(PIECES.len() as u64) as usize]).collect()
        }
        fn value(next: &mut impl FnMut(u64) -> u64, depth: u32) -> Value {
            let numbers = [0.0, -0.0, 1.0, 0.1, -2.2250738585072014e-308, 1.7976931348623157e308, 5e-324, 123456789.125, f64::NAN, f64::INFINITY];
            match next(if depth > 3 { 3 } else { 7 }) {
                0 => Value::Number(numbers[next(numbers.len() as u64) as usize]),
                1 => Value::Boolean(next(2) == 0),
                2 => Value::Text(text(next, 400)),
                3 => Value::List((0..next(12)).map(|_| value(next, depth + 1)).collect()),
                4 => Value::Record((0..next(8)).map(|_| (text(next, 20), value(next, depth + 1))).collect()),
                5 => Value::Effect { sink: text(next, 12), payload: Box::new(value(next, depth + 1)) },
                _ => Value::Sum { arm: next(1 << 40) as usize, payload: Box::new(value(next, depth + 1)) },
            }
        }
        let most = Timer::MAX_RECORDED_BYTES;
        let (mut kept, mut not) = (0, 0);
        for _ in 0..20_000 {
            let slots: Vec<Slot> = (0..1 + next(3)).map(|_| if next(5) == 0 { Slot::Absent } else { Slot::Value(value(&mut next, 0)) }).collect();
            let json = serde_json::to_vec(&slots.iter().map(|o| o.value().map(SValue::from_core)).collect::<Vec<_>>()).unwrap().len();
            if let Some(bound) = outputs_bound(&slots) {
                assert!(bound >= json, "{bound} < {json}");
            }
            let mut timer = Timer::new();
            timer.enter(&[0]);
            timer.produced(&[0], &slots);
            timer.leave(&[0]);
            assert_eq!(timer.timeline[0].outputs.is_some(), json <= most, "{json} bytes");
            if json <= most { kept += 1 } else { not += 1 }
        }
        assert!(kept > 1000 && not > 100, "both sides of the limit tried: {kept} kept, {not} not");
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
