//! Files start runs (`on file inbox`, a `file_arrival` leaf; docs/L4s/named-folders-L4.md §11
//! slices 4 and 5). Every second a thread looks at the folders running deployments watch that the
//! OS says changed, or that hold files still settling, by each file's size and modified time (the
//! folder isn't read); every [`WALK`] it looks at them all anyway, and a folder the OS gives no
//! notices for (a network share, say) every second. A file that is new, or changed,
//! since the deployment last saw it starts one run once it has stopped changing (the same at two
//! looks in a row), and that run's `ingress` is its handle, `{folder, name, size, hash, modified,
//! event}`, as `inbox.file(name)` would read it, `event` new or changed.
//!
//! - **Which files.** A flow's `on file` lines say which folders, whether for files new or
//!   changed there or removed (`deleted`: the handle as last seen, without a hash, event
//!   deleted), and which names (`matching`, a glob). A file no line is for is seen, not run on.
//! - **New since the deploy.** The files a folder holds when the deployment is deployed are seen,
//!   not run on. What it has seen is kept beside its log, so files that arrive while the
//!   server is down, or the deployment paused, start their runs when it is back.
//! - **Not its own.** A file its own runs wrote or copied there doesn't start a run (unless it
//!   changes again), nor does one they deleted, so a flow may write into the folder it watches
//!   without starting itself.
//! - **A deleted file is forgotten:** one put back under its name is new again.

use std::path::PathBuf;

use crate::*;

/// How often the watched folders are looked at.
pub(crate) const POLL: Duration = Duration::from_secs(1);

/// How often every watched folder is looked at, notices or not: a notice the OS dropped costs at
/// most this long.
const WALK: Duration = Duration::from_secs(30);

/// The most runs one look starts for one deployment: a folder that fills at once is worked through
/// over the next looks, and the others' looks aren't held up for long.
const MAX_RUNS_PER_LOOK: usize = 100;

/// What a deployment has seen of the folders it watches.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    /// The folders whose files start its runs (its `FileArrival` capabilities).
    pub(crate) folders: BTreeSet<String>,
    /// Which changes there start them: its `on file` lines.
    triggers: Vec<holon_core::FileTrigger>,
    seen: Mutex<BTreeMap<String, Seen>>,
    /// Where what it has seen is kept; none in memory.
    file: Option<PathBuf>,
}

/// What happened to a file that starts a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Event {
    New,
    Changed,
    /// Removed; as it was last seen.
    Deleted(folders::Signature),
}

impl Event {
    fn name(self) -> &'static str {
        match self {
            Event::New => "new",
            Event::Changed => "changed",
            Event::Deleted(_) => "deleted",
        }
    }
}

/// One folder, as the deployment saw it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Seen {
    /// Each file seen settled (run on, or there when it was first watched), by name.
    files: BTreeMap<String, folders::Signature>,
    /// Files new or changed at the last look, waiting for a look at which they're the same.
    #[serde(skip)]
    settling: BTreeMap<String, folders::Signature>,
}

impl Watch {
    /// The folders `caps` start runs on, for deployment `slug`, as `triggers` say (a folder none
    /// names: its files new or changed), with what it saw of them before (kept beside the
    /// deployments file at `deployments_path`), or none seen yet.
    pub(crate) fn open(deployments_path: &str, slug: &str, caps: &holon_core::Capabilities, mut triggers: Vec<holon_core::FileTrigger>) -> Watch {
        let folders: BTreeSet<String> = caps
            .iter()
            .filter_map(|c| match c {
                Capability::FileArrival(f) => Some(f),
                _ => None,
            })
            .collect();
        triggers.retain(|t| folders.contains(&t.folder));
        for f in &folders {
            if !triggers.iter().any(|t| t.folder == *f) {
                triggers.push(holon_core::FileTrigger { folder: f.clone(), deleted: false, matching: String::new() });
            }
        }
        let file = (deployments_path != ":memory:").then(|| Path::new(deployments_path).with_extension("arrivals").join(format!("{slug}.json")));
        let mut seen: BTreeMap<String, Seen> =
            file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        seen.retain(|f, _| folders.contains(f));
        Watch { folders, triggers, seen: Mutex::new(seen), file }
    }

    /// Whether a change to file `name` of `folder` starts a run.
    fn starts(&self, folder: &str, deleted: bool, name: &str) -> bool {
        self.triggers.iter().any(|t| t.folder == folder && t.starts(deleted, name))
    }

    /// Whether it watches any folder.
    pub(crate) fn watches(&self) -> bool {
        !self.folders.is_empty()
    }

