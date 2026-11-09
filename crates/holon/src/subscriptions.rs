//! Feeds start runs (`on.feed(prices, (e) => …)`, a `feed_event` leaf; host-capabilities L4
//! §5.9.2). Every second a thread compares the feeds running deployments listen to with the
//! connections it holds, and opens or closes them: one a deployment and feed, opened anew when
//! the deployment's version or the feed's binding changes. Each has a reading thread, which
//! reads the feed's events (sse.rs) into a buffer of the binding's `--buffer`, and waits while it
//! is full; and a running thread, which runs the handler on each, one at a time, in the order they
//! came, `e` the event: `{data, id, event}`, `data` a value when it is JSON, else its text.
//!
//! - **Where it is** is kept beside the log (`feed-seen/<slug>/<feed>.json`): the id of the last
//!   event run on, which a reconnect sends as `Last-Event-ID`, and the last [`RECENT`] ids run on,
//!   so an event sent again is passed over (the timeline says so). It moves at the run's end.
//!   With where the feed points: a binding pointed elsewhere starts anew.
//! - **Where a new listener starts:** at what the feed sends from now.
//! - **A connection lost** is opened again after a second, then twice as long each time it fails,
//!   to a minute; longer when the feed asks (`retry:`). The timeline says so once, and again
//!   when the feed is read again.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Condvar;

use crate::feeds::{Binding, Feeds};
use crate::sse::{Event, Next};
use crate::*;

/// How often the thread compares what is listened to with what it holds.
const TICK: Duration = Duration::from_secs(1);

/// How many of the ids last run on are kept, to know an event sent again.
const RECENT: usize = 1000;

/// The first wait before a lost connection is opened again, and the longest.
const FIRST_WAIT: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// Where a deployment is in one feed.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
struct Seen {
    /// Where the feed points, as its binding shows it.
    at: String,
    /// The id of the last event run on, when the feed gives ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<String>,
    /// The ids last run on, oldest first.
    #[serde(default)]
    recent: VecDeque<String>,
}

/// Where what `slug` has seen of its feeds is kept: none in memory.
fn seen_dir(deployments_path: &Place, slug: &str) -> Option<PathBuf> {
    deployments_path.beside("feed-seen").map(|dir| PathBuf::from(dir).join(slug))
}

