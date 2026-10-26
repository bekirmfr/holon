//! Exporting a deployment's runs, and checking an export anywhere, offline: `holon verify`.
//!
//! An export holds what checking a run log needs without the server: the log's lines exactly as
//! written (so the hash chain checks), its head, and every unit the runs ran, as canonical bytes
//! (so each checks against its hash). `holon verify` checks the chain, then runs each run again
//! from its entry: the same unit, request, state, clock and random, the same outbound answers and
//! collection reads, through the same core and rounds as the server; and compares what it
//! produces with what the log says the run produced: its run id, its outputs, the effects it
//! performed and the status it answered.
//!
//! A node that computed with a secret or a resume token can't be computed again: the log holds a
//! placeholder, never the value. What the log kept for that node stands in (`Asserting`), as in
//! the editor's replay, and the rest of the run is checked against it; the report says for how
//! many runs, and for how many nodes. What such a node produced is taken from the log, not
//! checked. Some runs can't be run again at all, and are counted, not failed: one whose such node
//! the log keeps no outputs for (too large, or too deep), one that failed on a budget (the clock
//! decided), one no route took (nothing ran), one that read a file whose text neither the export
//! nor `--files` holds (its folder keeps only hashes).

use std::collections::BTreeSet;

use crate::*;
use holon_core::Observer;
use holon_host::{Exchange, Limits, Performer, Waits};
use recorder::{line_hash, LogEntry};

/// What an export says it is.
pub(crate) const FORMAT: &str = "holon-runs/1";

/// A deployment's runs, as `GET /deployments/<slug>/export` writes them.
#[derive(Serialize, Deserialize)]
pub(crate) struct Export {
    pub(crate) export: String,
    pub(crate) deployment: String,
    /// The hash of the newest line: what a witness was sent.
    pub(crate) head: String,
    /// The log's lines exactly as written, oldest first.
    pub(crate) lines: Vec<String>,
    /// Every unit the runs ran, by hash (hex): its canonical bytes (hex).
    pub(crate) units: BTreeMap<String, String>,
    /// The text of each file a run read that its log entry keeps by hash, by its place in the
    /// content store (hex): checked against the file's hash, so it can't stand in for another.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) blobs: BTreeMap<String, String>,
    /// What of those texts it doesn't hold, when it was asked for less (`--max-files-bytes`,
    /// `--without-files`): runs that read them check only with the files given (`--files`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) files_left_out: Option<LeftOut>,
    /// How many of those texts the server no longer keeps (its folder's `keep: bytes for <d>` went
    /// by, and they were swept): runs that read them check only with the files given.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) files_not_kept: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The texts of files an export left out: how many, and how big in all.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct LeftOut {
    pub(crate) files: u64,
    pub(crate) bytes: u64,
}

