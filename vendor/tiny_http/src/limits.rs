//! Holon (PATCHED.md): what a server lets a connection hold. Each connection holds a thread of
//! the pool for as long as it is open (`client.rs` reads its requests one after another), so:
//!
//! - **Open connections** are counted, in all and from each address, and one past either bound
//!   is answered 503 at once and closed, not accepted and left without a thread (`Refusals`).
//! - **An idle connection** is closed: a socket read that waits longer than `idle` fails, and a
//!   connection that has sent nothing of its next request by then (or nothing at all) is closed
//!   without an answer, as a server closes a kept-alive connection it no longer wants. The same
//!   limit bounds every wait for a request's bytes, its body's included: one that stops for
//!   longer is cut off (408 when tiny_http reads it, an error to the handler that reads it).
//!   Bytes still coming, however slowly, keep it (a request's own up to its deadline, below); a
//!   response, however long, isn't a read.
//! - **A request** arrives whole by a deadline from its first byte: its line, its headers and a
//!   body tiny_http reads itself (1 KiB or less, before the application sees the request). Past
//!   it the client is told 408 and the connection closed, however steadily its bytes come, so a
//!   request trickled a byte at a time doesn't hold its connection for as long as each byte comes
//!   within the idle limit. A larger body is the application's to read, by its own deadline
//!   (Holon's `http::BODY_DEADLINE`).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::connection::Connection;

/// What a server's connections may hold (`Server::from_listener_limited`).
#[derive(Debug, Clone)]
pub struct Limits {
    /// The longest a socket read waits for a client's bytes; `None` waits for ever (upstream).
    pub idle: Option<Duration>,
    /// The longest a request takes to arrive whole, from its first byte: its head, and its body
    /// when tiny_http reads it (`Deadline`); `None` waits while its bytes keep coming (upstream).
    pub request: Option<Duration>,
    /// Connections open at once, from everyone.
    pub connections: usize,
    /// Connections open at once from one address (all of a Unix socket's count as one).
    pub from_one: usize,
}

/// The longest a connection waits for its client's bytes. Past Holon's own deadline for a
/// request's body (`http::BODY_DEADLINE`, 30 s), so a body is cut off by that deadline and its
/// 408 first, never by this.
pub const IDLE: Duration = Duration::from_secs(60);

/// The longest a request takes to arrive whole, from its first byte: its line, its headers and a
/// body of 1 KiB or less. Holon's deadline for a larger body, which Holon reads
/// (`http::BODY_DEADLINE`), is the same 30 s, as is the data plane's `body_timeout`; main.rs
/// asserts the first.
pub const REQUEST: Duration = Duration::from_secs(30);

/// Connections open at once: twice the requests Holon answers at once (`MAX_IN_FLIGHT`, 512),
/// room for the kept-alive ones between requests beside them.
pub const CONNECTIONS: usize = 1024;

/// Connections open at once from one address: half of all. One address can rightly hold every
/// request Holon answers at once: behind a proxy it trusts, every client's requests come from the
/// proxy's address, each on a connection of its own, and Holon counts each client it forwarded
/// for as itself, up to all it answers at once (`MAX_IN_FLIGHT`, 512; main.rs asserts it). The
/// rest is for everyone else; a browser opens six to a host, a command one or two.
pub const FROM_ONE: usize = CONNECTIONS / 2;

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            idle: Some(IDLE),
            request: Some(REQUEST),
            connections: CONNECTIONS,
            from_one: FROM_ONE,
        }
    }
}

/// Holon (PATCHED.md): when a connection's request has to have arrived by. Set at the request's
/// first byte (`ClientConnection::read_next_line`) and cleared once tiny_http has read it whole
/// (`ClientConnection::read`), so what the application reads after (a larger body) and the wait
/// for the next request are held to the idle limit alone. Each read of the connection
/// (`RefinedTcpStream`) waits what `wait` gives it.
pub(crate) struct Deadline {
    idle: Option<Duration>,
    request: Option<Duration>,
    by: Mutex<Option<Instant>>,
}

impl Deadline {
    pub(crate) fn new(idle: Option<Duration>, request: Option<Duration>) -> Arc<Deadline> {
        Arc::new(Deadline {
            idle,
            request,
            by: Mutex::new(None),
        })
    }

    /// A request's first byte has come: the rest of it is due by the request limit.
    pub(crate) fn start(&self) {
        if let Some(request) = self.request {
            *self.by.lock().unwrap() = Some(Instant::now() + request);
        }
    }

    /// The request has come whole.
    pub(crate) fn end(&self) {
        *self.by.lock().unwrap() = None;
    }

