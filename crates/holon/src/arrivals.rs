//! New files start runs (`on file inbox`, a `file_arrival` leaf; docs/L4s/named-folders-L4.md
//! §11 slice 4). Every second a thread looks at each folder a running deployment watches, by each
//! file's size and modified time (the folder isn't read). A file that is new, or changed, since
//! the deployment last saw it starts one run once it has stopped changing (the same at two looks
//! in a row), and that run's `ingress` is its handle, `{folder, name, size, hash, modified}`, as
//! `inbox.file(name)` would read it.
//!
//! - **New since the deploy.** The files a folder holds when the deployment is deployed are seen,
//!   not run on. What it has seen is kept beside its log, so files that arrive while the
//!   server is down, or the deployment paused, start their runs when it is back.
//! - **Not its own.** A file its own runs wrote or copied there doesn't start a run (unless it
//!   changes again), so a flow may write into the folder it watches without starting itself.
//! - **A deleted file is forgotten:** one put back under its name is new again.

use std::path::PathBuf;

use crate::*;

/// How often the watched folders are looked at.
pub(crate) const POLL: Duration = Duration::from_secs(1);

/// The most runs one look starts for one deployment: a folder that fills at once is worked through
/// over the next looks, and the others' looks aren't held up for long.
const MAX_RUNS_PER_LOOK: usize = 100;

/// What a deployment has seen of the folders it watches.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    /// The folders whose new files start its runs (its `FileArrival` capabilities).
    pub(crate) folders: BTreeSet<String>,
    seen: Mutex<BTreeMap<String, Seen>>,
    /// Where what it has seen is kept; none in memory.
    file: Option<PathBuf>,
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
    /// The folders `caps` start runs on, for deployment `slug`, with what it saw of them before
    /// (kept beside the deployments file at `deployments_path`), or none seen yet.
    pub(crate) fn open(deployments_path: &str, slug: &str, caps: &holon_core::Capabilities) -> Watch {
        let folders: BTreeSet<String> = caps
            .iter()
            .filter_map(|c| match c {
                Capability::FileArrival(f) => Some(f),
                _ => None,
            })
            .collect();
        let file = (deployments_path != ":memory:").then(|| Path::new(deployments_path).with_extension("arrivals").join(format!("{slug}.json")));
        let mut seen: BTreeMap<String, Seen> =
            file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        seen.retain(|f, _| folders.contains(f));
        Watch { folders, seen: Mutex::new(seen), file }
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

    /// Look at each folder once (`files` sees them as the deployment does): the files that start
    /// a run now, by folder, in name order, at most `max`.
    fn look(&self, files: &folders::Scoped, max: usize) -> Vec<(String, String)> {
        files.watch(&self.folders);
        let mut seen = self.seen.lock().unwrap();
        let mut due = Vec::new();
        let mut changed = false;
        for folder in &self.folders {
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
            let before = was.files.len();
            was.files.retain(|name, _| now.contains_key(name));
            changed |= was.files.len() != before;
            let mut settling = BTreeMap::new();
            for (name, signature) in now {
                if was.files.get(&name) == Some(&signature) {
                    continue;
                }
                if files.took_own(folder, &name, signature) {
                    was.files.insert(name, signature);
                    changed = true;
                } else if was.settling.get(&name) == Some(&signature) && due.len() < max {
                    was.files.insert(name.clone(), signature);
                    due.push((folder.clone(), name));
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
pub(crate) fn run_arrivals(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &str) {
    let watching: Vec<Watching> = deployments
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, d)| d.arrivals.watches() && d.life.status == Status::Running && d.server.is_none())
        .map(|(slug, d)| {
            (slug.clone(), d.unit, d.state.clone(), Arc::clone(&d.permit), Arc::clone(&d.secrets), Arc::clone(&d.folders), Arc::clone(&d.arrivals))
        })
        .collect();
    thread::scope(|scope| {
        for (slug, unit, state, permit, secrets, files, watch) in watching {
            scope.spawn(move || {
                for (folder, name) in watch.look(&files, MAX_RUNS_PER_LOOK) {
                    fire_one_arrival(store, deployments, deployments_path, &slug, unit, state.as_ref(), &permit, &secrets, &files, &folder, &name);
                }
            });
        }
    });
}

/// One run for file `name` of `folder`: its handle is what arrived.
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
    folder: &str,
    name: &str,
) {
    // Gone, or unreadable, since the look: nothing to run on.
    let handle = match files.handle(folder, name) {
        Ok(Some(h)) => h,
        Ok(None) => return,
        Err(e) => {
            eprintln!("new file [{slug}] {folder}/{name}: {e}");
            return;
        }
    };
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
    let mut waits = RunWaits { folders: Some(Arc::clone(files)), ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root_unit, &[], &resolver, store, state, &extra_sourced, permit, true, secrets, &mut waits, None);
    let mut entry = run_entry("file", started, &extra_sourced, &run);
    match durable::park_if_waiting(deployments, store, slug, &unit_hash, "file", None, &[], &extra_sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    Redactor::tokens(&waits.tokens).entry(&mut entry);
    let parked_any = !entry.parked.is_empty();
    log_run(deployments, slug, entry);
    println!("new file [{slug}] {folder}/{name}: {} effect(s) performed", run.performed.len());
    record_trigger(deployments, slug, false);
    if state.is_some() || parked_any {
        persist_deployments(deployments_path, deployments, store);
    }
}

/// The watcher: a thread that looks at the watched folders every [`POLL`].
pub(crate) fn spawn_watcher(store: SharedStore, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: String) {
    thread::spawn(move || loop {
        thread::sleep(POLL);
        run_arrivals(&store, &deployments, &deployments_path);
    });
}
