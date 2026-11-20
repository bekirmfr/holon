//! Holon (PATCHED.md): the limits on connections (limits.rs) and the response that ends one
//! (`client::Closer`), held against a real listener and real sockets.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::limits::{Limits, TOO_MANY, TOO_MANY_FROM_ONE};
use crate::{Header, Response, Server};

/// What a test here waits for, it waits this long for: a guard against a hang, not a measure, so
/// a loaded machine (a suite on every core) takes what it takes within it. Holon's own tests wait
/// as long (`EVENTUALLY` in crates/holon/src/tests.rs, whose timing sweep reads this file).
const EVENTUALLY: Duration = Duration::from_secs(60);

/// A server held to `limits` that answers `/think?ms=N` after reading its body and waiting N ms,
/// `/slow` with a body that streams for 1.6 s, `/close`
/// with `Connection: close`, `/ignore` with `ignored` (reading none of its body, as a route that
/// refuses a request before its body does), a request with a body with its length, and anything
/// else `ok`; each request on a thread of its own, as rouille answers them.
fn serve(limits: Limits) -> (Arc<Server>, SocketAddr) {
    serve_on(TcpListener::bind("127.0.0.1:0").unwrap(), limits)
}

/// The same, on `listener`.
fn serve_on(listener: TcpListener, limits: Limits) -> (Arc<Server>, SocketAddr) {
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
                } else if rq.url() == "/ignore" {
                    let _ = rq.respond(Response::from_string("ignored"));
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
    s.set_read_timeout(Some(EVENTUALLY)).unwrap();
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
        if start.elapsed() > EVENTUALLY {
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
    // Before the read gives up (EVENTUALLY): nothing else would end it, with no idle limit here
    // and no request begun on it.
    closed_within(&mut s).expect("the connection ends after it");
    server.unblock();
}

/// Past the bound from one address, a new connection is told 503 at once and closed, while
/// those inside it are still answered; once one closes, a new one is let in. Likewise the bound
/// on all connections.
#[test]
fn connections_past_a_bound_are_refused_at_once_and_those_inside_are_answered() {
    for &(connections, from_one, told) in &[(100, 4, TOO_MANY_FROM_ONE), (4, 100, TOO_MANY)] {
        let (server, at) = serve(Limits { idle: None, request: None, connections, from_one, ..Limits::default() });
        let mut inside: Vec<TcpStream> = (0..4).map(|_| connect(at)).collect();
        for s in inside.iter_mut() {
            s.write_all(get("/").as_bytes()).unwrap();
            assert_eq!(answer(s).unwrap().0, 200);
        }
        // Told while the four inside still hold their places (no idle limit, none closed): at
        // once, not once a place is free.
        let mut past = connect(at);
        let (status, _, body) = answer(&mut past).expect("one past the bound is told");
        assert_eq!(status, 503);
        assert!(String::from_utf8_lossy(&body).contains(told), "{}", String::from_utf8_lossy(&body));
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
    let (server, at) = serve(Limits { idle: None, request: None, connections: bound, from_one: bound, ..Limits::default() });
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
/// tiny_http reads itself, one the handler reads): a handler that thinks for twice
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

/// A request trickled a byte every 100 ms, each well within the idle limit, is told 408 at its
/// deadline and closed while its bytes are still coming, whether what trickles is its head or a
/// body tiny_http reads itself (1 KiB or less); another connection is answered meanwhile.
/// Without the deadline, each byte would keep it for as long as its client liked: here until it
/// ran out of bytes, 20 s in at the soonest. The client sends its next byte only once it has
/// waited 100 ms with no answer, so it sends nothing the server stops reading, which would reset
/// the connection on its close, a reset that can reach the client before it reads the 408. The
/// idle limit is the default's (60 s): no wait here comes near it, so only the deadline cuts.
#[test]
fn a_request_that_trickles_is_told_408_at_its_deadline_while_another_is_answered() {
    let deadline = Duration::from_millis(1500);
    let (server, at) = serve(Limits { request: Some(deadline), ..Limits::default() });
    let shapes = [
        ("its head", "GET / HTTP/1.1\r\nHost: test\r\nX-Slow: "),
        ("a body of 200 bytes", "POST /body HTTP/1.1\r\nHost: test\r\nContent-Length: 200\r\n\r\n"),
    ];
    for (shape, first) in shapes {
        let mut s = connect(at);
        let started = Instant::now();
        s.write_all(first.as_bytes()).unwrap();
        // not a bound: the client's tick, after which it sends a byte and looks again
        s.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let mut sent = 0;
        let mut byte = [0u8; 1];
        while sent < 200 {
            match s.peek(&mut byte) {
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => panic!("{}: {}", shape, e),
            }
            if sent == 3 {
                let mut other = connect(at);
                other.write_all(get("/").as_bytes()).unwrap();
                assert_eq!(answer(&mut other).map(|a| a.0), Ok(200), "{}: another connection is answered meanwhile", shape);
            }
            s.write_all(b"x").unwrap();
            sent += 1;
        }
        s.set_read_timeout(Some(EVENTUALLY)).unwrap();
        let told = answer(&mut s).map(|(status, _, body)| (status, String::from_utf8_lossy(&body).into_owned()));
        let took = started.elapsed();
        let why = r#"{"error":"the request didn't arrive whole within 1.5 seconds of its first byte"}"#;
        assert_eq!(told, Ok((408, why.to_string())), "{}: told 408, and why", shape);
        assert!(sent < 200, "{}: told at {:?}, once its bytes had stopped, not at its deadline", shape, took);
        assert!(took >= deadline - Duration::from_millis(50), "{}: told at {:?}, before its deadline", shape, took);
        closed_within(&mut s).unwrap_or_else(|e| panic!("{}: closed after the 408: {}", shape, e));
    }
    server.unblock();
}

/// A client whose request is cut off while it is still sending, as one cut off is as a rule,
/// reads the 408 whole, its words too, and can go on sending until it has: the server reads off
/// what it sends meanwhile, until it closes or for `LINGER` (2 s). Closed with those bytes
/// unread, the connection would be reset: on Windows a reset throws away a 408 the client hasn't
/// read yet; Linux keeps it readable, but there the closed socket answers the client's next byte
/// with a reset, and its write after that fails (a broken pipe). The client here sends a byte
/// every 50 ms throughout and reads only 1 s after its deadline: the linger is 30 s here, so the
/// client's read and its last byte come well inside it however the machine is loaded (29 s to
/// spare), while without one the reset comes within 50 ms of the 408. The idle limit is the
/// default's (60 s).
#[test]
fn a_client_still_sending_when_its_request_is_cut_off_reads_the_408() {
    let deadline = Duration::from_millis(1200);
    let (server, at) = serve(Limits { request: Some(deadline), linger: Duration::from_secs(30), ..Limits::default() });
    let mut s = connect(at);
    s.write_all(b"GET / HTTP/1.1\r\nHost: test\r\nX-Slow: ").unwrap();
    let mut sending = s.try_clone().unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopped = stop.clone();
    let sender = thread::spawn(move || -> Result<(), String> {
        while !stopped.load(std::sync::atomic::Ordering::SeqCst) {
            sending.write_all(b"x").map_err(|e| format!("a byte sent failed: {}", e))?;
            thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    });
    thread::sleep(deadline + Duration::from_millis(1000));
    let told = answer(&mut s).map(|(status, _, body)| (status, String::from_utf8_lossy(&body).into_owned()));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let sent = sender.join().unwrap();
    let why = r#"{"error":"the request didn't arrive whole within 1.2 seconds of its first byte"}"#;
    assert_eq!(told, Ok((408, why.to_string())), "read whole, though the client was still sending");
    assert_eq!(sent, Ok(()), "the client could send until it had read the 408");
    server.unblock();
}

/// The deadline is each request's, from its first byte: a kept-alive connection is answered
/// request after request, the next sent after a pause longer than the deadline; a body the
/// application reads (past 1 KiB) is held to the idle limit alone, however long it takes in all,
/// and so is the next request after it, and so is one on its connection's last request (where no
/// read of a next request begins, which would clear the deadline too); and a head that arrives in
/// pieces, each read of it
/// waiting what is left of its deadline, is answered. The waits that must pass the deadline are
/// at least that long however the machine is loaded; the idle limit is the default's (60 s), and
/// the head in pieces has a deadline of 60 s, so no wait here comes near a limit it isn't about.
#[test]
fn the_deadline_is_each_requests_and_not_on_a_body_the_application_reads() {
    let deadline = Duration::from_millis(800);
    let (server, at) = serve(Limits { request: Some(deadline), ..Limits::default() });
    let mut s = connect(at);
    for i in 0..3 {
        s.write_all(get("/").as_bytes()).unwrap();
        assert_eq!(answer(&mut s).map(|a| a.0), Ok(200), "request {} on one connection", i);
        // past the deadline
        thread::sleep(deadline + Duration::from_millis(200));
    }

    // 2,000 bytes, 200 every 200 ms: 2 s, past the deadline.
    s.write_all(b"POST /body HTTP/1.1\r\nHost: test\r\nContent-Length: 2000\r\n\r\n").unwrap();
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(200));
        s.write_all(&[b'x'; 200]).unwrap();
    }
    let (status, _, body) = answer(&mut s).unwrap();
    assert_eq!((status, String::from_utf8_lossy(&body).into_owned()), (200, "2000".to_string()), "a body the application reads is read whole");

    s.write_all(get("/").as_bytes()).unwrap();
    assert_eq!(answer(&mut s).map(|a| a.0), Ok(200), "the next request after it");

    let mut last = connect(at);
    last.write_all(b"POST /body HTTP/1.1\r\nHost: test\r\nConnection: close\r\nContent-Length: 2000\r\n\r\n").unwrap();
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(200));
        last.write_all(&[b'x'; 200]).unwrap();
    }
    let (status, _, body) = answer(&mut last).unwrap();
    assert_eq!((status, String::from_utf8_lossy(&body).into_owned()), (200, "2000".to_string()), "a body on its connection's last request is read whole");
    server.unblock();

    let (server, at) = serve(Limits { request: Some(Duration::from_secs(60)), ..Limits::default() });
    let mut s = connect(at);
    for piece in ["GET / HT", "TP/1.1\r\nHost: te", "st\r\n\r\n"] {
        s.write_all(piece.as_bytes()).unwrap();
        thread::sleep(Duration::from_millis(150));
    }
    assert_eq!(answer(&mut s).map(|a| a.0), Ok(200), "a head in pieces, within its deadline");
    server.unblock();
}