    /// The trigger labels it adds (`Files inbox`).
    pub(crate) fn labels(&self) -> impl Iterator<Item = String> + '_ {
        self.folders.iter().map(|f| format!("Files {f}"))
    }

    /// Forget what it saw (the deployment is deleted).
    pub(crate) fn forget(&self) {
        if let Some(f) = &self.file {
            let _ = std::fs::remove_file(f);
        }
    }

    fn keep(&self, seen: &BTreeMap<String, Seen>) {
        let Some(file) = &self.file else { return };
        let written = serde_json::to_vec(seen).map_err(|e| e.to_string()).and_then(|bytes| {
            std::fs::create_dir_all(file.parent().expect("a file in a folder")).map_err(|e| e.to_string())?;
            write_atomically(file, &bytes).map_err(|e| e.to_string())
        });
        if let Err(e) = written {
            eprintln!("holon: what {} saw of its folders wasn't kept: {e}", file.display());
        }
    }

    /// What its folders hold now isn't new: taken when it's deployed, so a file dropped a moment
    /// later starts a run, though the watcher hasn't looked yet. A folder it saw before keeps
    /// what it saw (a redeploy), and one not bound yet is seen at the first look that finds it.
    pub(crate) fn baseline(&self, files: &folders::Scoped) {
        files.watch(&self.folders);
        let mut seen = self.seen.lock().unwrap();
        let mut changed = false;
        for folder in &self.folders {
            if seen.contains_key(folder) {
                continue;
            }
            if let Ok(now) = files.signatures(folder) {
                seen.insert(folder.clone(), Seen { files: now, settling: BTreeMap::new() });
                changed = true;
            }
        }
        if changed {
            self.keep(&seen);
        }
    }

    /// Look at each folder once (`files` sees them as the deployment does), if `changed` says it
    /// may have changed, it was never looked at, or files there are settling: the files that start
    /// a run now, and what happened to each, by folder, in name order, at most `max`.
    fn look(&self, files: &folders::Scoped, max: usize, may_have_changed: impl Fn(&str) -> bool) -> Vec<(String, String, Event)> {
        files.watch(&self.folders);
        let mut seen = self.seen.lock().unwrap();
        let mut due = Vec::new();
        let mut changed = false;
        for folder in &self.folders {
            if seen.get(folder).is_some_and(|s| s.settling.is_empty()) && !may_have_changed(folder) {
                continue;
            }
            let now = match files.signatures(folder) {
                Ok(now) => now,
                // Unbound, or gone for now: looked at again next time.
                Err(_) => continue,
            };
            let Some(was) = seen.get_mut(folder) else {
                // First looked at: what is there now isn't new.
                seen.insert(folder.clone(), Seen { files: now, settling: BTreeMap::new() });
                changed = true;
                continue;
            };
            // Removed: forgotten, and a run for each a line is for, unless its own run removed it.
            let own = files.took_own_deletes(folder, &now);
            let gone: Vec<(String, folders::Signature)> = was.files.iter().filter(|(n, _)| !now.contains_key(*n)).map(|(n, s)| (n.clone(), *s)).collect();
            for (name, signature) in gone {
                if !own.contains(&name) && self.starts(folder, true, &name) {
                    if due.len() >= max {
                        continue;
                    }
                    due.push((folder.clone(), name.clone(), Event::Deleted(signature)));
                }
                was.files.remove(&name);
                changed = true;
            }
            let mut settling = BTreeMap::new();
            for (name, signature) in now {
                let before = was.files.get(&name).copied();
                if before == Some(signature) {
                    continue;
                }
                if files.took_own(folder, &name, signature) || !self.starts(folder, false, &name) {
                    was.files.insert(name, signature);
                    changed = true;
                } else if was.settling.get(&name) == Some(&signature) && due.len() < max {
                    was.files.insert(name.clone(), signature);
                    due.push((folder.clone(), name, if before.is_some() { Event::Changed } else { Event::New }));
                    changed = true;
                } else {
                    settling.insert(name, signature);
                }
            }
            was.settling = settling;
        }
        if changed {
            self.keep(&seen);
        }
        due
    }
}

/// A deployment that watches folders: its slug, unit, state, permit, secrets, folders and watch.
type Watching = (String, Hash, Option<DeploymentState>, Arc<Permit>, Vault, Arc<folders::Scoped>, Arc<Watch>);

