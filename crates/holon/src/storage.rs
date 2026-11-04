//! How the host keeps what it stores (docs/L4s/storage-L4.md): the server's own sweeps, on the
//! schedule `[storage] sweep_every` sets; what an admin sees of the host (`GET /admin/host`:
//! memory, disks, what it keeps on them, the sweeps) and sets (`POST /admin/storage`, which writes
//! `holon.toml` and reloads it).
//!
//! What the store and the folders' texts hold is counted by the housekeeping thread, at its first
//! look and after each round of sweeps (a store holds millions of files: a view never lists them),
//! so it is "as of" then; the logs, the caches, memory and the disks are read when asked (a few
//! `stat`s and one `statvfs` a disk). The same count removes the temporary files a crash left
//! (a write made aside, then renamed over: `….tmp-<pid>-<n>`, `….cut-<pid>-<n>`), once they are
//! older than any write takes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::permissions::{every_text, parse_every, Policy};
use crate::{err, folders, sweep, AppState, Response};

/// How often the server looks at whether a sweep is due: a changed `sweep_every` applies within it.
pub(crate) const LOOK_EVERY: Duration = Duration::from_secs(10);
/// How old a temporary file must be to be a crash's leftover: a write aside takes seconds; a run,
/// at most 15 minutes.
const LEFTOVER_AFTER: Duration = Duration::from_secs(15 * 60);

/// What a folder holds: its files, their bytes, and the disk they take (whole blocks: a file of
/// 300 bytes takes 4 KiB on most disks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Count {
    pub(crate) files: u64,
    pub(crate) bytes: u64,
    pub(crate) on_disk: u64,
}

impl Count {
    fn add(&mut self, m: &std::fs::Metadata) {
        self.files += 1;
        self.bytes += m.len();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.on_disk += m.blocks() * 512;
        }
        #[cfg(not(unix))]
        {
            self.on_disk += m.len();
        }
    }
}

/// Whether `name` is a temporary file of a write made aside (`sealing::write_private`,
/// `write_aside`, `holon_host::write_atomically`): `<name>.tmp-<pid>-<n>` or `<name>.cut-<pid>-<n>`.
pub(crate) fn is_aside(name: &str) -> bool {
    let Some((_, last)) = name.rsplit_once('.') else { return false };
    let Some(rest) = last.strip_prefix("tmp-").or_else(|| last.strip_prefix("cut-")) else { return false };
    let mut parts = rest.split('-');
    let digits = |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
}

/// Count `dir`'s files that `keep` names (none for a folder that isn't there), removing each one a
/// write made aside left more than `LEFTOVER_AFTER` before `now` (added to `left`).
fn count_dir(dir: &Path, keep: impl Fn(&str) -> bool, now: SystemTime, left: &mut Count) -> Option<Count> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut count = Count::default();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        if is_aside(name) {
            let old = meta.modified().ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age > LEFTOVER_AFTER);
            if old && std::fs::remove_file(entry.path()).is_ok() {
                left.add(&meta);
            }
            continue;
        }
        if keep(name) {
            count.add(&meta);
        }
    }
    Some(count)
}

/// Where the server keeps what it stores, by what it is: the data folder, the deployments' logs and
/// state heads, the content store's values, the folders' kept texts. None where it keeps nothing on
/// disk.
fn places(state: &AppState) -> Vec<(&'static str, PathBuf)> {
    let mut places = Vec::new();
    if state.deployments_path != ":memory:" {
        let deployments = Path::new(&state.deployments_path);
        if let Some(data) = deployments.parent() {
            places.push(("data", if data.as_os_str().is_empty() { PathBuf::from(".") } else { data.to_path_buf() }));
        }
        places.push(("logs", deployments.with_extension("logs")));
        places.push(("state heads", deployments.with_extension("state")));
    }
    if let Some(dir) = state.store.2.dir() {
        places.push(("store", dir.to_path_buf()));
    }
    if let Some(dir) = state.folders.blobs.dir() {
        places.push(("file texts", dir.to_path_buf()));
    }
    places
}

/// What the store and the folders' texts held when last counted, and what crashes left.
#[derive(Default)]
struct Census {
    at_ms: u64,
    /// The store's values, and how many puts there had been when they were counted.
    store: Option<(Count, u64)>,
    file_texts: Option<crate::blobs::Held>,
    /// The temporary files a crash left, removed since the server began.
    leftovers: Count,
    /// What each deployment's names keep in the store, and the file texts its log names, as the
    /// last store sweep (the server's or an admin's, a dry run too) walked them.
    shares: Option<Shares>,
}