    /// The connection is told 408 and ends: what its client still sends is read until `linger`
    /// from now (`ClientConnection::next`).
    pub(crate) fn linger(&self, linger: Duration) {
        *self.by.lock().unwrap() = Some(Instant::now() + linger);
    }

    /// What a client is told when its request is cut off: by the deadline, once it has passed
    /// (the read that waited out what was left of it fails as any read that waits too long does);
    /// otherwise by the idle limit, a read inside the request that waited the limit out.
    pub(crate) fn why(&self) -> String {
        let past = matches!(*self.by.lock().unwrap(), Some(by) if Instant::now() >= by);
        match (past, self.request, self.idle) {
            (true, Some(request), _) => format!(
                "the request didn't arrive whole within {} seconds of its first byte",
                request.as_secs_f64()
            ),
            (_, _, Some(idle)) => format!(
                "nothing more of the request arrived for {} seconds",
                idle.as_secs_f64()
            ),
            _ => "the request didn't arrive in time".to_string(),
        }
    }

    /// How long the next read may wait: the idle limit, or what is left to the deadline if that
    /// is less. Once the deadline has passed, a time-out, which `client.rs` answers 408.
    pub(crate) fn wait(&self) -> std::io::Result<Option<Duration>> {
        let by = *self.by.lock().unwrap();
        let Some(by) = by else { return Ok(self.idle) };
        let left = by.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, PastDeadline));
        }
        Ok(Some(self.idle.map_or(left, |idle| idle.min(left))))
    }
}

/// Holon (PATCHED.md): a read once a request's deadline (or a 408's linger) has passed.
#[derive(Debug)]
pub(crate) struct PastDeadline;

impl std::fmt::Display for PastDeadline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the request didn't arrive whole by its deadline")
    }
}

impl std::error::Error for PastDeadline {}

/// The most read off a connection told 408 while it lingers: one that sends more is closed then.
pub(crate) const LINGER_READS_MOST: usize = 64 * 1024;

/// What a connection past a bound is told before it is closed.
pub const TOO_MANY: &str =
    "the server has too many connections open: try again in a moment";
pub const TOO_MANY_FROM_ONE: &str =
    "the server has too many connections open from your address: close some, or try again in a moment";

/// The connections open now, in all and from each address.
pub(crate) struct Open {
    limits: Limits,
    counts: Mutex<Counts>,
}

struct Counts {
    all: usize,
    from: HashMap<Option<IpAddr>, usize>,
}

/// One connection counted in `Open` while any part of it (its reading half, its writing half)
/// is alive: a response still being written after the reading has stopped counts too.
pub(crate) struct Held {
    open: Arc<Open>,
    from: Option<IpAddr>,
}

impl Open {
    pub(crate) fn new(limits: Limits) -> Arc<Open> {
        Arc::new(Open {
            limits,
            counts: Mutex::new(Counts {
                all: 0,
                from: HashMap::new(),
            }),
        })
    }

    pub(crate) fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Count one more connection from `from`, unless a bound is reached: then what it is told.
    pub(crate) fn take(self: &Arc<Self>, from: Option<IpAddr>) -> Result<Arc<Held>, &'static str> {
        let mut counts = self.counts.lock().unwrap();
        let mine = counts.from.get(&from).copied().unwrap_or(0);
        if mine >= self.limits.from_one {
            return Err(TOO_MANY_FROM_ONE);
        }
        if counts.all >= self.limits.connections {
            return Err(TOO_MANY);
        }
        counts.all += 1;
        counts.from.insert(from, mine + 1);
        Ok(Arc::new(Held {
            open: Arc::clone(self),
            from,
        }))
    }

    /// The connections open now.
    #[cfg(test)]
    pub(crate) fn count(&self) -> usize {
        self.counts.lock().unwrap().all
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut counts = self.open.counts.lock().unwrap();
        counts.all -= 1;
        if let Some(n) = counts.from.get_mut(&self.from) {
            *n -= 1;
            if *n == 0 {
                counts.from.remove(&self.from);
            }
        }
    }
}

/// Tell a connection past a bound why, and end its sending side; `Refusals` closes it. Written
/// before anything is read, without waiting: the socket is new, so a few hundred bytes go into
/// its empty send buffer.
pub(crate) fn refuse(mut sock: Connection, why: &str) -> Connection {
    let body = format!("{{\"error\":\"{}\"}}", why);
    let answer = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nRetry-After: 1\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = sock.set_nonblocking(true);
    let _ = sock.write_all(answer.as_bytes());
    let _ = sock.flush();
    let _ = sock.shutdown(std::net::Shutdown::Write);
    sock
}

