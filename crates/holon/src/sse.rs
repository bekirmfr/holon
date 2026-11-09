//! Server-sent events (host-capabilities L4 §5.9.2): the client a feed's connection reads
//! (subscriptions.rs), written here over the `rustls` and `webpki-roots` the mail and the
//! databases use, so a connection the host closes is closed at once and one that says nothing
//! for a while is found out.
//!
//! - **The request:** a `GET` with `Accept: text/event-stream`, `Last-Event-ID` when the host knows
//!   where the feed is, and the headers the binding reads from their sources (feeds.rs).
//!   `https://`, plain `http://` only to this machine; a login in the URL is sent as Basic
//!   authentication.
//! - **The answer** is 200 with `text/event-stream`, its body chunked, of a length, or up to the
//!   close. A redirect isn't followed: the headers it would carry are the binding's.
//! - **The events** are read as the standard says ([`Parser`]): `data:` lines joined with a
//!   newline, `id:` (one holding a NUL ignored), `event:`, `retry:`, comments, and a blank line
//!   that dispatches what came before it when it has data. Lines end in CRLF, LF or CR. An event
//!   past [`MAX_EVENT`], or one of its lines, is passed over and the rest read on; nothing coming
//!   for the binding's `--idle` ends the connection.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::time::Duration;

use zeroize::Zeroizing;

/// The longest line, and the most data one event holds.
pub(crate) const MAX_EVENT: usize = 1024 * 1024;

/// The headers a request sends: each name, and its value read from its source.
pub(crate) type Headers = Vec<(String, Zeroizing<String>)>;

/// How long connecting, and the answer's head, may take.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The most an answer's status line and headers hold.
const MAX_HEAD: usize = 64 * 1024;

const USAGE: &str = "a feed's URL is https://host/path, or wss://host/path for a WebSocket (plain http:// and ws:// only to this machine), a login in it if the feed asks for one";

/// Where a feed is, as its binding's source holds it: server-sent events (`https://`) or a
/// WebSocket (`wss://`, ws.rs). Its `Debug` holds no login or query.
pub(crate) struct Url {
    tls: bool,
    /// A WebSocket's (`wss://`, `ws://`).
    ws: bool,
    /// As the URL writes it: `stream.example.com`, `[::1]`.
    host: String,
    port: u16,
    /// The path and query sent: `/v1/prices?key=…`.
    target: String,
    /// `user:password` from the URL, sent as Basic authentication.
    login: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.address())
    }
}

