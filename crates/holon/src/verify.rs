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
//! Some runs can't be run again, and are counted, not failed: one that used a secret or a resume
//! token (the log holds a placeholder, never the value), one that failed on a budget (the clock
//! decided), one no route took (nothing ran).

use crate::*;
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
}

/// `GET /deployments/<slug>/export` — the deployment's runs, and the units they ran, as one file.
pub(crate) fn handle_get_export(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str) -> Response {
    let (lines, head) = match deployments.lock().unwrap().get(slug) {
        Some(dep) => (dep.log.lines(), dep.log.head().to_string()),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    let mut units = BTreeMap::new();
    let mut queue: Vec<Hash> =
        lines.iter().filter_map(|l| serde_json::from_str::<LogEntry>(l).ok()).filter_map(|e| parse_hash(&e.unit)).collect();
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
    let export = Export { export: FORMAT.into(), deployment: slug.into(), head, lines, units };
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
/// it (one a witness holds).
pub(crate) fn check(export: &Export, witnessed: Option<&str>) -> Report {
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

    for (i, (entry, line)) in entries.iter().enumerate() {
        let earlier: Vec<&LogEntry> =
            entries[..i].iter().map(|(e, _)| e).filter(|e| !entry.run_id.is_empty() && e.run_id == entry.run_id).collect();
        report.reruns.push((entry.seq, rerun(entry, line, &earlier, &resolver)));
    }
    report
}

/// Run `entry` again from what it records (and what the earlier parts of the same run, `earlier`,
/// record: a resumed run's outbound answers and reads), and compare.
fn rerun(entry: &LogEntry, line: &str, earlier: &[&LogEntry], resolver: &MapResolver) -> Rerun {
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
        let cap = match name.as_str() {
            "Clock" => Capability::Clock,
            "Random" => Capability::Random,
            "NetworkIngress" => Capability::NetworkIngress,
            other => return Rerun::Skipped(format!("it read {other}, which verify can't supply")),
        };
        extra.push((cap, value.clone().into_core()));
    }
    let values: BTreeMap<String, Value> = entry.state_before.iter().map(|(k, v)| (k.clone(), v.clone().into_core())).collect();
    let Ok(state) = State::from_values(values) else {
        return Rerun::Differs("its state doesn't encode".into());
    };

    // A secret or a token stands in as the placeholder the log shows: a run that only passes one
    // along still checks; one that computes with it can't.
    let caps = capabilities(root, resolver);
    let mut sourced: Vec<(Capability, Value)> =
        state.iter().map(|(k, v)| (Capability::StateRead(k.clone()), v.clone())).collect();
    sourced.extend(extra.iter().cloned());
    let mut hidden = line.contains("‹secret:") || line.contains("‹token:");
    for cap in caps.iter() {
        match cap {
            Capability::SecretRead(name) => {
                sourced.push((Capability::SecretRead(name.clone()), Value::Text(format!("‹secret:{name}›"))));
                hidden = true;
            }
            Capability::Await(_) => hidden |= line.contains("‹token:"),
            _ => {}
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

    let prior: Vec<Round> = earlier.iter().flat_map(|e| &e.rounds).map(round_of).collect();
    let mut reads: Vec<(Vec<u32>, Value, Option<Value>)> = Vec::new();
    for read in earlier.iter().flat_map(|e| &e.reads).chain(&entry.reads) {
        reads.push((read.path.clone(), read.request.clone().into_core(), read.answer.clone().map(SValue::into_core)));
    }
    let mut waits = Logged {
        reads,
        events: entry.events.iter().map(|(k, v)| (k.clone(), v.clone().into_core())).collect(),
    };
    let limits = Limits { run_budget: Duration::from_secs(3600), max_requests: usize::MAX, ..Limits::clock_run() };
    let mut host = RecordingHost { performed: Vec::new() };
    let mut timer = recorder::Timer::new();
    let outcome = run_rounds(root, &inputs, &sourced, resolver, &mut host, &Offline, &limits, &mut timer, &mut waits, &prior);

    let outputs: Vec<Option<SValue>> = outcome.outputs.iter().map(|o| o.as_ref().map(SValue::from_core)).collect();
    if json(&outputs) != json(&entry.outputs) {
        differs.push("its outputs");
    }
    let performed: Vec<PerformedEntry> = host
        .performed
        .iter()
        .map(|(c, p)| PerformedEntry { capability: format!("{c:?}"), payload: SValue::from_core(p) })
        .collect();
    if json(&performed) != json(&entry.performed) {
        differs.push("the effects it performed");
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
        false if hidden => Rerun::Skipped("it used a secret or a resume token, which the log never holds".into()),
        false if entry.retries > 0 => {
            Rerun::Skipped("it ran again after what it read changed, and the log keeps only its last attempt".into())
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

/// Answers a run's waiting points and collection reads from its log entries; its tokens as the
/// placeholder the log shows.
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

/// `holon verify <export.json> [--head <hex>]`: check an export, say what was found, and exit 0
/// only if it checks.
pub(crate) fn run(args: &[String]) -> i32 {
    let mut file = None;
    let mut head = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--head" => head = args.next().cloned(),
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
    let report = check(&export, head.as_deref());
    print!("{}", describe(&report, head.as_deref()));
    if report.ok() { 0 } else { 1 }
}

const USAGE: &str = "usage: holon verify <export.json> [--head <hex>]

  Checks a deployment's runs, exported from GET /deployments/<slug>/export: that its run log's
  hash chain is unbroken, that every unit is what its hash says, and that every run, run again
  from its log entry, produces what the log says it did. --head checks that a head a witness
  holds is in the export, so no run up to it was changed or removed. Exits 0 when it all checks.";

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

/// `holon export <slug> [--server URL] [-o FILE]`: fetch a deployment's runs from a server, as the
/// editor's "export runs" does, signed in with the API token in `HOLON_TOKEN` (never on the
/// command line), and write them where `holon verify` reads them.
pub(crate) fn export(args: &[String]) -> i32 {
    let mut slug = None;
    let mut server = std::env::var("HOLON_SERVER").ok().filter(|s| !s.trim().is_empty());
    let mut out = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
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
    let server = server.unwrap_or_else(|| "http://127.0.0.1:7420".into());
    let server = server.trim_end_matches('/');
    let out = out.unwrap_or_else(|| format!("{slug}-runs.json"));
    let url = format!("{server}/deployments/{slug}/export");
    let mut request = ureq::get(&url).timeout(Duration::from_secs(120));
    if let Some(token) = std::env::var("HOLON_TOKEN").ok().filter(|t| !t.trim().is_empty()) {
        request = request.set("Authorization", &format!("Bearer {}", token.trim()));
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
                eprintln!("holon export: set HOLON_TOKEN to an API token (the editor's admin page, API tokens)");
            }
            return 1;
        }
        Err(e) => {
            eprintln!("holon export: can't reach {server}: {e}");
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
    if let Err(e) = std::fs::write(&out, &body) {
        eprintln!("holon export: can't write {out}: {e}");
        return 1;
    }
    let head = export.head.get(..12).unwrap_or(&export.head);
    println!("wrote {out}: {} runs of {slug:?}, head {head}", export.lines.len());
    println!("check it anywhere: holon verify {out}");
    0
}

const EXPORT_USAGE: &str = "usage: holon export <slug> [--server URL] [-o FILE]

  Writes a deployment's runs (its run log, exactly as written, and every unit they ran) to FILE
  (default <slug>-runs.json), for holon verify. --server is the holon server (default
  http://127.0.0.1:7420; or HOLON_SERVER). Signs in with the API token in HOLON_TOKEN.";