/// How long a refused connection is kept for its client to read its 503 and close, from its
/// refusal.
pub(crate) const LINGER: Duration = Duration::from_secs(2);
/// Refused connections kept at once, all told; past them, the oldest is closed when the next is
/// refused.
pub(crate) const LINGERING: usize = 256;
/// The most read from one refused connection in one pass, so a client that sends without end
/// can't hold the thread from the others and from their deadlines.
const READ_PER_PASS: usize = 64 * 1024;

/// The refused connections, kept a moment after their 503 by one thread for them all, and what
/// their clients send meanwhile read and thrown away. A socket closed with bytes it hasn't read
/// (the client's request, sent before it read anything) is reset on Windows and Linux both, and
/// a reset can reach the client before it reads the 503: it then sees only the reset. Read
/// empty, the socket closes cleanly once the client has read the 503 and closed its side, or at
/// its deadline.
///
/// The bound is held where a socket is handed over (`keep`, on the accept thread): one list,
/// oldest first, behind a mutex; past `LINGERING` the oldest is taken off there and closed at
/// once, and each deadline is stamped there, at the refusal. No queue sits in between, so
/// nothing waits uncounted however fast refusals come. The thread reads each socket where it
/// lies, under the lock, at most `READ_PER_PASS` (non-blocking, so a few reads), so the list
/// stays in refusal order and `keep` waits at most for one socket's read.
pub(crate) struct Refusals {
    shared: Arc<Kept>,
}

struct Kept {
    state: Mutex<KeptState>,
    more: std::sync::Condvar,
    linger: Duration,
}

struct KeptState {
    /// Oldest first: each one's number in refusal order, the socket, its deadline.
    list: std::collections::VecDeque<(u64, Connection, Instant)>,
    /// The number the next refused socket gets.
    next: u64,
    /// The server is gone: the thread ends once the list is empty.
    gone: bool,
}

impl Refusals {
    pub(crate) fn start() -> Refusals {
        Refusals::lingering(LINGER)
    }

    pub(crate) fn lingering(linger: Duration) -> Refusals {
        let shared = Arc::new(Kept {
            state: Mutex::new(KeptState {
                list: Default::default(),
                next: 0,
                gone: false,
            }),
            more: std::sync::Condvar::new(),
            linger,
        });
        let mine = Arc::clone(&shared);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                {
                    let mut s = mine.state.lock().unwrap();
                    while s.list.is_empty() && !s.gone {
                        s = mine.more.wait(s).unwrap();
                    }
                    if s.list.is_empty() {
                        return; // the server is gone, and nothing is kept
                    }
                }
                // One pass, oldest first: every socket kept, each read at most READ_PER_PASS and
                // its deadline looked at. `after` is the last one looked at, so one closed or
                // added by `keep` between two locks neither skips nor repeats one.
                let mut after: Option<u64> = None;
                loop {
                    let mut s = mine.state.lock().unwrap();
                    let at = match after {
                        None => 0,
                        Some(n) => s.list.partition_point(|(id, _, _)| *id <= n),
                    };
                    let Some((id, sock, until)) = s.list.get_mut(at) else {
                        break;
                    };
                    after = Some(*id);
                    let mut read = 0;
                    let open = loop {
                        if read >= READ_PER_PASS {
                            break true;
                        }
                        match sock.read(&mut buf) {
                            Ok(0) => break false, // the client closed: close too
                            Ok(k) => read += k,
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break true,
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(_) => break false,
                        }
                    };
                    let done = !open || Instant::now() >= *until;
                    let gone = if done { s.list.remove(at) } else { None };
                    drop(s);
                    drop(gone); // closed outside the lock
                }
                let s = mine.state.lock().unwrap();
                if !s.list.is_empty() {
                    let _ = mine.more.wait_timeout(s, Duration::from_millis(20));
                }
            }
        });
        Refusals { shared }
    }

    /// Keep a refused socket: a lock and a push. Past the bound, the oldest kept is taken off
    /// and closed here, after the lock is let go.
    pub(crate) fn keep(&self, sock: Connection) {
        let oldest = {
            let mut s = self.shared.state.lock().unwrap();
            let oldest = if s.list.len() >= LINGERING {
                s.list.pop_front()
            } else {
                None
            };
            let id = s.next;
            s.next += 1;
            s.list.push_back((id, sock, Instant::now() + self.shared.linger));
            oldest
        };
        self.shared.more.notify_one();
        drop(oldest);
    }

    /// The refused sockets held now.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.shared.state.lock().unwrap().list.len()
    }
}

