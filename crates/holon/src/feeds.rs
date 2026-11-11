//! Feeds (host-capabilities L4 §5.9.2): what a flow listens to with `on.feed(prices, (e) => …)`,
//! through a feed an admin binds.
//!
//! - **Bound by an admin,** as a mail is (`--feeds`, owner-only; `/admin/feeds`; `holon feed`): a
//!   name a flow imports (`import prices from "feed:prices"`) bound to a feed of server-sent
//!   events (sse.rs) or a WebSocket (ws.rs, §5.9.4), its URL read from a source (`env:…`,
//!   `vault:…`) each time a connection opens and never kept, with the headers each request sends,
//!   their values read from sources too; how many events may wait for their run (`--buffer`), what
//!   comes when they are that many (`--on-full`: wait, skip, or replace the one of its `--key`),
//!   the field that numbers a JSON message, one more each message of its `--key`, which says how
//!   many were skipped (`--sequence`, §5.9.6), and how long the feed may say nothing before the
//!   connection is opened again (`--idle`). A
//!   WebSocket's binding also says what it is sent when it opens (`--subscribe`) and to keep it
//!   (`--heartbeat`), which fields are a message's id (`--id`), which messages run (`--only`), and
//!   where the feed takes the position a connection opens from (`--resume`, §5.9.6). Or a Postgres
//!   channel (`--database`, `--channel`, §5.9.11): `LISTEN` on a bound database (listen.rs), each
//!   notification a message, `--id` and `--only` as a WebSocket's. For the whole server or one
//!   deployment.
//! - **Checked when bound:** a request to it answers 200 with `text/event-stream`, or a WebSocket's
//!   101, or the database takes the `LISTEN`, then it is closed. The binding keeps the sources,
//!   and where the URL points without its login or query, which is what is shown. A channel's
//!   database is the one the listening deployment sees by that name, read each time a connection
//!   opens ([`Binding::seen_by`]).
//! - **The grant** names the feed (`FeedEvent("prices")`); a deploy is refused while it isn't bound
//!   ([`Feeds::refuses`]). Several deployments may listen to one feed: each has its own
//!   connection.
//! - **What each connection is doing** (subscriptions.rs keeps it here) is in
//!   `GET /deployments/<slug>/feeds`.

use std::collections::VecDeque;
use crate::sync::RwLock;
use std::time::Duration;

use rouille::{Request, Response};

use crate::databases::{Database, Databases};
use crate::http::{err, read_body};
use crate::permissions::Manifest;
use crate::secret_sources::{Reference, Sources};
use zeroize::Zeroizing;

use crate::*;

/// How many events wait for their run, unless the binding says.
pub(crate) const BUFFER: usize = 1000;

/// The most events a binding lets wait.
const BUFFER_MOST: usize = 100_000;

/// How long a feed may say nothing, unless the binding says, in seconds.
pub(crate) const IDLE: u64 = 90;

/// The least idle time a binding sets.
const IDLE_LEAST: u64 = 5;

/// One bound feed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Binding {
    /// Where the host reads the feed's URL from (`env:…`, `vault:…`), never the URL itself. A
    /// channel's is its database's connection, as the listening deployment sees it: not kept.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) url: String,
    /// Where that URL points, without its login or query (`https://stream.example.com/v1/prices`);
    /// a channel's database, and the channel (`postgres://db.example.com:5432/ledger, channel
    /// ledger_entries`).
    pub(crate) address: String,
    /// The headers each request sends: a name, and where its value is read from.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) headers: BTreeMap<String, String>,
    /// How many events may wait for their run before the connection waits too.
    pub(crate) buffer: usize,
    /// How long the feed may say nothing before the connection is opened again, in seconds.
    pub(crate) idle: u64,
    /// What comes while `buffer` events wait.
    #[serde(default, skip_serializing_if = "OnFull::is_wait")]
    pub(crate) on_full: OnFull,
    /// The field of a message that is its key (a dotted path): `latest` replaces the one waiting
    /// of its key, and `sequence` numbers each key on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) key: Option<String>,
    /// The field of a JSON message that numbers it, a whole number one more each message of its
    /// key: what `e.gap` counts the numbers skipped of (a dotted path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sequence: Option<String>,
    /// A WebSocket's: what it is sent each time it opens, in order, each as written or the source
    /// it is read from (`env:…`, for one that holds a key).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) subscribe: Vec<String>,
    /// A WebSocket's: what it is sent every `every` seconds while it is open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) heartbeat: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) every: u64,
    /// A WebSocket's: the fields of a JSON message that are its id, joined with `/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) id: Vec<String>,
    /// A WebSocket's: the messages run, by the values a field may have (every field named must
    /// match); the rest are passed over.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) only: BTreeMap<String, Vec<String>>,
    /// A WebSocket's: where the feed takes the position a connection opens from, when the host
    /// knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resume: Option<Resume>,
    /// A Postgres channel's (§5.9.11): the database it is on, by the name a deployment binds it
    /// as, and the channel listened on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) database: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) channel: Option<String>,
}

impl Binding {
    /// This binding as `slug` sees it: a channel's on the database `slug` sees by its name, its
    /// connection where the URL is read from and where it is in the address. None it sees, or one
    /// that isn't a Postgres, leaves it no URL: a connection to it says so.
    pub(crate) fn seen_by(self, databases: &Databases, slug: &str) -> Binding {
        let Some(name) = &self.database else { return self };
        let database = databases.resolve(slug, name);
        self.on(database.as_ref())
    }

    fn on(self, database: Option<&Database>) -> Binding {
        let Some(channel) = self.channel.clone() else { return self };
        match database.filter(|d| d.engine == "postgres") {
            Some(d) => Binding { url: d.connection.clone().unwrap_or_default(), address: format!("{}, channel {channel}", d.at()), ..self },
            None => Binding { url: String::new(), ..self },
        }
    }
}

/// Where a WebSocket feed that replays takes the position it replays from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Resume {
    /// A parameter of the URL's query: `cursor=<position>`, in place of one of that name.
    Query(String),
    /// A header the request sends: `X-From: <position>`.
    Header(String),
    /// The messages sent in place of the subscription, each as written or the source it is read
    /// from, `{position}` in each replaced by it.
    Message(Vec<String>),
}

/// What a `--resume` message writes where the position goes.
const POSITION: &str = "{position}";

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// What a feed's connection does with what comes while its buffer is full.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OnFull {
    /// Stops reading, and the feed waits, as TCP makes it.
    #[default]
    Wait,
    /// Drops it, and the next event that finds room says how many were (`e.missed`).
    Skip,
    /// Replaces the one of its key that waits, full or not (`e.replaced`); a new key waits.
    Latest,
}

impl OnFull {
    fn is_wait(&self) -> bool {
        *self == OnFull::Wait
    }
}

/// How often a heartbeat goes out, unless the binding says, in seconds.
const EVERY: u64 = 30;

/// The longest message a binding sends, in bytes.
const MESSAGE_MOST: usize = 64 * 1024;

/// What binding a feed asks for, as `/admin/feeds` takes it.
#[derive(Deserialize, Default, Debug)]
pub(crate) struct Asked {
    #[serde(default)]
    pub(crate) url: Option<String>,
    /// Each header's name, and where its value is read from.
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) buffer: Option<u64>,
    /// `90s`, `2m`.
    #[serde(default)]
    pub(crate) idle: Option<String>,
    /// `wait`, `skip` or `latest`.
    #[serde(default)]
    pub(crate) on_full: Option<String>,
    #[serde(default)]
    pub(crate) key: Option<String>,
    /// `trade_id`.
    #[serde(default)]
    pub(crate) sequence: Option<String>,
    #[serde(default)]
    pub(crate) subscribe: Vec<String>,
    #[serde(default)]
    pub(crate) heartbeat: Option<String>,
    /// `30s`.
    #[serde(default)]
    pub(crate) every: Option<String>,
    /// `product_id,sequence`.
    #[serde(default)]
    pub(crate) id: Option<String>,
    /// `type=ticker`, each.
    #[serde(default)]
    pub(crate) only: Vec<String>,
    /// `query:cursor`, `header:X-From`, or `message:…` (again for another).
    #[serde(default)]
    pub(crate) resume: Vec<String>,
    /// A Postgres channel's: the database, as bound (`ledger`), and the channel (`ledger_entries`).
    #[serde(default)]
    pub(crate) database: Option<String>,
    #[serde(default)]
    pub(crate) channel: Option<String>,
}

