//! Feeds start runs (`on.feed(prices, (e) => …)`, a `feed_event` leaf; host-capabilities L4
//! §5.9.2). Every second a thread compares the feeds running deployments listen to with the
//! connections it holds, and opens or closes them: one a deployment and feed, opened anew when
//! the deployment's version or the feed's binding changes. Each has a reading thread, which
//! reads the feed's events (sse.rs, ws.rs) into a buffer of the binding's `--buffer`, and when it
//! is full waits, drops what comes, or replaces the waiting event of its key (`--on-full`, §5.9.4);
//! and a running thread, which runs the handler on each, one at a time, in the order they came,
//! `e` the event: `{data, id, event, missed, replaced, gap}`, `data` a value when it is JSON, else
//! its text, or a binary WebSocket message's bytes.
//!
//! - **Where it is** is kept beside the log (`feed-seen/<slug>/<feed>.json`): the id of the last
//!   event run on, which a reconnect sends as `Last-Event-ID`, and the last [`RECENT`] ids run on,
//!   so an event sent again is passed over (the timeline says so). It moves as the run's record
//!   does: a strict or an unrecorded run's at its end, a lazy run's once the sync covering its
//!   record is done (`Feeds::release`), kept in memory until then. With where the feed points: a
//!   binding pointed elsewhere starts anew.
//! - **Where a new listener starts:** at what the feed sends from now.
//! - **What the feed skipped** (`--sequence`, §5.9.6): a message's number, less the last read of
//!   its key, less one, is its `e.gap`, counted as it is read, before the buffer: one `skip`
//!   dropped or `latest` replaced carries its gap to the next of its key, as one `--only` passed
//!   over does. The last number of each key is kept with the position, so a gap across a reconnect
//!   or a restart is said.
//! - **A connection lost** is opened again after a second, then twice as long each time it fails,
//!   to a minute; longer when the feed asks (`retry:`). The timeline says so once, and again
//!   when the feed is read again.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Condvar;

use crate::feeds::{Binding, Event, Feeds, Incoming, OnFull};
use crate::*;

/// How often the thread compares what is listened to with what it holds.
const TICK: Duration = Duration::from_secs(1);

/// How many of the ids last run on are kept, to know an event sent again.
const RECENT: usize = 1000;

/// How many keys a feed's numbers are kept for (`--sequence`): past it, the least recently read is
/// forgotten, and its next message's gap is 0.
const KEYS: usize = 10_000;

/// The first wait before a lost connection is opened again, and the longest.
const FIRST_WAIT: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// Where a deployment is in one feed.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Clone)]
struct Seen {
    /// Where the feed points, as its binding shows it.
    at: String,
    /// The id of the last event run on, when the feed gives ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    position: Option<String>,
    /// The ids last run on, oldest first.
    #[serde(default)]
    recent: VecDeque<String>,
    /// The last number run on of each key (`--sequence`).
    #[serde(default, skip_serializing_if = "Numbers::is_empty")]
    numbers: Numbers,
}

/// The last number of each key of a feed that numbers its messages (`--sequence`), the least
/// recently set forgotten past [`KEYS`]; kept as a list, the least recent first.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
#[serde(from = "Vec<(String, u64)>", into = "Vec<(String, u64)>")]
struct Numbers {
    /// Each key's number, and when it was set.
    last: HashMap<String, (u64, u64)>,
    /// The keys, by when they were set.
    order: BTreeMap<u64, String>,
    at: u64,
}

impl Numbers {
    fn get(&self, key: &str) -> Option<u64> {
        self.last.get(key).map(|(n, _)| *n)
    }

    /// Set `key`'s number, the most recent: the key forgotten to make room, if one was.
    fn set(&mut self, key: &str, n: u64) -> Option<String> {
        self.at += 1;
        if let Some((_, was)) = self.last.insert(key.to_string(), (n, self.at)) {
            self.order.remove(&was);
        }
        self.order.insert(self.at, key.to_string());
        if self.last.len() <= KEYS {
            return None;
        }
        let (_, oldest) = self.order.pop_first()?;
        self.last.remove(&oldest);
        Some(oldest)
    }