impl Url {
    /// The feed `url` names, or why it names none. The error never holds the URL.
    pub(crate) fn parse(url: &str) -> Result<Url, String> {
        let (scheme, rest) = url.trim().split_once("://").ok_or(USAGE)?;
        let (tls, ws) = match scheme.to_ascii_lowercase().as_str() {
            "https" => (true, false),
            "http" => (false, false),
            "wss" => (true, true),
            "ws" => (false, true),
            _ => return Err(USAGE.into()),
        };
        let rest = rest.split('#').next().unwrap_or("");
        let at = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, target) = rest.split_at(at);
        let target = if target.starts_with('/') { target.to_string() } else { format!("/{target}") };
        if target.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err("a feed's URL holds a space or a control character".into());
        }
        let (login, host_port) = match authority.rsplit_once('@') {
            Some((l, h)) => (Some(Zeroizing::new(l.to_string())), h),
            None => (None, authority),
        };
        let (host, port) = match host_port.strip_prefix('[') {
            Some(v6) => {
                let (ip, after) = v6.split_once(']').ok_or("a feed's URL names an IPv6 address without its ]")?;
                (format!("[{ip}]"), after.strip_prefix(':'))
            }
            None => match host_port.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (host_port.to_string(), None),
            },
        };
        let host = host.to_ascii_lowercase();
        if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_[]:".contains(&b)) {
            return Err("a feed's URL names no host it can reach".into());
        }
        let port = match port {
            None | Some("") => if tls { 443 } else { 80 },
            Some(p) => p.parse().map_err(|_| "a feed's URL names a port that isn't one")?,
        };
        let url = Url { tls, ws, host, port, target, login };
        if !tls && !url.is_loopback() {
            return Err(match ws {
                true => "a WebSocket feed is read over wss://: plain ws:// only to this machine",
                false => "a feed is read over https://: plain http:// only to this machine",
            }
            .into());
        }
        Ok(url)
    }

    /// Whether it is a WebSocket's (`wss://`, `ws://`).
    pub(crate) fn is_websocket(&self) -> bool {
        self.ws
    }

    /// The whole URL a request names, its query with it but no login: `wss://host:8443/v1?key=…`.
    pub(crate) fn request_uri(&self) -> Zeroizing<String> {
        let scheme = match (self.ws, self.tls) {
            (true, true) => "wss",
            (true, false) => "ws",
            (false, true) => "https",
            (false, false) => "http",
        };
        Zeroizing::new(format!("{scheme}://{}:{}{}", self.host, self.port, self.target))
    }

    /// The `Authorization` a login in the URL sends: `Basic …`.
    pub(crate) fn basic(&self) -> Result<Option<Zeroizing<String>>, String> {
        let Some(login) = &self.login else { return Ok(None) };
        let decoded = crate::smtp::unpercent(login).map_err(|_| "the feed's login has a % that isn't %XX".to_string())?;
        Ok(Some(Zeroizing::new(format!("Basic {}", crate::smtp::base64(decoded.as_bytes())))))
    }

    fn is_loopback(&self) -> bool {
        let name = self.host.trim_start_matches('[').trim_end_matches(']');
        name == "localhost" || name.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }

    /// Where it points, without its login or query: `https://stream.example.com/v1/prices`.
    pub(crate) fn address(&self) -> String {
        let scheme = match (self.ws, self.tls) {
            (true, true) => "wss",
            (true, false) => "ws",
            (false, true) => "https",
            (false, false) => "http",
        };
        let port = if self.port == if self.tls { 443 } else { 80 } { String::new() } else { format!(":{}", self.port) };
        let path = self.target.split('?').next().unwrap_or("/");
        format!("{scheme}://{}{port}{path}", self.host)
    }
}

/// The connection's socket, shut from another thread to end the connection at once.
pub(crate) struct Closer(pub(crate) TcpStream);

impl Closer {
    pub(crate) fn close(&self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

/// An event, as the feed sent it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Event {
    /// Its `data:` lines, joined with a newline.
    pub(crate) data: String,
    /// Its name: `message` unless an `event:` line names it.
    pub(crate) name: String,
    /// Its own id, when an `id:` line gave it one.
    pub(crate) id: Option<String>,
    /// Where the feed is once it is read (the standard's last event ID): what `Last-Event-ID`
    /// sends when the connection opens again. Empty: nowhere the feed said.
    pub(crate) last_id: String,
}

/// The events of a stream, read line by line as the standard says.
#[derive(Default)]
pub(crate) struct Parser {
    data: String,
    name: String,
    id: Option<String>,
    last_id: String,
    /// The wait before opening the connection again the feed asked for, in milliseconds.
    retry: Option<u64>,
    /// The event being read is past [`MAX_EVENT`]: its lines are dropped until it ends.
    too_long: bool,
}

/// What a line dispatched: an event, or one passed over, past [`MAX_EVENT`].
#[derive(Debug, PartialEq)]
pub(crate) enum Next {
    Event(Event),
    PassedOver,
}

impl Parser {
    /// A parser that knows the feed is at `last_id` (a reconnect's).
    pub(crate) fn at(last_id: &str) -> Parser {
        Parser { last_id: last_id.to_string(), ..Parser::default() }
    }

