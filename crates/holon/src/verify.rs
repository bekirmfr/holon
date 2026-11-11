//! Exporting a deployment's runs, and checking an export anywhere, offline: `holon verify`.
//!
//! An export holds what checking a run log needs without the server: the log's lines exactly as
//! written (so the hash chain checks), its head, and every unit the runs ran, as canonical bytes
//! (so each checks against its hash). `holon verify` checks the chain, then runs each run again
//! from its entry: the same unit, request, state, clock and random, the same outbound answers and
//! collection reads, through the same core and rounds as the server; and compares what it
//! produces with what the log says the run produced: its run id, its outputs, the effects it
//! performed and the status it answered. And it checks that each run's state follows from the one
//! before (`check_states`): that it began from the state the run before it left, and left what its
//! writes make of the state it began with.
//!
//! A node that computed with a secret or a resume token can't be computed again: the log holds a
//! placeholder, never the value. What the log kept for that node stands in (`Asserting`), as in
//! the editor's replay, and the rest of the run is checked against it; the report says for how
//! many runs, and for how many nodes. What such a node produced is taken from the log, not
//! checked. A run that received one (a resume route's token, a bearer token equal to a secret)
//! keeps its logged run id, made from the value; the report says for which runs. Only a mark the
//! log wrote is a placeholder: a look-alike a caller sent is logged escaped, read back as the text
//! it was, and computed with (`Marks`).
//!
//! Some runs can't be run again at all, and are counted, not failed: one whose such node the log
//! keeps no outputs for (too large, or too deep), one that failed on a budget (the clock decided),
//! one no route took (nothing ran), one that read a file whose text neither the export nor
//! `--files` holds (its folder keeps only hashes).

use std::collections::BTreeSet;

use crate::*;
use holon_core::Observer;
use holon_host::{Exchange, Limits, Performer, Waits};
use recorder::{line_hash, LogEntry};

mod replay;
pub(crate) use replay::handle_get_replay;

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
    /// The bytes of each file a run read as bytes (`file_read_bytes`) that its log entry keeps by
    /// hash, as padded base64, by its place in the blob store: checked as a text is.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) blob_bytes: BTreeMap<String, String>,
    /// What of those texts it doesn't hold, when it was asked for less (`--max-files-bytes`,
    /// `--without-files`): runs that read them check only with the files given (`--files`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) files_left_out: Option<LeftOut>,
    /// How many of those texts the server no longer keeps (its folder's `keep: bytes for <d>` went
    /// by, and they were swept): runs that read them check only with the files given.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) files_not_kept: u64,
    /// The deployment's events (events.rs), their lines exactly as written, oldest first: their
    /// own chain, kept for good. Absent from exports before rc.30.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) events: Vec<String>,
    /// The hash of the newest event's line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) events_head: String,
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
    let (log, redactor, events, events_head) = match deployments.lock().unwrap().get(slug) {
        Some(dep) => (std::sync::Arc::clone(&dep.log), crate::secrets::Redactor::of(&dep.secrets), dep.events.lines(), dep.events.head().to_string()),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    // The lines and their head under one lock, so the export's chain ends at its head.
    let (lines, head) = log.with(|log| (log.lines(), log.head().to_string()));
    match export_of(store, folders, Some(&redactor), slug, lines, head, events, events_head, budget) {
        Ok(export) => Response::json(&export).with_additional_header("Content-Disposition", format!("attachment; filename=\"{slug}-runs.json\"")),
        Err(e) => err(500, e),
    }
}

/// An export of `lines` (a log's, as written), ending at `head`: every unit their runs ran, from
/// the store, and the file texts they read that the server keeps, up to `budget` bytes of them
/// (none holding one of the deployment's secrets, `redactor`'s; none at all without it). What
/// `GET /deployments/<slug>/export` answers, and what a backup of the runs a log lets go of is
/// (storage.rs).
#[allow(clippy::too_many_arguments)]
pub(crate) fn export_of(
    store: &SharedStore,
    folders: &folders::Folders,
    redactor: Option<&crate::secrets::Redactor>,
    slug: &str,
    lines: Vec<String>,
    head: String,
    events: Vec<String>,
    events_head: String,
    budget: u64,
) -> Result<Export, String> {
    let budget = if redactor.is_some() { budget } else { 0 };
    let mut units = BTreeMap::new();
    let entries: Vec<LogEntry> = lines.iter().filter_map(|l| crate::wire::read_json::<LogEntry>(l.as_bytes()).ok()).collect();
    let (mut blobs, mut blob_bytes) = (BTreeMap::new(), BTreeMap::new());
    let (mut held, mut left_out, mut not_kept) = (0u64, LeftOut { files: 0, bytes: 0 }, 0u64);
    let mut seen = BTreeSet::new();
    let read_files = entries.iter().flat_map(|e| &e.reads).filter_map(|r| Some((r.file.as_ref()?.blob.as_ref()?, reads_bytes(&r.request))));
    for (blob, bytes) in read_files {
        if !seen.insert((blob.clone(), bytes)) {
            continue;
        }
        // A file read as bytes is shipped as its bytes, in base64; one read as text, as its text.
        let kept = if bytes { folders.blobs.get(blob)? } else { folders::kept_text(folders, store, blob)?.map(String::into_bytes) };
        match kept {
            // A file holding one of the deployment's secrets isn't shipped: it is as if the
            // server didn't keep it, and verify asks for the file instead.
            Some(kept) if redactor.is_none_or(|r| crate::secrets::bytes_hold(&kept, &|t| r.holds(t))) => not_kept += 1,
            Some(kept) => {
                let size = kept.len() as u64;
                if held.saturating_add(size) > budget {
                    left_out.files += 1;
                    left_out.bytes += size;
                    continue;
                }
                held += size;
                if bytes {
                    blob_bytes.insert(blob.clone(), holon_core::bytes_text(&kept, "base64").expect("base64 writes any bytes"));
                } else {
                    blobs.insert(blob.clone(), String::from_utf8(kept).expect("a kept text is text"));
                }
            }
            None => not_kept += 1,
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
            Err(e) => return Err(format!("store error: {e:?}")),
        };
        queue.extend(child_refs(&unit));
        if let Ok(bytes) = encode_preimage(&unit) {
            units.insert(key, hex_of(&bytes));
        }
    }
    let files_left_out = (left_out.files > 0).then_some(left_out);
    Ok(Export {
        export: FORMAT.into(),
        deployment: slug.into(),
        head,
        lines,
        units,
        blobs,
        blob_bytes,
        files_left_out,
        files_not_kept: not_kept,
        events,
        events_head,
    })
}

/// An export from its file: as written, or gzipped (a backup's, storage.rs).
pub(crate) fn read_export(path: &Path) -> Result<Export, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(&bytes[..]), &mut text).map_err(|e| format!("it is gzipped, but: {e}"))?;
        text
    } else {
        String::from_utf8(bytes).map_err(|e| e.to_string())?
    };
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn hex_of(bytes: &[u8]) -> String {
    holon_core::hex(bytes)
}

pub(crate) fn bytes_of(hex: &str) -> Option<Vec<u8>> {
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
    /// The runs whose run id was taken from the log, not made again: it was made from a secret or
    /// a resume token the run received (in its request, its state or what it read), which the log
    /// never holds.
    pub(crate) ids_given: Vec<u64>,
    /// The events in it (events.rs), the event lines that aren't one (1-based), the events that
    /// don't follow the line before them (their chain starts at the first: they are kept for
    /// good), and whether the newest's hash is the events head the export names.
    pub(crate) events: usize,
    pub(crate) bad_event_lines: Vec<usize>,
    pub(crate) event_breaks: Vec<u64>,
    pub(crate) events_head_matches: bool,
    /// For a head given to check against: the event it is the hash of, if it is one.
    pub(crate) witnessed_event: Option<u64>,
    /// Whether each run's state follows from the one before; none when no run kept the state it
    /// left (a deployment without state, or a log from before rc.30).
    pub(crate) states: Option<States>,
    /// Where the secrets went: the outbound requests that carried one (its mark in the request),
    /// and those that went to a host its unit doesn't send it to (`sent_secrets`): by seq,
    /// secret and host.
    pub(crate) secret_requests: usize,
    pub(crate) secrets_outside: Vec<(u64, String, String)>,
    /// Where the tokens went: the outbound requests that carried one a call answered to a secret
    /// (`‹token:answer access_token #…›`), and those that went to a host its unit doesn't send
    /// what is answered to that secret to (its `answer:` destinations): by seq, token and host.
    pub(crate) token_requests: usize,
    pub(crate) tokens_outside: Vec<(u64, String, String)>,
}

