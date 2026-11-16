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
//! A run that failed is run again too: what it computed before it failed, and what its writes made
//! of its state, are checked as any run's; its status may be its failure's. Some runs can't be run
//! again at all: one that panicked (its record says what it did by then isn't in it, and is checked
//! to), one whose such node the log keeps no outputs for (computed from a secret, which it never
//! keeps; too large; or too deep), one that failed, and run again gets further (a limit of the server's stopped it, or dropped what it did), one
//! that read a file whose text neither the export nor `--files` holds (its folder keeps only
//! hashes). They are counted, not failed, and an export with any checks only so far: `holon
//! verify` exits 3, not 0. So does one with a run whose state isn't checked (`States::unchecked`):
//! nothing in it fails, and not everything in it was checked. One that failed may say less than it does run again, never other: the
//! same run id, the outputs it keeps as computed, what it did and sent the start of what it does,
//! and its status that or a failure's; one that says other differs. A request no route took, and a
//! run turned away before it began, ran nothing: there is nothing to run again, and their entries
//! are checked to say they did nothing.

use std::collections::BTreeSet;

use crate::*;
use holon_core::Observer;
use holon_host::{Exchange, Limits, Performer, Waiting, Waits};
use recorder::{line_hash, LogEntry};

mod replay;
pub(crate) use replay::{handle_get_replay, Replays};
#[cfg(test)]
pub(crate) use replay::{replay, Granted};

/// What an export says it is.
pub(crate) const FORMAT: &str = "holon-runs/1";

/// Why a run isn't run again when a node it needs computed something from a secret: the log keeps
/// none of that (`firing::without_secret_derived`), and only the secret's value could make it again.
pub(crate) const WITHHELD_NODE: &str = "it computed a value from a secret, which the log never keeps: only the secret could compute it again";

/// What of a run's line `holon verify` doesn't hold it to by running the run again, by field (a
/// field's path, as `tests::changes_of` names it, `*` any index), and what it is as `describe`
/// says it: the server's account of when, where and for whom it ran, which no run again can make.
/// Each is held by the chain alone, and so by a witness's head (`--head`): changed and the lines
/// chained again, it passes without one, and fails with one. Anything else changed in a line is
/// found (`tests::no_line_changed_and_chained_again_passes_as_checked`, audit R26).
///
/// A commit's number and cell say which run's state it began after and whether others landed
/// beside it: changed, verify may check less of its state, and says which runs it didn't. `by`
/// is the server's account of its caller, no more; an approval's decisions are held to it
/// (whoever started the run decides only a step that takes them, `decided_as`). `grant` names the
/// grant it ran under by its hash: the export holds the units the runs ran, not the grants.
pub(crate) const NOT_HELD: &[(&str, &str)] = &[
    ("at_ms", "when it began"),
    ("duration_ms", "how long it took"),
    ("timeline/*/start_us", "when each step began"),
    ("timeline/*/end_us", "when each step ended"),
    ("session", "its run session"),
    ("commit/cell", "which load of the state it committed to"),
    ("commit/n", "its commit's number"),
    ("retries", "how many times it ran again before its commit landed (its tries)"),
    ("by", "who called it (the server's account of its caller)"),
    ("grant", "the grant it ran under (by its hash)"),
    ("rounds/*/exchanges/*/start_us", "when each call began"),
    ("rounds/*/exchanges/*/end_us", "when each call ended"),
    // What only whoever holds the secret can hold: verify holds their form, each mark to the
    // start of a hash, and one commitment for one value across the export (`listed_secrets`).
    ("derived/*", "the rest of each hash of a value computed from a secret, past what its mark shows (only the secret makes the value again)"),
    ("committed", "the commitments to its secrets (only a secret and its salt make one again)"),
];

/// What a mark stands for, as a reason for a run not run again ends: said once, here.
pub(crate) const MARK_STANDS_FOR: &str = "only a text of 6 characters or more is carried through a replay";

/// Why a run that came out other than its entry says isn't said to differ, if the log left out
/// what it would take to run it again: the outputs of a node that may compute from a secret,
/// which the run ran ([`withheld_why`]), or of a node this check asked for (`unrecorded`) that
/// was too large or too deep to keep. None: the log kept all it keeps, and the run differs.
fn left_out(root: &holon_core::Unit, resolver: &impl Resolver, entry: &LogEntry, unrecorded: &BTreeSet<Vec<u32>>) -> Option<String> {
    let fed = taint::SecretFed::of(root, resolver);
    let derived_missing = entry.timeline.iter().any(|t| t.outputs.is_none() && fed.derives(&t.path)) || unrecorded.iter().any(|p| fed.derives(p));
    if entry.withheld && derived_missing {
        return Some(withheld_why(root, resolver, &fed, entry));
    }
    // A line that says it marked every value the run computed from a secret kept each step that
    // made one (`secrets::born`): one without is taken out, and the run differs.
    if derived_missing {
        return None;
    }
    (!unrecorded.is_empty()).then(|| "it computed with a secret or a resume token at a node the log keeps no outputs for (too large, or too deep)".into())
}

/// Why the log withheld what `entry`'s run computed from a secret, so that it isn't run again:
/// which leaf, from which secret, and why no mark stands for what it gave. The leaf is one that
/// computes something of its own from a secret (`SecretFed::computes`) and ran; one whose output
/// is declared as something a mark can't be (a number, a true or false, a list) is named before
/// one that gives a text. A run that computed where the log follows no steps says that.
fn withheld_why(root: &holon_core::Unit, resolver: &impl Resolver, fed: &taint::SecretFed, entry: &LogEntry) -> String {
    if !fed.followed_to(recorder::Timer::DEPTH) {
        return "it computed with a secret inside a function applied to each item, inside a module, or deeper than the log records a run's steps: the values made there can't each be marked, so the log withholds them, and only the secret could compute them again".into();
    }
    let unit_at = |path: &[u32]| {
        path.iter().try_fold(root, |unit, &child| match &unit.body {
            holon_core::UnitBody::Composite { children, .. } => resolver.resolve(children.get(child as usize)?),
            _ => None,
        })
    };
    // What a leaf's declared output says it gives, where that is no text.
    let gives = |unit: &holon_core::Unit, path: &[u32]| {
        unit.outputs.iter().enumerate().filter(|(port, _)| fed.output(path, *port).is_derived()).find_map(|(_, port)| match &port.ty {
            holon_core::Type::Number => Some("a number"),
            holon_core::Type::Boolean => Some("a true or false"),
            holon_core::Type::List(_) | holon_core::Type::Product(_) => Some("a list"),
            holon_core::Type::Record => Some("a record"),
            _ => None,
        })
    };
    let ran: Vec<(&[u32], &holon_core::Unit)> = fed.computing().filter(|path| entry.timeline.iter().any(|t| t.path == *path)).filter_map(|path| Some((path, unit_at(path)?))).collect();
    let Some((path, unit, what)) = ran.iter().find_map(|(path, unit)| Some((*path, *unit, Some(gives(unit, path)?)))).or(ran.first().map(|(path, unit)| (*path, *unit, None))) else {
        return WITHHELD_NODE.into();
    };
    let leaf = match &unit.body {
        holon_core::UnitBody::Primitive { name } => format!("`{name}`"),
        _ => "a step".into(),
    };
    let from: Vec<&str> = entry.secrets.keys().filter(|name| taint::SecretFed::of_secret(root, resolver, name).derives(path)).map(String::as_str).collect();
    let from = match from.as_slice() {
        [] => "a secret".to_string(),
        [name] => format!("the secret {name}"),
        names => format!("the secrets {}", names.join(", ")),
    };
    match what {
        Some(what) => format!("{leaf} computed {what} from {from}: {MARK_STANDS_FOR}"),
        None => format!("{leaf} computed from {from} a value no mark stands for (too short, or not a text): {MARK_STANDS_FOR}"),
    }
}

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
/// in all (oldest run first), or none with `?without_files=1`. With `?run=N`, run N alone (web
/// console L4, Q3): its line as written, which names the hash of the line before it, and what it
/// ran and read; checked offline as any export, it runs again to its entry, and its hash is the
/// head. Not its timeline: its events are the whole log's.
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
    let run = match request.get_param("run").map(|v| v.parse::<u64>()) {
        None => None,
        Some(Ok(n)) => Some(n),
        Some(Err(_)) => return err(400, "run is a run's number"),
    };
    let (log, redactor, events, events_head) = match deployments.lock().unwrap().get(slug) {
        Some(dep) => (std::sync::Arc::clone(&dep.log), crate::secrets::Redactor::of(&dep.secrets), dep.events.lines(), dep.events.head().to_string()),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    // The lines and their head under one lock, so the export's chain ends at its head.
    let (lines, head) = log.with(|log| (log.lines(), log.head().to_string()));
    let (lines, head, events, events_head, file) = match run {
        None => (lines, head, events, events_head, format!("{slug}-runs.json")),
        Some(n) => match lines.into_iter().find(|l| seq_of(l) == Some(n)) {
            Some(line) => (vec![line.clone()], line_hash(&line), Vec::new(), String::new(), format!("{slug}-run-{n}.json")),
            None => return err(404, format!("{slug:?}'s log holds no run {n}")),
        },
    };
    match export_of(store, folders, Some(&redactor), slug, lines, head, events, events_head, budget) {
        Ok(export) => Response::json(&export).with_additional_header("Content-Disposition", format!("attachment; filename=\"{file}\"")),
        Err(e) => err(500, e),
    }
}

