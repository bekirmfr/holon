//! Inbound WebSockets (host-capabilities L4 §5.12.14): a page opens a connection on one of a
//! server's websocket paths, and the server runs the websocket's blocks, `open` on the opening
//! request, `message` on each text message, `close` once the connection has ended. (`ws.rs` is
//! the other direction: a feed the host reads from a WebSocket.)
//!
//! - **The opening request** is a GET asking to upgrade on a websocket's path; one that doesn't ask
//!   is a 426. A page's origin must be the request's own host or one the flow answers pages from
//!   (`browsers`), else 403: a page can open a WebSocket to any site, with its cookies, and CORS
//!   doesn't apply. Past `max_websockets` open connections it is a 503, before `open` runs.
//! - **`open` answers as a route does:** a subscription accepts and subscribes the connection, a
//!   status of 300 or more refuses it with that response, any other answer accepts it and is its
//!   first message, a 204 none. The 101 goes out once the run's record is where its level puts it.
//! - **Each text message** is queued for its connection ([`QUEUE`] at most) and run by `message`,
//!   one at a time, in order; its answer is sent back on the connection. A binary message, one past
//!   `max_message_bytes`, text that isn't UTF-8, more than `messages_per_second` (in bursts of
//!   twice that) or a full queue closes the connection: 1003, 1009, 1007, 1008, 1013.
//! - **Replies and packs** go out as text messages, as the host sends them.
//! - **Pings** every [`PING_EVERY`]; a connection that says nothing for [`SILENT_FOR`] is closed
//!   (1001).
//! - **`close` runs once,** after the connection's last message has run, with how it ended: the
//!   page's code and reason, the host's, or 1006 for a connection that dropped without a close, or
//!   that `open` accepted and the host couldn't hold. Pausing or stopping the deployment closes its
//!   connections (1001), and their `close` runs. A redeploy keeps them; one without a websocket on
//!   their path closes them, as removing the deployment does, with no `close` left to run.
//!
//! The protocol is `tungstenite`'s, without its I/O: what hyper's upgraded connection reads is fed
//! to it through an in-memory [`Pipe`], and what it queues (frames, pongs, the closing handshake)
//! is written back.

use std::collections::VecDeque;
use std::io::{self, Read, Write};

use hyper::http::request::Parts;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tungstenite::error::{Error, ProtocolError};
use tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tungstenite::{Message, WebSocket};

use crate::data::DataResponse;
use crate::*;

/// The messages one connection may have waiting to run.
const QUEUE: usize = 32;

/// How often the host pings a connection.
const PING_EVERY: Duration = Duration::from_secs(15);

/// How long a connection may say nothing, not even a pong, before it is closed.
const SILENT_FOR: Duration = Duration::from_secs(30);

/// How long the host waits for the other side's close once it has sent its own.
const CLOSING_FOR: Duration = Duration::from_secs(5);

/// Limits on a deployment's WebSockets: the host's, like the channel limits.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SocketLimits {
    /// Its connections open at once.
    pub(crate) max_websockets: u64,
    /// The largest message a connection may send, in bytes.
    pub(crate) max_message_bytes: u64,
    /// The messages a connection may send a second, in bursts of twice that.
    pub(crate) messages_per_second: u64,
}

impl SocketLimits {
    /// What a deployment gets without asking.
    pub(crate) fn defaults() -> SocketLimits {
        SocketLimits { max_websockets: 100, max_message_bytes: 64 << 10, messages_per_second: 10 }
    }

    /// What no grant can exceed.
    pub(crate) fn ceilings() -> SocketLimits {
        SocketLimits { max_websockets: 10_000, max_message_bytes: 1 << 20, messages_per_second: 1000 }
    }
}