/// Where `--resume` says the feed takes the position: one parameter or header, or the messages
/// sent in place of the subscription; or why it says nothing a feed can take.
fn resume_of(asked: &[String]) -> Result<Option<Resume>, String> {
    const ONE: &str = "--resume is one place the feed takes the position: query:NAME, header:NAME, or message:MESSAGE (again for another message)";
    let (mut place, mut messages) = (None, Vec::new());
    for given in asked.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let Some((kind, what)) = given.split_once(':') else { return Err(format!("{ONE}, not {given:?}")) };
        let what = what.trim();
        let one = match kind {
            "query" if !what.is_empty() && what.len() <= 64 && what.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b)) => Resume::Query(what.to_string()),
            "header" if sse::is_header_name(what) => Resume::Header(what.to_string()),
            "query" | "header" => return Err(format!("--resume {kind}: names the {kind} the position goes in; {what:?} isn't one")),
            "message" if what.is_empty() || what.len() > MESSAGE_MOST => return Err(format!("--resume message: is a message of 1 byte to {} KiB", MESSAGE_MOST >> 10)),
            "message" if !is_source(what) && !what.contains(POSITION) => return Err(format!("--resume message: writes {POSITION} where the feed takes the position, as {{\"from\":{POSITION}}}")),
            "message" => {
                messages.push(what.to_string());
                continue;
            }
            _ => return Err(format!("{ONE}, not {given:?}")),
        };
        if place.replace(one).is_some() {
            return Err(ONE.into());
        }
    }
    match (place, messages.is_empty()) {
        (Some(_), false) => Err(ONE.into()),
        (place, true) => Ok(place),
        (None, false) => Ok(Some(Resume::Message(messages))),
    }
}

/// A field of a JSON message, as a binding names it: a dotted path (`data.u`), or why it isn't one.
fn path_of(what: &str, path: &str) -> Result<String, String> {
    let path = path.trim();
    let ok = !path.is_empty() && path.len() <= 256 && path.split('.').all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
    ok.then(|| path.to_string()).ok_or_else(|| format!("{what} names a field of a JSON message: a name, or a dotted path (data.u); {path:?} isn't one"))
}

/// The value at `path` in `data`, as text: a text as it is, anything else as JSON writes it.
pub(crate) fn field_text(data: &Value, path: &str) -> Option<String> {
    let mut at = data;
    for name in path.split('.') {
        let Value::Record(fields) = at else { return None };
        at = fields.get(name)?;
    }
    match at {
        Value::Text(t) => Some(t.clone()),
        Value::Record(_) | Value::List(_) => None,
        v => holon_core::stringify_json(v),
    }
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

/// What one deployment's connection to one feed is doing.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub(crate) struct Live {
    /// Where the feed points, as its binding shows it.
    pub(crate) address: String,
    /// `connecting`, `connected` or `reconnecting`.
    pub(crate) status: &'static str,
    /// Since when, in milliseconds since the epoch.
    pub(crate) since: u64,
    /// Why it isn't connected, when it isn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// The id of the last event run on, when the feed gives ids.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) position: Option<String>,
    /// How many events wait for their run.
    pub(crate) waiting: usize,
    /// Since the connection opened: how many messages `--only` passed over, a full buffer dropped
    /// (`skip`), and a newer one of their key replaced (`latest`).
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) passed_over: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) missed: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) replaced: u64,
    /// How many numbers the feed skipped (`--sequence`): what came while the host wasn't reading.
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) gaps: u64,
}

/// The feeds bound on this server, and what each connection to them is doing.
#[derive(Clone)]
pub(crate) struct Feeds(Arc<Inner>);

struct Inner {
    file: Option<PathBuf>,
    bound: RwLock<Bound>,
    /// What a binding's URL and headers are read from: the server's environment and dotenv
    /// file, Vault.
    sources: Sources,
    /// Each connection's state, by deployment and feed.
    live: Mutex<BTreeMap<(String, String), Live>>,
    /// Where lazy runs moved their feeds, by deployment, each written once the sync covering the
    /// run's entry (its seq) is done, in the log's order.
    held: Mutex<BTreeMap<String, VecDeque<(u64, Moved)>>>,
}

/// What keeps where a run moved its feed.
type Moved = Box<dyn FnOnce() + Send>;

impl std::fmt::Debug for Feeds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feeds").field("bound", &self.0.bound.read().unwrap()).finish()
    }
}