/// What is left of a body nobody read is read off by its connection before its next request,
/// with a small buffer, at most `leftover` bytes and by the request limit from then; past either,
/// or with no end to find, the connection ends once its answer is out, nothing more said on it,
/// and the server answers on. Upstream read the rest off into a buffer the request's
/// `Content-Length` sized: a length of 100 TB was an allocation that failed and ended the
/// process. A request inside a body is never read as one (`/inner` would be answered `ok`). The
/// request and idle limits are an hour here, so a rest read off or waited for wrongly fails the
/// test by its client's wait (`EVENTUALLY`), not by a limit.
#[test]
fn what_is_left_of_a_body_nobody_read_is_read_off_within_bounds_or_its_connection_ends() {
    let hour = Some(Duration::from_secs(3600));
    let (server, at) = serve(Limits { idle: hour, request: hour, leftover: 2000, ..Limits::default() });
    let inner = get("/inner");
    let body = |n: usize| format!("{}{}", inner, "x".repeat(n - inner.len()));
    let shapes: Vec<(&str, String, bool)> = vec![
        ("a length of 100 TB, none of it sent", "GET /ignore HTTP/1.1\r\nHost: test\r\nContent-Length: 100000000000000\r\n\r\n".to_string(), false),
        ("1,500 bytes, within the bound", format!("POST /ignore HTTP/1.1\r\nHost: test\r\nContent-Length: 1500\r\n\r\n{}", body(1500)), true),
        ("3,000 bytes, past the bound", format!("POST /ignore HTTP/1.1\r\nHost: test\r\nContent-Length: 3000\r\n\r\n{}", body(3000)), false),
        // within the bound, so only its end being unknown ends its connection
        ("a body its client was never told to send", "POST /ignore HTTP/1.1\r\nHost: test\r\nExpect: 100-continue\r\nContent-Length: 1500\r\n\r\n".to_string(), false),
    ];
    for (shape, request, goes_on) in shapes {
        let mut s = connect(at);
        s.write_all(request.as_bytes()).unwrap();
        let told = answer(&mut s).map(|(status, _, body)| (status, String::from_utf8_lossy(&body).into_owned()));
        assert_eq!(told, Ok((200, "ignored".to_string())), "{}: answered", shape);
        if goes_on {
            // an answer of its own (`/inner`, hidden in the body, would be answered `ok`)
            s.write_all(get("/think?ms=0").as_bytes()).unwrap();
            let next = answer(&mut s).map(|(status, _, body)| (status, String::from_utf8_lossy(&body).into_owned()));
            assert_eq!(next, Ok((200, "thought 0".to_string())), "{}: the next request on it", shape);
        } else {
            closed_within(&mut s).unwrap_or_else(|e| panic!("{}: the connection ends after its answer: {}", shape, e));
        }
        let mut again = connect(at);
        again.write_all(get("/").as_bytes()).unwrap();
        assert_eq!(answer(&mut again).map(|a| a.0), Ok(200), "{}: the server answers on", shape);
    }

    // The rest arriving after the answer, within the bound: read off as it comes, and the
    // connection goes on.
    let mut s = connect(at);
    s.write_all(b"POST /ignore HTTP/1.1\r\nHost: test\r\nContent-Length: 1500\r\n\r\nx").unwrap();
    assert_eq!(answer(&mut s).map(|a| a.0), Ok(200), "answered before the rest of its body");
    s.write_all(&[b'x'; 1499]).unwrap();
    s.write_all(get("/think?ms=0").as_bytes()).unwrap();
    let next = answer(&mut s).map(|(status, _, body)| (status, String::from_utf8_lossy(&body).into_owned()));
    assert_eq!(next, Ok((200, "thought 0".to_string())), "the next request, after the rest");
    server.unblock();
}