/// Why the host closes a connection: its close code and reason.
type Closing = (u16, &'static str);

/// What closes a deployment's connections when it is paused, stopped or removed.
pub(crate) const PAUSED: Closing = (1001, "the flow was paused or stopped");
pub(crate) const REMOVED: Closing = (1001, "the flow was removed");

/// A deployment's open connections, by id: kept across redeploys, as its channels are.
#[derive(Default)]
pub(crate) struct Sockets {
    open: Mutex<BTreeMap<String, Open>>,
}

/// One open connection as its deployment holds it: its websocket's path, and how to close it.
struct Open {
    path: String,
    closer: mpsc::UnboundedSender<Closing>,
}

impl Sockets {
    /// A place for the connection `conn` on the websocket `path`, unless `max` are open: what
    /// tells it to close.
    fn take(&self, conn: &str, path: &str, max: u64) -> Option<mpsc::UnboundedReceiver<Closing>> {
        let mut open = self.open.lock().unwrap();
        if open.len() as u64 >= max {
            return None;
        }
        let (closer, closing) = mpsc::unbounded_channel();
        open.insert(conn.to_string(), Open { path: path.to_string(), closer });
        Some(closing)
    }

    /// The connection `conn` is over: refused, or ended and its `close` run.
    fn leave(&self, conn: &str) {
        self.open.lock().unwrap().remove(conn);
    }

    /// Close the connection `conn`, saying `why`.
    fn close(&self, conn: &str, why: Closing) {
        if let Some(open) = self.open.lock().unwrap().get(conn) {
            let _ = open.closer.send(why);
        }
    }

    /// Close every connection, saying `why`.
    pub(crate) fn close_all(&self, why: Closing) {
        for open in self.open.lock().unwrap().values() {
            let _ = open.closer.send(why);
        }
    }

    /// Close the connections on a path `server` has no websocket on: the version deployed in place
    /// of theirs dropped it.
    pub(crate) fn close_gone(&self, server: Option<&Server>) {
        let kept = |path: &str| server.is_some_and(|s| s.websockets.iter().any(|w| w.path == path));
        for open in self.open.lock().unwrap().values().filter(|o| !kept(&o.path)) {
            let _ = open.closer.send((1001, "the flow's new version has no WebSocket here"));
        }
    }

    /// How many are open.
    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.open.lock().unwrap().len()
    }
}

/// One connection, as its runs need it.
struct Conn {
    store: SharedStore,
    deployments: Arc<Mutex<BTreeMap<String, Deployment>>>,
    slug: String,
    /// Its websocket's path as declared (`/room/:name`): each run finds its block by it, in the
    /// version deployed when it runs.
    path: String,
    /// Its id, 16 hex digits: `conn` on every record.
    id: String,
    /// The opening request's URL, its path, and its params and query as records: what every record
    /// carries.
    url: String,
    request_path: String,
    params: Value,
    query: Value,
    sockets: Arc<Sockets>,
}

impl Conn {
    /// A record of this connection, with `fields`: a message's text, or how it ended.
    fn record<const N: usize>(&self, fields: [(&str, Value); N]) -> Value {
        let mut r = BTreeMap::from([
            ("conn".to_string(), Value::Text(self.id.clone())),
            ("path".to_string(), Value::Text(self.request_path.clone())),
            ("params".to_string(), self.params.clone()),
            ("query".to_string(), self.query.clone()),
        ]);
        r.extend(fields.into_iter().map(|(k, v)| (k.to_string(), v)));
        Value::Record(r)
    }

