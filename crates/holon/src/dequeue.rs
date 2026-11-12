//! Queues start runs (`on.queue(payouts, (m) => …)`, a `queue_message` leaf; host-capabilities L4
//! §5.9.9). Every second a thread compares the queues running deployments listen to with the
//! listeners it holds, and starts or stops them: one a deployment and queue, started anew when the
//! deployment's version or the queue's binding changes. Each has a thread that long-polls the
//! queue (sqs.rs: `ReceiveMessage`, 20 s, up to 10, each kept out of sight for the binding's
//! `--visibility`), runs the handler on each message received, one at a time, in the order they
//! came, `m` the message: `{data, id, attributes, sent, count, group}`, `data` a value when it is
//! JSON, else its text; and receives again once their runs are over. A thread beside it keeps
//! what was received and isn't deleted out of sight, every half of the visibility, so a long run
//! keeps its message.
//!
//! - **Deleted once its run's record is on disk:** a strict or an unrecorded run's at its end, a
//!   lazy one's once the sync covering its record is done (`Queues::release`). A failed run's too,
//!   its record saying why, unless the binding says `--keep-failed`: then it is left, to come back
//!   once its visibility runs out, for the queue's redrive policy to count.
//! - **Once per message:** the last [`RECENT`] message ids run on are kept beside the log
//!   (`queue-seen/<slug>/<queue>.json`), each written once its record is on disk, before its
//!   message is deleted; a message among them (one whose delete a crash cut off, a standard queue's
//!   second delivery) is deleted without a run, and the timeline says so.
//! - **A queue that can't be reached** is tried again after a second, then twice as long each
//!   time, to a minute. The timeline says so once, and again when it is read again.
//! - **Stopped** (a new version, its binding changed or removed, the deployment paused or
//!   deleted): what was received and hasn't run is made visible again at once.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::sync::Condvar;

use crate::queues::{Binding, Queues};
use crate::sqs::Received;
use crate::*;

/// How often the thread compares what is listened to with what it holds.
const TICK: Duration = Duration::from_secs(1);

/// How many of the message ids last run on are kept, to know a message handed out again.
pub(crate) const RECENT: usize = 1000;

/// The first wait before a queue that couldn't be reached is tried again, and the longest.
const FIRST_WAIT: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// How long a receive waits for a message (SQS's longest), and how many it takes at once (its
/// most).
const WAIT: u32 = 20;
const BATCH: u32 = 10;

/// The ids of the messages of one queue a deployment ran on, oldest first.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Clone)]
struct Seen {
    #[serde(default)]
    recent: VecDeque<String>,
}

/// Where what `slug` ran on of its queues is kept: none in memory.
fn seen_dir(deployments_path: &Place, slug: &str) -> Option<PathBuf> {
    deployments_path.beside("queue-seen").map(|dir| PathBuf::from(dir).join(slug))
}

