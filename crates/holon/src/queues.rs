//! Queues (host-capabilities L4 §5.9.9): what a flow sends to with `payouts.send({ data, group })`
//! and listens to with `on.queue(payouts, (m) => …)`, through a queue an admin binds.
//!
//! - **Bound by an admin,** as a feed is (`--queues`, owner-only; `/admin/queues`; `holon queue`):
//!   a name a flow imports (`import payouts from "queue:payouts"`) bound to an Amazon SQS queue, or
//!   a service that speaks its API (ElasticMQ, LocalStack), its URL with its access key read from a
//!   source (`env:…`, `vault:…`) each time and never kept (sqs.rs); how long a message received is
//!   kept out of sight while it waits or runs (`--visibility`), and whether a failed run's message
//!   is left for the queue's redrive (`--keep-failed`). For the whole server or one deployment.
//! - **Checked when bound:** `GetQueueAttributes`, signed, so a wrong key, a wrong region or a queue
//!   that isn't there is refused with what the service said. Whether it is FIFO and its largest
//!   message are kept with the binding, with where it points without its login or query.
//! - **The grants** name the queue: `QueueSend("payouts")` and `QueueMessage("payouts")`. A deploy
//!   is refused while a queue it uses isn't bound, or another deployment listens to one it listens
//!   to ([`Queues::refuses`]): a message is deleted once, so one deployment listens to a queue.
//! - **Sent after the run,** as a mail is: spooled (beside the bindings file, a file a message,
//!   owner-only) before its run's record is written, held until that record is on disk, then sent
//!   in order, a thread for each binding, so a slow queue holds no run. A temporary failure (no
//!   connection, a 5xx, a throttle) is tried again after 1 s, 10 s, a minute, then every 5
//!   minutes, for a day; any other gives up. A FIFO queue's message goes in its `group`, else in a
//!   group named for the deployment, its deduplication id its run's and its place in the run, so
//!   one tried again after a lost answer is one message. A restart sends what the spool holds
//!   whose run its log holds. The first failure of a message, a message given up and a queue sent
//!   to again are on the deployment's timeline; the sends themselves are counted in its state.
//! - **What each listener is doing** (dequeue.rs keeps it here), and what each queue was sent, is
//!   in `GET /deployments/<slug>/queues`.

use std::collections::VecDeque;
use std::sync::{Condvar, OnceLock, RwLock};
use std::time::{Duration, Instant};

use rouille::{Request, Response};

use crate::http::{err, read_body};
use crate::permissions::Manifest;
use crate::secret_sources::{Reference, Sources};
use crate::smtp::Failure;

use crate::*;

/// What the error of a run whose messages a queue can't take starts with.
pub(crate) const SEND_FAILED: &str = "message not sent: ";

/// How long a message received is kept out of sight, unless the binding says, in seconds.
pub(crate) const VISIBILITY: u64 = 300;

/// The least and the most a binding keeps a message out of sight: SQS's own bounds are 0 and 12
/// hours, and less than 30 s leaves no time for a run and its record.
const VISIBILITY_LEAST: u64 = 30;
const VISIBILITY_MOST: u64 = 12 * 3600;

/// The most messages one run sends.
pub(crate) const MAX_SENDS: usize = 100;

/// The largest message a queue takes, when it doesn't say: SQS's default.
const LARGEST: u64 = 256 * 1024;

/// One bound queue.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Binding {
    /// Where the host reads the queue's URL with its key from (`env:…`, `vault:…`), never the URL
    /// itself.
    pub(crate) url: String,
    /// Where that URL points, without its login or query
    /// (`https://sqs.eu-west-1.amazonaws.com/123456789012/payouts`).
    pub(crate) address: String,
    /// Whether it is FIFO, as it said when it was bound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) fifo: bool,
    /// Its largest message, in bytes, as it said when it was bound.
    pub(crate) largest: u64,
    /// How long a message received is kept out of sight while it waits or runs, in seconds.
    pub(crate) visibility: u64,
    /// Whether a failed run's message is left for the queue to hand out again, for its redrive
    /// policy to count, rather than deleted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) keep_failed: bool,
}

/// What binding a queue asks for, as `/admin/queues` takes it.
#[derive(Deserialize, Default, Debug)]
pub(crate) struct Asked {
    #[serde(default)]
    pub(crate) url: Option<String>,
    /// `5m`, `90s`.
    #[serde(default)]
    pub(crate) visibility: Option<String>,
    #[serde(default)]
    pub(crate) keep_failed: bool,
}

/// Every binding, as [`folders::Bound`]: the server's by name, each deployment's under
/// `@deployments`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Bound {
    #[serde(rename = "@deployments", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) deployments: BTreeMap<String, BTreeMap<String, Binding>>,
    #[serde(flatten)]
    pub(crate) server: BTreeMap<String, Binding>,
}

/// What one deployment's listener to one queue is doing.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub(crate) struct Live {
    /// Where the queue points, as its binding shows it.
    pub(crate) address: String,
    /// `receiving` or `unreachable`.
    pub(crate) status: &'static str,
    /// Since when, in milliseconds since the epoch.
    pub(crate) since: u64,
    /// Why it can't receive, when it can't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// How many messages received wait for their run.
    pub(crate) waiting: usize,
    /// Since the listener started: how many messages ran, were passed over (run already), and
    /// were left for the queue's redrive (`--keep-failed`).
    pub(crate) run: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) passed_over: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) kept: u64,
}

/// What one deployment's messages to one queue came to, since the server started.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub(crate) struct Sent {
    pub(crate) sent: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) gave_up: u64,
    /// Whether the last try failed, and the queue hasn't taken one since.
    #[serde(skip)]
    failing: bool,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// What runs once a run's record is on disk: a message queued, a message deleted.
pub(crate) type Landed = Box<dyn FnOnce() + Send>;

/// The queues bound on this server, the messages on their way to them, and what each listener is
/// doing.
#[derive(Clone)]
pub(crate) struct Queues(Arc<Inner>);

struct Inner {
    file: Option<PathBuf>,
    bound: RwLock<Bound>,
    /// What a binding's URL is read from: the server's environment and dotenv file, Vault.
    sources: Sources,
    /// Where messages on their way are kept, a file each, until sent or given up; none: in memory
    /// only.
    spool: Option<PathBuf>,
    /// What waits for its run's record to be on disk, by deployment, in its log's order (the
    /// entry's seq): a logged run's messages, a lazy run's delete.
    held: Mutex<BTreeMap<String, VecDeque<(u64, Landed)>>>,
    /// The messages to send, by binding (`payouts`, or `app/payouts` for a deployment's own).
    outboxes: Mutex<BTreeMap<String, Arc<Outbox>>>,
    /// The deployments, whose timelines each outcome is noted on, and whose listeners a deploy is
    /// checked against: set once the server has them, when the outboxes start sending.
    deployments: OnceLock<Arc<Mutex<BTreeMap<String, Deployment>>>>,
    /// How long it waits to try a message again after its first, second, third and later
    /// temporary failures, and how long after its run it gives up.
    retry: ([Duration; 4], Duration),
    /// Each listener's state, by deployment and queue.
    live: Mutex<BTreeMap<(String, String), Live>>,
    /// What each deployment's messages to each queue came to.
    sent: Mutex<BTreeMap<(String, String), Sent>>,
}