fn given(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn is_source(s: &str) -> bool {
    s.starts_with("env:") || s.starts_with("vault:")
}

impl Feeds {
    /// None, kept nowhere: the tests, and a server told `--feeds :memory:`.
    pub(crate) fn memory() -> Feeds {
        Feeds::with(None, Bound::default())
    }

    fn with(file: Option<PathBuf>, bound: Bound) -> Feeds {
        Feeds(Arc::new(Inner { file, bound: RwLock::new(bound), sources: Sources::none(), live: Mutex::default(), held: Mutex::default() }))
    }

    /// The feeds bound in `file` (none yet if it doesn't exist).
    pub(crate) fn open(file: &str) -> Result<Feeds, String> {
        if file == ":memory:" {
            return Ok(Feeds::memory());
        }
        let path = PathBuf::from(file);
        let bound = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{file} isn't a feeds file: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Bound::default(),
            Err(e) => return Err(format!("can't read {file}: {e}")),
        };
        Ok(Feeds::with(Some(path), bound))
    }

    /// These, reading a binding's URL and headers from `sources`.
    pub(crate) fn with_sources(self, sources: Sources) -> Feeds {
        let inner = Arc::into_inner(self.0).expect("set up before it is shared");
        Feeds(Arc::new(Inner { sources, ..inner }))
    }

    /// Every binding: the server's, and each deployment's.
    pub(crate) fn all(&self) -> Bound {
        self.0.bound.read().unwrap().clone()
    }

    /// The binding deployment `deployment` sees as `name`: its own, else the server's.
    pub(crate) fn resolve(&self, deployment: &str, name: &str) -> Option<Binding> {
        let bound = self.0.bound.read().unwrap();
        bound.deployments.get(deployment).and_then(|d| d.get(name)).or_else(|| bound.server.get(name)).cloned()
    }

    /// Bind `name`, for the whole server or one deployment, replacing any binding of it there
    /// before, as `asked` says: once a request to it, with its headers, answers as a feed does, or
    /// `database` (the one `asked` names, as the deployment, or the server, sees it) takes the
    /// `LISTEN`.
    pub(crate) fn bind(&self, name: &str, asked: &Asked, database: Option<&Database>, for_deployment: Option<&str>) -> Result<Binding, String> {
        if !folders::is_folder_name(name) || name.contains('-') {
            return Err(format!("{name:?} isn't a feed's name: letters, digits and _, up to 64, as a flow imports it"));
        }
        if let Some(slug) = for_deployment.filter(|s| !folders::is_folder_name(s)) {
            return Err(format!("{slug:?} isn't a deployment's name"));
        }
        let listen = match (given(&asked.database), given(&asked.channel)) {
            (None, None) => None,
            (Some(_), None) | (None, Some(_)) => return Err("a Postgres channel is listened on with --database NAME and --channel CHANNEL, both".into()),
            (Some(db), Some(channel)) => {
                let url_only = asked.url.is_some() || !asked.headers.is_empty() || !asked.subscribe.is_empty() || asked.heartbeat.is_some() || asked.every.is_some() || !asked.resume.is_empty();
                if url_only {
                    return Err("--url, --header, --subscribe, --heartbeat and --resume are a feed's read from a URL: a channel is listened on in its --database".into());
                }
                if !listen::is_channel(channel) {
                    return Err(format!("--channel is a Postgres channel: letters, digits and _, up to 63 bytes; {channel:?} isn't one"));
                }
                let seen = for_deployment.map_or_else(|| "on this server".to_string(), |d| format!("for {d}"));
                let d = database.ok_or_else(|| format!("the database {db} isn't bound {seen}: an admin binds it (holon database add {db} …)"))?;
                if d.engine != "postgres" {
                    return Err(format!("LISTEN is Postgres's: the database {db} is {}", d.engine));
                }
                Some((db, channel, d))
            }
        };
        let url = match listen {
            Some(_) => "",
            None => given(&asked.url).ok_or("a feed is read from the URL --url names, or listened on with --database and --channel: it names neither")?,
        };
        if listen.is_none() && !is_source(url) {
            return Err("a feed's URL is read from a source, env:HOLON_SECRET_… or vault:…, holding https://host/path".into());
        }
        for (header, source) in &asked.headers {
            if !sse::is_header_name(header) {
                return Err(format!("{header:?} isn't a header's name"));
            }
            if !is_source(source.trim()) {
                return Err(format!("the header {header}'s value is read from a source, env:HOLON_SECRET_… or vault:…"));
            }
        }
        let buffer = match asked.buffer {
            None => BUFFER,
            Some(n) if (1..=BUFFER_MOST as u64).contains(&n) => n as usize,
            Some(_) => return Err(format!("--buffer is how many events may wait: 1 to {BUFFER_MOST}")),
        };
        let idle = match given(&asked.idle) {
            None => IDLE,
            Some(t) => folders::seconds_of(t).ok_or_else(|| format!("{t:?} isn't how long: 90s, 2m"))?,
        };
        if idle < IDLE_LEAST {
            return Err(format!("--idle is {IDLE_LEAST}s at least: a feed that says nothing for less may only be quiet"));
        }
        let on_full = match given(&asked.on_full) {
            None | Some("wait") => OnFull::Wait,
            Some("skip") => OnFull::Skip,
            Some("latest") => OnFull::Latest,
            Some(other) => return Err(format!("--on-full is wait, skip or latest, not {other:?}")),
        };
        let key = given(&asked.key).map(|k| path_of("--key", k)).transpose()?;
        let sequence = given(&asked.sequence).map(|s| path_of("--sequence", s)).transpose()?;
        match (on_full, &key, &sequence) {
            (OnFull::Latest, None, _) => return Err("--on-full latest replaces the waiting message of a message's key: it takes --key FIELD".into()),
            (OnFull::Wait | OnFull::Skip, Some(_), None) => {
                return Err("--key is a message's key for --on-full latest (which waiting message it replaces) and for --sequence (each key numbered on its own): it takes one of them".into());
            }
            _ => {}
        }
        let mut subscribe = Vec::new();
        for message in &asked.subscribe {
            let message = message.trim();
            if message.is_empty() || message.len() > MESSAGE_MOST {
                return Err(format!("--subscribe is a message of 1 byte to {} KiB", MESSAGE_MOST >> 10));
            }
            subscribe.push(message.to_string());
        }
        let heartbeat = given(&asked.heartbeat).map(str::to_string);
        if heartbeat.as_ref().is_some_and(|h| h.len() > MESSAGE_MOST || is_source(h)) {
            return Err(format!("--heartbeat is a message as written, up to {} KiB", MESSAGE_MOST >> 10));
        }
        let every = match (given(&asked.every), &heartbeat) {
            (None, None) => 0,
            (None, Some(_)) => EVERY,
            (Some(_), None) => return Err("--every is how often --heartbeat is sent: it takes --heartbeat MESSAGE".into()),
            (Some(t), Some(_)) => folders::seconds_of(t).filter(|s| (1..=3600).contains(s)).ok_or_else(|| format!("--every is how often the heartbeat goes: 1s to 1h, not {t:?}"))?,
        };
        let id = match given(&asked.id) {
            None => Vec::new(),
            Some(fields) => fields.split(',').map(|p| path_of("--id", p)).collect::<Result<Vec<_>, _>>()?,
        };
        let mut only: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for pair in &asked.only {
            let (field, value) = pair.split_once('=').ok_or_else(|| format!("--only is FIELD=VALUE (type=ticker), not {pair:?}"))?;
            only.entry(path_of("--only", field)?).or_default().push(value.trim().to_string());
        }
        let resume = resume_of(&asked.resume)?;
        if resume.is_some() && id.len() != 1 {
            return Err("--resume sends the feed the id of the message the host is at, which a cursor is one value of: it takes an --id of one field (--id seq)".into());
        }
        let headers: BTreeMap<String, String> = asked.headers.iter().map(|(h, s)| (h.clone(), s.trim().to_string())).collect();
        let (database, channel) = (listen.map(|(db, ..)| db.to_string()), listen.map(|(_, channel, _)| channel.to_string()));
        let binding = Binding { url: url.to_string(), address: String::new(), headers, buffer, idle, on_full, key, sequence, subscribe, heartbeat, every, id, only, resume, database, channel };
        let check = Duration::from_secs(10);
        let binding = match listen {
            Some((_, channel, d)) => {
                let seen = binding.clone().on(Some(d));
                let url = self.0.sources.fetch(&Reference::parse(&seen.url)?)?;
                listen::open(&url, channel, check).map_err(|why| format!("the channel wasn't listened on: {why}"))?;
                Binding { address: seen.address, ..binding }
            }
            None => {
                let (parsed, sent) = self.request(&binding)?;
                let websocket_only = !binding.subscribe.is_empty() || binding.heartbeat.is_some() || !binding.id.is_empty() || !binding.only.is_empty() || binding.resume.is_some();
                if websocket_only && !parsed.is_websocket() {
                    return Err("--subscribe, --heartbeat, --id, --only and --resume are a WebSocket's (wss://): this URL is a feed of server-sent events, whose events have their own ids and resume with Last-Event-ID".into());
                }
                let read = match parsed.is_websocket() {
                    true => ws::open(&parsed, &sent, &[], None, check).map(drop),
                    false => sse::open(&parsed, &sent, None, check).map(drop),
                };
                read.map_err(|why| format!("the feed wasn't read: {why}"))?;
                Binding { address: parsed.address(), ..binding }
            }
        };
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

    /// `binding`'s URL and headers, read from their sources now.
    fn request(&self, binding: &Binding) -> Result<(sse::Url, sse::Headers), String> {
        let url = sse::Url::parse(&self.0.sources.fetch(&Reference::parse(&binding.url)?)?)?;
        let mut headers = Vec::new();
        for (name, source) in &binding.headers {
            let value = self.0.sources.fetch(&Reference::parse(source)?).map_err(|e| format!("the header {name}: {e}"))?;
            headers.push((name.clone(), value));
        }
        Ok((url, headers))
    }

    /// A connection to `binding`'s feed, from `last_id` when the host knows where it is: what it
    /// sends `Last-Event-ID` with, or a WebSocket's id of the last message read, which goes where
    /// `--resume` says.
    pub(crate) fn connect(&self, binding: &Binding, last_id: Option<&str>) -> Result<Link, String> {
        let idle = Duration::from_secs(binding.idle);
        if let (Some(db), Some(channel)) = (&binding.database, &binding.channel) {
            if binding.url.is_empty() {
                return Err(format!("the database {db} isn't a Postgres bound for this deployment: LISTEN is Postgres's"));
            }
            let url = self.0.sources.fetch(&Reference::parse(&binding.url)?)?;
            return Ok(Link::Pg(Box::new(listen::open(&url, channel, idle)?), last_id.unwrap_or_default().to_string()));
        }
        let (mut url, mut headers) = self.request(binding)?;
        if !url.is_websocket() {
            return Ok(Link::Sse(sse::open(&url, &headers, last_id, idle)?));
        }
        let position = last_id.filter(|p| !p.is_empty());
        let mut messages = &binding.subscribe;
        match (&binding.resume, position) {
            (Some(Resume::Query(name)), Some(p)) => url = url.with_query(name, p),
            (Some(Resume::Header(name)), Some(p)) => headers.push((name.clone(), Zeroizing::new(p.to_string()))),
            (Some(Resume::Message(resumed)), Some(_)) => messages = resumed,
            _ => {}
        }
        let mut subscribe = Vec::new();
        for message in messages {
            let message = match is_source(message) {
                true => self.0.sources.fetch(&Reference::parse(message)?).map_err(|e| format!("the subscription: {e}"))?,
                false => Zeroizing::new(message.clone()),
            };
            subscribe.push(match (&binding.resume, position) {
                (Some(Resume::Message(_)), Some(p)) => Zeroizing::new(message.replace(POSITION, p)),
                _ => message,
            });
        }
        let heartbeat = binding.heartbeat.clone().map(|message| ws::Heartbeat { message, every: Duration::from_secs(binding.every.max(1)) });
        Ok(Link::Ws(Box::new(ws::open(&url, &headers, &subscribe, heartbeat, idle)?), last_id.unwrap_or_default().to_string()))
    }

    /// Unbind `name`, the server's or one deployment's: whether it was bound. A connection to it
    /// is closed within a second.
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

    /// Why a deployment asking for `manifest` can't run here, if it can't: a feed it listens to
    /// that isn't bound.
    pub(crate) fn refuses(&self, deployment: &str, manifest: &Manifest) -> Option<String> {
        manifest.capabilities.iter().filter(|c| !manifest.requirements.optional.contains(*c)).find_map(|item| {
            let name = item.strip_prefix("FeedEvent(\"").and_then(|r| r.strip_suffix("\")"))?;
            self.resolve(deployment, name).is_none().then(|| {
                format!("the feed {name:?} isn't bound on this server: an admin binds it (holon feed add {name} --url env:… or --database NAME --channel CHANNEL, with --for {deployment} for this deployment only)")
            })
        })
    }

    // ── what each connection is doing ──

    /// Set what `slug`'s connection to `feed` is doing.
    pub(crate) fn set_live(&self, slug: &str, feed: &str, change: impl FnOnce(&mut Live)) {
        let mut live = self.0.live.lock().unwrap();
        let l = live.entry((slug.to_string(), feed.to_string())).or_insert_with(|| Live { status: "connecting", since: now_ms(), ..Live::default() });
        change(l);
    }

    /// Keep where `slug`'s run whose entry is `seq` moved its feed once the sync that covers the
    /// entry is done ([`Feeds::release`]): a lazy run's.
    pub(crate) fn hold(&self, slug: &str, seq: u64, keep: Moved) {
        self.0.held.lock().unwrap().entry(slug.to_string()).or_default().push_back((seq, keep));
    }

    /// Forget where `slug`'s lazy runs moved its feeds: it is deleted.
    pub(crate) fn forget_held(&self, slug: &str) {
        self.0.held.lock().unwrap().remove(slug);
    }

    /// Keep where `slug`'s runs whose entries are before `synced`, which are on disk, moved their
    /// feeds, in its log's order.
    pub(crate) fn release(&self, slug: &str, synced: u64) {
        let ready: Vec<Moved> = {
            let mut held = self.0.held.lock().unwrap();
            let Some(q) = held.get_mut(slug) else { return };
            let mut ready = Vec::new();
            while let Some((_, keep)) = q.pop_front_if(|(seq, _)| *seq < synced) {
                ready.push(keep);
            }
            ready
        };
        for keep in ready {
            keep();
        }
    }

    /// Forget `slug`'s connection to `feed`: it is closed.
    pub(crate) fn forget_live(&self, slug: &str, feed: &str) {
        self.0.live.lock().unwrap().remove(&(slug.to_string(), feed.to_string()));
    }

    /// What `slug`'s connection to `feed` is doing, if it has one.
    pub(crate) fn live(&self, slug: &str, feed: &str) -> Option<Live> {
        self.0.live.lock().unwrap().get(&(slug.to_string(), feed.to_string())).cloned()
    }
}

/// An open feed: server-sent events, or a WebSocket or a Postgres channel with the id of the last
/// message it read.
pub(crate) enum Link {
    Sse(sse::Connection),
    Ws(Box<ws::Connection>, String),
    Pg(Box<listen::Connection>, String),
}

/// What a feed's connection read: an event, one passed over (past 1 MiB), or a WebSocket's or a
/// channel's message its binding's `--only` doesn't run, with its number (`--sequence`), which
/// counts though it doesn't run.
pub(crate) enum Incoming {
    Event(Event),
    PassedOver,
    Filtered(Option<(String, u64)>),
}

/// An event of either kind of feed, as a run takes it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Event {
    /// A value when it is JSON, else its text; a binary WebSocket message's bytes.
    pub(crate) data: Value,
    /// An SSE event's name, `message` unless it gives one.
    pub(crate) name: String,
    /// Its own id, when it has one.
    pub(crate) id: Option<String>,
    /// Where the feed is once it is read: what a reconnect sends as `Last-Event-ID`. Empty:
    /// nowhere the feed said.
    pub(crate) last_id: String,
    /// The value of the binding's `--key` in it, for `latest`.
    pub(crate) key: Option<String>,
    /// How many were dropped, or replaced, before it (`--on-full`).
    pub(crate) missed: u64,
    pub(crate) replaced: u64,
    /// Its number by the binding's `--sequence`, with the value of its `--key` ("" without one).
    pub(crate) number: Option<(String, u64)>,
    /// How many numbers of its key were skipped before it.
    pub(crate) gap: u64,
}