/// A connection's last request is answered whole before the connection ends, whatever its shape:
/// last by `Connection: close` or by HTTP 1.0, with no body (which lets go of the stream at once,
/// before it is answered), a body read, a body not read, or a body past the bound; its answer
/// taking a while (a handler that thinks 300 ms first). Then the end of the stream.
#[test]
fn a_connections_last_request_is_answered_whole_before_it_ends() {
    let (server, at) = serve(Limits { leftover: 2000, ..Limits::default() });
    let shapes: Vec<(&str, String, &str)> = vec![
        ("Connection: close, no body", "GET /think?ms=300 HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n".to_string(), "thought 0"),
        ("HTTP 1.0, no body", "GET /think?ms=300 HTTP/1.0\r\nHost: test\r\n\r\n".to_string(), "thought 0"),
        ("Connection: close, a body read", format!("POST /think?ms=300 HTTP/1.1\r\nHost: test\r\nConnection: close\r\nContent-Length: 1500\r\n\r\n{}", "x".repeat(1500)), "thought 1500"),
        ("Connection: close, a body not read", format!("POST /ignore HTTP/1.1\r\nHost: test\r\nConnection: close\r\nContent-Length: 1500\r\n\r\n{}", "x".repeat(1500)), "ignored"),
        ("Connection: close, a body past the bound", format!("POST /ignore HTTP/1.1\r\nHost: test\r\nConnection: close\r\nContent-Length: 3000\r\n\r\n{}", "x".repeat(3000)), "ignored"),
    ];
    for (shape, request, said) in shapes {
        let mut s = connect(at);
        s.write_all(request.as_bytes()).unwrap();
        let told = answer(&mut s).map(|(status, _, body)| (status, String::from_utf8_lossy(&body).into_owned()));
        assert_eq!(told, Ok((200, said.to_string())), "{}: answered whole", shape);
        closed_within(&mut s).unwrap_or_else(|e| panic!("{}: then the end of the stream: {}", shape, e));
    }
    server.unblock();
}

