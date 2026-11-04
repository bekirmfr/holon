//! How the host keeps what it stores (docs/L4s/storage-L4.md): the server's own sweeps, on the
//! schedule `[storage] sweep_every` sets, and what an admin sees of them (`GET /admin/host`) and
//! sets (`POST /admin/storage`, which writes `holon.toml` and reloads it).

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::permissions::{every_text, parse_every, Policy};
use crate::{err, folders, sweep, AppState, Response};

/// How often the server looks at whether a sweep is due: a changed `sweep_every` applies within it.
pub(crate) const LOOK_EVERY: Duration = Duration::from_secs(10);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// What one sweep did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Report {
    /// When it began (ms since 1970), and how long it took.
    pub(crate) at_ms: u64,
    pub(crate) took_ms: u64,
    /// What it removed: values from the store, texts from the folders' blob store.
    pub(crate) removed: u64,
    pub(crate) bytes: u64,
    /// Nothing was put since a sweep that spared nothing: it had nothing to look at.
    pub(crate) skipped: bool,
    pub(crate) error: Option<String>,
}

#[derive(Default)]
struct Kept {
    /// When the last round of sweeps ended, or the server began: the next is due `sweep_every`
    /// after it.
    since_ms: u64,
    store: Option<Report>,
    folders: Option<Report>,
    /// Whether a round is being made now.
    sweeping: bool,
}

/// The server's own sweeps: when they are due, and what the last of each did.
#[derive(Default)]
pub(crate) struct Sweeps {
    kept: Mutex<Kept>,
    /// What the store's sweep remembers between rounds (whether anything was put since).
    scheduled: Mutex<sweep::Scheduled>,
}

impl Sweeps {
    /// Look at whether a round is due at `now_ms` under the policy in force, and make it if so:
    /// the store's sweep (paced), then the folders' texts'. True if one was made. Called every
    /// `LOOK_EVERY` by the server's housekeeping thread; the first round is due a `sweep_every`
    /// after the first look.
    pub(crate) fn tick(&self, state: &AppState, now_ms: u64) -> bool {
        let every = state.policy().storage.sweep_every;
        {
            let mut kept = self.kept.lock().unwrap();
            if kept.since_ms == 0 {
                kept.since_ms = now_ms;
            }
            let due = every.is_some_and(|e| now_ms >= kept.since_ms.saturating_add(e.as_millis() as u64));
            if !due || kept.sweeping {
                return false;
            }
            kept.sweeping = true;
        }
        let store = self.sweep_store(state, now_ms);
        let folders = sweep_folders(state, now_ms);
        let mut kept = self.kept.lock().unwrap();
        kept.store = store.or(kept.store.take());
        kept.folders = Some(folders);
        kept.since_ms = now_ms.max(self::now_ms());
        kept.sweeping = false;
        true
    }

    /// The store's sweep, paced; none for a store in memory.
    fn sweep_store(&self, state: &AppState, now_ms: u64) -> Option<Report> {
        let began = Instant::now();
        let mut report = Report { at_ms: now_ms, ..Report::default() };
        match self.scheduled.lock().unwrap().sweep(&state.store, &state.deployments, now_ms) {
            Ok(Some(s)) => {
                report.removed = s.values as u64;
                report.bytes = s.bytes;
                if s.values > 0 {
                    let freed = format!("{} values, {} freed", s.values, folders::size_text(s.bytes));
                    println!("holon: swept the store: {freed}, in {:.1} s", began.elapsed().as_secs_f64());
                    state.activity.record("swept the store", &freed, None);
                }
            }
            // Nothing put since a sweep that spared nothing; or a store in memory, never swept.
            Ok(None) if state.store.2.on_disk() => report.skipped = true,
            Ok(None) => return None,
            Err(e) => {
                eprintln!("holon: sweeping the store: {e}");
                report.error = Some(e);
            }
        }
        report.took_ms = began.elapsed().as_millis() as u64;
        Some(report)
    }

    /// `GET /admin/host`'s part about sweeping.
    fn view(&self, policy: &Policy) -> serde_json::Value {
        let every = policy.storage.sweep_every;
        let kept = self.kept.lock().unwrap();
        serde_json::json!({
            "every": every_text(every),
            "next_at_ms": every.map(|e| kept.since_ms.saturating_add(e.as_millis() as u64)),
            "sweeping": kept.sweeping,
            "store": kept.store,
            "folders": kept.folders,
        })
    }
}

/// A span of milliseconds, said roughly: "40 s", "7 min", "3 h", "2 days" (the editor's `span`).
fn span(ms: u64) -> String {
    let s = (ms + 500) / 1000;
    if s < 90 {
        return format!("{s} s");
    }
    let m = (s + 30) / 60;
    if m < 90 {
        return format!("{m} min");
    }
    let h = (m + 30) / 60;
    if h < 36 {
        return format!("{h} h");
    }
    format!("{} days", (h + 12) / 24)
}

