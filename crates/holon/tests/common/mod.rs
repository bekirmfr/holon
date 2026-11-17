//! What every test binary that starts `holon serve` shares: how long to wait for what is to come,
//! a free port, a request over a socket and its answer's status, and the wait for a server to
//! answer. One copy, so a test file that starts a server waits as the others do (a fourth copy,
//! surfaces.rs's, asked once and failed on a runner where the server hadn't answered yet).
//! Each binary uses some of it.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

/// Longer than this, what a test waits for is taken never to come: a guard so a hang fails the
/// test, never a measure of how long anything took (on a loaded machine anything takes longer).
pub const EVENTUALLY: Duration = Duration::from_secs(60);

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Waits [`EVENTUALLY`] for `holon serve` (`child`) to answer on `port`: an answer, not a
/// connection taken. It takes connections once it has its port, before it has written the run
/// file its user's commands sign in with (local.rs), and answers them after (main.rs: the run
/// file published, then the server run), so a command sent once a connection is taken can find
/// no run file, and is told the server isn't set up. A connection refused (it hasn't its port
/// yet) is waited past too. If it ends first, fails at once with how it ended and what it said
/// (its stderr, kept in `said`), rather than waiting out the guard with nothing to say.
pub fn answering(child: &mut Child, port: u16, said: &Path) {
    let by = Instant::now() + EVENTUALLY;
    let words = || std::fs::read_to_string(said).unwrap_or_default();
    while status(&raw_http(port, b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")) == 0 {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("holon serve ended before it answered on {port} ({status}): {}", words());
        }
        assert!(Instant::now() < by, "holon serve answers on {port}: {}", words());
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `request` sent to `port` and the whole answer read, or nothing if no connection is taken.
pub fn raw_http(port: u16, request: &[u8]) -> String {
    raw_http_then(port, request, || {})
}

/// [`raw_http`], calling `sent` once the request is written (or at once, when no connection is
/// taken).
pub fn raw_http_then(port: u16, request: &[u8], sent: impl FnOnce()) -> String {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        sent();
        return String::new();
    };
    stream.set_read_timeout(Some(EVENTUALLY)).unwrap();
    let _ = stream.write_all(request);
    sent();
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// An answer's status, 0 when there is none.
pub fn status(answered: &str) -> u16 {
    answered.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0)
}