/// A request whose line says a version past 1.1 is told 505, and its connection ends and gives
/// its place back. Upstream wrote the 505 through a writer that waits for the request's own, while
/// it held the request: the connection waited for ever, its place held after its client had gone
/// (512 such requests from one address locked it out until a restart).
#[test]
fn a_version_past_1_1_is_told_505_and_its_connection_ends() {
    let (server, at) = serve(Limits::default());
    for version in ["HTTP/2.0", "HTTP/3.0"] {
        let mut s = connect(at);
        s.write_all(format!("GET / {}\r\nHost: test\r\n\r\n", version).as_bytes()).unwrap();
        assert_eq!(answer(&mut s).map(|a| a.0), Ok(505), "{}: told 505", version);
        closed_within(&mut s).unwrap_or_else(|e| panic!("{}: then the end of the stream: {}", version, e));
        drop(s);
        assert!(until(|| server.open.count() == 0), "{}: its place given back", version);
    }
    server.unblock();
}

/// A request whose head, or whose body's length, can be read more than one way is refused, and
/// its connection ends, nothing after it read as a request: a proxy in front that read it the
/// other way would pass on, inside what it took for a body, a request this server then answers as
/// its own ("request smuggling"; RFC 9112 6.3, 5.2, RFC 9110 5.5, 8.6). A body is taken only by
/// one `Content-Length` of digits, and any `Transfer-Encoding` is told 411. Each request here is
/// followed on its connection by `/inner`: after one refused, its status, why in words, and the
/// end of the stream; after one taken, its body read whole and `/inner` answered as the next
/// request.
#[test]
fn a_request_that_can_be_read_two_ways_is_refused_and_nothing_after_it_read() {
    let (server, at) = serve(Limits::default());
    let chunks = "5\r\nhello\r\n0\r\n\r\n";
    let post = |headers: &str, body: &str| format!("POST /think?ms=0 HTTP/1.1\r\nHost: test\r\n{}\r\n{}", headers, body);
    // what it is, the request, the status it is told, and its answer when taken
    let rows: Vec<(&str, String, u16, &str)> = vec![
        ("a length", post("Content-Length: 5\r\n", "hello"), 200, "thought 5"),
        ("a length with leading zeros", post("Content-Length: 005\r\n", "hello"), 200, "thought 5"),
        ("chunked", post("Transfer-Encoding: chunked\r\n", chunks), 411, ""),
        ("chunked, in capitals", post("TRANSFER-ENCODING: CHUNKED\r\n", chunks), 411, ""),
        ("a coding other than chunked", post("Transfer-Encoding: gzip\r\n", chunks), 411, ""),
        ("an empty Transfer-Encoding", post("Transfer-Encoding: \r\n", chunks), 411, ""),
        ("chunked and a length", post("Transfer-Encoding: chunked\r\nContent-Length: 5\r\n", chunks), 411, ""),
        ("a length and chunked", post("Content-Length: 5\r\nTransfer-Encoding: chunked\r\n", chunks), 411, ""),
        ("chunked in HTTP 1.0", format!("POST /think?ms=0 HTTP/1.0\r\nHost: test\r\nConnection: keep-alive\r\nTransfer-Encoding: chunked\r\n\r\n{}", chunks), 411, ""),
        ("a length with a sign", post("Content-Length: +5\r\n", "hello"), 400, ""),
        ("a length in words", post("Content-Length: five\r\n", "hello"), 400, ""),
        ("a length past any number", post("Content-Length: 99999999999999999999999\r\n", "hello"), 400, ""),
        ("two lengths in one field", post("Content-Length: 5, 5\r\n", "hello"), 400, ""),
        ("two lengths in two fields", post("Content-Length: 5\r\nContent-Length: 5\r\n", "hello"), 400, ""),
        ("a space before the colon", post("Content-Length : 5\r\n", "hello"), 400, ""),
        ("a header folded onto the line before", post("X-Note: a\r\n Content-Length: 5\r\n", "hello"), 400, ""),
        ("a lone LF inside a header line", post("X-Note: a\nContent-Length: 5\r\n", "hello"), 400, ""),
        ("a lone CR inside a header line", post("X-Note: a\rContent-Length: 5\r\n", "hello"), 400, ""),
        ("a NUL inside a header line", post("X-Note: a\0b\r\nContent-Length: 5\r\n", "hello"), 400, ""),
        ("an Expect other than 100-continue", post("Expect: 100 continue\r\nContent-Length: 5\r\n", "hello"), 417, ""),
    ];
    for (what, request, status, said) in rows {
        let mut s = connect(at);
        s.write_all(format!("{}{}", request, get("/inner")).as_bytes()).unwrap();
        let (told, _, body) = answer(&mut s).unwrap_or_else(|e| panic!("{}: an answer: {}", what, e));
        let body = String::from_utf8_lossy(&body).into_owned();
        assert_eq!(told, status, "{}: told {} ({})", what, status, body);
        if status == 200 {
            assert_eq!(body, said, "{}: its body read whole", what);
            let next = answer(&mut s).map(|(status, _, body)| (status, String::from_utf8_lossy(&body).into_owned()));
            assert_eq!(next, Ok((200, "ok".to_string())), "{}: what follows it, read as the next request", what);
        } else {
            assert!(body.starts_with("{\"error\":"), "{}: told why: {}", what, body);
            closed_within(&mut s).unwrap_or_else(|e| panic!("{}: then the end of the stream, nothing after it read: {}", what, e));
        }
    }
    server.unblock();
}

