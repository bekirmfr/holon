//! A flow's log lines (docs/L4s/logs-L4.md): what `console.log` and the rest write, kept by the
//! host beside a deployment's runs, and no effect a run performed: `holon verify` checks a run
//! without them, and a replay writes none.
//!
//! One file for each deployment, `<dir>/<slug>.jsonl`, a line of JSON each, rolled at a size cap
//! into `<slug>.1.jsonl` and `<slug>.2.jsonl`, the oldest dropped. Lines are buffered and written
//! about once a second, on a thread of their own: a run never waits for them, and a crash can
//! lose the last second. A server that keeps nothing keeps the newest lines in memory.
//!
//! Bounded on every side: a line is cut at [`LINE_BYTES`], a deployment writes at most
//! [`LINES_A_SECOND`] (these and the files' size and number are [`Caps`], the server's settings), and
//! what is dropped is counted: the next line kept says how many. A server told `--log-stdout` also
//! prints each line, from the same thread that writes the files.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use holon_core::Value;
use serde::{Deserialize, Serialize};

use crate::paths::Place;

/// The longest a line's text goes: the rest is cut, and the line says so.
const LINE_BYTES: usize = 4096;
/// A file is rolled when the lines to add would take it past this.
const FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Files kept for a deployment: the current one, and the two it was rolled into.
const FILES: usize = 3;
/// The most lines a deployment writes in a second: the rest are dropped, and counted.
const LINES_A_SECOND: u64 = 1000;
/// What a server that keeps nothing holds for each deployment.
const MEMORY_LINES: usize = 5000;
/// How often the buffered lines are written.
const FLUSH_EVERY: Duration = Duration::from_secs(1);
/// Each deployment's buffered lines, taken to be written.
type Written = Vec<(String, Vec<Line>)>;

/// What a server is set to keep, and say: how big each file of a deployment grows before it is rolled,
/// how many files it keeps, how many lines a second it writes, and whether each line is also printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Caps {
    pub(crate) file_bytes: u64,
    pub(crate) files: usize,
    pub(crate) per_second: u64,
    pub(crate) stdout: bool,
}

impl Default for Caps {
    fn default() -> Caps {
        Caps { file_bytes: FILE_BYTES, files: FILES, per_second: LINES_A_SECOND, stdout: false }
    }
}

/// The most files a deployment can be set to keep, and so the most that are looked for to forget.
pub(crate) const MOST_FILES: usize = 10;
/// The least a file can be set to hold: more than a line.
pub(crate) const LEAST_FILE_BYTES: u64 = 64 * 1024;

/// One line of a deployment's log.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Line {
    /// When it was written, in ms since the epoch.
    pub(crate) at: u64,
    /// `debug`, `log`, `info`, `warn` or `error`: the console's own methods.
    pub(crate) level: String,
    /// The version it was written by: the first 8 hex of its unit's hash.
    pub(crate) unit: String,
    /// The run it was written in: its seq in the deployment's log.
    pub(crate) run: u64,
    pub(crate) text: String,
    /// More than 0: this line stands for that many that were dropped before it (a rate over its
    /// cap, a file that couldn't be written), and its text says so.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) dropped: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// How loud a level is, to filter by: `debug` is the quietest, `log` and `info` the same.
fn rank(level: &str) -> u8 {
    match level {
        "debug" => 0,
        "warn" => 2,
        "error" => 3,
        _ => 1,
    }
}

/// What to read: lines at or above a level, of one run, or up to the end of one, after a time.
#[derive(Clone, Debug, Default)]
pub(crate) struct Query {
    pub(crate) level: Option<String>,
    /// Only the lines this run wrote.
    pub(crate) run: Option<u64>,
    /// Every line up to and including this run's.
    pub(crate) until: Option<u64>,
    /// Only lines written after this time (ms), to read on from where a reader was.
    pub(crate) after: Option<u64>,
    /// The newest this many, oldest first; 0 is as many as are kept.
    pub(crate) limit: usize,
}