    /// One line, its end taken off: what it dispatches, if it does.
    pub(crate) fn line(&mut self, line: &str) -> Option<Next> {
        if line.is_empty() {
            return self.dispatch();
        }
        if self.too_long || line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        if line.len() > MAX_EVENT || (field == "data" && self.data.len() + value.len() + 1 > MAX_EVENT) {
            (self.too_long, self.data) = (true, String::new());
            return None;
        }
        match field {
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            "event" => self.name = value.to_string(),
            "id" if !value.contains('\0') => {
                self.id = Some(value.to_string());
                self.last_id = value.to_string();
            }
            "retry" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => self.retry = value.parse().ok(),
            _ => {}
        }
        None
    }

    /// The event the lines since the last blank one make, if they have data.
    fn dispatch(&mut self) -> Option<Next> {
        let name = std::mem::take(&mut self.name);
        let id = self.id.take();
        if std::mem::take(&mut self.too_long) {
            return Some(Next::PassedOver);
        }
        if self.data.is_empty() {
            return None;
        }
        let mut data = std::mem::take(&mut self.data);
        data.pop();
        Some(Next::Event(Event { data, name: if name.is_empty() { "message".into() } else { name }, id, last_id: self.last_id.clone() }))
    }

    /// The wait before opening the connection again the feed asked for, if it asked.
    pub(crate) fn retry(&self) -> Option<Duration> {
        self.retry.map(Duration::from_millis)
    }
}

/// The lines of a stream: ended by CRLF, LF or CR, a byte-order mark at its start dropped.
pub(crate) struct Lines<R> {
    inner: R,
    after_cr: bool,
    first: bool,
}

impl<R: BufRead> Lines<R> {
    pub(crate) fn new(inner: R) -> Lines<R> {
        Lines { inner, after_cr: false, first: true }
    }

    /// The next whole line, or None at the stream's end (a line it cuts off is dropped). A line
    /// past [`MAX_EVENT`] is kept to a byte past it, the rest dropped, for the parser to pass over.
    pub(crate) fn next_line(&mut self) -> std::io::Result<Option<String>> {
        let mut line = Vec::new();
        loop {
            let buf = self.inner.fill_buf()?;
            if buf.is_empty() {
                return Ok(None);
            }
            let (mut used, mut ended) = (0, false);
            for &b in buf {
                used += 1;
                if std::mem::take(&mut self.after_cr) && b == b'\n' {
                    continue;
                }
                match b {
                    b'\n' => ended = true,
                    b'\r' => (ended, self.after_cr) = (true, true),
                    _ if line.len() <= MAX_EVENT => line.push(b),
                    _ => {}
                }
                if ended {
                    break;
                }
            }
            self.inner.consume(used);
            if ended {
                if std::mem::take(&mut self.first) && line.starts_with("\u{feff}".as_bytes()) {
                    line.drain(..3);
                }
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
        }
    }
}

/// A connection to a feed's host: plain, or TLS on top.
pub(crate) enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    /// How long a read waits. Set on the socket a read uses: on Windows, a clone's is its own.
    pub(crate) fn set_read_timeout(&self, wait: Duration) {
        let _ = match self {
            Stream::Plain(s) => s.set_read_timeout(Some(wait)),
            Stream::Tls(s) => s.sock.set_read_timeout(Some(wait)),
        };
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// A chunked body, its chunks' sizes taken off.
struct Chunked<R> {
    inner: R,
    /// What is left of the chunk being read.
    left: usize,
    /// A chunk was read whole, its CRLF not yet: read before the next size, so a chunk is handed
    /// on as soon as it comes.
    crlf: bool,
    done: bool,
}

impl<R: BufRead> Chunked<R> {
    fn size_line(&mut self) -> std::io::Result<String> {
        let mut line = Vec::new();
        (&mut self.inner).take(1024).read_until(b'\n', &mut line)?;
        if !line.ends_with(b"\n") {
            return Err(std::io::Error::new(ErrorKind::UnexpectedEof, "the feed's answer ended inside a chunk's size"));
        }
        Ok(String::from_utf8_lossy(&line).trim().to_string())
    }
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        if self.left == 0 {
            if std::mem::take(&mut self.crlf) {
                self.size_line()?;
            }
            let line = self.size_line()?;
            let size = usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16)
                .map_err(|_| std::io::Error::new(ErrorKind::InvalidData, "the feed's answer has a chunk without its size"))?;
            if size == 0 {
                self.done = true;
                return Ok(0);
            }
            self.left = size;
        }
        let want = buf.len().min(self.left);
        let n = self.inner.read(&mut buf[..want])?;
        if n == 0 {
            return Err(std::io::Error::new(ErrorKind::UnexpectedEof, "the feed's answer ended inside a chunk"));
        }
        self.left -= n;
        self.crlf = self.left == 0;
        Ok(n)
    }
}

/// An open feed: its events as they come.
pub(crate) struct Connection {
    lines: Lines<Box<dyn BufRead + Send>>,
    parser: Parser,
    socket: TcpStream,
    idle: Duration,
}

impl Connection {
    /// What shuts the connection from another thread.
    pub(crate) fn closer(&self) -> Option<Closer> {
        self.socket.try_clone().ok().map(Closer)
    }