/// Forget where `slug` is in its feeds (it is deleted).
pub(crate) fn forget(deployments_path: &Place, slug: &str) {
    if let Some(dir) = seen_dir(deployments_path, slug) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Where one deployment's place in one feed is kept: a file, or the server's memory.
#[derive(Clone)]
struct Kept {
    file: Option<PathBuf>,
    key: PathBuf,
    memory: Arc<Mutex<BTreeMap<PathBuf, Vec<u8>>>>,
}

impl Kept {
    fn read(&self) -> Seen {
        let bytes = match &self.file {
            Some(f) => std::fs::read(f).ok(),
            None => self.memory.lock().unwrap().get(&self.key).cloned(),
        };
        bytes.and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn keep(&self, seen: &Seen) {
        let bytes = serde_json::to_vec(seen).unwrap_or_default();
        let Some(file) = &self.file else {
            self.memory.lock().unwrap().insert(self.key.clone(), bytes);
            return;
        };
        let written = std::fs::create_dir_all(file.parent().expect("a file in a folder")).and_then(|_| write_atomically(file, &bytes));
        if let Err(e) = written {
            eprintln!("holon: where {} is in its feed wasn't kept: {e}", file.display());
        }
    }
}

/// What a connection's two threads share.
struct Shared {
    slug: String,
    feed: String,
    stop: AtomicBool,
    /// The open connection's socket, shut to close it at once.
    closer: Mutex<Option<sse::Closer>>,
    /// The events read, waiting for their run.
    buffer: Mutex<VecDeque<Event>>,
    changed: Condvar,
    capacity: usize,
    /// Held while an event is run on: a connection opened anew waits for the old one's run, so
    /// where the feed is has moved past it when it looks.
    running: Arc<Mutex<()>>,
    feeds: Feeds,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(c) = self.closer.lock().unwrap().take() {
            c.close();
        }
        self.changed.notify_all();
    }

    /// Wait `wait`, or until it is closed: whether it was.
    fn sleep(&self, wait: Duration) -> bool {
        let buffer = self.buffer.lock().unwrap();
        let _ = self.changed.wait_timeout_while(buffer, wait, |_| !self.stopped()).unwrap();
        self.stopped()
    }

    /// Put `event` in the buffer, waiting while it is full: false when it was closed instead.
    fn push(&self, event: Event) -> bool {
        let mut buffer = self.changed.wait_while(self.buffer.lock().unwrap(), |b| b.len() >= self.capacity && !self.stopped()).unwrap();
        if self.stopped() {
            return false;
        }
        buffer.push_back(event);
        let waiting = buffer.len();
        drop(buffer);
        self.changed.notify_all();
        self.feeds.set_live(&self.slug, &self.feed, |l| l.waiting = waiting);
        true
    }

    /// The next event, waiting for one: None once it is closed (what waits is read again from
    /// where the feed is, by the connection opened next).
    fn pop(&self) -> Option<Event> {
        let mut buffer = self.changed.wait_while(self.buffer.lock().unwrap(), |b| b.is_empty() && !self.stopped()).unwrap();
        if self.stopped() {
            return None;
        }
        let event = buffer.pop_front();
        let waiting = buffer.len();
        drop(buffer);
        self.changed.notify_all();
        self.feeds.set_live(&self.slug, &self.feed, |l| l.waiting = waiting);
        event
    }

    fn note(&self, deployments: &Mutex<BTreeMap<String, Deployment>>, what: String) {
        let detail = crate::events::Detail { name: Some(self.feed.clone()), ..Default::default() };
        crate::events::note(deployments, &self.slug, "feed", what, None, detail);
    }
}

/// One deployment's connection to one feed: what it was opened for (the deployment's unit, the
/// binding), and its threads' shared state.
struct Conn {
    version: (Hash, Binding),
    shared: Arc<Shared>,
}

/// The connections the thread holds.
#[derive(Default)]
pub(crate) struct Held {
    conns: BTreeMap<(String, String), Conn>,
    /// Each deployment and feed's run lock, kept across its connections.
    running: BTreeMap<(String, String), Arc<Mutex<()>>>,
    /// Where each is, for a server whose deployments are kept nowhere (`:memory:`).
    memory: Arc<Mutex<BTreeMap<PathBuf, Vec<u8>>>>,
}

impl Drop for Held {
    fn drop(&mut self) {
        for conn in self.conns.values() {
            conn.shared.close();
        }
    }
}

/// Open the connections running deployments' feed handlers want and aren't held, and close those
/// held that aren't wanted, or are wanted otherwise.
pub(crate) fn reconcile(store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: &Place, held: &mut Held) {
    let listening: Vec<((String, String), Hash, Arc<folders::Scoped>)> = deployments
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, d)| d.life.status == Status::Running)
        .flat_map(|(slug, d)| {
            let handlers = d.server.as_ref().map(|s| s.feeds.clone()).unwrap_or_default();
            handlers.into_iter().map(|h| ((slug.clone(), h.feed), d.unit, Arc::clone(&d.folders)))
        })
        .collect();
    let mut wanted: BTreeMap<(String, String), ((Hash, Binding), Feeds)> = BTreeMap::new();
    for (key, unit, files) in listening {
        let feeds = files.all.feeds.clone();
        if let Some(binding) = feeds.resolve(&key.0, &key.1) {
            wanted.insert(key, ((unit, binding), feeds));
        }
    }
    held.conns.retain(|key, conn| {
        let keep = wanted.get(key).is_some_and(|(version, _)| *version == conn.version);
        if !keep {
            conn.shared.close();
            conn.shared.feeds.forget_live(&key.0, &key.1);
        }
        keep
    });
    // A lock a closed connection's run still holds is kept: a connection opened again waits for it.
    held.running.retain(|key, lock| wanted.contains_key(key) || Arc::strong_count(lock) > 1);
    for (key, (version, feeds)) in wanted {
        if held.conns.contains_key(&key) {
            continue;
        }
        let running = Arc::clone(held.running.entry(key.clone()).or_default());
        let kept = Kept {
            file: seen_dir(deployments_path, &key.0).map(|d| d.join(format!("{}.json", key.1))),
            key: PathBuf::from(&key.0).join(format!("{}.json", key.1)),
            memory: Arc::clone(&held.memory),
        };
        let shared = Arc::new(Shared {
            slug: key.0.clone(),
            feed: key.1.clone(),
            stop: AtomicBool::new(false),
            closer: Mutex::new(None),
            buffer: Mutex::new(VecDeque::new()),
            changed: Condvar::new(),
            capacity: version.1.buffer,
            running,
            feeds,
        });
        open(store, deployments, deployments_path, &shared, &version.1, kept);
        held.conns.insert(key, Conn { version, shared });
    }
}