/// `GET /deployments/<slug>/export` — the deployment's runs, and the units they ran, as one file;
/// with the texts of the files they read that their log keeps by hash, up to `?max_files_bytes=N`
/// in all (oldest run first), or none with `?without_files=1`.
pub(crate) fn handle_get_export(
    store: &SharedStore,
    folders: &folders::Folders,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    slug: &str,
    request: &Request,
) -> Response {
    let budget = if request.get_param("without_files").is_some_and(|v| v != "0" && v != "false") {
        0
    } else {
        match request.get_param("max_files_bytes").map(|v| v.parse::<u64>()) {
            None => u64::MAX,
            Some(Ok(n)) => n,
            Some(Err(_)) => return err(400, "max_files_bytes is a number of bytes"),
        }
    };
    let (lines, head) = match deployments.lock().unwrap().get(slug) {
        Some(dep) => (dep.log.lines(), dep.log.head().to_string()),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    let mut units = BTreeMap::new();
    let entries: Vec<LogEntry> = lines.iter().filter_map(|l| serde_json::from_str::<LogEntry>(l).ok()).collect();
    let mut blobs = BTreeMap::new();
    let (mut held, mut left_out, mut not_kept) = (0u64, LeftOut { files: 0, bytes: 0 }, 0u64);
    let mut seen = BTreeSet::new();
    for blob in entries.iter().flat_map(|e| &e.reads).filter_map(|r| r.file.as_ref()?.blob.as_ref()) {
        if !seen.insert(blob.clone()) {
            continue;
        }
        match folders::kept_text(folders, store, blob) {
            Ok(Some(text)) => {
                let size = text.len() as u64;
                if held.saturating_add(size) > budget {
                    left_out.files += 1;
                    left_out.bytes += size;
                    continue;
                }
                held += size;
                blobs.insert(blob.clone(), text);
            }
            Ok(None) => not_kept += 1,
            Err(e) => return err(500, e),
        }
    }
    let mut queue: Vec<Hash> = entries.iter().filter_map(|e| parse_hash(&e.unit)).collect();
    while let Some(h) = queue.pop() {
        let key = to_hex(&h);
        if units.contains_key(&key) {
            continue;
        }
        let unit = match store.get(&h) {
            Ok(Some(unit)) => unit,
            Ok(None) => continue,
            Err(e) => return err(500, format!("store error: {e:?}")),
        };
        queue.extend(child_refs(&unit));
        if let Ok(bytes) = encode_preimage(&unit) {
            units.insert(key, hex_of(&bytes));
        }
    }
    let files_left_out = (left_out.files > 0).then_some(left_out);
    let export = Export { export: FORMAT.into(), deployment: slug.into(), head, lines, units, blobs, files_left_out, files_not_kept: not_kept };
    Response::json(&export).with_additional_header("Content-Disposition", format!("attachment; filename=\"{slug}-runs.json\""))
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn bytes_of(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

/// What one run came to when run again.
#[derive(Debug, PartialEq)]
pub(crate) enum Rerun {
    /// It produced what the log says.
    Matched,
    /// It didn't: what differs.
    Differs(String),
    /// It can't be run again: why.
    Skipped(String),
}

/// What checking an export found.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) deployment: String,
    /// The runs in it, oldest and newest seq.
    pub(crate) runs: usize,
    pub(crate) first: Option<u64>,
    pub(crate) last: Option<u64>,
    /// Whether the oldest run follows one no longer in the export (the log was trimmed).
    pub(crate) trimmed: bool,
    /// Lines that aren't a run (1-based): cut short by a crash, or changed into something else.
    pub(crate) bad_lines: Vec<usize>,
    /// Runs that don't follow the line before them.
    pub(crate) breaks: Vec<u64>,
    /// Its newest line's hash, and whether that is the head the export names.
    pub(crate) head: String,
    pub(crate) head_matches: bool,
    /// For a head given to check against (a witness's): the run it is the hash of, if any.
    pub(crate) witnessed: Option<Option<u64>>,
    /// Units whose bytes don't hash to the name they're under, or don't decode.
    pub(crate) bad_units: Vec<String>,
    /// Each run, by seq, with what running it again came to.
    pub(crate) reruns: Vec<(u64, Rerun)>,
    /// The runs checked given what the log kept for nodes that can't be computed again (fed a
    /// secret or a token): by seq, how many such nodes.
    pub(crate) given: BTreeMap<u64, usize>,
}

impl Report {
    /// Whether the export checks: the chain unbroken up to its head (and the given one), every
    /// unit what it says, and every run that could be run again producing what the log says.
    pub(crate) fn ok(&self) -> bool {
        self.breaks.is_empty()
            && self.head_matches
            && self.witnessed != Some(None)
            && self.bad_units.is_empty()
            && !self.reruns.iter().any(|(_, r)| matches!(r, Rerun::Differs(_)))
    }
}

/// Check `export`: its chain, its units, and each run, run again. `witnessed`: a head to find in
/// it (one a witness holds). `files`: files the runs read, by hash (`--files`), for reads the
/// export keeps only the hash of.
pub(crate) fn check(export: &Export, witnessed: Option<&str>, files: &BTreeMap<String, PathBuf>) -> Report {
    let mut report = Report { deployment: export.deployment.clone(), ..Report::default() };

    // The chain, as the server checks it when it opens a log (recorder.rs).
    let mut entries: Vec<(LogEntry, &str)> = Vec::new();
    let mut head = String::new();
    let mut chained = false;
    for (i, line) in export.lines.iter().enumerate() {
        let Ok(entry) = serde_json::from_str::<LogEntry>(line) else {
            report.bad_lines.push(i + 1);
            continue;
        };
        if head.is_empty() {
            report.trimmed = !entry.prev.is_empty();
        } else if (chained || !entry.prev.is_empty()) && entry.prev != head {
            report.breaks.push(entry.seq);
        }
        chained |= !entry.prev.is_empty();
        head = line_hash(line);
        if witnessed == Some(head.as_str()) {
            report.witnessed = Some(Some(entry.seq));
        }
        entries.push((entry, line));
    }
    if witnessed.is_some() && report.witnessed.is_none() {
        report.witnessed = Some(None);
    }
    report.head_matches = head == export.head;
    report.head = head;
    report.runs = entries.len();
    report.first = entries.first().map(|(e, _)| e.seq);
    report.last = entries.last().map(|(e, _)| e.seq);

    // The units, each checked against its name.
    let mut resolver = MapResolver::new();
    for (name, hex) in &export.units {
        let unit = bytes_of(hex).and_then(|b| decode_preimage(&b).ok());
        match unit.map(|u| (hash_unit(&u).map(|h| to_hex(&h)), u)) {
            Some((Ok(h), unit)) if &h == name => {
                let _ = resolver.insert(unit);
            }
            _ => report.bad_units.push(name.clone()),
        }
    }

    let texts = Texts { blobs: &export.blobs, files };
    for (i, (entry, _)) in entries.iter().enumerate() {
        let earlier: Vec<&LogEntry> =
            entries[..i].iter().map(|(e, _)| e).filter(|e| !entry.run_id.is_empty() && e.run_id == entry.run_id).collect();
        let mut given = 0;
        let came_to = rerun(entry, &earlier, &texts, &holon_module::WithEngine::new(&resolver, holon_module::shared()), &mut given);
        if given > 0 {
            report.given.insert(entry.seq, given);
        }
        report.reruns.push((entry.seq, came_to));
    }
    report
}

/// Where the text of a file a run read comes from when its log entry keeps only the hash: the
/// export's blobs, or the files given with `--files`.
struct Texts<'a> {
    blobs: &'a BTreeMap<String, String>,
    files: &'a BTreeMap<String, PathBuf>,
}

impl Texts<'_> {
    /// The text a logged file read answered: the entry's own, or found by its blob or its hash.
    /// `Err`: none here (what the run read), or a blob that isn't the file it says.
    fn answer(&self, read: &recorder::LoggedRead) -> Result<Option<Value>, Rerun> {
        let Some(file) = &read.file else { return Ok(read.answer.clone().map(SValue::into_core)) };
        if let Some(range) = &file.range {
            return self.range(read, file, range);
        }
        if let Some(answer) = &read.answer {
            return Ok(Some(answer.clone().into_core()));
        }
        let what = || what(read);
        if let Some(text) = file.blob.as_ref().and_then(|b| self.blobs.get(b)) {
            if blake3::hash(text.as_bytes()).to_hex().as_str() != file.hash {
                return Err(Rerun::Differs(format!("the text kept for {} isn't the file it read", what())));
            }
            return Ok(Some(Value::Text(text.clone())));
        }
        match self.files.get(&file.hash) {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(text) => Ok(Some(Value::Text(text))),
                Err(e) => Err(Rerun::Skipped(format!("it read {}, and {} can't be read as text: {e}", what(), path.display()))),
            },
            None => Err(Rerun::Skipped(format!(
                "it read {}, whose text the export doesn't hold (its folder keeps hashes, the server no longer keeps it, or the export left it out): give it with --files",
                what()
            ))),
        }
    }

    /// The answer of a read of part of a file: its text from the entry, the export's blobs or a
    /// file given with `--files`; proved part of the file it says by its hashes (ranges.rs), or
    /// taken from the file whole. `Err`: none here, or bytes that aren't that file's.
    fn range(&self, read: &recorder::LoggedRead, file: &recorder::LoggedFile, range: &recorder::LoggedRange) -> Result<Option<Value>, Rerun> {
        let lines = matches!(read.request.clone().into_core(), Value::Record(r) if r.contains_key("lines"));
        let kept = range.text.clone().or_else(|| file.blob.as_ref().and_then(|b| self.blobs.get(b)).cloned());
        if let Some(text) = kept {
            let proof: Option<Vec<[u8; 32]>> = range.proof.iter().map(|h| blake3::Hash::from_hex(h).ok().map(|h| *h.as_bytes())).collect();
            let (Some(before), Some(after), Some(proof)) = (ranges::unhex(&range.before), ranges::unhex(&range.after), proof) else {
                return Err(Rerun::Differs(format!("the proof kept for {} isn't hex", what(read))));
            };
            let bytes = [before.as_slice(), text.as_bytes(), after.as_slice()].concat();
            let a = range.start.checked_sub(before.len() as u64);
            let fits = range.end.checked_sub(range.start) == Some(text.len() as u64)
                && a.and_then(|a| ranges::root_of(file.size, a, &bytes, &proof)).is_some_and(|h| h.to_hex().as_str() == file.hash);
            if !fits {
                return Err(Rerun::Differs(format!("bytes {}..{} kept for {} aren't part of the file it read", range.start, range.end, what(read))));
            }
            return Ok(Some(folders::range_answer(lines, &text)));
        }
        let Some(path) = self.files.get(&file.hash) else {
            return Err(Rerun::Skipped(format!(
                "it read part of {}, whose text the export doesn't hold (its folder keeps hashes, the server no longer keeps it, or the export left it out): give the file with --files",
                what(read)
            )));
        };
        let mut part = vec![0u8; range.end.saturating_sub(range.start) as usize];
        let got = std::fs::File::open(path).and_then(|mut f| {
            use std::io::{Read, Seek};
            f.seek(std::io::SeekFrom::Start(range.start))?;
            f.read_exact(&mut part)
        });
        match got.map(|_| String::from_utf8(part)) {
            Ok(Ok(text)) => Ok(Some(folders::range_answer(lines, &text))),
            _ => Err(Rerun::Skipped(format!("it read part of {}, and {} doesn't have it as text", what(read), path.display()))),
        }
    }
}

