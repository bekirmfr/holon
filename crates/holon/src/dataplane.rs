//! The data plane's HTTP edge (docs/PLATFORM_LAYOUT.md §7b): one listener per deployed server
//! unit, on `hyper` over a shared `tokio` runtime. The control plane stays on `rouille`; this is
//! the side that faces callers nobody vetted, so it holds every connection and request to limits
//! from the first byte:
//!
//! - **Slow clients:** a request's headers must arrive within `header_timeout`, its body within
//!   `body_timeout` (408), or the connection is closed.
//! - **Large bodies:** a body over `max_body_bytes` is refused (413), before it is read when its
//!   `Content-Length` says so.
//! - **Many connections:** at most `max_connections` per listener; more wait in the OS's accept
//!   queue rather than each taking a thread.
//!
//! A request becomes a [`DataRequest`] and is answered by `serve_data` (listeners.rs), which runs
//! the flow synchronously, so it runs on the runtime's blocking pool. A subscription (channels.rs)
//! is answered with its headers, then its packs as server-sent events while the caller stays,
//! a comment every 15 s keeping the connection open through proxies. Listeners bind loopback
//! unless the operator opens another address (`--data-host`).

use std::convert::Infallible;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Body, Bytes, Frame, Incoming, SizeHint};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::sync::{watch, Semaphore};

use crate::data::{DataRequest, DataResponse};
use crate::*;

/// What the data plane holds every connection and request to.
#[derive(Clone, Debug)]
pub(crate) struct DataLimits {
    /// The address listeners bind: loopback unless the operator opens another (`--data-host`).
    pub(crate) host: IpAddr,
    /// How long a request's headers may take to arrive (slow-header attacks); also how long an
    /// idle kept-alive connection waits for the next request.
    pub(crate) header_timeout: Duration,
    /// How long a request's body may take to arrive.
    pub(crate) body_timeout: Duration,
    /// The largest request body: 10 MiB, matching the outbound limit (§4.2.1).
    pub(crate) max_body_bytes: usize,
    /// Open connections per listener.
    pub(crate) max_connections: usize,
    /// The names a request may name its host by besides an IP address and `localhost`
    /// (`--allow-host`, as the control plane): a name a proxy in front forwards (`host_allowed`).
    pub(crate) names: Vec<String>,
}

/// Whether a request naming its host `host` (its `Host` header, or its authority) is one this
/// data plane answers: by an IP address, by `localhost` (or a name under it), or by a name in
/// `names`. Any other name is refused: DNS rebinding points a name a page was loaded from at this
/// machine, and the page then reads a flow's answers as its own origin's, past the browser's
/// cross-origin rules; it can't do that by an address, and a name of the operator's is listed.
/// No host at all (an HTTP/1.0 client) is answered: a browser always sends one.
pub(crate) fn host_allowed(host: Option<&str>, names: &[String]) -> bool {
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else { return true };
    // The name without its port: `[v6]:port`, `name:port`, or a bare one.
    let name = match host.strip_prefix('[') {
        Some(rest) => return rest.split(']').next().is_some_and(|v6| v6.parse::<std::net::Ipv6Addr>().is_ok()),
        None => host.rsplit_once(':').filter(|(_, port)| port.bytes().all(|b| b.is_ascii_digit())).map_or(host, |(n, _)| n),
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    name.parse::<IpAddr>().is_ok()
        || name == "localhost"
        || name.ends_with(".localhost")
        || names.iter().any(|n| n.trim_end_matches('.').eq_ignore_ascii_case(&name) || n.eq_ignore_ascii_case(host))
}

impl Default for DataLimits {
    fn default() -> DataLimits {
        DataLimits {
            host: IpAddr::from([127, 0, 0, 1]),
            header_timeout: Duration::from_secs(10),
            body_timeout: Duration::from_secs(30),
            max_body_bytes: 10 * 1024 * 1024,
            max_connections: 1024,
            names: Vec::new(),
        }
    }
}

/// A listener's stop signal: sending (or dropping it) stops accepting and frees the port.
/// Requests already being answered finish.
pub(crate) type Stop = watch::Sender<bool>;

/// The runtime every listener shares, started with the first.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("holon-data")
            .build()
            .expect("the data plane's runtime starts")
    })
}

