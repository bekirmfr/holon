//! WebSocket feeds (host-capabilities L4 §5.9.4): the client a feed's connection reads when its
//! URL is `wss://` (subscriptions.rs), over the socket and TLS sse.rs opens, with the handshake
//! and frames through `tungstenite`, so a connection the host closes is closed at once and one
//! that says nothing for a while is found out.
//!
//! - **The handshake** asks to upgrade, with the binding's headers and a login in the URL as Basic
//!   authentication, and must answer 101. A redirect isn't followed, as for SSE.
//! - **Once open,** the binding's subscription messages are sent, in order; its heartbeat every
//!   `--every`; and WebSocket's own pings are answered with their pongs.
//! - **A message** is a text frame's text or a binary frame's bytes. One past [`MOST`] ends the
//!   connection, since it can't be passed over unread; one past `sse::MAX_EVENT` is read and
//!   passed over, as an SSE event is.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use tungstenite::client::IntoClientRequest;
use tungstenite::handshake::HandshakeError;
use tungstenite::http::{HeaderName, HeaderValue};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};
use zeroize::Zeroizing;

use crate::sse::{self, Closer, Stream, Url};

/// The most a message may hold before the connection ends: what is read before it can be passed
/// over.
const MOST: usize = 16 * 1024 * 1024;

/// What a WebSocket said: a message, or one passed over, past `sse::MAX_EVENT`.
#[derive(Debug, PartialEq)]
pub(crate) enum Said {
    Text(String),
    Binary(Vec<u8>),
    PassedOver,
}

/// What the host sends to keep a connection: its message, and how often.
#[derive(Clone, Debug)]
pub(crate) struct Heartbeat {
    pub(crate) message: String,
    pub(crate) every: Duration,
}

/// An open WebSocket feed.
pub(crate) struct Connection {
    ws: WebSocket<Stream>,
    socket: TcpStream,
    idle: Duration,
    heartbeat: Option<Heartbeat>,
    /// When something last came.
    heard: Instant,
    /// When the next heartbeat is due.
    beat: Instant,
}

impl Connection {
    /// What shuts the connection from another thread.
    pub(crate) fn closer(&self) -> Option<Closer> {
        self.socket.try_clone().ok().map(Closer)
    }