/// The file a logged read read, as `folder/name`.
fn what(read: &recorder::LoggedRead) -> String {
    let Value::Record(r) = read.request.clone().into_core() else { return "?".into() };
    let name = |v: Option<&Value>| match v {
        Some(Value::Text(t)) => Some(t.clone()),
        Some(Value::Record(q)) => match q.get("name") {
            Some(Value::Text(t)) => Some(t.clone()),
            _ => None,
        },
        _ => None,
    };
    let folder = name(r.get("folder")).unwrap_or_else(|| "?".into());
    let file = name(r.get("read")).or_else(|| name(r.get("lines"))).or_else(|| name(r.get("slice"))).unwrap_or_else(|| "?".into());
    format!("{folder}/{file}")
}

/// Run `entry` again from what it records (and what the earlier parts of the same run, `earlier`,
/// record: a resumed run's outbound answers and reads), and compare. `given` is set to how many
/// nodes the log's record stood in for (`Asserting`).
fn rerun(entry: &LogEntry, earlier: &[&LogEntry], texts: &Texts, resolver: &impl holon_core::Resolver, given: &mut usize) -> Rerun {
    if entry.run_id.is_empty() {
        return Rerun::Skipped("nothing ran: no route took the request".into());
    }
    if let Some(e) = &entry.error {
        return Rerun::Skipped(format!("it failed: {e}"));
    }
    let Some(deployment) = parse_hash(&entry.unit) else {
        return Rerun::Skipped("its entry doesn't name its unit (logged before runs did)".into());
    };
    let Some(deployed) = resolver.resolve(&deployment) else {
        return Rerun::Differs(format!("its unit {} isn't in the export", &entry.unit[..12.min(entry.unit.len())]));
    };

    // What it ran: a request's endpoint, with the request; anything else, the whole unit.
    let (root, inputs) = match &entry.route {
        Some(route) => {
            let Ok(server) = read_server(deployed, resolver) else {
                return Rerun::Differs("it names a route, but its unit isn't a server".into());
            };
            let Some(endpoint) = server.routes.iter().find(|r| format!("{} {}", r.method, r.path) == *route).map(|r| r.endpoint)
            else {
                return Rerun::Differs(format!("its unit has no route {route:?}"));
            };
            let Some(root) = resolver.resolve(&endpoint) else {
                return Rerun::Differs(format!("the endpoint of {route:?} isn't in the export"));
            };
            (root, vec![entry.request.clone().map(SValue::into_core)])
        }
        None => (deployed, Vec::new()),
    };

    let mut extra = Vec::new();
    for (name, value) in &entry.sourced {
        let Some(cap) = durable::capability_named(name) else {
            return Rerun::Skipped(format!("it read {name}, which verify can't supply"));
        };
        extra.push((cap, value.clone().into_core()));
    }
    let values: BTreeMap<String, Value> = entry.state_before.iter().map(|(k, v)| (k.clone(), v.clone().into_core())).collect();
    let Ok(state) = State::from_values(values) else {
        return Rerun::Differs("its state doesn't encode".into());
    };

    // A secret stands in as the placeholder the log shows (a token too: `Logged::token`). A node
    // that only passes one along computes the same; one that computes with it can't be computed
    // again, and what the log kept for it stands in (`Asserting`).
    let caps = capabilities(root, resolver);
    let mut sourced: Vec<(Capability, Value)> =
        state.iter().map(|(k, v)| (Capability::StateRead(k.clone()), v.clone())).collect();
    sourced.extend(extra.iter().cloned());
    for cap in caps.iter() {
        if let Capability::SecretRead(name) = cap {
            sourced.push((Capability::SecretRead(name.clone()), Value::Text(format!("‹secret:{name}›"))));
        }
    }

    let mut differs = Vec::new();
    let nonce = bytes_of(&entry.nonce).and_then(|b| <[u8; 16]>::try_from(b).ok());
    let id_matches = nonce.is_some_and(|nonce| {
        let id = |state: Option<&State>| to_hex(&run_id_of(deployment, &inputs, state, &extra, &nonce));
        id(Some(&state)) == entry.run_id || (state.is_empty() && id(None) == entry.run_id)
    });
    if !id_matches {
        differs.push("its run id");
    }

    // Its outbound requests are answered from the record (the earlier parts', then its own), never
    // sent again.
    let prior: Vec<Round> = earlier.iter().flat_map(|e| &e.rounds).chain(&entry.rounds).map(round_of).collect();
    // Its reads, its own first: a resumed part reads again what it shares (files, databases), and
    // its last part's answers are the ones it finished with.
    let mut reads: Vec<(Vec<u32>, Value, Option<Value>)> = Vec::new();
    for read in entry.reads.iter().chain(earlier.iter().rev().flat_map(|e| &e.reads)) {
        let answer = match texts.answer(read) {
            Ok(answer) => answer,
            Err(not) => return not,
        };
        reads.push((read.path.clone(), read.request.clone().into_core(), answer));
    }
    let mut waits = Logged {
        reads,
        events: entry.events.iter().map(|(k, v)| (k.clone(), v.clone().into_core())).collect(),
    };
    let limits = Limits { run_budget: Duration::from_secs(3600), max_requests: usize::MAX, ..Limits::clock_run() };
    let mut host = RecordingHost { performed: Vec::new() };
    let mut observer = Asserting::from_timeline(&entry.timeline);
    let outcome = run_rounds(root, &inputs, &sourced, resolver, &mut host, &Offline, &limits, &mut observer, &mut waits, &prior);
    *given = observer.stood_in.len();

    let outputs: Vec<Option<SValue>> = outcome.outputs.iter().map(|o| o.as_ref().map(|v| SValue::from_core(&folders::loggable_output(v)))).collect();
    if json(&outputs) != json(&entry.outputs) {
        differs.push("its outputs");
    }
    let performed: Vec<PerformedEntry> = host
        .performed
        .iter()
        .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: SValue::from_core(&folders::loggable(c, p)) })
        .collect();
    if json(&performed) != json(&entry.performed) {
        differs.push("the effects it performed");
    }
    // A part that settled waiting: the packs it sent as it parked are what it settled.
    let progress: Vec<PerformedEntry> = if outcome.waiting.is_empty() { Vec::new() } else { channels::settled_sends(&outcome.outputs) }
        .iter()
        .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: SValue::from_core(p) })
        .collect();
    if json(&progress) != json(&entry.progress) {
        differs.push("the packs it sent as it waited");
    }
    if entry.kind == "request"
        && let Some(status) = entry.status
        && data::respond(outcome.outputs.first().and_then(Option::as_ref)).status != status
    {
        differs.push("the status it answered");
    }
    if outcome.error.is_some() {
        differs.push("whether it finished");
    }

    match differs.is_empty() {
        true => Rerun::Matched,
        false if !observer.unrecorded.is_empty() => Rerun::Skipped(
            "it computed with a secret or a resume token at a node the log keeps no outputs for (too large, or too deep)".into(),
        ),
        false if entry.retries > 0 => {
            Rerun::Skipped("it ran again after what it read changed, and its entry holds only its last attempt's requests (logged before entries held them all)".into())
        }
        false => Rerun::Differs(differs.join(", ")),
    }
}