/// What one listener answers with.
struct Listener {
    store: SharedStore,
    deployments: Arc<Mutex<BTreeMap<String, Deployment>>>,
    deployments_path: Place,
    slug: String,
    limits: DataLimits,
    /// Whether each route's endpoint, by its hash, runs on the worker that read its request (see
    /// [`runs_here_for`]): worked out once per version of the route.
    here: Mutex<Here>,
}

/// Start the data-plane listener for `slug` on `port`, returning its stop signal. The port is
/// bound here, so a port in use is reported to the caller rather than lost in a task.
pub(crate) fn start_listener(state: &AppState, slug: &str, port: u16) -> Result<Stop, String> {
    let limits = state.data_limits.clone();
    let bound = std::net::TcpListener::bind((limits.host, port)).map_err(|e| e.to_string())?;
    bound.set_nonblocking(true).map_err(|e| e.to_string())?;
    let runtime = runtime();
    let socket = {
        let _entered = runtime.enter();
        tokio::net::TcpListener::from_std(bound).map_err(|e| e.to_string())?
    };
    let (stop, mut stopped) = watch::channel(false);
    let listener = Arc::new(Listener {
        store: state.store.clone(),
        deployments: Arc::clone(&state.deployments),
        deployments_path: state.deployments_path.clone(),
        slug: slug.to_string(),
        limits,
        here: Mutex::new(Here::default()),
    });
    runtime.spawn(async move {
        let permits = Arc::new(Semaphore::new(listener.limits.max_connections));
        loop {
            let permit = tokio::select! {
                p = Arc::clone(&permits).acquire_owned() => p.expect("the semaphore is never closed"),
                _ = stopped.changed() => break,
            };
            let stream = tokio::select! {
                accepted = socket.accept() => match accepted {
                    Ok((stream, _)) => stream,
                    Err(e) => {
                        eprintln!("holon: {:?} could not accept a connection: {e}", listener.slug);
                        continue;
                    }
                },
                _ = stopped.changed() => break,
            };
            let listener = Arc::clone(&listener);
            tokio::spawn(async move {
                let header_timeout = listener.limits.header_timeout;
                let service = service_fn(move |request| {
                    let listener = Arc::clone(&listener);
                    async move { Ok::<_, Infallible>(answer(listener, request).await) }
                });
                let _ = http1::Builder::new()
                    .timer(TokioTimer::new())
                    .header_read_timeout(header_timeout)
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
                drop(permit);
            });
        }
    });
    println!("holon: {slug:?} answering on http://{}:{port}", state.data_limits.host);
    Ok(stop)
}

/// A response's body: whole, a subscription's events as they come, a streamed call's answer as it
/// is read, or a big file as it is read.
enum Answer {
    Whole(Full<Bytes>),
    Events(Events),
    Forwarded(Forwarded),
    File(Streamed),
}

/// How much of a file is read at a time.
const FILE_PIECE: usize = 64 << 10;

/// A big file a route answered with, its pieces read on the blocking pool and hashed as they go.
/// The last piece is held back until the whole is the handle's: one that changed while it was
/// sent ends with an error instead, which cuts the connection before the caller has all of it.
struct Streamed {
    pieces: tokio::sync::mpsc::Receiver<std::io::Result<Bytes>>,
    size: u64,
    ended: bool,
}