impl std::fmt::Debug for Queues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queues").field("bound", &self.0.bound.read().unwrap()).finish()
    }
}

/// A binding's outbox, and its thread's signal that it holds a message.
#[derive(Default)]
struct Outbox {
    items: Mutex<VecDeque<Queued>>,
    ready: Condvar,
}

/// A message on its way: where it came from and goes, as its spool file's first line keeps it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Spooled {
    slug: String,
    queue: String,
    run: String,
    /// Its place among its run's messages.
    n: usize,
    /// When its run began, in milliseconds since the epoch: a day after, it is given up.
    at_ms: u64,
    /// Its FIFO group, when the run named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    group: Option<String>,
    /// Its run isn't logged (an unrecorded handler's): a restart sends it all the same.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    unlogged: bool,
}

pub(crate) struct Queued {
    /// The outbox it goes to: its binding's (`payouts`, or `app/payouts` for a deployment's own).
    key: String,
    spooled: Spooled,
    /// The message's body, as it is sent.
    body: String,
    /// Its spool file, removed once it is sent or given up.
    file: Option<PathBuf>,
    /// The temporary failures it has had.
    tries: u32,
    not_before: Instant,
}

fn given(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

impl Queues {
    /// None, kept nowhere: the tests, and a server told `--queues :memory:`.
    pub(crate) fn memory() -> Queues {
        Queues::with(None, Bound::default(), None)
    }

    fn with(file: Option<PathBuf>, bound: Bound, spool: Option<PathBuf>) -> Queues {
        let retry = ([1, 10, 60, 300].map(Duration::from_secs), Duration::from_secs(24 * 3600));
        Queues(Arc::new(Inner {
            file,
            bound: RwLock::new(bound),
            sources: Sources::none(),
            spool,
            held: Mutex::default(),
            outboxes: Mutex::default(),
            deployments: OnceLock::new(),
            retry,
            live: Mutex::default(),
            sent: Mutex::default(),
        }))
    }

    /// The queues bound in `file` (none yet if it doesn't exist), the spool beside it.
    pub(crate) fn open(file: &str) -> Result<Queues, String> {
        if file == ":memory:" {
            return Ok(Queues::memory());
        }
        let path = PathBuf::from(file);
        let bound = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{file} isn't a queues file: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Bound::default(),
            Err(e) => return Err(format!("can't read {file}: {e}")),
        };
        let spool = path.with_file_name("queue-spool");
        Ok(Queues::with(Some(path), bound, Some(spool)))
    }

    /// These, reading a binding's URL from `sources`.
    pub(crate) fn with_sources(self, sources: Sources) -> Queues {
        let inner = Arc::into_inner(self.0).expect("set up before it is shared");
        Queues(Arc::new(Inner { sources, ..inner }))
    }

    /// These, trying again after `delays` and giving up after `give_up`: the tests'.
    #[cfg(test)]
    pub(crate) fn with_retry(self, delays: [Duration; 4], give_up: Duration) -> Queues {
        let inner = Arc::into_inner(self.0).expect("set up before it is shared");
        Queues(Arc::new(Inner { retry: (delays, give_up), ..inner }))
    }

    /// Every binding: the server's, and each deployment's.
    pub(crate) fn all(&self) -> Bound {
        self.0.bound.read().unwrap().clone()
    }

    /// The binding deployment `deployment` sees as `name`, its own or the server's, and its
    /// outbox's key.
    fn resolve_keyed(&self, deployment: &str, name: &str) -> Option<(String, Binding)> {
        let bound = self.0.bound.read().unwrap();
        match bound.deployments.get(deployment).and_then(|d| d.get(name)) {
            Some(b) => Some((format!("{deployment}/{name}"), b.clone())),
            None => bound.server.get(name).map(|b| (name.to_string(), b.clone())),
        }
    }

    /// The binding deployment `deployment` sees as `name`: its own, else the server's.
    pub(crate) fn resolve(&self, deployment: &str, name: &str) -> Option<Binding> {
        self.resolve_keyed(deployment, name).map(|(_, b)| b)
    }

    /// Bind `name`, for the whole server or one deployment, replacing any binding of it there
    /// before, as `asked` says: once the queue answers `GetQueueAttributes`, signed with its key.
    pub(crate) fn bind(&self, name: &str, asked: &Asked, for_deployment: Option<&str>) -> Result<Binding, String> {
        if !folders::is_folder_name(name) || name.contains('-') {
            return Err(format!("{name:?} isn't a queue's name: letters, digits and _, up to 64, as a flow imports it"));
        }
        if let Some(slug) = for_deployment.filter(|s| !folders::is_folder_name(s)) {
            return Err(format!("{slug:?} isn't a deployment's name"));
        }
        let url = given(&asked.url).ok_or("a queue is reached at the URL --url names: it names none")?;
        if !url.starts_with("env:") && !url.starts_with("vault:") {
            return Err("a queue's URL is read from a source, env:HOLON_SECRET_… or vault:…, holding https://<key id>:<secret>@sqs.<region>.amazonaws.com/<account>/<queue>".into());
        }
        let visibility = match given(&asked.visibility) {
            None => VISIBILITY,
            Some(v) => folders::seconds_of(v).ok_or_else(|| format!("{v:?} isn't how long: 90s, 5m, 1h"))?,
        };
        if !(VISIBILITY_LEAST..=VISIBILITY_MOST).contains(&visibility) {
            return Err(format!("a message is kept out of sight for {} to {}", folders::span_text(VISIBILITY_LEAST), folders::span_text(VISIBILITY_MOST)));
        }
        let queue = self.client(url)?;
        let address = queue.address();
        let attributes = queue.attributes().map_err(|f| format!("{address} wasn't reached: {}", f.why()))?;
        let binding = Binding { url: url.to_string(), address, fifo: attributes.fifo, largest: attributes.largest, visibility, keep_failed: asked.keep_failed };
        let mut bound = self.0.bound.write().unwrap();
        let mut next = bound.clone();
        match for_deployment {
            Some(slug) => next.deployments.entry(slug.to_string()).or_default().insert(name.to_string(), binding.clone()),
            None => next.server.insert(name.to_string(), binding.clone()),
        };
        self.save(&next)?;
        *bound = next;
        Ok(binding)
    }

    /// The queue the URL `source` holds names, its URL read now.
    pub(crate) fn client(&self, source: &str) -> Result<sqs::Queue, String> {
        let url = self.0.sources.fetch(&Reference::parse(source)?)?;
        sqs::Queue::parse(&url)
    }

    /// Unbind `name`, the server's or one deployment's: whether it was bound. Its listener stops
    /// within a second; its messages on the way are given up as they come to be sent.
    pub(crate) fn unbind(&self, name: &str, for_deployment: Option<&str>) -> Result<bool, String> {
        let mut bound = self.0.bound.write().unwrap();
        let mut next = bound.clone();
        let removed = match for_deployment {
            Some(slug) => {
                let removed = next.deployments.get_mut(slug).and_then(|d| d.remove(name)).is_some();
                if next.deployments.get(slug).is_some_and(BTreeMap::is_empty) {
                    next.deployments.remove(slug);
                }
                removed
            }
            None => next.server.remove(name).is_some(),
        };
        if !removed {
            return Ok(false);
        }
        self.save(&next)?;
        *bound = next;
        Ok(true)
    }

    fn save(&self, bound: &Bound) -> Result<(), String> {
        let Some(file) = &self.0.file else { return Ok(()) };
        let bytes = serde_json::to_vec_pretty(bound).map_err(|e| e.to_string())?;
        crate::sealing::write_private(file, &bytes).map_err(|e| format!("can't write {}: {e}", file.display()))
    }

    /// Why a deployment asking for `manifest` can't run here, if it can't: a queue it sends to or
    /// listens to that isn't bound, or one it listens to that another deployment listens to.
    pub(crate) fn refuses(&self, deployment: &str, manifest: &Manifest) -> Option<String> {
        for item in manifest.capabilities.iter().filter(|c| !manifest.requirements.optional.contains(*c)) {
            let named = |kind: &str| item.strip_prefix(kind).and_then(|r| r.strip_prefix("(\"")).and_then(|r| r.strip_suffix("\")"));
            let (name, listens) = match (named("QueueSend"), named("QueueMessage")) {
                (Some(name), _) => (name, false),
                (_, Some(name)) => (name, true),
                _ => continue,
            };
            let Some(binding) = self.resolve(deployment, name) else {
                return Some(format!("the queue {name:?} isn't bound on this server: an admin binds it (holon queue add {name} --url env:…, or --for {deployment} for this deployment only)"));
            };
            if listens && let Some(why) = self.refuses_listening(deployment, &binding) {
                return Some(why);
            }
        }
        None
    }

    /// Why `deployment` can't listen to the queue `binding` points to: another deployment
    /// listens to it.
    fn refuses_listening(&self, deployment: &str, binding: &Binding) -> Option<String> {
        let deployments = self.0.deployments.get()?;
        let listeners: Vec<(String, Vec<String>)> = deployments
            .lock()
            .unwrap()
            .iter()
            .map(|(slug, d)| (slug.clone(), d.server.as_ref().map(|s| s.queues.iter().map(|q| q.queue.clone()).collect()).unwrap_or_default()))
            .collect();
        self.listened(deployment, binding, &listeners)
    }

    /// Why `deployment` can't listen to the queue `binding` points to: another of `listeners`
    /// (each deployment, and the queues it listens to) listens to it, under whatever name.
    fn listened(&self, deployment: &str, binding: &Binding, listeners: &[(String, Vec<String>)]) -> Option<String> {
        let (slug, _) = listeners
            .iter()
            .filter(|(slug, _)| slug != deployment)
            .find(|(slug, queues)| queues.iter().any(|q| self.resolve(slug, q).is_some_and(|other| other.address == binding.address)))?;
        Some(format!("its queue ({}) is listened to by {slug} already: a message is deleted once, so one deployment listens to a queue", binding.address))
    }

    // ── on its way ──

    /// The messages a logged run sent (`messages`: to which queue, and the message, as it
    /// performed them), spooled before its record is written. One whose queue isn't bound now is
    /// noted and dropped.
    fn spool(&self, slug: &str, run: &str, at_ms: u64, messages: Vec<(String, Value)>, unlogged: bool) -> Vec<Queued> {
        let mut out = Vec::new();
        for (n, (queue, message)) in messages.into_iter().enumerate() {
            let Some((key, _)) = self.resolve_keyed(slug, &queue) else {
                self.note(slug, &queue, format!("didn't send a message to {queue}: it isn't bound now"));
                continue;
            };
            let Some(body) = body_of(&message) else { continue };
            let group = match &message {
                Value::Record(m) => match m.get("group") {
                    Some(Value::Text(g)) => Some(g.clone()),
                    _ => None,
                },
                _ => None,
            };
            let spooled = Spooled { slug: slug.to_string(), queue, run: run.to_string(), n, at_ms, group, unlogged };
            let file = self.0.spool.as_ref().map(|dir| dir.join(format!("{slug}.{run}.{n}.msg")));
            if let (Some(dir), Some(file)) = (&self.0.spool, &file) {
                let mut kept = serde_json::to_vec(&spooled).unwrap_or_default();
                kept.push(b'\n');
                kept.extend_from_slice(body.as_bytes());
                if let Err(e) = holon_host::create_private_dir(dir).and_then(|_| crate::sealing::write_private(file, &kept)) {
                    self.note(slug, &spooled.queue, format!("didn't send a message to {}: it couldn't be spooled ({e})", spooled.queue));
                    continue;
                }
            }
            out.push(Queued { key, spooled, body, file, tries: 0, not_before: Instant::now() });
        }
        out
    }

    /// Spool a logged run's messages (`entry` is its record, about to be written), before its
    /// record is: what [`hold`](Self::hold) holds. A run that failed or parked sent none.
    pub(crate) fn spool_logged(&self, slug: &str, entry: &LogEntry, messages: Vec<(String, Value)>) -> Vec<Queued> {
        if messages.is_empty() || entry.error.is_some() || !entry.parked.is_empty() {
            return Vec::new();
        }
        self.spool(slug, &entry.run_id, entry.at_ms, messages, false)
    }

    /// Hold the messages spooled for entry `seq`, just written, until it is on disk
    /// ([`release`](Self::release)).
    pub(crate) fn hold_sends(&self, slug: &str, seq: u64, spooled: Vec<Queued>) {
        for item in spooled {
            let queues = self.clone();
            self.hold(slug, seq, Box::new(move || queues.queue(item)));
        }
    }

    /// Hold `landed` until `slug`'s entry `seq` is on disk ([`release`](Self::release)).
    pub(crate) fn hold(&self, slug: &str, seq: u64, landed: Landed) {
        self.0.held.lock().unwrap().entry(slug.to_string()).or_default().push_back((seq, landed));
    }

    /// Forget what `slug`'s runs hold: it is deleted.
    pub(crate) fn forget_held(&self, slug: &str) {
        self.0.held.lock().unwrap().remove(slug);
    }

    /// Do what `slug`'s entries before `synced`, which are on disk, held, in its log's order.
    pub(crate) fn release(&self, slug: &str, synced: u64) {
        let ready: Vec<Landed> = {
            let mut held = self.0.held.lock().unwrap();
            let Some(q) = held.get_mut(slug) else { return };
            let mut ready = Vec::new();
            while let Some((_, landed)) = q.pop_front_if(|(seq, _)| *seq < synced) {
                ready.push(landed);
            }
            ready
        };
        for landed in ready {
            landed();
        }
    }

    /// Spool and queue an unrecorded run's messages now: its log holds nothing to wait for.
    pub(crate) fn send_unlogged(&self, slug: &str, entry: &LogEntry, messages: Vec<(String, Value)>) {
        if messages.is_empty() || entry.error.is_some() || !entry.parked.is_empty() {
            return;
        }
        for q in self.spool(slug, &entry.run_id, entry.at_ms, messages, true) {
            self.queue(q);
        }
    }

    /// What the spool holds, queued again as the server starts: each message whose run its
    /// deployment's log holds, finished (`log`: a deployment's entries, none when it isn't
    /// deployed), and each unlogged one. Another's run a crash lost before its record was on
    /// disk: it is removed, never sent.
    pub(crate) fn recover(&self, mut log: impl FnMut(&str) -> Option<Vec<LogEntry>>) {
        let Some(dir) = &self.0.spool else { return };
        let Ok(files) = std::fs::read_dir(dir) else { return };
        let mut logged: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut found: Vec<(Spooled, String, PathBuf)> = Vec::new();
        for file in files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|e| e == "msg")) {
            let Ok(bytes) = std::fs::read(&file) else { continue };
            let Some(end) = bytes.iter().position(|b| *b == b'\n') else { continue };
            let (Ok(spooled), Ok(body)) = (serde_json::from_slice::<Spooled>(&bytes[..end]), String::from_utf8(bytes[end + 1..].to_vec())) else {
                let _ = std::fs::remove_file(&file);
                continue;
            };
            let landed = spooled.unlogged
                || logged
                    .entry(spooled.slug.clone())
                    .or_insert_with(|| log(&spooled.slug).unwrap_or_default().into_iter().filter(|e| e.error.is_none() && e.parked.is_empty()).map(|e| e.run_id).collect())
                    .contains(&spooled.run);
            if !landed {
                let _ = std::fs::remove_file(&file);
                continue;
            }
            found.push((spooled, body, file));
        }
        // In the order they were sent: by their run's start, then their place in it.
        found.sort_by(|(a, _, _), (b, _, _)| (a.at_ms, &a.run, a.n).cmp(&(b.at_ms, &b.run, b.n)));
        for (spooled, body, file) in found {
            let key = self.resolve_keyed(&spooled.slug, &spooled.queue).map_or_else(|| spooled.queue.clone(), |(k, _)| k);
            self.queue(Queued { key, spooled, body, file: Some(file), tries: 0, not_before: Instant::now() });
        }
    }

    /// Start sending: each outbox's thread, now and as an outbox first has a message, its outcomes
    /// noted on `deployments`' timelines; and a deploy checked against their listeners.
    pub(crate) fn serve(&self, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>) {
        if self.0.deployments.set(deployments).is_err() {
            return;
        }
        for (key, outbox) in self.0.outboxes.lock().unwrap().iter() {
            self.start(key, Arc::clone(outbox));
        }
    }

    fn queue(&self, item: Queued) {
        let mut outboxes = self.0.outboxes.lock().unwrap();
        let outbox = match outboxes.get(&item.key) {
            Some(o) => Arc::clone(o),
            None => {
                let o = Arc::new(Outbox::default());
                outboxes.insert(item.key.clone(), Arc::clone(&o));
                if self.0.deployments.get().is_some() {
                    self.start(&item.key, Arc::clone(&o));
                }
                o
            }
        };
        drop(outboxes);
        outbox.items.lock().unwrap().push_back(item);
        outbox.ready.notify_one();
    }

    fn start(&self, key: &str, outbox: Arc<Outbox>) {
        let queues = self.clone();
        let _ = std::thread::Builder::new().name(format!("holon-queue-{key}")).spawn(move || {
            loop {
                let item = {
                    let mut items = outbox.items.lock().unwrap();
                    loop {
                        // In order: the first waits for its time, and those after it with it.
                        let now = Instant::now();
                        if let Some(first) = items.front() {
                            if first.not_before <= now {
                                break items.pop_front().expect("found");
                            }
                            let wait = first.not_before - now;
                            items = outbox.ready.wait_timeout(items, wait).unwrap().0;
                        } else {
                            items = outbox.ready.wait_timeout(items, Duration::from_secs(3600)).unwrap().0;
                        }
                    }
                };
                if let Some(again) = queues.deliver(item) {
                    outbox.items.lock().unwrap().push_front(again);
                }
            }
        });
    }

    /// Send `item`: sent or given up, its spool file removed and its outcome counted; or, after a
    /// temporary failure within the day, given back to try again later, first in its outbox.
    fn deliver(&self, mut item: Queued) -> Option<Queued> {
        let s = &item.spooled;
        let binding = self.resolve(&s.slug, &s.queue);
        let outcome = binding.ok_or_else(|| Failure::Permanent(format!("the queue {:?} isn't bound now", s.queue))).and_then(|binding| {
            if item.body.len() as u64 > binding.largest {
                return Err(Failure::Permanent(format!("it is {} bytes, and the queue takes {} at most now", item.body.len(), binding.largest)));
            }
            let queue = self.client(&binding.url).map_err(Failure::Temporary)?;
            // A FIFO queue's group is the run's, else the deployment's, and its deduplication id the
            // run's and the send's place in it: a send tried again after a lost answer is one message.
            let (group, dedup) = match binding.fifo {
                true => (Some(s.group.clone().unwrap_or_else(|| s.slug.clone())), Some(format!("{}-{}", s.run, s.n))),
                false => (None, None),
            };
            queue.send(&item.body, group.as_deref(), dedup.as_deref())
        });
        let (delays, give_up) = self.0.retry;
        let age = Duration::from_millis(firing::started_now().0.saturating_sub(s.at_ms));
        let key = (s.slug.clone(), s.queue.clone());
        match outcome {
            Ok(_) => {
                let was_failing = {
                    let mut sent = self.0.sent.lock().unwrap();
                    let counted = sent.entry(key).or_default();
                    counted.sent += 1;
                    std::mem::take(&mut counted.failing)
                };
                if was_failing {
                    self.note(&s.slug, &s.queue, format!("sends to {} again", s.queue));
                }
            }
            Err(Failure::Temporary(why)) if age < give_up => {
                if item.tries == 0 {
                    let first = !std::mem::replace(&mut self.0.sent.lock().unwrap().entry(key).or_default().failing, true);
                    if first {
                        self.note(&s.slug, &s.queue, format!("couldn't send a message to {} yet: {why}; trying again", s.queue));
                    }
                }
                item.not_before = Instant::now() + delays[(item.tries as usize).min(3)];
                item.tries += 1;
                return Some(item);
            }
            Err(f) => {
                eprintln!("holon: {}: gave up sending a message of run {} to {}: {}", s.slug, s.run, s.queue, f.why());
                self.0.sent.lock().unwrap().entry(key).or_default().gave_up += 1;
                self.note(&s.slug, &s.queue, format!("gave up sending a message of run {} to {}: {}", s.run, s.queue, f.why()));
            }
        }
        if let Some(file) = &item.file {
            let _ = std::fs::remove_file(file);
        }
        None
    }

    /// Note `what` of `queue` on `slug`'s timeline.
    pub(crate) fn note(&self, slug: &str, queue: &str, what: String) {
        if let Some(deployments) = self.0.deployments.get() {
            note(deployments, slug, queue, what);
        }
    }

    /// What `slug`'s messages to `queue` came to, with how many wait to be sent.
    fn sent(&self, slug: &str, queue: &str) -> (Sent, usize) {
        let counted = self.0.sent.lock().unwrap().get(&(slug.to_string(), queue.to_string())).cloned().unwrap_or_default();
        let waiting = self.0.outboxes.lock().unwrap().values().map(|o| o.items.lock().unwrap().iter().filter(|i| i.spooled.slug == slug && i.spooled.queue == queue).count()).sum();
        (counted, waiting)
    }

    // ── what each listener is doing ──

    /// Set what `slug`'s listener to `queue` is doing.
    pub(crate) fn set_live(&self, slug: &str, queue: &str, change: impl FnOnce(&mut Live)) {
        let mut live = self.0.live.lock().unwrap();
        let l = live.entry((slug.to_string(), queue.to_string())).or_insert_with(|| Live { status: "receiving", since: feeds::now_ms(), ..Live::default() });
        change(l);
    }

    /// Forget `slug`'s listener to `queue`: it stopped.
    pub(crate) fn forget_live(&self, slug: &str, queue: &str) {
        self.0.live.lock().unwrap().remove(&(slug.to_string(), queue.to_string()));
    }

    /// What `slug`'s listener to `queue` is doing, if it has one.
    pub(crate) fn live(&self, slug: &str, queue: &str) -> Option<Live> {
        self.0.live.lock().unwrap().get(&(slug.to_string(), queue.to_string())).cloned()
    }
}