/// Forget what `slug` ran on of its queues (it is deleted), with what its lazy runs hold.
pub(crate) fn forget(deployments_path: &Place, slug: &str, queues: &Queues) {
    queues.forget_held(slug);
    if let Some(dir) = seen_dir(deployments_path, slug) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Where the ids one deployment ran on of one queue are kept: a file, or the server's memory.
#[derive(Clone)]
struct Kept {
    file: Option<PathBuf>,
    key: PathBuf,
    memory: Arc<Mutex<BTreeMap<PathBuf, Vec<u8>>>>,
    /// Held while the ids are read and written again: they are added from the listener's thread
    /// and from the sync's.
    lock: Arc<Mutex<()>>,
}

impl Kept {
    fn read(&self) -> Seen {
        let bytes = match &self.file {
            Some(f) => std::fs::read(f).ok(),
            None => self.memory.lock().unwrap().get(&self.key).cloned(),
        };
        bytes.and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Add `id` to the ids kept, the oldest past [`RECENT`] forgotten.
    fn add(&self, id: &str) {
        let _held = self.lock.lock().unwrap();
        let mut seen = self.read();
        if seen.recent.iter().any(|r| r == id) {
            return;
        }
        seen.recent.push_back(id.to_string());
        while seen.recent.len() > RECENT {
            seen.recent.pop_front();
        }
        let bytes = serde_json::to_vec(&seen).unwrap_or_default();
        let Some(file) = &self.file else {
            self.memory.lock().unwrap().insert(self.key.clone(), bytes);
            return;
        };
        let written = std::fs::create_dir_all(file.parent().expect("a file in a folder")).and_then(|_| write_atomically(file, &bytes));
        if let Err(e) = written {
            eprintln!("holon: what {} ran on wasn't kept: {e}", file.display());
        }
    }
}

/// What a listener's two threads share.
struct Shared {
    slug: String,
    queue: String,
    stop: AtomicBool,
    sleeping: Mutex<()>,
    changed: Condvar,
    /// The queue, its URL read when the listener started or last couldn't reach it.
    client: Mutex<Option<Arc<sqs::Queue>>>,
    /// The receipts of what was received and isn't deleted: kept out of sight.
    out: Mutex<Vec<String>>,
    queues: Queues,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.changed.notify_all();
    }

    /// Wait `wait`, or until it is stopped: whether it was.
    fn sleep(&self, wait: Duration) -> bool {
        let sleeping = self.sleeping.lock().unwrap();
        let _ = self.changed.wait_timeout_while(sleeping, wait, |_| !self.stopped()).unwrap();
        self.stopped()
    }

    /// Stop keeping `receipt`'s message out of sight: deleted, or left to come back.
    fn let_go(&self, receipt: &str) {
        self.out.lock().unwrap().retain(|r| r != receipt);
    }

    fn note(&self, deployments: &Mutex<BTreeMap<String, Deployment>>, what: String) {
        queues::note(deployments, &self.slug, &self.queue, what);
    }
}

/// One deployment's listener to one queue: what it was started for (the deployment's unit, the
/// binding), and its threads' shared state.
struct Listener {
    version: (Hash, Binding),
    shared: Arc<Shared>,
}

/// The listeners the thread holds.
#[derive(Default)]
pub(crate) struct Held {
    listeners: BTreeMap<(String, String), Listener>,
    /// What each ran on, for a server whose deployments are kept nowhere (`:memory:`).
    memory: Arc<Mutex<BTreeMap<PathBuf, Vec<u8>>>>,
    lock: Arc<Mutex<()>>,
}

impl Drop for Held {
    fn drop(&mut self) {
        for l in self.listeners.values() {
            l.shared.close();
        }
    }
}

/// Start the listeners running deployments' queue handlers want and aren't held, and stop those
/// held that aren't wanted, or are wanted otherwise.
pub(crate) fn reconcile(store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: &Place, held: &mut Held) {
    let listening: Vec<((String, String), Hash, Arc<folders::Scoped>)> = deployments
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, d)| d.life.status == Status::Running)
        .flat_map(|(slug, d)| {
            let handlers = d.server.as_ref().map(|s| s.queues.clone()).unwrap_or_default();
            handlers.into_iter().map(|h| ((slug.clone(), h.queue), d.unit, Arc::clone(&d.folders)))
        })
        .collect();
    let mut wanted: BTreeMap<(String, String), ((Hash, Binding), Queues)> = BTreeMap::new();
    for (key, unit, files) in listening {
        let queues = files.all.queues.clone();
        if let Some(binding) = queues.resolve(&key.0, &key.1) {
            wanted.insert(key, ((unit, binding), queues));
        }
    }
    held.listeners.retain(|key, l| {
        let keep = wanted.get(key).is_some_and(|(version, _)| *version == l.version);
        if !keep {
            l.shared.close();
            l.shared.queues.forget_live(&key.0, &key.1);
        }
        keep
    });
    for (key, (version, queues)) in wanted {
        if held.listeners.contains_key(&key) {
            continue;
        }
        let kept = Kept {
            file: seen_dir(deployments_path, &key.0).map(|d| d.join(format!("{}.json", key.1))),
            key: PathBuf::from(&key.0).join(format!("{}.json", key.1)),
            memory: Arc::clone(&held.memory),
            lock: Arc::clone(&held.lock),
        };
        let shared = Arc::new(Shared {
            slug: key.0.clone(),
            queue: key.1.clone(),
            stop: AtomicBool::new(false),
            sleeping: Mutex::new(()),
            changed: Condvar::new(),
            client: Mutex::new(None),
            out: Mutex::default(),
            queues,
        });
        start(store, deployments, deployments_path, &shared, &version.1, kept);
        held.listeners.insert(key, Listener { version, shared });
    }
}