/// Start `shared`'s reading and running threads.
fn open(store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, deployments_path: &Place, shared: &Arc<Shared>, binding: &Binding, kept: Kept) {
    let address = binding.address.clone();
    shared.feeds.set_live(&shared.slug, &shared.feed, |l| {
        *l = feeds::Live { address, status: "connecting", since: feeds::now_ms(), error: None, position: None, waiting: 0 };
    });
    let (reader, deps, b, k) = (Arc::clone(shared), Arc::clone(deployments), binding.clone(), kept.clone());
    thread::spawn(move || read(&reader, &deps, &b, &k));
    let (runner, deps, store, path) = (Arc::clone(shared), Arc::clone(deployments), store.clone(), deployments_path.clone());
    thread::spawn(move || run(&runner, &store, &deps, &path, &kept));
}

/// Where the deployment is in the feed `binding` points to, once a run of an earlier connection is
/// over: none when it was first listened to, or the binding points elsewhere now (it starts at
/// what the feed sends now).
fn position(shared: &Shared, binding: &Binding, kept: &Kept) -> Option<String> {
    let _running = shared.running.lock().unwrap();
    let seen = kept.read();
    if seen.at == binding.address {
        return seen.position;
    }
    kept.keep(&Seen { at: binding.address.clone(), position: None, recent: VecDeque::new() });
    None
}

/// The reading thread: the feed's events into the buffer, the connection opened again when it is
/// lost, until it is closed.
fn read(shared: &Shared, deployments: &Mutex<BTreeMap<String, Deployment>>, binding: &Binding, kept: &Kept) {
    let feed = &shared.feed;
    let mut last_read = position(shared, binding, kept);
    shared.feeds.set_live(&shared.slug, feed, |l| l.position = last_read.clone());
    let (mut wait, mut failing, mut listened) = (FIRST_WAIT, false, false);
    while !shared.stopped() {
        let mut conn = match shared.feeds.connect(binding, last_read.as_deref()) {
            Ok(c) => c,
            Err(why) => {
                if !failing {
                    shared.note(deployments, format!("lost the feed {feed}: {why}; trying again in {}", folders::span_text(wait.as_secs())));
                    failing = true;
                }
                shared.feeds.set_live(&shared.slug, feed, |l| {
                    if l.status != "reconnecting" {
                        (l.status, l.since) = ("reconnecting", feeds::now_ms());
                    }
                    l.error = Some(why);
                });
                if shared.sleep(wait) {
                    break;
                }
                wait = (wait * 2).min(LONGEST_WAIT);
                continue;
            }
        };
        *shared.closer.lock().unwrap() = conn.closer();
        if shared.stopped() {
            break;
        }
        if !listened {
            shared.note(deployments, format!("listens to the feed {feed} ({})", binding.address));
            listened = true;
        } else if failing {
            shared.note(deployments, format!("reads the feed {feed} again"));
        }
        failing = false;
        shared.feeds.set_live(&shared.slug, feed, |l| (l.status, l.since, l.error) = ("connected", feeds::now_ms(), None));
        let lost = loop {
            match conn.next_event() {
                Ok(Some(Next::Event(event))) => {
                    last_read = Some(event.last_id.clone()).filter(|id| !id.is_empty());
                    if !shared.push(event) {
                        break None;
                    }
                }
                Ok(Some(Next::PassedOver)) => shared.note(deployments, format!("passed over an event of {feed} past {} MiB", sse::MAX_EVENT >> 20)),
                Ok(None) => break Some("the feed ended the connection".to_string()),
                Err(why) => break Some(why),
            }
        };
        let Some(why) = lost.filter(|_| !shared.stopped()) else { break };
        // A feed that asks to be waited for longer is.
        wait = conn.retry().map_or(FIRST_WAIT, |r| r.clamp(FIRST_WAIT, LONGEST_WAIT));
        shared.feeds.set_live(&shared.slug, feed, |l| (l.status, l.since, l.error) = ("reconnecting", feeds::now_ms(), Some(why)));
        if shared.sleep(wait) {
            break;
        }
    }
}