/// `GET /admin/host`'s sweeps as lines, for one of them (`"store"` or `"folders"`, removing
/// `what`): how often the server sweeps, when next, and what the last did (the editor's
/// `scheduleLine` and `reportLine`).
pub(crate) fn sweep_lines(sweeps: &serde_json::Value, which: &str, what: &str) -> Vec<String> {
    let now = now_ms();
    let every = sweeps["every"].as_str().unwrap_or("10m");
    let schedule = if every == "off" {
        "the server doesn't sweep on its own: only an admin's sweep removes anything".to_string()
    } else if sweeps["sweeping"] == true {
        format!("the server sweeps every {every}; sweeping now")
    } else {
        match sweeps["next_at_ms"].as_u64() {
            Some(next) if next > now => format!("the server sweeps every {every}; the next in {}", span(next - now)),
            _ => format!("the server sweeps every {every}; the next is due now"),
        }
    };
    let r = &sweeps[which];
    let last = match r["at_ms"].as_u64() {
        None => "not swept yet".to_string(),
        Some(at) => {
            let ago = format!("{} ago", span(now.saturating_sub(at)));
            let n = |k: &str| r[k].as_u64().unwrap_or(0);
            if let Some(e) = r["error"].as_str() {
                format!("{ago}, failed: {e}")
            } else if r["skipped"] == true {
                format!("{ago}: nothing new to look at")
            } else if n("removed") == 0 {
                format!("{ago}: nothing to remove ({})", span(n("took_ms")))
            } else {
                format!("{ago}: {} {what}, {} freed in {}", n("removed"), folders::size_text(n("bytes")), span(n("took_ms")))
            }
        }
    };
    vec![schedule, format!("last: {last}")]
}

/// The folders' kept texts' sweep.
fn sweep_folders(state: &AppState, now_ms: u64) -> Report {
    let began = Instant::now();
    let mut report = Report { at_ms: now_ms, ..Report::default() };
    match folders::sweep(&state.folders, &state.deployments, now_ms, folders::SWEEP_SPARES_MS) {
        Ok(s) => {
            report.removed = s.texts as u64;
            report.bytes = s.bytes;
            if s.texts > 0 || s.chunks > 0 {
                let freed = format!("{} file texts, {} freed", s.texts, folders::size_text(s.bytes));
                println!("holon: swept {freed}");
                state.activity.record("swept file texts", &freed, None);
            }
        }
        Err(e) => {
            eprintln!("holon: sweeping file texts: {e}");
            report.error = Some(e);
        }
    }
    report.took_ms = began.elapsed().as_millis() as u64;
    report
}

/// The server's housekeeping: a thread that looks every `LOOK_EVERY` whether a round of sweeps
/// is due, and makes it. `state` is set once the server is built.
pub(crate) fn start(state: std::sync::Arc<std::sync::OnceLock<AppState>>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(LOOK_EVERY);
        if let Some(state) = state.get() {
            state.sweeps.tick(state, now_ms());
        }
    });
}

/// Write `sweep_every` into the policy file's `[storage]`, its other sections, keys and comments
/// kept: the file's new text, checked to be a policy that parses, or why not.
pub(crate) fn with_sweep_every(text: &str, every: &str) -> Result<String, String> {
    parse_every(every).map_err(|e| format!("sweep_every: {e}"))?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("the policy file doesn't parse: {e}"))?;
    if !doc.contains_table("storage") {
        doc["storage"] = toml_edit::table();
    }
    doc["storage"]["sweep_every"] = toml_edit::value(every.trim());
    let written = doc.to_string();
    Policy::parse(&written)?;
    Ok(written)
}

/// `GET /admin/host` and `POST /admin/storage` (an admin's: access.rs holds every `/admin/` path to
/// one), or None for any other path.
pub(crate) fn route(state: &AppState, request: &rouille::Request, by: &str) -> Option<Response> {
    match (request.method(), request.url().as_str()) {
        ("GET", "/admin/host") => Some(Response::json(&serde_json::json!({ "sweeps": state.sweeps.view(&state.policy()) }))),
        ("POST", "/admin/storage") => {
            let body = match crate::http::read_body(request) {
                Ok(b) => b,
                Err(r) => return Some(r),
            };
            let asked: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let Some(every) = asked["sweep_every"].as_str() else {
                return Some(err(400, "say how often the server sweeps: {\"sweep_every\": \"10m\"}, or \"off\""));
            };
            Some(set_sweep_every(state, every, by))
        }
        _ => None,
    }
}

/// Write `sweep_every` to the policy file and read it again, holding every deployment to it as a
/// reload does. The file is left as it was if the new text isn't a policy.
fn set_sweep_every(state: &AppState, every: &str, by: &str) -> Response {
    let text = match std::fs::read_to_string(&state.policy_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return err(500, format!("could not read {:?}: {e}", state.policy_path)),
    };
    let written = match with_sweep_every(&text, every) {
        Ok(w) => w,
        Err(e) => return err(400, e),
    };
    if let Err(e) = holon_host::write_atomically(std::path::Path::new(&state.policy_path), written.as_bytes()) {
        return err(500, format!("could not write {:?}: {e}", state.policy_path));
    }
    let reloaded = crate::grants::handle_post_permissions_reload(state);
    if reloaded.status_code != 200 {
        return reloaded;
    }
    state.activity.record("set how often the server sweeps", every.trim(), Some(by));
    Response::json(&serde_json::json!({ "sweeps": state.sweeps.view(&state.policy()) }))
}
