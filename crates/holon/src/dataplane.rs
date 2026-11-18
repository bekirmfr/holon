//! The data plane's HTTP edge (docs/PLATFORM_LAYOUT.md §7b): one listener per deployed server
//! unit, on `hyper` over a shared `tokio` runtime. The control plane stays on `rouille`; this is
//! the side that faces callers nobody vetted, so it holds every connection and request to limits
//! from the first byte:
//!
//! - **Slow clients:** a request's headers must arrive within `header_timeout`, its body within
//!   `body_timeout` (408), or the connection is closed.
//! - **Large bodies:** a body over `max_body_bytes` is refused (413), before it is read when its
//!   `Content-Length` says so.
//! - **Many connections:** at most `max_connections` per listener and `max_connections_total` in
//!   all (a WebSocket's for as long as it is open, [`Places`]); more wait in the OS's accept queue
//!   rather than each taking a thread. An accept that fails (no file handle left) is tried again
//!   after a wait, doubled at each failure to a second, never at once.
//! - **Memory:** while the server holds more than it may in all (`--max-memory`), a request is
//!   answered 503 before its body is read, and a run running fails (holon_core::memory).
//! - **Slow readers:** a write that makes no progress within `write_timeout` (a caller that stopped
//!   reading a file, a stream or any answer) closes the connection, and nothing waits on a caller
//!   on a thread: a file is read a piece at a time on the pool and sent from a task.
//! - **A share per deployment:** a deployment has at most its `max_concurrent_runs` runs on the
//!   pool at once, and the data plane at most `max_runs` (every deployment's together): past
//!   either, a request is answered 503 at once (`Retry-After`), never queued, so a deployment
//!   flooded, or waiting on a slow upstream, leaves the others their threads ([`Share`]).
//!
//! A request becomes a [`DataRequest`] and is answered by `serve_data` (listeners.rs), which runs
//! the flow synchronously, so it runs on the runtime's blocking pool: every run does, however
//! short, since nothing may compute on a worker (`answer` says why). A subscription (channels.rs)
//! is answered with its headers, then its packs as server-sent events while the caller stays,
//! a comment every 15 s keeping the connection open through proxies. A GET on a websocket's path
//! switches its connection to a WebSocket (websockets.rs). Listeners bind loopback unless the
//! operator opens another address (`--data-host`).

use std::convert::Infallible;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::task::{Context, Poll};

use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::body::{Body, Bytes, Frame, Incoming, SizeHint};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{watch, Notify, Semaphore};

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
    /// How long a write to a caller may make no progress before its connection is closed: a caller
    /// that stopped reading holds nothing past it ([`Deadlined`]).
    pub(crate) write_timeout: Duration,
    /// The runs the data plane has on the pool at once, every deployment's together ([`MAX_RUNS`]).
    pub(crate) max_runs: usize,
    /// Those runs, counted: one count for every listener of the server ([`Share`]).
    pub(crate) runs: Arc<Running>,
    /// Open connections, every listener's together, a WebSocket's for as long as it is open
    /// (`bounds::max_connections_total`): more wait in the OS's accept queue.
    pub(crate) max_connections_total: usize,
    /// Those connections' places: one for every listener of the server ([`Places`]).
    pub(crate) connections: Arc<Semaphore>,
}

/// A connection's places: its listener's and the server's ([`DataLimits::connections`]), given
/// back as it closes. hyper is done with a connection switched to a WebSocket as it switches, so
/// the WebSocket takes them ([`Places::keep`]) and holds them while it is open: without them a
/// server held any number of open sockets past its counts, each a file (audit R16).
#[derive(Clone)]
pub(crate) struct Places(Arc<Mutex<Option<[tokio::sync::OwnedSemaphorePermit; 2]>>>);

impl Places {
    /// The places, for whoever holds the connection from now on; none once taken.
    pub(crate) fn keep(&self) -> Option<[tokio::sync::OwnedSemaphorePermit; 2]> {
        self.0.lock().unwrap().take()
    }
}

/// The threads the data plane's runs, file reads and WebSocket blocks share: tokio's default,
/// said here so the shares below are of a known number.
const POOL: usize = 512;

/// The runs the data plane has on the pool at once, every deployment's together: the pool less
/// the threads a file's reads and the runs past it (a WebSocket's close) need, so those never
/// wait behind runs. The most any deployment's `max_concurrent_runs` may be (permissions.rs).
pub(crate) const MAX_RUNS: u64 = 448;