/// What is left of a body that trickles in after its answer, each byte well within the idle
/// limit, is read off no longer than the request limit from when it begins: the connection ends
/// while its bytes still come and gives its place back. And when the answer closed the
/// connection (`Connection: close`, as Holon's 408 for a body too slow does), none of it is read
/// off: the connection ends at once, after the linger (`LINGER`, 2 s). Upstream read on for as
/// long as each byte came within the idle limit. The client sends a byte every 100 ms, 200 at
/// most (20 s, where the bound and the linger take 3.5 s here); the second case's request limit
/// is an hour, so only a connection that ends at once ends in time.
#[test]
fn what_is_left_of_a_body_that_trickles_is_cut_off_and_its_place_given_back() {
    let cases = [
        ("/ignore", Limits { request: Some(Duration::from_millis(1500)), ..Limits::default() }),
        ("/close", Limits { request: Some(Duration::from_secs(3600)), ..Limits::default() }),
    ];
    for (path, limits) in cases {
        let (server, at) = serve(limits);
        let mut s = connect(at);
        s.write_all(format!("POST {} HTTP/1.1\r\nHost: test\r\nContent-Length: 5000\r\n\r\nx", path).as_bytes()).unwrap();
        assert_eq!(answer(&mut s).map(|a| a.0), Ok(200), "{}: answered", path);
        let mut sent = 0;
        while sent < 200 && server.open.count() > 0 {
            if s.write_all(b"x").is_err() {
                break;
            }
            sent += 1;
            thread::sleep(Duration::from_millis(100));
        }
        assert!(until(|| server.open.count() == 0), "{}: its place is given back", path);
        assert!(sent < 200, "{}: its place given back while its bytes still came, not once they stopped", path);
        server.unblock();
    }
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
    let (server, at) = serve(Limits { idle: None, request: None, connections: 100, from_one: 2, ..Limits::default() });
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
    // not a bound: a tick of the loop below, which sends a byte and reads again until `within`
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
        assert!(closed_by_the_server(c, EVENTUALLY), "refused client {}, past the bound, still kept", i);
    }
    for (i, c) in clients.iter_mut().enumerate().skip(LINGERING + extra - 10) {
        match answer(c) {
            Ok((503, _, _)) => {}
            other => panic!("refused client {}: {:?}", i, other),
        }
        // not a bound: nothing is to close them (they are kept an hour); a loaded machine only
        // looks less
        assert!(
            !closed_by_the_server(c, Duration::from_millis(300)),
            "refused client {}, inside the bound, closed",
            i
        );
    }
    assert_eq!(refusals.held(), LINGERING);
}