/// The run a log's line is of, read without the rest of it.
fn seq_of(line: &str) -> Option<u64> {
    #[derive(Deserialize)]
    struct Seq {
        seq: u64,
    }
    serde_json::from_str::<Seq>(line).ok().map(|s| s.seq)
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
/// The export at `path`: plain as `holon export` writes it, gzipped, or sealed as the server
/// keeps one on disk (a backup's archive, a removed deployment's records; at_rest.rs §5.5), which
/// `keys` opens: asked for only then, so a plain export needs none.
pub(crate) fn read_export(path: &Path, keys: impl FnOnce() -> Result<std::sync::Arc<crate::at_rest::Keyring>, String>) -> Result<Export, String> {
    let mut bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if crate::at_rest::is_sealed(&bytes) {
        let keys = keys()?;
        bytes = crate::at_rest::open_export(&keys, &bytes).map(|mut plain| std::mem::take(&mut *plain)).map_err(|e| format!("it is sealed, and doesn't open: {e}"))?;
    }
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
    /// It can't be run again: why. The export then checks only so far (`Report::complete`).
    Skipped(String),
    /// Nothing ran, and its entry says it did nothing: why (no route took the request, or the run
    /// was turned away before it began).
    NothingRan(String),
}

/// What checking an export found.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) deployment: String,
    /// The runs in it, oldest and newest seq.
    pub(crate) runs: usize,
    pub(crate) first: Option<u64>,
    pub(crate) last: Option<u64>,
    /// Whether the oldest run follows one no longer in the export (the log was trimmed, or it is
    /// one run's), and the hash of the line it names before it.
    pub(crate) trimmed: bool,
    pub(crate) after: String,
    /// Lines that aren't runs (1-based): changed into something else. An export never holds one a
    /// crash cut short (the log leaves out a line that doesn't open, recorder.rs `lines`), so
    /// one here fails it: left out, it would let the lines that are runs pass as the whole log
    /// (audit R26: an export of lines that are none read `0 runs … ok`).
    pub(crate) bad_lines: Vec<usize>,
    /// Runs whose line isn't one the server writes, by seq, and how (`ill_formed`): numbered
    /// out of order, of a kind it doesn't run, a request's method, URL or status not its own, a
    /// value no run holds, a run said lost that is here, its commit numbered other than commits
    /// are. Each is a line changed and chained again, which running it again may not show: a
    /// field it doesn't compute, or one that only chooses what it is compared with (audit R26).
    pub(crate) malformed: Vec<(u64, String)>,
    /// Runs that don't follow the line before them.
    pub(crate) breaks: Vec<u64>,
    /// Runs whose lines the server couldn't write, as the line after them says
    /// (`LogEntry::lost_since`): the first and the last of each run of them. Not a break: the
    /// chain goes on from the line before them.
    pub(crate) lost: Vec<(u64, u64)>,
    /// Runs missing between two lines that no line says were lost: the first and the last of each.
    /// The server numbers runs one after another and says each it couldn't write, so a gap nothing
    /// says is runs removed, the lines after them chained again.
    pub(crate) gaps: Vec<(u64, u64)>,
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
    /// secret or a token): by seq, each such node, so the report names what it took on trust.
    pub(crate) given: BTreeMap<u64, Vec<Stood>>,
    /// The runs whose run id was taken from the log, not made again: it was made from a secret or
    /// a resume token the run received (in its request, its state or what it read), which the log
    /// never holds.
    pub(crate) ids_given: Vec<u64>,
    /// The runs that called out: what each call was answered is as the line says, held by the
    /// chain alone (and a witnessed head), for every call, with a secret or without.
    pub(crate) called_out: Vec<u64>,
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
    /// The runs that began from nothing, as a log's first does: an empty state, after no commit,
    /// in an export from the log's first line ([`began_from_nothing`]).
    pub(crate) fresh: Vec<u64>,
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

/// A secret's mark, as its parts: `‹secret:name v3 #a1b2c3d4›` is the name, 3 and the
/// fingerprint. None for a mark of another kind, or one from before marks named a version.
fn secret_mark(mark: &str) -> Option<(&str, u32, &str)> {
    let (name, shown) = mark.strip_prefix("‹secret:")?.strip_suffix('›')?.split_once(' ')?;
    let (version, print) = shown_parts(shown)?;
    Some((name, version, print))
}

/// `v3 #a1b2c3d4`, as an entry's list of secrets has it, as the version and the fingerprint.
fn shown_parts(shown: &str) -> Option<(u32, &str)> {
    let (version, print) = shown.strip_prefix('v')?.split_once(" #")?;
    Some((version.parse().ok()?, print))
}

/// Whether what `entry` says of its secrets holds together, and with the runs before it: why
/// not, if it doesn't.
///
/// - Its list is well made: a kind that is one, a commitment that is a SHA-256's hex.
/// - A mark anywhere in its line that names the version the run was given of a secret kept here
///   carries the fingerprint its list has for it: one version, one value. (Not held of a
///   reference, whose value can change under a version while a run goes on.) And two marks of
///   one other version carry one fingerprint.
/// - One value has one commitment: the same owner, name, version and fingerprint, in any run of
///   the export, is the same commitment (`committed`: the first seen of each).
///
/// - What it lists as computed from a secret is hashes: each 64 hex digits, in order, none twice;
///   and a mark of such a value anywhere in its line (`‹derived #7c1e9a02›`) is the start of one
///   of them, so each mark is held to a whole hash the chain keeps.
///
/// It doesn't say the commitment is the value's: that takes the value and its salt.
fn listed_secrets(entry: &LogEntry, line: &str, committed: &mut BTreeMap<(bool, String, u32, String), (u64, String)>) -> Option<String> {
    for (name, c) in &entry.committed {
        if !matches!(c.kind.as_str(), "local" | "reference") || c.commitment.len() != 64 || !c.commitment.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Some(format!("what its entry says the secret {name:?} is held to isn't a kind and a commitment"));
        }
        let Some((version, print)) = entry.secrets.get(name).and_then(|shown| shown_parts(shown)) else {
            return Some(format!("its entry commits to the secret {name:?} and doesn't list which version it was given"));
        };
        match committed.get(&(c.server, name.clone(), version, print.to_string())) {
            Some((first, other)) if *other != c.commitment => {
                return Some(format!("the commitment its entry has for the secret {name} v{version} #{print}: run {first} recorded another for the same value"));
            }
            Some(_) => {}
            None => {
                committed.insert((c.server, name.clone(), version, print.to_string()), (entry.seq, c.commitment.clone()));
            }
        }
    }
    let hash = |h: &String| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !entry.derived.iter().all(hash) || !entry.derived.windows(2).all(|pair| pair[0] < pair[1]) {
        return Some("what its entry lists as computed from a secret isn't hashes, each once and in order".into());
    }
    if !line.contains("‹secret:") && !line.contains(secrets::DERIVED_MARK) {
        return None;
    }
    // Every mark in the line as the log wrote it: its request, its outputs, its nodes' records.
    let mut prints: BTreeMap<(String, u32), String> = BTreeMap::new();
    for (name, shown) in &entry.secrets {
        if entry.committed.get(name).is_some_and(|c| c.kind == "local")
            && let Some((version, print)) = shown_parts(shown)
        {
            prints.insert((name.clone(), version), print.to_string());
        }
    }
    let mut odd = None;
    let mut texts = vec![serde_json::from_str::<serde_json::Value>(line).ok()?];
    while let Some(v) = texts.pop() {
        match v {
            serde_json::Value::String(s) => {
                for (piece, mark) in secrets::read_marked(&s, entry.escaped) {
                    if mark && let Some(print) = piece.strip_prefix(secrets::DERIVED_MARK).and_then(|rest| rest.strip_suffix('›')) {
                        if print.len() != 8 || !entry.derived.iter().any(|h| h.starts_with(print)) {
                            odd = odd.or(Some(format!("a mark of a value computed from a secret reads #{print}, and its entry lists no hash that begins so")));
                        }
                        continue;
                    }
                    let Some((name, version, print)) = mark.then(|| secret_mark(&piece)).flatten() else { continue };
                    let kept = prints.entry((name.to_string(), version)).or_insert_with(|| print.to_string());
                    if kept != print {
                        odd = odd.or(Some(format!("a mark of the secret {name} v{version} reads #{print}, where its entry, or another mark of it, has #{kept}")));
                    }
                }
            }
            serde_json::Value::Array(items) => texts.extend(items),
            serde_json::Value::Object(fields) => texts.extend(fields.into_iter().flat_map(|(k, v)| [serde_json::Value::String(k), v])),
            _ => {}
        }
    }
    odd
}

/// What the log's record stood in for, running one run again.
#[derive(Default)]
struct Given {
    /// Nodes fed a secret or a token (`Asserting`).
    nodes: Vec<Stood>,
    /// The run id.
    id: bool,
    /// What was taken from the log for a leaf that is a function of its inputs, each of which
    /// tells its value from another (`Asserting::taken`).
    taken: Vec<Taken>,
}

/// What the log recorded for one leaf fed what it never holds, taken as recorded: what the leaf
/// and its inputs are (a hash of them), its name, and the outputs. Two of these for one leaf and
/// the same inputs, anywhere in an export, are the same outputs, or one of them isn't the leaf's.
#[derive(Clone)]
struct Taken {
    of: [u8; 32],
    leaf: String,
    outputs: Vec<Option<Value>>,
}

/// A node whose outputs were taken from the log, not computed again: its leaf, and what it was
/// fed that the log never holds. A unit names no composite it sits in, so the leaf is all there
/// is to name it by.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Stood {
    /// The leaf's name (`hmac_sha256`), or `a module`.
    pub(crate) leaf: String,
    /// The secrets in its inputs, each as the run's entry shows it (`api_token v3 #a1b2c3d4`).
    pub(crate) secrets: Vec<String>,
    /// The tokens in its inputs, each as its mark reads (`answer access_token #a1b2c3d4`).
    pub(crate) tokens: Vec<String>,
    /// The tainted keys in its inputs (`authorization`): values of the request's the host never
    /// keeps.
    pub(crate) tainted: Vec<String>,
    /// Whether its inputs held a value the run computed from a secret (`‹derived #7c1e9a02›`).
    /// Not which: such a mark is a run's own, and the same step in two runs reads as one here.
    pub(crate) derived: bool,
}

impl Stood {
    /// What it was fed, as the report says it: `the secret api_token v3 #a1b2c3d4`.
    fn fed(&self) -> String {
        let some = |one: &str, many: &str, items: &[String]| match items {
            [] => None,
            [item] => Some(format!("{one} {item}")),
            items => Some(format!("{many} {}", items.join(", "))),
        };
        let fed: Vec<String> = [
            some("the secret", "the secrets", &self.secrets),
            some("the token", "the tokens", &self.tokens),
            some("the request's", "the request's", &self.tainted).map(|t| t + ", which the log never keeps"),
            self.derived.then(|| "a value computed from a secret".to_string()),
        ]
        .into_iter()
        .flatten()
        .collect();
        if fed.is_empty() { "a placeholder".into() } else { fed.join(" and ") }
    }
}