    /// The next message; None when the feed ended the connection, or why it was read no further.
    pub(crate) fn next_message(&mut self) -> Result<Option<Said>, String> {
        loop {
            let now = Instant::now();
            if let Some(h) = &self.heartbeat
                && now >= self.beat
            {
                self.ws.send(Message::text(h.message.clone())).map_err(|e| format!("the heartbeat wasn't sent: {e}"))?;
                self.beat = now + h.every;
            }
            let quiet = self.heard + self.idle;
            if now >= quiet {
                return Err(format!("nothing came for {}", crate::folders::span_text(self.idle.as_secs())));
            }
            let mut wait = quiet - now;
            if self.heartbeat.is_some() {
                wait = wait.min(self.beat.saturating_duration_since(now));
            }
            self.ws.get_ref().set_read_timeout(wait.max(Duration::from_millis(10)));
            match self.ws.read() {
                Ok(message) => {
                    self.heard = Instant::now();
                    match message {
                        Message::Text(t) if t.len() > sse::MAX_EVENT => return Ok(Some(Said::PassedOver)),
                        Message::Binary(b) if b.len() > sse::MAX_EVENT => return Ok(Some(Said::PassedOver)),
                        Message::Text(t) => return Ok(Some(Said::Text(t.as_str().to_string()))),
                        Message::Binary(b) => return Ok(Some(Said::Binary(b.to_vec()))),
                        // A ping is answered as it is read; a close, once the feed closes after it.
                        Message::Ping(_) | Message::Pong(_) | Message::Close(_) | Message::Frame(_) => {}
                    }
                }
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Ok(None),
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {}
                Err(tungstenite::Error::Capacity(_)) => return Err(format!("the feed sent a message past {} MiB", MOST >> 20)),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

/// The WebSocket at `url`, asked for with `headers`; once open, sent each of `subscribe`, then
/// `heartbeat` as often as it says, and ended by a read that waits past `idle`. The error says
/// why it isn't open, never with the URL's login or query, nor a header's or message's value.
pub(crate) fn open(url: &Url, headers: &[(String, Zeroizing<String>)], subscribe: &[Zeroizing<String>], heartbeat: Option<Heartbeat>, idle: Duration) -> Result<Connection, String> {
    let address = url.address();
    let (stream, socket) = sse::connect(url)?;
    let uri = url.request_uri();
    let mut request = uri.as_str().into_client_request().map_err(|_| format!("{address} isn't a URL a WebSocket is opened at"))?;
    let asked = request.headers_mut();
    asked.insert("user-agent", HeaderValue::from_static("holon"));
    if let Some(basic) = url.basic()? {
        asked.insert("authorization", HeaderValue::from_str(&basic).map_err(|_| "the feed's login isn't one a header carries".to_string())?);
    }
    for (name, value) in headers {
        let header = HeaderName::from_bytes(name.as_bytes()).ok().filter(|_| sse::is_header_name(name)).ok_or_else(|| format!("{name:?} isn't a header's name"))?;
        if value.contains(['\r', '\n', '\0']) {
            return Err(format!("the header {name}'s value holds a line break"));
        }
        let value = HeaderValue::from_str(value).map_err(|_| format!("the header {name}'s value isn't one a header carries"))?;
        asked.append(header, value);
    }
    let config = WebSocketConfig::default().max_message_size(Some(MOST)).max_frame_size(Some(MOST));
    let mut ws = match tungstenite::client::client_with_config(request, stream, Some(config)) {
        Ok((ws, _)) => ws,
        Err(HandshakeError::Failure(tungstenite::Error::Http(answer))) => {
            let moved = if answer.headers().contains_key("location") { " (a redirect, which isn't followed: bind the URL it points to)" } else { "" };
            return Err(format!("{address} answered {}{moved}", answer.status().as_u16()));
        }
        Err(HandshakeError::Failure(tungstenite::Error::Io(e))) => return Err(format!("{address} didn't answer: {e}")),
        Err(HandshakeError::Failure(e)) => return Err(format!("{address} didn't answer as a WebSocket does: {e}")),
        Err(HandshakeError::Interrupted(_)) => return Err(format!("{address} didn't answer in time")),
    };
    for message in subscribe {
        ws.send(Message::text(message.to_string())).map_err(|e| format!("{address}: the subscription wasn't sent: {e}"))?;
    }
    let now = Instant::now();
    let beat = now + heartbeat.as_ref().map_or(Duration::ZERO, |h| h.every);
    Ok(Connection { ws, socket, idle, heartbeat, heard: now, beat })
}

/// A WebSocket feed in memory, for the tests: what it is sent goes to every connection open, and
/// it keeps each connection's request and what each sent it. A connection that asks for a
/// `cursor=N` in its query is sent again what it was sent from the message whose `seq` (or
/// `payload.seq`) is N, as Jetstream does.
#[cfg(test)]
pub(crate) mod stand_in {
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use tungstenite::handshake::server::{Request, Response};
    use tungstenite::Message;

    #[derive(Default)]
    struct Inner {
        /// Each message sent, in order.
        sent: Mutex<Vec<Message>>,
        /// Each connection's request: its path, then its headers as `Name: value` lines.
        asked: Mutex<Vec<String>>,
        /// What the connections sent it: a text message's text, `<pong>` for a pong.
        heard: Mutex<Vec<String>>,
        open: Mutex<Vec<TcpStream>>,
    }

    pub(crate) struct StandIn {
        port: u16,
        inner: Arc<Inner>,
    }

    impl StandIn {
        pub(crate) fn start() -> StandIn {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let inner = Arc::new(Inner::default());
            let shared = Arc::clone(&inner);
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let inner = Arc::clone(&shared);
                    std::thread::spawn(move || serve(stream, &inner));
                }
            });
            StandIn { port, inner }
        }

        pub(crate) fn url(&self, path: &str) -> String {
            format!("ws://127.0.0.1:{}{path}", self.port)
        }

        /// Send `message` to every connection open, and to none opened after.
        pub(crate) fn send(&self, message: Message) {
            self.inner.sent.lock().unwrap().push(message);
        }

        pub(crate) fn text(&self, text: &str) {
            self.send(Message::text(text.to_string()));
        }

        /// End every connection open, as a feed that drops its connections does.
        pub(crate) fn cut(&self) {
            for s in self.inner.open.lock().unwrap().drain(..) {
                let _ = s.shutdown(Shutdown::Both);
            }
        }

        pub(crate) fn asked(&self) -> Vec<String> {
            self.inner.asked.lock().unwrap().clone()
        }

        pub(crate) fn heard(&self) -> Vec<String> {
            self.inner.heard.lock().unwrap().clone()
        }

        pub(crate) fn open(&self) -> usize {
            self.inner.open.lock().unwrap().len()
        }
    }

    // The handshake's callback answers tungstenite's own error response, large as it is.
    #[allow(clippy::result_large_err)]
    fn serve(stream: TcpStream, inner: &Inner) {
        let Ok(socket) = stream.try_clone() else { return };
        // What it was asked, and where it starts, are known before it answers: a test that reads
        // them once its connection is open finds them, and what is sent after is sent.
        let mut next = 0;
        let record = |req: &Request, res: Response| {
            let mut head = req.uri().to_string();
            for (name, value) in req.headers() {
                head.push_str(&format!("\n{}: {}", name.as_str(), value.to_str().unwrap_or("")));
            }
            let cursor = req.uri().query().and_then(|q| q.split('&').find_map(|p| p.strip_prefix("cursor="))).and_then(|c| c.parse::<f64>().ok());
            let sent = inner.sent.lock().unwrap();
            let seq = |m: &Message| {
                let v: serde_json::Value = serde_json::from_str(m.to_text().ok()?).ok()?;
                v["seq"].as_f64().or(v["payload"]["seq"].as_f64())
            };
            next = cursor.and_then(|c| sent.iter().position(|m| seq(m).is_some_and(|s| s >= c))).unwrap_or(sent.len());
            inner.asked.lock().unwrap().push(head);
            Ok(res)
        };
        let Ok(mut ws) = tungstenite::accept_hdr(stream, record) else { return };
        inner.open.lock().unwrap().push(socket.try_clone().unwrap());
        // On the socket the reads use: on Windows, a clone's timeout is its own.
        let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(20)));
        loop {
            let pending: Vec<Message> = inner.sent.lock().unwrap()[next..].to_vec();
            next += pending.len();
            for m in pending {
                if ws.send(m).is_err() {
                    return gone(&socket, inner);
                }
            }
            match ws.read() {
                Ok(Message::Text(t)) => inner.heard.lock().unwrap().push(t.as_str().to_string()),
                Ok(Message::Pong(_)) => inner.heard.lock().unwrap().push("<pong>".into()),
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {}
                Err(_) => return gone(&socket, inner),
            }
        }
    }

    fn gone(socket: &TcpStream, inner: &Inner) {
        let at = socket.peer_addr().ok();
        inner.open.lock().unwrap().retain(|s| s.peer_addr().ok() != at);
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::StandIn;
    use super::*;

    fn eventually(what: &str, done: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < until, "never: {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_websocket_is_opened_with_its_headers_and_subscription_and_read_as_it_comes() {
        let feed = StandIn::start();
        let url = Url::parse(&feed.url("/v1/prices?key=s3cret").replace("ws://", "ws://trader:pa%40ss@")).unwrap();
        assert_eq!(url.address(), format!("{}", feed.url("/v1/prices")));
        let headers = vec![("X-Desk".to_string(), Zeroizing::new("fx".to_string()))];
        let subscribe = [Zeroizing::new(r#"{"type":"subscribe","product_ids":["BTC-USD"]}"#.to_string())];
        let mut conn = open(&url, &headers, &subscribe, None, Duration::from_secs(5)).unwrap();
        eventually("the subscription", || feed.heard().len() == 1);
        assert_eq!(feed.heard(), [r#"{"type":"subscribe","product_ids":["BTC-USD"]}"#]);
        let asked = &feed.asked()[0];
        assert!(asked.starts_with("/v1/prices?key=s3cret"), "{asked}");
        assert!(asked.contains("x-desk: fx") && asked.contains(&format!("authorization: Basic {}", crate::smtp::base64(b"trader:pa@ss"))), "{asked}");
        feed.text(r#"{"product_id":"BTC-USD","price":"43000.10","sequence":41}"#);
        feed.send(Message::binary(vec![0u8, 1, 2]));
        feed.send(Message::Ping(vec![9u8].into()));
        feed.text("plain");
        assert_eq!(conn.next_message().unwrap(), Some(Said::Text(r#"{"product_id":"BTC-USD","price":"43000.10","sequence":41}"#.into())));
        assert_eq!(conn.next_message().unwrap(), Some(Said::Binary(vec![0, 1, 2])));
        assert_eq!(conn.next_message().unwrap(), Some(Said::Text("plain".into())));
        eventually("the ping answered", || feed.heard().contains(&"<pong>".to_string()));
        // Past 1 MiB: passed over, the rest read on.
        feed.text(&"x".repeat(sse::MAX_EVENT + 1));
        feed.text("after");
        assert_eq!(conn.next_message().unwrap(), Some(Said::PassedOver));
        assert_eq!(conn.next_message().unwrap(), Some(Said::Text("after".into())));
        // Cut: ended. Closed from another thread: ended at once.
        feed.cut();
        assert!(conn.next_message().map_or(true, |m| m.is_none()));
        eventually("the connection gone", || feed.open() == 0);
        let mut conn = open(&url, &headers, &subscribe, None, Duration::from_secs(5)).unwrap();
        eventually("the subscription sent again", || feed.heard().iter().filter(|h| h.contains("subscribe")).count() == 2);
        conn.closer().unwrap().close();
        assert!(conn.next_message().map_or(true, |m| m.is_none()));
    }

    #[test]
    fn a_heartbeat_goes_out_when_due_and_a_quiet_feed_is_found_out() {
        let feed = StandIn::start();
        let url = Url::parse(&feed.url("/")).unwrap();
        let heartbeat = Heartbeat { message: "ping".into(), every: Duration::from_millis(100) };
        let mut conn = open(&url, &[], &[], Some(heartbeat), Duration::from_millis(700)).unwrap();
        // Nothing comes: heartbeats go out while it waits, then it is found quiet.
        let why = conn.next_message().unwrap_err();
        assert!(why.starts_with("nothing came for"), "{why}");
        assert!(feed.heard().iter().filter(|h| *h == "ping").count() >= 3, "{:?}", feed.heard());
    }

    #[test]
    fn what_isnt_a_websocket_is_refused_and_says_so_without_the_query() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (i, s) in listener.incoming().flatten().enumerate() {
                let mut s = s;
                let mut buf = [0u8; 2048];
                let _ = std::io::Read::read(&mut s, &mut buf);
                let answer: &[u8] = if i == 0 { b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok" } else { b"HTTP/1.1 301 Moved\r\nLocation: wss://elsewhere/\r\nContent-Length: 0\r\n\r\n" };
                let _ = std::io::Write::write_all(&mut s, answer);
            }
        });
        let url = Url::parse(&format!("ws://127.0.0.1:{port}/feed?key=s3cret")).unwrap();
        let plain = open(&url, &[], &[], None, Duration::from_secs(5)).err().unwrap();
        assert_eq!(plain, format!("ws://127.0.0.1:{port}/feed answered 200"));
        let moved = open(&url, &[], &[], None, Duration::from_secs(5)).err().unwrap();
        assert!(moved.contains("answered 301 (a redirect") && !moved.contains("s3cret"), "{moved}");
        assert!(Url::parse("ws://stream.example.com/").err().unwrap().contains("plain ws:// only to this machine"));
        assert!(Url::parse("wss://stream.example.com/").unwrap().is_websocket());
    }
}