/// Note `what` of `queue` on `slug`'s timeline.
pub(crate) fn note(deployments: &Mutex<BTreeMap<String, Deployment>>, slug: &str, queue: &str, what: String) {
    let detail = crate::events::Detail { name: Some(queue.to_string()), ..Default::default() };
    crate::events::note(deployments, slug, "queue", what, None, detail);
}

/// A message's body, as it is sent: its data's text as it is, anything else as JSON writes it;
/// none for what JSON can't write (bytes).
pub(crate) fn body_of(message: &Value) -> Option<String> {
    let Value::Record(m) = message else { return None };
    match m.get("data")? {
        Value::Text(t) => Some(t.clone()),
        v => holon_core::stringify_json(v),
    }
}

/// The messages a run performed, to which queue, and each message whole: what it spools.
pub(crate) fn of(performed: &[(Capability, Value)]) -> Vec<(String, Value)> {
    performed.iter().filter_map(|(c, p)| if let Capability::QueueSend(queue) = c { Some((queue.clone(), p.clone())) } else { None }).collect()
}

/// Messages a run's queues won't take: past what one run may send, one JSON can't write, an
/// empty one, or one past its queue's largest (`largest`: the queue's, as its binding says): why,
/// if so.
pub(crate) fn past_limit(performed: &[(Capability, Value)], largest: impl Fn(&str) -> Option<u64>) -> Option<String> {
    let sends: Vec<(&String, &Value)> = performed.iter().filter_map(|(c, p)| if let Capability::QueueSend(q) = c { Some((q, p)) } else { None }).collect();
    if sends.len() > MAX_SENDS {
        return Some(format!("a run sends {MAX_SENDS} messages at most, and this one sent {}", sends.len()));
    }
    for (queue, message) in sends {
        let Some(body) = body_of(message) else { return Some(format!("a message to {queue} is JSON or text, and this one's data holds bytes")) };
        if body.is_empty() {
            return Some(format!("a message to {queue} is empty, and a queue takes 1 byte at least"));
        }
        let most = largest(queue).unwrap_or(LARGEST);
        if body.len() as u64 > most {
            return Some(format!("a message to {queue} is {} bytes, and it takes {most} at most", body.len()));
        }
    }
    None
}