/// Start `shared`'s listening thread and the one that keeps what it received out of sight.
fn start(store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: &Place, shared: &Arc<Shared>, binding: &Binding, kept: Kept) {
    let address = binding.address.clone();
    shared.queues.set_live(&shared.slug, &shared.queue, |l| {
        *l = queues::Live { address, status: "receiving", since: feeds::now_ms(), ..queues::Live::default() };
    });
    let (listener, deps, store, path, b) = (Arc::clone(shared), Arc::clone(deployments), store.clone(), deployments_path.clone(), binding.clone());
    thread::spawn(move || listen(&listener, &store, &deps, &path, &b, &kept));
    let (keeper, visibility) = (Arc::clone(shared), binding.visibility);
    thread::spawn(move || keep_out_of_sight(&keeper, visibility));
}

/// The queue, its URL read now unless it was already.
fn client(shared: &Shared, binding: &Binding) -> Result<Arc<sqs::Queue>, String> {
    let mut client = shared.client.lock().unwrap();
    if let Some(c) = client.as_ref() {
        return Ok(Arc::clone(c));
    }
    let made = Arc::new(shared.queues.client(&binding.url)?);
    *client = Some(Arc::clone(&made));
    Ok(made)
}

/// The listening thread: what the queue hands out, run one at a time, received again once each
/// batch is run, until it is stopped.
fn listen(shared: &Arc<Shared>, store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: &Place, binding: &Binding, kept: &Kept) {
    let queue = &shared.queue;
    let visibility = binding.visibility as u32;
    let mut recent = kept.read().recent;
    let (mut wait, mut failing, mut listened) = (FIRST_WAIT, false, false);
    // A server stopping (stopping.rs) receives no more: its runs are what it already took.
    while !shared.stopped() && !store.1.stopping() {
        let received = client(shared, binding).and_then(|c| c.receive(BATCH, WAIT, visibility).map(|batch| (c, batch)).map_err(|f| f.why().to_string()));
        let (client, batch) = match received {
            Ok(r) => r,
            Err(why) => {
                if !failing {
                    shared.note(deployments, format!("couldn't receive from the queue {queue}: {why}; trying again in {}", folders::span_text(wait.as_secs())));
                    failing = true;
                }
                shared.queues.set_live(&shared.slug, queue, |l| {
                    if l.status != "unreachable" {
                        (l.status, l.since) = ("unreachable", feeds::now_ms());
                    }
                    l.error = Some(why);
                });
                // Its URL read again when it is tried again.
                *shared.client.lock().unwrap() = None;
                if shared.sleep(wait) {
                    break;
                }
                wait = (wait * 2).min(LONGEST_WAIT);
                continue;
            }
        };
        if !listened {
            shared.note(deployments, format!("listens to the queue {queue} ({})", binding.address));
            listened = true;
        } else if failing {
            shared.note(deployments, format!("receives from the queue {queue} again"));
        }
        (failing, wait) = (false, FIRST_WAIT);
        shared.out.lock().unwrap().extend(batch.iter().map(|m| m.receipt.clone()));
        shared.queues.set_live(&shared.slug, queue, |l| {
            if l.status != "receiving" {
                (l.status, l.since) = ("receiving", feeds::now_ms());
            }
            (l.error, l.waiting) = (None, batch.len());
        });
        for (i, m) in batch.iter().enumerate() {
            if shared.stopped() || store.1.stopping() {
                // What hasn't run is handed out again at once, to what listens next.
                for rest in &batch[i..] {
                    let _ = client.extend(&rest.receipt, 0);
                    shared.let_go(&rest.receipt);
                }
                break;
            }
            shared.queues.set_live(&shared.slug, queue, |l| l.waiting = batch.len() - i - 1);
            if recent.contains(&m.id) {
                shared.note(deployments, format!("passed over a message of {queue} it ran already ({})", m.id));
                shared.queues.set_live(&shared.slug, queue, |l| l.passed_over += 1);
                delete(shared, deployments, &client, m);
                continue;
            }
            let Some(listening) = listening(store, deployments, &shared.slug, queue) else {
                let _ = client.extend(&m.receipt, 0);
                shared.let_go(&m.receipt);
                continue;
            };
            let Some((held_for, failed)) = fire_one_queue(store, deployments, deployments_path, &listening, value_of(m)) else {
                // It didn't run (the host's own failure): it comes back once its visibility runs out.
                shared.let_go(&m.receipt);
                continue;
            };
            shared.queues.set_live(&shared.slug, queue, |l| l.run += 1);
            if failed && binding.keep_failed {
                // Left for the queue to hand out again once its visibility runs out.
                shared.let_go(&m.receipt);
                shared.queues.set_live(&shared.slug, queue, |l| l.kept += 1);
                continue;
            }
            recent.push_back(m.id.clone());
            while recent.len() > RECENT {
                recent.pop_front();
            }
            let (shared, deployments, client, kept, m) = (Arc::clone(shared), Arc::clone(deployments), Arc::clone(&client), kept.clone(), m.clone());
            let landed: queues::Landed = Box::new(move || {
                kept.add(&m.id);
                delete(&shared, &deployments, &client, &m);
            });
            match held_for {
                Some(seq) => listening.6.all.queues.hold(&listening.0, seq, landed),
                None => landed(),
            }
        }
    }
}