/// What checking that each run's state follows from the one before found (`check_states`).
#[derive(Debug, Default)]
pub(crate) struct States {
    /// The runs that began from the state the run before them left; of those, the ones that ran
    /// beside another run (`beside`), whose state may be one an earlier run left or began with,
    /// and the ones after a restore of a run's state (`restored`), from the state it put back.
    pub(crate) began: Vec<u64>,
    pub(crate) beside: Vec<u64>,
    pub(crate) restored: Vec<u64>,
    /// The runs that left what their writes make of the state they began with.
    pub(crate) left: Vec<u64>,
    /// What wasn't checked, by why.
    pub(crate) unchecked: BTreeMap<&'static str, Vec<u64>>,
    /// The runs whose state doesn't follow, and how.
    pub(crate) breaks: Vec<(u64, String)>,
}

impl States {
    fn unchecked(&mut self, why: &'static str, seq: u64) {
        self.unchecked.entry(why).or_default().push(seq);
    }
}

/// What a run's writes make of the state it began with, run again: its cells, each it wrote set
/// to what it wrote (`wrote`); and the collections it wrote (their keys in the state), whose
/// roots only their tries would give.
struct Left {
    state: State,
    wrote: BTreeSet<String>,
    collections: BTreeSet<String>,
}

/// What the log's record stood in for, running one run again.
#[derive(Default)]
struct Given {
    /// Nodes fed a secret or a token (`Asserting`).
    nodes: usize,
    /// The run id.
    id: bool,
}

