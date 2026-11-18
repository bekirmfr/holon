//! Holon (PATCHED.md): the limits on connections (limits.rs) and the response that ends one
//! (`client::Closer`), held against a real listener and real sockets.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::limits::{Limits, TOO_MANY, TOO_MANY_FROM_ONE};
use crate::{Header, Response, Server};

/// Longer than anything here should take, on a loaded machine too.
const PATIENCE: Duration = Duration::from_secs(10);

/// A server held to `limits` that answers `/think?ms=N` after reading its body and waiting N ms,
/// `/slow` with a body that streams for 1.6 s, `/close`
/// with `Connection: close`, a request with a body with its length, and anything else `ok`;
/// each request on a thread of its own, as rouille answers them.
fn serve(limits: Limits) -> (Arc<Server>, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = Arc::new(Server::from_listener_limited(listener, None, limits).unwrap());
    let at = server.server_addr().to_ip().unwrap();
    let inside = server.clone();
    thread::spawn(move || {
        for mut rq in inside.incoming_requests() {
            thread::spawn(move || {
                if let Some(ms) = rq.url().strip_prefix("/think?ms=") {
                    // Read what body it has, think, then answer.
                    let ms: u64 = ms.parse().unwrap();
                    let mut body = Vec::new();
                    let _ = rq.as_reader().read_to_end(&mut body);
                    thread::sleep(Duration::from_millis(ms));
                    let _ = rq.respond(Response::from_string(format!("thought {}", body.len())));
                } else if rq.url() == "/slow" {
                    let _ = rq.respond(Response::empty(200).with_data(Slow(8), None));
                } else if rq.url() == "/close" {
                    let close = Header::from_bytes(&b"Connection"[..], &b"close"[..]).unwrap();
                    let _ = rq.respond(Response::from_string("bye").with_header(close));
                } else if rq.body_length().unwrap_or(0) > 0 {
                    let mut body = Vec::new();
                    let read = rq.as_reader().read_to_end(&mut body);
                    let said = match read {
                        Ok(_) => format!("{}", body.len()),
                        Err(e) => format!("failed: {}", e),
                    };
                    let _ = rq.respond(Response::from_string(said));
                } else {
                    let _ = rq.respond(Response::from_string("ok"));
                }
            });
        }
    });
    (server, at)
}

/// A body of `n` chunks of ten bytes, one every 200 ms.
struct Slow(usize);

impl Read for Slow {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.0 == 0 {
            return Ok(0);
        }
        thread::sleep(Duration::from_millis(200));
        self.0 -= 1;
        buf[..10].copy_from_slice(b"0123456789");
        Ok(10)
    }
}

fn connect(at: SocketAddr) -> TcpStream {
    let s = TcpStream::connect(at).unwrap();
    s.set_read_timeout(Some(PATIENCE)).unwrap();
    s
}

/// One response read off `s`: its status, its head, its body (by its length, or chunked).
fn answer(s: &mut TcpStream) -> Result<(u16, String, Vec<u8>), String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match s.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            Ok(_) => return Err(format!("closed after {:?}", String::from_utf8_lossy(&head))),
            Err(e) => return Err(format!("no answer: {}", e)),
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let status: u16 = head[9..12].parse().unwrap();
    let lower = head.to_ascii_lowercase();
    let mut body = Vec::new();
    if let Some(at) = lower.find("content-length:") {
        let n: usize = lower[at + 15..].lines().next().unwrap().trim().parse().unwrap();
        body.resize(n, 0);
        s.read_exact(&mut body).map_err(|e| format!("body: {}", e))?;
    } else if lower.contains("transfer-encoding: chunked") {
        let mut rest = Vec::new();
        while !rest.ends_with(b"\r\n0\r\n\r\n") && rest != b"0\r\n\r\n" {
            match s.read(&mut byte) {
                Ok(1) => rest.push(byte[0]),
                Ok(_) => return Err("closed inside a chunked body".into()),
                Err(e) => return Err(format!("chunked body: {}", e)),
            }
        }
        // the chunks' data, without their sizes
        let text = String::from_utf8_lossy(&rest).into_owned();
        for (i, part) in text.split("\r\n").enumerate() {
            if i % 2 == 1 {
                body.extend_from_slice(part.as_bytes());
            }
        }
    }
    Ok((status, head, body))
}