/// Delete `m`, received from `client`: said on the timeline when it can't be, as it comes back.
fn delete(shared: &Shared, deployments: &Mutex<BTreeMap<String, Deployment>>, client: &sqs::Queue, m: &Received) {
    if let Err(f) = client.delete(&m.receipt) {
        shared.note(deployments, format!("couldn't delete a message of {} ({}): {}; it is passed over when it comes back", shared.queue, m.id, f.why()));
    }
    shared.let_go(&m.receipt);
}

/// The thread that keeps what was received and isn't deleted out of sight, every half of the
/// visibility, until it is stopped.
fn keep_out_of_sight(shared: &Shared, visibility: u64) {
    let every = Duration::from_secs((visibility / 2).max(1));
    while !shared.sleep(every) {
        let receipts = shared.out.lock().unwrap().clone();
        let Some(client) = shared.client.lock().unwrap().clone() else { continue };
        for receipt in receipts {
            // One deleted meanwhile is refused: nothing to keep.
            let _ = client.extend(&receipt, visibility as u32);
        }
    }
}

/// `m`, as the handler reads it: `{data, id, attributes, sent, count, group}`, `data` a value when
/// the body is JSON, else its text; `group` only for a FIFO queue's.
fn value_of(m: &Received) -> Value {
    let data = holon_core::parse_json(&m.body).unwrap_or_else(|| Value::Text(m.body.clone()));
    let attributes = m.attributes.iter().map(|(k, v)| (k.clone(), Value::Text(v.clone()))).collect();
    let mut r = BTreeMap::from([
        ("data".to_string(), data),
        ("id".to_string(), Value::Text(m.id.clone())),
        ("attributes".to_string(), Value::Record(attributes)),
        ("sent".to_string(), Value::Number((m.sent_ms / 1000) as f64)),
        ("count".to_string(), Value::Number(m.count as f64)),
    ]);
    if let Some(g) = &m.group {
        r.insert("group".to_string(), Value::Text(g.clone()));
    }
    Value::Record(r)
}

/// A deployment listening to a queue: its slug, the handler, unit, state, permit, secrets and
/// folders.
type Listening = (String, holon_core::QueueHandler, Hash, Option<DeploymentState>, Arc<Permit>, Vault, Arc<folders::Scoped>);

/// `slug`'s handler of `queue` as it is now, if `slug` takes runs and listens to it.
fn listening(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, queue: &str) -> Option<Listening> {
    let deployments = deployments.lock().unwrap();
    let d = deployments.get(slug).filter(|d| d.takes_runs(store))?;
    let handler = d.server.as_ref()?.queues.iter().find(|q| q.queue == queue)?.clone();
    Some((slug.to_string(), handler, d.unit, d.state.clone(), Arc::clone(&d.permit), Arc::clone(&d.secrets), Arc::clone(&d.folders)))
}