impl Report {
    /// Whether the export checks: the chain unbroken up to its head (and the given one), every
    /// unit what it says, and every run that could be run again producing what the log says.
    pub(crate) fn ok(&self) -> bool {
        self.breaks.is_empty()
            && self.head_matches
            && self.event_breaks.is_empty()
            && self.bad_event_lines.is_empty()
            && self.events_head_matches
            && (self.witnessed != Some(None) || self.witnessed_event.is_some())
            && self.bad_units.is_empty()
            && !self.reruns.iter().any(|(_, r)| matches!(r, Rerun::Differs(_)))
            && self.states.as_ref().is_none_or(|s| s.breaks.is_empty())
            && self.secrets_outside.is_empty()
            && self.tokens_outside.is_empty()
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
        let Ok(entry) = crate::wire::read_json::<LogEntry>(line.as_bytes()) else {
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
    check_events(export, witnessed, &mut report);
    report.head_matches = head == export.head;
    report.head = head;
    report.runs = entries.len();
    report.first = entries.first().map(|(e, _)| e.seq);
    report.last = entries.last().map(|(e, _)| e.seq);

    // The units, each checked against its name, and what each references, for the bounds a
    // rerun is held to.
    let mut resolver = MapResolver::new();
    let mut refs: std::collections::BTreeMap<Hash, Vec<Hash>> = std::collections::BTreeMap::new();
    for (name, hex) in &export.units {
        let unit = bytes_of(hex).and_then(|b| decode_preimage(&b).ok());
        match unit.map(|u| (hash_unit(&u).map(|h| to_hex(&h)), u)) {
            Some((Ok(h), unit)) if &h == name => {
                let children = child_refs(&unit);
                if let Ok(hash) = resolver.insert(unit) {
                    refs.insert(hash, children);
                }
            }
            _ => report.bad_units.push(name.clone()),
        }
    }

    let texts = Texts { blobs: &export.blobs, blob_bytes: &export.blob_bytes, files };
    let marks = Marks::new();
    let mut lefts = Vec::new();
    for (i, (entry, _)) in entries.iter().enumerate() {
        let earlier: Vec<&LogEntry> =
            entries[..i].iter().map(|(e, _)| e).filter(|e| !entry.run_id.is_empty() && e.run_id == entry.run_id).collect();
        let mut given = Given::default();
        let mut left = None;
        let came_to = match past_bounds(&entry.unit, &refs, &resolver) {
            Some(why) => Rerun::Differs(why),
            None => rerun(entry, &earlier, &texts, &holon_module::WithEngine::new(&resolver, holon_module::shared()), &marks, &mut given, &mut left),
        };
        lefts.push(left);
        if given.nodes > 0 {
            report.given.insert(entry.seq, given.nodes);
        }
        if given.id {
            report.ids_given.push(entry.seq);
        }
        report.reruns.push((entry.seq, came_to));
    }
    let events: Vec<crate::events::Event> = export.events.iter().filter_map(|l| serde_json::from_str(l).ok()).collect();
    report.states = check_states(&entries, &lefts, &events, &marks, &resolver);
    check_sends(&entries, &resolver, &marks, &mut report);
    report
}

/// Why the server could not have run `unit` (an entry's, hex), when it could not: it holds a
/// deployment's closure to the bounds `units::closure` checks before it runs one, so a unit past
/// them never ran there, and isn't evaluated here either. The evaluator recurses once or more for
/// each level a closure nests, so a record of units nested deeper than any the server runs could
/// otherwise exhaust this process's stack. Nor does it run a closure with a unit that doesn't
/// validate.
fn past_bounds(unit: &str, refs: &std::collections::BTreeMap<Hash, Vec<Hash>>, resolver: &MapResolver) -> Option<String> {
    use crate::units::{closure_depth, invalid, MAX_CLOSURE_DEPTH, MAX_CLOSURE_UNITS};
    let root = parse_hash(unit)?;
    // Counted as the server counts them: each unit present once; one missing is no unit.
    let mut seen = BTreeSet::new();
    let mut queue = vec![root];
    while let Some(h) = queue.pop() {
        let Some(children) = refs.get(&h) else { continue };
        if !seen.insert(h) {
            continue;
        }
        if seen.len() > MAX_CLOSURE_UNITS {
            return Some(format!("its unit's closure has more than {MAX_CLOSURE_UNITS} units, more than the server runs"));
        }
        queue.extend(children.iter().copied());
    }
    if closure_depth(root, refs) > MAX_CLOSURE_DEPTH {
        return Some(format!("its unit nests more than {MAX_CLOSURE_DEPTH} units deep, deeper than the server runs"));
    }
    seen.iter().find_map(|h| Some(format!("its closure's unit {} {}, and the server doesn't run it", to_hex(h), invalid(resolver.resolve(h)?, resolver)?)))
}

/// What a request failed with before it left the host: it went nowhere.
const NOT_SENT: &[&str] = &["blocked_secret", "blocked_host", "blocked_scheme", "blocked_address", "blocked_channel", "revoked", "bad_url", "bad_header", "bad_request"];

/// Where the secrets went: each outbound request that carried a secret's value (the log marks it
/// in the request) went to a host its unit sends that secret to (core's `sent_secrets`, what the
/// approval showed). A request refused before it left went nowhere.
///
/// And where the tokens went: a token a call answered (`‹token:answer access_token #…›` in its
/// response) is what is answered to the secrets its request carried, or to those of a token it
/// carried; each request that carried it went to a host its unit sends what is answered to one of
/// them to (an `answer:` destination). A token whose request carried no secret's mark (one
/// computed from a secret, withheld) is held to every secret's.
fn check_sends(entries: &[(LogEntry, &str)], resolver: &impl holon_core::Resolver, marks: &Marks, report: &mut Report) {
    let mut sent_by_unit: BTreeMap<String, BTreeMap<String, BTreeSet<String>>> = BTreeMap::new();
    for (entry, _) in entries {
        // Each token this run got, and the secrets it was answered to.
        let mut answered: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for x in entry.rounds.iter().flat_map(|r| &r.exchanges) {
            let request = marks.read(&x.request, entry.escaped);
            let response = marks.read(&x.response, entry.escaped);
            let named = marks.secrets_in(&request);
            let tokens = marks.tokens_in(&request);
            let owners: BTreeSet<String> = named.iter().cloned().chain(tokens.iter().flat_map(|t| answered.get(t).cloned().unwrap_or_default())).collect();
            for token in marks.tokens_in(&response) {
                answered.entry(token).or_default().extend(owners.iter().cloned());
            }
            if named.is_empty() && tokens.is_empty() {
                continue;
            }
            if let Value::Record(r) = &response
                && let Some(Value::Text(error)) = r.get("error")
                && NOT_SENT.contains(&error.as_str())
            {
                continue;
            }
            let host = match &request {
                Value::Record(r) => match r.get("url") {
                    Some(Value::Text(url)) => outbound::authority_of(url).unwrap_or_default(),
                    _ => String::new(),
                },
                _ => String::new(),
            };
            let sent = sent_by_unit
                .entry(entry.unit.clone())
                .or_insert_with(|| parse_hash(&entry.unit).and_then(|h| resolver.resolve(&h)).map(|u| holon_core::sent_secrets(u, resolver)).unwrap_or_default());
            if !named.is_empty() {
                report.secret_requests += 1;
            }
            for name in named {
                if !sent.get(&name).is_some_and(|hosts| hosts.contains(&host)) {
                    report.secrets_outside.push((entry.seq, name, host.clone()));
                }
            }
            if !tokens.is_empty() {
                report.token_requests += 1;
            }
            let shown = format!("{}{host}", holon_core::ANSWER);
            for token in tokens {
                let owners = answered.get(&token).filter(|o| !o.is_empty()).cloned().unwrap_or_else(|| sent.keys().cloned().collect());
                if !owners.iter().any(|name| sent.get(name).is_some_and(|to| to.contains(&shown))) {
                    let name = token.split(" #").next().unwrap_or(&token).to_string();
                    report.tokens_outside.push((entry.seq, name, host.clone()));
                }
            }
        }
    }
}

/// Whether a log entry is of a run: a request no route took ran nothing.
fn ran(e: &LogEntry) -> bool {
    !e.run_id.is_empty()
}

/// When a run ran, to the millisecond, its commit included (`duration_ms` is taken as it returns).
fn span(e: &LogEntry) -> (u64, u64) {
    (e.at_ms, e.at_ms + e.duration_ms.max(0.0).ceil() as u64)
}

/// Whether an event puts a state in place: a deploy, a restore, a stop and run again.
fn moves_state(ev: &crate::events::Event) -> bool {
    matches!(ev.kind.as_str(), "deploy" | "restore" | "lifecycle")
}

/// Whether two runs ran at once, give or take a millisecond.
fn beside(a: &LogEntry, b: &LogEntry) -> bool {
    let ((a0, a1), (b0, b1)) = (span(a), span(b));
    a0 <= b1 + 1 && b0 <= a1 + 1
}

/// Whether each run's state follows from the one before (docs/L4s/history-L4.md §6): it began
/// from the state the run before it left, and left what its writes make of the state it began
/// with. States compare by their snapshot's hash (`restore::snapshot_hash`), made again from each
/// run's `state_before`; a collection by its root, not its items.
///
/// Runs of one deployment run side by side and commit in turn, each only if the cells it could
/// read are as it found them (firing.rs), and its line is written after its commit. So a run that
/// ran beside another may have begun from a state an earlier one left or began with, and may have
/// left cells it couldn't read as the other wrote them: of those, only the cells it wrote are
/// checked, in a state a later run began from. Two runs committing close together may even be
/// numbered the other way round: the run before a run is the one whose commit it began after, as
/// its entry says (`recorder::Commit`), wherever its line is; and whether another commit landed
/// while it ran is said by their numbers. One that began after a commit older than every one here
/// has no run before it here, as the first hasn't. A line without them (logged before) is taken
/// by its place and its times.
///
/// A run of an unrecorded handler isn't in the log, nor a lazy one's whose record a power loss lost:
/// where every cell that doesn't follow is one such a handler may write, it is said unchecked, as
/// its author chose, not a break. A cell no such handler writes, a handler whose units aren't all
/// here, or a run before it that couldn't be run again leaves it a break.
fn check_states(entries: &[(LogEntry, &str)], lefts: &[Option<Left>], events: &[crate::events::Event], marks: &Marks, resolver: &impl holon_core::Resolver) -> Option<States> {
    if !entries.iter().any(|(e, _)| e.state_after.is_some()) {
        return None;
    }
    let mut states = States::default();
    let mut light: BTreeMap<&str, Light> = BTreeMap::new();
    for (e, _) in entries {
        light.entry(e.unit.as_str()).or_insert_with(|| Light::of(&e.unit, resolver));
    }
    let light = |e: &LogEntry| &light[e.unit.as_str()];
    // Each run's state as it began: its snapshot's hash, and the state.
    let began: Vec<Option<(String, State)>> = entries
        .iter()
        .map(|(e, _)| {
            let values: BTreeMap<String, Value> = e.state_before.iter().map(|(k, v)| (k.clone(), marks.read(v, e.escaped))).collect();
            if values.values().any(|v| marks.carried(v)) {
                return None;
            }
            let state = State::from_values(values).ok()?;
            Some((restore::snapshot_hash(&state).ok()?, state))
        })
        .collect();
    let hash = |i: usize| began[i].as_ref().map(|(h, _)| h);
    for (i, (b, _)) in entries.iter().enumerate() {
        if !ran(b) {
            continue;
        }
        let others = || entries.iter().enumerate().filter(move |(a, (e, _))| *a != i && ran(e));
        // Whether another commit landed between its beginning and its own, as their numbers say.
        let landed_beside = b.commit.as_ref().and_then(|c| Some(c.n? > c.after? + 1));
        let side_by_side = landed_beside.unwrap_or_else(|| others().any(|(_, (a, _))| beside(a, b)));
        let first_part = entries[..i].iter().position(|(e, _)| e.run_id == b.run_id);
        let resumed = first_part.is_some() || !b.events.is_empty();

        // Where it began.
        match hash(i) {
            None => states.unchecked("where it began: its state holds a secret's or a token's placeholder", b.seq),
            Some(h) if resumed => match first_part {
                Some(f) if hash(f) == Some(h) => states.began.push(b.seq),
                Some(f) => states.breaks.push((b.seq, format!("a resumed part begins from the state its run began with (run {}), and it didn't", entries[f].0.seq))),
                None => states.unchecked("where it began: it is a resumed part, and its run's first part isn't here", b.seq),
            },
            // The run whose commit it began after left the state it began from.
            Some(h) if began_after(entries, b).is_some_and(|p| p.state_after.as_ref() == Some(h)) => states.began.push(b.seq),
            Some(h) => {
                // The run before it that committed: one that failed didn't.
                match entries[..i].iter().rposition(|(e, _)| ran(e) && e.error.is_none()) {
                    None => states.unchecked("where it began: no run before it here", b.seq),
                    Some(pi) => match &entries[pi].0.state_after {
                        None => states.unchecked("where it began: the run before it kept no state it left (logged before rc.30)", b.seq),
                        Some(left) if left == h => states.began.push(b.seq),
                        // A deploy, a restore or a stop between them put a state in place: a
                        // restore of a run's state, the one that run left (deploy.rs).
                        Some(_) if let Some(ev) = events.iter().rfind(|ev| moves_state(ev) && entries[pi].0.seq < ev.run_seq && ev.run_seq <= b.seq) => {
                            let from = ev.from.as_deref().and_then(|f| f.strip_prefix("run ")).and_then(|n| n.parse::<u64>().ok());
                            match from.and_then(|n| entries.iter().find(|(e, _)| e.seq == n)).and_then(|(e, _)| e.state_after.as_ref().map(|s| (e.seq, s))) {
                                Some((_, put)) if ev.kind == "restore" && put == h => {
                                    states.began.push(b.seq);
                                    states.restored.push(b.seq);
                                }
                                Some((n, _)) if ev.kind == "restore" => {
                                    states.breaks.push((b.seq, format!("it began from a state other than the one the restore before it put back (run {n}'s)")))
                                }
                                _ => states.unchecked(
                                    "where it began: after a deploy, a stop, or a restore of a state not here, which put a state in place (the events say which)",
                                    b.seq,
                                ),
                            }
                        }
                        Some(_) => {
                            let p = &entries[pi].0;
                            let ends = span(b).1 + 1;
                            let from_before =
                                beside(p, b) && others().any(|(a, (e, _))| e.at_ms <= ends && (e.state_after.as_ref() == Some(h) || hash(a) == Some(h)));
                            let from_beside = others().any(|(_, (e, _))| beside(e, b) && e.state_after.as_ref() == Some(h));
                            if from_before || from_beside {
                                states.began.push(b.seq);
                                states.beside.push(b.seq);
                            } else if before_the_first(entries, b) {
                                states.unchecked("where it began: no run before it here", b.seq);
                            } else if let Some(why) = began[i]
                                .as_ref()
                                .and_then(|(_, now)| moved(p, lefts[pi].as_ref(), now))
                                .and_then(|moved| lightly(&moved, light(p), light(b)))
                            {
                                states.unchecked(why, b.seq);
                            } else {
                                states.breaks.push((b.seq, format!("it began from a state the run before it (run {}) didn't leave, nor a run beside it", p.seq)));
                            }
                        }
                    },
                }
            }
        }

        // What it left. An unrecorded handler's runs may have written the cells it writes beside
        // it, as another run may any.
        let (Some(after), Some(left)) = (&b.state_after, &lefts[i]) else { continue };
        let unrecorded = &light(b).unrecorded;
        // Numbered, an unrecorded handler's commits are counted too.
        let (ran_beside, side_by_side) = (side_by_side, side_by_side || (landed_beside.is_none() && !unrecorded.is_empty()));
        if resumed {
            states.unchecked("what it left: it is a resumed part, whose writes land on the state as it is by then", b.seq);
            continue;
        }
        if !side_by_side && left.collections.is_empty() {
            match restore::snapshot_hash(&left.state) {
                Ok(h) if &h == after => states.left.push(b.seq),
                _ => states.breaks.push((b.seq, "the state it left isn't the one it began with, its writes applied".into())),
            }
            continue;
        }
        // The state it left, as a run that began from it found it.
        let Some((_, then)) = began.iter().flatten().find(|(h, _)| h == after) else {
            states.unchecked(
                if ran_beside {
                    "what it left: it ran beside another run, and no run here began from the state it left"
                } else if side_by_side {
                    "what it left: an unrecorded handler that writes the state may have run beside it, and no run here began from the state it left"
                } else {
                    "what it left: it wrote a collection, and no run here began from the state it left"
                },
                b.seq,
            );
            continue;
        };
        let same = |k: &String| then.hash_of(k) == left.state.hash_of(k);
        let rest = || then.iter().chain(left.state.iter()).map(|(k, _)| k).filter(|k| !left.collections.contains(*k) && !unrecorded.contains(*k)).all(same);
        if !left.wrote.iter().all(same) {
            states.breaks.push((b.seq, "the state it left doesn't hold what it wrote".into()));
        } else if !ran_beside && !rest() {
            states.breaks.push((b.seq, "the state it left isn't the one it began with, its writes applied".into()));
        } else {
            states.left.push(b.seq);
        }
    }
    Some(states)
}

/// The run whose commit `b` began after (`recorder::Commit`), wherever its line is; none for a
/// line without one, one that began from the state as the server loaded it or put it in place,
/// or after a run the log doesn't hold.
fn began_after<'a>(entries: &'a [(LogEntry, &str)], b: &LogEntry) -> Option<&'a LogEntry> {
    let c = b.commit.as_ref()?;
    let after = c.after?;
    entries.iter().map(|(e, _)| e).find(|e| ran(e) && e.commit.as_ref().is_some_and(|p| p.cell == c.cell && p.n == Some(after)))
}