/// What the store's last sweep found each deployment holds there (sweep.rs, `Swept::held`).
struct Shares {
    at_ms: u64,
    store: std::collections::BTreeMap<String, (usize, u64)>,
    shared: (usize, u64),
    file_texts: std::collections::BTreeMap<String, (usize, u64)>,
}

/// An entry kept in a log's memory takes about this many times the line it was written as:
/// measured 1.1 to 3.6 times on the soaks' logs (`measure_log_memory`).
const ENTRY_MEMORY: u64 = 3;

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
    census: Mutex<Census>,
}

impl Sweeps {
    /// Look at whether a round is due at `now_ms` under the policy in force, and make it if so:
    /// the store's sweep (paced), then the folders' texts'. True if one was made. Called every
    /// `LOOK_EVERY` by the server's housekeeping thread; the first round is due a `sweep_every`
    /// after the first look.
    pub(crate) fn tick(&self, state: &AppState, now_ms: u64) -> bool {
        let every = state.policy().storage.sweep_every;
        // The first look counts what is kept, so the host's view has it before the first round.
        let (due, first) = {
            let mut kept = self.kept.lock().unwrap();
            let first = kept.since_ms == 0;
            if first {
                kept.since_ms = now_ms;
            }
            let due = !kept.sweeping && every.is_some_and(|e| now_ms >= kept.since_ms.saturating_add(e.as_millis() as u64));
            kept.sweeping |= due;
            (due, first)
        };
        if !due {
            if first {
                self.count(state, now_ms);
            }
            return false;
        }
        let store = self.sweep_store(state, now_ms);
        let folders = sweep_folders(state, now_ms);
        {
            let mut kept = self.kept.lock().unwrap();
            kept.store = store.or(kept.store.take());
            kept.folders = Some(folders);
            kept.since_ms = now_ms.max(self::now_ms());
            kept.sweeping = false;
        }
        self.count(state, now_ms);
        true
    }

    /// Count what the store and the folders' texts hold, removing what crashes left in every place
    /// the server keeps things (`places`).
    pub(crate) fn count(&self, state: &AppState, now_ms: u64) {
        let now = UNIX_EPOCH + Duration::from_millis(now_ms);
        let puts = state.store.2.count();
        let mut left = Count::default();
        let mut store = None;
        for (what, dir) in places(state) {
            match what {
                // A value's file is named by its hash.
                "store" => store = count_dir(&dir, |n| n.len() == 64, now, &mut left),
                "logs" | "state heads" | "data" => {
                    count_dir(&dir, |_| false, now, &mut left);
                }
                _ => {}
            }
        }
        let file_texts = state.folders.blobs.dir().is_some().then(|| state.folders.blobs.held());
        let mut census = self.census.lock().unwrap();
        census.at_ms = now_ms;
        census.store = store.map(|s| (s, puts));
        census.file_texts = file_texts;
        census.leftovers.files += left.files;
        census.leftovers.bytes += left.bytes;
        census.leftovers.on_disk += left.on_disk;
        if left.files > 0 {
            let freed = format!("{} files, {} freed", left.files, folders::size_text(left.on_disk));
            println!("holon: removed the temporary files a crash left: {freed}");
            state.activity.record("removed what a crash left", &freed, None);
        }
    }