/// A message's number by its binding's `--sequence`, with its key: none when the binding names no
/// sequence, or the field isn't a whole number.
fn numbered(binding: &Binding, data: &Value) -> Option<(String, u64)> {
    let n = field_text(data, binding.sequence.as_deref()?)?.parse::<u64>().ok()?;
    Some((binding.key.as_ref().and_then(|k| field_text(data, k)).unwrap_or_default(), n))
}

impl Link {
    /// What shuts the connection from another thread.
    pub(crate) fn closer(&self) -> Option<sse::Closer> {
        match self {
            Link::Sse(c) => c.closer(),
            Link::Ws(c, _) => c.closer(),
            Link::Pg(c, _) => Some(c.closer()),
        }
    }

    /// The wait before opening it again the feed asked for, if it asked (`retry:`).
    pub(crate) fn retry(&self) -> Option<Duration> {
        match self {
            Link::Sse(c) => c.retry(),
            Link::Ws(..) | Link::Pg(..) => None,
        }
    }

    /// What it reads next, as `binding` says it is read; None when the feed ended the connection,
    /// or why it was read no further.
    pub(crate) fn next(&mut self, binding: &Binding) -> Result<Option<Incoming>, String> {
        let key = |data: &Value| binding.key.as_ref().and_then(|k| field_text(data, k));
        match self {
            Link::Sse(c) => Ok(c.next_event()?.map(|next| match next {
                sse::Next::PassedOver => Incoming::PassedOver,
                sse::Next::Event(e) => {
                    let data = holon_core::parse_json(&e.data).unwrap_or(Value::Text(e.data));
                    Incoming::Event(Event { key: key(&data), number: numbered(binding, &data), data, name: e.name, id: e.id, last_id: e.last_id, missed: 0, replaced: 0, gap: 0 })
                }
            })),
            Link::Ws(c, last) => Ok(c.next_message()?.map(|said| {
                let data = match said {
                    ws::Said::PassedOver => return Incoming::PassedOver,
                    ws::Said::Text(t) => holon_core::parse_json(&t).unwrap_or(Value::Text(t)),
                    ws::Said::Binary(b) => Value::Bytes(b),
                };
                message(binding, data, "message", last)
            })),
            Link::Pg(c, last) => Ok(c.next_payload()?.map(|payload| {
                let data = holon_core::parse_json(&payload).unwrap_or(Value::Text(payload));
                message(binding, data, binding.channel.as_deref().unwrap_or_default(), last)
            })),
        }
    }
}

/// A WebSocket's or a channel's message, `data`, as `binding` reads it: run or not (`--only`),
/// its id (`--id`), which is where the feed is (`last`) once it has one.
fn message(binding: &Binding, data: Value, name: &str, last: &mut String) -> Incoming {
    let runs = binding.only.iter().all(|(field, values)| field_text(&data, field).is_some_and(|v| values.contains(&v)));
    if !runs {
        return Incoming::Filtered(numbered(binding, &data));
    }
    let parts: Option<Vec<String>> = binding.id.iter().map(|p| field_text(&data, p)).collect();
    let id = parts.filter(|p| !p.is_empty()).map(|p| p.join("/"));
    if let Some(id) = &id {
        last.clone_from(id);
    }
    let key = binding.key.as_ref().and_then(|k| field_text(&data, k));
    Incoming::Event(Event { key, number: numbered(binding, &data), data, name: name.to_string(), id, last_id: last.clone(), missed: 0, replaced: 0, gap: 0 })
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// `GET /deployments/<slug>/feeds`: each feed its handlers listen to, and what its connection is
/// doing.
pub(crate) fn handle_get_feeds(state: &AppState, slug: &str) -> Response {
    let listened: Vec<(String, String)> = match state.deployments.lock().unwrap().get(slug) {
        Some(d) => d.server.as_ref().map(|s| s.feeds.iter().map(|f| (f.feed.clone(), f.label.clone())).collect()).unwrap_or_default(),
        None => return err(404, format!("no deployment {slug:?}")),
    };
    let feeds = &state.folders.feeds;
    let shown: Vec<serde_json::Value> = listened
        .into_iter()
        .map(|(feed, label)| {
            let mut shown = serde_json::json!({ "feed": feed, "label": label, "status": "closed", "waiting": 0 });
            if let Some(address) = feeds.resolve(slug, &feed).map(|b| b.seen_by(&state.folders.databases, slug).address) {
                shown["address"] = serde_json::json!(address);
            }
            if let Some(live) = feeds.live(slug, &feed) {
                let mut l = serde_json::to_value(&live).unwrap_or_default();
                l["feed"] = serde_json::json!(feed);
                l["label"] = serde_json::json!(label);
                shown = l;
            }
            shown
        })
        .collect();
    Response::json(&serde_json::json!({ "feeds": shown }))
}

// ── admin ──

fn shown(name: &str, deployment: Option<&str>, b: &Binding) -> serde_json::Value {
    let mut shown = serde_json::json!({ "name": name, "deployment": deployment, "url": b.url, "address": b.address, "headers": b.headers, "buffer": b.buffer, "idle": b.idle, "on_full": b.on_full });
    let extra = serde_json::json!({ "key": b.key, "sequence": b.sequence, "subscribe": b.subscribe, "heartbeat": b.heartbeat, "every": b.every, "id": b.id, "only": b.only, "resume": b.resume, "database": b.database, "channel": b.channel });
    for (k, v) in extra.as_object().into_iter().flatten() {
        let empty = v.is_null() || v.as_array().is_some_and(Vec::is_empty) || v.as_object().is_some_and(serde_json::Map::is_empty) || v.as_u64() == Some(0);
        if !empty {
            shown[k] = v.clone();
        }
    }
    shown
}

fn labelled(name: &str, deployment: Option<&str>) -> String {
    deployment.map_or_else(|| name.to_string(), |d| format!("{name} (for {d})"))
}

/// `/admin/feeds` (list, bind), `/admin/feeds/<name>` (unbind; `?deployment=<slug>` for a
/// deployment's own), or None for any other path. Only an admin gets this far (can.rs); what
/// they do goes in the activity log.
pub(crate) fn route(state: &AppState, request: &Request, by: &str) -> Option<Response> {
    let url = request.url();
    let method = request.method();
    let all = &state.folders.feeds;
    if url == "/admin/feeds" {
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
                        let database = given(&b.asked.database).and_then(|db| state.folders.databases.resolve(b.deployment.as_deref().unwrap_or_default(), db));
                        let f = all.bind(&b.name, &b.asked, database.as_ref(), b.deployment.as_deref()).map_err(|e| err(400, e))?;
                        Ok((b, f))
                    });
                match bound {
                    Ok((b, f)) => {
                        state.activity.record("bound the feed", &labelled(&b.name, b.deployment.as_deref()), Some(by));
                        if let Some(slug) = &b.deployment {
                            let detail = crate::events::Detail { name: Some(b.name.clone()), ..Default::default() };
                            crate::events::note(&state.deployments, slug, "binding", format!("bound the feed {}", b.name), Some(by), detail);
                        }
                        Response::json(&shown(&b.name, b.deployment.as_deref(), &f))
                    }
                    Err(e) => e,
                }
            }
            _ => Response::empty_404(),
        });
    }
    let name = url.strip_prefix("/admin/feeds/")?;
    if method != "DELETE" {
        return Some(Response::empty_404());
    }
    let deployment = request.get_param("deployment").filter(|d| !d.is_empty());
    Some(match all.unbind(name, deployment.as_deref()) {
        Ok(true) => {
            state.activity.record("unbound the feed", &labelled(name, deployment.as_deref()), Some(by));
            if let Some(slug) = &deployment {
                let detail = crate::events::Detail { name: Some(name.to_string()), ..Default::default() };
                crate::events::note(&state.deployments, slug, "binding", format!("unbound the feed {name}"), Some(by), detail);
            }
            Response::empty_204()
        }
        Ok(false) => err(404, format!("no feed {name:?} is bound{}", deployment.map_or(String::new(), |d| format!(" for {d}")))),
        Err(e) => err(500, e),
    })
}

