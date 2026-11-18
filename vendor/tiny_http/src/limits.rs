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
//!   Bytes still coming, however slowly, keep it; a response, however long, isn't a read.

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
    /// Connections open at once, from everyone.
    pub connections: usize,
    /// Connections open at once from one address (all of a Unix socket's count as one).
    pub from_one: usize,
}

/// The longest a connection waits for its client's bytes. Past Holon's own deadline for a
/// request's body (`http::BODY_DEADLINE`, 30 s), so a body is cut off by that deadline and its
/// 408 first, never by this.
pub const IDLE: Duration = Duration::from_secs(60);

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
            connections: CONNECTIONS,
            from_one: FROM_ONE,
        }
    }
}

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

/// How long a refused connection is kept for its client to read its 503 and close.
const LINGER: Duration = Duration::from_secs(2);
/// Refused connections kept at once; past them, the oldest is closed.
const LINGERING: usize = 256;

/// The refused connections, kept a moment after their 503 by one thread for them all, and what
/// their clients send meanwhile read and thrown away. A socket closed with bytes it hasn't read
/// (the client's request, sent before it read anything) is reset on Windows and Linux both, and
/// a reset can reach the client before it reads the 503: it then sees only the reset. Read
/// empty, the socket closes cleanly once the client has read the 503 and closed its side, or at
/// `LINGER`.
pub(crate) struct Refusals {
    to: Mutex<std::sync::mpsc::Sender<Connection>>,
}

impl Refusals {
    pub(crate) fn start() -> Refusals {
        let (to, from) = std::sync::mpsc::channel::<Connection>();
        std::thread::spawn(move || {
            let mut kept: std::collections::VecDeque<(Connection, Instant)> = Default::default();
            let mut buf = [0u8; 4096];
            loop {
                // Wait for one when none is kept, else look again every 20 ms.
                let next = if kept.is_empty() {
                    from.recv().map_err(|_| ())
                } else {
                    match from.recv_timeout(Duration::from_millis(20)) {
                        Ok(sock) => Ok(sock),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(()),
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(()),
                    }
                };
                match next {
                    Ok(sock) => {
                        if kept.len() >= LINGERING {
                            kept.pop_front();
                        }
                        kept.push_back((sock, Instant::now() + LINGER));
                    }
                    Err(()) if kept.is_empty() => return, // the server is gone
                    Err(()) => {}
                }
                kept.retain_mut(|(sock, until)| {
                    loop {
                        match sock.read(&mut buf) {
                            Ok(0) => return false, // the client closed: close too
                            Ok(_) => continue,
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => return false,
                        }
                    }
                    Instant::now() < *until
                });
            }
        });
        Refusals { to: Mutex::new(to) }
    }

    pub(crate) fn keep(&self, sock: Connection) {
        if let Ok(to) = self.to.lock() {
            let _ = to.send(sock);
        }
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