impl Drop for Refusals {
    fn drop(&mut self) {
        if let Ok(mut s) = self.shared.state.lock() {
            s.gone = true;
        }
        self.shared.more.notify_one();
    }
}

/// A socket read that failed for waiting longer than the idle limit: `TimedOut` on Windows,
/// `WouldBlock` where the limit is SO_RCVTIMEO's EAGAIN (Linux, macOS).
pub(crate) fn timed_out(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

/// The first wait after an accept fails; doubled while they fail in a row, to
/// [`ACCEPT_WAIT_MOST`] (as Holon's data plane waits, and Go's `net/http`).
pub(crate) const ACCEPT_WAIT_FIRST: Duration = Duration::from_millis(5);

/// The longest wait between accepts that fail.
pub(crate) const ACCEPT_WAIT_MOST: Duration = Duration::from_secs(1);

/// The least time between a line about failed accepts and the next run's start being said. A
/// run that starts sooner is counted, not said, and the next line says how many there were. A
/// client that holds the server near its limit of file handles can end a run and start the next
/// as handles come and go; said each time, that is two lines a run with nothing to bound them.
pub(crate) const SAY_EVERY: Duration = Duration::from_secs(60);

/// Holon (PATCHED.md): the accepts that failed in a row, and how long the accept thread waits
/// before the next. An accept that fails for want of a file handle fails again at once, so it is
/// waited on, not tried again at once. A run of them is said as it starts and as it ends, unless
/// it starts within [`SAY_EVERY`] of the last line said: then it is counted, and said with the
/// next line, whenever that is (a run that ends once that long has passed says what came of the
/// ones not said). At most a line or two a minute, however often runs start.
pub(crate) struct Failing {
    at: String,
    /// The run now: the accepts failed in it, the next wait, and whether its start was said.
    run: Option<Run>,
    /// When a line was last said.
    said_at: Option<Instant>,
    /// The runs ended since then that weren't said, and the accepts that failed in them.
    unsaid: (u64, u64),
}

struct Run {
    failed: u64,
    wait: Duration,
    said: bool,
}

impl Failing {
    /// For the server listening at `at`.
    pub(crate) fn new(at: String) -> Failing {
        Failing { at, run: None, said_at: None, unsaid: (0, 0) }
    }

    /// An accept failed with `e` at `now`: what to say (at the start of a run only, and only past
    /// `SAY_EVERY`), and how long to wait before the next.
    pub(crate) fn failed(&mut self, e: &std::io::Error, now: Instant) -> (Option<String>, Duration) {
        if let Some(run) = self.run.as_mut() {
            run.failed += 1;
            run.wait = (run.wait * 2).min(ACCEPT_WAIT_MOST);
            return (None, run.wait);
        }
        let said = if self.due(now) {
            self.said_at = Some(now);
            let before = self.unsaid_text("; ");
            Some(format!(
                "holon: the control port ({}) could not accept a connection: {}; trying again, less often while it fails{}",
                self.at, e, before
            ))
        } else {
            None
        };
        self.run = Some(Run { failed: 1, wait: ACCEPT_WAIT_FIRST, said: said.is_some() });
        (said, ACCEPT_WAIT_FIRST)
    }

    /// An accept succeeded at `now`: what to say, if it ends a run of failures said, or one not
    /// said once `SAY_EVERY` has passed since the last line.
    pub(crate) fn accepted(&mut self, now: Instant) -> Option<String> {
        let run = self.run.take()?;
        if run.said {
            return Some(format!(
                "holon: the control port ({}) accepts connections again, after {} that failed",
                self.at, run.failed
            ));
        }
        self.unsaid.0 += 1;
        self.unsaid.1 += run.failed;
        if !self.due(now) {
            return None;
        }
        self.said_at = Some(now);
        let since = self.unsaid_text("; ");
        Some(format!("holon: the control port ({}) accepts connections again{}", self.at, since))
    }

    fn due(&self, now: Instant) -> bool {
        self.said_at.map_or(true, |at| now.saturating_duration_since(at) >= SAY_EVERY)
    }

    /// The runs not said since the last line, after `lead`, counted afresh; nothing if none.
    fn unsaid_text(&mut self, lead: &str) -> String {
        let (runs, failed) = std::mem::take(&mut self.unsaid);
        let times = match runs {
            0 => return String::new(),
            1 => "once more".to_string(),
            n => format!("{} more times", n),
        };
        format!("{}it couldn't for a while {} since the last of these lines ({} accepts that failed in all)", lead, times, failed)
    }
}