    /// The next event, or one passed over; None when the feed ended the connection, or why it
    /// was read no further.
    pub(crate) fn next_event(&mut self) -> Result<Option<Next>, String> {
        loop {
            let line = match self.lines.next_line() {
                Ok(Some(l)) => l,
                Ok(None) => return Ok(None),
                Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
                    return Err(format!("nothing came for {}", crate::folders::span_text(self.idle.as_secs())));
                }
                Err(e) => return Err(e.to_string()),
            };
            if let Some(next) = self.parser.line(&line) {
                return Ok(Some(next));
            }
        }
    }

    /// The wait before opening it again the feed asked for, if it asked.
    pub(crate) fn retry(&self) -> Option<Duration> {
        self.parser.retry()
    }
}

/// Whether `name` is a header's name HTTP takes.
pub(crate) fn is_header_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// A connection to `url`'s host, TLS for `https://` and `wss://`, the certificate checked against
/// the public CAs; and its socket, to close it from another thread.
pub(crate) fn connect(url: &Url) -> Result<(Stream, TcpStream), String> {
    let address = url.address();
    let unreachable = |why: String| format!("{address} can't be reached: {why}");
    let connect_to = url.host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<_> = (connect_to, url.port).to_socket_addrs().map_err(|e| unreachable(e.to_string()))?.collect();
    let mut last = "it has no address".to_string();
    let mut tcp = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, TIMEOUT) {
            Ok(t) => {
                tcp = Some(t);
                break;
            }
            Err(e) => last = e.to_string(),
        }
    }
    let tcp = tcp.ok_or_else(|| unreachable(last))?;
    let _ = tcp.set_read_timeout(Some(TIMEOUT));
    let _ = tcp.set_write_timeout(Some(TIMEOUT));
    let _ = tcp.set_nodelay(true);
    let socket = tcp.try_clone().map_err(|e| unreachable(e.to_string()))?;
    let stream = match url.tls {
        true => Stream::Tls(Box::new(crate::smtp::tls(connect_to, tcp).map_err(|f| f.why().to_string())?)),
        false => Stream::Plain(tcp),
    };
    Ok((stream, socket))
}