impl Query {
    fn takes(&self, line: &Line) -> bool {
        self.level.as_deref().is_none_or(|l| rank(&line.level) >= rank(l))
            && self.run.is_none_or(|r| line.run == r)
            && self.until.is_none_or(|u| line.run <= u)
            && self.after.is_none_or(|a| line.at > a)
    }
}

/// A deployment's lines not yet written, its rate window, and what it has dropped.
#[derive(Default)]
struct Pending {
    buffer: Vec<Line>,
    window: Option<Instant>,
    in_window: u64,
    dropped: u64,
    /// Lines not yet printed (`--log-stdout`), with the time each was written.
    printed: Vec<(u64, String)>,
    /// When its last line was written: the next is never earlier, so a reader that has the last
    /// it saw reads on from there (`after`) and misses none.
    last_at: u64,
    /// A server that keeps nothing: the newest lines.
    memory: VecDeque<Line>,
}

struct Inner {
    /// Where the files go; none: nowhere, the lines are kept in memory.
    dir: Option<PathBuf>,
    caps: Caps,
    slugs: Mutex<BTreeMap<String, Pending>>,
    flusher: AtomicBool,
}

/// The lines of every deployment on this server.
#[derive(Clone)]
pub(crate) struct Logs(Arc<Inner>);

impl Logs {
    /// Kept nowhere: the tests, and a server told to keep nothing.
    pub(crate) fn memory() -> Logs {
        Logs::new(None, Caps::default())
    }

    fn new(dir: Option<PathBuf>, caps: Caps) -> Logs {
        Logs(Arc::new(Inner { dir, caps, slugs: Mutex::new(BTreeMap::new()), flusher: AtomicBool::new(false) }))
    }

    /// In a folder beside the deployments file (`<deployments>.logs/`), or in memory when that is
    /// kept nowhere.
    pub(crate) fn beside(deployments_path: &Place, caps: Caps) -> Logs {
        Logs::new(deployments_path.beside("logs").map(PathBuf::from), caps)
    }

    /// The lines a run wrote, at their levels, for `slug`: cut, counted against its rate, and
    /// buffered to be written. `run` is the run's seq in the deployment's log, `unit` its version.
    pub(crate) fn write(&self, slug: &str, run: u64, unit: &str, at: u64, lines: &[(String, String)]) {
        if lines.is_empty() || !is_file_name(slug) {
            return;
        }
        let unit: String = unit.chars().take(8).collect();
        {
            let mut slugs = self.0.slugs.lock().unwrap();
            let pending = slugs.entry(slug.to_string()).or_default();
            for (level, text) in lines {
                let at = at.max(pending.last_at + 1);
                pending.last_at = at;
                let now = Instant::now();
                if pending.window.is_none_or(|w| now.duration_since(w) >= Duration::from_secs(1)) {
                    pending.window = Some(now);
                    pending.in_window = 0;
                }
                if pending.in_window >= self.0.caps.per_second {
                    pending.dropped += 1;
                    continue;
                }
                pending.in_window += 1;
                if pending.dropped > 0 {
                    let n = std::mem::take(&mut pending.dropped);
                    let marker = Line { at, level: "warn".into(), unit: unit.clone(), run, text: format!("{n} lines were dropped: over {} a second", self.0.caps.per_second), dropped: n };
                    self.keep(pending, slug, marker);
                }
                self.keep(pending, slug, Line { at, level: level.clone(), unit: unit.clone(), run, text: cut(text), dropped: 0 });
            }
        }
        self.start_flusher();
    }