/// Whether `b` began after a run's commit older than every one here, before the first of those
/// landed: the run that made it is before the first here, as in a log trimmed to its newest runs,
/// where runs beside each other began after commits whose lines are gone. Not one that began from
/// the state as the server loaded it (`after` 0), whose run before it may be here.
fn before_the_first(entries: &[(LogEntry, &str)], b: &LogEntry) -> bool {
    let Some(c) = &b.commit else { return false };
    let Some(after) = c.after.filter(|a| *a > 0) else { return false };
    let numbered = entries.iter().map(|(e, _)| e).filter(|e| ran(e)).filter_map(|e| Some((e.commit.as_ref().filter(|d| d.cell == c.cell)?.n?, e)));
    numbered.min_by_key(|(n, _)| *n).is_some_and(|(n, first)| after < n && b.at_ms <= span(first).1 + 1)
}

/// The cells a run began with otherwise than the run before it (`p`) left them: as its writes made
/// of the state it began with, run again (`left`). A collection it wrote, whose root only its trie
/// gives, is taken as it is now if that makes the state `p` says it left; if not, it moved.
/// `None`: `p` couldn't be run again, so what it left isn't known.
fn moved(p: &LogEntry, left: Option<&Left>, now: &State) -> Option<BTreeSet<String>> {
    let left = left?;
    let keys = || left.state.iter().chain(now.iter()).map(|(k, _)| k);
    let mut moved: BTreeSet<String> = keys().filter(|k| !left.collections.contains(*k) && left.state.hash_of(k) != now.hash_of(k)).cloned().collect();
    let mut guess = left.state.clone();
    for k in &left.collections {
        match now.get(k) {
            Some(v) => guess.insert(k.clone(), v.clone()).ok()?,
            None => drop(guess.remove(k)),
        }
    }
    if restore::snapshot_hash(&guess).ok() != p.state_after {
        moved.extend(left.collections.iter().cloned());
    }
    Some(moved)
}

/// Why the cells that `moved` between two runs (of `p`'s version, then `b`'s) are left unchecked:
/// each is one a handler whose runs the log doesn't hold may write, an unrecorded one, or a lazy
/// one, whose last second of records a power loss may have lost (nothing in the log says one
/// came between them). `None`: one isn't, which is a break.
fn lightly(moved: &BTreeSet<String>, p: &Light, b: &Light) -> Option<&'static str> {
    let unrecorded = |k: &String| p.unrecorded.contains(k) || b.unrecorded.contains(k);
    let lazy = |k: &String| p.lazy.contains(k) || b.lazy.contains(k);
    if moved.is_empty() || !moved.iter().all(|k| unrecorded(k) || lazy(k)) {
        None
    } else if moved.iter().all(unrecorded) {
        Some("where it began: an unrecorded handler that writes the state may have run before it, which the log doesn't hold")
    } else {
        Some("where it began: a lazy handler that writes the state may have run before it, its record lost to a power loss")
    }
}

/// The cells a version's (its unit, hex) handlers whose runs the log may not hold may write: its
/// unrecorded handlers', and its lazy ones' (a collection's by its key in the state). A handler
/// whose units aren't all here may write any, so it gives none: what moved stays a break.
#[derive(Default)]
struct Light {
    unrecorded: BTreeSet<String>,
    lazy: BTreeSet<String>,
}

impl Light {
    fn of(unit: &str, resolver: &impl holon_core::Resolver) -> Light {
        let mut light = Light::default();
        let Some(server) = parse_hash(unit).and_then(|h| resolver.resolve(&h)).and_then(|u| read_server(u, resolver).ok()) else { return light };
        for (_, level, handler) in server.handlers() {
            let cells = match level {
                holon_core::LogLevel::Unrecorded => &mut light.unrecorded,
                holon_core::LogLevel::Lazy => &mut light.lazy,
                holon_core::LogLevel::Strict => continue,
            };
            let Some(unit) = resolver.resolve(&handler).filter(|u| whole(u, resolver)) else { continue };
            cells.extend(capabilities(unit, resolver).iter().filter_map(|c| match c {
                Capability::StateWrite(key) => Some(key),
                Capability::CollectionWrite(name) => Some(collections::cell_key(&name)),
                _ => None,
            }));
        }
        light
    }
}