/// A logged shape as JSON: values compare as the log writes them.
fn json<T: Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}

fn round_of(round: &recorder::LoggedRound) -> Round {
    Round {
        exchanges: round
            .exchanges
            .iter()
            .map(|x| Exchange {
                path: x.path.clone(),
                request: x.request.clone().into_core(),
                response: x.response.clone().into_core(),
                start_us: x.start_us,
                end_us: x.end_us,
            })
            .collect(),
    }
}

/// Answers a run's waiting points, collection reads and file reads from its log entries (a file's
/// text from the export or `--files`); its tokens as the placeholder the log shows.
struct Logged {
    reads: Vec<(Vec<u32>, Value, Option<Value>)>,
    events: BTreeMap<String, Value>,
}

impl Waits for Logged {
    fn read(&mut self, path: &[u32], request: &Value) -> Option<Value> {
        self.reads.iter().find(|(p, r, _)| p == path && r == request).and_then(|(_, _, a)| a.clone())
    }
    fn event(&self, name: &str) -> Option<Value> {
        self.events.get(name).cloned()
    }
    fn token(&mut self, name: &str) -> String {
        format!("‹token:{name}›")
    }
}

/// Observes a run being checked: the server's timer (so its steps are reported the same way), and,
/// for a node whose inputs carry what the log never holds (a secret's or a token's placeholder),
/// the outputs the log kept for it in the last round it recorded, stood in for what core would
/// compute, as the editor's replay does (`editor/src/replay.ts`). The rest of the run is checked
/// against them; what such a node produced is taken from the log, not checked. A composite fed a
/// placeholder is opened instead: the leaves inside it that compute with it are stood in for,
/// and the rest of it checked.
struct Asserting {
    timer: recorder::Timer,
    /// Each node's outputs in the last round the log recorded them in, by path.
    recorded: BTreeMap<Vec<u32>, Vec<Option<Value>>>,
    /// The nodes stood in for.
    stood_in: BTreeSet<Vec<u32>>,
    /// The nodes that needed standing in for and that the log keeps no outputs for (larger than
    /// the log keeps, or deeper than it records).
    unrecorded: BTreeSet<Vec<u32>>,
}