/// `GET /deployments/<slug>/queues`: each queue its handlers listen to, and what its listener is
/// doing; and each queue it sends to, and what its messages came to.
pub(crate) fn handle_get_queues(state: &AppState, slug: &str) -> Response {
    let (listened, sends): (Vec<(String, String)>, Vec<String>) = match state.deployments.lock().unwrap().get(slug) {
        Some(d) => (
            d.server.as_ref().map(|s| s.queues.iter().map(|q| (q.queue.clone(), q.label.clone())).collect()).unwrap_or_default(),
            d.permit.queues_sent().into_iter().collect(),
        ),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    let queues = &state.folders.queues;
    let listening: Vec<serde_json::Value> = listened
        .into_iter()
        .map(|(queue, label)| {
            let mut shown = serde_json::json!({ "queue": queue, "label": label, "status": "stopped", "waiting": 0, "run": 0 });
            if let Some(address) = queues.resolve(slug, &queue).map(|b| b.address) {
                shown["address"] = serde_json::json!(address);
            }
            if let Some(live) = queues.live(slug, &queue) {
                let mut l = serde_json::to_value(&live).unwrap_or_default();
                l["queue"] = serde_json::json!(queue);
                l["label"] = serde_json::json!(label);
                shown = l;
            }
            shown
        })
        .collect();
    let sending: Vec<serde_json::Value> = sends
        .into_iter()
        .map(|queue| {
            let (sent, waiting) = queues.sent(slug, &queue);
            let mut shown = serde_json::to_value(&sent).unwrap_or_default();
            shown["queue"] = serde_json::json!(queue);
            shown["waiting"] = serde_json::json!(waiting);
            if let Some(address) = queues.resolve(slug, &queue).map(|b| b.address) {
                shown["address"] = serde_json::json!(address);
            }
            shown
        })
        .collect();
    Response::json(&serde_json::json!({ "listening": listening, "sending": sending }))
}

// ── admin ──

fn shown(name: &str, deployment: Option<&str>, b: &Binding) -> serde_json::Value {
    serde_json::json!({ "name": name, "deployment": deployment, "url": b.url, "address": b.address, "fifo": b.fifo, "largest": b.largest, "visibility": b.visibility, "keep_failed": b.keep_failed })
}

fn labelled(name: &str, deployment: Option<&str>) -> String {
    deployment.map_or_else(|| name.to_string(), |d| format!("{name} (for {d})"))
}

/// `/admin/queues` (list, bind), `/admin/queues/<name>` (unbind; `?deployment=<slug>` for a
/// deployment's own), or None for any other path. Only an admin gets this far (access.rs); what
/// they do goes in the activity log.
pub(crate) fn route(state: &AppState, request: &Request, by: &str) -> Option<Response> {
    let url = request.url();
    let method = request.method();
    let all = &state.folders.queues;
    if url == "/admin/queues" {
        return Some(match method {
            "GET" => {
                let bound = all.all();
                let server = bound.server.iter().map(|(n, b)| shown(n, None, b));
                let own = bound.deployments.iter().flat_map(|(s, bs)| bs.iter().map(move |(n, b)| shown(n, Some(s), b)));
                Response::json(&server.chain(own).collect::<Vec<_>>())
            }
            "POST" => {
                #[derive(Deserialize)]
                struct Bind {
                    name: String,
                    #[serde(flatten)]
                    asked: Asked,
                    #[serde(default)]
                    deployment: Option<String>,
                }
                let bound = read_body(request)
                    .and_then(|b| serde_json::from_slice::<Bind>(&b).map_err(|e| err(400, format!("invalid body: {e}"))))
                    .map(|b| Bind { deployment: b.deployment.filter(|d| !d.trim().is_empty()), ..b })
                    .and_then(|b| {
                        let q = all.bind(&b.name, &b.asked, b.deployment.as_deref()).map_err(|e| err(400, e))?;
                        Ok((b, q))
                    });
                match bound {
                    Ok((b, q)) => {
                        state.activity.record("bound the queue", &labelled(&b.name, b.deployment.as_deref()), Some(by));
                        if let Some(slug) = &b.deployment {
                            let detail = crate::events::Detail { name: Some(b.name.clone()), ..Default::default() };
                            crate::events::note(&state.deployments, slug, "binding", format!("bound the queue {}", b.name), Some(by), detail);
                        }
                        Response::json(&shown(&b.name, b.deployment.as_deref(), &q))
                    }
                    Err(e) => e,
                }
            }
            _ => Response::empty_404(),
        });
    }
    let name = url.strip_prefix("/admin/queues/")?;
    if method != "DELETE" {
        return Some(Response::empty_404());
    }
    let deployment = request.get_param("deployment").filter(|d| !d.is_empty());
    Some(match all.unbind(name, deployment.as_deref()) {
        Ok(true) => {
            state.activity.record("unbound the queue", &labelled(name, deployment.as_deref()), Some(by));
            if let Some(slug) = &deployment {
                let detail = crate::events::Detail { name: Some(name.to_string()), ..Default::default() };
                crate::events::note(&state.deployments, slug, "binding", format!("unbound the queue {name}"), Some(by), detail);
            }
            Response::empty_204()
        }
        Ok(false) => err(404, format!("no queue {name:?} is bound{}", deployment.map_or(String::new(), |d| format!(" for {d}")))),
        Err(e) => err(500, e),
    })
}

/// `holon queue add NAME --url SOURCE [--visibility T] [--keep-failed] [--for SLUG] | list |
/// remove NAME`: bind, show and unbind the queues of the server this user runs here (or
/// `--server`), as an admin.
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut server = None;
    let mut for_deployment: Option<String> = None;
    let mut asked = serde_json::Map::new();
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--for" => for_deployment = args.next().cloned(),
            "--url" | "--visibility" => {
                let Some(v) = args.next() else {
                    eprintln!("holon queue: {a} takes a value\n\n{QUEUE_USAGE}");
                    return 2;
                };
                asked.insert(a[2..].to_string(), serde_json::Value::String(v.clone()));
            }
            "--keep-failed" => {
                asked.insert("keep_failed".into(), serde_json::Value::Bool(true));
            }
            "--help" | "-h" => {
                println!("{QUEUE_USAGE}");
                return 0;
            }
            _ if !a.starts_with('-') => words.push(a.as_str()),
            _ => {
                eprintln!("holon queue: no option {a:?}\n\n{QUEUE_USAGE}");
                return 2;
            }
        }
    }
    let (method, path, body) = match words.as_slice() {
        ["list"] => ("GET", "/admin/queues".to_string(), None),
        ["add", name] => {
            if !asked.contains_key("url") {
                eprintln!("holon queue add: it takes --url SOURCE, where the server reads the queue's URL\n\n{QUEUE_USAGE}");
                return 2;
            }
            asked.insert("name".into(), serde_json::Value::String(name.to_string()));
            asked.insert("deployment".into(), serde_json::json!(for_deployment));
            ("POST", "/admin/queues".to_string(), Some(serde_json::Value::Object(asked)))
        }
        ["remove", name] => {
            let only = for_deployment.as_ref().map_or(String::new(), |d| format!("?deployment={}", crate::mcp::seg(d)));
            ("DELETE", format!("/admin/queues/{}{only}", crate::mcp::seg(name)), None)
        }
        _ => {
            eprintln!("{QUEUE_USAGE}");
            return 2;
        }
    };
    let (server, answer) = match local::admin_call("holon queue", server.as_deref(), method, &path, body.as_ref()) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let line = |q: &serde_json::Value| {
        let mut said = vec![q["name"].as_str().unwrap_or("").to_string(), format!("{} (from {})", q["address"].as_str().unwrap_or(""), q["url"].as_str().unwrap_or(""))];
        if q["fifo"].as_bool() == Some(true) {
            said.push("FIFO".into());
        }
        said.push(format!("{} KiB at most", q["largest"].as_u64().unwrap_or(LARGEST) / 1024));
        said.push(format!("out of sight for {}", folders::span_text(q["visibility"].as_u64().unwrap_or(VISIBILITY))));
        if q["keep_failed"].as_bool() == Some(true) {
            said.push("a failed run's message kept".into());
        }
        format!("{}{}", said.join("  "), q["deployment"].as_str().map_or(String::new(), |s| format!(", for {s} only")))
    };
    match words[0] {
        "list" => {
            let all: Vec<serde_json::Value> = serde_json::from_str(&answer).unwrap_or_default();
            if all.is_empty() {
                println!("no queues are bound on {server}: holon queue add NAME --url env:… binds one");
            }
            for q in &all {
                println!("{}", line(q));
            }
        }
        "add" => println!("bound {}", line(&serde_json::from_str(&answer).unwrap_or_default())),
        _ => println!("unbound {}{}: its listener stops", words[1], for_deployment.as_ref().map_or(String::new(), |d| format!(" (for {d})"))),
    }
    0
}