/// Whether every unit `unit` references is here, and every one those reference.
fn whole(unit: &holon_core::Unit, resolver: &impl holon_core::Resolver) -> bool {
    let (mut seen, mut stack) = (BTreeSet::new(), vec![unit]);
    while let Some(unit) = stack.pop() {
        for h in holon_core::child_refs(unit) {
            if seen.insert(h) {
                match resolver.resolve(&h) {
                    Some(child) => stack.push(child),
                    None => return false,
                }
            }
        }
    }
    true
}

/// The events' chain: unbroken from the first (nothing before it, as they are kept for good) to
/// the head the export names.
fn check_events(export: &Export, witnessed: Option<&str>, report: &mut Report) {
    let mut head = String::new();
    for (i, line) in export.events.iter().enumerate() {
        let Ok(event) = serde_json::from_str::<crate::events::Event>(line) else {
            report.bad_event_lines.push(i + 1);
            continue;
        };
        if event.prev != head {
            report.event_breaks.push(event.seq);
        }
        head = line_hash(line);
        if witnessed == Some(head.as_str()) {
            report.witnessed_event = Some(event.seq);
        }
        report.events += 1;
    }
    report.events_head_matches = head == export.events_head;
}

/// Where the text of a file a run read comes from when its log entry keeps only the hash: the
/// export's blobs, or the files given with `--files`; its bytes, for a file read as bytes.
struct Texts<'a> {
    blobs: &'a BTreeMap<String, String>,
    blob_bytes: &'a BTreeMap<String, String>,
    files: &'a BTreeMap<String, PathBuf>,
}

/// Whether a logged read asked for its file's bytes (`folders::reads_bytes`).
fn reads_bytes(request: &SValue) -> bool {
    matches!(request, SValue::Record(pairs) if pairs.iter().any(|(k, v)| k == "bytes" && *v == SValue::Boolean(true)))
}