impl Asserting {
    fn from_timeline(timeline: &[recorder::NodeTiming]) -> Asserting {
        let mut rounds: BTreeMap<Vec<u32>, u32> = BTreeMap::new();
        let mut recorded = BTreeMap::new();
        for t in timeline {
            let Some(outputs) = &t.outputs else { continue };
            if rounds.get(&t.path).is_some_and(|&r| r > t.round) {
                continue;
            }
            rounds.insert(t.path.clone(), t.round);
            recorded.insert(t.path.clone(), outputs.iter().map(|o| o.clone().map(SValue::into_core)).collect());
        }
        Asserting { timer: recorder::Timer::new(), recorded, stood_in: BTreeSet::new(), unrecorded: BTreeSet::new() }
    }
}

impl Observer for Asserting {
    fn enter(&mut self, path: &[u32]) {
        self.timer.enter(path)
    }
    fn leave(&mut self, path: &[u32]) {
        self.timer.leave(path)
    }
    fn produced(&mut self, path: &[u32], outputs: &[holon_core::Slot]) {
        self.timer.produced(path, outputs)
    }
    fn round(&mut self, n: usize) {
        self.timer.round(n)
    }
    fn failed(&mut self, path: &[u32], reason: &str) {
        self.timer.failed(path, reason)
    }
    fn asserted(&mut self, path: &[u32], unit: &holon_core::Unit, inputs: &[Option<Value>]) -> Option<Vec<Option<Value>>> {
        if matches!(unit.body, holon_core::UnitBody::Composite { .. }) || !inputs.iter().flatten().any(carries_placeholder) {
            return None;
        }
        match self.recorded.get(path) {
            Some(outputs) => {
                self.stood_in.insert(path.to_vec());
                Some(outputs.clone())
            }
            None => {
                self.unrecorded.insert(path.to_vec());
                None
            }
        }
    }
}