/// The feed at `url`, asked for with `headers` and, when the host knows where the feed is,
/// `Last-Event-ID: last_id`; once open, a read that waits past `idle` ends it. The error says
/// why it isn't open, never with the URL's login or query, nor a header's value.
pub(crate) fn open(url: &Url, headers: &[(String, Zeroizing<String>)], last_id: Option<&str>, idle: Duration) -> Result<Connection, String> {
    let address = url.address();
    let unreachable = |why: String| format!("{address} can't be reached: {why}");
    let (mut stream, socket) = connect(url)?;
    let host = match url.port == if url.tls { 443 } else { 80 } {
        true => url.host.clone(),
        false => format!("{}:{}", url.host, url.port),
    };
    let mut request = Zeroizing::new(format!("GET {} HTTP/1.1\r\nHost: {host}\r\nAccept: text/event-stream\r\nCache-Control: no-cache\r\nUser-Agent: holon\r\n", url.target));
    if let Some(basic) = url.basic()? {
        request.push_str(&format!("Authorization: {}\r\n", basic.as_str()));
    }
    if let Some(id) = last_id.filter(|i| !i.is_empty() && !i.contains(['\r', '\n'])) {
        request.push_str(&format!("Last-Event-ID: {id}\r\n"));
    }
    for (name, value) in headers {
        if !is_header_name(name) {
            return Err(format!("{name:?} isn't a header's name"));
        }
        if value.contains(['\r', '\n', '\0']) {
            return Err(format!("the header {name}'s value holds a line break"));
        }
        request.push_str(&format!("{name}: {}\r\n", value.as_str()));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).and_then(|_| stream.flush()).map_err(|e| unreachable(e.to_string()))?;

    let mut reader = BufReader::new(stream);
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = (&mut reader).take(MAX_HEAD as u64).read_until(b'\n', &mut line).map_err(|e| format!("{address} didn't answer: {e}"))?;
        if n == 0 {
            return Err(format!("{address} closed the connection without an answer"));
        }
        let line = String::from_utf8_lossy(&line).trim_end().to_string();
        if line.is_empty() {
            break;
        }
        head.push(line);
        if head.iter().map(String::len).sum::<usize>() > MAX_HEAD {
            return Err(format!("{address} answered with a head past {} KiB", MAX_HEAD >> 10));
        }
    }
    let status = head.first().and_then(|s| s.split_whitespace().nth(1)).and_then(|c| c.parse::<u16>().ok()).ok_or_else(|| format!("{address} didn't answer as HTTP does"))?;
    let header = |name: &str| head.iter().skip(1).find_map(|h| h.split_once(':').filter(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()));
    if status != 200 {
        let moved = header("location").map_or(String::new(), |_| " (a redirect, which isn't followed: bind the URL it points to)".to_string());
        return Err(format!("{address} answered {status}{moved}"));
    }
    let content_type = header("content-type").unwrap_or_default();
    if !content_type.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/event-stream") {
        let given = if content_type.is_empty() { "no content type".to_string() } else { content_type };
        return Err(format!("{address} answered with {given}, not text/event-stream"));
    }
    reader.get_ref().set_read_timeout(idle);
    let chunked = header("transfer-encoding").is_some_and(|t| t.to_ascii_lowercase().contains("chunked"));
    let body: Box<dyn BufRead + Send> = match (chunked, header("content-length").and_then(|l| l.parse::<u64>().ok())) {
        (true, _) => Box::new(BufReader::new(Chunked { inner: reader, left: 0, crlf: false, done: false })),
        (false, Some(length)) => Box::new(BufReader::new(reader.take(length))),
        (false, None) => Box::new(reader),
    };
    Ok(Connection { lines: Lines::new(body), parser: Parser::at(last_id.unwrap_or("")), socket, idle })
}

/// A feed of server-sent events on this machine, for the tests: what it is told to send it sends
/// to each connection open, and it keeps what it sent, so a connection that opens with
/// `Last-Event-ID` gets what came after that event first.
#[cfg(test)]
pub(crate) mod stand_in {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct Inner {
        /// Each event sent: its id, and the lines that make it.
        sent: Mutex<Vec<(String, String)>>,
        more: Condvar,
        /// The connections open, by number.
        open: Mutex<Vec<(u64, TcpStream)>>,
        opened: std::sync::atomic::AtomicU64,
        /// Each connection's request head, in order, noted once it is answered.
        asked: Mutex<Vec<String>>,
    }

    /// The stand-in: `/events` is the feed (chunked), `/plain` answers text, anything else 404.
    pub(crate) struct StandIn {
        port: u16,
        inner: Arc<Inner>,
    }

    impl StandIn {
        pub(crate) fn start() -> StandIn {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let inner = Arc::new(Inner::default());
            let serving = Arc::clone(&inner);
            std::thread::spawn(move || {
                for conn in listener.incoming().flatten() {
                    let inner = Arc::clone(&serving);
                    std::thread::spawn(move || serve(conn, &inner));
                }
            });
            StandIn { port, inner }
        }