impl Report {
    /// Whether the export checks: the chain unbroken up to its head (and the given one), every
    /// unit what it says, and every run that could be run again producing what the log says.
    pub(crate) fn ok(&self) -> bool {
        self.bad_lines.is_empty()
            && self.malformed.is_empty()
            && self.breaks.is_empty()
            && self.gaps.is_empty()
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

    /// Whether everything in it was checked: every run run again (none [`Rerun::Skipped`]), and
    /// every run's state (none [`States::unchecked`]). An export that checks but isn't complete
    /// has nothing found wrong in it, and not everything in it checked: `holon verify` exits 3,
    /// so a script reading its exit code doesn't take it as checked.
    pub(crate) fn complete(&self) -> bool {
        self.all_run_again() && self.states.as_ref().is_none_or(|s| s.unchecked.is_empty())
    }

    /// Whether every run was run again: none [`Rerun::Skipped`].
    fn all_run_again(&self) -> bool {
        !self.reruns.iter().any(|(_, r)| matches!(r, Rerun::Skipped(_)))
    }
}

/// Check `export`: its chain, its units, and each run, run again. `witnessed`: a head to find in
/// it (one a witness holds). `files`: files the runs read, by hash (`--files`), for reads the
/// export keeps only the hash of.
pub(crate) fn check(export: &Export, witnessed: Option<&str>, files: &BTreeMap<String, PathBuf>) -> Report {
    let mut report = Report { deployment: export.deployment.clone(), ..Report::default() };
    let marks = Marks::new();

    // The chain, as the server checks it when it opens a log (recorder.rs).
    let mut entries: Vec<(LogEntry, &str)> = Vec::new();
    let mut head = String::new();
    let mut chained = false;
    let mut last: Option<u64> = None;
    let mut numbered: BTreeMap<(String, u64), u64> = BTreeMap::new();
    for (i, line) in export.lines.iter().enumerate() {
        let Ok(entry) = crate::wire::read_json::<LogEntry>(line.as_bytes()) else {
            report.bad_lines.push(i + 1);
            continue;
        };
        if head.is_empty() {
            report.trimmed = !entry.prev.is_empty();
            report.after = entry.prev.clone();
        } else if (chained || !entry.prev.is_empty()) && entry.prev != head {
            report.breaks.push(entry.seq);
        }
        // Numbered after the line before it, and what it says was lost before it not here.
        if let Some(before) = last
            && entry.seq <= before
        {
            report.malformed.push((entry.seq, format!("it is numbered {}, after run {before}", entry.seq)));
        }
        match entry.lost_since {
            Some(from) if from >= entry.seq => report.malformed.push((entry.seq, format!("it says the runs from {from} were lost before it, and it is run {}", entry.seq))),
            Some(from) if last.is_some_and(|before| from <= before) => {
                report.malformed.push((entry.seq, format!("it says the runs from {from} were lost before it, and run {} is here", last.unwrap_or(0))))
            }
            _ => {}
        }
        if let Some(why) = ill_formed(&entry, &marks) {
            report.malformed.push((entry.seq, why));
        }
        // A commit is counted once in its state's count.
        if let Some(c) = &entry.commit
            && let Some(n) = c.n
            && let Some(other) = numbered.insert((c.cell.clone(), n), entry.seq)
        {
            report.malformed.push((entry.seq, format!("its commit is numbered {n}, as run {other}'s is")));
        }
        // The runs missing before it, but those it says were lost.
        let said = lost_before(&entry);
        if let Some(before) = last
            && let from = said.map_or(entry.seq, |(from, _)| from)
            && from > before + 1
        {
            report.gaps.push((before + 1, from - 1));
        }
        last = Some(entry.seq);
        report.lost.extend(said);
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
    let mut lefts = Vec::new();
    // What each leaf taken from the log gave, by the leaf and its inputs: the first run that
    // recorded it, and what it recorded.
    let mut taken: BTreeMap<[u8; 32], (u64, Vec<Option<Value>>)> = BTreeMap::new();
    // The commitment each value of a secret was recorded with: by whose secret it is, its name
    // and version and the value's fingerprint, the first run that recorded it, and what.
    let mut committed: BTreeMap<(bool, String, u32, String), (u64, String)> = BTreeMap::new();
    for (i, (entry, line)) in entries.iter().enumerate() {
        let earlier: Vec<&LogEntry> =
            entries[..i].iter().map(|(e, _)| e).filter(|e| !entry.run_id.is_empty() && e.run_id == entry.run_id).collect();
        let mut given = Given::default();
        let mut left = None;
        let came_to = match past_bounds(&entry.unit, &refs, &resolver) {
            Some(why) => Rerun::Differs(why),
            None => rerun(entry, &earlier, &texts, &holon_module::WithEngine::new(&resolver, holon_module::shared()), &marks, &mut given, &mut left),
        };
        lefts.push(left);
        // A leaf is a function of its inputs: what the log recorded for it here is what it
        // recorded for the same leaf and inputs in every other run, or one of the two isn't what
        // the leaf gave. Which one, only the secret could say; the later one is said to differ.
        let mut belied = None;
        for t in &given.taken {
            match taken.get(&t.of) {
                Some((first, outputs)) if *outputs != t.outputs => belied = belied.or(Some((t.leaf.clone(), *first))),
                Some(_) => {}
                None => {
                    taken.insert(t.of, (entry.seq, t.outputs.clone()));
                }
            }
        }
        let came_to = match (came_to, belied) {
            (Rerun::Differs(why), _) => Rerun::Differs(why),
            (_, Some((leaf, first))) => Rerun::Differs(format!("what the log recorded for {leaf}, taken from it: run {first} recorded other for the same inputs")),
            (came_to, None) => came_to,
        };
        // What it says of its secrets agrees with itself, and with the runs before it.
        let came_to = match (came_to, listed_secrets(entry, line, &mut committed)) {
            (Rerun::Differs(why), Some(also)) => Rerun::Differs(format!("{why}; {also}")),
            (Rerun::Differs(why), None) => Rerun::Differs(why),
            (_, Some(why)) => Rerun::Differs(why),
            (came_to, None) => came_to,
        };
        if !given.nodes.is_empty() {
            report.given.insert(entry.seq, given.nodes);
        }
        if given.id {
            report.ids_given.push(entry.seq);
        }
        if entry.rounds.iter().any(|r| !r.exchanges.is_empty()) {
            report.called_out.push(entry.seq);
        }
        report.reruns.push((entry.seq, came_to));
    }
    let events: Vec<crate::events::Event> = export.events.iter().filter_map(|l| serde_json::from_str(l).ok()).collect();
    report.states = check_states(&entries, &lefts, &events, &marks, &resolver, !report.trimmed);
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

/// Whether a log entry is of a run the state follows from: a request no route took ran nothing,
/// and nor did a run turned away before it began (it read no state, and left none). A run that
/// panicked ran, but its entry keeps nothing of it (`firing::panicked`): no state it began from or
/// left.
fn ran(e: &LogEntry) -> bool {
    !e.run_id.is_empty() && !e.error.as_deref().is_some_and(|e| e.ends_with(firing::TURNED_AWAY) || e.starts_with(firing::PANICKED))
}

/// When a run ran, to the millisecond, its commit included (`duration_ms` is taken as it returns).
fn span(e: &LogEntry) -> (u64, u64) {
    (e.at_ms, e.at_ms + e.duration_ms.max(0.0).ceil() as u64)
}

/// Whether an event puts a state in place: a deploy, a restore, a stop and run again. Not one
/// saying which state the server loaded after a crash (`Event::loaded`), which only the run that
/// began from that very state is taken after (`check_states`).
pub(crate) fn moves_state(ev: &crate::events::Event) -> bool {
    matches!(ev.kind.as_str(), "deploy" | "restore" | "lifecycle") && ev.loaded.is_none()
}

/// The runs whose lines the server couldn't write before `entry`'s, as it says: the first and the
/// last.
fn lost_before(entry: &LogEntry) -> Option<(u64, u64)> {
    entry.lost_since.filter(|from| *from < entry.seq).map(|from| (from, entry.seq - 1))
}

/// Whether two runs ran at once, give or take a millisecond.
fn beside(a: &LogEntry, b: &LogEntry) -> bool {
    let ((a0, a1), (b0, b1)) = (span(a), span(b));
    a0 <= b1 + 1 && b0 <= a1 + 1
}

/// Why a run with no run before it here isn't checked where it began, in an export that begins
/// after the log's first line: the runs before it would check it.
pub(crate) const NONE_BEFORE: &str = "where it began: no run before it here, as the export begins after the log's first run (the runs before it would check it)";

/// Why a run with no run before it here, in an export from the log's first line, isn't checked
/// where it began: its line says it began from a state, or after a commit, as no log's first run does.
pub(crate) const NOT_FROM_NOTHING: &str = "where it began: no run before it here, and not from nothing (an empty state, after no commit), as a log's first run begins";

/// Whether `b`'s line says it began from nothing, as a log's first run does: an empty state, after
/// no commit (`after` 0, or none in a line written before commits were numbered).
fn began_from_nothing(b: &LogEntry) -> bool {
    b.state_before.is_empty() && b.commit.as_ref().and_then(|c| c.after).unwrap_or(0) == 0
}

/// Why a run that began from the state the server loaded after a crash isn't checked where it began.
pub(crate) const CUT_OFF: &str =
    "where it began: from the state the server loaded after a crash, which a run the crash cut off between its commit and its line may have left (its events say so)";

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
/// has no run before it here, as the first hasn't. A line without them is taken by its place and
/// its times.
///
/// A run with no run before it here began, in an export from the log's first line
/// (`from_the_first`), from nothing: an empty state, after no commit ([`began_from_nothing`]); one
/// whose line says otherwise, or in an export that begins later (a trimmed log, one run's), isn't
/// checked where it began ([`NOT_FROM_NOTHING`], [`NONE_BEFORE`]).
///
/// A run the server cut off between its commit and its line, crashing, isn't in the log either:
/// the server, started again from the state it left, says so as an event naming that state
/// (`Event::loaded`, persist.rs), and the run after it here, if it began from that very state, is
/// said unchecked ([`CUT_OFF`]). One that began from another, or after a stop that left its mark
/// (stopping.rs), as no event then says, is a break.
///
/// A run of an unrecorded handler isn't in the log, nor a lazy one's whose record a power loss lost:
/// where every cell that doesn't follow is one such a handler may write, it is said unchecked, as
/// its author chose, not a break. A cell no such handler writes, a handler whose units aren't all
/// here, or a run before it that couldn't be run again leaves it a break. Nor is a run whose line
/// the server couldn't write, which the line after it says: a run after the one before it here,
/// and after it, may have begun from what it left, and is said unchecked.
fn check_states(
    entries: &[(LogEntry, &str)],
    lefts: &[Option<Left>],
    events: &[crate::events::Event],
    marks: &Marks,
    resolver: &impl holon_core::Resolver,
    from_the_first: bool,
) -> Option<States> {
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
    let lost: Vec<(u64, u64)> = entries.iter().filter_map(|(e, _)| lost_before(e)).collect();
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
                // The run before it that committed: one that failed did only if it left a state.
                match entries[..i].iter().rposition(|(e, _)| ran(e) && (e.error.is_none() || e.state_after.is_some())) {
                    None if from_the_first && began_from_nothing(b) => states.fresh.push(b.seq),
                    None => states.unchecked(if from_the_first { NOT_FROM_NOTHING } else { NONE_BEFORE }, b.seq),
                    Some(pi) => match &entries[pi].0.state_after {
                        None => states.unchecked(
                            "where it began: the run before it kept no state it left (its version held none, or the state, unchanged, couldn't be kept)",
                            b.seq,
                        ),
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
                                states.unchecked(if from_the_first { NOT_FROM_NOTHING } else { NONE_BEFORE }, b.seq);
                            } else if let Some(why) = began[i]
                                .as_ref()
                                .and_then(|(_, now)| moved(p, lefts[pi].as_ref(), now))
                                .and_then(|moved| lightly(&moved, light(p), light(b)))
                            {
                                states.unchecked(why, b.seq);
                            } else if lost.iter().any(|(from, to)| p.seq < *to && *from < b.seq) {
                                states.unchecked("where it began: a run between it and the run before it here, whose line the server couldn't write, may have left it", b.seq);
                            } else if events.iter().any(|ev| ev.loaded.as_ref() == Some(h) && p.seq < ev.run_seq && ev.run_seq <= b.seq) {
                                states.unchecked(CUT_OFF, b.seq);
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
        // Its writes landed on the state the commit before its own left (each commit's state is
        // kept, then counted, the state's lock held: firing.rs). A resumed part's land on the
        // state as it is by then, as one's beside another run's do.
        let on_the_one_before = b.commit.as_ref().and_then(|c| state_at(entries, lefts, &began, &c.cell, c.n?.checked_sub(1)?));
        if left.collections.is_empty()
            && let Some(then) = on_the_one_before
            && (resumed || side_by_side)
        {
            match written_on(then, left) {
                Some(h) if &h == after => states.left.push(b.seq),
                _ => states.breaks.push((b.seq, "the state it left isn't the one the commit before its own left, its writes applied".into())),
            }
            continue;
        }
        if resumed {
            states.unchecked("what it left: it is a resumed part, whose writes land on the state as it is by then, and the commit before its own isn't here", b.seq);
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

/// The state the commit numbered `k` of `cell` left (0: the one the server loaded), where this
/// log shows it whole: as a run that began after it found it, or as the run that made it left it
/// (its writes run again, or a run that began from it), by the hash its line names.
fn state_at<'a>(entries: &[(LogEntry, &str)], lefts: &'a [Option<Left>], began: &'a [Option<(String, State)>], cell: &str, k: u64) -> Option<&'a State> {
    let commit = |e: &LogEntry| e.commit.clone().filter(|c| c.cell == cell);
    let after_it = entries.iter().zip(began).find(|((e, _), _)| ran(e) && commit(e).is_some_and(|c| c.after == Some(k)));
    if let Some((_, Some((_, state)))) = after_it {
        return Some(state);
    }
    let (i, (made, _)) = entries.iter().enumerate().find(|(_, (e, _))| ran(e) && commit(e).is_some_and(|c| c.n == Some(k)))?;
    let kept = made.state_after.as_ref()?;
    began.iter().flatten().find(|(h, _)| h == kept).map(|(_, s)| s).or_else(|| {
        lefts[i].as_ref().filter(|l| l.collections.is_empty() && restore::snapshot_hash(&l.state).ok().as_ref() == Some(kept)).map(|l| &l.state)
    })
}

/// The hash of the state `left`'s writes make of `then`, as the commit writes them: every one, or
/// none if one can't be (firing.rs).
fn written_on(then: &State, left: &Left) -> Option<String> {
    let mut on = then.clone();
    for k in &left.wrote {
        let landed = match left.state.get(k) {
            Some(v) => on.insert(k.clone(), v.clone()).is_ok(),
            None => {
                on.remove(k);
                true
            }
        };
        if !landed {
            return restore::snapshot_hash(then).ok();
        }
    }
    restore::snapshot_hash(&on).ok()
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
        "request" => server.routes.iter().find(|r| format!("{} {}", r.method, r.path) == label).map(|r| r.endpoint),
        _ => return Err("handler of its kind"),
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
        return nothing_ran(entry, "no route took the request");
    }
    match &entry.error {
        Some(e) if e.ends_with(firing::TURNED_AWAY) => return nothing_ran(entry, "it was turned away before it began (a run lock)"),
        // As the server writes it (`firing::panicked`): nothing it did is in it, so nothing is
        // checked, and one that says it did something is no such entry.
        Some(e) if e.starts_with(firing::PANICKED) && did_something(entry) => {
            return Rerun::Differs("it panicked, and yet its entry says it did something".into());
        }
        Some(e) if e.starts_with(firing::PANICKED) => return Rerun::Skipped(format!("it failed: {e}")),
        _ => {}
    }
    let Some(deployment) = parse_hash(&entry.unit) else {
        return Rerun::Differs("its entry doesn't name its unit".into());
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

    // Its outbound requests are answered from the record, never sent again: the earlier parts' as
    // answered before it began, as the server answers them (and, for a run that ran again, its
    // earlier tries': `firing.rs`, a retry sends nothing twice); its own as it sends them, so it
    // takes its steps in the rounds it took them (each round's steps are in its timeline).
    let own: Vec<Round> = entry.rounds.iter().map(|r| round_of(r, marks, entry.escaped)).collect();
    // Its last try's rounds, the newest: as many as the rounds after the first its steps say.
    let last_try = match entry.retries {
        0 => own.len(),
        _ => entry.timeline.iter().map(|t| t.round as usize).max().unwrap_or(0).min(own.len()),
    };
    let (tried, sent) = own.split_at(own.len() - last_try);
    let prior: Vec<Round> = earlier.iter().flat_map(|e| e.rounds.iter().map(|r| round_of(r, marks, e.escaped))).chain(tried.iter().cloned()).collect();
    let network = Answering::new(sent.iter().flat_map(|r| &r.exchanges).map(|x| (x.path.clone(), x.request.clone(), x.response.clone())).collect());
    // Its reads, its own first: a resumed part reads again what it shares (files, databases), and
    // its last part's answers are the ones it finished with.
    let mut reads: Vec<(Vec<u32>, Value, Option<Value>)> = Vec::new();
    // The texts of the files it read that the log keeps by their hash, as a node carries them.
    let mut kept: Vec<String> = Vec::new();
    let own = entry.reads.iter().map(|r| (r, entry.escaped));
    for (read, escaped) in own.chain(earlier.iter().rev().flat_map(|e| e.reads.iter().map(|r| (r, e.escaped)))) {
        let answer = match texts.answer(read, marks, escaped) {
            Ok(answer) => answer,
            Err(not) => return not,
        };
        if read.file.is_some() && read.answer.is_none() {
            kept.extend(answer.as_ref().and_then(carried_text));
        }
        reads.push((read.path.clone(), marks.read(&read.request, escaped), answer));
    }
    let mut waits = Logged {
        reads,
        events: entry.events.iter().map(|(k, v)| (k.clone(), marks.read(v, entry.escaped))).collect(),
        reached: BTreeMap::new(),
        marks: marks.clone(),
    };
    let limits = rerun_limits();
    let mut host = RecordingHost::new();
    let mut observer = Asserting::from_timeline(&entry.timeline, marks, entry.escaped);
    let outcome = run_rounds(root, &inputs, &sourced, resolver, &mut host, &network, &limits, &mut observer, &mut waits, &prior);
    // A node computed on its placeholders gives what the run gave, written with them; the log
    // recording other for it is a record that isn't the run's.
    if observer.belied {
        differs.push("what a step computed on its marks gives, from what the log recorded for it");
    }
    // What is taken from the log can't be computed, but it can be what its leaf never gives.
    if observer.misshapen {
        differs.push("the shape of what the log recorded for a step taken from it");
    }
    // Each call its line records was made: by the node at its path, with that very request.
    if network.unasked() {
        differs.push("a call its line records that no node makes run again");
    }
    given.taken = observer.taken.values().cloned().collect();
    // Each node stood in for, its secrets as the run's entry shows them (their version and
    // fingerprint), as a request that carried one is said (`check_sends`).
    given.nodes = observer
        .stood_in
        .values()
        .map(|stood| Stood {
            secrets: stood.secrets.iter().map(|name| entry.secrets.get(name).map_or(name.clone(), |shown| format!("{name} {shown}"))).collect(),
            ..stood.clone()
        })
        .collect();
    // A write of a secret's value is refused where it runs (firing.rs), and run again here with
    // its placeholder it can't be told from one that was performed (a file's text, logged by
    // hash, differs too): the run isn't checked.
    let keeps = |c: &holon_core::Capability| {
        use holon_core::Capability::*;
        matches!(c, StateWrite(_) | CollectionWrite(_) | DbWrite(_) | ChannelSend(_) | MailSend(_) | QueueSend(_) | FileWrite(_))
    };
    if host.performed.iter().any(|(c, p)| keeps(c) && marks.carries_secret(p)) {
        return Rerun::Skipped("it wrote a secret's value, or one computed from it, which the server refuses: not run again".into());
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
    // What may be computed from a secret it was given is withheld as the server withholds it
    // (`firing::without_secret_derived`, `FiredRun::withholds_output`), from the same walk of
    // the same unit.
    let given_one = entry.secrets.keys().any(|name| caps.iter().any(|c| matches!(c, Capability::SecretRead(s) if s == *name)));
    let fed = if given_one { taint::SecretFed::of(root, resolver) } else { taint::SecretFed::default() };
    // A line that says its record withholds keeps nothing that may be computed from a secret,
    // and is one that computed with one: else it says other than the server writes.
    if entry.withheld && (fed.is_empty() || entry.timeline.iter().any(|t| t.outputs.is_some() && fed.derives(&t.path))) {
        return Rerun::Differs("its line says it withholds what the run computed from a secret, and keeps a step that may hold it, or computed with none".into());
    }
    // The steps it took, as the log keeps them.
    kept.extend(folders::written_texts(&host.performed).into_iter().filter_map(|t| carried_text(&Value::Text(t.to_string()))));
    kept.extend(folders::written_bytes(&host.performed).into_iter().filter_map(|b| carried_text(&Value::Bytes(b.to_vec()))));
    let again = std::mem::take(&mut observer.timer.timeline);
    // Where the line says the run marked every value computed from a secret, the steps it kept
    // whole for it (`secrets::born`, by the same walk).
    let whole = given_one && !entry.withheld;
    let steps = steps_of(&entry.timeline, &again, &|path| entry.withheld && fed.derives(path), &|path| whole && fed.holds(path), &kept, !observer.unrecorded.is_empty(), marks, entry.escaped);
    if !steps.whole {
        differs.push("the steps it took (its timeline)");
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
        && entry.error.is_none()
        && let Some(status) = entry.status
        && answered() != status
    {
        differs.push("the status it answered");
    }
    // What it answered, as its line keeps it, made again where its status is: a run that
    // finished. A kind that answers nothing keeps none.
    if entry.error.is_none() && !answered_as_said(entry, outcome.outputs.first().and_then(Option::as_ref), fed.derives_output(0), marks) {
        differs.push("what it answered");
    }
    // Where it waits: the points it settled waiting at, as it parked (`durable::park_if_waiting`),
    // or none, where parking it failed (its error says so).
    let waits_at: Vec<&str> = outcome.waiting.iter().map(|w| w.name.as_str()).collect();
    let parked: Vec<&str> = entry.parked.iter().map(String::as_str).collect();
    if parked != waits_at && (entry.error.is_none() || !parked.is_empty()) {
        differs.push("where it waits");
    }
    // What was delivered to it: each at a point it reached waiting, and an approval's what its
    // decisions deliver, taken as the server takes them, by the steps it asks for there and
    // whoever started it (its first part's line says who).
    let starter = earlier.iter().find(|e| e.kind != "resume").and_then(|e| e.by.as_deref());
    let delivered = waits.events.iter().find_map(|(name, event)| match waits.reached.get(name) {
        None => Some(format!("{name:?} was delivered, and it doesn't wait there")),
        Some(Some(steps)) => durable::StoredWaiting::decided_as(name, steps, event, starter).err(),
        Some(None) => None,
    });
    if delivered.is_some() {
        differs.push("what was delivered to it");
    }
    // A run that finished and changed its state kept the state it left (`firing::keep_state`), or
    // failed saying why: a line that keeps none is no such run's.
    let changed = host.performed.iter().any(|(c, p)| match c {
        Capability::StateWrite(key) => state.get(key) != Some(p),
        Capability::CollectionWrite(_) => true,
        _ => false,
    });
    if entry.error.is_none() && outcome.error.is_none() && changed && entry.state_after.is_none() {
        differs.push("the state it left (its line keeps none)");
    }
    // One that failed may have failed here too, or after it finished: at its commit, or past a
    // limit of the server's.
    if outcome.error.is_some() && entry.error.is_none() {
        differs.push("whether it finished");
    }

    // What its writes make of the state it began with, for `check_states`: a placeholder in it
    // would hash as itself, not as the value it stands for.
    if differs.is_empty() && !state.iter().any(|(_, v)| marks.carried(v)) && !host.performed.iter().any(|(_, p)| marks.carried(p)) {
        let mut after = state.clone();
        let (mut wrote, mut collections) = (BTreeSet::new(), BTreeSet::new());
        // As the commit writes them (firing.rs): every one, or none if one can't be, or if its
        // error says so (a collection's write was refused).
        let mut landed = !entry.error.as_deref().is_some_and(|e| e.contains(firing::STATE_NOT_WRITTEN));
        for (cap, payload) in &host.performed {
            match cap {
                Capability::StateWrite(key) => {
                    landed &= after.insert(key.clone(), payload.clone()).is_ok();
                    wrote.insert(key.clone());
                }
                Capability::CollectionWrite(name) => {
                    collections.insert(collections::cell_key(name));
                }
                _ => {}
            }
        }
        if !landed {
            (after, wrote, collections) = (state.clone(), BTreeSet::new(), BTreeSet::new());
        }
        *left = Some(Left { state: after, wrote, collections });
    }

    match differs.is_empty() {
        true => Rerun::Matched,
        // The log keeps no outputs for a node that may compute with a secret unless it has a mark
        // for every value the run computed from one, by the rule the server withholds them by
        // (`firing::without_secret_derived`): run again without them, such a run isn't the run
        // it was, whether or not a node here asked for what the log left out (one that reads a
        // secret inside a function applied to each item is asked for nothing). For other nodes,
        // the outputs were too large or too deep to keep. Each is a different thing to be told.
        false if let Some(why) = left_out(root, resolver, entry, &observer.unrecorded) => Rerun::Skipped(why),
        // Stopped by a limit verify isn't held to (its budget, the clock), or one past which the
        // server dropped what it did (a file's, a queue's size): run again, it goes further. So
        // its entry may say less than the run again does, never other: the same run, each output
        // it keeps as computed, what it did and sent the start of what it does again, and the
        // status it answered either that or a failure's. A changed line can take away, not change.
        false if let Some(e) = &entry.error => {
            let starts = |logged: &[PerformedEntry], again: &[PerformedEntry]| logged.len() <= again.len() && json(&logged) == json(&&again[..logged.len()]);
            let failure = |status: u16| status >= 500 || (status == 409 && e.starts_with(databases::UNMET));
            let less = id_matches
                && entry.outputs.len() <= outputs.len()
                && entry.outputs.iter().zip(&outputs).all(|(kept, again)| kept.is_none() || json(kept) == json(again))
                && starts(&entry.performed, &performed)
                && starts(&entry.progress, &progress)
                && steps.start
                && (entry.kind != "request" || entry.status.is_none_or(|status| failure(status) || answered() == status));
            match less {
                true => Rerun::Skipped(format!("it failed, and run again it gets further than its entry says (a limit of the server's stopped it, or dropped what it did): {e}")),
                false => Rerun::Differs(format!("{} (it failed, and its entry says other than what it does run again, not only less)", differs.join(", "))),
            }
        }
        false => Rerun::Differs(match delivered {
            Some(why) => format!("{} ({why})", differs.join(", ")),
            None => differs.join(", "),
        }),
    }
}

/// The limits a run is run again with, here and by `replay`: no clock or count of requests to
/// stop it, and the most steps, fuel and memory any grant gives a round (`max_steps`,
/// `max_fuel`, `max_memory`), so a run within its own runs again in full, and one stopped by its
/// own gets further, as a run past its budget does.
pub(crate) fn rerun_limits() -> Limits {
    let ceilings = Limits::ceilings();
    Limits {
        run_budget: Duration::from_secs(3600),
        max_requests: usize::MAX,
        max_steps: ceilings.max_steps,
        max_fuel: ceilings.max_fuel,
        max_memory: ceilings.max_memory,
        ..Limits::clock_run()
    }
}

/// The kinds of run the server logs, by what started it (`firing::run_entry`'s callers).
pub(crate) const KINDS: &[&str] = &["request", "trigger", "resume", "tick", "file", "mail", "feed", "queue", "websocket"];

/// How `entry` isn't a line the server writes, in what holds it together without running it
/// (`check` holds it to the lines beside it, `rerun` to what it does). Its kind is one the server
/// runs. A request says its method, URL and status, and the record its handler was given is the
/// one its method and URL make (`data::request_record`); no other kind says them but a
/// WebSocket's (its opening request's, its answer's) and an action's status (a failure's,
/// deploy.rs). What it read from the world it names as the server does, each once. Each value is
/// one a run can hold, as the log writes it (a record's keys once each, in order): one the log
/// reads back as another runs again as that other. Its commit is after
/// one before its own, a resumed part's after none (it began from its run's first), and it is
/// numbered once it kept the state it left. `None`: none of these.
fn ill_formed(entry: &LogEntry, marks: &Marks) -> Option<String> {
    let kind = entry.kind.as_str();
    if !KINDS.contains(&kind) {
        return Some(format!("its kind {kind:?} isn't one the server runs"));
    }
    let request = kind == "request";
    match (&entry.method, &entry.url) {
        (Some(_), Some(_)) if request || kind == "websocket" => {}
        (None, None) if !request => {}
        _ => return Some(format!("its method and URL aren't what a {kind}'s line says")),
    }
    match entry.status {
        None if request => return Some("it is a request's, and says no status".into()),
        Some(s) if kind == "trigger" && s < 500 => return Some(format!("an action's line says a status only when it failed, and it says {s}")),
        Some(_) if !matches!(kind, "request" | "trigger" | "websocket") => return Some(format!("a {kind}'s line says no status, and it says one")),
        _ => {}
    }
    if let (true, Some(method), Some(url), Some(record)) = (request, &entry.method, &entry.url, &entry.request) {
        let Value::Record(fields) = marks.read(record, entry.escaped) else {
            return Some("its request isn't a request's record".into());
        };
        let Value::Text(url) = marks.read(&SValue::Text(url.clone()), entry.escaped) else { unreachable!("a text reads as a text") };
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let Value::Record(made) = data::request_record(method, path, BTreeMap::new(), query, &[], "") else { unreachable!("a request's record") };
        // A value the log keeps out of it (a tainted query parameter) is marked where the log
        // found it, which a URL's percent-encoding may hide: only its method is then compared.
        let marked = marks.carried(&Value::Text(url.clone())) || fields.values().any(|v| marks.carried(v));
        let differs = |k: &str| fields.get(k) != made.get(k);
        if differs("method") || (!marked && (differs("path") || differs("query"))) {
            return Some("its request isn't the one its method and URL make".into());
        }
    }
    // What it read from the world, each once, by a name the server writes (`durable::capability_named`).
    let mut read = BTreeSet::new();
    for (name, _) in &entry.sourced {
        if durable::capability_named(name).is_none() {
            return Some(format!("it says it read {name}, which no run reads"));
        }
        if !read.insert(name) {
            return Some(format!("it says it read {name} twice"));
        }
    }
    let mut held = true;
    entry.clone().values_mut(&mut |v| held &= SValue::from_core(&v.clone().into_core()) == *v);
    if !held {
        return Some("a value in it isn't one a run holds (a record with a key twice, or out of order)".into());
    }
    // What was delivered to the points it waited at is given to a resumed part (durable.rs).
    if !entry.events.is_empty() && kind != "resume" {
        return Some("it says what was delivered to points it waited at, and it isn't a resumed part".into());
    }
    if let Some(c) = &entry.commit {
        match (c.after, c.n) {
            (Some(after), Some(n)) if after >= n => return Some(format!("its commit, {n}, isn't after the one it began after, {after}")),
            (Some(_), _) if kind == "resume" => return Some("it is a resumed part, and its commit says it began after one".into()),
            (None, _) if kind != "resume" => return Some("its commit doesn't say which it began after".into()),
            (_, None) if entry.state_after.is_some() => return Some("it kept the state it left, and its commit isn't numbered".into()),
            // Each commit counted names the state it left (`firing::keep_state`), kept or not.
            (_, Some(_)) if entry.state_after.is_none() => return Some("its commit is numbered, and it names no state it left".into()),
            _ => {}
        }
    } else if entry.state_after.is_some() {
        return Some("it kept the state it left, and says no commit".into());
    }
    None
}

/// Whether `entry`'s answer (`response_body`) is the one its run gives with `first`, its first
/// output run again, as its line keeps it (listeners.rs, deploy.rs, websockets.rs): a request's
/// body, or the file it answered with; an action's answer as JSON; a WebSocket block's reply, its
/// open block's answer, a close block's none. Withheld where it may be computed from a secret
/// (`derived`), the run marking none of it (`FiredRun::withholds_output`). A kind that answers
/// nothing keeps none. A body past what the line keeps (64 KiB), cut at a length a mark's value
/// isn't known at, is compared up to its first mark.
fn answered_as_said(entry: &LogEntry, first: Option<&Value>, derived: bool, marks: &Marks) -> bool {
    let kept = |body: &[u8]| recorder::body_text(body);
    let made = match entry.kind.as_str() {
        "request" => {
            let answer = data::respond(first);
            Some(answer.file.as_ref().map_or_else(|| kept(&answer.body), data::Served::line))
        }
        "trigger" => Some(deploy::action_json(deploy::action_answer(first.cloned()).1.as_ref()).unwrap_or_default()),
        "websocket" => match entry.route.as_deref().and_then(|r| r.rsplit(' ').next()) {
            Some("open") => Some(kept(&data::respond(first).body)),
            Some("message") => Some(kept(websockets::reply_of(first).unwrap_or_default().as_bytes())),
            _ => None,
        },
        _ => None,
    };
    let (Some(made), Some(said)) = (made, &entry.response_body) else {
        return entry.response_body.is_none() && made_none(entry);
    };
    if derived && said == taint::WITHHELD {
        return true;
    }
    let SValue::Text(made) = marks.write(SValue::Text(made)) else { unreachable!("a text writes as a text") };
    match made.ends_with('…') && made.contains('‹') {
        true => made.split('‹').next() == said.split('‹').next(),
        false => made == *said,
    }
}

/// Whether `entry`'s kind keeps no answer: a WebSocket's close block, and every kind but a
/// request, an action and a WebSocket's.
fn made_none(entry: &LogEntry) -> bool {
    match entry.kind.as_str() {
        "request" | "trigger" => false,
        "websocket" => entry.route.as_deref().and_then(|r| r.rsplit(' ').next()) == Some("close"),
        _ => true,
    }
}

/// Whether an entry says its run did anything: computed, did, sent, called out, read or wrote.
fn did_something(entry: &LogEntry) -> bool {
    entry.outputs.iter().any(Option::is_some)
        || !entry.performed.is_empty()
        || !entry.progress.is_empty()
        || !entry.rounds.is_empty()
        || !entry.reads.is_empty()
        || entry.state_after.is_some()
}

/// What an entry of a run that didn't run comes to: nothing, if it says it did nothing.
fn nothing_ran(entry: &LogEntry, why: &str) -> Rerun {
    match did_something(entry) {
        true => Rerun::Differs(format!("{why}, and yet its entry says it did something")),
        false => Rerun::NothingRan(why.into()),
    }
}

/// Whether the steps a run took again agree with the ones its entry says it took.
struct Steps {
    /// Each agrees, and there are as many.
    whole: bool,
    /// The entry's are the start of them, as far as a limit let the run go (`steps_of`): run
    /// again, a run a limit stopped goes further.
    start: bool,
}

/// Whether the steps a run took again (`again`, as the timer took them) are those its entry says it
/// took (`logged`), as the log keeps them (`firing::entry_with`): the same nodes in the same order
/// and rounds, each with what it gave, as the log writes it, and why it gave nothing (a thrown
/// message, a module's error). Where the log keeps no outputs, a node agrees with any: one that may
/// compute from a secret the run was given (`withheld`; unless the run marks each value computed
/// from one, and then the log's stand in for them, `Asserting`); one that carries the text of a
/// file the log keeps by its hash (`kept`, `firing::without_kept_files`); and one larger than the
/// log keeps (`Timer::MAX_RECORDED_BYTES`), which a value held by its mark may be, its own size not
/// known here. Why a node that may compute from a secret gave nothing is withheld too. Where the
/// run again asked for what the log keeps no outputs for (`short`: a node fed a secret deeper than
/// the log follows a run's steps), a node that gave nothing again is taken as the log says.
///
/// Before, verify read a node's logged outputs only where it stood them in for a node fed a secret:
/// the steps a run's view shows, each step's values and why one failed, were the line's say-so
/// (audit R26).
#[allow(clippy::too_many_arguments)]
fn steps_of(
    logged: &[recorder::NodeTiming],
    again: &[recorder::NodeTiming],
    withheld: &dyn Fn(&[u32]) -> bool,
    kept_whole: &dyn Fn(&[u32]) -> bool,
    kept: &[String],
    short: bool,
    marks: &Marks,
    escaped: bool,
) -> Steps {
    let carries_kept = |o: &[Option<SValue>]| serde_json::to_string(o).is_ok_and(|j| kept.iter().any(|t| j.contains(t.as_str())));
    let nothing = |n: &recorder::NodeTiming| n.outputs.as_ref().is_none_or(|o| o.iter().all(Option::is_none));
    let agrees = |l: &recorder::NodeTiming, a: &recorder::NodeTiming| {
        if l.path != a.path || l.round != a.round {
            return false;
        }
        if short && nothing(a) {
            return true;
        }
        let held = withheld(&l.path);
        let outputs = match (&l.outputs, &a.outputs) {
            (Some(l), Some(a)) => json(l) == json(&a.iter().map(|o| o.clone().map(|v| marks.write(v))).collect::<Vec<_>>()),
            (None, None) => true,
            (None, Some(a)) if held || carries_kept(a) => true,
            // A step holding a mark: too large to keep as the run had it (the mark is shorter
            // than the value). Not a step the line says the run kept whole (`kept_whole`: it
            // marked every value computed from a secret, and the walk says this step may give
            // one; `secrets::born` asks the same): one without is taken out. Where the walk says
            // clean, a mark is a copy a caller sent or a host echoed, and explains it.
            (None, Some(a)) => !kept_whole(&l.path) && a.iter().flatten().any(|v| marks.carried(&v.clone().into_core())),
            (Some(l), None) => l.iter().flatten().any(|v| marks.carried(&marks.read(v, escaped))),
        };
        let reason = match (&l.reason, &a.reason) {
            (Some(r), _) if held => r == taint::WITHHELD,
            (None, _) if held => true,
            (Some(l), Some(a)) => marks.write(SValue::Text(a.clone())) == SValue::Text(l.clone()),
            (None, None) => true,
            _ => false,
        };
        outputs && reason
    };
    // Where a limit stopped it, the steps it took agree up to the first that gave nothing, where it
    // stopped. After it, each gave nothing (what the round was in the middle of, a step budget
    // ending it), or they are the same steps (a module that ran out gave nothing, and the round
    // went on without it: what they computed from it is less, not checked).
    let agreed = logged.iter().zip(again).take_while(|(l, a)| agrees(l, a)).count();
    let same = |(l, a): (&recorder::NodeTiming, &recorder::NodeTiming)| l.path == a.path && l.round == a.round;
    let after = || logged[agreed..].iter().zip(&again[agreed..]);
    let start = logged.len() <= again.len()
        && (agreed == logged.len() || (nothing(&logged[agreed]) && (logged[agreed..].iter().all(nothing) || after().all(same))));
    Steps { whole: agreed == logged.len() && logged.len() == again.len(), start }
}

/// A file's text, lines or bytes as a node that carries them is written (`firing::without_kept_files`):
/// a text as JSON writes it inside its quotes, lines as their list's JSON, bytes as their base64.
/// None for nothing.
fn carried_text(v: &Value) -> Option<String> {
    match v {
        Value::Text(t) if !t.is_empty() => serde_json::to_string(t).ok().map(|q| q[1..q.len() - 1].to_string()),
        Value::List(l) if !l.is_empty() => serde_json::to_string(&SValue::from_core(v)).ok(),
        Value::Bytes(b) if !b.is_empty() => holon_core::bytes_text(b, "base64"),
        _ => None,
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
    /// Each point it reached waiting that it was delivered an event at, by name: an approval's
    /// steps, none for an event's.
    reached: BTreeMap<String, Option<String>>,
    marks: Marks,
}

impl Waits for Logged {
    fn read(&mut self, path: &[u32], request: &Value) -> Option<Value> {
        self.reads.iter().find(|(p, r, _)| p == path && r == request).and_then(|(_, _, a)| a.clone())
    }
    fn event(&self, name: &str) -> Option<Value> {
        self.events.get(name).cloned()
    }
    fn event_at(&mut self, at: &Waiting) -> Option<Value> {
        let event = self.event(&at.name);
        if event.is_some() {
            self.reached.insert(at.name.clone(), at.approval.clone());
        }
        event
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
///
/// Not every leaf fed a placeholder needs its record: one that only moves the value, or says
/// whether it is another, gives with the placeholder what it gave with the value
/// (`holon_core::taint::on_a_mark`). Those are computed, and what the log recorded for them is
/// held to what they give: a record that says other differs (`belied`). Unless a node before it
/// in the run couldn't be computed nor stood in for (`unrecorded`): what this run again computes
/// after that one isn't the run's, so the record is taken, as for any other leaf.
struct Asserting {
    timer: recorder::Timer,
    /// Each node's outputs, by path and by the round the log recorded them in.
    recorded: BTreeMap<Vec<u32>, BTreeMap<u32, Vec<Option<Value>>>>,
    /// The round being run.
    round: u32,
    /// Whether a node computed on its placeholders gave other than the log recorded for it.
    belied: bool,
    /// What was taken from the log for a leaf that is a function of its inputs alone, where
    /// every placeholder in them tells its value from another's: by path, for `check` to hold
    /// the export's other runs to.
    taken: BTreeMap<Vec<u32>, Taken>,
    /// Whether something taken from the log isn't what its leaf can give: the wrong number of
    /// outputs, one of another type than the leaf declares, a hash that isn't 32 bytes.
    misshapen: bool,
    /// The nodes stood in for, by path: each one's leaf, and the secrets (by name) and tokens in
    /// its inputs.
    stood_in: BTreeMap<Vec<u32>, Stood>,
    /// The nodes that needed standing in for and that the log keeps no outputs for (computed from
    /// a secret, larger than the log keeps, or deeper than it records).
    unrecorded: BTreeSet<Vec<u32>>,
    marks: Marks,
}

impl Asserting {
    fn from_timeline(timeline: &[recorder::NodeTiming], marks: &Marks, escaped: bool) -> Asserting {
        let mut recorded: BTreeMap<Vec<u32>, BTreeMap<u32, Vec<Option<Value>>>> = BTreeMap::new();
        for t in timeline {
            let Some(outputs) = &t.outputs else { continue };
            recorded.entry(t.path.clone()).or_default().insert(t.round, outputs.iter().map(|o| o.as_ref().map(|v| marks.read(v, escaped))).collect());
        }
        Asserting { timer: recorder::Timer::new(), recorded, round: 0, belied: false, taken: BTreeMap::new(), misshapen: false, stood_in: BTreeMap::new(), unrecorded: BTreeSet::new(), marks: marks.clone() }
    }

    /// What the log recorded for the node at `path` in this round, as it ran: in a round it
    /// recorded none for, the round's before it, else its last. `Err`: it waited on an answer here
    /// (nothing yet, and something in a later round), so it is run, and asks for it.
    fn at(&self, path: &[u32]) -> Result<Option<&Vec<Option<Value>>>, ()> {
        let Some(rounds) = self.recorded.get(path) else { return Ok(None) };
        let given = |o: &Vec<Option<Value>>| o.iter().any(Option::is_some);
        match rounds.get(&self.round) {
            Some(o) if !given(o) && rounds.range(self.round + 1..).any(|(_, o)| given(o)) => Err(()),
            Some(o) => Ok(Some(o)),
            None => Ok(rounds.range(..self.round).next_back().or(rounds.iter().next_back()).map(|(_, o)| o)),
        }
    }

    /// Whether `leaf`, fed `inputs`, is computed on the placeholders they carry, and how
    /// (`holon_core::taint::on_a_mark`), given what these inputs are:
    ///
    /// - a record's key that is a placeholder orders and finds by the placeholder, not the value:
    ///   not computed;
    /// - `as_text` writes anything but a text as JSON, where a placeholder is escaped as its value
    ///   wouldn't be: computed for a text only;
    /// - a comparison is of two texts, each placeholder in them one that tells its value from
    ///   another (a fingerprint): a tainted value's doesn't, nor a token's without one.
    fn computes(&self, leaf: &str, inputs: &[Option<&Value>]) -> Option<holon_core::taint::OnAMark> {
        use holon_core::taint::OnAMark;
        let how = holon_core::taint::on_a_mark(leaf)?;
        if inputs.iter().flatten().any(|v| self.marks.in_a_key(v)) {
            return None;
        }
        let texts = || inputs.iter().flatten().all(|v| matches!(v, Value::Text(_)));
        match how {
            OnAMark::Moves if leaf == "as_text" => texts().then_some(how),
            OnAMark::Moves => Some(how),
            OnAMark::Compares => (texts() && inputs.iter().flatten().all(|v| self.marks.told_apart(v))).then_some(how),
        }
    }

    /// What `unit`, a leaf that compares (`OnAMark::Compares`), gives for `inputs`: each
    /// placeholder read by its fingerprint, so one value under two names or two versions is one
    /// value. Nothing where an input didn't arrive, as core gives.
    fn compared(&self, name: &str, unit: &holon_core::Unit, inputs: &[Option<&Value>]) -> Vec<Option<Value>> {
        let by_print: Vec<Value> = inputs.iter().flatten().map(|v| self.marks.by_print(v)).collect();
        let all_here = by_print.len() == inputs.len();
        vec![all_here.then(|| holon_core::primitives::compute(name, &by_print.iter().collect::<Vec<_>>(), &unit.params)).flatten()]
    }
}

/// Whether `outputs`, what the log recorded for the leaf `name`, is something that leaf can
/// give: as many outputs as it has, each of the type it declares where it declares one, and a
/// SHA-256 or an HMAC of one 32 bytes in the encoding its node says, or the one mark the log
/// wrote for it (`computed`): a hash of a part of a secret, or of a value computed from one, says
/// that part, so the record marks it as it does any value computed from a secret. Nothing is what
/// any leaf gives when an input didn't arrive.
fn shaped(name: &str, unit: &holon_core::Unit, outputs: &[Option<Value>], computed: &str) -> bool {
    use holon_core::Type;
    if outputs.len() != unit.outputs.len().max(1) {
        return false;
    }
    let of_type = |ty: &Type, v: &Value| match ty {
        Type::Number => matches!(v, Value::Number(_)),
        Type::Boolean => matches!(v, Value::Boolean(_)),
        Type::Text => matches!(v, Value::Text(_)),
        Type::List(_) => matches!(v, Value::List(_)),
        Type::Record => matches!(v, Value::Record(_)),
        _ => true,
    };
    let typed = outputs.iter().zip(&unit.outputs).all(|(o, port)| o.as_ref().is_none_or(|v| of_type(&port.ty, v)));
    let digest = || match (&outputs[0], unit.params.iter().find(|p| p.name() == "encoding").and_then(|p| p.value())) {
        (Some(Value::Text(t)), _) if t.starts_with(computed) && t.find('›') == Some(t.len() - '›'.len_utf8()) => true,
        (Some(Value::Text(t)), Some(encoding)) => holon_core::text_bytes(t, encoding).is_some_and(|b| b.len() == 32),
        (None, _) => true,
        _ => false,
    };
    typed && (!matches!(name, "sha256" | "hmac_sha256") || digest())
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
        self.round = n as u32;
        self.timer.round(n)
    }
    fn failed(&mut self, path: &[u32], reason: &str) {
        self.timer.failed(path, reason)
    }
    fn asserted(&mut self, path: &[u32], unit: &holon_core::Unit, inputs: &[Option<&Value>]) -> Option<Vec<Option<Value>>> {
        if matches!(unit.body, holon_core::UnitBody::Composite { .. }) || !inputs.iter().flatten().any(|v| self.marks.carried(v)) {
            return None;
        }
        // A call moves what it is fed into its request and gives what was answered: computed on
        // its marks, it asks, and is answered only by the call its line records at its path with
        // that very request, each mark where it put it (R61). Not after a node that couldn't be
        // computed, where what it is fed isn't the run's.
        if let holon_core::UnitBody::Primitive { name } = &unit.body
            && holon_core::taint::is_answered(name)
            && self.unrecorded.is_empty()
        {
            return None;
        }
        if let holon_core::UnitBody::Primitive { name } = &unit.body
            && let Some(how) = self.computes(name, inputs)
        {
            let gave = match how {
                // A leaf: it resolves nothing.
                holon_core::taint::OnAMark::Moves => {
                    let inputs: Vec<Option<Value>> = inputs.iter().map(|v| v.cloned()).collect();
                    holon_core::eval(unit, &inputs, &holon_core::MapResolver::new()).into_vec()
                }
                holon_core::taint::OnAMark::Compares => self.compared(name, unit, inputs),
            };
            match self.at(path).ok().flatten() {
                Some(recorded) if *recorded != gave && self.unrecorded.is_empty() => {
                    self.belied = true;
                    return Some(gave);
                }
                // After a node that couldn't be computed, what it gives here isn't the run's.
                Some(recorded) if *recorded != gave => {}
                _ => return Some(gave),
            }
        }
        let Ok(at) = self.at(path) else { return None };
        match at.cloned() {
            Some(outputs) => {
                let leaf = match &unit.body {
                    holon_core::UnitBody::Primitive { name } => name.clone(),
                    _ => "a module".into(),
                };
                let mut fed = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
                let mut derived = false;
                for v in inputs.iter().flatten() {
                    fed.0.extend(self.marks.secrets_in(v));
                    fed.1.extend(self.marks.tokens_in(v));
                    fed.2.extend(self.marks.tainted_in(v));
                    derived |= !self.marks.rest_of(Marks::DERIVED, v).is_empty();
                }
                let list = |set: BTreeSet<String>| set.into_iter().collect();
                // A pure leaf's record can be held to its shape, and, where its inputs tell
                // their values apart, to the export's other records of it.
                if let holon_core::UnitBody::Primitive { name } = &unit.body
                    && holon_core::capabilities_of(name, &unit.params).is_some_and(|c| c.is_pure())
                {
                    self.misshapen |= !shaped(name, unit, &outputs, &self.marks.of_computed());
                    // After a node that couldn't be computed, the inputs seen here aren't the
                    // run's (two runs that signed two messages both show none): nothing to hold
                    // one record to another by.
                    if self.unrecorded.is_empty() && inputs.iter().flatten().all(|v| self.marks.all_told_apart(v)) {
                        let of = *blake3::hash(format!("{name}\0{:?}\0{inputs:?}", unit.params).as_bytes()).as_bytes();
                        // Less the print of a value the run computed: that is the run's own.
                        let outputs = outputs.iter().map(|o| o.as_ref().map(|v| self.marks.across_runs(v))).collect();
                        self.taken.insert(path.to_vec(), Taken { of, leaf: leaf.clone(), outputs });
                    }
                }
                self.stood_in.insert(path.to_vec(), Stood { leaf, secrets: list(fed.0), tokens: list(fed.1), tainted: list(fed.2), derived });
                Some(outputs)
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
    /// What follows a placeholder's start for a value a run computed from a secret, up to its
    /// print (`secrets::DERIVED_MARK`, less the `‹`).
    const DERIVED: &'static str = "derived #";

    /// How the mark of a value a run computed from a secret begins, as this check reads it.
    fn of_computed(&self) -> String {
        format!("{}{}", self.start, Marks::DERIVED)
    }

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

    /// The tainted keys whose placeholder is in `v` (`‹taint:authorization›`: a value of the
    /// request's the host never keeps), by key.
    fn tainted_in(&self, v: &Value) -> BTreeSet<String> {
        self.rest_of("taint:", v)
    }

    /// The tokens whose placeholder is in `v` (as [`Marks::read`] gives it): each as its mark
    /// reads, `answer access_token #a1b2c3d4`, so one token is told from another by its print.
    fn tokens_in(&self, v: &Value) -> BTreeSet<String> {
        self.rest_of("token:", v)
    }

    /// What follows `kind` in each placeholder of that kind in `v`, up to the placeholder's end.
    fn rest_of(&self, kind: &str, v: &Value) -> BTreeSet<String> {
        let start = format!("{}{kind}", self.start);
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
        // So is what the run computed from a secret.
        let secret = |t: &str| t.contains(&format!("{}secret:", self.start)) || t.contains(&format!("{}token:answer ", self.start)) || t.contains(&format!("{}{}", self.start, Marks::DERIVED));
        match v {
            Value::Text(t) => secret(t),
            Value::List(items) => items.iter().any(|v| self.carries_secret(v)),
            Value::Record(fields) => fields.iter().any(|(k, v)| secret(k) || self.carries_secret(v)),
            Value::Sum { payload, .. } | Value::Effect { payload, .. } => self.carries_secret(payload),
            Value::Bytes(b) => secrets::bytes_hold(b, &secret),
            Value::Number(_) | Value::Boolean(_) => false,
        }
    }

    /// Each placeholder in `t`, as what follows its start up to its end (`secret:name v3 #a1b2c3d4`),
    /// with where it starts and where what follows it does.
    fn each_in<'a>(&'a self, t: &'a str) -> impl Iterator<Item = (usize, usize, &'a str)> + 'a {
        t.match_indices(self.start.as_str()).filter_map(move |(at, _)| {
            let body = &t[at + self.start.len()..];
            let end = body.find('›')?;
            Some((at, at + self.start.len() + end + '›'.len_utf8(), &body[..end]))
        })
    }

    /// The fingerprint a placeholder's body ends with (`… #a1b2c3d4`), if it has one: what tells
    /// its value from another's, whatever its name or version.
    fn print_of(body: &str) -> Option<&str> {
        let (_, print) = body.rsplit_once(" #")?;
        (print.len() == 8 && print.bytes().all(|b| b.is_ascii_hexdigit())).then_some(print)
    }

    /// Whether every placeholder in the text `v` tells its value from another's: a secret's or a
    /// token's with its fingerprint. A tainted value's has none, so two of them read alike.
    fn told_apart(&self, v: &Value) -> bool {
        match v {
            Value::Text(t) => self.each_in(t).all(|(_, _, body)| Marks::print_of(body).is_some()),
            _ => false,
        }
    }

    /// Whether every placeholder anywhere in `v` tells its value from another's
    /// ([`Marks::told_apart`]): in a text, a list, a record's keys and values. Bytes that carry
    /// one don't say (a placeholder in bytes isn't read apart).
    fn all_told_apart(&self, v: &Value) -> bool {
        let text = |t: &str| self.each_in(t).all(|(_, _, body)| Marks::print_of(body).is_some());
        match v {
            Value::Text(t) => text(t),
            Value::List(items) => items.iter().all(|v| self.all_told_apart(v)),
            Value::Record(fields) => fields.iter().all(|(k, v)| text(k) && self.all_told_apart(v)),
            Value::Sum { payload, .. } | Value::Effect { payload, .. } => self.all_told_apart(payload),
            Value::Bytes(b) => !secrets::bytes_hold(b, &|t: &str| t.contains(&self.start)),
            Value::Number(_) | Value::Boolean(_) => true,
        }
    }

    /// The text `v` with each placeholder written as its fingerprint alone, so that one value
    /// under two names or two versions reads as one. For comparing, never for what is kept.
    fn by_print(&self, v: &Value) -> Value {
        let Value::Text(t) = v else { return v.clone() };
        let (mut out, mut from) = (String::with_capacity(t.len()), 0);
        for (at, end, body) in self.each_in(t) {
            if at < from {
                continue;
            }
            out.push_str(&t[from..at]);
            match Marks::print_of(body) {
                // A print of a value a run computed is of another kind than a secret's or a
                // token's (another key): the two never read as one value.
                Some(print) if body.starts_with(Marks::DERIVED) => out.push_str(&format!("{}{}{print}›", self.start, Marks::DERIVED)),
                Some(print) => out.push_str(&format!("{}#{print}›", self.start)),
                None => out.push_str(&t[at..end]),
            }
            from = end;
        }
        out.push_str(&t[from..]);
        Value::Text(out)
    }

    /// `v` with each placeholder of a value a run computed from a secret written without its
    /// print: such a mark is keyed by its run, so one value reads otherwise in the next run. For
    /// holding one run's record of a leaf to another's (`Taken`), never for what is kept.
    fn across_runs(&self, v: &Value) -> Value {
        let derived = format!("{}{}", self.start, Marks::DERIVED);
        let mut v = SValue::from_core(v);
        secrets::texts_mut(&mut v, &mut |t| {
            if !t.contains(&derived) {
                return;
            }
            let (mut out, mut from) = (String::with_capacity(t.len()), 0);
            for (at, end, body) in self.each_in(t) {
                if at < from || !body.starts_with(Marks::DERIVED) {
                    continue;
                }
                out.push_str(&t[from..at]);
                out.push_str(&derived);
                out.push('›');
                from = end;
            }
            out.push_str(&t[from..]);
            *t = out;
        });
        v.into_core()
    }

    /// Whether a record's key anywhere in `v` carries a placeholder.
    fn in_a_key(&self, v: &Value) -> bool {
        match v {
            Value::Record(fields) => fields.iter().any(|(k, v)| k.contains(&self.start) || self.in_a_key(v)),
            Value::List(items) => items.iter().any(|v| self.in_a_key(v)),
            Value::Sum { payload, .. } | Value::Effect { payload, .. } => self.in_a_key(payload),
            Value::Text(_) | Value::Bytes(_) | Value::Number(_) | Value::Boolean(_) => false,
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

/// The network, for a run being checked: each request the log says it sent, by the node that sent
/// it, answered as the log says; any other as [`Offline`] answers.
struct Answering {
    exchanges: Vec<(Vec<u32>, Value, Value)>,
    /// Which of them a node asked for.
    asked: Mutex<Vec<bool>>,
}

impl Answering {
    fn new(exchanges: Vec<(Vec<u32>, Value, Value)>) -> Answering {
        let asked = Mutex::new(vec![false; exchanges.len()]);
        Answering { exchanges, asked }
    }

    /// Whether a call it records went unasked: one the run, run again, doesn't make.
    fn unasked(&self) -> bool {
        self.asked.lock().unwrap().contains(&false)
    }
}

impl Performer for Answering {
    fn perform(&self, request: &Value, limits: &Limits) -> Value {
        Offline.perform(request, limits)
    }
    fn perform_at(&self, path: &[u32], request: &Value, limits: &Limits) -> Value {
        // The first not yet asked: a call recorded twice is answered once, and the other is one
        // the run doesn't make.
        let mut asked = self.asked.lock().unwrap();
        match self.exchanges.iter().enumerate().find(|(i, (p, r, _))| !asked[*i] && p == path && r == request) {
            Some((i, (_, _, answer))) => {
                asked[i] = true;
                answer.clone()
            }
            None => Offline.perform(request, limits),
        }
    }
}

/// The keys a sealed export opens under (§2.7): `keys.json` in the data folder (`--data`, else
/// the server's own), its root where `--root-key` says (else the OS key, as the server's). Read
/// only: a folder with no keys.json is said, never given one.
pub(crate) fn keys_for(data: Option<&str>, root_key: Option<&str>) -> Result<std::sync::Arc<crate::at_rest::Keyring>, String> {
    let data = match data {
        Some(d) => PathBuf::from(d),
        None => crate::migrate::data_folder(&crate::local::holon_home().ok_or("no home folder (USERPROFILE or HOME), and no HOLON_HOME")?),
    };
    let keys = data.join("keys.json");
    if !keys.exists() {
        return Err(format!("it is sealed under a server's keys, and {} isn't there: pass --data with the data folder it came from (and --root-key where its root key is)", keys.display()));
    }
    crate::at_rest::Keyring::open(&keys, root_key, &[]).map(std::sync::Arc::new)
}

/// `holon verify <export.json> [--head <hex>] [--files DIR] [--data DIR] [--root-key SOURCE]`:
/// check an export, say what was found, and exit 0 only if it checks, every run in it run again
/// and every run's state checked; 3 if nothing in it fails, but some runs couldn't be run again or
/// some states checked; 1 if anything doesn't check.
pub(crate) fn run(args: &[String]) -> i32 {
    let mut file = None;
    let mut head = None;
    let mut dir = None;
    let mut data = None;
    let mut root_key = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--head" => head = args.next().cloned(),
            "--files" => dir = args.next().cloned(),
            "--data" => data = args.next().cloned(),
            "--root-key" => root_key = args.next().cloned(),
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
    let export: Export = match read_export(Path::new(&file), || keys_for(data.as_deref(), root_key.as_deref())) {
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
    match (report.ok(), report.complete()) {
        (false, _) => 1,
        (true, false) => 3,
        (true, true) => 0,
    }
}

const USAGE: &str = "usage: holon verify <export.json> [--head HEX] [--files DIR] [--data DIR] [--root-key SOURCE]

  Checks a deployment's exported runs (holon export), offline: that its run log's hash chain is
  unbroken, that every unit is what its hash says, and that every run, run again from its log
  entry, produces what the log says it did. Exits 0 when it all checks, 1 when anything doesn't,
  3 when nothing fails but not everything was checked: some runs couldn't be run again, or some
  runs' states couldn't be checked (it says which, and why), 2 when it can't read the export.

  --head HEX               a head a witness holds: check it is in the export, so no run in the
                           export, from its oldest up to that head, was changed or removed
                           (of runs a trimmed log no longer holds, it says nothing)
  --files DIR              where the files the runs read are, for those whose log keeps only
                           their hashes: each is found by its hash, wherever it is under DIR
  --data DIR               for an export the server keeps sealed (a backup's archive, a removed
                           deployment's records): the data folder whose keys.json sealed it, if
                           not this machine's server's (HOLON_DATA, or data/ in HOLON_HOME)
  --root-key SOURCE        where that keys.json's root key is: os, env:VAR or file:PATH (the
                           server's --root-key); the OS key store if not given. An export
                           holon export wrote is plain, and needs neither

examples:
  holon verify shop-runs.json
  holon verify shop-runs.json --head \"$(cat witness-head.txt)\"
  holon verify file-audit-runs.json --files /srv/inbox
  holon verify backup/shop/holon-archive-shop-runs-…json.gz --root-key file:/etc/holon/root.key";

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
    if r.trimmed && r.runs == 1 {
        chain += &format!(" (run {} alone: the line before it hashes to {}, which only the log has)", r.first.unwrap_or(0), short(&r.after));
    } else if r.trimmed {
        chain += &format!(" (from run {}; older runs were trimmed)", r.first.unwrap_or(0));
    }
    out += &format!("  {chain}\n");
    if !r.bad_lines.is_empty() {
        out += &format!("  lines {} AREN'T RUNS: changed into something else\n", list(&r.bad_lines));
    }
    for (seq, why) in &r.malformed {
        out += &format!("  run {seq} ISN'T A LINE THE SERVER WRITES: {why}\n");
    }
    // Runs, first to last: theirs, and whether they are one.
    let spans = |s: &[(u64, u64)]| {
        let runs: Vec<String> = s.iter().map(|(a, b)| if a == b { a.to_string() } else { format!("{a} to {b}") }).collect();
        (runs.join(", "), matches!(s, [(a, b)] if a == b))
    };
    if !r.lost.is_empty() {
        let (runs, one) = spans(&r.lost);
        out += &format!(
            "  {} {runs} {} in the file: the server couldn't write {}, and the line after says so\n",
            if one { "run" } else { "runs" },
            if one { "isn't" } else { "aren't" },
            if one { "its line" } else { "their lines" }
        );
    }
    if !r.gaps.is_empty() {
        let (runs, one) = spans(&r.gaps);
        out += &format!(
            "  {} {runs} MISSING: not in the file, and no line says the server couldn't write {} (removed, and the lines after chained again)\n",
            if one { "run" } else { "runs" },
            if one { "it" } else { "them" }
        );
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
    // A witnessed head is the hash of one line, which chains back through every run before it, but
    // only the runs here can be checked against it: what it proves is named from the oldest of
    // them, and a log trimmed to its newest runs says nothing of the ones before.
    let from = r.first.unwrap_or(0);
    let older = if r.trimmed { " (older runs were trimmed: it says nothing of them)" } else { "" };
    match (&r.witnessed, head) {
        (Some(None), Some(_)) if r.witnessed_event.is_some() => {}
        (Some(Some(seq)), Some(h)) if r.breaks.iter().any(|b| b <= seq) => {
            out += &format!("  witnessed head {}: run {seq}, but the chain breaks before it, so runs from {from} to {seq} were changed{older}\n", short(h))
        }
        (Some(Some(seq)), Some(h)) if *seq == from => out += &format!("  witnessed head {}: run {seq}, so it wasn't changed or removed{older}\n", short(h)),
        (Some(Some(seq)), Some(h)) => out += &format!("  witnessed head {}: run {seq}, so no run from {from} to {seq} was changed or removed{older}\n", short(h)),
        (Some(None), Some(h)) => {
            out += &format!(
                "  witnessed head {}: NOT FOUND: a run up to it was changed or removed, or it is older than the oldest run here\n",
                short(h)
            )
        }
        _ => {}
    }
    // What no run again can make, which only the chain holds: a witnessed head holds it too.
    if head.is_none() && r.runs > 0 {
        let said: Vec<&str> = NOT_HELD.iter().map(|(_, said)| *said).collect();
        out += &format!("  held by the chain alone, not by running each run again (a witnessed head holds it too): {}\n", said.join(", "));
    }
    if !r.bad_units.is_empty() {
        out += &format!("  units: {} DON'T MATCH their hash\n", r.bad_units.iter().map(|u| short(u)).collect::<Vec<_>>().join(", "));
    }

    let matched = r.reruns.iter().filter(|(_, x)| *x == Rerun::Matched).count();
    let mut not: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    let mut nothing: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for (seq, x) in &r.reruns {
        match x {
            Rerun::Skipped(why) => not.entry(why).or_default().push(*seq),
            Rerun::NothingRan(why) => nothing.entry(why).or_default().push(*seq),
            _ => {}
        }
    }
    // A run that matched with a step taken from the log matched given that step: the line that
    // says it matched says so, and the lines after name each such step.
    let on_trust = r.reruns.iter().filter(|(seq, x)| *x == Rerun::Matched && r.given.contains_key(seq)).count();
    out += &format!("  re-ran {matched} runs: each produced what its log entry says");
    out += &match on_trust {
        0 => "\n".to_string(),
        n => format!(", {n} of them given what the log kept for a step (named below)\n"),
    };
    if !r.given.is_empty() {
        let seqs: Vec<u64> = r.given.keys().copied().collect();
        let nodes: usize = r.given.values().map(Vec::len).sum();
        out += &format!(
            "  {} checked given what the log kept for {nodes} node{} fed a secret, a token or a tainted value (not computed again):\n",
            count_of(&seqs),
            if nodes == 1 { "" } else { "s" }
        );
        // By leaf and what it was fed: how many nodes, in which runs.
        let mut steps: BTreeMap<(&str, String), (usize, Vec<u64>)> = BTreeMap::new();
        for (seq, stood) in &r.given {
            for s in stood {
                let step = steps.entry((s.leaf.as_str(), s.fed())).or_default();
                step.0 += 1;
                if step.1.last() != Some(seq) {
                    step.1.push(*seq);
                }
            }
        }
        for ((leaf, fed), (nodes, seqs)) in &steps {
            out += &format!("    {leaf} fed {fed}: {nodes} node{} in {}, as recorded\n", if *nodes == 1 { "" } else { "s" }, count_of(seqs));
        }
    }
    if !r.called_out.is_empty() {
        out += &format!(
            "  {} called out: what each call was answered is as recorded, held by the chain alone (and a witnessed head)\n",
            count_of(&r.called_out)
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
    for (why, seqs) in &nothing {
        out += &format!("  nothing to run again ({}): {why}\n", count_of(seqs));
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
        let fresh = match s.fresh.len() {
            0 => String::new(),
            1 => format!("run {} began from nothing, as a log's first does; ", s.fresh[0]),
            _ => format!("{} began from nothing, as a log's first does; ", count_of(&s.fresh)),
        };
        out += &format!(
            "  state: {fresh}{} runs began from the state the run before them left{beside}, {} left what their writes make of the one they began with\n",
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
    out += match (r.ok(), r.all_run_again(), r.complete()) {
        (false, _, _) => "FAILED\n",
        (true, false, false) if r.states.as_ref().is_some_and(|s| !s.unchecked.is_empty()) => {
            "INCOMPLETE: nothing here fails, but not every run was run again, nor every run's state checked (said above)\n"
        }
        (true, false, _) => "INCOMPLETE: nothing here fails, but not every run was run again (said above)\n",
        (true, true, false) => "INCOMPLETE: nothing here fails, but not every run's state was checked (said above)\n",
        (true, true, true) => "ok\n",
    };
    // What ok may mean of a run that called out, said where it can't be missed.
    if r.ok() && !r.called_out.is_empty() {
        out += &format!("  ({} called out: what their calls were answered is the log's say-so, held by the chain)\n", count_of(&r.called_out));
    }
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
    let mut run: Option<u64> = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--to" => to = args.next().cloned(),
            "--as" => name = args.next().cloned(),
            "--run" => match args.next().and_then(|v| v.parse::<u64>().ok()) {
                Some(n) => run = Some(n),
                None => {
                    eprintln!("holon export: --run takes a run's number (holon runs <slug> lists them)");
                    return 2;
                }
            },
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
    // One run's file is named for it.
    let name = name.or_else(|| run.map(|n| format!("{slug}-run-{n}.json")));
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
    let query: Vec<String> = files.into_iter().chain(run.map(|n| format!("run={n}"))).collect();
    let path = format!("/deployments/{slug}/export{}", if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) });
    let (code, response) = match local::request(&target, "GET", &path, None, Duration::from_secs(120)) {
        Ok(answered) => answered,
        Err(e) => {
            eprintln!("holon export: {e}");
            eprintln!("holon export: is holon serve running there? --server takes the port or address it printed");
            return 1;
        }
    };
    if code >= 400 {
        let body = response.into_string().unwrap_or_default();
        let why = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_string))
            .unwrap_or_else(|| body.trim().to_string());
        eprintln!("holon export: {server} answered {code}: {why}");
        if code == 401 {
            eprintln!("holon export: {}", local::sign_in_hint(server, target.from));
        }
        return 1;
    }
    // Read as every command reads a server's answer: whole, or said not to be.
    let body = match local::read_answer(response.into_reader(), local::MAX_ANSWER) {
        Ok(body) => body,
        Err(why) => {
            eprintln!("holon export: {server} answered {code}, and its answer couldn't be read: {why}");
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
    match run {
        Some(n) => println!("wrote {path}: run {n} of {slug:?}, its hash {head}"),
        None => println!("wrote {path}: {} runs of {slug:?}{events}, head {head}", export.lines.len()),
    }
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
  --as NAME                the file's name (default <slug>-runs.json, <slug>-run-N.json)
  --run N                  run N alone: its line as written, which names the hash of the one
                           before it, and the units and files it ran and read
  --max-files-bytes N      ship the files' texts up to N bytes in all (64K, 10M, 2G), the
                           oldest runs' first
  --without-files          ship none of them: holon verify checks those runs with --files
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon export shop
  holon export shop --to backups --as shop-2026-11.json
  holon export shop --run 42
  holon export file-audit --max-files-bytes 10M

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  as you with holon login (and with none running here, the one you signed in to needs no
  --server), or as an agent or a service with its token in HOLON_TOKEN.";

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards: the command line reference says what `holon verify --help` says a witnessed head
    /// proves, and what it doesn't.
    /// Why: the two were written apart; the reference said only that the head is in the export,
    /// and the usage was corrected without it (audit R49).
    #[test]
    fn the_references_head_proves_what_the_usage_says() {
        let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let usage = words(USAGE);
        let head = usage.split("--head HEX a head a witness holds: check it is in the export, ").nth(1).expect("the usage's --head");
        let proves = head[..head.find("--files").expect("--files after --head")].trim_end();
        let cli = words(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual/reference/cli.md")).unwrap());
        let said = format!("`--head` checks that a head a witness holds is in the export, {proves}.");
        assert!(cli.contains(&said), "docs/manual/reference/cli.md's holon verify should say: {said}");
    }

    /// Guards: what an entry says of its secrets holds together (`listed_secrets`). Its list is a
    /// kind and a commitment for a version it names; a mark of the version a run was given of a
    /// secret kept here reads the fingerprint its list has; two marks of one version read one
    /// fingerprint; and one value has one commitment in every run of an export.
    /// Why: a run computed on its marks (`Asserting::computes`) and a commitment a reader later
    /// checks a value against both rest on the marks and the list saying the same thing. An
    /// entry put together from parts of two runs, or a commitment changed in one line, would
    /// otherwise pass.
    /// If it fails: the row named says which of those a line can now break unseen, or which
    /// sound line is now said to differ (a reference's mark, a look-alike the run wrote itself).
    #[test]
    fn what_an_entry_says_of_its_secrets_holds_together() {
        let hex = |c: char| c.to_string().repeat(64);
        let kept = |kind: &str, commitment: &str| recorder::Committed { kind: kind.into(), commitment: commitment.into(), server: false };
        let entry = |seq: u64, shown: &str, c: recorder::Committed| LogEntry {
            seq,
            secrets: BTreeMap::from([("k".to_string(), shown.to_string())]),
            committed: BTreeMap::from([("k".to_string(), c)]),
            escaped: true,
            ..LogEntry::default()
        };
        let line = |texts: &[&str]| serde_json::json!({ "request": { "headers": texts, "nested": [{ "deep": texts }] } }).to_string();
        let computed = |hashes: &[&str]| LogEntry { derived: hashes.iter().map(|h| h.to_string()).collect(), ..entry(1, "v2 #aaaaaaaa", kept("local", &hex('1'))) };
        let same = "‹secret:k v2 #aaaaaaaa›";
        // (what the entry says, the texts its line holds, why it doesn't hold together)
        let rows: Vec<(&str, LogEntry, Vec<&str>, Option<&str>)> = vec![
            ("no mark at all", entry(1, "v2 #aaaaaaaa", kept("local", &hex('1'))), vec!["plain"], None),
            ("marks that read as the list does", entry(1, "v2 #aaaaaaaa", kept("local", &hex('1'))), vec![same, "Bearer ‹secret:k v2 #aaaaaaaa›"], None),
            ("a mark of the given version with another fingerprint", entry(1, "v2 #aaaaaaaa", kept("local", &hex('1'))), vec![same, "‹secret:k v2 #bbbbbbbb›"], Some("a mark of the secret k v2 reads #bbbbbbbb")),
            ("an older version's mark, alone", entry(1, "v2 #aaaaaaaa", kept("local", &hex('1'))), vec![same, "‹secret:k v1 #cccccccc›"], None),
            ("an older version's marks, two fingerprints", entry(1, "v2 #aaaaaaaa", kept("local", &hex('1'))), vec!["‹secret:k v1 #cccccccc›", "‹secret:k v1 #dddddddd›"], Some("a mark of the secret k v1 reads #")),
            ("a reference's mark, other than it was given: its value may change under a version", entry(1, "v2 #aaaaaaaa", kept("reference", &hex('1'))), vec!["‹secret:k v2 #bbbbbbbb›"], None),
            ("a reference's marks, two fingerprints in one line", entry(1, "v2 #aaaaaaaa", kept("reference", &hex('1'))), vec!["‹secret:k v2 #bbbbbbbb›", "‹secret:k v2 #cccccccc›"], Some("a mark of the secret k v2 reads #")),
            ("a look-alike the run wrote itself, escaped", entry(1, "v2 #aaaaaaaa", kept("local", &hex('1'))), vec![same, "‹‹secret:k v2 #bbbbbbbb›"], None),
            ("another secret's mark", entry(1, "v2 #aaaaaaaa", kept("local", &hex('1'))), vec![same, "‹secret:other v2 #bbbbbbbb›"], None),
            ("a token's mark and a tainted value's", entry(1, "v2 #aaaaaaaa", kept("local", &hex('1'))), vec![same, "‹token:answer access_token #bbbbbbbb›", "‹taint:authorization›"], None),
            ("a kind that isn't one", entry(1, "v2 #aaaaaaaa", kept("here", &hex('1'))), vec![same], Some("isn't a kind and a commitment")),
            ("a commitment cut short", entry(1, "v2 #aaaaaaaa", kept("local", "abcd")), vec![same], Some("isn't a kind and a commitment")),
            ("a commitment that isn't hex", entry(1, "v2 #aaaaaaaa", kept("local", &hex('z'))), vec![same], Some("isn't a kind and a commitment")),
            ("a commitment to a version it doesn't list", entry(1, "the second", kept("local", &hex('1'))), vec![], Some("doesn't list which version")),
            // What it computed from a secret: each mark the start of a hash its entry lists.
            ("a computed value's mark, its hash listed", computed(&[&hex('3'), &hex('7')]), vec!["Basic ‹derived #77777777›", "‹derived #33333333›"], None),
            ("a computed value's mark with no hash listed", computed(&[&hex('3')]), vec!["‹derived #77777777›"], Some("lists no hash that begins so")),
            ("a computed value's mark, nothing listed", computed(&[]), vec!["‹derived #77777777›"], Some("lists no hash that begins so")),
            ("a computed value's mark cut short", computed(&[&hex('3')]), vec!["‹derived #3333›"], Some("lists no hash that begins so")),
            ("a look-alike of one the run wrote itself, escaped", computed(&[]), vec!["‹‹derived #77777777›"], None),
            ("hashes listed out of order", computed(&[&hex('7'), &hex('3')]), vec![], Some("isn't hashes, each once and in order")),
            ("a hash listed twice", computed(&[&hex('3'), &hex('3')]), vec![], Some("isn't hashes, each once and in order")),
            ("a listed hash that isn't one", computed(&["abcd"]), vec![], Some("isn't hashes, each once and in order")),
        ];
        for (what, entry, texts, why) in rows {
            let said = listed_secrets(&entry, &line(&texts), &mut BTreeMap::new());
            match why {
                None => assert_eq!(said, None, "{what}"),
                Some(why) => assert!(said.as_deref().is_some_and(|s| s.contains(why)), "{what}: {said:?}"),
            }
        }
        // Across an export: one value, one commitment; another version, another value of a
        // reference under one version, and the server's secret of the same name each have their own.
        let mut seen = BTreeMap::new();
        let mut says = |e: LogEntry| listed_secrets(&e, &line(&[]), &mut seen);
        assert_eq!(says(entry(0, "v2 #aaaaaaaa", kept("local", &hex('1')))), None);
        assert_eq!(says(entry(1, "v2 #aaaaaaaa", kept("local", &hex('1')))), None, "the same again");
        assert_eq!(says(entry(2, "v3 #aaaaaaaa", kept("local", &hex('2')))), None, "another version");
        assert_eq!(says(entry(3, "v2 #bbbbbbbb", kept("reference", &hex('3')))), None, "another value");
        assert_eq!(says(entry(4, "v2 #aaaaaaaa", recorder::Committed { server: true, ..kept("local", &hex('4')) })), None, "the server's");
        let other = says(entry(5, "v2 #aaaaaaaa", kept("local", &hex('5'))));
        assert_eq!(other.as_deref(), Some("the commitment its entry has for the secret k v2 #aaaaaaaa: run 0 recorded another for the same value"));
    }

    /// Guards: every leaf `verify` computes on a placeholder (`Asserting::computes`, by core's
    /// `taint::on_a_mark`) gives with the placeholder what it gives with the value: computed
    /// with a secret's value and its result written as the log writes it, it is what it gives
    /// computed with the log's marks. Every leaf of the catalog, inputs made from each port's
    /// type, each holding a secret in a text, a list and a record.
    /// Why: `verify` reports such a leaf as checked, and holds the log's record of it to what it
    /// computes. A leaf on the list that orders by a text, or writes it as JSON, would make a
    /// sound export differ, or pass a record that isn't the run's.
    /// If it fails: the leaf named isn't exact on a mark for those inputs: take it off core's
    /// list, or narrow `computes`.
    #[test]
    fn a_leaf_computed_on_a_mark_gives_what_it_gives_with_the_value() {
        use holon_core::taint::OnAMark;
        // Two secrets, the first with what JSON escapes; each written by the log with a
        // fingerprint, as a run's entry shows it.
        let (secret, other) = ("s3cr3t \"quoted\" \\ é value", "an0ther-secret-value");
        let mut resolved = secrets::Resolved::default();
        resolved.current = BTreeMap::from([("k".to_string(), zeroize::Zeroizing::new(secret.to_string())), ("o".to_string(), zeroize::Zeroizing::new(other.to_string()))]);
        resolved.shown = BTreeMap::from([("k".to_string(), "v1 #0a0b0c0d".to_string()), ("o".to_string(), "v2 #1a1b1c1d".to_string())]);
        let vault: secrets::Vault = std::sync::Arc::new(crate::sync::Mutex::new(resolved));
        let redactor = secrets::Redactor::of(&vault);
        let marks = Marks::new();
        // A value as the log writes it, read as `verify` reads it.
        let logged = |v: &Value| {
            let mut s = SValue::from_core(v);
            redactor.svalue(&mut s);
            marks.read(&s, false)
        };
        let text = |t: &str| Value::Text(t.to_string());
        let texts = [text(secret), text(&format!("Bearer {secret}")), text(other), text("plain text"), text(&format!("{secret}, then {other}")), text(&format!("Bearer {other}"))];
        let of = |ty: &holon_core::Type, pick: usize| -> Value {
            use holon_core::Type::*;
            let t = |i: usize| texts[(pick + i) % texts.len()].clone();
            match ty {
                Number => Value::Number((pick % 3) as f64),
                Boolean => Value::Boolean(pick.is_multiple_of(2)),
                Text => t(0),
                List(_) => Value::List(vec![t(0), t(1), text("b"), t(2), t(0)]),
                Record => Value::Record(BTreeMap::from([("id".to_string(), t(0)), ("z".to_string(), t(1)), ("a".to_string(), Value::List(vec![t(2)]))])),
                _ => match pick % 4 {
                    0 => t(0),
                    1 => Value::List(vec![t(0), t(1)]),
                    2 => Value::Record(BTreeMap::from([("id".to_string(), t(0)), ("n".to_string(), Value::Number(1.0))])),
                    _ => t(3),
                },
            }
        };
        let asserting = Asserting::from_timeline(&[], &marks, false);
        let resolver = holon_core::MapResolver::new();
        let mut computed: BTreeMap<&str, usize> = BTreeMap::new();
        for name in holon_core::GENESIS.iter().copied().filter(|n| holon_core::taint::on_a_mark(n).is_some()) {
            let unit = holon_core::interface_of(name).unwrap_or_else(|| panic!("{name} has an interface"));
            for case in 0..48usize {
                let real: Vec<Option<Value>> = unit.inputs.iter().enumerate().map(|(i, p)| Some(of(&p.ty, case / (i + 1) + i * 5))).collect();
                let marked: Vec<Option<Value>> = real.iter().map(|v| v.as_ref().map(&logged)).collect();
                let refs: Vec<Option<&Value>> = marked.iter().map(Option::as_ref).collect();
                if !refs.iter().flatten().any(|v| marks.carried(v)) {
                    continue;
                }
                let Some(how) = asserting.computes(name, &refs) else { continue };
                *computed.entry(name).or_default() += 1;
                let with_the_value: Vec<Option<Value>> = holon_core::eval(&unit, &real, &resolver).iter().map(|o| o.as_ref().map(&logged)).collect();
                let with_the_mark: Vec<Option<Value>> = match how {
                    OnAMark::Moves => holon_core::eval(&unit, &marked, &resolver).into_vec(),
                    OnAMark::Compares => asserting.compared(name, &unit, &refs),
                };
                assert_eq!(with_the_mark, with_the_value, "{name}, case {case}: fed {marked:?}");
            }
        }
        // None is on the list and never tried: a leaf added to it is held here from then on.
        for name in holon_core::GENESIS.iter().copied().filter(|n| holon_core::taint::on_a_mark(n).is_some()) {
            assert!(computed.get(name).is_some_and(|&n| n > 0), "{name} is on core's list and no case computed it: {computed:?}");
        }
        // And the comparisons met both answers, a secret under another name among them.
        let same = [Some(logged(&text(&format!("Bearer {secret}")))), Some(logged(&text(&format!("Bearer {secret}"))))];
        let apart = [Some(logged(&text(secret))), Some(logged(&text(other)))];
        let unit = holon_core::interface_of("secure_equals").unwrap();
        let refs = |v: &[Option<Value>; 2]| -> Vec<Option<Value>> { v.to_vec() };
        let gave = |v: &[Option<Value>; 2]| asserting.compared("secure_equals", &unit, &refs(v).iter().map(Option::as_ref).collect::<Vec<_>>());
        assert_eq!((gave(&same), gave(&apart)), (vec![Some(Value::Boolean(true))], vec![Some(Value::Boolean(false))]));
        // A tainted value's mark tells nothing apart: a comparison of one isn't computed.
        let tainted = Some(Value::Text(format!("{}taint:authorization›", marks.start)));
        assert_eq!(asserting.computes("secure_equals", &[tainted.as_ref(), same[0].as_ref()]), None);
        assert_eq!(asserting.computes("concat", &[tainted.as_ref(), same[0].as_ref()]), Some(OnAMark::Moves), "moving one is");
    }

    /// Guards: what verify says a witnessed head proves names the runs it covers: from the oldest
    /// run in the export to the head's, and, when the log was trimmed to its newest runs, that it
    /// says nothing of the older ones; whether the chain holds there or breaks.
    /// Why: a witnessed head is the hash of one line, chained back through every run before it,
    /// but only the runs an export holds can be checked against it. "No run up to it was changed"
    /// read as every run since the deployment began, where a trimmed export (the newest 500 by
    /// default) can't say so of any run before its first (launch review B6).
    /// If it fails: verify says more than the export holds, and a reader takes a trimmed export
    /// as proof of runs it doesn't have (the message shows the line).
    #[test]
    fn what_a_witnessed_head_proves_is_named_from_the_oldest_run_here() {
        let head = "ab".repeat(32);
        // Trimmed, the oldest run here, the head's run, where the chain breaks.
        for (trimmed, first, seq, breaks) in [(false, 0, 2, vec![]), (true, 500, 742, vec![]), (true, 500, 742, vec![600]), (false, 0, 5, vec![3]), (true, 742, 742, vec![])] {
            let r = Report {
                deployment: "shop".into(),
                runs: (seq - first + 1) as usize,
                first: Some(first),
                last: Some(seq),
                trimmed,
                breaks: breaks.clone(),
                head_matches: true,
                events_head_matches: true,
                witnessed: Some(Some(seq)),
                ..Default::default()
            };
            let said = describe(&r, Some(&head));
            let line = said.lines().find(|l| l.contains("witnessed head")).unwrap_or_else(|| panic!("no witnessed line in:\n{said}"));
            let covered = if first == seq { format!("run {seq}, so it wasn't") } else { format!("runs from {first} to {seq} were") };
            let covered = if first == seq || !breaks.is_empty() { covered } else { format!("no run from {first} to {seq} was") };
            assert!(line.contains(&covered), "trimmed {trimmed}, runs {first} to {seq}, breaks {breaks:?}: {line}");
            assert_eq!(line.contains("older runs were trimmed: it says nothing of them"), trimmed, "{line}");
            assert!(!line.contains("up to it"), "{line}");
        }
    }

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