/// One look at every running deployment's watched folders, each deployment on its own thread:
/// the runs new files start, one after another, before it returns.
#[cfg(test)]
pub(crate) fn run_arrivals(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &str) {
    look_at(store, deployments, deployments_path, None);
}

/// What the OS says changed in the watched folders.
struct Notices {
    /// None if the OS gives none: every folder is looked at every time.
    watcher: Option<notify::RecommendedWatcher>,
    /// Each folder watched, by its path, and whether notices come for it.
    watched: BTreeMap<PathBuf, bool>,
    /// The paths changed since the last look.
    changed: Arc<Mutex<BTreeSet<PathBuf>>>,
    /// Whether notices were lost (the OS's queue overflowed): every folder may have changed.
    lost: Arc<std::sync::atomic::AtomicBool>,
    walked: std::time::Instant,
}

impl Notices {
    fn new() -> Notices {
        let changed: Arc<Mutex<BTreeSet<PathBuf>>> = Arc::default();
        let lost: Arc<std::sync::atomic::AtomicBool> = Arc::default();
        let (c, l) = (Arc::clone(&changed), Arc::clone(&lost));
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
            Ok(e) if e.need_rescan() => l.store(true, std::sync::atomic::Ordering::Relaxed),
            Ok(e) => c.lock().unwrap().extend(e.paths),
            Err(_) => l.store(true, std::sync::atomic::Ordering::Relaxed),
        });
        if let Err(e) = &watcher {
            eprintln!("holon: no change notices for watched folders ({e}): each is looked at every second");
        }
        Notices { watcher: watcher.ok(), watched: BTreeMap::new(), changed, lost, walked: std::time::Instant::now() }
    }

    /// Watch `paths` (and no others), and say which of them may have changed since the last time:
    /// those notices came for, those new here, those no notices come for; every one, every [`WALK`].
    fn changed(&mut self, paths: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
        use notify::Watcher;
        let gone: Vec<PathBuf> = self.watched.keys().filter(|p| !paths.contains(*p)).cloned().collect();
        for p in gone {
            if self.watched.remove(&p) == Some(true)
                && let Some(w) = &mut self.watcher
            {
                let _ = w.unwatch(&p);
            }
        }
        let mut due = BTreeSet::new();
        for p in paths {
            if !self.watched.contains_key(p) {
                let notified = self.watcher.as_mut().is_some_and(|w| w.watch(p, notify::RecursiveMode::Recursive).is_ok());
                self.watched.insert(p.clone(), notified);
                due.insert(p.clone());
            }
        }
        let changed = std::mem::take(&mut *self.changed.lock().unwrap());
        let lost = self.lost.swap(false, std::sync::atomic::Ordering::Relaxed);
        let walk = lost || self.walked.elapsed() >= WALK;
        if walk {
            self.walked = std::time::Instant::now();
        }
        for (p, notified) in &self.watched {
            if walk || !notified || changed.iter().any(|c| c.starts_with(p)) {
                due.insert(p.clone());
            }
        }
        due
    }
}

/// One look at the watched folders: all of them without `notices`, else those that may have
/// changed.
fn look_at(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &str, notices: Option<&mut Notices>) {
    let watching: Vec<Watching> = deployments
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, d)| d.arrivals.watches() && d.life.status == Status::Running && d.server.is_none())
        .map(|(slug, d)| {
            (slug.clone(), d.unit, d.state.clone(), Arc::clone(&d.permit), Arc::clone(&d.secrets), Arc::clone(&d.folders), Arc::clone(&d.arrivals))
        })
        .collect();
    // The folders each looks at: by their paths, those that may have changed.
    let paths = |files: &folders::Scoped, watch: &Watch| -> BTreeMap<String, PathBuf> {
        watch.folders.iter().filter_map(|f| files.get(f).map(|b| (f.clone(), b.path))).collect()
    };
    let due: Option<BTreeSet<PathBuf>> = notices.map(|n| n.changed(&watching.iter().flat_map(|w| paths(&w.5, &w.6).into_values()).collect()));
    thread::scope(|scope| {
        for (slug, unit, state, permit, secrets, files, watch) in watching {
            let at = paths(&files, &watch);
            let due = due.as_ref();
            scope.spawn(move || {
                let may_have_changed = |f: &str| due.is_none_or(|due| at.get(f).is_none_or(|p| due.contains(p)));
                for (folder, name, event) in watch.look(&files, MAX_RUNS_PER_LOOK, may_have_changed) {
                    fire_one_arrival(store, deployments, deployments_path, &slug, unit, state.as_ref(), &permit, &secrets, &files, (&folder, &name, event));
                }
            });
        }
    });
}