    /// Run its websocket's `on` block with `record`, the block as the version deployed now has it,
    /// and log the run (kind `websocket`, its route the block's label, its request the record):
    /// what it answered, with the host's checks applied, or None when it has no such block. A
    /// `message` runs only while the deployment does; `close` runs whatever its status, the end of
    /// a connection opened while it ran. On the blocking pool: a run may wait on locks and disk.
    ///
    /// `open`'s answer is a route's ([`data::respond`]), its entry's status the handshake's: 101,
    /// or the refusal's. `message`'s is its value alone, as an action's: 200 with the text to send
    /// back, or 204 for nothing.
    fn run(&self, on: &str, record: Value) -> Option<DataResponse> {
        let deployments = &*self.deployments;
        let slug = self.slug.as_str();
        let (block, state, permit, secrets, folders, channels) = {
            let table = deployments.lock().unwrap();
            let dep = table.get(slug)?;
            if on == "message" && !dep.takes_runs(&self.store) {
                return None;
            }
            let websocket = dep.server.as_ref()?.websockets.iter().find(|w| w.path == self.path)?;
            (websocket.block(on)?.clone(), dep.state.clone(), Arc::clone(&dep.permit), Arc::clone(&dep.secrets), Arc::clone(&dep.folders), Arc::clone(&dep.channels))
        };
        let failed_to_start = |why: String| {
            eprintln!("holon: {slug:?} {}: {why}", block.label);
            record_trigger(deployments, slug, true);
            Some(DataResponse::json_error(500, &why))
        };
        let resolver = match closure(&self.store, block.handler) {
            Ok(r) => r,
            Err(e) => return failed_to_start(format!("store error while materializing closure: {e:?}")),
        };
        let Some(handler) = resolver.resolve(&block.handler) else {
            return failed_to_start("the block's handler no longer resolves in the store".into());
        };
        let sourced = [(Capability::Clock, Value::Number(clock_seconds() as f64)), (Capability::Random, random_bytes())];
        let started = started_now();
        let unrecorded = permit.log_level(Some(&block.label), block.log) == holon_core::LogLevel::Unrecorded;
        let mut waits = RunWaits { folders: Some(folders), channels: Some(channels), slug: Some(slug.to_string()), unrecorded, ..Default::default() };
        let resolver = holon_module::WithEngine::new(&resolver, holon_module::shared());
        let inputs = [Some(record)];
        let mut run = fire_with_state(handler, &inputs, &resolver, &self.store, state.as_ref(), &sourced, &permit, false, &secrets, &mut waits, None);

        // An answer carrying a secret goes back only where a person approved it, as a route's
        // (listeners.rs).
        let output = run.outputs.first().and_then(Option::as_ref);
        let mut carried = output.filter(|_| !run.secret_fed.is_empty()).map(|a| secrets::secrets_in(a, &secrets)).unwrap_or_default();
        if run.secret_fed.reaches_output(0) {
            carried.extend(holon_core::answered_secrets(handler, &resolver));
        }
        let unapproved = carried.into_iter().find(|name| !permit.allows(&Capability::SecretAnswer(name.clone())));
        let mut error = match &unapproved {
            Some(name) => Some(format!("refused to answer with the secret {name:?}: this deployment wasn't approved to answer its callers with it")),
            None => run.error.clone(),
        };
        let mut answer = match on {
            "open" => data::respond(output),
            "message" => match reply_of(output) {
                Some(text) => DataResponse { status: 200, headers: Vec::new(), body: text.into_bytes(), subscribe: None, file: None },
                None => DataResponse { status: 204, headers: Vec::new(), body: Vec::new(), subscribe: None, file: None },
            },
            _ => DataResponse { status: 204, headers: Vec::new(), body: Vec::new(), subscribe: None, file: None },
        };
        if error.is_some() {
            answer = DataResponse::json_error(500, "the WebSocket's block failed: its run log says why");
        }
        // Subscribing to a channel it wasn't granted to let callers subscribe to.
        if let Some(s) = answer.subscribe.as_ref().filter(|s| !permit.allows(&Capability::ChannelRead(s.channel.clone()))) {
            let why = format!("this deployment may not let callers subscribe to the channel {:?}", s.channel);
            answer = DataResponse::json_error(500, &why);
            error = Some(why);
        }
        // A file is a route's answer, not a connection's.
        if answer.file.is_some() {
            let why = "a WebSocket's open block answers with a message or a subscription, not a file".to_string();
            answer = DataResponse::json_error(500, &why);
            error = Some(why);
        }
        let failed = error.is_some() || answer.status >= 500;
        let status = match on {
            "open" if answer.status < 300 => Some(101),
            "open" => Some(answer.status),
            _ => failed.then_some(500),
        };
        let body = match on {
            _ if run.secret_fed.derives_output(0) => Some(crate::taint::WITHHELD.to_string()),
            "close" => None,
            _ => Some(recorder::body_text(&answer.body)),
        };
        record_trigger(deployments, slug, failed);
        // Whether an unrecorded run sent packs or mail, which go out even though it isn't logged.
        let sends = unrecorded && run.performed.iter().chain(&run.progress).any(|(c, _)| matches!(c, Capability::ChannelSend(_) | Capability::MailSend(_)));
        let handed = calls::handed(&run.rounds);
        let duration_ms = started.1.elapsed().as_secs_f64() * 1000.0;
        let counted = Counted { route: Some(&block.label), at_ms: started.0, duration_ms, status: status.unwrap_or(200), error: error.as_deref() };
        let entry = || {
            let mut entry = run_entry_taking("websocket", started, &sourced, &mut run);
            entry.duration_ms = duration_ms;
            entry.route = Some(block.label.clone());
            entry.request = inputs[0].as_ref().map(SValue::from_core);
            entry.status = status;
            entry.response_body = body;
            entry.error = error.clone();
            if on == "open" {
                entry.method = Some("GET".to_string());
                entry.url = Some(self.url.clone());
            }
            entry
        };
        let tokens = || Redactor::tokens(&waits.tokens).and(Redactor::handed(&handed));
        // A strict run's answer goes out once its record is on disk; one whose record may not be
        // isn't answered as done (C2 e).
        if let Some(Logged { kept: Err(e), .. }) = log_or_count(deployments, slug, counted, entry, tokens, block.log, sends) {
            eprintln!("holon: {slug:?} {}: the run's record may not be on disk: {e}", block.label);
            return Some(DataResponse::json_error(500, &format!("the block ran, but {e}")));
        }
        Some(answer)
    }