const QUEUE_USAGE: &str = "usage: holon queue add NAME --url SOURCE [--visibility T] [--keep-failed] [--for SLUG]
       holon queue list
       holon queue remove NAME [--for SLUG]
       (each with [--server PORT|URL])

  Binds a name a flow imports (import payouts from \"queue:payouts\") to an Amazon SQS queue, or
  a service that speaks its API (ElasticMQ, LocalStack): a flow sends to it once its run is over
  (payouts.send({ data, group })), and its messages start runs (on.queue(payouts, (m) => …)), one
  at a time, each deleted once its run is over. A flow never sees the queue's URL or its key.
  Admins.

  add NAME                 bind NAME, once the queue answers, signed with its key
    --url SOURCE           where the server reads the queue's URL, as it reads a secret:
                           env:HOLON_SECRET_PAYOUTS_URL (its environment or --dotenv file) or
                           vault:<mount>/<path>#<field>. The URL is never stored, shown or
                           logged: https://<key id>:<secret>@sqs.<region>.amazonaws.com/<account>/
                           <queue>, the secret percent-encoded; ?region= for a service under
                           another name, ?token= for a temporary key; plain http:// only to this
                           machine
    --visibility T         how long a message received is kept out of sight while it waits for
                           its run and runs: 30s to 12h (5m); the listener keeps it so through a
                           longer run
    --keep-failed          leave a failed run's message for the queue to hand out again, for its
                           redrive policy to count, rather than delete it
    --for SLUG             bind it for that deployment only
  list                     the names bound, and where each points
  remove NAME              unbind it (--for SLUG: that deployment's); its listener stops
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon queue add payouts --url env:HOLON_SECRET_PAYOUTS_URL
  holon queue add payout_retries --url env:HOLON_SECRET_RETRIES_URL --visibility 2m --keep-failed
  holon queue list

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqs::stand_in::StandIn;

    fn queues(vars: &[(&str, &str)]) -> Queues {
        Queues::memory().with_sources(Sources::of(vars))
    }

    fn asked(url: &str) -> Asked {
        Asked { url: Some(url.into()), ..Asked::default() }
    }

    #[test]
    fn a_queue_is_bound_once_it_answers_signed_and_shown_without_its_key() {
        let sqs = StandIn::start();
        sqs.make("payouts");
        sqs.make("retries.fifo");
        let q = queues(&[("HOLON_SECRET_PAYOUTS_URL", &sqs.url("payouts")), ("HOLON_SECRET_RETRIES_URL", &sqs.url("retries.fifo"))]);
        let b = q.bind("payouts", &asked("env:HOLON_SECRET_PAYOUTS_URL"), None).unwrap();
        assert_eq!((b.address.ends_with("/000000000000/payouts"), b.fifo, b.largest, b.visibility, b.keep_failed), (true, false, 262_144, VISIBILITY, false));
        assert!(!b.address.contains(sqs::stand_in::KEY_ID), "shown without its key");
        let r = q.bind("retries", &Asked { visibility: Some("2m".into()), keep_failed: true, ..asked("env:HOLON_SECRET_RETRIES_URL") }, Some("app")).unwrap();
        assert_eq!((r.fifo, r.visibility, r.keep_failed), (true, 120, true));
        assert_eq!(q.resolve("app", "retries"), Some(r.clone()));
        assert_eq!(q.resolve("other", "retries"), None);
        assert!(q.unbind("retries", Some("app")).unwrap());
        assert!(!q.unbind("retries", Some("app")).unwrap());
    }

    #[test]
    fn a_binding_is_refused_for_what_isnt_a_queue_it_reaches() {
        let sqs = StandIn::start();
        sqs.make("payouts");
        let wrong = sqs.url("payouts").replace("%2B", "%2C");
        let q = queues(&[("HOLON_SECRET_Q", &sqs.url("payouts")), ("HOLON_SECRET_WRONG", &wrong), ("HOLON_SECRET_MISSING", &sqs.url("nowhere"))]);
        let refused = |name: &str, a: Asked| q.bind(name, &a, None).unwrap_err();
        assert!(refused("pay-outs", asked("env:HOLON_SECRET_Q")).contains("isn't a queue's name"));
        assert!(refused("payouts", asked("https://k:s@sqs.eu-west-1.amazonaws.com/1/q")).contains("read from a source"));
        assert!(refused("payouts", asked("env:HOLON_SECRET_WRONG")).contains("SignatureDoesNotMatch"));
        assert!(refused("payouts", asked("env:HOLON_SECRET_MISSING")).contains("QueueDoesNotExist"));
        assert!(refused("payouts", Asked { visibility: Some("10s".into()), ..asked("env:HOLON_SECRET_Q") }).contains("out of sight for 30s to 12h"));
        assert!(refused("payouts", Asked { visibility: Some("13h".into()), ..asked("env:HOLON_SECRET_Q") }).contains("out of sight"));
    }

    #[test]
    fn a_runs_messages_are_held_to_what_a_run_sends_and_what_the_queue_takes() {
        let send = |data: Value| (Capability::QueueSend("payouts".into()), Value::Record(BTreeMap::from([("data".to_string(), data)])));
        assert_eq!(past_limit(&[send(Value::Text("x".into()))], |_| Some(10)), None);
        assert!(past_limit(&vec![send(Value::Text("x".into())); MAX_SENDS + 1], |_| None).unwrap().contains("100 messages at most"));
        assert!(past_limit(&[send(Value::Text("12345678901".into()))], |_| Some(10)).unwrap().contains("11 bytes, and it takes 10 at most"));
        assert!(past_limit(&[send(Value::Text(String::new()))], |_| None).unwrap().contains("1 byte at least"));
        assert!(past_limit(&[send(Value::Bytes(vec![1]))], |_| None).unwrap().contains("holds bytes"));
        // Text is sent as it is, anything else as JSON writes it.
        let record = Value::Record(BTreeMap::from([("id".to_string(), Value::Text("po_1".into())), ("n".to_string(), Value::Number(2.0))]));
        assert_eq!(body_of(&send(record).1).as_deref(), Some("{\"id\":\"po_1\",\"n\":2}"));
        assert_eq!(body_of(&send(Value::Text("plain".into())).1).as_deref(), Some("plain"));
    }

    fn until(what: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !what() {
            assert!(Instant::now() < end, "it didn't come to pass");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn entry(run: &str) -> LogEntry {
        LogEntry { run_id: run.into(), at_ms: firing::started_now().0, ..Default::default() }
    }

    fn message(data: &str, group: Option<&str>) -> Value {
        let mut m = BTreeMap::from([("data".to_string(), Value::Text(data.into()))]);
        if let Some(g) = group {
            m.insert("group".into(), Value::Text(g.into()));
        }
        Value::Record(m)
    }

    #[test]
    fn a_runs_messages_go_once_its_record_is_on_disk_in_order_a_fifo_ones_grouped_and_deduplicated() {
        let sqs = StandIn::start();
        sqs.make("payouts");
        sqs.make("retries.fifo");
        let q = queues(&[("HOLON_SECRET_P", &sqs.url("payouts")), ("HOLON_SECRET_R", &sqs.url("retries.fifo"))]);
        q.bind("payouts", &asked("env:HOLON_SECRET_P"), None).unwrap();
        q.bind("retries", &asked("env:HOLON_SECRET_R"), None).unwrap();
        q.serve(Arc::default());
        let sent = vec![("payouts".to_string(), message("one", None)), ("payouts".to_string(), message("two", None)), ("retries".to_string(), message("again", Some("acct_1"))), ("retries".to_string(), message("mine", None))];
        let spooled = q.spool_logged("app", &entry("r1"), sent);
        q.hold_sends("app", 4, spooled);
        std::thread::sleep(Duration::from_millis(200));
        assert!(sqs.messages("payouts").is_empty(), "held until its record is on disk");
        q.release("app", 4);
        assert!(sqs.messages("payouts").is_empty(), "a sync through 3 doesn't cover 4");
        q.release("app", 5);
        until(|| sqs.messages("payouts").len() == 2 && sqs.messages("retries.fifo").len() == 2);
        assert_eq!(sqs.messages("payouts").iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["one", "two"]);
        let fifo = sqs.messages("retries.fifo");
        assert_eq!(fifo.iter().map(|m| (m.body.as_str(), m.group.as_deref())).collect::<Vec<_>>(), [("again", Some("acct_1")), ("mine", Some("app"))], "its group, else the deployment's");
        assert_eq!(q.sent("app", "payouts"), (Sent { sent: 2, ..Sent::default() }, 0));
        // A failed or parked run sends nothing.
        assert!(q.spool_logged("app", &LogEntry { error: Some("x".into()), ..entry("r2") }, vec![("payouts".into(), message("no", None))]).is_empty());
    }

    #[test]
    fn a_message_is_tried_again_on_what_is_temporary_and_given_up_on_the_rest() {
        let sqs = StandIn::start();
        sqs.make("payouts");
        let q = queues(&[("HOLON_SECRET_P", &sqs.url("payouts"))]).with_retry([Duration::from_millis(50); 4], Duration::from_secs(3600));
        q.bind("payouts", &asked("env:HOLON_SECRET_P"), None).unwrap();
        q.serve(Arc::default());
        sqs.fail(&[(500, "InternalError"), (400, "RequestThrottled")]);
        q.send_unlogged("app", &entry("r1"), vec![("payouts".into(), message("one", None))]);
        until(|| sqs.messages("payouts").len() == 1);
        sqs.fail(&[(400, "InvalidParameterValue")]);
        q.send_unlogged("app", &entry("r2"), vec![("payouts".into(), message("two", None))]);
        until(|| q.sent("app", "payouts").0.gave_up == 1);
        assert_eq!(q.sent("app", "payouts").0.sent, 1);
        assert_eq!(sqs.messages("payouts").len(), 1);
    }

    #[test]
    fn a_restart_sends_what_the_spool_holds_whose_run_its_log_holds() {
        let dir = std::env::temp_dir().join(format!("holon-queues-spool-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("queues.json");
        let sqs = StandIn::start();
        sqs.make("payouts");
        let vars = [("HOLON_SECRET_P", sqs.url("payouts"))];
        let vars: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let first = Queues::open(file.to_str().unwrap()).unwrap().with_sources(Sources::of(&vars));
        first.bind("payouts", &asked("env:HOLON_SECRET_P"), None).unwrap();
        // Two runs spooled, neither sent: one whose record landed, one a crash lost.
        first.hold_sends("app", 1, first.spool_logged("app", &entry("landed"), vec![("payouts".into(), message("kept", None))]));
        first.hold_sends("app", 2, first.spool_logged("app", &entry("lost"), vec![("payouts".into(), message("gone", None))]));
        let again = Queues::open(file.to_str().unwrap()).unwrap().with_sources(Sources::of(&vars));
        again.recover(|slug| (slug == "app").then(|| vec![LogEntry { run_id: "landed".into(), ..Default::default() }]));
        again.serve(Arc::default());
        until(|| sqs.messages("payouts").len() == 1);
        assert_eq!(sqs.messages("payouts")[0].body, "kept");
        until(|| std::fs::read_dir(dir.join("queue-spool")).map(|d| d.count() == 0).unwrap_or(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deploy_using_a_queue_no_one_bound_or_another_listens_to_is_refused() {
        let sqs = StandIn::start();
        sqs.make("payouts");
        let q = queues(&[("HOLON_SECRET_P", &sqs.url("payouts"))]);
        let manifest = |caps: &[&str]| Manifest { capabilities: caps.iter().map(|c| c.to_string()).collect(), requirements: Default::default() };
        assert!(q.refuses("app", &manifest(&["QueueSend(\"payouts\")"])).unwrap().contains("isn't bound on this server"));
        assert!(q.refuses("app", &manifest(&["QueueMessage(\"payouts\")"])).unwrap().contains("holon queue add payouts"));
        let b = q.bind("payouts", &asked("env:HOLON_SECRET_P"), None).unwrap();
        assert_eq!(q.refuses("app", &manifest(&["QueueSend(\"payouts\")", "QueueMessage(\"payouts\")"])), None);
        // Another deployment listening to it, under another name, and the deployment itself.
        q.bind("events", &asked("env:HOLON_SECRET_P"), Some("other")).unwrap();
        let listeners = [("other".to_string(), vec!["events".to_string()]), ("app".to_string(), vec!["payouts".to_string()])];
        assert!(q.listened("app", &b, &listeners).unwrap().contains("is listened to by other already"));
        assert_eq!(q.listened("app", &b, &listeners[1..]), None, "its own listening is no other's");
    }
}
