//! Holon (PATCHED.md): what a server lets a connection hold. Each connection holds a thread of
//! the pool for as long as it is open (`client.rs` reads its requests one after another), so:
//!
//! - **Open connections** are counted, in all and from each address, and one past either bound
//!   is answered 503 and closed at once, not accepted and left without a thread.
//! - **An idle connection** is closed: a socket read that waits longer than `idle` fails, and a
//!   connection that has sent nothing of its next request by then (or nothing at all) is closed
//!   without an answer, as a server closes a kept-alive connection it no longer wants. The same
//!   limit bounds every wait for a request's bytes, its body's included: one that stops for
//!   longer is cut off (408 when tiny_http reads it, an error to the handler that reads it).
//!   Bytes still coming, however slowly, keep it; a response, however long, isn't a read.

use std::collections::HashMap;
use std::io::Write;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// Connections open at once from one address: a quarter of all. Everything on the server's
/// machine (the editor, the commands, a proxy in front) comes from one address, so this is
/// generous next to what one browser opens (six to a host) or a command (one or two).
pub const FROM_ONE: usize = CONNECTIONS / 4;

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

/// Tell a connection past a bound why, and close it. Written before anything is read: the
/// socket is new, so a few hundred bytes go into its empty send buffer without waiting.
pub(crate) fn refuse(mut sock: Connection, why: &str) {
    let body = format!("{{\"error\":\"{}\"}}", why);
    let answer = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nRetry-After: 1\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = sock.write_all(answer.as_bytes());
    let _ = sock.flush();
    let _ = sock.shutdown(std::net::Shutdown::Write);
}

/// A socket read that failed for waiting longer than the idle limit: `TimedOut` on Windows,
/// `WouldBlock` where the limit is SO_RCVTIMEO's EAGAIN (Linux, macOS).
pub(crate) fn timed_out(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}
