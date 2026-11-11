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
use crate::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::permissions::{every_text, Policy};
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
        self.add_bytes(m);
    }

    /// Its bytes and the disk it takes, as one file more of something counted already (a log's
    /// sealed segment, beside its file).
    fn add_bytes(&mut self, m: &std::fs::Metadata) {
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

/// What every file the server writes to the backup is named starting with: the server's own are
/// known by it at a glance (to the server, and to a person looking at the drive), and the server
/// only ever exports or removes a file that has it. The backup may be a drive or a synced folder
/// that holds other things too.
pub(crate) const ARCHIVE_PREFIX: &str = "holon-archive-";

/// The name of a batch of runs a log lets go of, `<first seq>` to `<last seq>` (recorder.rs
/// `keep_aside`): `holon-archive-<first>-<last>.jsonl`, each seq 20 digits.
pub(crate) fn batch_name(first: u64, last: u64) -> String {
    format!("{ARCHIVE_PREFIX}{first:020}-{last:020}.jsonl")
}

/// The seqs a batch's name holds, `<first>-<last>` (each 20 digits), if it is one.
fn batch_seqs(name: &str) -> Option<&str> {
    let seqs = name.strip_prefix(ARCHIVE_PREFIX)?.strip_suffix(".jsonl")?;
    let (first, last) = seqs.split_once('-')?;
    let seq = |p: &str| p.len() == 20 && p.bytes().all(|b| b.is_ascii_digit());
    (seq(first) && seq(last)).then_some(seqs)
}

/// Whether `name` is a batch of runs a log wrote to the backup (`batch_name`): nothing else in
/// the backup is the server's to export or remove.
pub(crate) fn is_batch(name: &str) -> bool {
    batch_seqs(name).is_some()
}

/// The name of the export the housekeeping makes of the batch `batch` of `slug`'s runs:
/// `holon-archive-<slug>-runs-<first>-<last>.json.gz`.
fn export_name(slug: &str, batch: &str) -> Option<String> {
    batch_seqs(batch).map(|seqs| format!("{ARCHIVE_PREFIX}{slug}-runs-{seqs}.json.gz"))
}

/// Whether `name` is an export the housekeeping made of a batch (`export_name`).
fn is_export(name: &str) -> bool {
    name.strip_prefix(ARCHIVE_PREFIX)
        .and_then(|n| n.strip_suffix(".json.gz"))
        .and_then(|n| n.rsplit_once("-runs-"))
        .is_some_and(|(slug, seqs)| !slug.is_empty() && is_batch(&format!("{ARCHIVE_PREFIX}{seqs}.jsonl")))
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
    if let Some(deployments) = state.deployments_path.at() {
        if let Some(data) = deployments.parent() {
            places.push(("data", data.into()));
        }
        places.push(("logs", deployments.with_extension("logs").into()));
        places.push(("state heads", deployments.with_extension("state").into()));
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
    /// The store's: the named values older than it spares it moved into packs.
    pub(crate) packed: u64,
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
    watch: Mutex<Watch>,
    /// Why the last export of a backup batch couldn't be made, if it couldn't (`archive`).
    backup_error: Mutex<Option<String>>,
}

/// What the housekeeping watches between its looks, for the warnings (`warnings`).
#[derive(Default)]
struct Watch {
    /// At each look in the last hour, whether the flow cache and the value cache let go of
    /// anything to fit their sizes since the look before, newest last; and how many each had let
    /// go of then.
    caches: std::collections::VecDeque<(bool, bool)>,
    evicted: (u64, u64),
    /// Each flow's disk at the last count, and how much it grew since the one before.
    disk: std::collections::BTreeMap<String, u64>,
    grew: std::collections::BTreeMap<String, u64>,
    /// The warnings said last look, by key: a new one is said once in the server's log.
    said: std::collections::BTreeSet<String>,
    /// The deployments whose logs `roll` cut, to keep as granted again once there is room.
    rolled: std::collections::BTreeSet<String>,
}

/// The fewest runs `roll` leaves a log keeping.
pub(crate) const ROLL_FLOOR: usize = 20;

/// Whether `dep` records any of its runs: a clock's, a file handler's, or a route's or an action's
/// its grant doesn't keep out of its log. One that records none goes on when a disk is full.
fn records_runs(dep: &crate::Deployment) -> bool {
    let Some(server) = &dep.server else { return true };
    let recorded = |label: String, level| dep.permit.log_level(Some(&label), level) != holon_core::LogLevel::Unrecorded;
    !server.clocks.is_empty()
        || !server.files.is_empty()
        || server.routes.iter().any(|r| recorded(format!("{} {}", r.method, r.path), r.log))
        || server.websockets.iter().flat_map(|w| w.blocks()).any(|b| recorded(b.label.clone(), b.log))
        || server.actions.iter().any(|a| recorded(a.label.clone(), a.log))
}

/// How many looks an hour is (`LOOK_EVERY`).
const LOOKS_AN_HOUR: usize = 360;

/// Something about the host an admin should know, and what to do about it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Warning {
    /// What it is about, the same from one look to the next: `disk:<path>`, `cache:flows`.
    pub(crate) key: String,
    pub(crate) says: String,
    pub(crate) todo: String,
}

/// What the host should warn of now: each disk past `[storage] warn_at`, with the flow that grew
/// most since the store was last counted; a cache at its size for most of the last hour; sweeping
/// off with the store past a tenth of its disk.
pub(crate) fn warnings(state: &AppState) -> Vec<Warning> {
    let policy = state.policy();
    let at = policy.storage.warn_at;
    let size = |b: u64| folders::size_text(b);
    let mut out = Vec::new();
    let watch = state.sweeps.watch.lock().unwrap();
    let fastest = watch.grew.iter().max_by_key(|(_, g)| **g).filter(|(_, g)| **g > 0).map(|(slug, g)| (slug.clone(), *g));
    let full = watch.caches.len() >= LOOKS_AN_HOUR / 12;
    let (flows_full, values_full) = watch.caches.iter().fold((0, 0), |(f, v), (a, b)| (f + *a as usize, v + *b as usize));
    let looks = watch.caches.len();
    drop(watch);
    let disks = disks(state);
    for d in &disks {
        let (total, free) = (d["size"].as_u64().unwrap_or(0), d["free"].as_u64().unwrap_or(0));
        // Past `full_at` is warned of too: one written in another unit than `warn_at` (85%, and
        // 500G free) may be reached first.
        if !at.past(total, free) && !policy.storage.full_at.past(total, free) {
            continue;
        }
        let holds: Vec<&str> = d["holds"].as_array().into_iter().flatten().filter_map(|h| h.as_str()).collect();
        let grew = fastest.as_ref().map(|(slug, g)| format!("; the flow that grew most since the store was last counted: {slug}, by {}", size(*g))).unwrap_or_default();
        out.push(Warning {
            key: format!("disk:{}", d["path"].as_str().unwrap_or("")),
            says: format!(
                "the disk with {} is {}% full for the server: {} free of {} (it warns at {}){grew}",
                holds.join(", "),
                self::full(d),
                size(free),
                size(total),
                crate::permissions::threshold_text(at)
            ),
            todo: match (policy.storage.full_at.past(total, free), policy.storage.when_full) {
                (true, crate::permissions::WhenFull::Pause) => format!("past {}: the flows that record runs are paused until it is back under {}; make room: lower the keep of the flows holding most (holon host shows them), sweep now (holon store sweep), or give the server more disk", crate::permissions::threshold_text(policy.storage.full_at), crate::permissions::threshold_text(at)),
                (true, crate::permissions::WhenFull::Roll) => format!("past {}: the oldest runs of the logs keeping the most are being cut, and swept; make room for good: lower the keep of the flows holding most, or give the server more disk", crate::permissions::threshold_text(policy.storage.full_at)),
                (true, crate::permissions::WhenFull::Warn) => format!("past {}, and the server only warns (when_full = \"warn\"): once it fills, a strict run fails its sync and answers 500; make room now, or have it pause or roll (holon policy --when-full)", crate::permissions::threshold_text(policy.storage.full_at)),
                _ => "make room: lower the keep of the flows holding most (holon host shows them), sweep now (holon store sweep), or give the server more disk".into(),
            },
        });
    }
    let (flows, values) = (state.store.3.usage(), state.store.4.usage());
    if full && flows_full * 2 > looks {
        out.push(Warning {
            key: "cache:flows".into(),
            says: format!("the flow cache has been at its size ({}) most of the last hour: it lets go of flows their runs load again from disk", size(flows.2)),
            todo: "raise it: holon serve --flow-cache".into(),
        });
    }
    if full && values_full * 2 > looks {
        out.push(Warning {
            key: "cache:values".into(),
            says: format!("the value cache has been at its size ({}) most of the last hour: it lets go of values runs read again from disk", size(values.2)),
            todo: "raise it: holon serve --value-cache (or HOLON_VALUE_CACHE)".into(),
        });
    }
    if let Some(e) = log_backup_error(state) {
        out.push(Warning {
            key: "backup".into(),
            says: format!("the backup can't be written to: {e}"),
            todo: "the runs it would hold are kept in the logs until it can: check the drive or folder is there and writable, or change [storage] backup (holon policy --backup)".into(),
        });
    }
    if let Some(e) = state.sweeps.backup_error.lock().unwrap().clone() {
        out.push(Warning {
            key: "backup-export".into(),
            says: format!("a batch of runs in the backup can't be made an export: {e}"),
            todo: "its runs are in the batch still, as the log wrote them, and the other batches are made exports all the same; check the drive or folder is writable and the batch is whole, it is tried again every ten seconds".into(),
        });
    }
    for (pack, e) in state.store.2.packs().map(|p| p.unreadable().to_vec()).unwrap_or_default() {
        out.push(Warning {
            key: format!("pack:{}", pack.display()),
            says: format!("a pack of the store's values can't be read, and its values are missing: {} ({e})", pack.display()),
            todo: "restore the file from a copy of the data folder and restart the server; the store's other values read as they were".into(),
        });
    }
    if policy.storage.sweep_every.is_none() {
        let store = state.sweeps.census.lock().unwrap().store.map(|(c, _)| c.on_disk).unwrap_or(0);
        let disk = disks.iter().find(|d| d["holds"].as_array().is_some_and(|h| h.iter().any(|x| x == "store"))).and_then(|d| d["size"].as_u64()).unwrap_or(0);
        if disk > 0 && store * 10 > disk {
            out.push(Warning {
                key: "sweep-off".into(),
                says: format!("the server doesn't sweep on its own, and its store takes {} of its disk's {}", size(store), size(disk)),
                todo: "sweep now (holon store sweep), or turn sweeping on: holon policy --sweep-every 10m".into(),
            });
        }
    }
    out
}

impl Sweeps {
    /// Look at whether a round is due at `now_ms` under the policy in force, and make it if so:
    /// the store's sweep (paced), then the folders' texts'. True if one was made. Called every
    /// `LOOK_EVERY` by the server's housekeeping thread; the first round is due a `sweep_every`
    /// after the first look.
    pub(crate) fn tick(&self, state: &AppState, now_ms: u64) -> bool {
        self.watch(state);
        self.act(state, now_ms);
        archive(state);
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

    /// Note how full the caches are, and say each warning once, as it starts and as it ends: in
    /// the server's log and its activity.
    fn watch(&self, state: &AppState) {
        {
            // A cache at its size lets go of what runs read, to make room: since the last look?
            let evicted = (state.store.3.evicted(), state.store.4.evicted());
            let mut watch = self.watch.lock().unwrap();
            let (flows, values) = (evicted.0 > watch.evicted.0, evicted.1 > watch.evicted.1);
            watch.evicted = evicted;
            watch.caches.push_back((flows, values));
            while watch.caches.len() > LOOKS_AN_HOUR {
                watch.caches.pop_front();
            }
        }
        let now = warnings(state);
        let keys: std::collections::BTreeSet<String> = now.iter().map(|w| w.key.clone()).collect();
        let mut watch = self.watch.lock().unwrap();
        for w in now.iter().filter(|w| !watch.said.contains(&w.key)) {
            eprintln!("holon: warning: {}; {}", w.says, w.todo);
            state.activity.record("warned", &w.says, None);
        }
        for key in watch.said.difference(&keys) {
            println!("holon: no longer warning of {key}");
        }
        watch.said = keys;
    }

    /// When a disk is past `[storage] full_at`, do what `when_full` says: pause every deployment
    /// that records runs, or cut the oldest runs of the biggest log on disk to half (to
    /// `ROLL_FLOOR` at the least), one a look, sweeping at the first; once every disk is back under
/// `warn_at` and `full_at`, resume what it paused
    /// and keep what it cut as granted again. Each on its deployment's timeline.
    pub(crate) fn act(&self, state: &AppState, now_ms: u64) {
        let policy = state.policy();
        let disks = disks(state);
        let past = |t: crate::permissions::Threshold| {
            disks.iter().find(|d| t.past(d["size"].as_u64().unwrap_or(0), d["free"].as_u64().unwrap_or(0))).map(full)
        };
        let (full_pct, warned) = (past(policy.storage.full_at), past(policy.storage.warn_at).is_some());
        // Each log writes what it lets go of to the backup, as the policy says now; a full disk
        // rolling may let them go without, if it can't (the admin chose room over history).
        let may_drop = policy.storage.when_full == crate::permissions::WhenFull::Roll && full_pct.is_some();
        let backup = policy.storage.backup.clone().map(|dir| (dir.into(), may_drop));
        let logs: Vec<std::sync::Arc<crate::recorder::LogCell>> = state.deployments.lock().unwrap().values().map(|d| std::sync::Arc::clone(&d.log)).collect();
        for log in logs {
            log.with(|l| l.set_backup(backup.clone()));
        }
        let why = |pct: u64| format!("the disk was full ({pct}% for the server, past {})", crate::permissions::threshold_text(policy.storage.full_at));
        let mut changed = false;
        match (policy.storage.when_full, full_pct) {
            (crate::permissions::WhenFull::Pause, Some(pct)) => {
                let mut table = state.deployments.lock().unwrap();
                for (slug, dep) in table.iter_mut() {
                    if dep.life.status == crate::lifecycle::Status::Running && records_runs(dep) {
                        let _ = dep.life.pause();
                        dep.channels.close();
                        dep.sockets.close_all(crate::websockets::PAUSED);
                        dep.grant.paused_full = true;
                        eprintln!("holon: paused {slug:?}: {}", why(pct));
                        dep.note("lifecycle", format!("paused by the host: {}", why(pct)), None, Default::default());
                        changed = true;
                    }
                }
            }
            (crate::permissions::WhenFull::Roll, Some(pct)) => {
                let logs: Vec<(String, std::sync::Arc<crate::recorder::LogCell>)> = state.deployments.lock().unwrap().iter().map(|(s, d)| (s.clone(), std::sync::Arc::clone(&d.log))).collect();
                // The biggest on disk, its file and its sealed segments, of those keeping more
                // than the floor: what frees the most.
                let file = |slug: &str| crate::recorder::file_for(&state.deployments_path, slug).and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
                let most = logs
                    .iter()
                    .map(|(slug, log)| {
                        let (kept, sealed) = log.with(|l| (l.holds().0, l.sealed_holds().2));
                        (slug, log, kept, file(slug) + sealed)
                    })
                    .filter(|(_, _, kept, _)| *kept > ROLL_FLOOR)
                    .max_by_key(|(_, _, kept, disk)| (*disk, *kept));
                if let Some((slug, log, kept, _)) = most {
                    let keep = (kept / 2).max(ROLL_FLOOR);
                    log.with(|l| {
                        let age = l.max_age_ms();
                        l.retain(keep, age);
                        l.want_cut();
                    });
                    // Cut back now, as the periodic sync would within a second (the lock released
                    // but for the swap): the sweep below reads the log as it is, and a run it no
                    // longer keeps but its file still holds would name what it put.
                    log.cut_if_planned();
                    eprintln!("holon: cut {slug:?}'s log to its newest {keep} runs: {}", why(pct));
                    if let Some(dep) = state.deployments.lock().unwrap().get_mut(slug) {
                        dep.note("lifecycle", format!("the host cut its log from {kept} runs to its newest {keep}: {}", why(pct)), None, Default::default());
                    }
                    // A cut each look; the first of a full disk's cuts sweeps too, at once: what
                    // the cut runs alone named goes, an hour after it was put (sweeping after each
                    // cut would hold the next up for as long as a sweep takes).
                    let first = {
                        let mut watch = self.watch.lock().unwrap();
                        let first = watch.rolled.is_empty();
                        watch.rolled.insert(slug.clone());
                        first
                    };
                    changed = true;
                    if first {
                        self.sweep_store(state, now_ms);
                    }
                }
            }
            _ => {}
        }
        if !warned && full_pct.is_none() {
            // Room again: what it did, undone. Under `full_at` too: one written in another unit
            // than `warn_at` may be reached first, and undoing what it does at once would pause
            // and resume (or cut and keep again) at every look.
            let rolled = std::mem::take(&mut self.watch.lock().unwrap().rolled);
            let mut table = state.deployments.lock().unwrap();
            for (slug, dep) in table.iter_mut() {
                if dep.grant.paused_full && dep.life.status == crate::lifecycle::Status::Paused {
                    let _ = dep.life.run(crate::lifecycle::RunFrom::Latest, crate::deploy::epoch_now());
                    dep.arm_clocks(SystemTime::now());
                    println!("holon: resumed {slug:?}: there is room on its disk again");
                    dep.note("lifecycle", "resumed by the host: there is room on its disk again".into(), None, Default::default());
                    changed = true;
                }
                dep.grant.paused_full = false;
                if rolled.contains(slug) {
                    let (keep, age) = (dep.permit.log.keep_runs as usize, dep.permit.log.max_age_ms());
                    dep.log.with(|l| l.retain(keep, age));
                    dep.note("lifecycle", format!("the host keeps its runs as granted again ({keep}): there is room on its disk"), None, Default::default());
                    changed = true;
                }
            }
        }
        if changed {
            crate::persist_deployments(&state.deployments_path, &state.deployments);
            crate::sync_listeners(state);
        }
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
                    // And beside each log, its sealed segments' folder.
                    for sealed in std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".sealed")) {
                        count_dir(&sealed.path(), |_| false, now, &mut left);
                    }
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
        drop(census);
        // Each flow's disk, against the last count: which grows fastest.
        let each = self.all_holds(state);
        let mut watch = self.watch.lock().unwrap();
        let mut grew = std::collections::BTreeMap::new();
        let mut disk = std::collections::BTreeMap::new();
        for f in each["each"].as_array().into_iter().flatten() {
            let (Some(slug), Some(d)) = (f["slug"].as_str(), f["disk"].as_u64()) else { continue };
            if let Some(before) = watch.disk.get(slug) {
                grew.insert(slug.to_string(), d.saturating_sub(*before));
            }
            disk.insert(slug.to_string(), d);
        }
        (watch.disk, watch.grew) = (disk, grew);
        drop(watch);
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
                report.packed = s.packed as u64;
                if s.packed > 0 {
                    println!("holon: packed {} of the store's long-lived values ({})", s.packed, folders::size_text(s.packed_bytes));
                }
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

    /// What `when_full` has done and not yet undone: the deployments it paused, the logs it cut.
    fn acting(&self, state: &AppState) -> serde_json::Value {
        let paused: Vec<String> = state.deployments.lock().unwrap().iter().filter(|(_, d)| d.grant.paused_full).map(|(s, _)| s.clone()).collect();
        let rolled: Vec<String> = self.watch.lock().unwrap().rolled.iter().cloned().collect();
        serde_json::json!({ "paused": paused, "rolled": rolled })
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
        let ((kept, entries, entry_bytes, lines), (segments, sealed_runs, sealed_bytes)) = log.with(|l| (l.holds(), l.sealed_holds()));
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
        let disk = file.on_disk + sealed_bytes + store.map_or(0, |s| s.1) + texts.map_or(0, |t| t.1);
        Some(serde_json::json!({
            "slug": slug,
            "memory": memory,
            "disk": disk,
            "log": { "kept": kept, "entries": entries, "entry_bytes": entry_bytes, "memory": log_memory, "lines": lines, "file": file, "sealed": { "segments": segments, "runs": sealed_runs, "bytes": sealed_bytes } },
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
        if let Some(dir) = state.deployments_path.beside("logs") {
            // Counted now: one file a deployment, and its sealed segments beside it (their bytes,
            // not more logs). What crashes left is removed by the census only.
            logs = std::fs::read_dir(&dir).ok().map(|entries| {
                let mut c = Count::default();
                for e in entries.flatten() {
                    let name = e.file_name();
                    match (name.to_str(), e.metadata()) {
                        (Some(n), Ok(m)) if m.is_file() && n.ends_with(".jsonl") => c.add(&m),
                        (Some(n), Ok(m)) if m.is_file() && is_aside(n) => left.add(&m),
                        (Some(n), Ok(m)) if m.is_dir() && n.ends_with(".sealed") => {
                            for f in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                                match (f.file_name().to_str(), f.metadata()) {
                                    (Some(n), Ok(m)) if m.is_file() && n.ends_with(".jsonl.gz") => c.add_bytes(&m),
                                    (Some(n), Ok(m)) if m.is_file() && is_aside(n) => left.add(&m),
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                }
                c
            });
        }
        serde_json::json!({
            "at_ms": at,
            "store": census.store.map(|(c, at_puts)| {
                // Its values each their own file, and those packed (read now: the packs' index is in memory).
                let packed = state.store.2.packs().map(|p| p.held()).unwrap_or_default();
                serde_json::json!({
                    "values": c.files + packed.values as u64,
                    "bytes": c.bytes + packed.bytes,
                    "on_disk": c.on_disk + packed.on_disk,
                    "put_since": puts.saturating_sub(at_puts),
                    "packed": { "values": packed.values, "packs": packed.packs, "bytes": packed.bytes, "on_disk": packed.on_disk },
                })
            }),
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
#[cfg(not(windows))]
fn memory() -> (Option<u64>, Option<u64>) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else { return (None, None) };
    let field = |name: &str| {
        status.lines().find_map(|l| l.strip_prefix(name)).and_then(|v| v.trim().strip_suffix("kB")).and_then(|v| v.trim().parse::<u64>().ok()).map(|kb| kb * 1024)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// The process's memory, its working set (Windows): resident and its peak, in bytes.
#[cfg(windows)]
fn memory() -> (Option<u64>, Option<u64>) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    // SAFETY: the current process's handle is a pseudo-handle, never closed; `counters` is the
    // struct of the size given, which the call fills.
    if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32) } == 0 {
        return (None, None);
    }
    // The peak is counted a moment behind the set as it grows: never below what is resident now.
    (Some(counters.WorkingSetSize as u64), Some(counters.PeakWorkingSetSize.max(counters.WorkingSetSize) as u64))
}

/// Each disk the server keeps things on, once, with what it keeps there: its size, what is free
/// to the server (what a quota or the space kept for root leaves it: what it can still write),
/// and what is in use. How full it is for the server is what isn't free to it.
fn disks(state: &AppState) -> Vec<serde_json::Value> {
    let mut seen: Vec<(Disk, PathBuf, Vec<&'static str>)> = Vec::new();
    for (what, dir) in places(state) {
        // A folder not made yet is on its parent's disk.
        let Some(at) = dir.ancestors().find(|p| p.exists()).map(Path::to_path_buf) else { continue };
        let Some(disk) = disk_of(&at) else { continue };
        match seen.iter_mut().find(|(d, _, _)| *d == disk) {
            Some((_, _, holds)) => holds.push(what),
            None => seen.push((disk, at, vec![what])),
        }
    }
    seen.into_iter()
        .filter_map(|(disk, at, holds)| {
            let (size, free, used) = space_of(&disk, &at)?;
            Some(serde_json::json!({ "path": at.display().to_string(), "holds": holds, "size": size, "free": free, "used": used }))
        })
        .collect()
}

/// The disk a folder is on: its device.
#[cfg(unix)]
type Disk = u64;

#[cfg(unix)]
fn disk_of(at: &Path) -> Option<Disk> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(at).ok().map(|m| m.dev())
}

/// A disk's size, what is free to the server and what is in use, read at a folder on it.
#[cfg(unix)]
fn space_of(_: &Disk, at: &Path) -> Option<(u64, u64, u64)> {
    let path = std::ffi::CString::new(at.as_os_str().as_encoded_bytes()).ok()?;
    let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a NUL-terminated string and `vfs` is a statvfs the call fills.
    if unsafe { libc::statvfs(path.as_ptr(), &mut vfs) } != 0 {
        return None;
    }
    let unit = vfs.f_frsize as u64;
    let size = vfs.f_blocks as u64 * unit;
    Some((size, vfs.f_bavail as u64 * unit, size.saturating_sub(vfs.f_bfree as u64 * unit)))
}

/// The disk a folder is on: its volume's root (`C:\`, a share's `\\nas\runs\`, or the folder a
/// volume is mounted at), in capitals, as Windows reads a name the same in either case.
#[cfg(windows)]
type Disk = std::ffi::OsString;

#[cfg(windows)]
fn disk_of(at: &Path) -> Option<Disk> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW;
    let wide: Vec<u16> = at.as_os_str().encode_wide().chain(Some(0)).collect();
    // As long as a path may be.
    let mut root = vec![0u16; 32_768];
    // SAFETY: `wide` is NUL-terminated, and `root` holds as many as the length given.
    if unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return None;
    }
    let len = root.iter().position(|c| *c == 0)?;
    Some(std::ffi::OsString::from_wide(&root[..len]).to_ascii_uppercase())
}

/// A disk's size, what is free to the server (what a quota leaves it) and what is in use, read at
/// its root.
#[cfg(windows)]
fn space_of(disk: &Disk, _: &Path) -> Option<(u64, u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let root: Vec<u16> = disk.encode_wide().chain(Some(0)).collect();
    let (mut free, mut size, mut unused) = (0u64, 0u64, 0u64);
    // SAFETY: `root` is NUL-terminated, and each pointer is to a u64 the call fills.
    if unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut free, &mut size, &mut unused) } == 0 {
        return None;
    }
    Some((size, free, size.saturating_sub(unused)))
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
        "warn_at": crate::permissions::threshold_text(state.policy().storage.warn_at),
        "full_at": crate::permissions::threshold_text(state.policy().storage.full_at),
        "when_full": state.policy().storage.when_full.text(),
        "acting": state.sweeps.acting(state),
        "backup": backup_view(state),
        "warnings": warnings(state),
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
    // What to see to first.
    for w in v["warnings"].as_array().into_iter().flatten() {
        lines.push(format!("! {}", s(&w["says"])));
        lines.push(format!("  {}", s(&w["todo"])));
    }
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
        let warns = v["warn_at"].as_str().map(|w| format!(", warns at {w}")).unwrap_or_default();
        lines.push(format!("disk         {}% full: {} free of {} ({} in use{warns}): {} ({})", full(d), size(&d["free"]), size(&d["size"]), size(&d["used"]), holds.join(", "), s(&d["path"])));
    }
    let acting = &v["acting"];
    let names = |x: &serde_json::Value| x.as_array().into_iter().flatten().filter_map(|n| n.as_str()).collect::<Vec<_>>().join(", ");
    if acting["paused"].as_array().is_some_and(|a| !a.is_empty()) {
        lines.push(format!("paused       for the full disk: {}", names(&acting["paused"])));
    }
    if acting["rolled"].as_array().is_some_and(|a| !a.is_empty()) {
        lines.push(format!("cut          for the full disk: {}", names(&acting["rolled"])));
    }
    if let (Some(at), Some(what)) = (v["full_at"].as_str(), v["when_full"].as_str()) {
        lines.push(format!("when full    past {at}: {what}"));
    }
    match v["backup"].as_object() {
        Some(b) => lines.push(format!(
            "backup       {}: {} exports, {}{}",
            b["dir"].as_str().unwrap_or(""),
            n(&b["exports"]),
            size(&b["bytes"]),
            if n(&b["waiting"]) > 0 { format!(", {} batches to make into exports", n(&b["waiting"])) } else { String::new() }
        )),
        None => lines.push(format!("backup       none: the runs a log lets go of go (holon policy --backup {})", crate::paths::EXAMPLE)),
    }
    let k = &v["kept"];
    let counted = k["at_ms"].as_u64().map_or("not counted yet".to_string(), |at| format!("counted {} ago", span(now_ms().saturating_sub(at))));
    lines.push(format!("kept         {counted}"));
    if k["store"].is_object() {
        let st = &k["store"];
        lines.push(format!("  store        {} values, {} on disk ({} of bytes); {} put since", n(&st["values"]), size(&st["on_disk"]), size(&st["bytes"]), n(&st["put_since"])));
        if n(&st["packed"]["values"]) > 0 {
            lines.push(format!("    packed       {} of them in {} packs, {} on disk", n(&st["packed"]["values"]), n(&st["packed"]["packs"]), size(&st["packed"]["on_disk"])));
        }
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
    if n(&log["sealed"]["segments"]) > 0 {
        lines.push(format!("    sealed       {} older runs in {} segments, {} gzipped", n(&log["sealed"]["runs"]), n(&log["sealed"]["segments"]), size(&log["sealed"]["bytes"])));
    }
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

  Prints what the host holds: the server's memory and its caches against their sizes; each disk
  it keeps things on, and how full; what it keeps there (the content store's values, counted at
  its last sweep, the logs, the folders' kept texts, and what a crash left, removed once old);
  how often it sweeps, when next, and what the last sweeps removed; and what each flow holds,
  the most first. Admins.

  --for SLUG               what one flow holds, in memory and on disk (anyone who may read the
                           deployment)
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon host
  holon host --for shop
  holon host --json
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

/// Turn each batch of runs a log wrote aside to the backup
/// (`<backup>/<slug>/holon-archive-<first>-<last>.jsonl`, recorder.rs `keep_aside`) into an export
/// `holon verify` checks, gzipped: `<backup>/<slug>/holon-archive-<slug>-runs-<first>-<last>.json.gz`, with the units the runs ran and the file
/// texts they read that the server keeps (none if the deployment is gone). The batch is removed
/// once its export is written; one that can't be stays (its runs are in it still, as lines), is
/// warned of, and is tried again at the next look, the others made meanwhile.
pub(crate) fn archive(state: &AppState) {
    let Some(dir) = state.policy().storage.backup.clone() else { return };
    let Ok(slugs) = std::fs::read_dir(&dir) else { return };
    let mut failed: Option<String> = None;
    for slug_dir in slugs.flatten().filter(|e| e.path().is_dir()) {
        let slug = slug_dir.file_name().to_string_lossy().into_owned();
        let Ok(batches) = std::fs::read_dir(slug_dir.path()) else { continue };
        // Only the batches a log wrote: whatever else the folder holds is left as it is.
        let mut batches: Vec<PathBuf> = batches.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(is_batch)).collect();
        batches.sort();
        for batch in batches {
            if let Err(e) = archive_one(state, &slug, &batch) {
                let said = format!("{}: {e}", batch.display());
                // Said once, as it starts, not every look it is tried again.
                if state.sweeps.backup_error.lock().unwrap().as_deref() != Some(said.as_str()) && failed.is_none() {
                    eprintln!("holon: backing up {said}");
                }
                failed.get_or_insert(said);
            }
        }
    }
    *state.sweeps.backup_error.lock().unwrap() = failed;
}

fn archive_one(state: &AppState, slug: &str, batch: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(batch).map_err(|e| e.to_string())?;
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let Some(last) = lines.last() else { return std::fs::remove_file(batch).map_err(|e| e.to_string()) };
    let head = crate::recorder::line_hash(last);
    let redactor = state.deployments.lock().unwrap().get(slug).map(|d| crate::secrets::Redactor::of(&d.secrets));
    let export = crate::verify::export_of(&state.store, &state.folders, redactor.as_ref(), slug, lines, head, Vec::new(), String::new(), u64::MAX)?;
    let json = serde_json::to_vec(&export).map_err(|e| e.to_string())?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, &json).map_err(|e| e.to_string())?;
    let bytes = gz.finish().map_err(|e| e.to_string())?;
    let name = batch.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let to = batch.with_file_name(export_name(slug, name).ok_or("not a batch's name")?);
    holon_host::write_atomically(&to, &bytes).map_err(|e| e.to_string())?;
    std::fs::remove_file(batch).map_err(|e| e.to_string())
}

/// What the backup holds: its exports and their bytes, the batches still to be made into one,
/// and why it couldn't be written to last, if it couldn't.
fn backup_view(state: &AppState) -> serde_json::Value {
    let Some(dir) = state.policy().storage.backup.clone() else { return serde_json::Value::Null };
    let (mut exports, mut bytes, mut waiting) = (0u64, 0u64, 0u64);
    for slug_dir in std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| e.path().is_dir()) {
        for f in std::fs::read_dir(slug_dir.path()).into_iter().flatten().flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if is_export(&name) {
                exports += 1;
                bytes += f.metadata().map_or(0, |m| m.len());
            } else if is_batch(&name) {
                waiting += 1;
            }
        }
    }
    serde_json::json!({ "dir": dir.display().to_string(), "exports": exports, "bytes": bytes, "waiting": waiting, "error": backup_error(state) })
}

/// Why the backup can't be written to now, if it can't: a log's cut-back (which then kept its
/// runs), or the housekeeping making an export.
fn backup_error(state: &AppState) -> Option<String> {
    log_backup_error(state).or_else(|| state.sweeps.backup_error.lock().unwrap().clone())
}

/// Why a log's last cut-back couldn't write what it let go of to the backup, if one couldn't.
fn log_backup_error(state: &AppState) -> Option<String> {
    let logs: Vec<(String, std::sync::Arc<crate::recorder::LogCell>)> = state.deployments.lock().unwrap().iter().map(|(s, d)| (s.clone(), std::sync::Arc::clone(&d.log))).collect();
    logs.iter()
        .find_map(|(slug, log)| log.with(|l| l.backup_error().map(|e| format!("{slug}: {e}"))))
}

/// The server's housekeeping: a thread that looks every `LOOK_EVERY` whether a round of sweeps
/// is due, and makes it. `state` is set once the server is built.
pub(crate) fn start(state: std::sync::Arc<std::sync::OnceLock<AppState>>) {
    std::thread::spawn(move || loop {
        // A look at once, so the logs have the policy's backup before they first cut back.
        if let Some(state) = state.get() {
            state.sweeps.tick(state, now_ms());
        }
        std::thread::sleep(LOOK_EVERY);
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

/// The `[storage]` keys an admin sets here, with what each says it is.
const SETTINGS: [(&str, &str); 5] = [
    ("sweep_every", "how often the server sweeps: \"10m\", or \"off\""),
    ("warn_at", "when a disk is full enough to warn of: \"85%\", or \"5G\" free"),
    ("full_at", "when it is full enough to act: \"95%\", or \"1G\" free"),
    ("when_full", "what it does then: \"warn\", \"pause\" or \"roll\""),
    ("backup", "where the runs a log lets go of are written first: a folder's whole path, or \"none\""),
];

/// `GET /admin/host` and `POST /admin/storage` (an admin's: can.rs), or None for any other path.
pub(crate) fn route(state: &AppState, request: &rouille::Request, by: &str) -> Option<Response> {
    match (request.method(), request.url().as_str()) {
        ("GET", "/admin/host") => Some(Response::json(&host_view(state))),
        ("GET", "/admin/warnings") => Some(Response::json(&serde_json::json!({ "warnings": warnings(state) }))),
        ("POST", "/admin/storage") => {
            let body = match crate::http::read_body(request) {
                Ok(b) => b,
                Err(r) => return Some(r),
            };
            let asked: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            let set: Vec<(&str, &str)> = SETTINGS.iter().filter_map(|(k, _)| asked[*k].as_str().map(|v| (*k, v))).collect();
            if set.is_empty() {
                let says: Vec<String> = SETTINGS.iter().map(|(k, what)| format!("{k:?} ({what})")).collect();
                return Some(err(400, format!("say what to set: {}", says.join(", or "))));
            }
            Some(set_storage(state, &set, by))
        }
        _ => None,
    }
}

/// Write `set` into the policy file's `[storage]` and read it again, holding every deployment to
/// it as a reload does. The file is left as it was if the new text isn't a policy.
fn set_storage(state: &AppState, set: &[(&str, &str)], by: &str) -> Response {
    let Some(policy_path) = state.policy_path.at() else { return err(409, "this server reads its policy from no file, so there is none to write it into") };
    let mut text = match std::fs::read_to_string(policy_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return err(500, format!("could not read {policy_path:?}: {e}")),
    };
    // Each key written, then the whole checked: two set at once may only make sense together.
    let mut doc: toml_edit::DocumentMut = match text.parse() {
        Ok(d) => d,
        Err(e) => return err(400, format!("the policy file doesn't parse: {e}")),
    };
    if !doc.contains_table("storage") {
        doc["storage"] = toml_edit::table();
    }
    for (key, value) in set {
        doc["storage"][*key] = toml_edit::value(value.trim());
    }
    text = doc.to_string();
    if let Err(e) = Policy::parse(&text) {
        return err(400, e);
    }
    if let Err(e) = holon_host::write_atomically(policy_path, text.as_bytes()) {
        return err(500, format!("could not write {policy_path:?}: {e}"));
    }
    let reloaded = crate::grants::handle_post_permissions_reload(state);
    if reloaded.status_code != 200 {
        return reloaded;
    }
    for (key, value) in set {
        let what = match *key {
            "warn_at" => "set when the server warns its disk is full",
            "full_at" => "set when the server acts on a full disk",
            "when_full" => "set what the server does when its disk is full",
            "backup" => "set where the server backs up the runs it lets go of",
            _ => "set how often the server sweeps",
        };
        state.activity.record(what, value.trim(), Some(by));
    }
    Response::json(&serde_json::json!({ "sweeps": state.sweeps.view(&state.policy()), "warnings": warnings(state) }))
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

    /// The backup's own files are known by their names, and nothing else in its folder is: it may
    /// be a drive that holds other things.
    #[test]
    fn a_backup_batch_and_its_export_are_known_by_their_names_and_nothing_else_is() {
        let batch = batch_name(3, 17);
        assert_eq!(batch, format!("holon-archive-{:020}-{:020}.jsonl", 3, 17));
        assert!(is_batch(&batch), "{batch}");
        let export = export_name("notes", &batch).unwrap();
        assert_eq!(export, format!("holon-archive-notes-runs-{:020}-{:020}.json.gz", 3, 17));
        assert!(is_export(&export), "{export}");
        assert!(is_export(&export_name("my-runs-app", &batch).unwrap()), "a slug with -runs- in it");
        let unprefixed = format!("{:020}-{:020}.jsonl", 3, 17);
        for not in ["data.jsonl", "holon-archive-data.jsonl", "holon-archive-3-17.jsonl", &unprefixed, &format!("holon-archive-{:020}-{:020}.json", 3, 17), &format!("holon-archive-{:020}-{:019}.jsonl", 3, 17), &format!("{batch}.tmp-1-2"), &format!("x-{batch}")] {
            assert!(!is_batch(not), "{not}");
        }
        assert_eq!(export_name("notes", "data.jsonl"), None);
        for not in ["photos.json.gz", "holon-archive-photos.json.gz", "holon-archive-notes-runs-3-17.json.gz", &format!("notes-runs-{:020}-{:020}.json.gz", 3, 17), &format!("holon-archive--runs-{:020}-{:020}.json.gz", 3, 17)] {
            assert!(!is_export(not), "{not}");
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
                &format!("backup       none: the runs a log lets go of go (holon policy --backup {})", crate::paths::EXAMPLE),
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