/// Whether `v` carries a secret's or a token's placeholder anywhere: what the log shows in place
/// of a value it never holds.
fn carries_placeholder(v: &Value) -> bool {
    match v {
        Value::Text(t) => t.contains("‹secret:") || t.contains("‹token:"),
        Value::List(items) => items.iter().any(carries_placeholder),
        Value::Record(fields) => fields.values().any(carries_placeholder),
        Value::Sum { payload, .. } | Value::Effect { payload, .. } => carries_placeholder(payload),
        Value::Number(_) | Value::Boolean(_) => false,
    }
}

/// The network, for a run being checked: there is none. A request the log has no answer for is
/// answered as a failed one, and the run then differs from the log.
struct Offline;

impl Performer for Offline {
    fn perform(&self, _: &Value, _: &Limits) -> Value {
        Value::Record(BTreeMap::from([
            ("status".to_string(), Value::Number(0.0)),
            ("error".to_string(), Value::Text("not in the log".into())),
        ]))
    }
}

/// `holon verify <export.json> [--head <hex>] [--files DIR]`: check an export, say what was
/// found, and exit 0 only if it checks.
pub(crate) fn run(args: &[String]) -> i32 {
    let mut file = None;
    let mut head = None;
    let mut dir = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--head" => head = args.next().cloned(),
            "--files" => dir = args.next().cloned(),
            "--help" | "-h" => {
                println!("{USAGE}");
                return 0;
            }
            _ if file.is_none() && !a.starts_with('-') => file = Some(a.clone()),
            _ => {
                eprintln!("holon verify: no option {a:?}\n\n{USAGE}");
                return 2;
            }
        }
    }
    let Some(file) = file else {
        eprintln!("{USAGE}");
        return 2;
    };
    let export: Export = match std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string())) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("holon verify: can't read {file}: {e}");
            return 2;
        }
    };
    if export.export != FORMAT {
        eprintln!("holon verify: {file} isn't a run export this holon reads ({:?}, not {FORMAT:?})", export.export);
        return 2;
    }
    let files = match dir.as_deref().map(|d| folders::files_in(Path::new(d))) {
        None => BTreeMap::new(),
        Some(Ok(files)) => files,
        Some(Err(e)) => {
            eprintln!("holon verify: can't read the files: {e}");
            return 2;
        }
    };
    let report = check(&export, head.as_deref(), &files);
    print!("{}", describe(&report, head.as_deref()));
    if report.ok() { 0 } else { 1 }
}

const USAGE: &str = "usage: holon verify <export.json> [--head <hex>] [--files DIR]

  Checks a deployment's runs, exported from GET /deployments/<slug>/export: that its run log's
  hash chain is unbroken, that every unit is what its hash says, and that every run, run again
  from its log entry, produces what the log says it did. --head checks that a head a witness
  holds is in the export, so no run up to it was changed or removed. --files gives the files the
  runs read (whole, or part of) whose folder keeps only their hashes: each is found by its hash, wherever
  it is under DIR. Exits 0 when it all checks.";