/// Runs in hand, counted, and a wake-up for whoever waits for one to end: a deployment's, or the
/// data plane's ([`Share`]).
#[derive(Debug, Default)]
pub(crate) struct Running {
    count: AtomicUsize,
    freed: Notify,
}

impl Running {
    /// One more, if there are fewer than `most`.
    fn take(&self, most: usize) -> bool {
        self.count.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < most).then_some(n + 1)).is_ok()
    }

    /// One fewer, and whoever waits woken.
    fn give(&self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
        self.freed.notify_waiters();
    }
}

/// A deployment's share of the pool: its runs at once held to its grant's `max_concurrent_runs`,
/// and the data plane's to `max_runs`. Every deployment's runs go to the one pool (`answer` says
/// why none runs on a worker); without a share, 512 requests to one deployment held on a slow
/// upstream, or a flood of them, left every other deployment's requests queued behind them with
/// no answer (audit R15).
#[derive(Clone)]
pub(crate) struct Share {
    deployments: Arc<Mutex<BTreeMap<String, Deployment>>>,
    slug: String,
    deployment: Arc<Running>,
    data_plane: Arc<Running>,
    max_runs: usize,
    /// The deployment's `max_concurrent_runs` as its grant said when last read: what a run is held
    /// to while another holds the deployments table (a writer writing, sealing or syncing it), so
    /// no worker waits on the table's lock (release audit R45, D5). A grant changed meanwhile is
    /// read by the next request to find the table free.
    known: Arc<AtomicUsize>,
}

/// A run admitted ([`Share`]): its place in the deployment's share and the data plane's, given
/// back when it is dropped, as the run ends.
pub(crate) struct Admitted {
    deployment: Arc<Running>,
    data_plane: Arc<Running>,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        self.deployment.give();
        self.data_plane.give();
    }
}

impl Share {
    /// The deployment's share as its grant says now, or as it said when last read if the table
    /// is held: this runs on a worker, which never waits on the table ([`Share::known`]). None
    /// once the deployment is gone.
    fn most(&self) -> Option<usize> {
        let Some(table) = self.deployments.try_lock() else {
            return Some(self.known.load(Ordering::Relaxed));
        };
        let most = table.get(&self.slug).map(|d| usize::try_from(d.permit.durable.max_concurrent_runs).unwrap_or(usize::MAX))?;
        self.known.store(most, Ordering::Relaxed);
        Some(most)
    }

    fn admit_within(&self, most: usize) -> Result<Admitted, String> {
        if !self.deployment.take(most) {
            return Err(format!("{:?} is answering {most} requests at once, as many as it may (max_concurrent_runs): try again", self.slug));
        }
        if !self.data_plane.take(self.max_runs) {
            self.deployment.give();
            return Err(format!("the server is answering {} requests at once, as many as it may: try again", self.max_runs));
        }
        Ok(Admitted { deployment: Arc::clone(&self.deployment), data_plane: Arc::clone(&self.data_plane) })
    }

    /// A run admitted now, or why not: the deployment's share is in hand, or the data plane's, or
    /// the deployment is gone.
    pub(crate) fn try_admit(&self) -> Result<Admitted, String> {
        let most = self.most().ok_or_else(|| format!("{:?} is not deployed as a server", self.slug))?;
        self.admit_within(most)
    }

    /// A run admitted once there is room: for one that is not answered with a refusal to try
    /// again (a WebSocket's message, its close). Waiting holds no thread. A deployment gone is
    /// held to the data plane's share alone, so what it leaves to run (a close) runs.
    pub(crate) async fn admit(&self) -> Admitted {
        loop {
            let (deployment, data_plane) = (self.deployment.freed.notified(), self.data_plane.freed.notified());
            tokio::pin!(deployment, data_plane);
            deployment.as_mut().enable();
            data_plane.as_mut().enable();
            if let Ok(admitted) = self.admit_within(self.most().unwrap_or(usize::MAX)) {
                return admitted;
            }
            tokio::select! {
                _ = deployment => {}
                _ = data_plane => {}
            }
        }
    }
}