/// The running thread: a run of the handler on each event, one at a time, until it is closed.
fn run(shared: &Shared, store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &Place, kept: &Kept) {
    while let Some(event) = shared.pop() {
        let _running = shared.running.lock().unwrap();
        if shared.stopped() {
            break;
        }
        let mut seen = kept.read();
        let position = Some(event.last_id.clone()).filter(|id| !id.is_empty());
        if let Some(id) = event.id.as_ref().filter(|id| !id.is_empty() && seen.recent.contains(id)) {
            shared.note(deployments, format!("passed over an event of {} it ran already ({id})", shared.feed));
        } else {
            let Some(listening) = listening(deployments, &shared.slug, &shared.feed) else { break };
            fire_one_feed(store, deployments, deployments_path, &listening, value_of(&event));
            if let Some(id) = event.id.as_ref().filter(|id| !id.is_empty()) {
                seen.recent.push_back(id.clone());
                while seen.recent.len() > RECENT {
                    seen.recent.pop_front();
                }
            }
        }
        if position.is_some() {
            seen.position = position.clone();
        }
        kept.keep(&seen);
        shared.feeds.set_live(&shared.slug, &shared.feed, |l| l.position = seen.position.clone());
    }
}

/// `e`, as the handler reads it: `{data, id, event}`, `data` a value when it is JSON, else its
/// text, `id` only when the feed gave one.
fn value_of(event: &Event) -> Value {
    let data = holon_core::parse_json(&event.data).unwrap_or_else(|| Value::Text(event.data.clone()));
    let mut e = BTreeMap::from([("data".to_string(), data), ("event".to_string(), Value::Text(event.name.clone()))]);
    if let Some(id) = &event.id {
        e.insert("id".to_string(), Value::Text(id.clone()));
    }
    Value::Record(e)
}

/// A deployment listening to a feed: its slug, the handler, unit, state, permit, secrets and
/// folders.
type Listening = (String, holon_core::FeedHandler, Hash, Option<DeploymentState>, Arc<Permit>, Vault, Arc<folders::Scoped>);

/// `slug`'s handler of `feed` as it is now, if `slug` runs and listens to it.
fn listening(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, feed: &str) -> Option<Listening> {
    let deployments = deployments.lock().unwrap();
    let d = deployments.get(slug).filter(|d| d.life.status == Status::Running)?;
    let handler = d.server.as_ref()?.feeds.iter().find(|f| f.feed == feed)?.clone();
    Some((slug.to_string(), handler, d.unit, d.state.clone(), Arc::clone(&d.permit), Arc::clone(&d.secrets), Arc::clone(&d.folders)))
}