    /// Whether its websocket has an `on` block, in the version deployed now.
    fn has(&self, on: &str) -> bool {
        let table = self.deployments.lock().unwrap();
        let websocket = table.get(&self.slug).and_then(|d| d.server.as_ref()).and_then(|s| s.websockets.iter().find(|w| w.path == self.path));
        websocket.is_some_and(|w| w.block(on).is_some())
    }

    /// Run `close` with how the connection ended, and let its place go.
    fn closed(&self, code: u16, reason: &str) {
        let record = self.record([("code", Value::Number(code as f64)), ("reason", Value::Text(reason.to_string()))]);
        self.run("close", record);
        self.sockets.leave(&self.id);
    }
}

/// What a `message` block's output sends back, as an action's answer is read: its value, or a
/// `respond` envelope's body when its status is 200; text as it is, anything else as JSON. Nothing
/// for no value, a write alone, another status, or a value with no JSON form.
fn reply_of(output: Option<&Value>) -> Option<String> {
    let value = match output? {
        Value::Effect { .. } => return None,
        Value::Record(f) if matches!(f.get("status"), Some(Value::Number(_))) => match f.get("status") {
            Some(Value::Number(n)) if *n == 200.0 => f.get("body")?,
            _ => return None,
        },
        v => v,
    };
    match value {
        Value::Text(t) => Some(t.clone()),
        v => holon_core::stringify_json(v),
    }
}

/// Whether a page loaded from `origin` (`http://localhost:8132`) is on the request's own `host`.
fn same_origin(origin: &str, host: Option<&str>) -> bool {
    let site = origin.strip_prefix("https://").or_else(|| origin.strip_prefix("http://"));
    matches!((site, host), (Some(site), Some(host)) if site.eq_ignore_ascii_case(host))
}