/// Stream `file`, the host's checks done and the file opened (`listeners::serve_file`): a file
/// that changed while it was sent is cut, and said so on `slug`'s timeline.
fn stream_file(file: data::Served, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, slug: String) -> Streamed {
    let (sender, pieces) = tokio::sync::mpsc::channel(4);
    let size = file.size;
    tokio::task::spawn_blocking(move || {
        let Some(opened) = &file.opened else { return };
        let mut reader = std::io::Read::take(&**opened, size + 1);
        let mut hasher = blake3::Hasher::new();
        let (mut held, mut read) = (None::<Bytes>, 0u64);
        let failed = loop {
            let mut piece = vec![0u8; FILE_PIECE];
            match std::io::Read::read(&mut reader, &mut piece) {
                Ok(0) => break None,
                Ok(n) => {
                    piece.truncate(n);
                    hasher.update(&piece);
                    read += n as u64;
                    // The caller went away: nothing more to send.
                    if let Some(before) = held.replace(Bytes::from(piece))
                        && sender.blocking_send(Ok(before)).is_err()
                    {
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => break Some(format!("couldn't be read while it was sent ({e})")),
            }
        };
        let failed = failed.or_else(|| (read != size || hasher.finalize().to_hex().as_str() != file.hash).then(|| "changed while it was sent".to_string()));
        match failed {
            None => {
                if let Some(last) = held {
                    let _ = sender.blocking_send(Ok(last));
                }
            }
            Some(why) => {
                let what = format!("{}/{} {why}: the connection was cut", file.folder, file.name);
                let detail = crate::events::Detail { name: Some(file.folder.clone()), ..Default::default() };
                crate::events::note(&deployments, &slug, "folder", what.clone(), None, detail);
                let _ = sender.blocking_send(Err(std::io::Error::other(what)));
            }
        }
    });
    Streamed { pieces, size, ended: false }
}

/// What a run's forwarder hands the data plane: the head, then the pieces.
enum Reply {
    Head(String),
    Chunk(Bytes),
}

/// The run's forwarder, on the blocking pool: each piece waits for room (the caller reads at its
/// pace, and the upstream with it); a caller that went away gets nothing more.
struct Replying(tokio::sync::mpsc::Sender<Reply>);

impl outbound::Forward for Replying {
    fn head(&self, content_type: &str) {
        let _ = self.0.blocking_send(Reply::Head(content_type.to_string()));
    }
    fn chunk(&self, bytes: &[u8]) {
        let _ = self.0.blocking_send(Reply::Chunk(Bytes::copy_from_slice(bytes)));
    }
}

/// A streamed call's answer as it is read, then the end of the run: a run that failed after the
/// reply began ends an event stream with `event: error`.
struct Forwarded {
    pieces: tokio::sync::mpsc::Receiver<Reply>,
    run: tokio::task::JoinHandle<DataResponse>,
    events: bool,
    ended: bool,
}

impl Forwarded {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        if self.ended {
            return Poll::Ready(None);
        }
        match self.pieces.poll_recv(cx) {
            Poll::Ready(Some(Reply::Chunk(b))) => return Poll::Ready(Some(Ok(Frame::data(b)))),
            Poll::Ready(Some(Reply::Head(_))) => return self.poll(cx),
            Poll::Ready(None) => {}
            Poll::Pending => return Poll::Pending,
        }
        let done = match Pin::new(&mut self.run).poll(cx) {
            Poll::Ready(done) => done,
            Poll::Pending => return Poll::Pending,
        };
        self.ended = true;
        let failed = done.as_ref().map_or(true, |r| r.status >= 500);
        if failed && self.events {
            let why = done.map(|r| String::from_utf8_lossy(&r.body).into_owned()).unwrap_or_else(|_| "{}".into());
            return Poll::Ready(Some(Ok(Frame::data(Bytes::from(format!("event: error\ndata: {why}\n\n"))))));
        }
        Poll::Ready(None)
    }
}

/// A subscription's packs as server-sent events, then the event that says why it ended; a
/// comment while nothing is sent.
struct Events {
    subscription: channels::Subscription,
    heartbeat: tokio::time::Interval,
    ended: bool,
}

/// How often a quiet subscription sends a comment.
const HEARTBEAT: Duration = Duration::from_secs(15);

impl Body for Answer {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, std::io::Error>>> {
        let never = |r: Result<Frame<Bytes>, Infallible>| r.map_err(|e| match e {});
        let events = match self.get_mut() {
            Answer::Whole(full) => return Pin::new(full).poll_frame(cx).map(|f| f.map(never)),
            Answer::Forwarded(f) => return f.poll(cx).map(|f| f.map(never)),
            Answer::File(file) => {
                if file.ended {
                    return Poll::Ready(None);
                }
                return match file.pieces.poll_recv(cx) {
                    Poll::Ready(Some(Ok(piece))) => Poll::Ready(Some(Ok(Frame::data(piece)))),
                    Poll::Ready(Some(Err(e))) => {
                        file.ended = true;
                        Poll::Ready(Some(Err(e)))
                    }
                    Poll::Ready(None) => {
                        file.ended = true;
                        Poll::Ready(None)
                    }
                    Poll::Pending => Poll::Pending,
                };
            }
            Answer::Events(events) => events,
        };
        if events.ended {
            return Poll::Ready(None);
        }
        let text = match events.subscription.packs.poll_recv(cx) {
            Poll::Ready(Some(pack)) => channels::event_text(&pack),
            Poll::Ready(None) => {
                events.ended = true;
                channels::end_text(events.subscription.end()).to_string()
            }
            Poll::Pending => match events.heartbeat.poll_tick(cx) {
                Poll::Ready(_) => ": still here\n\n".to_string(),
                Poll::Pending => return Poll::Pending,
            },
        };
        Poll::Ready(Some(Ok(Frame::data(Bytes::from(text)))))
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Answer::Whole(full) => full.is_end_stream(),
            Answer::Events(events) => events.ended,
            Answer::Forwarded(f) => f.ended,
            Answer::File(file) => file.ended,
        }
    }

    // A whole answer says its length (Content-Length), as it did before streams, and so does a
    // file; a stream doesn't know it (chunked).
    fn size_hint(&self) -> SizeHint {
        match self {
            Answer::Whole(full) => full.size_hint(),
            Answer::File(file) => SizeHint::with_exact(file.size),
            Answer::Events(_) | Answer::Forwarded(_) => SizeHint::default(),
        }
    }
}