    fn is_empty(&self) -> bool {
        self.last.is_empty()
    }

    /// Each key and its number, the least recently set first.
    fn listed(&self) -> Vec<(String, u64)> {
        self.order.values().map(|key| (key.clone(), self.last[key].0)).collect()
    }
}

impl PartialEq for Numbers {
    fn eq(&self, other: &Numbers) -> bool {
        self.listed() == other.listed()
    }
}

impl From<Vec<(String, u64)>> for Numbers {
    fn from(listed: Vec<(String, u64)>) -> Numbers {
        let mut numbers = Numbers::default();
        for (key, n) in listed {
            numbers.set(&key, n);
        }
        numbers
    }
}

impl From<Numbers> for Vec<(String, u64)> {
    fn from(numbers: Numbers) -> Vec<(String, u64)> {
        numbers.listed()
    }
}

/// What the reading thread counts a feed's numbers with (`--sequence`): the last number read of
/// each key, the gaps of messages `--only` passed over that the next of their key to run carries,
/// and the keys read since the connection opened.
#[derive(Default)]
struct Counting {
    last: Numbers,
    carried: HashMap<String, u64>,
    here: HashSet<String>,
}

impl Counting {
    /// A connection opened: the first message of each key on it may begin its count anew.
    fn opened(&mut self) {
        self.here.clear();
    }

    /// How many numbers were skipped before `n`, of `key`: its number, less the last read of its
    /// key, less one. 0 for a key's first, and for a number not above the last: the first of its
    /// key since the connection opened begins the count anew (a feed that numbers each connection
    /// on its own, or one that replays), and the key goes on from it; a later one was sent again.
    fn gap(&mut self, key: &str, n: u64) -> u64 {
        let first_here = self.here.insert(key.to_string());
        let gap = match self.last.get(key) {
            Some(last) if n > last => n - last - 1,
            Some(_) if !first_here => return 0,
            _ => 0,
        };
        if let Some(forgotten) = self.last.set(key, n) {
            self.carried.remove(&forgotten);
            self.here.remove(&forgotten);
        }
        gap
    }

    /// A message that runs: its gap, with those `--only` passed over of its key before it; and its
    /// own.
    fn runs(&mut self, key: &str, n: u64) -> (u64, u64) {
        let own = self.gap(key, n);
        (own + self.carried.remove(key).unwrap_or(0), own)
    }

    /// A message `--only` passed over: its number counts, and its gap is carried by the next of its
    /// key that runs.
    fn passed_over(&mut self, key: &str, n: u64) -> u64 {
        let gap = self.gap(key, n);
        if gap > 0 {
            *self.carried.entry(key.to_string()).or_default() += gap;
        }
        gap
    }
}

/// Where what `slug` has seen of its feeds is kept: none in memory.
fn seen_dir(deployments_path: &Place, slug: &str) -> Option<PathBuf> {
    deployments_path.beside("feed-seen").map(|dir| PathBuf::from(dir).join(slug))
}