        pub(crate) fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }

        /// Send an event of `data` with the id `id`.
        pub(crate) fn send(&self, id: &str, data: &str) {
            self.send_lines(id, &format!("id: {id}\ndata: {data}\n\n"));
        }

        /// Send `lines` as they are, kept as the event `id`.
        pub(crate) fn send_lines(&self, id: &str, lines: &str) {
            self.inner.sent.lock().unwrap().push((id.to_string(), lines.to_string()));
            self.inner.more.notify_all();
        }

        /// Cut every connection open.
        pub(crate) fn cut(&self) {
            for (_, c) in self.inner.open.lock().unwrap().drain(..) {
                let _ = c.shutdown(Shutdown::Both);
            }
            self.inner.more.notify_all();
        }

        /// Each connection's request head so far.
        pub(crate) fn asked(&self) -> Vec<String> {
            self.inner.asked.lock().unwrap().clone()
        }

        /// How many connections are open.
        pub(crate) fn open(&self) -> usize {
            self.inner.open.lock().unwrap().len()
        }
    }

    fn serve(conn: TcpStream, inner: &Inner) {
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            if line.trim().is_empty() {
                break;
            }
            head.push_str(&line);
        }
        let path = head.split_whitespace().nth(1).unwrap_or("").split('?').next().unwrap_or("").to_string();
        let mut out = conn;
        match path.as_str() {
            "/events" => {}
            "/plain" => {
                let _ = out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nhi");
                inner.asked.lock().unwrap().push(head);
                return;
            }
            _ => {
                let _ = out.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                inner.asked.lock().unwrap().push(head);
                return;
            }
        }
        let last = head.lines().find_map(|l| l.split_once(':').filter(|(n, _)| n.eq_ignore_ascii_case("last-event-id")).map(|(_, v)| v.trim().to_string()));
        // Where it starts is known before it answers: what is sent once it has answered is sent.
        let mut next = {
            let sent = inner.sent.lock().unwrap();
            match last.and_then(|id| sent.iter().position(|(i, _)| *i == id)) {
                Some(at) => at + 1,
                None => sent.len(),
            }
        };
        if out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n").is_err() {
            return;
        }
        let n = inner.opened.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        inner.open.lock().unwrap().push((n, out.try_clone().unwrap()));
        inner.asked.lock().unwrap().push(head);
        let gone = || inner.open.lock().unwrap().retain(|(o, _)| *o != n);
        loop {
            let lines: Vec<String> = {
                let sent = inner.sent.lock().unwrap();
                let sent = if sent.len() > next { sent } else { inner.more.wait_timeout(sent, Duration::from_millis(100)).unwrap().0 };
                sent[next.min(sent.len())..].iter().map(|(_, l)| l.clone()).collect()
            };
            for l in &lines {
                let chunk = format!("{:x}\r\n{l}\r\n", l.len());
                if out.write_all(chunk.as_bytes()).is_err() {
                    return gone();
                }
                next += 1;
            }
            // A cut connection is found out at the next write: a comment, as feeds keep alive.
            if lines.is_empty() && out.write_all(b"3\r\n:\n\n\r\n").is_err() {
                return gone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stand_in::StandIn;
    use super::*;

    fn nexts(text: &str) -> Vec<Next> {
        let mut lines = Lines::new(text.as_bytes());
        let mut parser = Parser::default();
        let mut out = Vec::new();
        while let Some(line) = lines.next_line().unwrap() {
            if let Some(next) = parser.line(&line) {
                out.push(next);
            }
        }
        out
    }

    fn events(text: &str) -> Vec<Event> {
        nexts(text).into_iter().map(|n| if let Next::Event(e) = n { e } else { panic!("passed over") }).collect()
    }

    /// The next thing a connection reads, which must be an event.
    fn next_event(conn: &mut Connection) -> Event {
        match conn.next_event().unwrap() {
            Some(Next::Event(e)) => e,
            other => panic!("not an event: {other:?}"),
        }
    }

    fn event(data: &str, name: &str, id: Option<&str>, last_id: &str) -> Event {
        Event { data: data.into(), name: name.into(), id: id.map(Into::into), last_id: last_id.into() }
    }

    #[test]
    fn data_lines_are_joined_and_a_blank_line_dispatches_them() {
        assert_eq!(events("data: a\ndata:b\ndata\n\n"), [event("a\nb\n", "message", None, "")]);
        // Nothing dispatched without data, nor what a stream ends inside.
        assert_eq!(events("event: x\n\ndata: cut"), []);
        assert_eq!(events(": a comment\nevent: price\ndata: {\"p\":1}\n\n"), [event("{\"p\":1}", "price", None, "")]);
        // One space after the colon is taken off, no more.
        assert_eq!(events("data:  two\n\n"), [event(" two", "message", None, "")]);
    }

    #[test]
    fn an_id_is_where_the_feed_is_until_another_says_otherwise() {
        let got = events("id: 1\ndata: a\n\ndata: b\n\nid\ndata: c\n\nid: 3\0x\ndata: d\n\n");
        assert_eq!(got, [event("a", "message", Some("1"), "1"), event("b", "message", None, "1"), event("c", "message", Some(""), ""), event("d", "message", None, "")]);
        // A reconnect's parser knows where the feed is.
        let mut parser = Parser::at("41");
        assert_eq!(parser.line("data: x"), None);
        assert_eq!(parser.line(""), Some(Next::Event(event("x", "message", None, "41"))));
    }

    #[test]
    fn lines_end_in_crlf_lf_or_cr_and_a_bom_is_dropped() {
        assert_eq!(events("\u{feff}data: a\r\n\r\ndata: b\r\rdata: c\n\n"), [event("a", "message", None, ""), event("b", "message", None, ""), event("c", "message", None, "")]);
    }

    #[test]
    fn retry_is_taken_when_it_is_digits() {
        let mut parser = Parser::default();
        for line in ["retry: 5000", "retry: soon"] {
            parser.line(line);
        }
        assert_eq!(parser.retry(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn an_event_or_a_line_too_long_is_passed_over_and_the_rest_read() {
        let half = "x".repeat(MAX_EVENT / 2);
        let two_halves = format!("id: 1\ndata: {half}\ndata: {half}\nid: 2\n\nid: 3\ndata: after\n\n");
        assert_eq!(nexts(&two_halves), [Next::PassedOver, Next::Event(event("after", "message", Some("3"), "3"))]);
        // A line too long is kept to a byte past the most, the rest dropped as it comes.
        let long = format!("data: {}\r\ndata: z\n\ndata: after\n\n", "y".repeat(2 * MAX_EVENT));
        let mut lines = Lines::new(long.as_bytes());
        assert_eq!(lines.next_line().unwrap().unwrap().len(), MAX_EVENT + 1);
        assert_eq!(nexts(&long), [Next::PassedOver, Next::Event(event("after", "message", None, ""))]);
    }

    #[test]
    fn a_url_is_https_or_http_to_this_machine_and_is_shown_without_its_login_or_query() {
        let url = Url::parse("https://user:pa%40ss@Stream.Example.com/v1/prices?key=s3cret#x").unwrap();
        assert_eq!(url.address(), "https://stream.example.com/v1/prices");
        assert_eq!(format!("{url:?}"), "https://stream.example.com/v1/prices");
        assert_eq!(url.target, "/v1/prices?key=s3cret");
        assert_eq!(Url::parse("http://127.0.0.1:8080?x=1").unwrap().address(), "http://127.0.0.1:8080/");
        assert_eq!(Url::parse("http://[::1]:9/e").unwrap().address(), "http://[::1]:9/e");
        for (bad, why) in [
            ("http://stream.example.com/e", "plain http:// only to this machine"),
            ("ftp://x/e", "https://host/path"),
            ("https://user:s3cret@/e", "no host"),
            ("https://h:port/e", "a port"),
        ] {
            let e = Url::parse(bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
            assert!(!e.contains("s3cret"), "{e}");
        }
    }

    #[test]
    fn a_feed_is_read_as_it_comes_and_reopened_from_where_it_was() {
        let feed = StandIn::start();
        let url = Url::parse(&feed.url("/events")).unwrap();
        let headers = vec![("Authorization".to_string(), Zeroizing::new("Bearer t0ken".to_string()))];
        let mut conn = open(&url, &headers, None, Duration::from_secs(5)).unwrap();
        feed.send("1", "{\"symbol\":\"ACME\",\"price\":10}");
        feed.send_lines("2", "id: 2\nevent: price\ndata: a\ndata: b\n\n");
        assert_eq!(conn.next_event().unwrap(), Some(Next::Event(event("{\"symbol\":\"ACME\",\"price\":10}", "message", Some("1"), "1"))));
        assert_eq!(conn.next_event().unwrap(), Some(Next::Event(event("a\nb", "price", Some("2"), "2"))));
        assert!(feed.asked()[0].contains("Authorization: Bearer t0ken"), "{:?}", feed.asked());
        assert!(!feed.asked()[0].to_ascii_lowercase().contains("last-event-id"));
        // Cut, and opened again from 1: 2 is sent again, then what comes.
        feed.cut();
        assert!(conn.next_event().is_err() || conn.next_event().unwrap().is_none());
        let mut conn = open(&url, &headers, Some("1"), Duration::from_secs(5)).unwrap();
        feed.send("3", "c");
        assert_eq!(next_event(&mut conn).id.as_deref(), Some("2"));
        assert_eq!(next_event(&mut conn).id.as_deref(), Some("3"));
        assert!(feed.asked()[1].contains("Last-Event-ID: 1"), "{:?}", feed.asked());
        // Closed from another thread, at once.
        conn.closer().unwrap().close();
        assert!(conn.next_event().is_err() || conn.next_event().unwrap().is_none());
    }

    #[test]
    fn a_feed_that_says_nothing_for_its_idle_time_is_found_out() {
        let feed = StandIn::start();
        let url = Url::parse(&feed.url("/events")).unwrap();
        // The stand-in keeps alive every 100ms: a comment is something.
        let mut conn = open(&url, &[], None, Duration::from_millis(500)).unwrap();
        feed.send("1", "a");
        assert_eq!(next_event(&mut conn).data, "a");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut c, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = c.read(&mut buf);
            c.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n").unwrap();
            std::thread::sleep(Duration::from_secs(3));
        });
        let quiet = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        let mut conn = open(&quiet, &[], None, Duration::from_millis(300)).unwrap();
        assert!(conn.next_event().unwrap_err().starts_with("nothing came for"));
    }

    #[test]
    fn an_answer_that_isnt_a_feed_is_refused() {
        let feed = StandIn::start();
        let plain = open(&Url::parse(&feed.url("/plain")).unwrap(), &[], None, Duration::from_secs(5)).err().unwrap();
        assert!(plain.ends_with("answered with text/plain, not text/event-stream"), "{plain}");
        let missing = open(&Url::parse(&feed.url("/missing")).unwrap(), &[], None, Duration::from_secs(5)).err().unwrap();
        assert!(missing.ends_with("/missing answered 404"), "{missing}");
        let bad = open(&Url::parse(&feed.url("/events")).unwrap(), &[("Bad Name".into(), Zeroizing::new("x".into()))], None, Duration::from_secs(5)).err().unwrap();
        assert_eq!(bad, "\"Bad Name\" isn't a header's name");
        let split = open(&Url::parse(&feed.url("/events")).unwrap(), &[("X-Key".into(), Zeroizing::new("a\r\nb".into()))], None, Duration::from_secs(5)).err().unwrap();
        assert_eq!(split, "the header X-Key's value holds a line break");
    }
}