/// What a read gives once the server has closed its side: the end of the stream (or, where a
/// platform resets instead, a reset), and how long it took.
fn closed_within(s: &mut TcpStream) -> Result<Duration, String> {
    let start = Instant::now();
    let mut buf = [0u8; 64];
    match s.read(&mut buf) {
        Ok(0) => Ok(start.elapsed()),
        Ok(n) => Err(format!("sent {:?}", String::from_utf8_lossy(&buf[..n]))),
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset
            || e.kind() == std::io::ErrorKind::ConnectionAborted => Ok(start.elapsed()),
        Err(e) => Err(format!("still open after {:?}: {}", start.elapsed(), e)),
    }
}

fn until(done: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while !done() {
        if start.elapsed() > PATIENCE {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
    true
}

fn get(path: &str) -> String {
    format!("GET {} HTTP/1.1\r\nHost: test\r\n\r\n", path)
}

/// A socket that never sends anything (a browser's preconnect, or one opened to hold a thread)
/// is closed once the idle limit passes, not before; one kept alive after a request is closed
/// once it has been idle that long, and a new one is answered.
#[test]
fn an_idle_connection_is_closed_after_the_limit_and_a_new_one_is_answered() {
    let idle = Duration::from_millis(500);
    let (server, at) = serve(Limits { idle: Some(idle), ..Limits::default() });

    let mut silent = connect(at);
    let took = closed_within(&mut silent).expect("a silent socket is closed");
    assert!(took >= idle - Duration::from_millis(50), "closed at {:?}, before the limit", took);

    let mut kept = connect(at);
    kept.write_all(get("/").as_bytes()).unwrap();
    assert_eq!(answer(&mut kept).unwrap().0, 200);
    let took = closed_within(&mut kept).expect("a kept-alive socket idle past the limit is closed");
    assert!(took >= idle - Duration::from_millis(50), "closed at {:?}, before the limit", took);

    let mut fresh = connect(at);
    fresh.write_all(get("/").as_bytes()).unwrap();
    assert_eq!(answer(&mut fresh).unwrap().0, 200, "a new connection is answered");
    assert!(until(|| server.open.count() <= 1), "closed connections give their places back");
    server.unblock();
}

/// The limit is on waiting, not on taking long: a request whose bytes come slowly but keep
/// coming, a body likewise, and a response that streams for longer than the limit all go
/// through whole.
#[test]
fn a_slow_but_steady_request_and_a_long_response_are_not_cut() {
    let idle = Duration::from_millis(600);
    let (server, at) = serve(Limits { idle: Some(idle), ..Limits::default() });

    // The request a byte every 100 ms (1.8 s in all).
    let mut s = connect(at);
    for b in get("/").bytes() {
        s.write_all(&[b]).unwrap();
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(answer(&mut s).unwrap().0, 200, "a request trickled in is answered");

    // A body of 2,000 bytes (past the 1 KiB tiny_http reads itself), 200 every 100 ms.
    let mut s = connect(at);
    s.write_all(b"POST /body HTTP/1.1\r\nHost: test\r\nContent-Length: 2000\r\n\r\n").unwrap();
    for _ in 0..10 {
        s.write_all(&[b'x'; 200]).unwrap();
        thread::sleep(Duration::from_millis(100));
    }
    let (status, _, body) = answer(&mut s).unwrap();
    assert_eq!((status, String::from_utf8_lossy(&body).into_owned()), (200, "2000".to_string()), "a body trickled in is read whole");

    // A response that streams for 1.6 s, past the limit, while the client sends nothing more.
    let mut s = connect(at);
    s.write_all(get("/slow").as_bytes()).unwrap();
    let (status, _, body) = answer(&mut s).expect("a long response arrives whole");
    assert_eq!((status, body.len()), (200, 80));
    server.unblock();
}

/// A response the application sends with `Connection: close` says so and is the connection's
/// last: the client reads to the end of the stream after it.
#[test]
fn a_response_with_connection_close_ends_its_connection() {
    let (server, at) = serve(Limits { idle: None, ..Limits::default() });
    let mut s = connect(at);
    s.write_all(get("/close").as_bytes()).unwrap();
    let (status, head, body) = answer(&mut s).unwrap();
    assert_eq!((status, body.as_slice()), (200, &b"bye"[..]));
    assert!(head.to_ascii_lowercase().contains("connection: close"), "the response says it: {}", head);
    let took = closed_within(&mut s).expect("the connection ends after it");
    assert!(took < Duration::from_secs(5), "ended at {:?}", took);
    server.unblock();
}

/// Past the bound from one address, a new connection is told 503 at once and closed, while
/// those inside it are still answered; once one closes, a new one is let in. Likewise the bound
/// on all connections.
#[test]
fn connections_past_a_bound_are_refused_at_once_and_those_inside_are_answered() {
    for &(connections, from_one, told) in &[(100, 4, TOO_MANY_FROM_ONE), (4, 100, TOO_MANY)] {
        let (server, at) = serve(Limits { idle: None, connections, from_one });
        let mut inside: Vec<TcpStream> = (0..4).map(|_| connect(at)).collect();
        for s in inside.iter_mut() {
            s.write_all(get("/").as_bytes()).unwrap();
            assert_eq!(answer(s).unwrap().0, 200);
        }
        let mut past = connect(at);
        let started = Instant::now();
        let (status, _, body) = answer(&mut past).expect("one past the bound is told");
        assert_eq!(status, 503);
        assert!(String::from_utf8_lossy(&body).contains(told), "{}", String::from_utf8_lossy(&body));
        assert!(started.elapsed() < Duration::from_secs(5), "told at once, not at {:?}", started.elapsed());
        closed_within(&mut past).expect("and closed");

        inside[0].write_all(get("/").as_bytes()).unwrap();
        assert_eq!(answer(&mut inside[0]).unwrap().0, 200, "one inside the bound is still answered");

        drop(inside.pop());
        assert!(until(|| server.open.count() < 4), "a closed connection gives its place back");
        let mut again = connect(at);
        again.write_all(get("/").as_bytes()).unwrap();
        assert_eq!(answer(&mut again).unwrap().0, 200, "a new one is let in");
        server.unblock();
    }
}

/// Sockets opened three times past the bound and left silent hold no more threads than the
/// bound lets connections be open: a thread a connection at most, never one a socket.
#[test]
fn sockets_past_the_bound_hold_no_threads() {
    let bound = 16;
    let (server, at) = serve(Limits { idle: None, connections: bound, from_one: bound });
    let mut sockets: Vec<TcpStream> = (0..3 * bound).map(|_| connect(at)).collect();
    // Each past the bound is told, and only then is everything accepted.
    let mut refused = 0;
    for s in sockets.iter_mut().skip(bound) {
        if let Ok((503, _, _)) = answer(s) {
            refused += 1;
        }
    }
    assert_eq!(refused, 2 * bound, "every socket past the bound is refused");
    assert_eq!(server.open.count(), bound);
    let threads = server.pool.threads();
    assert!(threads <= bound + 4, "{} threads for {} connections", threads, bound);
    // And those inside are served.
    sockets[0].write_all(get("/").as_bytes()).unwrap();
    assert_eq!(answer(&mut sockets[0]).unwrap().0, 200);
    server.unblock();
}

/// A connection owed an answer is not idle, whatever shape its request has (no body, a body
/// tiny_http reads itself, one the handler reads, a chunked one): a handler that thinks for twice
/// the limit is answered whole, the next request sent at once on the same connection is
/// answered, and the connection, left alone after that, is closed no sooner than the limit after
/// its last answer.
#[test]
fn a_connection_owed_an_answer_is_not_idle_whatever_its_request() {
    let idle = Duration::from_millis(500);
    let (server, at) = serve(Limits { idle: Some(idle), ..Limits::default() });
    let think = format!("/think?ms={}", 2 * idle.as_millis());
    let small = "x".repeat(100);
    let large = "x".repeat(2000);
    let shapes: Vec<(&str, String, usize)> = vec![
        ("no body", get(&think), 0),
        ("a body of 100 bytes", format!("POST {} HTTP/1.1\r\nHost: test\r\nContent-Length: 100\r\n\r\n{}", think, small), 100),
        ("a body of 2,000 bytes", format!("POST {} HTTP/1.1\r\nHost: test\r\nContent-Length: 2000\r\n\r\n{}", think, large), 2000),
        ("a chunked body", format!("POST {} HTTP/1.1\r\nHost: test\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n", think), 5),
    ];
    let runs: Vec<_> = shapes
        .into_iter()
        .map(|(shape, request, length)| {
            thread::spawn(move || -> Result<(), String> {
                let mut s = connect(at);
                s.write_all(request.as_bytes()).unwrap();
                let (status, _, body) = answer(&mut s).map_err(|e| format!("{}: the answer: {}", shape, e))?;
                let want = format!("thought {}", length);
                if status != 200 || String::from_utf8_lossy(&body) != want {
                    return Err(format!("{}: answered {} {:?}", shape, status, String::from_utf8_lossy(&body)));
                }
                s.write_all(get("/").as_bytes()).map_err(|e| format!("{}: the next request: {}", shape, e))?;
                let next = answer(&mut s).map_err(|e| format!("{}: the next request on it: {}", shape, e))?;
                if next.0 != 200 {
                    return Err(format!("{}: the next request answered {}", shape, next.0));
                }
                let took = closed_within(&mut s).map_err(|e| format!("{}: left alone: {}", shape, e))?;
                if took < idle - Duration::from_millis(50) {
                    return Err(format!("{}: closed {:?} after its last answer, before the limit", shape, took));
                }
                Ok(())
            })
        })
        .collect();
    let failed: Vec<String> = runs.into_iter().filter_map(|r| r.join().unwrap().err()).collect();
    assert!(failed.is_empty(), "{:#?}", failed);
    server.unblock();
}

/// Behind a proxy, every client's request comes from the proxy's one address, each on a
/// connection of its own: one address is let in as many connections as Holon answers requests
/// at once (512), and only the one past `FROM_ONE` is refused.
#[test]
fn one_address_is_let_in_every_request_holon_answers_at_once() {
    assert!(crate::limits::FROM_ONE >= 512, "one address holds Holon's MAX_IN_FLIGHT");
    let (server, at) = serve(Limits::default());
    let mut held: Vec<TcpStream> = (0..crate::limits::FROM_ONE).map(|_| connect(at)).collect();
    for (i, s) in held.iter_mut().enumerate() {
        s.write_all(get("/").as_bytes()).unwrap();
        assert_eq!(answer(s).map(|a| a.0), Ok(200), "connection {} from one address", i);
    }
    let mut past = connect(at);
    assert_eq!(answer(&mut past).map(|a| a.0), Ok(503), "the one past FROM_ONE");
    server.unblock();
}

/// A client past a bound that sends its request at once, before it reads (as every client
/// does), still reads the 503, whether its request is a line or a 256 KiB body it is still
/// sending when the 503 is written: what it sends is read off the socket and thrown away until it
/// closes, where a close with bytes unread would be a reset, cutting its sending and its reading.
#[test]
fn a_refused_client_that_sent_its_request_first_reads_the_503() {
    let (server, at) = serve(Limits { idle: None, connections: 100, from_one: 2 });
    let mut inside: Vec<TcpStream> = (0..2).map(|_| connect(at)).collect();
    for s in inside.iter_mut() {
        s.write_all(get("/").as_bytes()).unwrap();
        assert_eq!(answer(s).unwrap().0, 200);
    }
    let (mut told, mut other) = (0, Vec::new());
    let body = vec![b'x'; 256 * 1024];
    for i in 0..50 {
        let mut s = connect(at);
        let sent = if i % 2 == 0 {
            s.write_all(get("/").as_bytes())
        } else {
            let head = format!("POST /upload HTTP/1.1\r\nHost: test\r\nContent-Length: {}\r\n\r\n", body.len());
            s.write_all(head.as_bytes()).and_then(|_| s.write_all(&body))
        };
        if let Err(e) = sent {
            other.push(format!("{}: sending: {}", if i % 2 == 0 { "GET" } else { "POST" }, e));
            continue;
        }
        match answer(&mut s) {
            Ok((503, _, _)) => told += 1,
            Ok((status, _, _)) => other.push(format!("{}: {}", if i % 2 == 0 { "GET" } else { "POST" }, status)),
            Err(e) => other.push(format!("{}: {}", if i % 2 == 0 { "GET" } else { "POST" }, e)),
        }
    }
    eprintln!("refused clients that sent first: {} of 50 read the 503; the rest: {:?}", told, other);
    assert_eq!(told, 50, "every refused client reads its 503: {:?}", other);
    server.unblock();
}

/// Whether the server has closed its socket of a refused client (as against its end of the
/// stream, which every refused client reads after its 503): bytes sent to a closed socket are
/// answered with a reset, and bytes sent to a kept one are read and thrown away. Sends a byte
/// and reads, again, until a reset or `within`.
fn closed_by_the_server(c: &mut TcpStream, within: Duration) -> bool {
    let start = Instant::now();
    c.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    let mut buf = [0u8; 512];
    while start.elapsed() < within {
        if c.write_all(b"x").is_err() {
            return true;
        }
        match c.read(&mut buf) {
            Ok(_) => thread::sleep(Duration::from_millis(5)),
            Err(e) if crate::limits::timed_out(&e) => {}
            Err(_) => return true, // reset or aborted
        }
    }
    false
}

/// Refused sockets held never pass `LINGERING`, however many come and however fast, and one
/// client that sends without end holds nothing up. More than the bound are handed to `keep` as
/// the accept thread hands them (the first a client that never stops sending, the rest clients
/// that send nothing and stay open), with a linger far past the test, so no deadline closes
/// one. The count held never passes the bound after any `keep`; the oldest past it, after their
/// 503, are closed (closed when newer came, not at a deadline); the newest are kept.
#[test]
fn refused_sockets_held_never_pass_the_bound_and_the_oldest_are_closed_first() {
    use crate::connection::Connection;
    use crate::limits::{refuse, Refusals, LINGERING};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let at = listener.local_addr().unwrap();
    let refusals = Refusals::lingering(Duration::from_secs(3600));
    let extra = 40;
    let (mut clients, mut most) = (Vec::new(), 0);
    for i in 0..LINGERING + extra {
        let c = connect(at);
        let (s, _) = listener.accept().unwrap();
        if i == 0 {
            let mut w = c.try_clone().unwrap();
            thread::spawn(move || {
                let chunk = vec![b'x'; 1 << 20];
                while w.write_all(&chunk).is_ok() {}
            });
        }
        refusals.keep(refuse(Connection::from(s), TOO_MANY));
        most = most.max(refusals.held());
        clients.push(c);
    }
    assert!(most <= LINGERING, "refused sockets held at most {}, over the bound {}", most, LINGERING);
    // The oldest past the bound (the sender, the first, is closed with its bytes unread: a reset
    // it may meet before its 503, so it is left out).
    for (i, c) in clients.iter_mut().enumerate().take(extra).skip(1) {
        match answer(c) {
            Ok((503, _, _)) => {}
            other => panic!("refused client {}: {:?}", i, other),
        }
        assert!(closed_by_the_server(c, PATIENCE), "refused client {}, past the bound, still kept", i);
    }
    for (i, c) in clients.iter_mut().enumerate().skip(LINGERING + extra - 10) {
        match answer(c) {
            Ok((503, _, _)) => {}
            other => panic!("refused client {}: {:?}", i, other),
        }
        assert!(
            !closed_by_the_server(c, Duration::from_millis(300)),
            "refused client {}, inside the bound, closed",
            i
        );
    }
    assert_eq!(refusals.held(), LINGERING);
}