/// `holon feed add NAME --url SOURCE [--header NAME=SOURCE]… [--buffer N] [--on-full …] [--key
/// FIELD] [--sequence FIELD] [--idle T] [--subscribe MESSAGE]… [--heartbeat MESSAGE] [--id
/// FIELD,…] [--only FIELD=VALUE]… [--resume WHERE]… [--for SLUG] | add NAME --database NAME
/// --channel CHANNEL […] | list | remove NAME`: bind, show and unbind the feeds of the server this
/// user runs here (or `--server`), as an admin.
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut server = None;
    let mut for_deployment: Option<String> = None;
    let mut asked = serde_json::Map::new();
    let mut headers = serde_json::Map::new();
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--for" => for_deployment = args.next().cloned(),
            "--url" | "--buffer" | "--idle" | "--header" | "--on-full" | "--key" | "--sequence" | "--subscribe" | "--heartbeat" | "--every" | "--id" | "--only" | "--resume" | "--database" | "--channel" => {
                let Some(v) = args.next() else {
                    eprintln!("holon feed: {a} takes a value\n\n{FEED_USAGE}");
                    return 2;
                };
                match a.as_str() {
                    "--header" => {
                        let Some((name, source)) = v.split_once('=') else {
                            eprintln!("holon feed: --header is NAME=SOURCE (Authorization=env:HOLON_SECRET_PRICES_AUTH)\n\n{FEED_USAGE}");
                            return 2;
                        };
                        headers.insert(name.trim().to_string(), serde_json::Value::String(source.trim().to_string()));
                    }
                    "--buffer" => {
                        let Ok(n) = v.parse::<u64>() else {
                            eprintln!("holon feed: --buffer is how many events may wait: a number\n\n{FEED_USAGE}");
                            return 2;
                        };
                        asked.insert("buffer".into(), serde_json::json!(n));
                    }
                    "--subscribe" | "--only" | "--resume" => {
                        let list = asked.entry(a[2..].to_string()).or_insert_with(|| serde_json::json!([]));
                        if let Some(list) = list.as_array_mut() {
                            list.push(serde_json::Value::String(v.clone()));
                        }
                    }
                    _ => {
                        asked.insert(a[2..].replace('-', "_"), serde_json::Value::String(v.clone()));
                    }
                }
            }
            "--help" | "-h" => {
                println!("{FEED_USAGE}");
                return 0;
            }
            _ if !a.starts_with('-') => words.push(a.as_str()),
            _ => {
                eprintln!("holon feed: no option {a:?}\n\n{FEED_USAGE}");
                return 2;
            }
        }
    }
    let (method, path, body) = match words.as_slice() {
        ["list"] => ("GET", "/admin/feeds".to_string(), None),
        ["add", name] => {
            if !asked.contains_key("url") && !asked.contains_key("database") {
                eprintln!("holon feed add: it takes --url SOURCE, where the server reads the feed's URL, or --database NAME --channel CHANNEL\n\n{FEED_USAGE}");
                return 2;
            }
            asked.insert("name".into(), serde_json::Value::String(name.to_string()));
            asked.insert("headers".into(), serde_json::Value::Object(headers));
            asked.insert("deployment".into(), serde_json::json!(for_deployment));
            ("POST", "/admin/feeds".to_string(), Some(serde_json::Value::Object(asked)))
        }
        ["remove", name] => {
            let only = for_deployment.as_ref().map_or(String::new(), |d| format!("?deployment={}", crate::mcp::seg(d)));
            ("DELETE", format!("/admin/feeds/{}{only}", crate::mcp::seg(name)), None)
        }
        _ => {
            eprintln!("{FEED_USAGE}");
            return 2;
        }
    };
    let (server, answer) = match local::admin_call("holon feed", server.as_deref(), method, &path, body.as_ref()) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let line = |f: &serde_json::Value| {
        let headers: Vec<String> = f["headers"].as_object().into_iter().flatten().map(|(h, s)| format!("{h} from {}", s.as_str().unwrap_or(""))).collect();
        let reads = match f["database"].as_str() {
            Some(db) => format!("listens on {} (the database {db})", f["address"].as_str().unwrap_or("")),
            None => format!("reads {} (from {})", f["address"].as_str().unwrap_or(""), f["url"].as_str().unwrap_or("")),
        };
        let mut said = vec![f["name"].as_str().unwrap_or("").to_string(), reads];
        if !headers.is_empty() {
            said.push(format!("sends {}", headers.join(", ")));
        }
        let texts = |k: &str| -> Vec<String> { f[k].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect() };
        let subscribe = texts("subscribe");
        if !subscribe.is_empty() {
            said.push(format!("subscribes with {}", subscribe.join(", ")));
        }
        if let Some(beat) = f["heartbeat"].as_str() {
            said.push(format!("sends {beat} every {}", folders::span_text(f["every"].as_u64().unwrap_or(EVERY))));
        }
        let only: Vec<String> = f["only"].as_object().into_iter().flatten().map(|(k, vs)| format!("{k} {}", vs.as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" or "))).collect();
        if !only.is_empty() {
            said.push(format!("runs only {}", only.join(" and ")));
        }
        let id = texts("id");
        if !id.is_empty() {
            said.push(format!("its id {}", id.join("/")));
        }
        if let Some(sequence) = f["sequence"].as_str() {
            said.push(format!("numbered by {sequence}{}", f["key"].as_str().map_or(String::new(), |k| format!(", each {k} on its own"))));
        }
        let resume = &f["resume"];
        if let Some(name) = resume["query"].as_str() {
            said.push(format!("resumes from its position with {name}= in the URL"));
        } else if let Some(name) = resume["header"].as_str() {
            said.push(format!("resumes from its position with the header {name}"));
        } else if let Some(messages) = resume["message"].as_array() {
            said.push(format!("resumes from its position with {}", messages.iter().filter_map(|m| m.as_str()).collect::<Vec<_>>().join(", ")));
        }
        let full = match (f["on_full"].as_str(), f["key"].as_str()) {
            (Some("skip"), _) => ", the rest dropped".to_string(),
            (Some("latest"), Some(key)) => format!(", one a {key}, the newest"),
            _ => String::new(),
        };
        said.push(format!("{} may wait{full}  quiet for {} at most", f["buffer"].as_u64().unwrap_or(BUFFER as u64), folders::span_text(f["idle"].as_u64().unwrap_or(IDLE))));
        format!("{}{}", said.join("  "), f["deployment"].as_str().map_or(String::new(), |s| format!(", for {s} only")))
    };
    match words[0] {
        "list" => {
            let all: Vec<serde_json::Value> = serde_json::from_str(&answer).unwrap_or_default();
            if all.is_empty() {
                println!("no feeds are bound on {server}: holon feed add NAME --url env:… (or --database NAME --channel CHANNEL) binds one");
            }
            for f in &all {
                println!("{}", line(f));
            }
        }
        "add" => println!("bound {}", line(&serde_json::from_str(&answer).unwrap_or_default())),
        _ => println!("unbound {}{}: its connections close", words[1], for_deployment.as_ref().map_or(String::new(), |d| format!(" (for {d})"))),
    }
    0
}