    /// The store's sweep, paced; none for a store in memory.
    fn sweep_store(&self, state: &AppState, now_ms: u64) -> Option<Report> {
        let began = Instant::now();
        let mut report = Report { at_ms: now_ms, ..Report::default() };
        match self.scheduled.lock().unwrap().sweep(&state.store, &state.deployments, now_ms) {
            Ok(Some(s)) => {
                self.note(&s, now_ms);
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

    /// Note what a store sweep found each deployment holds (a dry run's too: it walks the same).
    pub(crate) fn note(&self, swept: &sweep::Swept, at_ms: u64) {
        self.census.lock().unwrap().shares = Some(Shares { at_ms, store: swept.held.clone(), shared: swept.shared, file_texts: swept.file_texts.clone() });
    }

    /// What `slug` holds, in memory and on disk, or none if there is no such deployment.
    fn holds(&self, state: &AppState, slug: &str) -> Option<serde_json::Value> {
        let (log, unit, cell) = {
            let table = state.deployments.lock().unwrap();
            let dep = table.get(slug)?;
            (std::sync::Arc::clone(&dep.log), dep.unit, dep.state.clone())
        };
        let (kept, entries, entry_bytes, lines) = log.with(|l| l.holds());
        let mut file = Count::default();
        if let Some(path) = crate::recorder::file_for(&state.deployments_path, slug)
            && let Ok(m) = std::fs::metadata(path)
        {
            file.add(&m);
        }
        let state_memory: u64 = cell.map_or(0, |c| c.state.lock().unwrap().iter().map(|(k, v)| k.len() as u64 + crate::units::footprint(v)).sum());
        let flow = state.store.3.held_for(&unit);
        let census = self.census.lock().unwrap();
        let shares = census.shares.as_ref();
        let store = shares.map(|s| s.store.get(slug).copied().unwrap_or_default());
        let texts = shares.map(|s| s.file_texts.get(slug).copied().unwrap_or_default());
        let log_memory = entry_bytes * ENTRY_MEMORY;
        let memory = log_memory + state_memory + flow.map_or(0, |f| f.0);
        let disk = file.on_disk + store.map_or(0, |s| s.1) + texts.map_or(0, |t| t.1);
        Some(serde_json::json!({
            "slug": slug,
            "memory": memory,
            "disk": disk,
            "log": { "kept": kept, "entries": entries, "entry_bytes": entry_bytes, "memory": log_memory, "lines": lines, "file": file },
            "state_memory": state_memory,
            "flow_cache": flow.map(|(own, shared)| serde_json::json!({ "own": own, "shared": shared })),
            "store": store.map(|(values, bytes)| serde_json::json!({ "values": values, "bytes": bytes })),
            "file_texts": texts.map(|(texts, bytes)| serde_json::json!({ "texts": texts, "bytes": bytes })),
            "shares_at_ms": shares.map(|s| s.at_ms),
        }))
    }

    /// Every deployment's `holds`, the most first; and what the store keeps that more than one names.
    fn all_holds(&self, state: &AppState) -> serde_json::Value {
        let slugs: Vec<String> = state.deployments.lock().unwrap().keys().cloned().collect();
        let mut each: Vec<serde_json::Value> = slugs.iter().filter_map(|s| self.holds(state, s)).collect();
        let total = |v: &serde_json::Value| v["memory"].as_u64().unwrap_or(0) + v["disk"].as_u64().unwrap_or(0);
        each.sort_by_key(|v| std::cmp::Reverse(total(v)));
        let census = self.census.lock().unwrap();
        let shared = census.shares.as_ref().map(|s| serde_json::json!({ "values": s.shared.0, "bytes": s.shared.1 }));
        serde_json::json!({ "each": each, "shared_in_store": shared })
    }

    /// `GET /admin/host`'s part about what the store and the folders' texts hold.
    fn kept_view(&self, state: &AppState) -> serde_json::Value {
        let census = self.census.lock().unwrap();
        let puts = state.store.2.count();
        let at = (census.at_ms > 0).then_some(census.at_ms);
        let mut logs = None;
        let mut left = Count::default();
        if state.deployments_path != ":memory:" {
            let dir = Path::new(&state.deployments_path).with_extension("logs");
            // Counted now: one file a deployment. What crashes left is removed by the census only.
            logs = std::fs::read_dir(&dir).ok().map(|entries| {
                let mut c = Count::default();
                for e in entries.flatten() {
                    let name = e.file_name();
                    match (name.to_str(), e.metadata()) {
                        (Some(n), Ok(m)) if m.is_file() && n.ends_with(".jsonl") => c.add(&m),
                        (Some(n), Ok(m)) if m.is_file() && is_aside(n) => left.add(&m),
                        _ => {}
                    }
                }
                c
            });
        }
        serde_json::json!({
            "at_ms": at,
            "store": census.store.map(|(c, at_puts)| serde_json::json!({ "values": c.files, "bytes": c.bytes, "on_disk": c.on_disk, "put_since": puts.saturating_sub(at_puts) })),
            "logs": logs,
            "file_texts": census.file_texts.map(|h| serde_json::json!({ "texts": h.texts, "chunks": h.chunks, "bytes": h.bytes })),
            "leftovers": { "removed": census.leftovers, "waiting": left },
        })
    }

    /// `GET /admin/host`'s part about sweeping.
    fn view(&self, policy: &Policy) -> serde_json::Value {
        let every = policy.storage.sweep_every;
        let kept = self.kept.lock().unwrap();
        // Before the first look, the first round is due a `sweep_every` from now.
        let since = if kept.since_ms == 0 { now_ms() } else { kept.since_ms };
        serde_json::json!({
            "every": every_text(every),
            "next_at_ms": every.map(|e| since.saturating_add(e.as_millis() as u64)),
            "sweeping": kept.sweeping,
            "store": kept.store,
            "folders": kept.folders,
        })
    }
}

/// The process's memory, from `/proc/self/status` (Linux): resident and its peak, in bytes.
fn memory() -> (Option<u64>, Option<u64>) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else { return (None, None) };
    let field = |name: &str| {
        status.lines().find_map(|l| l.strip_prefix(name)).and_then(|v| v.trim().strip_suffix("kB")).and_then(|v| v.trim().parse::<u64>().ok()).map(|kb| kb * 1024)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// Each disk the server keeps things on, once, with what it keeps there: its size, what is free
/// to the server (what a quota or the space kept for root leaves it: what it can still write),
/// and what is in use. How full it is for the server is what isn't free to it.
#[cfg(unix)]
fn disks(state: &AppState) -> Vec<serde_json::Value> {
    use std::os::unix::fs::MetadataExt;
    let mut seen: Vec<(u64, PathBuf, Vec<&'static str>)> = Vec::new();
    for (what, dir) in places(state) {
        // A folder not made yet is on its parent's disk.
        let Some(at) = dir.ancestors().find(|p| p.exists()).map(Path::to_path_buf) else { continue };
        let Ok(meta) = std::fs::metadata(&at) else { continue };
        match seen.iter_mut().find(|(dev, _, _)| *dev == meta.dev()) {
            Some((_, _, holds)) => holds.push(what),
            None => seen.push((meta.dev(), at, vec![what])),
        }
    }
    seen.into_iter()
        .filter_map(|(_, at, holds)| {
            let path = std::ffi::CString::new(at.as_os_str().as_encoded_bytes()).ok()?;
            let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
            // SAFETY: `path` is a NUL-terminated string and `vfs` is a statvfs the call fills.
            if unsafe { libc::statvfs(path.as_ptr(), &mut vfs) } != 0 {
                return None;
            }
            let unit = vfs.f_frsize as u64;
            let size = vfs.f_blocks as u64 * unit;
            let free = vfs.f_bavail as u64 * unit;
            let used = size.saturating_sub(vfs.f_bfree as u64 * unit);
            Some(serde_json::json!({ "path": at.display().to_string(), "holds": holds, "size": size, "free": free, "used": used }))
        })
        .collect()
}

#[cfg(not(unix))]
fn disks(_state: &AppState) -> Vec<serde_json::Value> {
    Vec::new()
}

/// `GET /admin/host`: memory, disks, what the server keeps on them, and its sweeps.
pub(crate) fn host_view(state: &AppState) -> serde_json::Value {
    let (resident, peak) = memory();
    let (closures, flow_bytes, flow_budget) = state.store.3.usage();
    let (values, value_bytes, value_budget) = state.store.4.usage();
    serde_json::json!({
        "memory": {
            "resident": resident,
            "peak": peak,
            "flow_cache": { "entries": closures, "bytes": flow_bytes, "budget": flow_budget },
            "value_cache": { "entries": values, "bytes": value_bytes, "budget": value_budget },
        },
        "disks": disks(state),
        "kept": state.sweeps.kept_view(state),
        "deployments": state.sweeps.all_holds(state),
        "sweeps": state.sweeps.view(&state.policy()),
    })
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

/// `GET /admin/host` as lines (`holon host`, the terminal editor's admin view).
pub(crate) fn host_lines(v: &serde_json::Value) -> Vec<String> {
    let size = |x: &serde_json::Value| x.as_u64().map_or("?".to_string(), folders::size_text);
    let n = |x: &serde_json::Value| x.as_u64().unwrap_or(0);
    let mut lines = Vec::new();
    let m = &v["memory"];
    lines.push(match m["resident"].as_u64() {
        Some(_) => format!("memory       {} (at most {} so far)", size(&m["resident"]), size(&m["peak"])),
        None => "memory       not told on this system".to_string(),
    });
    let cache = |c: &serde_json::Value, what: &str| {
        let budget = n(&c["budget"]);
        let full = (n(&c["bytes"]) * 100).checked_div(budget).unwrap_or(0);
        format!("{} {what}, {} of {} ({full}%)", n(&c["entries"]), size(&c["bytes"]), size(&c["budget"]))
    };
    lines.push(format!("  flow cache   {}", cache(&m["flow_cache"], "flows")));
    lines.push(format!("  value cache  {}", cache(&m["value_cache"], "values")));
    for d in v["disks"].as_array().into_iter().flatten() {
        let holds: Vec<&str> = d["holds"].as_array().into_iter().flatten().filter_map(|h| h.as_str()).collect();
        lines.push(format!("disk         {}% full: {} free of {} ({} in use): {} ({})", full(d), size(&d["free"]), size(&d["size"]), size(&d["used"]), holds.join(", "), s(&d["path"])));
    }
    let k = &v["kept"];
    let counted = k["at_ms"].as_u64().map_or("not counted yet".to_string(), |at| format!("counted {} ago", span(now_ms().saturating_sub(at))));
    lines.push(format!("kept         {counted}"));
    if k["store"].is_object() {
        let st = &k["store"];
        lines.push(format!("  store        {} values, {} on disk ({} of bytes); {} put since", n(&st["values"]), size(&st["on_disk"]), size(&st["bytes"]), n(&st["put_since"])));
    }
    if k["logs"].is_object() {
        lines.push(format!("  logs         {} logs, {} on disk", n(&k["logs"]["files"]), size(&k["logs"]["on_disk"])));
    }
    if k["file_texts"].is_object() {
        lines.push(format!("  file texts   {} texts, {}", n(&k["file_texts"]["texts"]), size(&k["file_texts"]["bytes"])));
    }
    let (removed, waiting) = (&k["leftovers"]["removed"], &k["leftovers"]["waiting"]);
    if n(&removed["files"]) > 0 || n(&waiting["files"]) > 0 {
        lines.push(format!("  left by crashes  {} files removed ({}), {} still to remove", n(&removed["files"]), size(&removed["on_disk"]), n(&waiting["files"])));
    }
    let flows = v["deployments"]["each"].as_array().cloned().unwrap_or_default();
    if !flows.is_empty() {
        let mut rows = vec![["flow", "memory", "disk", "log in memory", "log on disk", "in the store"].map(String::from).to_vec()];
        for f in &flows {
            let store = if f["store"].is_object() { format!("{} values", n(&f["store"]["values"])) } else { "not counted yet".into() };
            rows.push(vec![s(&f["slug"]).to_string(), format!("~{}", size(&f["memory"])), size(&f["disk"]), format!("~{} ({} of {} runs)", size(&f["log"]["memory"]), n(&f["log"]["entries"]), n(&f["log"]["kept"])), size(&f["log"]["file"]["on_disk"]), store]);
        }
        lines.push("flows        what each holds, the most first (memory about: a run kept in memory takes about 3 times its line)".to_string());
        lines.extend(crate::cmdline::table(&rows).into_iter().map(|l| format!("  {l}")));
    }
    let sw = &v["sweeps"];
    let mut store = sweep_lines(sw, "store", "values").into_iter();
    lines.push(format!("sweeps       {}", store.next().unwrap_or_default().trim_start_matches("the server sweeps ")));
    lines.push(format!("  store        {}", store.next().unwrap_or_default()));
    lines.push(format!("  file texts   {}", sweep_lines(sw, "folders", "texts").pop().unwrap_or_default()));
    lines
}

/// How full a disk is for the server, 0 to 100: what isn't free to it.
pub(crate) fn full(disk: &serde_json::Value) -> u64 {
    let (size, free) = (disk["size"].as_u64().unwrap_or(0), disk["free"].as_u64().unwrap_or(0));
    if size == 0 { 0 } else { (size.saturating_sub(free) * 100).div_ceil(size) }
}

/// `GET /deployments/<slug>/holds` as lines (`holon host --for SLUG`, each flow's page in the
/// terminal editor).
pub(crate) fn holds_lines(v: &serde_json::Value) -> Vec<String> {
    let size = |x: &serde_json::Value| x.as_u64().map_or("?".to_string(), folders::size_text);
    let n = |x: &serde_json::Value| x.as_u64().unwrap_or(0);
    let log = &v["log"];
    let mut lines = vec![
        format!("in memory    about {}", size(&v["memory"])),
        format!("  its log      about {}: the newest {} of the {} runs it keeps, {} as written", size(&log["memory"]), n(&log["entries"]), n(&log["kept"]), size(&log["entry_bytes"])),
        format!("  its state    {}", size(&v["state_memory"])),
    ];
    lines.push(match v["flow_cache"].as_object() {
        Some(f) => format!("  its flow     {} cached ({} more shared with other flows)", size(&f["own"]), size(&f["shared"])),
        None => "  its flow     not in the cache now".to_string(),
    });
    lines.push(format!("on disk      {}", size(&v["disk"])));
    lines.push(format!("  its log      {}: {} lines", size(&log["file"]["on_disk"]), n(&log["lines"])));
    let at = v["shares_at_ms"].as_u64().map(|at| format!(" (as the store's last sweep found, {} ago)", span(now_ms().saturating_sub(at))));
    match (v["store"].as_object(), &at) {
        (Some(st), Some(at)) => lines.push(format!("  its states   {} values, {}{at}", n(&st["values"]), size(&st["bytes"]))),
        _ => lines.push("  its states   not counted yet: the store's first sweep counts them".to_string()),
    }
    if let Some(t) = v["file_texts"].as_object().filter(|t| n(&t["texts"]) > 0) {
        lines.push(format!("  file texts   {} its runs read, {}", n(&t["texts"]), size(&t["bytes"])));
    }
    lines
}

fn s(v: &serde_json::Value) -> &str {
    v.as_str().unwrap_or("")
}

const HOST_USAGE: &str = "usage: holon host [--for SLUG] [--json] [--server PORT|URL]

  What the host holds: the server's memory and its caches against their sizes; each disk it keeps
  things on, how full; what it keeps there (the content store's values, counted at its last
  sweep, the logs, the folders' kept texts, and what a crash left, removed once it is old); and
  how often it sweeps, when next, and what the last sweeps removed; and what each flow holds, the
  most first. An admin's. --for SLUG: what one flow holds, in memory and on disk (anyone who may
  read the deployment).
";

/// `holon host`.
pub(crate) fn cli(args: &[String]) -> i32 {
    const WHO: &str = "holon host";
    let usage = format!("{HOST_USAGE}\n{}", crate::cmdline::SIGN_IN);
    let args = match crate::cmdline::parse(WHO, &usage, args, &["--for"], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (path, lines): (String, fn(&serde_json::Value) -> Vec<String>) = match args.value("--for") {
        Some(slug) => (format!("/deployments/{}/holds", crate::cmdline::seg(slug)), holds_lines),
        None => ("/admin/host".into(), host_lines),
    };
    match crate::cmdline::json(WHO, &args, "GET", &path, None) {
        Ok(v) if args.flag("--json") => {
            crate::cmdline::print_json(&v);
            0
        }
        Ok(v) => {
            lines(&v).iter().for_each(|l| println!("{l}"));
            0
        }
        Err(code) => code,
    }
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

/// `GET /deployments/<slug>/holds`: what one deployment holds, in memory and on disk; as readable
/// as the deployment is.
pub(crate) fn handle_get_holds(state: &AppState, slug: &str) -> Response {
    match state.sweeps.holds(state, slug) {
        Some(v) => Response::json(&v),
        None => err(404, format!("no deployment {slug:?}")),
    }
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
        ("GET", "/admin/host") => Some(Response::json(&host_view(state))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_made_aside_is_known_by_its_name_and_nothing_else_is() {
        for aside in ["notes-api.jsonl.tmp-21605-17", "deployments.json.tmp-1-0", "x.jsonl.cut-4242-3"] {
            assert!(is_aside(aside), "{aside}");
        }
        for not in ["notes-api.jsonl", "a.tmp", "x.tmp-12", "x.tmp-12-", "x.tmp-a-1", "x.tmp-1-2-3", "tmp-1-2", "05dcd5a7"] {
            assert!(!is_aside(not), "{not}");
        }
    }

    /// What a crash left aside is removed once it is older than any write takes; a write aside
    /// now, and every other file, are left as they are and counted as asked.
    #[test]
    fn what_a_crash_left_is_removed_once_it_is_old_and_nothing_else_is() {
        let dir = std::env::temp_dir().join(format!("holon-leftovers-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in [("a.jsonl", 10), ("b.jsonl", 20), ("a.jsonl.tmp-99-1", 1000), ("a.jsonl.cut-99-2", 500), ("notes", 5)] {
            std::fs::write(dir.join(name), vec![b'x'; bytes]).unwrap();
        }
        let mut left = Count::default();
        // Now: every file is new; nothing is a leftover yet.
        let count = count_dir(&dir, |n| n.ends_with(".jsonl"), SystemTime::now(), &mut left).unwrap();
        assert_eq!((count.files, count.bytes, left.files), (2, 30, 0));
        assert!(dir.join("a.jsonl.tmp-99-1").exists());
        // Twenty minutes on: the two written aside are a crash's, and go.
        let later = SystemTime::now() + Duration::from_secs(20 * 60);
        let count = count_dir(&dir, |n| n.ends_with(".jsonl"), later, &mut left).unwrap();
        assert_eq!((count.files, count.bytes), (2, 30));
        assert_eq!((left.files, left.bytes), (2, 1500));
        assert!(left.on_disk >= left.bytes || cfg!(not(unix)));
        let names: std::collections::BTreeSet<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["a.jsonl", "b.jsonl", "notes"].map(String::from).into_iter().collect());
        assert!(count_dir(&dir.join("none"), |_| true, later, &mut left).is_none(), "a folder not there counts nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_a_flow_holds_reads_as_lines() {
        let v = serde_json::json!({
            "slug": "notes-api", "memory": 6 << 20, "disk": 60 << 20,
            "log": { "kept": 500, "entries": 32, "entry_bytes": 2 << 20, "memory": 6 << 20, "lines": 1130, "file": { "files": 1, "bytes": 56 << 20, "on_disk": 56 << 20 } },
            "state_memory": 2048, "flow_cache": { "own": 195 << 10, "shared": 0 },
            "store": null, "file_texts": null, "shares_at_ms": null,
        });
        assert_eq!(
            holds_lines(&v),
            [
                "in memory    about 6 MiB",
                "  its log      about 6 MiB: the newest 32 of the 500 runs it keeps, 2 MiB as written",
                "  its state    2 KiB",
                "  its flow     195 KiB cached (0 bytes more shared with other flows)",
                "on disk      60 MiB",
                "  its log      56 MiB: 1130 lines",
                "  its states   not counted yet: the store's first sweep counts them",
            ]
        );
    }

    #[test]
    fn the_host_reads_as_lines() {
        let now = now_ms();
        let v = serde_json::json!({
            "memory": { "resident": 742 << 20, "peak": 811 << 20, "flow_cache": { "entries": 12, "bytes": 32 << 20, "budget": 256 << 20 }, "value_cache": { "entries": 1234, "bytes": 48 << 20, "budget": 64 << 20 } },
            "disks": [{ "path": "/data", "holds": ["data", "logs", "store"], "size": 100u64 << 30, "free": 40u64 << 30, "used": 60u64 << 30 }],
            "kept": { "at_ms": now - 180_000, "store": { "values": 432808, "bytes": 1u64 << 30, "on_disk": 2u64 << 30, "put_since": 3200 }, "logs": { "files": 34, "bytes": 192 << 20, "on_disk": 192 << 20 }, "file_texts": null, "leftovers": { "removed": { "files": 3, "bytes": 62 << 20, "on_disk": 62 << 20 }, "waiting": { "files": 0, "bytes": 0, "on_disk": 0 } } },
            "sweeps": { "every": "10m", "next_at_ms": now + 420_000, "sweeping": false, "store": null, "folders": null },
        });
        assert_eq!(
            host_lines(&v),
            [
                "memory       742 MiB (at most 811 MiB so far)",
                "  flow cache   12 flows, 32 MiB of 256 MiB (12%)",
                "  value cache  1234 values, 48 MiB of 64 MiB (75%)",
                "disk         60% full: 40 GiB free of 100 GiB (60 GiB in use): data, logs, store (/data)",
                "kept         counted 3 min ago",
                "  store        432808 values, 2 GiB on disk (1 GiB of bytes); 3200 put since",
                "  logs         34 logs, 192 MiB on disk",
                "  left by crashes  3 files removed (62 MiB), 0 still to remove",
                "sweeps       every 10m; the next in 7 min",
                "  store        last: not swept yet",
                "  file texts   last: not swept yet",
            ]
        );
    }
}