/// The answer to a request on one of `slug`'s websocket paths: the refusal, or the 101 that
/// switches its connection to a WebSocket, served from then on. None for a request on any other
/// path, or not a GET, which the routes answer.
pub(crate) async fn open(store: &SharedStore, deployments: &Arc<Mutex<BTreeMap<String, Deployment>>>, slug: &str, parts: &mut Parts) -> Option<DataResponse> {
    if parts.method != hyper::Method::GET {
        return None;
    }
    let url = parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let (declared, raw_params, status, browsers, limits, sockets, has_open) = {
        let table = deployments.lock().unwrap();
        let dep = table.get(slug)?;
        let (websocket, params) = holon_core::match_websocket(dep.server.as_ref()?, path)?;
        (websocket.path.clone(), params, dep.life.status, dep.browsers.clone(), dep.permit.sockets.clone(), Arc::clone(&dep.sockets), websocket.open.is_some())
    };
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok());
    let lists = |name: &str, token: &str| header(name).is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)));
    let key = header("sec-websocket-key").map(str::to_string);
    let Some(key) = key.filter(|_| lists("upgrade", "websocket") && lists("connection", "upgrade") && header("sec-websocket-version") == Some("13")) else {
        let mut refused = DataResponse::json_error(426, &format!("{declared} is a WebSocket: open it with new WebSocket(…)"));
        refused.headers.extend([("upgrade".to_string(), "websocket".to_string()), ("sec-websocket-version".to_string(), "13".to_string())]);
        return Some(refused);
    };
    let host = header("host").map(str::to_string).or_else(|| parts.uri.authority().map(|a| a.to_string()));
    if let Some(origin) = header("origin")
        && !same_origin(origin, host.as_deref())
        && !browsers.iter().any(|b| b == "*" || b == origin)
    {
        return Some(DataResponse::json_error(403, &format!("a page from {origin} may not open this WebSocket: the flow doesn't answer pages from there (browsers)")));
    }
    if status != Status::Running {
        return Some(DataResponse::json_error(503, &format!("{slug:?} is {}", status_word(status))));
    }
    if store.1.stopping() {
        return Some(DataResponse::json_error(503, stopping::REFUSED));
    }
    let id = format!("{:016x}", u64::from_be_bytes(rand::random::<[u8; 8]>()));
    let Some(closing) = sockets.take(&id, &declared, limits.max_websockets) else {
        return Some(DataResponse::json_error(503, &format!("{slug:?} holds as many WebSocket connections as it may (max_websockets, {})", limits.max_websockets)));
    };
    let params = raw_params.into_iter().map(|(k, v)| (k, data::percent_decode(&v))).collect();
    let headers: Vec<(String, String)> = parts.headers.iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let mut opening = data::request_record("GET", path, params, query, &headers, "");
    let Value::Record(fields) = &mut opening else { unreachable!("a request's record is a record") };
    fields.insert("conn".to_string(), Value::Text(id.clone()));
    let conn = Arc::new(Conn {
        store: store.clone(),
        deployments: Arc::clone(deployments),
        slug: slug.to_string(),
        path: declared,
        id,
        params: fields["params"].clone(),
        query: fields["query"].clone(),
        url: url.clone(),
        request_path: path.to_string(),
        sockets,
    });

    // Without an open block, a connection is accepted as it is.
    let ran = Arc::clone(&conn);
    let answer = if has_open {
        tokio::task::spawn_blocking(move || ran.run("open", opening)).await.unwrap_or_else(|_| Some(DataResponse::json_error(500, "the WebSocket's open block failed")))
    } else {
        None
    };
    let answer = answer.unwrap_or(DataResponse { status: 204, headers: Vec::new(), body: Vec::new(), subscribe: None, file: None });
    let refuse = |answer: DataResponse| {
        conn.sockets.leave(&conn.id);
        Some(answer)
    };
    if answer.status >= 300 {
        return refuse(answer);
    }
    // Accepted, but not held after all (its channel full, the flow gone): close runs as a dropped
    // connection's does (1006, what the page sees too), so what open kept is let go.
    let dropped = |status: u16, why: String| {
        let ran = Arc::clone(&conn);
        async move {
            let answer = DataResponse::json_error(status, &why);
            let _ = tokio::task::spawn_blocking(move || ran.closed(1006, &why)).await;
            Some(answer)
        }
    };
    let subscription = match &answer.subscribe {
        None => None,
        Some(ask) => {
            let channel = deployments.lock().unwrap().get(slug).map(|d| (Arc::clone(&d.channels), d.permit.channels.clone()));
            let Some((channels, channel_limits)) = channel else {
                return dropped(503, format!("{slug:?} is not deployed as a server")).await;
            };
            match channels.subscribe(&ask.channel, ask.kept, None, &channel_limits) {
                Ok(s) => Some(s),
                Err(e) => return dropped(503, e).await,
            }
        }
    };
    let first = (answer.subscribe.is_none() && answer.status != 204).then(|| String::from_utf8_lossy(&answer.body).into_owned());
    let Some(upgrade) = parts.extensions.remove::<hyper::upgrade::OnUpgrade>() else {
        return dropped(500, "this connection can't be switched to a WebSocket".into()).await;
    };
    tokio::spawn(serve(conn, upgrade, closing, first, subscription, limits));
    Some(DataResponse {
        status: 101,
        headers: vec![
            ("upgrade".into(), "websocket".into()),
            ("connection".into(), "Upgrade".into()),
            ("sec-websocket-accept".into(), tungstenite::handshake::derive_accept_key(key.as_bytes())),
        ],
        body: Vec::new(),
        subscribe: None,
        file: None,
    })
}

