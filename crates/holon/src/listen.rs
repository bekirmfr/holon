//! A Postgres channel as a feed's connection (host-capabilities L4 §5.9.11): `LISTEN` on a client
//! of its own, opened with a bound database's connection, each notification's payload a message.
//!
//! - **Its own session:** a `LISTEN` belongs to the session that said it, so it is never one of
//!   the database's pooled connections (engines.rs), and TLS is theirs (`engines::pg_tls`).
//! - **Read a second at a time,** so a close from another thread ([`Connection::closer`]) is seen
//!   within one. After the binding's `--idle` with none, the server is asked whether the
//!   connection is still there, and it is opened again only when it doesn't answer: Postgres keeps
//!   no notification for a session that isn't listening, so opening it again for quiet would lose
//!   what was sent meanwhile.
//! - **No position:** what commits while no connection listens is never sent; a feed's
//!   `--sequence` counts it (`e.gap`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use postgres::fallible_iterator::FallibleIterator;

use crate::engines::pg_said;
use crate::sse::Closer;

/// How long a read waits for a notification before it looks whether it was closed.
const WAIT: Duration = Duration::from_secs(1);

/// How long the server has to say the connection is still there.
const ASKED: Duration = Duration::from_secs(10);

/// The longest name Postgres keeps, in bytes.
const NAME_MOST: usize = 63;

/// Whether `name` is a channel a binding listens on: letters, digits and `_`, up to 63 bytes.
pub(crate) fn is_channel(name: &str) -> bool {
    !name.is_empty() && name.len() <= NAME_MOST && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// A session listening on a channel.
pub(crate) struct Connection {
    client: postgres::Client,
    closed: Arc<AtomicBool>,
    /// How long it may hear nothing before the server is asked whether it is still there.
    idle: Duration,
    heard: Instant,
}

/// A session on the database at `url` listening on `channel`, or why there is none.
pub(crate) fn open(url: &str, channel: &str, idle: Duration) -> Result<Connection, String> {
    let mut config: postgres::Config = url.parse().map_err(|e| format!("the database's connection isn't a Postgres URL: {e}"))?;
    config.connect_timeout(Duration::from_secs(5)).application_name("holon (listen)");
    let mut client = config.connect(crate::engines::pg_tls()).map_err(|e| format!("the database can't be reached: {}", pg_said(&e)))?;
    client.batch_execute(&format!("LISTEN \"{channel}\"")).map_err(|e| format!("the database didn't listen on {channel}: {}", pg_said(&e)))?;
    Ok(Connection { client, closed: Arc::new(AtomicBool::new(false)), idle, heard: Instant::now() })
}

impl Connection {
    /// What closes it from another thread: its next read ends within a second.
    pub(crate) fn closer(&self) -> Closer {
        Closer::Flag(Arc::clone(&self.closed))
    }

    /// The next notification's payload; None once it is closed, or why it was read no further.
    pub(crate) fn next_payload(&mut self) -> Result<Option<String>, String> {
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let next = self.client.notifications().timeout_iter(WAIT).next().map(|n| n.map(|n| n.payload().to_string()));
            match next {
                Ok(Some(payload)) => {
                    self.heard = Instant::now();
                    return Ok(Some(payload));
                }
                Ok(None) if self.heard.elapsed() < self.idle => {}
                Ok(None) => {
                    self.client.is_valid(ASKED).map_err(|e| format!("the database didn't answer after {} quiet: {}", crate::folders::span_text(self.idle.as_secs()), pg_said(&e)))?;
                    self.heard = Instant::now();
                }
                Err(e) => return Err(format!("the connection was lost: {}", pg_said(&e))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_is_a_name_postgres_keeps() {
        assert!(is_channel("ledger_entries") && is_channel("Ledger2") && is_channel(&"c".repeat(63)));
        for not in ["", "ledger-entries", "ledger entries", "a\"b", "ledger.entries", &"c".repeat(64)] {
            assert!(!is_channel(not), "{not:?}");
        }
    }
}
