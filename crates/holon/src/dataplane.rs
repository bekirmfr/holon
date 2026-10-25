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
}

impl Default for DataLimits {
    fn default() -> DataLimits {
        DataLimits {
            host: IpAddr::from([127, 0, 0, 1]),
            header_timeout: Duration::from_secs(10),
            body_timeout: Duration::from_secs(30),
            max_body_bytes: 10 * 1024 * 1024,
            max_connections: 1024,
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
    deployments_path: String,
    slug: String,
    limits: DataLimits,
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

/// A response's body: whole, a subscription's events as they come, or a streamed call's answer
/// as it is read.
enum Answer {
    Whole(Full<Bytes>),
    Events(Events),
    Forwarded(Forwarded),
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
    type Error = Infallible;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        let events = match self.get_mut() {
            Answer::Whole(full) => return Pin::new(full).poll_frame(cx),
            Answer::Forwarded(f) => return f.poll(cx),
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
        }
    }

    // A whole answer says its length (Content-Length), as it did before streams; a stream doesn't
    // know it (chunked).
    fn size_hint(&self) -> SizeHint {
        match self {
            Answer::Whole(full) => full.size_hint(),
            Answer::Events(_) | Answer::Forwarded(_) => SizeHint::default(),
        }
    }
}

/// Read one request within the limits, answer it with `serve_data` on the blocking pool, and turn
/// the answer into an HTTP response.
async fn answer(listener: Arc<Listener>, request: hyper::Request<Incoming>) -> hyper::Response<Answer> {
    let (parts, body) = request.into_parts();
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
    // A streamed call's answer comes back as it is read, before the run is over.
    let (pieces, mut forwarded) = tokio::sync::mpsc::channel(64);
    let forward = outbound::Forwarder::new(Arc::new(Replying(pieces)));
    let mut run = tokio::task::spawn_blocking(move || {
        serve_data_forwarding(&listener.store, &listener.deployments, &listener.deployments_path, &listener.slug, &request, Some(forward))
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
