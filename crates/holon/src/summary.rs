//! The host at a glance, as the console's Overview shows it (`GET /summary?hours=24`):
//!
//! - each deployment's runs and failures hour by hour, counted from what its log keeps of each run
//!   with no line read (recorder.rs's `hourly`), and its unrecorded handlers' runs (their tallies,
//!   which have no hours);
//! - the host's totals: runs and failures in the window, and how many flows run, are paused or are
//!   stopped;
//! - the newest runs across every flow, from what each log holds in memory: when, which route, the
//!   status it answered and whether it failed, never what came in or went out;
//! - its run logs: how many runs they hold on record, the breaks each found in its chain as it was
//!   read and written, the witness its heads go to (heads.rs), and the last check.
//!
//! `POST /summary/check` checks every log again now: it reads each line of each (with the log's
//! lock released, `LogCell::each_line`) and finds where a line doesn't name the hash of the one
//! before it, as the log does as it opens. What it found is kept, and the summary says it until
//! the next check (kept in `AppState::checked`). The same chain `holon verify` checks offline, for one deployment's export.

use std::sync::Arc;

use rouille::{Request, Response};
use serde::{Deserialize, Serialize};

use crate::lifecycle::Status;
use crate::recorder::{line_hash, LogCell};
use crate::AppState;

/// The longest window a summary counts: a week of hours.
const MAX_HOURS: usize = 24 * 7;
/// How many of the newest runs across every flow it says.
const LATEST: usize = 10;

/// A summary, as `GET /summary` answers it.
#[derive(Serialize)]
struct Summary {
    /// When it was made, and the hours it counts: `hours` of them ending at `at_ms`, the oldest
    /// first in each deployment's `runs` and `failed`.
    at_ms: u64,
    hours: usize,
    totals: Totals,
    deployments: Vec<FlowSummary>,
    latest: Vec<Latest>,
    chain: Chain,
}

#[derive(Serialize, Default)]
struct Totals {
    /// Runs the logs hold that started in the window, and of them those that failed.
    runs: u64,
    failed: u64,
    running: usize,
    paused: usize,
    stopped: usize,
}

#[derive(Serialize)]
struct FlowSummary {
    slug: String,
    status: Status,
    /// Its runs in each hour, and of them those that failed (an error, or a 5xx answered).
    runs: Vec<u64>,
    failed: Vec<u64>,
    /// Its unrecorded handlers' runs, all told: never logged, so never in an hour.
    unrecorded: Unrecorded,
    /// How many runs its log holds, its head (hex), and the runs whose line doesn't name the one
    /// before it.
    on_record: usize,
    head: String,
    breaks: Vec<u64>,
}

#[derive(Serialize, Default)]
struct Unrecorded {
    runs: u64,
    failed: u64,
}

/// One of the newest runs: which, when, how long, and how it ended.
#[derive(Serialize)]
struct Latest {
    slug: String,
    seq: u64,
    at_ms: u64,
    duration_ms: f64,
    kind: String,
    route: Option<String>,
    status: Option<u16>,
    failed: bool,
    /// It waited, for an approval or an event, rather than answering: a later run goes on with it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    waited: bool,
}

#[derive(Serialize)]
struct Chain {
    /// The runs every log holds, and how many breaks they found as they were read and written.
    on_record: usize,
    breaks: usize,
    /// Where the logs' heads go, and when it last took a batch, if heads are exported.
    witness: Option<crate::heads::Witness>,
    /// The last check, if there was one since the server started.
    checked: Option<Check>,
}

/// What a check found (`POST /summary/check`).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Check {
    at_ms: u64,
    /// The lines it read, across every log.
    runs: u64,
    /// Each run whose line doesn't name the hash of the line before it.
    breaks: Vec<Break>,
    /// Each log's head as it read it: the hash of its newest line.
    heads: Vec<(String, String)>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
struct Break {
    slug: String,
    seq: u64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// `GET /summary?hours=24` (1 to a week of hours), of the deployments `who` may see the runs of.
pub(crate) fn handle_get_summary(state: &AppState, request: &Request, who: &crate::access::Caller) -> Response {
    let hours = request.get_param("hours").and_then(|h| h.parse::<usize>().ok()).unwrap_or(24).clamp(1, MAX_HOURS);
    let sees = |slug: &str| crate::can::can(who, crate::can::Action::RunView, &crate::can::Resource::Flow(slug)).is_ok();
    Response::json(&summary(state, now_ms(), hours, sees))
}

fn summary(state: &AppState, at_ms: u64, hours: usize, sees: impl Fn(&str) -> bool) -> Summary {
    // The table only to find each log, so a summary never holds up a deployment's runs.
    let found: Vec<(String, Status, Unrecorded, Arc<LogCell>)> = state
        .deployments
        .lock()
        .unwrap()
        .iter()
        .filter(|(slug, _)| sees(slug))
        .map(|(slug, dep)| {
            let unrecorded = dep.tallies.values().fold(Unrecorded::default(), |u, t| Unrecorded { runs: u.runs + t.runs, failed: u.failed + t.failed });
            (slug.clone(), dep.life.status, unrecorded, Arc::clone(&dep.log))
        })
        .collect();
    let mut totals = Totals::default();
    let mut latest: Vec<Latest> = Vec::new();
    let mut deployments = Vec::with_capacity(found.len());
    for (slug, status, unrecorded, log) in found {
        let ((runs, failed), on_record, head, breaks) = log.with(|log| {
            latest.extend(log.newest().take(LATEST).map(|e| Latest {
                slug: slug.clone(),
                seq: e.seq,
                at_ms: e.at_ms,
                duration_ms: e.duration_ms,
                kind: e.kind.clone(),
                route: e.route.clone(),
                status: e.status,
                failed: crate::recorder::ran_failed(e),
                waited: !e.parked.is_empty(),
            }));
            (log.hourly(at_ms, hours), log.runs_kept(), log.head().to_string(), log.breaks().to_vec())
        });
        totals.runs += runs.iter().sum::<u64>();
        totals.failed += failed.iter().sum::<u64>();
        match status {
            Status::Running => totals.running += 1,
            Status::Paused => totals.paused += 1,
            Status::Stopped => totals.stopped += 1,
        }
        deployments.push(FlowSummary { slug, status, runs, failed, unrecorded, on_record, head, breaks });
    }
    latest.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| b.seq.cmp(&a.seq)));
    latest.truncate(LATEST);
    let chain = Chain {
        on_record: deployments.iter().map(|d| d.on_record).sum(),
        breaks: deployments.iter().map(|d| d.breaks.len()).sum(),
        witness: crate::heads::witness(),
        checked: state.checked.lock().unwrap().clone(),
    };
    Summary { at_ms, hours, totals, deployments, latest, chain }
}