const FEED_USAGE: &str = "usage: holon feed add NAME --url SOURCE [--header NAME=SOURCE]… [--buffer N]
                       [--on-full wait|skip|latest] [--key FIELD] [--sequence FIELD]
                       [--idle T] [--for SLUG]
                       [--subscribe MESSAGE]… [--heartbeat MESSAGE [--every T]]
                       [--id FIELD[,FIELD]…] [--only FIELD=VALUE]…
                       [--resume query:NAME|header:NAME|message:MESSAGE…]
       holon feed add NAME --database NAME --channel CHANNEL [--id FIELD[,FIELD]…]
                       [--only FIELD=VALUE]… [--buffer N] [--on-full …] [--key FIELD]
                       [--sequence FIELD] [--idle T] [--for SLUG]
       holon feed list
       holon feed remove NAME [--for SLUG]
       (each with [--server PORT|URL])

  Binds a name a flow imports (import prices from \"feed:prices\") to a feed of server-sent
  events, a WebSocket or a Postgres channel, whose events start its runs (on.feed(prices, (e) =>
  …)), one at a time, in the order they came. A flow never sees the feed's URL or its headers.
  Admins.

  add NAME                 bind NAME, once a request to it answers as a feed does
    --url SOURCE           where the server reads the feed's URL, as it reads a secret:
                           env:HOLON_SECRET_PRICES_URL (its environment or --dotenv file) or
                           vault:<mount>/<path>#<field>. The URL is never stored, shown or
                           logged: https://stream.example.com/v1/prices?key=…, a login in it
                           sent as Basic authentication; wss://… for a WebSocket; plain http://
                           and ws:// only to this machine
    --header NAME=SOURCE   a header each request sends, its value read from a source:
                           Authorization=env:HOLON_SECRET_PRICES_AUTH (again for another)
    --buffer N             how many events may wait for their run (1000)
    --on-full WHAT         what comes while that many wait: wait (the connection waits too, as
                           unless given), skip (it is dropped, and the next event's e.missed
                           says how many were), latest (it replaces the waiting event of its
                           --key, full or not, its e.replaced saying how many it did)
    --key FIELD            the field that is a message's key, for latest (one event of a key
                           waits) and --sequence (each key numbered on its own): product_id
    --sequence FIELD       for a feed whose JSON messages a whole number counts one by one, of
                           each --key: each event's e.gap says how many numbers were skipped
                           before it, across a reconnect or a restart: trade_id
    --idle T               how long the feed may say nothing before the connection is opened
                           again: 90s, 2m (5s at least)
    --for SLUG             bind it for that deployment only
  A WebSocket's:
    --subscribe MESSAGE    sent each time it opens, as written or from a source (env:…), in
                           order: {\"type\":\"subscribe\",\"channels\":[\"ticker\"]}
    --heartbeat MESSAGE    sent every --every T (30s) while it is open, for a feed that drops a
                           client that says nothing
    --id FIELD[,FIELD]     the fields of a JSON message that are its id: product_id,sequence
                           gives BTC-USD/41; a dotted path for a field in a field (data.u)
    --only FIELD=VALUE     run only the messages whose field has that value (again for another
                           value, or field); the rest, a subscription's answer, are passed over
    --resume WHERE         for a feed that replays: where it takes the id of the message the
                           host is at (an --id of one field) when a connection opens again:
                           query:cursor (cursor=<id> in the URL), header:NAME, or message:MESSAGE
                           sent in place of --subscribe, {position} in it the id (again for
                           another); a new listener reads from now
  A Postgres channel's (--id and --only as a WebSocket's):
    --database NAME        the database it is on, as bound (holon database add): the one the
                           listening deployment sees by that name, a Postgres
    --channel CHANNEL      the channel listened on (LISTEN): what NOTIFY or pg_notify sends to,
                           letters, digits and _; each notification's payload is an event. What
                           is sent while no connection listens is lost: --sequence counts it
  list                     the names bound, and where each reads
  remove NAME              unbind it (--for SLUG: that deployment's); its connections close
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon feed add prices --url env:HOLON_SECRET_PRICES_URL
  holon feed add prices --url env:HOLON_SECRET_PRICES_URL --header Authorization=env:HOLON_SECRET_PRICES_AUTH --idle 2m
  holon feed add ticker --url env:HOLON_SECRET_TICKER_URL --subscribe '{\"type\":\"subscribe\",\"channels\":[\"ticker\"],\"product_ids\":[\"BTC-USD\"]}' --only type=ticker --id product_id,sequence --on-full latest --key product_id
  holon feed add posts --url env:HOLON_SECRET_POSTS_URL --id payload.seq --resume query:cursor --only payload.operation=create
  holon feed add trades --url env:HOLON_SECRET_TRADES_URL --subscribe '{\"type\":\"subscribe\",\"channels\":[\"matches\"],\"product_ids\":[\"BTC-USD\"]}' --only type=match --only type=last_match --id product_id,trade_id --sequence trade_id --key product_id
  holon feed add entries --database ledger --channel ledger_entries --id id
  holon feed list

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::stand_in::StandIn;

    fn feeds(vars: &[(&str, &str)]) -> Feeds {
        Feeds::memory().with_sources(Sources::of(vars))
    }

    fn asked(url: &str) -> Asked {
        Asked { url: Some(url.into()), ..Asked::default() }
    }

    #[test]
    fn a_feed_is_bound_once_it_answers_as_a_feed_and_shown_without_its_query() {
        let feed = StandIn::start();
        let url = feed.url("/events?key=s3cret");
        let all = feeds(&[("HOLON_SECRET_PRICES_URL", &url), ("HOLON_SECRET_PRICES_AUTH", "Bearer t0ken")]);
        let mut a = asked("env:HOLON_SECRET_PRICES_URL");
        a.headers.insert("Authorization".into(), "env:HOLON_SECRET_PRICES_AUTH".into());
        a.idle = Some("2m".into());
        let b = all.bind("prices", &a, None, None).unwrap();
        assert_eq!(b.address, url.split('?').next().unwrap());
        assert_eq!((b.buffer, b.idle), (BUFFER, 120));
        assert_eq!(b.headers["Authorization"], "env:HOLON_SECRET_PRICES_AUTH");
        assert!(feed.asked()[0].contains("Authorization: Bearer t0ken"));
        assert_eq!(all.resolve("app", "prices"), Some(b.clone()));
        // A deployment's own is its own.
        all.bind("prices", &Asked { buffer: Some(5), ..asked("env:HOLON_SECRET_PRICES_URL") }, None, Some("app")).unwrap();
        assert_eq!(all.resolve("app", "prices").unwrap().buffer, 5);
        assert_eq!(all.resolve("other", "prices"), Some(b));
        assert!(all.unbind("prices", Some("app")).unwrap());
        assert!(!all.unbind("prices", Some("app")).unwrap());
    }

    #[test]
    fn a_binding_is_refused_for_what_isnt_a_feed() {
        let feed = StandIn::start();
        let all = feeds(&[
            ("HOLON_SECRET_PLAIN", &feed.url("/plain")),
            ("HOLON_SECRET_GONE", &feed.url("/gone")),
            ("HOLON_SECRET_REMOTE", "http://stream.example.com/e?key=s3cret"),
            ("HOLON_SECRET_OK", &feed.url("/events")),
        ]);
        let refused = |a: &Asked| all.bind("prices", a, None, None).unwrap_err();
        assert!(refused(&asked("env:HOLON_SECRET_PLAIN")).ends_with("answered with text/plain, not text/event-stream"));
        assert!(refused(&asked("env:HOLON_SECRET_GONE")).ends_with("/gone answered 404"));
        let remote = refused(&asked("env:HOLON_SECRET_REMOTE"));
        assert!(remote.contains("plain http:// only to this machine") && !remote.contains("s3cret"), "{remote}");
        assert!(refused(&asked("https://stream.example.com/e")).starts_with("a feed's URL is read from a source"));
        assert!(refused(&asked("env:HOLON_SECRET_MISSING")).contains("HOLON_SECRET_MISSING"));
        let mut a = asked("env:HOLON_SECRET_OK");
        a.headers.insert("Authorization".into(), "env:HOLON_SECRET_NOT_THERE".into());
        assert!(refused(&a).starts_with("the header Authorization: "), "{}", refused(&a));
        a.headers = BTreeMap::from([("Authorization".into(), "Bearer x".into())]);
        assert!(refused(&a).contains("is read from a source"));
        assert!(refused(&Asked { idle: Some("1s".into()), ..asked("env:HOLON_SECRET_OK") }).starts_with("--idle is 5s at least"));
        assert!(refused(&Asked { buffer: Some(0), ..asked("env:HOLON_SECRET_OK") }).starts_with("--buffer is how many events may wait"));
        assert!(all.bind("pri-ces", &asked("env:HOLON_SECRET_OK"), None, None).unwrap_err().contains("isn't a feed's name"));
        assert_eq!(all.all(), Bound::default());
    }

    fn until(what: impl Fn() -> bool) {
        let by = std::time::Instant::now() + Duration::from_secs(5);
        while !what() {
            assert!(std::time::Instant::now() < by, "it didn't come");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_websocket_is_bound_with_its_options_and_read_as_they_say() {
        let feed = ws::stand_in::StandIn::start();
        let url = feed.url("/ws?key=s3cret");
        let hello = "{\"type\":\"subscribe\",\"key\":\"k3y\"}";
        let all = feeds(&[("HOLON_SECRET_TICKER_URL", &url), ("HOLON_SECRET_TICKER_HELLO", hello)]);
        let channels = "{\"type\":\"subscribe\",\"channels\":[\"ticker\"]}";
        let a = Asked {
            subscribe: vec!["env:HOLON_SECRET_TICKER_HELLO".into(), channels.into()],
            heartbeat: Some("ping".into()),
            id: Some("product_id, sequence".into()),
            only: vec!["type=ticker".into(), "type=snapshot".into()],
            on_full: Some("latest".into()),
            key: Some("product_id".into()),
            ..asked("env:HOLON_SECRET_TICKER_URL")
        };
        let b = all.bind("ticker", &a, None, None).unwrap();
        assert_eq!(b.address, url.split('?').next().unwrap());
        assert_eq!((b.every, b.id.clone(), b.on_full, b.key.as_deref()), (EVERY, vec!["product_id".to_string(), "sequence".into()], OnFull::Latest, Some("product_id")));
        assert_eq!(b.only["type"], vec!["ticker".to_string(), "snapshot".into()]);
        assert_eq!(b.subscribe[0], "env:HOLON_SECRET_TICKER_HELLO", "the source is kept, not what it holds");
        assert!(feed.heard().is_empty(), "the check sends no subscription");
        let shown = shown("ticker", None, &b).to_string();
        assert!(!shown.contains("s3cret") && !shown.contains("k3y") && shown.contains("\"on_full\":\"latest\""), "{shown}");
        // A connection sends its subscription, the source read, then runs what --only says.
        let mut link = all.connect(&b, None).unwrap();
        until(|| feed.heard().len() == 2);
        assert_eq!(feed.heard(), vec![hello.to_string(), channels.into()]);
        feed.text("{\"type\":\"subscriptions\"}");
        feed.text("{\"type\":\"ticker\",\"product_id\":\"BTC-USD\",\"sequence\":41,\"price\":\"100.5\"}");
        feed.text("{\"type\":\"ticker\",\"price\":\"7\"}");
        assert!(matches!(link.next(&b), Ok(Some(Incoming::Filtered(None)))));
        let Ok(Some(Incoming::Event(e))) = link.next(&b) else { panic!("an event") };
        assert_eq!((e.id.as_deref(), e.last_id.as_str(), e.key.as_deref(), e.name.as_str()), (Some("BTC-USD/41"), "BTC-USD/41", Some("BTC-USD"), "message"));
        assert_eq!(field_text(&e.data, "price").as_deref(), Some("100.5"));
        let Ok(Some(Incoming::Event(e))) = link.next(&b) else { panic!("an event") };
        assert_eq!((e.id, e.last_id.as_str(), e.key), (None, "BTC-USD/41", None), "no id of its own: the feed is where it was");
    }

    #[test]
    fn a_websockets_options_are_refused_where_they_dont_fit() {
        let (sse, ws) = (StandIn::start(), ws::stand_in::StandIn::start());
        let all = feeds(&[("HOLON_SECRET_SSE", &sse.url("/events")), ("HOLON_SECRET_WS", &ws.url("/ws"))]);
        let refused = |a: Asked| all.bind("prices", &a, None, None).unwrap_err();
        let on_sse = refused(Asked { subscribe: vec!["hi".into()], ..asked("env:HOLON_SECRET_SSE") });
        assert!(on_sse.contains("are a WebSocket's"), "{on_sse}");
        assert!(refused(Asked { only: vec!["type=ticker".into()], ..asked("env:HOLON_SECRET_SSE") }).contains("are a WebSocket's"));
        assert!(refused(Asked { on_full: Some("latest".into()), ..asked("env:HOLON_SECRET_WS") }).contains("it takes --key FIELD"));
        assert!(refused(Asked { key: Some("product_id".into()), ..asked("env:HOLON_SECRET_WS") }).starts_with("--key is a message's key for --on-full latest"));
        assert!(refused(Asked { sequence: Some("trade id".into()), ..asked("env:HOLON_SECRET_WS") }).starts_with("--sequence names a field"));
        assert!(refused(Asked { on_full: Some("drop".into()), ..asked("env:HOLON_SECRET_WS") }).starts_with("--on-full is wait, skip or latest"));
        assert!(refused(Asked { id: Some("data..u".into()), ..asked("env:HOLON_SECRET_WS") }).starts_with("--id names a field"));
        assert!(refused(Asked { only: vec!["type".into()], ..asked("env:HOLON_SECRET_WS") }).starts_with("--only is FIELD=VALUE"));
        assert!(refused(Asked { every: Some("10s".into()), ..asked("env:HOLON_SECRET_WS") }).contains("it takes --heartbeat"));
        assert!(refused(Asked { heartbeat: Some("ping".into()), every: Some("2h".into()), ..asked("env:HOLON_SECRET_WS") }).starts_with("--every is how often"));
        // Any feed's: an SSE binding skips or keeps the latest.
        let b = all.bind("prices", &Asked { on_full: Some("latest".into()), key: Some("data.symbol".into()), ..asked("env:HOLON_SECRET_SSE") }, None, None).unwrap();
        assert_eq!((b.on_full, b.key.as_deref()), (OnFull::Latest, Some("data.symbol")));
    }

    #[test]
    fn a_websocket_is_resumed_where_its_binding_says() {
        let feed = ws::stand_in::StandIn::start();
        let all = feeds(&[("HOLON_SECRET_POSTS_URL", &feed.url("/subscribe?cursor=1&key=s3cret")), ("HOLON_SECRET_HELLO", "{\"op\":\"hello\",\"from\":{position}}")]);
        let bind = |resume: &[&str], subscribe: &[&str]| {
            let a = Asked { id: Some("seq".into()), resume: resume.iter().map(|s| s.to_string()).collect(), subscribe: subscribe.iter().map(|s| s.to_string()).collect(), ..asked("env:HOLON_SECRET_POSTS_URL") };
            all.bind("posts", &a, None, None).unwrap()
        };
        let asked = |link: Link| {
            drop(link);
            feed.asked().last().unwrap().clone()
        };
        // In the query: none for a new listener; the id in place of the URL's own cursor.
        let b = bind(&["query:cursor"], &[]);
        assert_eq!(b.resume, Some(Resume::Query("cursor".into())));
        assert!(asked(all.connect(&b, None).unwrap()).starts_with("/subscribe?cursor=1&key=s3cret"));
        let at = asked(all.connect(&b, Some("26984429105")).unwrap());
        assert!(at.starts_with("/subscribe?key=s3cret&cursor=26984429105\n"), "{at}");
        // In a header.
        let b = bind(&["header:X-From"], &[]);
        assert!(asked(all.connect(&b, Some("41")).unwrap()).contains("\nx-from: 41"));
        // In the messages sent in place of the subscription, a source read.
        let b = bind(&["message:{\"op\":\"subscribe\",\"from\":{position}}", "message:env:HOLON_SECRET_HELLO"], &["{\"op\":\"subscribe\"}"]);
        let link = all.connect(&b, None).unwrap();
        until(|| feed.heard().len() == 1);
        drop(link);
        let link = all.connect(&b, Some("41")).unwrap();
        until(|| feed.heard().len() == 3);
        drop(link);
        assert_eq!(feed.heard(), ["{\"op\":\"subscribe\"}", "{\"op\":\"subscribe\",\"from\":41}", "{\"op\":\"hello\",\"from\":41}"]);
        let shown = shown("posts", None, &b).to_string();
        assert!(shown.contains("\"resume\":{\"message\":[") && shown.contains("env:HOLON_SECRET_HELLO") && !shown.contains("s3cret"), "{shown}");
        // A feed that replays from the cursor sends its message again: the stand-in does.
        let b = bind(&["query:cursor"], &[]);
        for seq in 1..=3 {
            feed.text(&format!("{{\"seq\":{seq}}}"));
        }
        let mut link = all.connect(&b, Some("2")).unwrap();
        let ids: Vec<Option<String>> = (0..2).map(|_| match link.next(&b) { Ok(Some(Incoming::Event(e))) => e.id, _ => None }).collect();
        assert_eq!(ids, [Some("2".to_string()), Some("3".into())]);
    }

    #[test]
    fn a_resume_is_refused_where_it_doesnt_fit() {
        let (sse, ws) = (StandIn::start(), ws::stand_in::StandIn::start());
        let all = feeds(&[("HOLON_SECRET_SSE", &sse.url("/events")), ("HOLON_SECRET_WS", &ws.url("/ws"))]);
        let refused = |id: Option<&str>, resume: &[&str], url: &str| {
            let a = Asked { id: id.map(Into::into), resume: resume.iter().map(|s| s.to_string()).collect(), ..asked(url) };
            all.bind("posts", &a, None, None).unwrap_err()
        };
        let ws = "env:HOLON_SECRET_WS";
        assert!(refused(Some("seq"), &["query:cursor"], "env:HOLON_SECRET_SSE").contains("resume with Last-Event-ID"));
        assert!(refused(None, &["query:cursor"], ws).contains("it takes an --id of one field"));
        assert!(refused(Some("product_id,sequence"), &["query:cursor"], ws).contains("it takes an --id of one field"));
        assert!(refused(Some("seq"), &["query:cursor", "header:X-From"], ws).starts_with("--resume is one place"));
        assert!(refused(Some("seq"), &["query:cursor", "message:{position}"], ws).starts_with("--resume is one place"));
        assert!(refused(Some("seq"), &["cursor"], ws).starts_with("--resume is one place"));
        assert!(refused(Some("seq"), &["query:a b"], ws).starts_with("--resume query: names the query"));
        assert!(refused(Some("seq"), &["header:X:Y"], ws).starts_with("--resume header: names the header"));
        assert!(refused(Some("seq"), &["message:{\"from\":1}"], ws).contains("writes {position}"));
        assert_eq!(all.all(), Bound::default());
    }

    #[test]
    fn a_feed_numbers_its_messages_by_its_sequence_each_key_on_its_own() {
        let (sse, ws) = (StandIn::start(), ws::stand_in::StandIn::start());
        let all = feeds(&[("HOLON_SECRET_SSE", &sse.url("/events")), ("HOLON_SECRET_TRADES", &ws.url("/ws"))]);
        // Any feed's whose data is JSON: one of server-sent events too, without a key.
        let b = all.bind("prices", &Asked { sequence: Some("data.n".into()), ..asked("env:HOLON_SECRET_SSE") }, None, None).unwrap();
        assert_eq!((b.sequence.as_deref(), b.key.as_deref()), (Some("data.n"), None));
        let a = Asked { sequence: Some("trade_id".into()), key: Some("product_id".into()), only: vec!["type=match".into()], ..asked("env:HOLON_SECRET_TRADES") };
        let b = all.bind("trades", &a, None, None).unwrap();
        assert!(shown("trades", None, &b).to_string().contains("\"sequence\":\"trade_id\""));
        let mut link = all.connect(&b, None).unwrap();
        until(|| ws.asked().len() == 2);
        ws.text(r#"{"type":"match","product_id":"BTC-USD","trade_id":104}"#);
        ws.text(r#"{"type":"open","product_id":"BTC-USD","trade_id":105}"#);
        ws.text(r#"{"type":"match","product_id":"BTC-USD","trade_id":"106.5"}"#);
        ws.text(r#"{"type":"match","trade_id":7}"#);
        let Ok(Some(Incoming::Event(e))) = link.next(&b) else { panic!("an event") };
        assert_eq!(e.number, Some(("BTC-USD".into(), 104)));
        assert!(matches!(link.next(&b), Ok(Some(Incoming::Filtered(Some((k, 105))))) if k == "BTC-USD"), "passed over, its number counts");
        let Ok(Some(Incoming::Event(e))) = link.next(&b) else { panic!("an event") };
        assert_eq!(e.number, None, "not a whole number");
        let Ok(Some(Incoming::Event(e))) = link.next(&b) else { panic!("an event") };
        assert_eq!(e.number, Some((String::new(), 7)), "no key: a count of its own");
    }

    #[test]
    fn a_channel_is_bound_only_on_a_postgres_and_without_a_urls_options() {
        let all = feeds(&[]);
        let asked = |db: &str, channel: &str| Asked { database: Some(db.into()), channel: Some(channel.into()), ..Asked::default() };
        let sqlite = Database { engine: "sqlite".into(), path: None, connection: None, address: None, access: crate::folders::FolderAccess::Read, lock: Default::default() };
        let refused = |a: &Asked, d: Option<&Database>, slug: Option<&str>| all.bind("entries", a, d, slug).unwrap_err();
        assert!(refused(&asked("ledger", "ledger_entries"), None, Some("app")).contains("the database ledger isn't bound for app"));
        assert!(refused(&asked("notes", "ledger_entries"), Some(&sqlite), None).contains("LISTEN is Postgres's: the database notes is sqlite"));
        assert!(refused(&Asked { channel: None, ..asked("ledger", "") }, Some(&sqlite), None).contains("--database NAME and --channel CHANNEL, both"));
        assert!(refused(&asked("ledger", "ledger entries"), Some(&sqlite), None).contains("--channel is a Postgres channel"));
        for a in [Asked { url: Some("env:X".into()), ..asked("ledger", "c") }, Asked { heartbeat: Some("ping".into()), ..asked("ledger", "c") }, Asked { resume: vec!["query:cursor".into()], ..asked("ledger", "c") }] {
            assert!(refused(&a, Some(&sqlite), None).contains("a channel is listened on in its --database"), "{a:?}");
        }

        // Seen by a deployment: its database's connection and address, or none.
        let postgres = Database { engine: "postgres".into(), connection: Some("env:HOLON_SECRET_LEDGER_URL".into()), address: Some("postgres://db.example.com:5432/ledger".into()), ..sqlite.clone() };
        let b = Binding { url: String::new(), address: String::new(), headers: BTreeMap::new(), buffer: BUFFER, idle: IDLE, on_full: OnFull::Wait, key: None, sequence: None, subscribe: Vec::new(), heartbeat: None, every: 0, id: Vec::new(), only: BTreeMap::new(), resume: None, database: Some("ledger".into()), channel: Some("ledger_entries".into()) };
        let seen = b.clone().on(Some(&postgres));
        assert_eq!((seen.url.as_str(), seen.address.as_str()), ("env:HOLON_SECRET_LEDGER_URL", "postgres://db.example.com:5432/ledger, channel ledger_entries"));
        assert_eq!(b.clone().on(Some(&sqlite)).url, "");
        assert_eq!(b.clone().on(None).url, "");
        let err = all.connect(&b, None).err().unwrap();
        assert!(err.contains("the database ledger isn't a Postgres bound for this deployment"), "{err}");
        // Kept without a URL: its database's is read as each deployment sees it.
        let stored = serde_json::to_value(&b).unwrap();
        assert!(stored.get("url").is_none() && stored["database"] == "ledger" && stored["channel"] == "ledger_entries", "{stored}");
    }

    #[test]
    fn a_field_is_found_by_its_path_and_read_as_text() {
        let data = holon_core::parse_json("{\"data\":{\"u\":41,\"s\":\"BTC\",\"ok\":true,\"l\":[1]}}").unwrap();
        assert_eq!(field_text(&data, "data.u").as_deref(), Some("41"));
        assert_eq!(field_text(&data, "data.s").as_deref(), Some("BTC"));
        assert_eq!(field_text(&data, "data.ok").as_deref(), Some("true"));
        assert_eq!(field_text(&data, "data.l"), None);
        assert_eq!(field_text(&data, "data.none"), None);
        assert_eq!(field_text(&Value::Text("x".into()), "data"), None);
    }

    #[test]
    fn a_deploy_listening_to_a_feed_no_one_bound_is_refused() {
        let feed = StandIn::start();
        let all = feeds(&[("HOLON_SECRET_PRICES_URL", &feed.url("/events"))]);
        let manifest = Manifest { capabilities: ["FeedEvent(\"prices\")".to_string()].into_iter().collect(), requirements: Default::default() };
        assert!(all.refuses("app", &manifest).unwrap().starts_with("the feed \"prices\" isn't bound on this server"));
        all.bind("prices", &asked("env:HOLON_SECRET_PRICES_URL"), None, Some("app")).unwrap();
        assert_eq!(all.refuses("app", &manifest), None);
        assert!(all.refuses("other", &manifest).is_some());
    }
}