/// One run for file `name` of `folder`: its handle is what arrived, or what was removed.
#[allow(clippy::too_many_arguments)]
fn fire_one_arrival(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    deployments_path: &str,
    slug: &str,
    unit_hash: Hash,
    state: Option<&DeploymentState>,
    permit: &Permit,
    secrets: &Vault,
    files: &Arc<folders::Scoped>,
    (folder, name, event): (&str, &str, Event),
) {
    let handle = match event {
        Event::Deleted((size, modified)) => Value::Record(BTreeMap::from([
            ("folder".to_string(), Value::Text(folder.to_string())),
            ("name".to_string(), Value::Text(name.to_string())),
            ("size".to_string(), Value::Number(size as f64)),
            ("modified".to_string(), Value::Number(modified as f64)),
        ])),
        // Gone, or unreadable, since the look: nothing to run on.
        _ => match files.handle(folder, name) {
            Ok(Some(h)) => h,
            Ok(None) => return,
            Err(e) => {
                eprintln!("{} file [{slug}] {folder}/{name}: {e}", event.name());
                return;
            }
        },
    };
    let Value::Record(mut handle) = handle else { return };
    handle.insert("event".to_string(), Value::Text(event.name().to_string()));
    let handle = Value::Record(handle);
    let resolver = match materialize_closure(store, unit_hash) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("new file [{slug}]: store error while materializing closure: {e:?}");
            record_trigger(deployments, slug, true);
            return;
        }
    };
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        eprintln!("new file [{slug}]: deployed unit no longer resolves in the store");
        record_trigger(deployments, slug, true);
        return;
    };
    let epoch_seconds = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let extra_sourced = [
        (Capability::FileArrival(folder.to_string()), handle),
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Number(rand::random::<f64>())),
    ];
    let started = started_now();
    let channels = deployments.lock().unwrap().get(slug).map(|d| Arc::clone(&d.channels));
    let mut waits = RunWaits { folders: Some(Arc::clone(files)), channels, ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root_unit, &[], &resolver, store, state, &extra_sourced, permit, true, secrets, &mut waits, None);
    let mut entry = run_entry("file", started, &extra_sourced, &run);
    match durable::park_if_waiting(deployments, store, slug, &unit_hash, "file", None, &[], &extra_sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    let parked_any = !entry.parked.is_empty();
    log_run(deployments, slug, entry, Redactor::tokens(&waits.tokens));
    println!("{} file [{slug}] {folder}/{name}: {} effect(s) performed", event.name(), run.performed.len());
    record_trigger(deployments, slug, false);
    if state.is_some() || parked_any {
        persist_deployments(deployments_path, deployments, store);
    }
}

/// The watcher: a thread that looks at the watched folders that may have changed every [`POLL`].
pub(crate) fn spawn_watcher(store: SharedStore, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: String) {
    thread::spawn(move || {
        let mut notices = Notices::new();
        loop {
            thread::sleep(POLL);
            look_at(&store, &deployments, &deployments_path, Some(&mut notices));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder is looked at when first watched, then when the OS says it changed (a file dropped
    /// in a folder inside it too), and not otherwise.
    #[test]
    fn notices_say_which_folders_changed() {
        let dir = std::env::temp_dir().join(format!("holon-notices-{}-{}", std::process::id(), rand::random::<u32>()));
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(a.join("inner")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let (a, b) = (a.canonicalize().unwrap(), b.canonicalize().unwrap());
        let mut notices = Notices::new();
        let paths = BTreeSet::from([a.clone(), b.clone()]);
        assert_eq!(notices.changed(&paths), paths, "new here: looked at");
        if notices.watched.values().any(|n| !n) {
            return; // No notices on this file system: every folder every second, as said.
        }
        assert!(notices.changed(&paths).is_empty(), "nothing changed");
        std::fs::write(a.join("inner").join("x.csv"), "x").unwrap();
        let t0 = std::time::Instant::now();
        let mut due = BTreeSet::new();
        while due.is_empty() && t0.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(50));
            due = notices.changed(&paths);
        }
        assert_eq!(due, BTreeSet::from([a.clone()]), "only the folder the file was dropped in");
        notices.walked -= WALK;
        assert_eq!(notices.changed(&paths), paths, "every one, every WALK");
        assert_eq!(notices.changed(&BTreeSet::from([b.clone()])), BTreeSet::new(), "one no longer watched is dropped");
        assert_eq!(notices.watched.keys().cloned().collect::<Vec<_>>(), [b]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