/// An accept that fails doesn't end the server. Upstream's accept thread ended at the first: its
/// listener closed, and the error it queued ended the loop over the requests (rouille's `run()`
/// as this one), so nothing was answered again and nothing said so. Here every accept fails for
/// a while, an error from the system each time (the listener made non-blocking, with no
/// connection waiting: `WouldBlock`), then the listener blocks again (through a clone kept of
/// it), and the next connection is answered.
///
/// Unix only: there a clone is the same open socket, its mode shared. On Windows a clone is a
/// socket of its own (`WSADuplicateSocket`), its mode not the listener's, and making the
/// listener itself non-blocking for good would hand the connection a non-blocking socket, whose
/// reads fail as if timed out. The waits and what is said are held on every system by the test
/// after this one; the code that fails over is the same.
#[cfg(unix)]
#[test]
fn an_accept_that_fails_is_waited_on_and_the_next_connection_answered() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let same = listener.try_clone().unwrap();
    listener.set_nonblocking(true).unwrap();
    let kind = listener.accept().map(|_| ()).map_err(|e| e.kind());
    assert_eq!(kind, Err(std::io::ErrorKind::WouldBlock), "the listener made non-blocking");
    let (server, at) = serve_on(listener, Limits::default());
    // Accepts fail, waited on 5, 10, 20, 40 and 80 ms.
    thread::sleep(Duration::from_millis(150));
    // The clone is the same open socket: the listener blocks again.
    same.set_nonblocking(false).unwrap();
    let mut c = connect(at);
    c.write_all(get("/").as_bytes()).unwrap();
    assert_eq!(answer(&mut c).map(|(status, ..)| status), Ok(200), "the next connection is answered");
    server.unblock();
}