    /// The lines of `slug` a query takes, oldest first.
    pub(crate) fn read(&self, slug: &str, query: &Query) -> Vec<Line> {
        if !is_file_name(slug) {
            return Vec::new();
        }
        self.flush();
        let mut all: Vec<Line> = match &self.0.dir {
            None => self.0.slugs.lock().unwrap().get(slug).map(|p| p.memory.iter().cloned().collect()).unwrap_or_default(),
            Some(dir) => {
                let mut lines = Vec::new();
                for n in (0..self.0.caps.files).rev() {
                    let Ok(text) = std::fs::read_to_string(file_of(dir, slug, n)) else { continue };
                    lines.extend(text.lines().filter_map(|l| serde_json::from_str::<Line>(l).ok()));
                }
                lines
            }
        };
        all.retain(|l| query.takes(l));
        if query.limit > 0 && all.len() > query.limit {
            all.drain(..all.len() - query.limit);
        }
        all
    }

    /// `slug`'s lines gone, with its deployment.
    pub(crate) fn forget(&self, slug: &str) {
        if !is_file_name(slug) {
            return;
        }
        self.0.slugs.lock().unwrap().remove(slug);
        if let Some(dir) = &self.0.dir {
            for n in 0..MOST_FILES {
                let _ = std::fs::remove_file(file_of(dir, slug, n));
            }
        }
    }

    /// Every buffered line written now, each deployment's to its own file, rolled first when the
    /// lines to add would take it past its cap. A file that can't be written counts its lines as
    /// dropped: the run that wrote them has long since answered.
    pub(crate) fn flush(&self) {
        let (taken, mut printed): (Written, Vec<(u64, String)>) = {
            let mut slugs = self.0.slugs.lock().unwrap();
            let taken = slugs.iter_mut().filter(|(_, p)| !p.buffer.is_empty()).map(|(s, p)| (s.clone(), std::mem::take(&mut p.buffer))).collect();
            (taken, slugs.values_mut().flat_map(|p| std::mem::take(&mut p.printed)).collect())
        };
        if !printed.is_empty() {
            printed.sort_by_key(|(at, _)| *at);
            let mut out = std::io::stdout().lock();
            for (_, line) in printed {
                let _ = writeln!(out, "{line}");
            }
        }
        let Some(dir) = &self.0.dir else { return };
        for (slug, lines) in taken {
            let mut text = String::new();
            for line in &lines {
                if let Ok(json) = serde_json::to_string(line) {
                    text.push_str(&json);
                    text.push('\n');
                }
            }
            if append(dir, &slug, text.as_bytes(), &self.0.caps).is_err() {
                self.0.slugs.lock().unwrap().entry(slug).or_default().dropped += lines.len() as u64;
            }
        }
    }

    /// A thread that writes the buffered lines every second, begun with the first line, and gone
    /// when the logs are.
    fn start_flusher(&self) {
        if (self.0.dir.is_none() && !self.0.caps.stdout) || self.0.flusher.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(FLUSH_EVERY);
                let Some(inner) = weak.upgrade() else { return };
                Logs(inner).flush();
            }
        });
    }
}

impl Logs {
    /// A line kept: in memory, or to be written, and to be printed when the server is set to.
    fn keep(&self, pending: &mut Pending, slug: &str, line: Line) {
        if self.0.caps.stdout {
            pending.printed.push((line.at, printed_line(slug, &line)));
        }
        pending.push(line, self.0.dir.is_none());
    }
}