/// What a report says, in a few lines.
pub(crate) fn describe(r: &Report, head: Option<&str>) -> String {
    let short = |h: &str| h.get(..12).unwrap_or(h).to_string();
    let mut out = String::new();
    let span = match (r.first, r.last) {
        (Some(a), Some(b)) => format!(", runs {a} to {b}"),
        _ => String::new(),
    };
    out += &format!("{}: {} runs{span}\n", r.deployment, r.runs);

    let mut chain = if r.breaks.is_empty() {
        "chain: unbroken".to_string()
    } else {
        format!("chain: BROKEN at run {}: changed, removed or inserted outside holon", list(&r.breaks))
    };
    if r.trimmed {
        chain += &format!(" (from run {}; older runs were trimmed)", r.first.unwrap_or(0));
    }
    out += &format!("  {chain}\n");
    if !r.bad_lines.is_empty() {
        out += &format!("  lines {} aren't runs (cut short by a crash, or changed)\n", list(&r.bad_lines));
    }
    out += &if r.head_matches {
        format!("  head: {} is the newest run's\n", short(&r.head))
    } else {
        "  head: DOESN'T MATCH the newest run: the export was changed\n".to_string()
    };
    match (&r.witnessed, head) {
        (Some(Some(seq)), Some(h)) if r.breaks.iter().any(|b| b <= seq) => {
            out += &format!("  witnessed head {}: run {seq}, but the chain breaks before it, so runs up to it were changed\n", short(h))
        }
        (Some(Some(seq)), Some(h)) => out += &format!("  witnessed head {}: run {seq}, so no run up to it was changed or removed\n", short(h)),
        (Some(None), Some(h)) => {
            out += &format!(
                "  witnessed head {}: NOT FOUND: a run up to it was changed or removed, or it is older than the oldest run here\n",
                short(h)
            )
        }
        _ => {}
    }
    if !r.bad_units.is_empty() {
        out += &format!("  units: {} DON'T MATCH their hash\n", r.bad_units.iter().map(|u| short(u)).collect::<Vec<_>>().join(", "));
    }

    let matched = r.reruns.iter().filter(|(_, x)| *x == Rerun::Matched).count();
    let mut not: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for (seq, x) in &r.reruns {
        if let Rerun::Skipped(why) = x {
            not.entry(why).or_default().push(*seq);
        }
    }
    out += &format!("  re-ran {matched} runs: each produced what its log entry says\n");
    if !r.given.is_empty() {
        let seqs: Vec<u64> = r.given.keys().copied().collect();
        let nodes: usize = r.given.values().sum();
        out += &format!(
            "  {} checked given what the log kept for {nodes} node{} fed a secret or a resume token (not computed again)\n",
            count_of(&seqs),
            if nodes == 1 { "" } else { "s" }
        );
    }
    for (why, seqs) in &not {
        out += &format!("  not re-run ({}): {why}\n", count_of(seqs));
    }
    for (seq, x) in &r.reruns {
        if let Rerun::Differs(what) = x {
            out += &format!("  run {seq} DIFFERS: {what}\n");
        }
    }
    out += if r.ok() { "ok\n" } else { "FAILED\n" };
    out
}

fn count_of(seqs: &[u64]) -> String {
    let n = seqs.len();
    format!("{n} run{}: {}", if n == 1 { "" } else { "s" }, list(seqs))
}

fn list<T: std::fmt::Display>(items: &[T]) -> String {
    let shown: Vec<String> = items.iter().take(10).map(T::to_string).collect();
    shown.join(", ") + if items.len() > 10 { ", …" } else { "" }
}

/// `holon export <slug> [--server PORT|URL] [--to FOLDER] [--as NAME] [--max-files-bytes N |
/// --without-files]`: fetch a deployment's runs from a server, as the editor's "export runs" does,
/// and write them where `holon verify` reads them. The server and sign-in come from the one this
/// user runs here (local.rs), or `--server` and `HOLON_TOKEN`.
pub(crate) fn export(args: &[String]) -> i32 {
    let mut slug = None;
    let mut server = None;
    let mut files: Option<String> = None;
    let (mut to, mut name, mut out) = (None, None, None);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--to" => to = args.next().cloned(),
            "--as" => name = args.next().cloned(),
            "--without-files" => files = Some("without_files=1".into()),
            "--max-files-bytes" => match args.next().and_then(|v| budget_of(v)) {
                Some(n) => files = Some(format!("max_files_bytes={n}")),
                None => {
                    eprintln!("holon export: --max-files-bytes takes a size: 1048576, 64K, 10M or 2G");
                    return 2;
                }
            },
            // The whole path at once, as rc.7 and rc.8 took it.
            "-o" | "--out" => out = args.next().cloned(),
            "--help" | "-h" => {
                println!("{EXPORT_USAGE}");
                return 0;
            }
            _ if slug.is_none() && !a.starts_with('-') => slug = Some(a.clone()),
            _ => {
                eprintln!("holon export: no option {a:?}\n\n{EXPORT_USAGE}");
                return 2;
            }
        }
    }
    let Some(slug) = slug else {
        eprintln!("{EXPORT_USAGE}");
        return 2;
    };
    let out = match export_path(&slug, to.as_deref(), name.as_deref(), out.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("holon export: {e}");
            return 2;
        }
    };
    let target = match local::target(server.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("holon export: {e}");
            return 1;
        }
    };
    let server = target.server.as_str();
    let url = format!("{server}/deployments/{slug}/export{}", files.map_or(String::new(), |q| format!("?{q}")));
    let mut request = ureq::get(&url).timeout(Duration::from_secs(120));
    if let Some(token) = &target.token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let body = match request.call() {
        Ok(response) => {
            let mut text = String::new();
            if let Err(e) = response.into_reader().read_to_string(&mut text) {
                eprintln!("holon export: reading the answer from {server} failed: {e}");
                return 1;
            }
            text
        }
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            let why = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| body.trim().to_string());
            eprintln!("holon export: {server} answered {code}: {why}");
            if code == 401 {
                eprintln!("holon export: for a server elsewhere, set HOLON_TOKEN to an API token (the editor's admin page, API tokens)");
            }
            return 1;
        }
        Err(e) => {
            eprintln!("holon export: can't reach {server}: {e}");
            eprintln!("holon export: is holon serve running there? --server takes the port or address it printed");
            return 1;
        }
    };
    let export: Export = match serde_json::from_str(&body) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("holon export: {server} didn't answer with runs: {e}");
            return 1;
        }
    };
    let written = match out.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&out, &body)),
        None => std::fs::write(&out, &body),
    };
    if let Err(e) = written {
        eprintln!("holon export: can't write {}: {e}", out.display());
        return 1;
    }
    let head = export.head.get(..12).unwrap_or(&export.head);
    // Where it is in full, so the hint below runs from any folder.
    let path = std::path::absolute(&out).unwrap_or(out).display().to_string();
    let arg = if path.contains(' ') { format!("\"{path}\"") } else { path.clone() };
    println!("wrote {path}: {} runs of {slug:?}, head {head}", export.lines.len());
    if let Some(l) = &export.files_left_out {
        let s = if l.files == 1 { "" } else { "s" };
        println!("left out the text of {} file{s} its runs read ({} bytes): holon verify --files <folder> checks those runs", l.files, l.bytes);
    }
    if export.files_not_kept > 0 {
        let n = export.files_not_kept;
        let s = if n == 1 { "" } else { "s" };
        println!("the server no longer keeps the text of {n} file{s} its runs read (swept after their folder's keep): holon verify --files <folder> checks those runs");
    }
    println!("check it anywhere: holon verify {arg}");
    0
}