/// Forget where `slug` is in its feeds (it is deleted), with where its lazy runs moved them.
pub(crate) fn forget(deployments_path: &Place, slug: &str, feeds: &Feeds) {
    feeds.forget_held(slug);
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

/// Where a deployment is in one feed as its runs moved it, kept across its connections: a lazy
/// run's move waits here for the sync that covers its record.
#[derive(Default)]
struct Made {
    /// Where its runs moved it, once read or moved.
    seen: Option<Seen>,
    /// How many moves were made, and the newest kept: a move held for a sync is kept only if
    /// none newer was.
    moves: u64,
    kept: Arc<Mutex<u64>>,
}

impl Made {
    /// Where the deployment is, as moved or else as kept.
    fn seen(&mut self, kept: &Kept) -> &mut Seen {
        self.seen.get_or_insert_with(|| kept.read())
    }

    /// A move to where it is now: what keeps it, unless a newer move was kept first.
    fn moved(&mut self, kept: &Kept) -> Box<dyn FnOnce() + Send> {
        self.moves += 1;
        let (at, newest, kept, seen) = (self.moves, Arc::clone(&self.kept), kept.clone(), self.seen(kept).clone());
        Box::new(move || {
            let mut newest = newest.lock().unwrap();
            if at > *newest {
                kept.keep(&seen);
                *newest = at;
            }
        })
    }

    /// Whether a move waits for its sync.
    fn holding(&self) -> bool {
        self.moves > *self.kept.lock().unwrap()
    }
}

/// The events read, waiting for their run.
#[derive(Default)]
struct Waiting {
    events: VecDeque<Event>,
    /// How many `skip` dropped since an event last found room: the next one's `e.missed`.
    missed: u64,
    /// The gaps of the events `skip` dropped, by key: the next of its key that finds room carries
    /// them (`--sequence`).
    gaps: HashMap<String, u64>,
}

/// What a connection's two threads share.
struct Shared {
    slug: String,
    feed: String,
    stop: AtomicBool,
    /// The open connection's socket, shut to close it at once.
    closer: Mutex<Option<sse::Closer>>,
    buffer: Mutex<Waiting>,
    changed: Condvar,
    capacity: usize,
    on_full: OnFull,
    /// Held while an event is run on: a connection opened anew waits for the old one's run, so
    /// where the feed is has moved past it when it looks.
    made: Arc<Mutex<Made>>,
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

    /// Put `event` in the buffer, as the binding says (`--on-full`): it waits while the buffer is
    /// full; or is dropped, counted in the next event's `missed`, its gap carried by the next of its
    /// key; or replaces the waiting event of its key, full or not, counted in its `replaced`, that
    /// one's gap added to its own. False when it was closed instead.
    fn push(&self, mut event: Event) -> bool {
        let mut buffer = self.buffer.lock().unwrap();
        match self.on_full {
            OnFull::Skip if buffer.events.len() >= self.capacity => {
                buffer.missed += 1;
                if let Some((key, _)) = event.number.as_ref().filter(|_| event.gap > 0) {
                    *buffer.gaps.entry(key.clone()).or_default() += event.gap;
                }
                drop(buffer);
                self.feeds.set_live(&self.slug, &self.feed, |l| l.missed += 1);
                return !self.stopped();
            }
            OnFull::Latest => {
                let waiting = event.key.as_ref().and_then(|key| buffer.events.iter_mut().find(|e| e.key.as_ref() == Some(key)));
                if let Some(waiting) = waiting {
                    event.replaced += waiting.replaced + 1;
                    event.gap += waiting.gap;
                    *waiting = event;
                    drop(buffer);
                    self.feeds.set_live(&self.slug, &self.feed, |l| l.replaced += 1);
                    return !self.stopped();
                }
            }
            _ => {}
        }
        let mut buffer = self.changed.wait_while(buffer, |b| b.events.len() >= self.capacity && !self.stopped()).unwrap();
        if self.stopped() {
            return false;
        }
        event.missed += std::mem::take(&mut buffer.missed);
        if let Some((key, _)) = &event.number {
            event.gap += buffer.gaps.remove(key).unwrap_or(0);
        }
        buffer.events.push_back(event);
        let waiting = buffer.events.len();
        drop(buffer);
        self.changed.notify_all();
        self.feeds.set_live(&self.slug, &self.feed, |l| l.waiting = waiting);
        true
    }

    /// The next event, waiting for one: None once it is closed (what waits is read again from
    /// where the feed is, by the connection opened next).
    fn pop(&self) -> Option<Event> {
        let mut buffer = self.changed.wait_while(self.buffer.lock().unwrap(), |b| b.events.is_empty() && !self.stopped()).unwrap();
        if self.stopped() {
            return None;
        }
        let event = buffer.events.pop_front();
        let waiting = buffer.events.len();
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
    /// Each deployment and feed's run lock, with where its runs moved it, kept across its
    /// connections.
    made: BTreeMap<(String, String), Arc<Mutex<Made>>>,
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
        if let Some(binding) = feeds.resolve(&key.0, &key.1).map(|b| b.seen_by(&files.all.databases, &key.0)) {
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
    // A lock a closed connection's run still holds is kept: a connection opened again waits for
    // it; and a move that waits for its sync, which a connection opened again goes on from.
    held.made.retain(|key, made| wanted.contains_key(key) || Arc::strong_count(made) > 1 || made.lock().unwrap().holding());
    for (key, (version, feeds)) in wanted {
        if held.conns.contains_key(&key) {
            continue;
        }
        let made = Arc::clone(held.made.entry(key.clone()).or_default());
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
            buffer: Mutex::default(),
            changed: Condvar::new(),
            capacity: version.1.buffer,
            on_full: version.1.on_full,
            made,
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
        *l = feeds::Live { address, status: "connecting", since: feeds::now_ms(), ..feeds::Live::default() };
    });
    let (reader, deps, b, k) = (Arc::clone(shared), Arc::clone(deployments), binding.clone(), kept.clone());
    thread::spawn(move || read(&reader, &deps, &b, &k));
    let (runner, deps, store, path) = (Arc::clone(shared), Arc::clone(deployments), store.clone(), deployments_path.clone());
    thread::spawn(move || run(&runner, &store, &deps, &path, &kept));
}

/// Where the deployment is in the feed `binding` points to, once a run of an earlier connection is
/// over, with the last number run on of each key: none when it was first listened to, or the
/// binding points elsewhere now (it starts at what the feed sends now).
fn position(shared: &Shared, binding: &Binding, kept: &Kept) -> (Option<String>, Numbers) {
    let mut made = shared.made.lock().unwrap();
    let seen = made.seen(kept);
    if seen.at == binding.address {
        return (seen.position.clone(), seen.numbers.clone());
    }
    *seen = Seen { at: binding.address.clone(), ..Seen::default() };
    made.moved(kept)();
    (None, Numbers::default())
}

/// The reading thread: the feed's events into the buffer, the connection opened again when it is
/// lost, until it is closed.
fn read(shared: &Shared, deployments: &Mutex<BTreeMap<String, Deployment>>, binding: &Binding, kept: &Kept) {
    let feed = &shared.feed;
    let (mut last_read, numbers) = position(shared, binding, kept);
    let mut counting = Counting { last: numbers, ..Counting::default() };
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
        } else if binding.channel.is_some() {
            shared.note(deployments, format!("listens to the feed {feed} again: what was sent meanwhile is lost"));
        } else if failing {
            shared.note(deployments, format!("reads the feed {feed} again"));
        }
        failing = false;
        counting.opened();
        shared.feeds.set_live(&shared.slug, feed, |l| (l.status, l.since, l.error) = ("connected", feeds::now_ms(), None));
        let lost = loop {
            match conn.next(binding) {
                Ok(Some(Incoming::Event(mut event))) => {
                    last_read = Some(event.last_id.clone()).filter(|id| !id.is_empty());
                    if let Some((key, n)) = &event.number {
                        let (gap, own) = counting.runs(key, *n);
                        event.gap = gap;
                        if own > 0 {
                            shared.feeds.set_live(&shared.slug, feed, |l| l.gaps += own);
                        }
                    }
                    if !shared.push(event) {
                        break None;
                    }
                }
                Ok(Some(Incoming::PassedOver)) => shared.note(deployments, format!("passed over an event of {feed} past {} MiB", sse::MAX_EVENT >> 20)),
                Ok(Some(Incoming::Filtered(number))) => {
                    let gap = number.map_or(0, |(key, n)| counting.passed_over(&key, n));
                    shared.feeds.set_live(&shared.slug, feed, |l| (l.passed_over, l.gaps) = (l.passed_over + 1, l.gaps + gap));
                }
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
        let mut made = shared.made.lock().unwrap();
        if shared.stopped() {
            break;
        }
        let ran_already = event.id.as_ref().filter(|id| !id.is_empty() && made.seen(kept).recent.contains(id));
        if let Some(id) = ran_already {
            // Kept with the next run's move: it runs on no record.
            shared.note(deployments, format!("passed over an event of {} it ran already ({id})", shared.feed));
            let seen = made.seen(kept);
            seen.position = Some(event.last_id.clone()).filter(|id| !id.is_empty()).or(seen.position.take());
            continue;
        }
        let Some(listening) = listening(store, deployments, &shared.slug, &shared.feed) else { break };
        let held_for = fire_one_feed(store, deployments, deployments_path, &listening, value_of(&event));
        let seen = made.seen(kept);
        if let Some(id) = event.id.as_ref().filter(|id| !id.is_empty()) {
            seen.recent.push_back(id.clone());
            while seen.recent.len() > RECENT {
                seen.recent.pop_front();
            }
        }
        seen.position = Some(event.last_id.clone()).filter(|id| !id.is_empty()).or(seen.position.take());
        if let Some((key, n)) = &event.number {
            seen.numbers.set(key, *n);
        }
        let position = seen.position.clone();
        let moved = made.moved(kept);
        match held_for {
            Some(seq) => shared.feeds.hold(&shared.slug, seq, moved),
            None => moved(),
        }
        shared.feeds.set_live(&shared.slug, &shared.feed, |l| l.position = position);
    }
}

/// `e`, as the handler reads it: `{data, id, event, missed, replaced, gap}`, `data` a value when it
/// is JSON, else its text, or a binary message's bytes; `id` only when the feed gave one.
fn value_of(event: &Event) -> Value {
    let mut e = BTreeMap::from([
        ("data".to_string(), event.data.clone()),
        ("event".to_string(), Value::Text(event.name.clone())),
        ("missed".to_string(), Value::Number(event.missed as f64)),
        ("replaced".to_string(), Value::Number(event.replaced as f64)),
        ("gap".to_string(), Value::Number(event.gap as f64)),
    ]);
    if let Some(id) = &event.id {
        e.insert("id".to_string(), Value::Text(id.clone()));
    }
    Value::Record(e)
}

/// A deployment listening to a feed: its slug, the handler, unit, state, permit, secrets and
/// folders.
type Listening = (String, holon_core::FeedHandler, Hash, Option<DeploymentState>, Arc<Permit>, Vault, Arc<folders::Scoped>);

/// `slug`'s handler of `feed` as it is now, if `slug` takes runs and listens to it.
fn listening(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, feed: &str) -> Option<Listening> {
    let deployments = deployments.lock().unwrap();
    let d = deployments.get(slug).filter(|d| d.takes_runs(store))?;
    let handler = d.server.as_ref()?.feeds.iter().find(|f| f.feed == feed)?.clone();
    Some((slug.to_string(), handler, d.unit, d.state.clone(), Arc::clone(&d.permit), Arc::clone(&d.secrets), Arc::clone(&d.folders)))
}

/// One run of `l`'s handler for `event`, logged under its label: the seq of its entry when its
/// record is lazy, which where it moved the feed waits for.
fn fire_one_feed(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, deployments_path: &Place, l: &Listening, event: Value) -> Option<u64> {
    let (slug, handler, unit_hash, state, permit, secrets, files) = l;
    let resolver = match closure(store, *unit_hash) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("feed event [{slug}]: store error while materializing closure: {e:?}");
            record_trigger(deployments, slug, true);
            return None;
        }
    };
    // The handler runs alone: its unit is the run's.
    let unit_hash = handler.handler;
    let label = handler.label.clone();
    let Some(root_unit) = resolver.resolve(&unit_hash) else {
        eprintln!("feed event [{slug}]: deployed unit no longer resolves in the store");
        record_trigger(deployments, slug, true);
        return None;
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
    let level = permit.log_level(Some(&label), declared);
    let unrecorded = level == holon_core::LogLevel::Unrecorded;
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
    let logged = log_run(deployments, slug, entry, Redactor::tokens(&waits.tokens).and(Redactor::handed(&calls::handed(&run.rounds))), declared);
    let held_for = logged.as_ref().and_then(|l| l.seq).filter(|_| level == holon_core::LogLevel::Lazy);
    if let Some(l) = logged {
        l.said(slug);
    }
    println!("feed event [{slug}] {label}: {} effect(s) performed", run.performed.len());
    record_trigger(deployments, slug, false);
    if parked_any {
        persist_deployments(deployments_path, deployments);
    }
    held_for
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

    fn event(data: Value, key: Option<&str>, id: Option<&str>) -> Event {
        Event { data, name: "price".into(), id: id.map(Into::into), last_id: id.unwrap_or_default().into(), key: key.map(Into::into), missed: 0, replaced: 0, number: None, gap: 0 }
    }

    fn field(v: &Value, k: &str) -> Option<Value> {
        match v {
            Value::Record(m) => m.get(k).cloned(),
            _ => None,
        }
    }

    #[test]
    fn an_event_is_its_data_its_name_its_own_id_and_what_its_buffer_dropped() {
        let price = holon_core::parse_json("{\"symbol\":\"ACME\",\"price\":10.5}").unwrap();
        let json = value_of(&Event { missed: 3, gap: 2, ..event(price, None, Some("41")) });
        assert_eq!(field(&field(&json, "data").unwrap(), "price"), Some(Value::Number(10.5)));
        assert_eq!(field(&json, "id"), Some(Value::Text("41".into())));
        assert_eq!(field(&json, "event"), Some(Value::Text("price".into())));
        assert_eq!((field(&json, "missed"), field(&json, "replaced"), field(&json, "gap")), (Some(Value::Number(3.0)), Some(Value::Number(0.0)), Some(Value::Number(2.0))));
        let text = value_of(&event(Value::Text("hello there".into()), None, None));
        assert_eq!(field(&text, "data"), Some(Value::Text("hello there".into())));
        assert_eq!(field(&text, "id"), None);
    }

    fn shared(on_full: OnFull, capacity: usize) -> Shared {
        Shared {
            slug: "app".into(),
            feed: "prices".into(),
            stop: AtomicBool::new(false),
            closer: Mutex::new(None),
            buffer: Mutex::default(),
            changed: Condvar::new(),
            capacity,
            on_full,
            made: Arc::default(),
            feeds: Feeds::memory(),
        }
    }

    fn waiting(s: &Shared) -> Vec<(Option<String>, u64, u64)> {
        s.buffer.lock().unwrap().events.iter().map(|e| (e.id.clone(), e.missed, e.replaced)).collect()
    }

    #[test]
    fn skip_drops_what_comes_while_full_and_the_next_event_says_how_many() {
        let s = shared(OnFull::Skip, 2);
        for id in ["1", "2", "3", "4"] {
            assert!(s.push(event(Value::Number(0.0), None, Some(id))));
        }
        assert_eq!(waiting(&s), vec![(Some("1".into()), 0, 0), (Some("2".into()), 0, 0)], "3 and 4 dropped");
        assert_eq!(s.pop().unwrap().id.as_deref(), Some("1"));
        assert!(s.push(event(Value::Number(0.0), None, Some("5"))));
        assert_eq!(waiting(&s), vec![(Some("2".into()), 0, 0), (Some("5".into()), 2, 0)]);
        assert_eq!(s.feeds.live("app", "prices").map(|l| (l.missed, l.waiting)), Some((2, 2)));
    }

    #[test]
    fn latest_replaces_the_waiting_event_of_its_key_in_its_place_full_or_not() {
        let s = shared(OnFull::Latest, 3);
        assert!(s.push(event(Value::Number(0.0), Some("BTC"), Some("b1"))));
        assert!(s.push(event(Value::Number(0.0), Some("ETH"), Some("e1"))));
        assert!(s.push(event(Value::Number(0.0), Some("BTC"), Some("b2"))), "not full: replaced still");
        assert!(s.push(event(Value::Number(0.0), Some("BTC"), Some("b3"))));
        assert!(s.push(event(Value::Number(0.0), None, Some("n1"))), "no key: it waits as it came");
        assert_eq!(waiting(&s), vec![(Some("b3".into()), 0, 2), (Some("e1".into()), 0, 0), (Some("n1".into()), 0, 0)]);
        assert!(s.push(event(Value::Number(0.0), Some("ETH"), Some("e2"))), "full: its key still replaces");
        assert_eq!(waiting(&s)[1], (Some("e2".into()), 0, 1));
        assert_eq!(s.feeds.live("app", "prices").map(|l| l.replaced), Some(3));
        // A new key finds the buffer full of others, and waits for room.
        let s = Arc::new(s);
        let pusher = {
            let s = Arc::clone(&s);
            thread::spawn(move || s.push(event(Value::Number(0.0), Some("SOL"), Some("s1"))))
        };
        thread::sleep(Duration::from_millis(100));
        assert_eq!(waiting(&s).len(), 3, "SOL waits");
        assert_eq!(s.pop().unwrap().id.as_deref(), Some("b3"));
        assert!(pusher.join().unwrap());
        assert_eq!(waiting(&s).last(), Some(&(Some("s1".into()), 0, 0)));
    }

    #[test]
    fn a_lazy_runs_move_is_kept_once_its_sync_is_done_and_never_over_a_newer_one() {
        let kept = Kept { file: None, key: Path::new("app").join("prices.json"), memory: Arc::default() };
        let feeds = Feeds::memory();
        let mut made = Made::default();
        // The last number of each key moves with the position.
        let at = |p: &str, n: u64| Seen { at: "wss://feed".into(), position: Some(p.into()), numbers: Numbers::from(vec![("BTC-USD".to_string(), n)]), ..Seen::default() };
        *made.seen(&kept) = at("1", 101);
        feeds.hold("app", 7, made.moved(&kept));
        assert!(made.holding());
        assert_eq!(kept.read(), Seen::default(), "not kept before its sync");
        feeds.release("app", 7);
        assert_eq!(kept.read(), Seen::default(), "a sync through 6 doesn't cover 7");
        feeds.release("app", 8);
        assert_eq!(kept.read(), at("1", 101));
        assert!(!made.holding());
        // A lazy move, then a strict one kept at once: the lazy one, released after, is older.
        made.seen(&kept).position = Some("2".into());
        made.seen(&kept).numbers.set("BTC-USD", 102);
        feeds.hold("app", 9, made.moved(&kept));
        made.seen(&kept).position = Some("3".into());
        made.seen(&kept).numbers.set("BTC-USD", 103);
        made.moved(&kept)();
        feeds.release("app", 10);
        assert_eq!(kept.read(), at("3", 103));
    }

    #[test]
    fn where_a_deployment_is_is_kept_in_memory_without_a_file() {
        let kept = Kept { file: None, key: Path::new("app").join("prices.json"), memory: Arc::default() };
        assert_eq!(kept.read(), Seen::default());
        let seen = Seen { at: "http://127.0.0.1:1/events".into(), position: Some("41".into()), recent: VecDeque::from(["41".to_string()]), numbers: Numbers::from(vec![("BTC-USD".to_string(), 41)]) };
        kept.keep(&seen);
        assert_eq!(kept.read(), seen);
    }

    #[test]
    fn a_gap_is_how_many_numbers_of_its_key_were_skipped_before_it() {
        let mut c = Counting::default();
        c.opened();
        // A key's first; the next; one skipped; several; each key on its own.
        assert_eq!(c.runs("BTC", 100), (0, 0));
        assert_eq!(c.runs("BTC", 101), (0, 0));
        assert_eq!(c.runs("BTC", 103), (1, 1));
        assert_eq!(c.runs("ETH", 7), (0, 0), "a key's first");
        assert_eq!(c.runs("BTC", 110), (6, 6));
        assert_eq!(c.runs("ETH", 8), (0, 0));
        // Not above the last, on the connection: sent again, and the key stays where it was.
        assert_eq!(c.runs("BTC", 105), (0, 0));
        assert_eq!(c.runs("BTC", 110), (0, 0));
        assert_eq!(c.runs("BTC", 111), (0, 0));
        // One --only passes over: its number counts, and its gap is carried by the next of its key
        // that runs.
        assert_eq!(c.passed_over("BTC", 113), 1);
        assert_eq!(c.passed_over("BTC", 114), 0);
        assert_eq!(c.runs("ETH", 9), (0, 0), "another key carries none of it");
        assert_eq!(c.runs("BTC", 117), (3, 2));
        assert_eq!(c.runs("BTC", 118), (0, 0), "carried once");
        // A connection opened again: the first of a key not above its last begins the count anew,
        // and the key goes on from it; one above it says what came meanwhile.
        c.opened();
        assert_eq!(c.runs("ETH", 1), (0, 0), "begun anew");
        assert_eq!(c.runs("ETH", 3), (1, 1));
        assert_eq!(c.runs("BTC", 120), (1, 1));
    }

    #[test]
    fn skip_and_latest_carry_the_gaps_of_what_they_drop_or_replace() {
        let numbered = |key: &str, n: u64, gap: u64| Event { number: Some((key.into(), n)), gap, ..event(Value::Number(0.0), Some(key), Some(format!("{key}/{n}").as_str())) };
        let gaps = |s: &Shared| s.buffer.lock().unwrap().events.iter().map(|e| (e.id.clone().unwrap_or_default(), e.missed, e.gap)).collect::<Vec<_>>();
        // A dropped event's gap goes to the next of its key that finds room, not to another key's.
        let s = shared(OnFull::Skip, 1);
        assert!(s.push(numbered("BTC", 1, 0)));
        assert!(s.push(numbered("BTC", 4, 2)), "dropped");
        assert!(s.push(numbered("ETH", 9, 1)), "dropped");
        s.pop();
        assert!(s.push(numbered("ETH", 10, 0)));
        assert_eq!(gaps(&s), [("ETH/10".to_string(), 2, 1)]);
        s.pop();
        assert!(s.push(numbered("BTC", 5, 0)));
        assert_eq!(gaps(&s), [("BTC/5".to_string(), 0, 2)]);
        // One that replaces another adds that one's gap to its own.
        let s = shared(OnFull::Latest, 3);
        assert!(s.push(numbered("BTC", 1, 0)));
        assert!(s.push(numbered("BTC", 4, 2)));
        assert!(s.push(numbered("BTC", 6, 1)));
        assert_eq!(gaps(&s), [("BTC/6".to_string(), 0, 3)]);
    }

    #[test]
    fn the_numbers_are_kept_by_key_the_least_recent_forgotten() {
        let mut numbers = Numbers::default();
        for k in 0..KEYS {
            assert_eq!(numbers.set(&format!("k{k}"), k as u64), None);
        }
        assert_eq!(numbers.set("k0", 7), None, "set again: the most recent");
        assert_eq!(numbers.set("new", 1), Some("k1".to_string()), "the least recently set forgotten");
        assert_eq!((numbers.get("k0"), numbers.get("k1"), numbers.get("new")), (Some(7), None, Some(1)));
        // Kept as a list, the least recent first, and read back as it was.
        let kept = Numbers::from(vec![("BTC-USD".to_string(), 104), ("ETH-USD".into(), 9)]);
        let json = serde_json::to_string(&kept).unwrap();
        assert_eq!(json, r#"[["BTC-USD",104],["ETH-USD",9]]"#);
        let back: Numbers = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kept);
        // A restart counts from them: what came while the host wasn't reading.
        let mut c = Counting { last: back, ..Counting::default() };
        c.opened();
        assert_eq!(c.runs("BTC-USD", 108), (3, 3));
        // Past the keys kept, a key the counting forgot begins anew.
        let mut c = Counting { last: numbers, ..Counting::default() };
        assert_eq!(c.runs("k1", 500), (0, 0));
        assert_eq!(c.last.get("k2"), None, "the next least recent made room for it");
    }
}