impl Texts<'_> {
    /// The text a logged file read answered: the entry's own, or found by its blob or its hash.
    /// `Err`: none here (what the run read), or a blob that isn't the file it says.
    /// `marks`, `escaped`: an answer the entry holds is read as the run had it (`Marks::read`).
    fn answer(&self, read: &recorder::LoggedRead, marks: &Marks, escaped: bool) -> Result<Option<Value>, Rerun> {
        let Some(file) = &read.file else { return Ok(read.answer.as_ref().map(|a| marks.read(a, escaped))) };
        if let Some(range) = &file.range {
            return self.range(read, file, range);
        }
        if let Some(answer) = &read.answer {
            return Ok(Some(marks.read(answer, escaped)));
        }
        let what = || what(read);
        if reads_bytes(&read.request) {
            return self.bytes(file, &what());
        }
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

    /// The answer of a read of a file's bytes its entry doesn't hold: from the export's bytes or a
    /// file given with `--files`, checked against the file's hash.
    fn bytes(&self, file: &recorder::LoggedFile, what: &str) -> Result<Option<Value>, Rerun> {
        if let Some(kept) = file.blob.as_ref().and_then(|b| self.blob_bytes.get(b)) {
            let bytes = holon_core::text_bytes(kept, "base64").ok_or_else(|| Rerun::Differs(format!("the bytes kept for {what} aren't base64")))?;
            if blake3::hash(&bytes).to_hex().as_str() != file.hash {
                return Err(Rerun::Differs(format!("the bytes kept for {what} aren't the file it read")));
            }
            return Ok(Some(Value::Bytes(bytes)));
        }
        match self.files.get(&file.hash) {
            Some(path) => std::fs::read(path).map(|b| Some(Value::Bytes(b))).map_err(|e| Rerun::Skipped(format!("it read {what}, and {} can't be read: {e}", path.display()))),
            None => Err(Rerun::Skipped(format!(
                "it read {what}, whose bytes the export doesn't hold (its folder keeps hashes, the server no longer keeps them, or the export left them out): give it with --files"
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

/// The handler a run of `kind` labelled `label` ran in `server`, and whether it is given what the
/// run was sent (a request, an action's input, a WebSocket block's record) or finds its start
/// among what it sourced (a clock's tick, a new file, a mail, a feed's event, a queue's message);
/// or, when `server` has none, what it would be, as a person names it. A resumed run's later part
/// is its first part's handler's, of whichever kind its label names. The one place a run's kind is
/// read for its handler, so `holon verify` and a replay run each kind again alike.
pub(crate) fn handler_of(server: &Server, kind: &str, label: &str) -> Result<(Hash, bool), &'static str> {
    if kind == "resume" {
        let started = ["request", "websocket", "tick", "file", "mail", "feed", "queue", "trigger"];
        return started.into_iter().find_map(|kind| handler_of(server, kind, label).ok()).ok_or("handler");
    }
    let found = match kind {
        "tick" => server.clocks.iter().find(|c| c.label == label).map(|c| c.handler),
        "file" => server.files.iter().find(|f| f.label == label).map(|f| f.handler),
        "mail" => server.mails.iter().find(|m| m.label == label).map(|m| m.handler),
        "feed" => server.feeds.iter().find(|f| f.label == label).map(|f| f.handler),
        "queue" => server.queues.iter().find(|q| q.label == label).map(|q| q.handler),
        "websocket" => server.websockets.iter().flat_map(|w| w.blocks()).find(|b| b.label == label).map(|b| b.handler),
        "trigger" => server.actions.iter().find(|a| a.label == label).map(|a| a.handler),
        _ => server.routes.iter().find(|r| format!("{} {}", r.method, r.path) == label).map(|r| r.endpoint),
    };
    let (what, sent) = match kind {
        "tick" => ("clock", false),
        "file" => ("file handler", false),
        "mail" => ("mail handler", false),
        "feed" => ("feed handler", false),
        "queue" => ("queue handler", false),
        "websocket" => ("WebSocket block", true),
        "trigger" => ("action", true),
        _ => ("route", true),
    };
    found.map(|handler| (handler, sent)).ok_or(what)
}

/// Run `entry` again from what it records (and what the earlier parts of the same run, `earlier`,
/// record: a resumed run's outbound answers and reads), and compare. `given` is set to what the
/// log's record stood in for: nodes (`Asserting`), and the run id.
fn rerun(
    entry: &LogEntry,
    earlier: &[&LogEntry],
    texts: &Texts,
    resolver: &impl holon_core::Resolver,
    marks: &Marks,
    given: &mut Given,
    left: &mut Option<Left>,
) -> Rerun {
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

    // What it ran: the handler its kind and label name (`handler_of`), with what was sent, or
    // finding its start among the sourced values; with no label, the whole unit.
    let (root, inputs) = match &entry.route {
        Some(label) => {
            let Ok(server) = read_server(deployed, resolver) else {
                return Rerun::Differs("it names a handler, but its unit isn't a server".into());
            };
            let (handler, sent) = match handler_of(&server, &entry.kind, label) {
                Ok(found) => found,
                Err(what) => return Rerun::Differs(format!("its unit has no {what} {label:?}")),
            };
            let Some(root) = resolver.resolve(&handler) else {
                return Rerun::Differs(format!("the handler of {label:?} isn't in the export"));
            };
            (root, if sent { vec![entry.request.as_ref().map(|r| marks.read(r, entry.escaped))] } else { Vec::new() })
        }
        None => (deployed, Vec::new()),
    };

    let mut extra = Vec::new();
    for (name, value) in &entry.sourced {
        let Some(cap) = durable::capability_named(name) else {
            return Rerun::Skipped(format!("it read {name}, which verify can't supply"));
        };
        extra.push((cap, marks.read(value, entry.escaped)));
    }
    let values: BTreeMap<String, Value> = entry.state_before.iter().map(|(k, v)| (k.clone(), marks.read(v, entry.escaped))).collect();
    let Ok(state) = State::from_values(values) else {
        return Rerun::Differs("its state isn't data a state holds, or doesn't encode".into());
    };

    // A secret stands in as its placeholder (a token too: `Logged::token`), as the marks the log
    // wrote do (`Marks`). A node that only passes one along computes the same; one that computes
    // with it can't be computed again, and what the log kept for it stands in (`Asserting`).
    let caps = capabilities(root, resolver);
    // With the version and fingerprint the run was given (an entry from before names none).
    let mut placeholders = Vec::new();
    for cap in caps.iter() {
        if let Capability::SecretRead(name) = cap {
            let shown = entry.secrets.get(&name).map_or(name.clone(), |shown| format!("{name} {shown}"));
            placeholders.push((Capability::SecretRead(name.clone()), Value::Text(marks.of("secret", &shown))));
        }
    }
    // Lent to its reads as they are (`run_rounds`): what it read from the world, read back from
    // the log, must be data, as its state is (`State` holds nothing else).
    let sourced: Vec<(Capability, &Value)> = state
        .iter()
        .map(|(k, v)| (Capability::StateRead(k.clone()), v))
        .chain(extra.iter().filter(|(_, v)| holon_host::is_storable(v)).chain(&placeholders).map(|(cap, value)| (cap.clone(), value)))
        .collect();

    let mut differs = Vec::new();
    let nonce = bytes_of(&entry.nonce).and_then(|b| <[u8; 16]>::try_from(b).ok());
    // A run that received a secret or a token (a resume route's in its path, a bearer token equal
    // to a secret) was given its id from the value, which the log never holds: it can't be made
    // again, and the log's stands, as what the log kept stands in for a node fed one. Only a mark
    // the log wrote counts: a look-alike a caller sent is computed with.
    given.id = inputs.iter().flatten().chain(state.iter().map(|(_, v)| v)).chain(extra.iter().map(|(_, v)| v)).any(|v| marks.carried(v));
    let id_matches = given.id || nonce.is_some_and(|nonce| {
        let id = |state: Option<&State>| to_hex(&run_id_of(deployment, &inputs, state, &extra, &nonce));
        id(Some(&state)) == entry.run_id || (state.is_empty() && id(None) == entry.run_id)
    });
    if !id_matches {
        differs.push("its run id");
    }

    // Its outbound requests are answered from the record (the earlier parts', then its own), never
    // sent again.
    let parts = || earlier.iter().copied().chain([entry]);
    let prior: Vec<Round> = parts().flat_map(|e| e.rounds.iter().map(|r| round_of(r, marks, e.escaped))).collect();
    // Its reads, its own first: a resumed part reads again what it shares (files, databases), and
    // its last part's answers are the ones it finished with.
    let mut reads: Vec<(Vec<u32>, Value, Option<Value>)> = Vec::new();
    let own = entry.reads.iter().map(|r| (r, entry.escaped));
    for (read, escaped) in own.chain(earlier.iter().rev().flat_map(|e| e.reads.iter().map(|r| (r, e.escaped)))) {
        let answer = match texts.answer(read, marks, escaped) {
            Ok(answer) => answer,
            Err(not) => return not,
        };
        reads.push((read.path.clone(), marks.read(&read.request, escaped), answer));
    }
    let mut waits = Logged {
        reads,
        events: entry.events.iter().map(|(k, v)| (k.clone(), marks.read(v, entry.escaped))).collect(),
        marks: marks.clone(),
    };
    let limits = Limits { run_budget: Duration::from_secs(3600), max_requests: usize::MAX, ..Limits::clock_run() };
    let mut host = RecordingHost::new();
    let mut observer = Asserting::from_timeline(&entry.timeline, marks, entry.escaped);
    let outcome = run_rounds(root, &inputs, &sourced, resolver, &mut host, &Offline, &limits, &mut observer, &mut waits, &prior);
    given.nodes = observer.stood_in.len();
    // A write of a secret's value is refused where it runs (firing.rs), and run again here with
    // its placeholder it can't be told from one that was performed (a file's text, logged by
    // hash, differs too): the run isn't checked.
    let keeps = |c: &holon_core::Capability| {
        use holon_core::Capability::*;
        matches!(c, StateWrite(_) | CollectionWrite(_) | DbWrite(_) | ChannelSend(_) | MailSend(_) | QueueSend(_) | FileWrite(_))
    };
    if host.performed.iter().any(|(c, p)| keeps(c) && marks.carries_secret(p)) {
        return Rerun::Skipped("it wrote a secret's value, which the server refuses: not run again".into());
    }

    // What it produced, as the log would write it, to compare with what the log wrote.
    let outputs: Vec<Option<SValue>> =
        outcome.outputs.iter().map(|o| o.as_ref().map(|v| marks.write(SValue::from_core(&folders::loggable_output(v))))).collect();
    if json(&outputs) != json(&entry.outputs) {
        differs.push("its outputs");
    }
    let performed: Vec<PerformedEntry> = host
        .performed
        .iter()
        .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: marks.write(SValue::from_core(&folders::loggable(c, p))) })
        .collect();
    if json(&performed) != json(&entry.performed) {
        differs.push("the effects it performed");
    }
    // A part that settled waiting: the packs it sent as it parked are what it settled.
    let progress: Vec<PerformedEntry> = if outcome.waiting.is_empty() { Vec::new() } else { channels::settled_sends(&outcome.outputs) }
        .iter()
        .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: marks.write(SValue::from_core(p)) })
        .collect();
    if json(&progress) != json(&entry.progress) {
        differs.push("the packs it sent as it waited");
    }
    // A file's status is the host's checks' too: a 304 for the request's cached copy, or what its
    // entry's error says they found.
    let answered = || {
        let answer = data::respond(outcome.outputs.first().and_then(Option::as_ref));
        let Some(file) = &answer.file else { return answer.status };
        let headers = match inputs.first() {
            Some(Some(Value::Record(r))) => r.get("headers"),
            _ => None,
        };
        let if_none_match = match headers {
            Some(Value::Record(h)) => match h.get("if-none-match") {
                Some(Value::Text(v)) => Some(v.as_str()),
                _ => None,
            },
            _ => None,
        };
        listeners::served_status(answer.status, file, if_none_match, entry.error.as_deref())
    };
    if entry.kind == "request"
        && let Some(status) = entry.status
        && answered() != status
    {
        differs.push("the status it answered");
    }
    if outcome.error.is_some() {
        differs.push("whether it finished");
    }

    // What its writes make of the state it began with, for `check_states`: a placeholder in it
    // would hash as itself, not as the value it stands for.
    if differs.is_empty() && !state.iter().any(|(_, v)| marks.carried(v)) && !host.performed.iter().any(|(_, p)| marks.carried(p)) {
        let mut after = state.clone();
        let (mut wrote, mut collections) = (BTreeSet::new(), BTreeSet::new());
        for (cap, payload) in &host.performed {
            match cap {
                // As the commit writes them (firing.rs).
                Capability::StateWrite(key) if after.insert(key.clone(), payload.clone()).is_ok() => {
                    wrote.insert(key.clone());
                }
                Capability::CollectionWrite(name) => {
                    collections.insert(collections::cell_key(name));
                }
                _ => {}
            }
        }
        *left = Some(Left { state: after, wrote, collections });
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

fn round_of(round: &recorder::LoggedRound, marks: &Marks, escaped: bool) -> Round {
    Round {
        exchanges: round
            .exchanges
            .iter()
            .map(|x| Exchange {
                path: x.path.clone(),
                request: marks.read(&x.request, escaped),
                response: marks.read(&x.response, escaped),
                start_us: x.start_us,
                end_us: x.end_us,
            })
            .collect(),
    }
}

/// Answers a run's waiting points, collection reads and file reads from its log entries (a file's
/// text from the export or `--files`); its tokens as their placeholder (`Marks`).
struct Logged {
    reads: Vec<(Vec<u32>, Value, Option<Value>)>,
    events: BTreeMap<String, Value>,
    marks: Marks,
}

impl Waits for Logged {
    fn read(&mut self, path: &[u32], request: &Value) -> Option<Value> {
        self.reads.iter().find(|(p, r, _)| p == path && r == request).and_then(|(_, _, a)| a.clone())
    }
    fn event(&self, name: &str) -> Option<Value> {
        self.events.get(name).cloned()
    }
    fn token(&mut self, name: &str) -> String {
        self.marks.of("token", name)
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
    marks: Marks,
}

impl Asserting {
    fn from_timeline(timeline: &[recorder::NodeTiming], marks: &Marks, escaped: bool) -> Asserting {
        let mut rounds: BTreeMap<Vec<u32>, u32> = BTreeMap::new();
        let mut recorded = BTreeMap::new();
        for t in timeline {
            let Some(outputs) = &t.outputs else { continue };
            if rounds.get(&t.path).is_some_and(|&r| r > t.round) {
                continue;
            }
            rounds.insert(t.path.clone(), t.round);
            recorded.insert(t.path.clone(), outputs.iter().map(|o| o.as_ref().map(|v| marks.read(v, escaped))).collect());
        }
        Asserting { timer: recorder::Timer::new(), recorded, stood_in: BTreeSet::new(), unrecorded: BTreeSet::new(), marks: marks.clone() }
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
    fn asserted(&mut self, path: &[u32], unit: &holon_core::Unit, inputs: &[Option<&Value>]) -> Option<Vec<Option<Value>>> {
        if matches!(unit.body, holon_core::UnitBody::Composite { .. }) || !inputs.iter().flatten().any(|v| self.marks.carried(v)) {
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

/// The placeholders of values the log never holds, as a run being checked has them. Each mark the
/// log wrote (`‹secret:name v3 #a1b2c3d4›`, `‹token:name›`) stands in as
/// `‹<tag>|secret:name v3 #a1b2c3d4›`, with a tag
/// made for this check, and so does each secret and token the run gets again: text a caller sent
/// that only looks like a mark (which the log escaped, `secrets::write_marked`) is read back as
/// the text it was, and computed with like any other.
#[derive(Clone)]
struct Marks {
    /// `‹<tag>|`: what starts a placeholder.
    start: String,
}

impl Marks {
    fn new() -> Marks {
        let mut tag = [0u8; 16];
        getrandom::fill(&mut tag).expect("the OS random source");
        Marks { start: format!("‹{}|", hex_of(&tag)) }
    }

    /// The placeholder of a secret's (`kind` "secret") or a token's value.
    fn of(&self, kind: &str, name: &str) -> String {
        format!("{}{kind}:{name}›", self.start)
    }

    /// The secrets whose placeholder is in `v` (as [`Marks::read`] gives it), by name.
    fn secrets_in(&self, v: &Value) -> BTreeSet<String> {
        let start = format!("{}secret:", self.start);
        let found = std::cell::RefCell::new(BTreeSet::new());
        secrets::value_holds(v, &|s: &str| {
            for (i, _) in s.match_indices(start.as_str()) {
                let rest = &s[i + start.len()..];
                let name = &rest[..rest.find([' ', '›']).unwrap_or(rest.len())];
                found.borrow_mut().insert(name.to_string());
            }
            false
        });
        found.into_inner()
    }

    /// The tokens whose placeholder is in `v` (as [`Marks::read`] gives it): each as its mark
    /// reads, `answer access_token #a1b2c3d4`, so one token is told from another by its print.
    fn tokens_in(&self, v: &Value) -> BTreeSet<String> {
        let start = format!("{}token:", self.start);
        let found = std::cell::RefCell::new(BTreeSet::new());
        secrets::value_holds(v, &|s: &str| {
            for (i, _) in s.match_indices(start.as_str()) {
                let rest = &s[i + start.len()..];
                found.borrow_mut().insert(rest[..rest.find('›').unwrap_or(rest.len())].to_string());
            }
            false
        });
        found.into_inner()
    }

    /// A value the log wrote (its entry `escaped` or not), as the run is run again with it.
    fn read(&self, v: &SValue, escaped: bool) -> Value {
        let mut v = v.clone();
        secrets::texts_mut(&mut v, &mut |s| {
            *s = secrets::read_marked(s, escaped)
                .into_iter()
                .map(|(t, mark)| if mark { format!("{}{}", self.start, &t['‹'.len_utf8()..]) } else { t })
                .collect()
        });
        v.into_core()
    }

    /// A value the run produced again, as the log would write it: its placeholders as marks, a
    /// look-alike escaped.
    fn write(&self, mut v: SValue) -> SValue {
        secrets::texts_mut(&mut v, &mut |s| {
            if !s.contains('‹') {
                return;
            }
            let mut pieces: Vec<(String, bool)> = Vec::new();
            let mut rest = s.as_str();
            while let Some(i) = rest.find(&self.start) {
                let after = &rest[i + self.start.len()..];
                let Some(end) = after.find('›') else { break };
                pieces.push((rest[..i].to_string(), false));
                pieces.push((format!("‹{}", &after[..end + '›'.len_utf8()]), true));
                rest = &after[end + '›'.len_utf8()..];
            }
            pieces.push((rest.to_string(), false));
            let pieces: Vec<(&str, bool)> = pieces.iter().map(|(t, m)| (t.as_str(), *m)).collect();
            *s = secrets::write_marked(&pieces).0;
        });
        v
    }

    /// Whether `v` carries a secret's placeholder anywhere, a record's keys included.
    fn carries_secret(&self, v: &Value) -> bool {
        // A credential a call answered to a request that held a secret (`secrets::answered_tokens`)
        // is refused a write as the secret is.
        let secret = |t: &str| t.contains(&format!("{}secret:", self.start)) || t.contains(&format!("{}token:answer ", self.start));
        match v {
            Value::Text(t) => secret(t),
            Value::List(items) => items.iter().any(|v| self.carries_secret(v)),
            Value::Record(fields) => fields.iter().any(|(k, v)| secret(k) || self.carries_secret(v)),
            Value::Sum { payload, .. } | Value::Effect { payload, .. } => self.carries_secret(payload),
            Value::Bytes(b) => secrets::bytes_hold(b, &secret),
            Value::Number(_) | Value::Boolean(_) => false,
        }
    }

    /// Whether `v` carries a placeholder anywhere.
    fn carried(&self, v: &Value) -> bool {
        match v {
            Value::Text(t) => t.contains(&self.start),
            Value::List(items) => items.iter().any(|v| self.carried(v)),
            Value::Record(fields) => fields.values().any(|v| self.carried(v)),
            Value::Sum { payload, .. } | Value::Effect { payload, .. } => self.carried(payload),
            Value::Bytes(b) => secrets::bytes_hold(b, &|t: &str| t.contains(&self.start)),
            Value::Number(_) | Value::Boolean(_) => false,
        }
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
    let export: Export = match read_export(Path::new(&file)) {
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

const USAGE: &str = "usage: holon verify <export.json> [--head HEX] [--files DIR]

  Checks a deployment's exported runs (holon export), offline: that its run log's hash chain is
  unbroken, that every unit is what its hash says, and that every run, run again from its log
  entry, produces what the log says it did. Exits 0 when it all checks, 1 when anything doesn't.

  --head HEX               a head a witness holds: check it is in the export, so no run up to
                           it was changed or removed
  --files DIR              where the files the runs read are, for those whose log keeps only
                           their hashes: each is found by its hash, wherever it is under DIR

examples:
  holon verify shop-runs.json
  holon verify shop-runs.json --head \"$(cat witness-head.txt)\"
  holon verify file-audit-runs.json --files /srv/inbox";

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
    if r.events > 0 || !r.event_breaks.is_empty() || !r.bad_event_lines.is_empty() {
        out += &if !r.event_breaks.is_empty() {
            format!("  events: {}, chain BROKEN at event {}: changed, removed or inserted outside holon\n", r.events, list(&r.event_breaks))
        } else {
            format!("  events: {}, chain unbroken from the first\n", r.events)
        };
        if !r.bad_event_lines.is_empty() {
            out += &format!("  event lines {} aren't events (cut short by a crash, or changed)\n", list(&r.bad_event_lines));
        }
    }
    if !r.events_head_matches {
        out += "  events head: DOESN'T MATCH the newest event: the export was changed\n";
    }
    match (r.witnessed_event, head) {
        (Some(seq), Some(h)) if r.event_breaks.iter().any(|b| *b <= seq) => {
            out += &format!("  witnessed head {}: event {seq}, but the events' chain breaks before it, so events up to it were changed\n", short(h))
        }
        (Some(seq), Some(h)) => out += &format!("  witnessed head {}: event {seq}, so no event up to it was changed or removed\n", short(h)),
        _ => {}
    }
    match (&r.witnessed, head) {
        (Some(None), Some(_)) if r.witnessed_event.is_some() => {}
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
    if !r.ids_given.is_empty() {
        out += &format!(
            "  {} checked given the run id the log kept (made from a secret or a resume token the run received; not computed again)\n",
            count_of(&r.ids_given)
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
    if r.secret_requests > 0 && r.secrets_outside.is_empty() {
        out += &format!(
            "  secrets: {} outbound request{} carried one, each to a host its approval shows it going to\n",
            r.secret_requests,
            if r.secret_requests == 1 { "" } else { "s" }
        );
    }
    for (seq, name, host) in &r.secrets_outside {
        out += &format!("  run {seq} SENT the secret {name:?} to {host}, where its approval doesn't show it going\n");
    }
    if r.token_requests > 0 && r.tokens_outside.is_empty() {
        out += &format!(
            "  tokens: {} outbound request{} carried what a call answered to a secret, each to a host its approval shows that going to\n",
            r.token_requests,
            if r.token_requests == 1 { "" } else { "s" }
        );
    }
    for (seq, name, host) in &r.tokens_outside {
        out += &format!("  run {seq} SENT a token ({name}) to {host}, where its approval doesn't show what is answered to its secret going\n");
    }
    if let Some(s) = &r.states {
        let mut how = Vec::new();
        if !s.beside.is_empty() {
            how.push(format!("{} of them beside another run, from a state a run before or beside them left or began with", s.beside.len()));
        }
        if !s.restored.is_empty() {
            how.push(format!("{} of them after a restore, from the state it put back", s.restored.len()));
        }
        let beside = if how.is_empty() { String::new() } else { format!(" ({})", how.join("; ")) };
        out += &format!(
            "  state: {} runs began from the state the run before them left{beside}, {} left what their writes make of the one they began with\n",
            s.began.len(),
            s.left.len()
        );
        for (why, seqs) in &s.unchecked {
            out += &format!("  state not checked ({}), {why}\n", count_of(seqs));
        }
        for (seq, how) in &s.breaks {
            out += &format!("  run {seq}'s state DOESN'T FOLLOW: {how}\n");
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
    // Its owner's only, as the log it is a copy of.
    let written = match out.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(dir) => std::fs::create_dir_all(dir).and_then(|()| holon_host::write_atomically(&out, body.as_bytes())),
        None => holon_host::write_atomically(&out, body.as_bytes()),
    };
    if let Err(e) = written {
        eprintln!("holon export: can't write {}: {e}", out.display());
        return 1;
    }
    let head = export.head.get(..12).unwrap_or(&export.head);
    // Where it is in full, so the hint below runs from any folder.
    let path = std::path::absolute(&out).unwrap_or(out).display().to_string();
    let arg = if path.contains(' ') { format!("\"{path}\"") } else { path.clone() };
    let events = match export.events.len() {
        0 => String::new(),
        1 => " and its 1 event".to_string(),
        n => format!(" and its {n} events"),
    };
    println!("wrote {path}: {} runs of {slug:?}{events}, head {head}", export.lines.len());
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

const EXPORT_USAGE: &str = "usage: holon export <slug> [options]

  Writes a deployment's runs to a file for holon verify: its run log exactly as written, and
  every unit its runs ran. With them come the texts of the files they read that their log keeps
  only by hash (a big file, or part of one); it says what it left out.

  --to FOLDER              the folder to write in (default this one; made if it isn't there)
  --as NAME                the file's name (default <slug>-runs.json)
  --max-files-bytes N      ship the files' texts up to N bytes in all (64K, 10M, 2G), the
                           oldest runs' first
  --without-files          ship none of them: holon verify checks those runs with --files
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon export shop
  holon export shop --to backups --as shop-2026-11.json
  holon export file-audit --max-files-bytes 10M

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_files_budget_is_bytes_or_k_m_g() {
        assert_eq!([budget_of("1048576"), budget_of("64K"), budget_of("10m"), budget_of("2GB"), budget_of("0")], [Some(1 << 20), Some(64 << 10), Some(10 << 20), Some(2 << 30), Some(0)]);
        assert_eq!([budget_of("lots"), budget_of("-1"), budget_of("K"), budget_of("99999999999G")], [None; 4]);
    }

    /// A record's unit nested deeper than the server runs is refused before it is run again, as
    /// the server refuses it before it runs: the bound `units::closure` holds, counted the same.
    #[test]
    fn a_unit_nested_past_the_servers_bound_is_not_run_again() {
        use crate::units::MAX_CLOSURE_DEPTH;
        let mut refs = std::collections::BTreeMap::new();
        let leaf = [7u8; 32];
        refs.insert(leaf, vec![]);
        // A chain of `levels` units over the leaf, the leaf counted: its root's hash (made up; the
        // bound reads only the references).
        let chain = |refs: &mut std::collections::BTreeMap<Hash, Vec<Hash>>, levels: usize| {
            let mut below = leaf;
            for k in 1..levels {
                let mut h = [0u8; 32];
                h[..8].copy_from_slice(&(levels as u64).to_le_bytes());
                h[8..16].copy_from_slice(&(k as u64).to_le_bytes());
                refs.insert(h, vec![below]);
                below = h;
            }
            to_hex(&below)
        };
        let at = chain(&mut refs, MAX_CLOSURE_DEPTH);
        let past = chain(&mut refs, MAX_CLOSURE_DEPTH + 1);
        assert_eq!(past_bounds(&at, &refs, &MapResolver::new()), None);
        assert_eq!(past_bounds(&past, &refs, &MapResolver::new()), Some(format!("its unit nests more than {MAX_CLOSURE_DEPTH} units deep, deeper than the server runs")));
        // A unit the export doesn't hold, or a name that isn't a hash, is left to the rerun to say.
        assert_eq!(past_bounds(&to_hex(&[9u8; 32]), &refs, &MapResolver::new()), None);
        assert_eq!(past_bounds("nope", &refs, &MapResolver::new()), None);
    }

    /// Nor is one whose closure holds a unit `validate` refuses, which the server doesn't run.
    #[test]
    fn a_unit_that_doesnt_validate_is_not_run_again() {
        use holon_core::{Edge, Port, Type, Unit, UnitBody, BOUNDARY};
        let mut resolver = MapResolver::new();
        let clock = resolver.insert(Unit { params: vec![], ..holon_core::interface_of("clock").unwrap() }).unwrap();
        let out = vec![Port { name: "out".into(), ty: Type::Number }];
        let bad = Unit { body: UnitBody::Composite { children: vec![clock], edges: vec![Edge { from_child: 0, from_port: 3, to_child: BOUNDARY, to_port: 0 }] }, params: vec![], inputs: vec![], outputs: out.clone() };
        let bad = resolver.insert(bad).unwrap();
        let good = Unit { body: UnitBody::Composite { children: vec![clock], edges: vec![Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 }] }, params: vec![], inputs: vec![], outputs: out };
        let good = resolver.insert(good).unwrap();
        let wrapper = resolver.insert(Unit { body: UnitBody::Composite { children: vec![bad], edges: vec![] }, params: vec![], inputs: vec![], outputs: vec![] }).unwrap();
        let refs = std::collections::BTreeMap::from([(clock, vec![]), (bad, vec![clock]), (good, vec![clock]), (wrapper, vec![bad])]);
        for root in [bad, wrapper] {
            let why = past_bounds(&to_hex(&root), &refs, &resolver).expect("refused");
            assert!(why.contains(&to_hex(&bad)) && why.contains("has no such output") && why.ends_with("the server doesn't run it"), "{why}");
        }
        assert_eq!(past_bounds(&to_hex(&good), &refs, &resolver), None);
    }
}