/// A size as `--max-files-bytes` takes it: bytes, or with K, M or G (KiB, MiB, GiB), a B after it
/// optional.
pub(crate) fn budget_of(v: &str) -> Option<u64> {
    let v = v.trim().to_ascii_uppercase();
    let v = v.strip_suffix('B').unwrap_or(&v);
    let (digits, unit) = match v.char_indices().last()? {
        (i, 'K') => (&v[..i], 1u64 << 10),
        (i, 'M') => (&v[..i], 1 << 20),
        (i, 'G') => (&v[..i], 1 << 30),
        _ => (v, 1),
    };
    digits.parse::<u64>().ok()?.checked_mul(unit)
}

/// Where `holon export` writes: `--to` (a folder, default here) and `--as` (a name, default
/// `<slug>-runs.json`, `.json` added when it has no extension), or `-o`, the whole path.
pub(crate) fn export_path(slug: &str, to: Option<&str>, name: Option<&str>, out: Option<&str>) -> Result<std::path::PathBuf, String> {
    use std::path::{Path, PathBuf};
    if let Some(out) = out {
        if to.is_some() || name.is_some() {
            return Err("-o is the whole path: give it, or --to and --as, not both".into());
        }
        return Ok(PathBuf::from(out));
    }
    let name = match name {
        Some(n) if n.contains(['/', '\\']) => return Err(format!("--as takes a file name ({n:?} is a path): the folder goes in --to")),
        Some(n) if n.trim().is_empty() => return Err("--as needs a file name".into()),
        Some(n) if Path::new(n).extension().is_none() => format!("{n}.json"),
        Some(n) => n.to_string(),
        None => format!("{slug}-runs.json"),
    };
    Ok(Path::new(to.unwrap_or("")).join(name))
}

const EXPORT_USAGE: &str = "usage: holon export <slug> [--server PORT|URL] [--to FOLDER] [--as NAME]
                    [--max-files-bytes N | --without-files]

  Writes a deployment's runs (its run log, exactly as written, and every unit they ran) to a file
  for holon verify: NAME (default <slug>-runs.json) in FOLDER (default this one, made if it
  isn't there). From the holon serve you run on this machine,
  signed in as you; with several running, --server <port> picks one. A server elsewhere:
  --server <URL> (or HOLON_SERVER), signed in with an API token in HOLON_TOKEN.

  With the runs come the texts of the files they read that their log keeps by hash (a big file,
  or part of one). --max-files-bytes N (64K, 10M, 2G) ships them up to N bytes in all, the oldest
  runs' first; --without-files ships none. It says what it left out: holon verify checks those
  runs with --files, the folder of the originals.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_budget_is_bytes_or_k_m_g() {
        assert_eq!([budget_of("1048576"), budget_of("64K"), budget_of("10m"), budget_of("2GB"), budget_of("0")], [Some(1 << 20), Some(64 << 10), Some(10 << 20), Some(2 << 30), Some(0)]);
        assert_eq!([budget_of("lots"), budget_of("-1"), budget_of("K"), budget_of("99999999999G")], [None; 4]);
    }
}