/// A connection whose writes must make progress within `deadline`: a write the caller doesn't
/// take (it stopped reading, its window shut) past it fails, and the connection with it, so a
/// caller can hold a connection open only while it reads. hyper's own timers bound a request's
/// headers, not an answer's writes; a file or a stream to a caller that stopped reading held its
/// connection, and the thread sending it, for good (audit R15).
struct Deadlined<S> {
    inner: S,
    deadline: Duration,
    stalled: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<S> Deadlined<S> {
    fn new(inner: S, deadline: Duration) -> Deadlined<S> {
        Deadlined { inner, deadline, stalled: None }
    }

    /// `poll` of a write: progress clears the deadline; none starts it, and past it is an error.
    fn within<T>(&mut self, cx: &mut Context<'_>, poll: impl FnOnce(Pin<&mut S>, &mut Context<'_>) -> Poll<std::io::Result<T>>) -> Poll<std::io::Result<T>>
    where
        S: Unpin,
    {
        match poll(Pin::new(&mut self.inner), cx) {
            Poll::Pending => {
                let deadline = self.deadline;
                let stalled = self.stalled.get_or_insert_with(|| Box::pin(tokio::time::sleep(deadline)));
                match stalled.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the caller stopped reading"))),
                    Poll::Pending => Poll::Pending,
                }
            }
            done => {
                self.stalled = None;
                done
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Deadlined<S> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Deadlined<S> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        self.get_mut().within(cx, |s, cx| s.poll_write(cx, buf))
    }
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[std::io::IoSlice<'_>]) -> Poll<std::io::Result<usize>> {
        self.get_mut().within(cx, |s, cx| s.poll_write_vectored(cx, bufs))
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.get_mut().within(cx, |s, cx| s.poll_flush(cx))
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.get_mut().within(cx, |s, cx| s.poll_shutdown(cx))
    }
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
            write_timeout: Duration::from_secs(30),
            max_runs: MAX_RUNS as usize,
            runs: Arc::new(Running::default()),
            max_connections_total: crate::bounds::CONNECTIONS,
            connections: Arc::new(Semaphore::new(crate::bounds::CONNECTIONS)),
        }
    }
}

/// How long a listener waits after an accept fails before it tries again, doubled at each failure
/// in a row to [`ACCEPT_WAIT_MOST`] (as Go's `net/http` does).
const ACCEPT_WAIT_FIRST: Duration = Duration::from_millis(5);
const ACCEPT_WAIT_MOST: Duration = Duration::from_secs(1);

/// A listener's stop signal: sending (or dropping it) stops accepting and frees the port.
/// Requests already being answered finish.
pub(crate) type Stop = watch::Sender<bool>;

/// The runtime every listener shares, started with the first.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .max_blocking_threads(POOL)
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
    share: Share,
}