/// Serve one accepted connection once it is switched: its messages run by `message` in order, as
/// [`work`] runs them, then `close` with how it ended.
async fn serve(conn: Arc<Conn>, upgrade: hyper::upgrade::OnUpgrade, closing: mpsc::UnboundedReceiver<Closing>, first: Option<String>, subscription: Option<channels::Subscription>, limits: SocketLimits) {
    let (queue, queued) = mpsc::channel(QUEUE);
    let (outbox, replies) = mpsc::channel(QUEUE);
    let worker = tokio::spawn(work(Arc::clone(&conn), queued, outbox));
    let (code, reason) = match upgrade.await {
        Ok(upgraded) => talk(TokioIo::new(upgraded), closing, first, subscription, &limits, queue, replies).await,
        Err(_) => (1006, String::new()),
    };
    // The messages that came before its end have run.
    let _ = worker.await;
    let _ = tokio::task::spawn_blocking(move || conn.closed(code, &reason)).await;
}

/// Run each message `message` as it is queued, one at a time, and hand its answer to `outbox`. A
/// websocket without a `message` block takes no messages: one closes its connection (1003).
async fn work(conn: Arc<Conn>, mut queued: mpsc::Receiver<String>, outbox: mpsc::Sender<String>) {
    while let Some(text) = queued.recv().await {
        let ran = Arc::clone(&conn);
        let answer = tokio::task::spawn_blocking(move || {
            if !ran.has("message") {
                ran.sockets.close(&ran.id, (1003, "this WebSocket takes no messages"));
                return None;
            }
            let record = ran.record([("text", Value::Text(text))]);
            ran.run("message", record)
        })
        .await;
        if let Ok(Some(answer)) = answer
            && answer.status == 200
        {
            let _ = outbox.send(String::from_utf8_lossy(&answer.body).into_owned()).await;
        }
    }
}

/// What `tungstenite` reads from and writes to: the bytes read from the connection, and those it
/// queues for it. Reading what isn't there yet would block; once the connection has ended, it
/// reads as ended.
#[derive(Default)]
struct Pipe {
    incoming: VecDeque<u8>,
    outgoing: Vec<u8>,
    ended: bool,
}

impl Read for Pipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.incoming.is_empty() && !self.ended {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.incoming.read(buf)
    }
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.outgoing.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A connection's messages a second: `rate` of them, and bursts of twice that.
struct Bucket {
    rate: f64,
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn new(per_second: u64) -> Bucket {
        let rate = per_second as f64;
        Bucket { rate, tokens: 2.0 * rate, at: Instant::now() }
    }