/// `POST /summary/check`: every log's chain checked again now, line by line; what it found is
/// answered and kept for the summary.
pub(crate) fn handle_post_check(state: &AppState) -> Response {
    let check = check_all(state, now_ms());
    *state.checked.lock().unwrap() = Some(check.clone());
    Response::json(&check)
}

fn check_all(state: &AppState, at_ms: u64) -> Check {
    let logs: Vec<(String, Arc<LogCell>)> = state.deployments.lock().unwrap().iter().map(|(slug, dep)| (slug.clone(), Arc::clone(&dep.log))).collect();
    let mut check = Check { at_ms, runs: 0, breaks: Vec::new(), heads: Vec::with_capacity(logs.len()) };
    for (slug, log) in logs {
        let (runs, breaks, head) = check_log(&log);
        check.runs += runs;
        check.breaks.extend(breaks.into_iter().map(|seq| Break { slug: slug.clone(), seq }));
        check.heads.push((slug, head));
    }
    check
}

/// What a check reads of a line: its run and the hash it names.
#[derive(Deserialize)]
struct Linked {
    seq: u64,
    #[serde(default)]
    prev: String,
}

/// One log's lines read again: how many, the runs whose line doesn't follow the one before (as
/// the log finds them as it opens, recorder.rs), and the hash of the newest. A line that doesn't
/// parse (the newest, cut short as it is written) is left out.
fn check_log(log: &LogCell) -> (u64, Vec<u64>, String) {
    let (mut runs, mut breaks, mut head, mut chained) = (0u64, Vec::new(), String::new(), false);
    log.each_line(|line| {
        let Ok(linked) = serde_json::from_str::<Linked>(line) else { return };
        if (chained || !linked.prev.is_empty()) && !head.is_empty() && linked.prev != head {
            breaks.push(linked.seq);
        }
        chained |= !linked.prev.is_empty();
        head = line_hash(line);
        runs += 1;
    });
    (runs, breaks, head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::{Log, LogEntry};

    fn entry(at_ms: u64, status: Option<u16>, error: Option<&str>) -> LogEntry {
        LogEntry { at_ms, status, error: error.map(str::to_string), kind: "request".into(), route: Some("GET /".into()), ..LogEntry::default() }
    }

    const HOUR: u64 = 3600 * 1000;

    #[test]
    fn a_log_counts_its_runs_and_failures_by_hour() {
        let now = 100 * HOUR;
        let mut log = Log::open(None);
        // One before the window, not counted; three hours back, one that answered; two back, one
        // that failed (an error); this last hour, a 503 (failed) and a 404 (answered, not failed).
        log.push(entry(now - 30 * HOUR, Some(200), None));
        log.push(entry(now - 2 * HOUR - 1, Some(200), None));
        log.push(entry(now - 2 * HOUR + 5, None, Some("boom")));
        log.push(entry(now - HOUR + 1, Some(503), None));
        log.push(entry(now - 10, Some(404), None));
        let (runs, failed) = log.hourly(now, 24);
        assert_eq!(runs.len(), 24);
        assert_eq!(&runs[21..], &[1, 1, 2]);
        assert_eq!(&failed[21..], &[0, 1, 1]);
        assert_eq!(runs.iter().sum::<u64>(), 4, "the run before the window isn't counted");
        assert_eq!(log.runs_kept(), 5);
    }

    #[test]
    fn failures_of_runs_no_longer_kept_are_let_go_of() {
        let now = 10 * HOUR;
        let mut log = Log::open_keeping(None, 2);
        log.push(entry(now - 3, None, Some("boom")));
        log.push(entry(now - 2, Some(200), None));
        log.push(entry(now - 1, Some(200), None));
        let (runs, failed) = log.hourly(now, 1);
        assert_eq!((runs, failed), (vec![2], vec![0]));
    }

    #[test]
    fn a_check_reads_every_line_and_finds_a_line_changed_outside_holon() {
        let dir = std::env::temp_dir().join(format!("holon-summary-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("flow.log.jsonl");
        let mut log = Log::open(Some(path.clone()));
        for i in 0..4 {
            log.push(entry(1000 + i, Some(200), None));
        }
        let head = log.head().to_string();
        drop(log);
        let cell = LogCell::new(Log::open(Some(path.clone())));
        let (runs, breaks, read) = check_log(&cell);
        assert_eq!((runs, breaks, read), (4, vec![], head));
        // Run 2's line changed: run 3 no longer names it.
        let text = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        lines[2] = lines[2].replace("\"status\":200", "\"status\":201");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let cell = LogCell::new(Log::open(Some(path)));
        let (runs, breaks, _) = check_log(&cell);
        assert_eq!((runs, breaks), (4, vec![3]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