/// Accepts that fail are said once a run of them, as it starts and as it ends, and waited on
/// longer each time, to a second: an accept that fails for want of a file handle fails again at
/// once, and a loop that tried again at once would spin on a core. A run that starts within a
/// minute of the last line is counted, not said, and the next line says how many: runs that end
/// and start again as handles come and go get a line or two a minute, not two a run.
#[test]
fn accepts_that_fail_are_said_once_a_run_and_waited_on_longer_each_time() {
    use crate::limits::{Failing, ACCEPT_WAIT_FIRST, ACCEPT_WAIT_MOST, SAY_EVERY};
    let e = std::io::Error::new(std::io::ErrorKind::Other, "no file handle left");
    let t0 = Instant::now();
    let at = |s: u64| t0 + Duration::from_secs(s);
    let mut failing = Failing::new("127.0.0.1:7".to_string());
    assert_eq!(failing.accepted(at(0)), None, "nothing said while none failed");

    // A run: its start said, with where and why; waited on longer each time; its end said.
    let (said, wait) = failing.failed(&e, at(0));
    let said = said.expect("the first is said");
    assert!(said.contains("127.0.0.1:7") && said.contains("no file handle left"), "{}", said);
    assert_eq!(wait, ACCEPT_WAIT_FIRST);
    let mut waits = vec![wait];
    for _ in 0..11 {
        let (said, wait) = failing.failed(&e, at(0));
        assert_eq!(said, None, "said once a run");
        waits.push(wait);
    }
    assert!(waits.windows(2).all(|w| w[1] == (w[0] * 2).min(ACCEPT_WAIT_MOST)), "{:?}", waits);
    assert_eq!(waits.last(), Some(&ACCEPT_WAIT_MOST), "{:?}", waits);
    let again = failing.accepted(at(1)).expect("its end is said");
    assert!(again.contains("after 12 that failed"), "{}", again);
    assert_eq!(failing.accepted(at(1)), None, "said once");

    // Runs within a minute of the last line: counted, not said, start or end; each waits afresh.
    for (s, failures) in [(2, 1), (3, 3)] {
        for i in 0..failures {
            let (said, wait) = failing.failed(&e, at(s));
            assert_eq!(said, None, "a run {} s after the last line, said", s);
            if i == 0 {
                assert_eq!(wait, ACCEPT_WAIT_FIRST, "a new run waits afresh");
            }
        }
        assert_eq!(failing.accepted(at(s)), None, "the end of a run not said, said");
    }

    // Past a minute, the next run's start is said, with the two before it.
    let later = SAY_EVERY.as_secs() + 1;
    let (said, _) = failing.failed(&e, at(later));
    let said = said.expect("a run past a minute is said");
    assert!(said.contains("2 more times") && said.contains("(4 accepts that failed in all)"), "{}", said);
    let again = failing.accepted(at(later)).expect("its end is said");
    assert!(again.contains("after 1 that failed"), "{}", again);

    // Runs that keep coming: one more not said; when one ends a minute after the last line, what
    // came of those not said is said, once.
    failing.failed(&e, at(later + 1));
    assert_eq!(failing.accepted(at(later + 1)), None);
    failing.failed(&e, at(later + 2));
    failing.failed(&e, at(later + 2));
    let quiet = failing.accepted(at(later + 1 + SAY_EVERY.as_secs())).expect("said a minute on");
    assert!(quiet.contains("accepts connections again") && quiet.contains("2 more times") && quiet.contains("(3 accepts that failed in all)"), "{}", quiet);
    let (said, _) = failing.failed(&e, at(later + 2 + SAY_EVERY.as_secs()));
    assert_eq!(said, None, "within a minute of that line");
}