/// Read one request within the limits, answer it with `serve_data` on the blocking pool, and turn
/// the answer into an HTTP response.
async fn answer(listener: Arc<Listener>, request: hyper::Request<Incoming>) -> hyper::Response<Answer> {
    let (parts, body) = request.into_parts();
    let host = parts.headers.get(hyper::header::HOST).and_then(|v| v.to_str().ok()).or_else(|| parts.uri.authority().map(|a| a.as_str()));
    if !host_allowed(host, &listener.limits.names) {
        let host = host.unwrap_or_default();
        return to_hyper(DataResponse::json_error(
            421,
            &format!("Host {host:?} isn't a name this server answers to (a page loaded from a name pointed at this machine can't read a flow's answers): reach it by its address or localhost, or start the server with --allow-host {}", host.rsplit_once(':').map_or(host, |(n, _)| n)),
        ));
    }
    let limit = listener.limits.max_body_bytes;
    let too_large = || DataResponse::json_error(413, &format!("the request body is larger than {limit} bytes"));
    let declared = parts.headers.get(hyper::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    if declared.is_some_and(|n| n > limit as u64) {
        return to_hyper(too_large());
    }
    let body = match tokio::time::timeout(listener.limits.body_timeout, Limited::new(body, limit).collect()).await {
        Err(_) => return to_hyper(DataResponse::json_error(408, "the request body took too long to arrive")),
        Ok(Err(e)) if e.downcast_ref::<LengthLimitError>().is_some() => return to_hyper(too_large()),
        Ok(Err(_)) => return to_hyper(DataResponse::json_error(400, "the request body could not be read")),
        Ok(Ok(collected)) => collected.to_bytes(),
    };
    let last_event_id = parts.headers.get("last-event-id").and_then(|v| channels::PackId::parse(v.to_str().ok()?));
    let request = DataRequest {
        method: parts.method.as_str().to_string(),
        url: parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_string(),
        headers: parts
            .headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect(),
        body: body.to_vec(),
        by: None,
    };
    let deployments = Arc::clone(&listener.deployments);
    let slug = listener.slug.clone();
    let origin = allowed_origin(&deployments, &slug, &request.headers);
    // A short route runs here, on the worker that read its request: no hop onto the blocking
    // pool, and no channel to forward on, since it calls out to nothing (§4.5 a, `runs_here_for`).
    if runs_here_for(&listener.store, &listener.deployments, &listener.slug, &request, &listener.here) {
        // A run that panics is answered as one on the blocking pool is (R1), not dropped.
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            serve_data_forwarding(&listener.store, &listener.deployments, &listener.deployments_path, &listener.slug, &request, None, origin)
        }))
        .unwrap_or_else(|_| DataResponse::json_error(500, "the request's handler failed"));
        return to_hyper(answered);
    }
    // A streamed call's answer comes back as it is read, before the run is over.
    let (pieces, mut forwarded) = tokio::sync::mpsc::channel(64);
    let forward = outbound::Forwarder::new(Arc::new(Replying(pieces)));
    let allowed = origin.clone();
    let mut run = tokio::task::spawn_blocking(move || {
        serve_data_forwarding(&listener.store, &listener.deployments, &listener.deployments_path, &listener.slug, &request, Some(forward), allowed)
    });
    let (head, done) = tokio::select! {
        head = forwarded.recv() => (head, None),
        done = &mut run => (None, Some(done)),
    };
    if let Some(Reply::Head(content_type)) = head {
        let mut headers = vec![("content-type".to_string(), content_type.clone()), ("cache-control".into(), "no-cache".into()), ("x-accel-buffering".into(), "no".into())];
        if let Some(o) = origin {
            headers.extend([("access-control-allow-origin".to_string(), o), ("vary".into(), "Origin".into())]);
        }
        let events = content_type.starts_with("text/event-stream");
        return with_headers(200, &headers, Answer::Forwarded(Forwarded { pieces: forwarded, run, events, ended: false }));
    }
    let done = match done {
        Some(done) => done,
        None => run.await,
    };
    let answered = done.unwrap_or_else(|_| DataResponse::json_error(500, "the request's handler failed"));
    // A big file: its bytes follow the headers, read from the file the host opened.
    if let Some(file) = answered.file.clone().filter(|f| f.opened.is_some()) {
        return with_headers(answered.status, &answered.headers, Answer::File(stream_file(file, deployments, slug)));
    }
    let Some(ask) = answered.subscribe.clone() else { return to_hyper(answered) };
    // A subscription: its packs follow the headers, from the deployment's channel.
    let channel = deployments.lock().unwrap().get(&slug).map(|d| (Arc::clone(&d.channels), d.permit.channels.clone()));
    let Some((channels, limits)) = channel else {
        return to_hyper(DataResponse::json_error(503, &format!("{slug:?} is not deployed as a server")));
    };
    match channels.subscribe(&ask.channel, ask.kept, last_event_id, &limits) {
        Err(e) => to_hyper(DataResponse::json_error(503, &e)),
        Ok(subscription) => {
            let heartbeat = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
            with_headers(answered.status, &answered.headers, Answer::Events(Events { subscription, heartbeat, ended: false }))
        }
    }
}