/// One run of `l`'s handler for `event`, logged under its label.
fn fire_one_feed(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &Place, l: &Listening, event: Value) {
    let (slug, handler, unit_hash, state, permit, secrets, files) = l;
    let resolver = match closure(store, *unit_hash) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("feed event [{slug}]: store error while materializing closure: {e:?}");
            record_trigger(deployments, slug, true);
            return;
        }
    };
    // The handler runs alone: its unit is the run's.
    let unit_hash = handler.handler;
    let label = handler.label.clone();
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        eprintln!("feed event [{slug}]: deployed unit no longer resolves in the store");
        record_trigger(deployments, slug, true);
        return;
    };
    let epoch_seconds = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let extra_sourced = [
        (Capability::FeedEvent(handler.feed.clone()), event),
        (Capability::Clock, Value::Number(epoch_seconds as f64)),
        (Capability::Random, Value::Bytes(rand::random::<[u8; 32]>().to_vec())),
    ];
    let started = started_now();
    let channels = deployments.lock().unwrap().get(slug.as_str()).map(|d| Arc::clone(&d.channels));
    let declared = handler.log;
    let unrecorded = permit.log_level(Some(&label), declared) == holon_core::LogLevel::Unrecorded;
    let mut waits = RunWaits { folders: Some(Arc::clone(files)), channels, slug: Some(slug.to_string()), unrecorded, ..Default::default() };
    let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
    let run = fire_with_state(root_unit, &[], &resolver, store, state.as_ref(), &extra_sourced, permit, true, secrets, &mut waits, None);
    let mut entry = run_entry("feed", started, &extra_sourced, &run);
    entry.route = Some(label.clone());
    match durable::park_if_waiting(deployments, store, slug, &unit_hash, "feed", Some(label.clone()), declared, &[], &extra_sourced, &run, &waits) {
        Ok(names) => entry.parked = names,
        Err(e) => entry.error = Some(e),
    }
    let parked_any = !entry.parked.is_empty();
    if let Some(l) = log_run(deployments, slug, entry, Redactor::tokens(&waits.tokens).and(Redactor::handed(&calls::handed(&run.rounds))), declared) {
        l.said(slug);
    }
    println!("feed event [{slug}] {label}: {} effect(s) performed", run.performed.len());
    record_trigger(deployments, slug, false);
    if parked_any {
        persist_deployments(deployments_path, deployments);
    }
}

/// The thread that holds the feeds' connections, comparing them with what is listened to every
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
    fn an_event_is_its_data_as_a_value_its_name_and_its_own_id() {
        let e = |data: &str, id: Option<&str>| value_of(&Event { data: data.into(), name: "price".into(), id: id.map(Into::into), last_id: "9".into() });
        let field = |v: &Value, k: &str| match v {
            Value::Record(m) => m.get(k).cloned(),
            _ => None,
        };
        let json = e("{\"symbol\":\"ACME\",\"price\":10.5}", Some("41"));
        assert_eq!(field(&field(&json, "data").unwrap(), "price"), Some(Value::Number(10.5)));
        assert_eq!(field(&json, "id"), Some(Value::Text("41".into())));
        assert_eq!(field(&json, "event"), Some(Value::Text("price".into())));
        let text = e("hello there", None);
        assert_eq!(field(&text, "data"), Some(Value::Text("hello there".into())));
        assert_eq!(field(&text, "id"), None);
    }

    #[test]
    fn where_a_deployment_is_is_kept_in_memory_without_a_file() {
        let kept = Kept { file: None, key: Path::new("app").join("prices.json"), memory: Arc::default() };
        assert_eq!(kept.read(), Seen::default());
        let seen = Seen { at: "http://127.0.0.1:1/events".into(), position: Some("41".into()), recent: VecDeque::from(["41".to_string()]) };
        kept.keep(&seen);
        assert_eq!(kept.read(), seen);
    }
}