/// One run of `l`'s handler for `message`, logged under its label: the seq of its entry when its
/// record is lazy, which its delete waits for, and whether it failed. None: it didn't run.
fn fire_one_queue(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &Place, l: &Listening, message: Value) -> Option<(Option<u64>, bool)> {
    let (slug, handler, unit_hash, state, permit, secrets, files) = l;
    let resolver = match closure(store, *unit_hash) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("queue message [{slug}]: store error while materializing closure: {e:?}");
            record_trigger(deployments, slug, true);
            return None;
        }
    };
    // The handler runs alone: its unit is the run's.
    let unit_hash = handler.handler;
    let label = handler.label.clone();
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        eprintln!("queue message [{slug}]: deployed unit no longer resolves in the store");
        record_trigger(deployments, slug, true);
        return None;
    };
    let epoch_seconds = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let extra_sourced = [
        (Capability::QueueMessage(handler.queue.clone()), message),
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Bytes(rand::random::<[u8; 32]>().to_vec())),
    ];
    let started = started_now();
    let channels = deployments.lock().unwrap().get(slug.as_str()).map(|d| Arc::clone(&d.channels));
    let declared = handler.log;
    let level = permit.log_level(Some(&label), declared);
    let unrecorded = level == holon_core::LogLevel::Unrecorded;
    let mut waits = RunWaits { folders: Some(Arc::clone(files)), channels, slug: Some(slug.to_string()), unrecorded, ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root_unit, &[], &resolver, store, state.as_ref(), &extra_sourced, permit, true, secrets, &mut waits, None);
    let mut entry = run_entry("queue", started, &extra_sourced, &run);
    entry.route = Some(label.clone());
    match durable::park_if_waiting(deployments, store, slug, &unit_hash, "queue", Some(label.clone()), None, declared, &[], &extra_sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    let (parked_any, failed) = (!entry.parked.is_empty(), entry.error.is_some());
    let logged = log_run(deployments, slug, entry, Redactor::tokens(&waits.tokens).and(Redactor::handed(&calls::handed(&run.rounds))), declared);
    let held_for = logged.as_ref().and_then(|l| l.seq).filter(|_| level == holon_core::LogLevel::Lazy);
    if let Some(l) = logged {
        l.said(slug);
    }
    println!("queue message [{slug}] {label}: {} effect(s) performed", run.performed.len());
    record_trigger(deployments, slug, false);
    if parked_any {
        persist_deployments(deployments_path, deployments);
    }
    Some((held_for, failed))
}

/// The thread that holds the queues' listeners, comparing them with what is listened to every
/// [`TICK`].
pub(crate) fn spawn(store: SharedStore, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: Place) {
    thread::spawn(move || {
        let mut held = Held::default();
        loop {
            thread::sleep(TICK);
            reconcile(&store, &deployments, &deployments_path, &mut held);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_message_is_its_data_its_id_its_attributes_when_it_was_sent_and_its_count() {
        let m = Received {
            id: "m-1".into(),
            receipt: "r".into(),
            body: "{\"type\":\"payout.paid\"}".into(),
            attributes: BTreeMap::from([("source".to_string(), "acme".to_string())]),
            sent_ms: 1_700_000_000_123,
            count: 2,
            group: Some("acct_1".into()),
        };
        let v = value_of(&m);
        let Value::Record(r) = &v else { panic!("a record") };
        assert_eq!(r["data"], Value::Record(BTreeMap::from([("type".to_string(), Value::Text("payout.paid".into()))])));
        assert_eq!((&r["id"], &r["sent"], &r["count"], &r["group"]), (&Value::Text("m-1".into()), &Value::Number(1_700_000_000.0), &Value::Number(2.0), &Value::Text("acct_1".into())));
        assert_eq!(r["attributes"], Value::Record(BTreeMap::from([("source".to_string(), Value::Text("acme".into()))])));
        // Text that isn't JSON is its text; a standard queue's has no group.
        let plain = value_of(&Received { body: "hello".into(), group: None, ..m });
        let Value::Record(r) = plain else { panic!("a record") };
        assert_eq!((r["data"].clone(), r.contains_key("group")), (Value::Text("hello".into()), false));
    }

    #[test]
    fn the_ids_run_on_are_kept_the_oldest_forgotten() {
        let kept = Kept { file: None, key: Path::new("app").join("payouts.json"), memory: Arc::default(), lock: Arc::default() };
        assert_eq!(kept.read(), Seen::default());
        for n in 0..RECENT + 2 {
            kept.add(&format!("m-{n}"));
        }
        kept.add("m-5");
        let seen = kept.read();
        assert_eq!((seen.recent.len(), seen.recent.front().map(String::as_str), seen.recent.back().map(String::as_str)), (RECENT, Some("m-2"), Some("m-1001")));
    }
}