/// A line as `--log-stdout` prints it, on one line: `11:37:09.195 loud: LOG   run 0  asked ada`, the
/// time in UTC as `holon logs` tells it, the deployment named, a newline in the text written out.
pub(crate) fn printed_line(slug: &str, l: &Line) -> String {
    let secs = l.at / 1000;
    let clock = format!("{:02}:{:02}:{:02}.{:03}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60, l.at % 1000);
    format!("{clock} {slug}: {:<5} run {}  {}", l.level.to_uppercase(), l.run, l.text.replace('\n', "\\n"))
}

impl Pending {
    fn push(&mut self, line: Line, in_memory: bool) {
        if in_memory {
            self.memory.push_back(line);
            while self.memory.len() > MEMORY_LINES {
                self.memory.pop_front();
            }
        } else {
            self.buffer.push(line);
        }
    }
}

/// A line's text: a text as it is, any other value as JSON.
pub(crate) fn text_of(value: &Value) -> String {
    match value {
        Value::Text(t) => t.clone(),
        other => holon_core::stringify_json(other).unwrap_or_else(|| "(a value with no JSON form)".into()),
    }
}

/// `text` cut at [`LINE_BYTES`], saying so.
fn cut(text: &str) -> String {
    if text.len() <= LINE_BYTES {
        return text.to_string();
    }
    let mut end = LINE_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… (cut: {} bytes)", &text[..end], text.len())
}

/// Whether `slug` can name a file here: no path in it.
fn is_file_name(slug: &str) -> bool {
    !slug.is_empty() && slug.len() <= 128 && slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `<dir>/<slug>.jsonl` for the current file (`n` 0), `<slug>.1.jsonl` for the one before.
fn file_of(dir: &Path, slug: &str, n: usize) -> PathBuf {
    if n == 0 { dir.join(format!("{slug}.jsonl")) } else { dir.join(format!("{slug}.{n}.jsonl")) }
}

/// `bytes` added to `slug`'s file, which is rolled first if they would take it past its cap.
fn append(dir: &Path, slug: &str, bytes: &[u8], caps: &Caps) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let current = file_of(dir, slug, 0);
    let size = std::fs::metadata(&current).map_or(0, |m| m.len());
    if size > 0 && size + bytes.len() as u64 > caps.file_bytes {
        // The oldest goes; each of the others moves one on; the current one becomes the first.
        let _ = std::fs::remove_file(file_of(dir, slug, caps.files - 1));
        for n in (0..caps.files - 1).rev() {
            let _ = std::fs::rename(file_of(dir, slug, n), file_of(dir, slug, n + 1));
        }
    }
    std::fs::OpenOptions::new().create(true).append(true).open(current)?.write_all(bytes)
}

/// `GET /deployments/<slug>/logs?level=warn&run=17&until=17&after=<at>&limit=200`: a deployment's
/// lines, oldest first (docs/L4s/logs-L4.md §5). A person signed in reads them. An agent reads
/// them only where the deployment's grant lets agents (§6): `AgentReadsLogs`, which a person approves.
pub(crate) fn handle_get_logs(state: &crate::AppState, slug: &str, caller: &crate::access::Caller, request: &rouille::Request) -> rouille::Response {
    use crate::http::err;
    if !state.deployments.lock().unwrap().contains_key(slug) {
        return err(404, format!("no deployment {slug:?}"));
    }
    let agents_may = state.deployments.lock().unwrap().get(slug).is_some_and(|d| d.grant.items.iter().any(|i| i == "AgentReadsLogs"));
    if let Err(refusal) = crate::can::can(caller, crate::can::Action::RunView, &crate::can::Resource::Logs { slug, agents_may }) {
        return refusal.response();
    }
    let number = |name: &str| request.get_param(name).and_then(|v| v.parse::<u64>().ok());
    let query = Query {
        level: request.get_param("level").filter(|l| ["debug", "log", "info", "warn", "error"].contains(&l.as_str())),
        run: number("run"),
        until: number("until"),
        after: number("after"),
        limit: number("limit").map_or(200, |n| n.clamp(1, 5000)) as usize,
    };
    rouille::Response::json(&serde_json::json!({ "slug": slug, "lines": state.folders.logs.read(slug, &query) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> String {
        t.to_string()
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-logs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn files_logs(dir: &Path) -> Logs {
        files_logs_with(dir, Caps::default())
    }

    /// Logs in `dir` set as `caps` say, with no thread of their own: the tests flush when they mean to.
    fn files_logs_with(dir: &Path, caps: Caps) -> Logs {
        let logs = Logs::new(Some(dir.to_path_buf()), caps);
        logs.0.flusher.store(true, Ordering::SeqCst);
        logs
    }

    #[test]
    fn lines_are_read_back_oldest_first_with_their_run_and_level() {
        let logs = Logs::memory();
        logs.write("shop", 3, "abcdef0123456789", 10, &[("log".into(), text("paying")), ("warn".into(), text("7"))]);
        logs.write("shop", 4, "abcdef0123456789", 20, &[("error".into(), text("refused"))]);
        let all = logs.read("shop", &Query::default());
        assert_eq!(all.iter().map(|l| (l.run, l.level.as_str(), l.text.as_str())).collect::<Vec<_>>(), [(3, "log", "paying"), (3, "warn", "7"), (4, "error", "refused")]);
        assert_eq!(all[0].unit, "abcdef01", "a version is its first 8 hex");
        assert_eq!(logs.read("other", &Query::default()), vec![], "a deployment's own");
    }

    #[test]
    fn a_query_takes_a_level_a_run_or_up_to_one() {
        let logs = Logs::memory();
        for run in 1..=3 {
            logs.write("a", run, "u", run * 10, &[("debug".into(), text("d")), ("warn".into(), text("w"))]);
        }
        let q = |level: Option<&str>, run: Option<u64>, until: Option<u64>, after: Option<u64>, limit: usize| Query { level: level.map(String::from), run, until, after, limit };
        assert_eq!(logs.read("a", &q(Some("warn"), None, None, None, 0)).len(), 3, "warn and louder");
        assert_eq!(logs.read("a", &q(None, Some(2), None, None, 0)).len(), 2, "one run's");
        assert_eq!(logs.read("a", &q(None, None, Some(2), None, 0)).len(), 4, "up to the end of run 2");
        assert_eq!(logs.read("a", &q(None, None, None, Some(21), 0)).iter().map(|l| l.run).collect::<Vec<_>>(), [3, 3], "after a time: a deployment's times rise line by line");
        assert_eq!(logs.read("a", &q(None, None, None, None, 2)).iter().map(|l| l.level.as_str()).collect::<Vec<_>>(), ["debug", "warn"], "the newest two");
    }

    #[test]
    fn a_long_line_is_cut_and_says_so() {
        let logs = Logs::memory();
        logs.write("a", 1, "u", 1, &[("log".into(), text(&"é".repeat(5000)))]);
        let line = &logs.read("a", &Query::default())[0];
        assert!(line.text.len() < LINE_BYTES + 40 && line.text.contains("(cut: 10000 bytes)"), "{}", line.text.len());
    }

    #[test]
    fn a_rate_over_its_cap_drops_lines_and_the_next_one_says_how_many() {
        let logs = Logs::memory();
        let many: Vec<(String, String)> = (0..LINES_A_SECOND as usize + 5).map(|i| ("log".to_string(), i.to_string())).collect();
        logs.write("a", 1, "u", 1, &many);
        assert_eq!(logs.read("a", &Query::default()).len(), LINES_A_SECOND as usize, "the rest are dropped");
        // A new second: what was dropped is said before the next line.
        logs.0.slugs.lock().unwrap().get_mut("a").unwrap().window = Some(Instant::now() - Duration::from_secs(2));
        logs.write("a", 2, "u", 2, &[("log".into(), text("again"))]);
        let all = logs.read("a", &Query::default());
        let marker = &all[all.len() - 2];
        assert_eq!((marker.dropped, marker.level.as_str()), (5, "warn"));
        assert!(marker.text.starts_with("5 lines were dropped"));
        assert_eq!(all.last().unwrap().text, "again");
    }

    #[test]
    fn files_roll_at_their_cap_and_keep_three() {
        let dir = temp("roll");
        let logs = files_logs(&dir);
        let big = "x".repeat(LINE_BYTES - 100);
        let per_file = FILE_BYTES as usize / (LINE_BYTES - 50);
        for run in 0..(per_file * 4) as u64 {
            // A new second each time: the rate cap is another test's.
            if let Some(p) = logs.0.slugs.lock().unwrap().get_mut("a") {
                p.window = None;
            }
            logs.write("a", run, "u", run, &[("log".into(), text(&big))]);
            if run % 100 == 0 {
                logs.flush();
            }
        }
        logs.flush();
        let on_disk: Vec<_> = (0..4).map(|n| file_of(&dir, "a", n).exists()).collect();
        assert_eq!(on_disk, [true, true, true, false], "the current, and the two it was rolled into");
        let read = logs.read("a", &Query::default());
        assert!(!read.is_empty() && read.windows(2).all(|w| w[0].run < w[1].run), "oldest first, across the files");
        assert_eq!(read.last().unwrap().run, (per_file * 4) as u64 - 1, "the newest is kept");
        assert!(read.first().unwrap().run > 0, "the oldest went");
        logs.forget("a");
        assert!(!file_of(&dir, "a", 0).exists() && logs.read("a", &Query::default()).is_empty(), "gone with its deployment");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_written_is_there_after_the_logs_are_made_again() {
        let dir = temp("again");
        let logs = files_logs(&dir);
        logs.write("a", 1, "u", 1, &[("info".into(), text("kept"))]);
        logs.flush();
        let again = files_logs(&dir);
        assert_eq!(again.read("a", &Query::default())[0].text, "kept", "after a restart");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_caps_a_server_is_set_to_are_the_ones_kept() {
        let dir = temp("caps");
        let caps = Caps { file_bytes: LEAST_FILE_BYTES, files: 2, per_second: 3, stdout: false };
        let logs = files_logs_with(&dir, caps);
        // Over 3 a second: the rest are dropped, and said.
        logs.write("a", 1, "u", 1, &(0..5).map(|i| ("log".to_string(), i.to_string())).collect::<Vec<_>>());
        assert_eq!(logs.read("a", &Query::default()).len(), 3);
        // Two files of 64 KiB: a third rolling drops the oldest.
        let big = "x".repeat(LINE_BYTES - 100);
        for run in 0..60u64 {
            if let Some(p) = logs.0.slugs.lock().unwrap().get_mut("a") {
                p.window = None;
            }
            logs.write("a", 100 + run, "u", 100 + run, &[("log".into(), big.clone())]);
            logs.flush();
        }
        assert!(file_of(&dir, "a", 0).exists() && file_of(&dir, "a", 1).exists() && !file_of(&dir, "a", 2).exists(), "two files");
        let read = logs.read("a", &Query::default());
        assert!(read.first().unwrap().run > 100 && read.last().unwrap().run == 159, "the oldest went, the newest is kept");
        // Forgotten, whatever the number it was once set to keep.
        std::fs::write(file_of(&dir, "a", 5), "{}").unwrap();
        logs.forget("a");
        assert!(!file_of(&dir, "a", 0).exists() && !file_of(&dir, "a", 5).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_server_set_to_print_has_each_line_ready_to_print_on_one_line() {
        let logs = Logs::new(None, Caps { stdout: true, ..Caps::default() });
        logs.write("loud", 3, "u", 1_794_257_758_195, &[("warn".into(), text("two\nlines"))]);
        let printed: Vec<String> = logs.0.slugs.lock().unwrap().get("loud").unwrap().printed.iter().map(|(_, l)| l.clone()).collect();
        assert_eq!(printed, ["20:55:58.195 loud: WARN  run 3  two\\nlines"]);
        // Not set to: nothing is held to print.
        let quiet = Logs::memory();
        quiet.write("loud", 3, "u", 1, &[("log".into(), text("x"))]);
        assert!(quiet.0.slugs.lock().unwrap().get("loud").unwrap().printed.is_empty());
    }

    #[test]
    fn a_name_that_is_a_path_writes_nothing() {
        let logs = Logs::memory();
        logs.write("../x", 1, "u", 1, &[("log".into(), text("no"))]);
        assert!(logs.read("../x", &Query::default()).is_empty());
    }
}