/// What a run on the worker that read its request may read: only what never waits. Its request,
/// the clock and random numbers are in hand; state, collections, secrets, databases and folders sit
/// behind locks a writer holds while it syncs to disk (a state's head, the store's values, the
/// vault), so a route reading one goes to the blocking pool (docs/L4s/resilience-audit-L4.md, R2).
const READS_HERE: &[&str] = &["NetworkIngress", "Clock", "Random"];

/// Which of a deployment's routes run inline, by endpoint hash, for the version of the deployment
/// it was worked out for: one deployed afresh starts it again, so it holds only the live version's.
#[derive(Default)]
pub(crate) struct Here {
    unit: Option<holon_core::Hash>,
    endpoints: std::collections::HashMap<holon_core::Hash, bool>,
}

/// Whether `request` runs on the data plane's worker rather than the blocking pool (§4.5 a): only a
/// run that can never wait, since a worker that waits holds up every other connection on it
/// (docs/L4s/resilience-audit-L4.md, R2). Its route's runs are unrecorded (granted so), so it
/// writes no log line and waits on no log's lock; it reads only [`READS_HERE`], so it calls out to
/// nothing and waits on no writer; and its flow is in memory already, so nothing is loaded from the
/// store. The hop costs 30–60 µs, about what such a run takes. Worked out once per endpoint hash;
/// anything unknown goes to the pool.
pub(crate) fn runs_here_for(
    store: &SharedStore,
    deployments: &Mutex<BTreeMap<String, Deployment>>,
    slug: &str,
    request: &DataRequest,
    here: &Mutex<Here>,
) -> bool {
    let path = request.url.split_once('?').map_or(request.url.as_str(), |(p, _)| p);
    let (unit, endpoint) = {
        let table = deployments.lock().unwrap();
        let Some(dep) = table.get(slug) else { return false };
        let Some((route, _)) = dep.server.as_ref().and_then(|s| holon_core::match_route(s, &request.method, path)) else { return false };
        let label = format!("{} {}", route.method, route.path);
        if dep.permit.log_level(Some(&label), route.log) != holon_core::LogLevel::Unrecorded {
            return false;
        }
        (dep.unit, route.endpoint)
    };
    // Loaded already, or it would be read from the store here: the pool loads it.
    if !store.3.holds(&unit) {
        return false;
    }
    {
        let mut known = here.lock().unwrap();
        if known.unit != Some(unit) {
            *known = Here { unit: Some(unit), endpoints: std::collections::HashMap::new() };
        }
        if let Some(short) = known.endpoints.get(&endpoint) {
            return *short;
        }
    }
    let short = crate::units::closure(store, unit).is_ok_and(|resolver| {
        holon_core::Resolver::resolve(&resolver, &endpoint).is_some_and(|unit| {
            holon_core::capabilities(unit, &resolver).iter().all(|c| READS_HERE.contains(&holon_core::consent_name(&c).split('(').next().unwrap_or("")))
        })
    });
    let mut known = here.lock().unwrap();
    if known.unit == Some(unit) {
        known.endpoints.insert(endpoint, short);
    }
    short
}

/// An answer as an HTTP response.
fn to_hyper(answer: DataResponse) -> hyper::Response<Answer> {
    with_headers(answer.status, &answer.headers, Answer::Whole(Full::new(Bytes::from(answer.body))))
}

/// A response with `status`, `headers` and `body`. A header the flow set that isn't valid HTTP is
/// a 500, not a malformed response.
fn with_headers(status: u16, headers: &[(String, String)], body: Answer) -> hyper::Response<Answer> {
    let mut builder = hyper::Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder.body(body).unwrap_or_else(|_| {
        let r = DataResponse::json_error(500, "the handler set a header that isn't valid HTTP");
        let mut response = hyper::Response::new(Answer::Whole(Full::new(Bytes::from(r.body))));
        *response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
        response.headers_mut().insert(hyper::header::CONTENT_TYPE, hyper::header::HeaderValue::from_static("application/json"));
        response
    })
}