/// Start the data-plane listener for `slug` on `port`, returning its stop signal. The port is
/// bound here, so a port in use is reported to the caller rather than lost in a task. `most` is
/// the deployment's `max_concurrent_runs` as the caller read it from the table.
pub(crate) fn start_listener(state: &AppState, slug: &str, port: u16, most: usize) -> Result<Stop, String> {
    let limits = state.data_limits.clone();
    let bound = std::net::TcpListener::bind((limits.host, port)).map_err(|e| e.to_string())?;
    bound.set_nonblocking(true).map_err(|e| e.to_string())?;
    let runtime = runtime();
    let socket = {
        let _entered = runtime.enter();
        tokio::net::TcpListener::from_std(bound).map_err(|e| e.to_string())?
    };
    let (stop, mut stopped) = watch::channel(false);
    let share = Share {
        deployments: Arc::clone(&state.deployments),
        slug: slug.to_string(),
        deployment: Arc::new(Running::default()),
        data_plane: Arc::clone(&limits.runs),
        max_runs: limits.max_runs,
        known: Arc::new(AtomicUsize::new(most)),
    };
    let listener = Arc::new(Listener {
        store: state.store.clone(),
        deployments: Arc::clone(&state.deployments),
        deployments_path: state.deployments_path.clone(),
        slug: slug.to_string(),
        limits,
        share,
    });
    runtime.spawn(async move {
        let permits = Arc::new(Semaphore::new(listener.limits.max_connections));
        let connections = Arc::clone(&listener.limits.connections);
        // Accepts failed in a row (out of file handles, say), and how long to wait before the next.
        let mut failing: Option<(u64, Duration)> = None;
        loop {
            // A place in the listener's connections, then in the server's: past either, a caller
            // waits in the OS's accept queue.
            let places = tokio::select! {
                places = async {
                    let mine = Arc::clone(&permits).acquire_owned().await;
                    let all = Arc::clone(&connections).acquire_owned().await;
                    [mine.expect("the semaphore is never closed"), all.expect("the semaphore is never closed")]
                } => places,
                _ = stopped.changed() => break,
            };
            let stream = tokio::select! {
                accepted = socket.accept() => match accepted {
                    Ok((stream, _)) => {
                        if let Some((failed, _)) = failing.take() {
                            eprintln!("holon: {:?} accepts connections again, after {failed} that failed", listener.slug);
                        }
                        stream
                    }
                    Err(e) => {
                        // Waited on, not tried again at once: an accept that fails (no file
                        // handle left) fails again at once, and a loop trying spun on a core and
                        // printed a line each time (audit R16). Said once a run of failures.
                        let (failed, wait) = match failing {
                            None => {
                                eprintln!("holon: {:?} could not accept a connection: {e}; trying again, less often while it fails", listener.slug);
                                (1, ACCEPT_WAIT_FIRST)
                            }
                            Some((failed, wait)) => (failed + 1, (wait * 2).min(ACCEPT_WAIT_MOST)),
                        };
                        failing = Some((failed, wait));
                        drop(places);
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = stopped.changed() => break,
                        }
                        continue;
                    }
                },
                _ = stopped.changed() => break,
            };
            let listener = Arc::clone(&listener);
            let places = Places(Arc::new(Mutex::new(Some(places))));
            tokio::spawn(async move {
                let header_timeout = listener.limits.header_timeout;
                let write_timeout = listener.limits.write_timeout;
                let held = places.clone();
                let service = service_fn(move |mut request: hyper::Request<Incoming>| {
                    let listener = Arc::clone(&listener);
                    // For a WebSocket to keep, should the connection switch to one.
                    request.extensions_mut().insert(held.clone());
                    async move { Ok::<_, Infallible>(answer(listener, request).await) }
                });
                let _ = http1::Builder::new()
                    .timer(TokioTimer::new())
                    .header_read_timeout(header_timeout)
                    .serve_connection(TokioIo::new(Deadlined::new(stream, write_timeout)), service)
                    .with_upgrades()
                    .await;
                // Given back, unless a WebSocket kept them.
                drop(places);
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

/// One piece of a file read on the pool, and hashed there: no worker computes (`answer`).
fn read_piece(opened: &std::fs::File, left: u64, mut hasher: blake3::Hasher) -> (blake3::Hasher, std::io::Result<Vec<u8>>) {
    let mut piece = vec![0u8; FILE_PIECE.min(usize::try_from(left).unwrap_or(FILE_PIECE))];
    loop {
        match std::io::Read::read(&mut &*opened, &mut piece) {
            Ok(n) => {
                piece.truncate(n);
                hasher.update(&piece);
                return (hasher, Ok(piece));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return (hasher, Err(e)),
        }
    }
}

/// Stream `file`, the host's checks done and the file opened (`listeners::serve_file`): a file
/// that changed while it was sent is cut, and said so on `slug`'s timeline. Sent from a task: each
/// piece is read on the pool, and the wait for the caller to take it holds no thread. A file
/// sent from the pool waited there on the caller, so callers that stopped reading big files held
/// the pool's threads until none was left for any deployment's runs (audit R15).
fn stream_file(file: data::Served, deployments: Arc<Mutex<BTreeMap<String, Deployment>>>, slug: String) -> Streamed {
    let (sender, pieces) = tokio::sync::mpsc::channel(4);
    let size = file.size;
    tokio::spawn(async move {
        let Some(opened) = file.opened.clone() else { return };
        let mut hasher = blake3::Hasher::new();
        let (mut held, mut read) = (None::<Bytes>, 0u64);
        let failed = loop {
            // One byte past its size is read for, so a file that grew is seen to.
            let (left, reading) = (size + 1 - read, Arc::clone(&opened));
            let Ok((back, piece)) = tokio::task::spawn_blocking(move || read_piece(&reading, left, hasher)).await else { return };
            hasher = back;
            match piece {
                Ok(piece) if piece.is_empty() => break None,
                Ok(piece) => {
                    read += piece.len() as u64;
                    // The caller went away, or its connection was cut: nothing more to send.
                    if let Some(before) = held.replace(Bytes::from(piece))
                        && sender.send(Ok(before)).await.is_err()
                    {
                        return;
                    }
                    if read > size {
                        break None;
                    }
                }
                Err(e) => break Some(format!("couldn't be read while it was sent ({e})")),
            }
        };
        let failed = failed.or_else(|| (read != size || hasher.finalize().to_hex().as_str() != file.hash).then(|| "changed while it was sent".to_string()));
        match failed {
            None => {
                if let Some(last) = held {
                    let _ = sender.send(Ok(last)).await;
                }
            }
            Some(why) => {
                let what = format!("{}/{} {why}: the connection was cut", file.folder, file.name);
                let detail = crate::events::Detail { name: Some(file.folder.clone()), ..Default::default() };
                crate::events::note(&deployments, &slug, "folder", what.clone(), None, detail);
                let _ = sender.send(Err(std::io::Error::other(what))).await;
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
/// A request's run, on the blocking pool: its answer, and the channel a subscription it answers
/// with reads from, looked up there too (none: it subscribes to nothing, or the deployment is gone).
type Ran = (DataResponse, Option<(Arc<channels::Channels>, channels::ChannelLimits)>);

struct Forwarded {
    pieces: tokio::sync::mpsc::Receiver<Reply>,
    /// The run, with the subscription's channel it would have needed (none: a streamed answer
    /// subscribes to nothing).
    run: tokio::task::JoinHandle<Ran>,
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
            Poll::Ready(done) => done.map(|(answered, _)| answered),
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

/// What a request is answered (503) while the server holds more memory than it may in all.
pub(crate) const SHORT_OF_MEMORY: &str = "the server holds as much memory as it may (--max-memory): try again";

/// Read one request within the limits, answer it with `serve_data` on the blocking pool, and turn
/// the answer into an HTTP response.
async fn answer(listener: Arc<Listener>, request: hyper::Request<Incoming>) -> hyper::Response<Answer> {
    let (mut parts, body) = request.into_parts();
    let host = parts.headers.get(hyper::header::HOST).and_then(|v| v.to_str().ok()).or_else(|| parts.uri.authority().map(|a| a.as_str()));
    if !host_allowed(host, &listener.limits.names) {
        let host = host.unwrap_or_default();
        return to_hyper(DataResponse::json_error(
            421,
            &format!("Host {host:?} isn't a name this server answers to (a page loaded from a name pointed at this machine can't read a flow's answers): reach it by its address or localhost, or start the server with --allow-host {}", host.rsplit_once(':').map_or(host, |(n, _)| n)),
        ));
    }
    // A server holding more memory than it may in all takes on no more: no body read, no run, no
    // WebSocket opened, until its runs have let some go (audit R16).
    if holon_core::memory::process_over() {
        let mut busy = DataResponse::json_error(503, SHORT_OF_MEMORY);
        busy.headers.push(("retry-after".into(), "1".into()));
        return to_hyper(busy);
    }
    // A connection a page opens on one of its websockets' paths is the flow's from here on.
    if let Some(answered) = websockets::open(&listener.store, &listener.deployments, &listener.slug, &listener.share, &mut parts).await {
        return to_hyper(answered);
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
        // Without a copy where the body came in one piece.
        body: Vec::from(body),
        by: None,
    };
    let deployments = Arc::clone(&listener.deployments);
    let slug = listener.slug.clone();
    // Every run goes to the blocking pool, however short: nothing computes on a worker.
    // Nor does a worker take the deployments table's lock, whose holders write and seal: what the
    // answer needs of the table (the origins it answers, a subscription's channel) is read there
    // too (release audit R45, D5). The worker
    // that reads a request is the one watching the network for every listener on the server, and
    // tokio wakes another to take the watch over only when it has more than one task to share. A
    // run computing here, even a route's that waits on nothing, leaves the whole server deaf
    // until it ends: no connection accepted and no byte read, on any port. A short route was
    // answered here once (request-path 12.1), about 20 µs faster at one connection, and one
    // request that computed a while stopped every flow from answering (B5). Before running
    // anything on a worker again, read performance.md, "Nothing computes on a data-plane worker":
    // what it saved, what it cost, and the ways around it that were weighed and left.
    // Its deployment's share of the pool, or a refusal to try again: never a queue behind runs of
    // its own, or another deployment's ([`Share`]).
    let admitted = match listener.share.try_admit() {
        Ok(admitted) => admitted,
        Err(why) => {
            let mut busy = DataResponse::json_error(503, &why);
            busy.headers.push(("retry-after".into(), "1".into()));
            return to_hyper(busy);
        }
    };
    // A streamed call's answer comes back as it is read, before the run is over. The run's
    // thread waits on the caller for each piece, at most the write deadline: a caller that
    // stops reading has its connection cut there ([`Deadlined`]), and the pieces nowhere to go.
    let (pieces, mut forwarded) = tokio::sync::mpsc::channel(64);
    let forward = outbound::Forwarder::new(Arc::new(Replying(pieces)));
    // The origin, set before the run starts: a streamed answer's head, sent while it runs, needs it.
    let origin: Arc<OnceLock<Option<String>>> = Arc::new(OnceLock::new());
    let told = Arc::clone(&origin);
    let mut run = tokio::task::spawn_blocking(move || {
        let _admitted = admitted;
        let (deployments, slug) = (&listener.deployments, listener.slug.as_str());
        let allowed = allowed_origin(deployments, slug, &request.headers);
        let _ = told.set(allowed.clone());
        let answered = serve_data_forwarding(&listener.store, deployments, &listener.deployments_path, slug, &request, Some(forward), allowed);
        let channel = answered.subscribe.as_ref().and_then(|_| deployments.lock().unwrap().get(slug).map(|d| (Arc::clone(&d.channels), d.permit.channels.clone())));
        (answered, channel)
    });
    let (head, done) = tokio::select! {
        head = forwarded.recv() => (head, None),
        done = &mut run => (None, Some(done)),
    };
    if let Some(Reply::Head(content_type)) = head {
        let mut headers = vec![("content-type".to_string(), content_type.clone()), ("cache-control".into(), "no-cache".into()), ("x-accel-buffering".into(), "no".into())];
        if let Some(o) = origin.get().cloned().flatten() {
            headers.extend([("access-control-allow-origin".to_string(), o), ("vary".into(), "Origin".into())]);
        }
        let events = content_type.starts_with("text/event-stream");
        return with_headers(200, &headers, Answer::Forwarded(Forwarded { pieces: forwarded, run, events, ended: false }));
    }
    let done = match done {
        Some(done) => done,
        None => run.await,
    };
    let (answered, channel) = done.unwrap_or_else(|_| (DataResponse::json_error(500, "the request's handler failed"), None));
    // A big file: its bytes follow the headers, read from the file the host opened.
    if let Some(file) = answered.file.clone().filter(|f| f.opened.is_some()) {
        return with_headers(answered.status, &answered.headers, Answer::File(stream_file(file, deployments, slug)));
    }
    let Some(ask) = answered.subscribe.clone() else { return to_hyper(answered) };
    // A subscription: its packs follow the headers, from the deployment's channel.
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
/// a 500, not a malformed response. Every data-plane answer is built here (a test holds that no
/// other place builds one), so each goes out with `X-Content-Type-Options: nosniff`: a browser
/// takes it as the type it says, and a JSON answer that echoes what its caller sent isn't run as a
/// page or a script (audit R51). A flow that sets the header keeps its own. Whether a flow's page
/// may be framed is the flow's to say (a widget is meant to be): its own headers.
fn with_headers(status: u16, headers: &[(String, String)], body: Answer) -> hyper::Response<Answer> {
    let mut builder = hyper::Response::builder().status(status);
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("x-content-type-options")) {
        builder = builder.header("x-content-type-options", "nosniff");
    }
    builder.body(body).unwrap_or_else(|_| {
        let r = DataResponse::json_error(500, "the handler set a header that isn't valid HTTP");
        let mut response = hyper::Response::new(Answer::Whole(Full::new(Bytes::from(r.body))));
        *response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
        response.headers_mut().insert(hyper::header::CONTENT_TYPE, hyper::header::HeaderValue::from_static("application/json"));
        response.headers_mut().insert(hyper::header::X_CONTENT_TYPE_OPTIONS, hyper::header::HeaderValue::from_static("nosniff"));
        response
    })
}