    /// Whether one more message is within the rate.
    fn take(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f64() * self.rate).min(2.0 * self.rate);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

/// One connection's WebSocket, as the host speaks it: what it reads, queued for `message`; what
/// is sent on it (its first message, `message`'s answers, its subscription's packs, pings); and
/// how it ended, once it has: the code and reason of whichever side closed it first, or 1006.
async fn talk<IO: AsyncRead + AsyncWrite + Unpin>(
    mut io: IO,
    mut closing: mpsc::UnboundedReceiver<Closing>,
    first: Option<String>,
    mut subscription: Option<channels::Subscription>,
    limits: &SocketLimits,
    queue: mpsc::Sender<String>,
    mut replies: mpsc::Receiver<String>,
) -> (u16, String) {
    let most = usize::try_from(limits.max_message_bytes).unwrap_or(usize::MAX);
    let config = WebSocketConfig::default().max_message_size(Some(most)).max_frame_size(Some(most));
    let mut ws = WebSocket::from_raw_socket(Pipe::default(), Role::Server, Some(config));
    let mut bucket = Bucket::new(limits.messages_per_second);
    let mut ping = tokio::time::interval_at(Instant::now() + PING_EVERY, PING_EVERY);
    let mut heard = Instant::now();
    // How it ended, once one side closed it; and, when the host did, until when it waits for the
    // other side's close.
    let mut ended: Option<(u16, String)> = None;
    let mut until: Option<Instant> = None;
    let mut buf = vec![0u8; 16 << 10];
    // The host closes it: its code and reason are how it ended, unless the other side closed it
    // first.
    let close = |ws: &mut WebSocket<Pipe>, ended: &mut Option<(u16, String)>, until: &mut Option<Instant>, (code, reason): (u16, String)| {
        if ended.is_none() {
            let _ = ws.close(Some(CloseFrame { code: code.into(), reason: reason.clone().into() }));
            *ended = Some((code, reason));
            *until = Some(Instant::now() + CLOSING_FOR);
        }
    };
    if let Some(text) = first {
        let _ = ws.send(Message::text(text));
    }
    loop {
        // What is queued goes out; a connection that can't take it has dropped.
        let _ = ws.flush();
        let out = std::mem::take(&mut ws.get_mut().outgoing);
        if !out.is_empty() && (io.write_all(&out).await.is_err() || io.flush().await.is_err()) {
            return ended.unwrap_or((1006, String::new()));
        }
        tokio::select! {
            read = io.read(&mut buf) => {
                match read {
                    Ok(0) | Err(_) => ws.get_mut().ended = true,
                    Ok(n) => ws.get_mut().incoming.extend(&buf[..n]),
                }
                heard = Instant::now();
                loop {
                    match ws.read() {
                        Ok(Message::Text(text)) if ended.is_none() => {
                            let why = if !bucket.take() {
                                Some((1008, format!("more than {} messages a second", limits.messages_per_second)))
                            } else {
                                queue.try_send(text.to_string()).err().map(|_| (1013, "too many messages waiting to run".to_string()))
                            };
                            if let Some(why) = why {
                                close(&mut ws, &mut ended, &mut until, why);
                            }
                        }
                        Ok(Message::Binary(_)) => close(&mut ws, &mut ended, &mut until, (1003, "this WebSocket takes text messages".into())),
                        Ok(Message::Close(frame)) => {
                            let said = frame.map_or((1005, String::new()), |f| (u16::from(f.code), f.reason.to_string()));
                            ended.get_or_insert(said);
                        }
                        Ok(_) => {}
                        Err(Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => break,
                        // What follows a message it can't take can't be read, the other side's close
                        // included: the close is sent, and the connection let go.
                        Err(Error::Capacity(_)) => {
                            close(&mut ws, &mut ended, &mut until, (1009, format!("a message past {most} bytes")));
                            until = Some(Instant::now());
                            break;
                        }
                        Err(Error::Utf8(_)) => {
                            close(&mut ws, &mut ended, &mut until, (1007, "a text message that isn't UTF-8".into()));
                            until = Some(Instant::now());
                            break;
                        }
                        Err(Error::ConnectionClosed | Error::AlreadyClosed | Error::Protocol(ProtocolError::ResetWithoutClosingHandshake)) => {
                            let _ = ws.flush();
                            let out = std::mem::take(&mut ws.get_mut().outgoing);
                            let _ = io.write_all(&out).await;
                            let _ = io.shutdown().await;
                            return ended.unwrap_or((1006, String::new()));
                        }
                        Err(_) => {
                            close(&mut ws, &mut ended, &mut until, (1002, "not the WebSocket protocol".into()));
                            until = Some(Instant::now());
                            break;
                        }
                    }
                }
            }
            Some(reply) = replies.recv() => {
                let _ = ws.send(Message::text(reply));
            }
            pack = async { subscription.as_mut().expect("guarded").packs.recv().await }, if subscription.is_some() => match pack {
                Some(pack) => {
                    let _ = ws.send(Message::text(pack.json.clone()));
                }
                None => {
                    let why = match subscription.take().map(|s| s.end()) {
                        Some(channels::End::Missed) => (1013, "too slow to read its channel's packs".to_string()),
                        _ => (PAUSED.0, PAUSED.1.to_string()),
                    };
                    close(&mut ws, &mut ended, &mut until, why);
                }
            },
            _ = ping.tick() => {
                if heard.elapsed() > SILENT_FOR {
                    close(&mut ws, &mut ended, &mut until, (1001, "nothing heard for 30 s".into()));
                } else {
                    let _ = ws.send(Message::Ping(Default::default()));
                }
            }
            why = closing.recv(), if ended.is_none() => {
                let (code, reason) = why.unwrap_or(REMOVED);
                close(&mut ws, &mut ended, &mut until, (code, reason.to_string()));
            }
            _ = tokio::time::sleep_until(until.unwrap_or_else(Instant::now)), if until.is_some() => {
                let _ = io.shutdown().await;
                return ended.unwrap_or((1006, String::new()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_answers_with_its_value_or_a_200s_body_and_nothing_else() {
        let text = |t: &str| Value::Text(t.into());
        let envelope = |status: f64, body: Value| Value::Record(BTreeMap::from([("status".to_string(), Value::Number(status)), ("body".to_string(), body)]));
        assert_eq!(reply_of(Some(&text("hi"))), Some("hi".into()));
        assert_eq!(reply_of(Some(&Value::Record(BTreeMap::from([("n".to_string(), Value::Number(1.0))])))), Some("{\"n\":1}".into()));
        assert_eq!(reply_of(Some(&envelope(200.0, text("ok")))), Some("ok".into()));
        assert_eq!(reply_of(Some(&envelope(204.0, text("")))), None);
        assert_eq!(reply_of(Some(&envelope(404.0, text("no")))), None);
        assert_eq!(reply_of(None), None);
    }

    #[test]
    fn a_page_on_the_requests_own_host_is_its_own_origin() {
        assert!(same_origin("http://localhost:8132", Some("localhost:8132")));
        assert!(same_origin("https://Chat.example", Some("chat.example")));
        assert!(!same_origin("http://localhost:8132", Some("localhost:8133")));
        assert!(!same_origin("http://evil.example", Some("localhost:8132")));
        assert!(!same_origin("null", Some("localhost:8132")));
        assert!(!same_origin("http://localhost:8132", None));
    }

    #[test]
    fn a_connection_sends_twice_its_rate_at_once_then_its_rate() {
        let mut bucket = Bucket::new(3);
        assert_eq!((0..7).filter(|_| bucket.take()).count(), 6);
    }

    #[test]
    fn its_places_are_its_limit_and_closing_tells_each() {
        let sockets = Sockets::default();
        let mut a = sockets.take("a", "/room/:name", 2).expect("a place");
        let mut b = sockets.take("b", "/feed", 2).expect("a place");
        assert!(sockets.take("c", "/feed", 2).is_none(), "past max_websockets");
        let server = |paths: &[&str]| Server {
            port: Some(1),
            taint: vec![],
            tags: vec![],
            routes: vec![],
            clocks: vec![],
            files: vec![],
            mails: vec![],
            feeds: vec![],
            queues: vec![],
            websockets: paths
                .iter()
                .map(|p| holon_core::WebSocket { path: p.to_string(), segments: vec![], open: None, message: None, close: None })
                .collect(),
            actions: vec![],
        };
        sockets.close_gone(Some(&server(&["/room/:name"])));
        assert_eq!((a.try_recv().ok(), b.try_recv().map(|c| c.0).ok()), (None, Some(1001)));
        sockets.close_all(PAUSED);
        assert_eq!(a.try_recv().ok(), Some(PAUSED));
        sockets.leave("a");
        assert_eq!(sockets.count(), 1);
        assert!(sockets.take("c", "/feed", 2).is_some());
    }
}
