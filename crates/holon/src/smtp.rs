//! SMTP (RFC 5321), as the host sends a flow's mail through a binding (mails.rs): written here,
//! over the `rustls` and `webpki-roots` the database connections use, so the host writes every
//! byte it sends (host-capabilities L4 §5.2.2, fork 50).
//!
//! - **Where:** a URL with its login, read from the binding's source as a database's is and never
//!   kept: `smtps://user:password@host` (TLS from the first byte, port 465 unless given) or
//!   `smtp://user:password@host:587` (STARTTLS, required: a server that doesn't offer it is
//!   refused, so a login never crosses in clear). Plain SMTP only to a loopback address: a relay on
//!   the machine, the tests' sink.
//! - **The login:** AUTH PLAIN or LOGIN, whichever the server offers; none when the URL has none.
//! - **What a failure is:** [`Failure::Temporary`] (no connection, a 4xx), tried again;
//!   [`Failure::Permanent`] (a 5xx), given up. Neither says the password, nor the user.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use zeroize::Zeroizing;

/// How long a connection, a command's answer or a write may take.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The longest answer line read: past it, the server isn't one.
const MAX_LINE: usize = 4096;

/// Where a binding sends, as its URL says.
pub(crate) struct Server {
    /// TLS from the first byte (`smtps://`).
    implicit_tls: bool,
    host: String,
    port: u16,
    login: Option<(Zeroizing<String>, Zeroizing<String>)>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.address())
    }
}

impl Server {
    /// The server `url` names, or why it isn't one. The error never holds the URL: it may hold a
    /// password.
    pub(crate) fn parse(url: &str) -> Result<Server, String> {
        let (implicit_tls, rest) = match url.split_once("://") {
            Some(("smtps", rest)) => (true, rest),
            Some(("smtp", rest)) => (false, rest),
            _ => return Err("the URL isn't smtps://user:password@host or smtp://user:password@host:587".into()),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let (login, at) = match authority.rsplit_once('@') {
            Some((user_info, at)) => {
                let (user, password) = user_info.split_once(':').ok_or("the URL's login is user:password")?;
                (Some((unpercent(user)?, unpercent(password)?)), at)
            }
            None => (None, authority),
        };
        let default = if implicit_tls { 465 } else { 587 };
        let number = |p: &str| p.parse::<u16>().map_err(|_| "the URL's port isn't a number".to_string());
        let (host, port) = match at.strip_prefix('[') {
            Some(rest) => match rest.split_once(']') {
                Some((h, "")) => (h, default),
                Some((h, p)) => (h, number(p.strip_prefix(':').ok_or("the URL's host is cut short")?)?),
                None => return Err("the URL's host is cut short".into()),
            },
            None => match at.rsplit_once(':') {
                Some((h, p)) => (h, number(p)?),
                None => (at, default),
            },
        };
        let host = host.to_ascii_lowercase();
        if host.is_empty() {
            return Err("the URL names no host".into());
        }
        let server = Server { implicit_tls, host, port, login };
        Ok(server)
    }

    /// Where it points, without its login: `smtps://smtp.example.com:465`.
    pub(crate) fn address(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        format!("{}://{host}:{}", if self.implicit_tls { "smtps" } else { "smtp" }, self.port)
    }

    fn is_loopback(&self) -> bool {
        self.host == "localhost" || self.host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }

    /// Log in and leave: whether the server takes the login (or, with none, talks SMTP).
    pub(crate) fn check(&self) -> Result<(), Failure> {
        let mut s = self.open()?;
        s.quit();
        Ok(())
    }

    /// Send `message` (MIME, lines ending CR LF) from `from` to each of `to`.
    pub(crate) fn send(&self, from: &str, to: &[String], message: &[u8]) -> Result<(), Failure> {
        let mut s = self.open()?;
        s.expect(&format!("MAIL FROM:<{from}>"), 250, "the sender")?;
        for rcpt in to {
            if let Err(f) = s.expect(&format!("RCPT TO:<{rcpt}>"), 250, &format!("the recipient {rcpt}")) {
                let _ = s.command("RSET");
                s.quit();
                return Err(f);
            }
        }
        s.expect("DATA", 354, "the message")?;
        s.write(&dot_stuffed(message))?;
        s.expect_answer(250, "the message")?;
        s.quit();
        Ok(())
    }

    /// A session, greeted, over TLS unless it is a loopback's plain one, and logged in.
    fn open(&self) -> Result<Session, Failure> {
        let tcp = self.connect()?;
        let local = tcp.local_addr().map(|a| a.ip()).ok();
        let mut s = if self.implicit_tls { Session { stream: Stream::Tls(Box::new(tls(&self.host, tcp)?)), buffer: Vec::new() } } else { Session { stream: Stream::Plain(tcp), buffer: Vec::new() } };
        s.expect_answer(220, "the greeting")?;
        // An address literal: the name this machine goes by isn't known.
        let ehlo = match local {
            Some(std::net::IpAddr::V4(ip)) => format!("EHLO [{ip}]"),
            Some(std::net::IpAddr::V6(ip)) => format!("EHLO [IPv6:{ip}]"),
            None => "EHLO [127.0.0.1]".into(),
        };
        let mut offers = s.expect(&ehlo, 250, "EHLO")?;
        if !self.implicit_tls && !self.is_loopback() {
            if !offers.iter().any(|l| l.eq_ignore_ascii_case("STARTTLS")) {
                return Err(Failure::Permanent(format!("{} offers no STARTTLS, and a login never crosses in clear: use smtps://, or a port that offers it", self.address())));
            }
            s.expect("STARTTLS", 220, "STARTTLS")?;
            // Nothing may come before TLS does: what did was sent in clear, by anyone between.
            if !s.buffer.is_empty() {
                return Err(Failure::Permanent("the server said more than STARTTLS asks for before TLS began".into()));
            }
            let Stream::Plain(tcp) = s.stream else { unreachable!("plain before STARTTLS") };
            s = Session { stream: Stream::Tls(Box::new(tls(&self.host, tcp)?)), buffer: Vec::new() };
            offers = s.expect(&ehlo, 250, "EHLO")?;
        }
        if let Some((user, password)) = &self.login {
            let auth: Vec<String> = offers.iter().filter_map(|l| l.get(..5).filter(|p| p.eq_ignore_ascii_case("AUTH ")).map(|_| l[5..].to_ascii_uppercase())).collect();
            let offered = |m: &str| auth.iter().any(|a| a.split_whitespace().any(|w| w == m));
            let refused = |f: Failure| match f {
                Failure::Permanent(_) => Failure::Permanent(format!("{} refused the login", self.address())),
                t => t,
            };
            if offered("PLAIN") {
                let mut plain = Zeroizing::new(Vec::with_capacity(user.len() + password.len() + 2));
                plain.push(0);
                plain.extend_from_slice(user.as_bytes());
                plain.push(0);
                plain.extend_from_slice(password.as_bytes());
                let line = Zeroizing::new(format!("AUTH PLAIN {}", base64(&plain)));
                s.expect(&line, 235, "the login").map_err(refused)?;
            } else if offered("LOGIN") {
                s.expect("AUTH LOGIN", 334, "the login").map_err(refused)?;
                s.expect(&Zeroizing::new(base64(user.as_bytes())), 334, "the login").map_err(refused)?;
                s.expect(&Zeroizing::new(base64(password.as_bytes())), 235, "the login").map_err(refused)?;
            } else {
                return Err(Failure::Permanent(format!("{} offers no AUTH PLAIN or LOGIN", self.address())));
            }
        }
        Ok(s)
    }

    fn connect(&self) -> Result<TcpStream, Failure> {
        let unreachable = |why: String| Failure::Temporary(format!("{} can't be reached: {why}", self.address()));
        let addrs: Vec<_> = (self.host.as_str(), self.port).to_socket_addrs().map_err(|e| unreachable(e.to_string()))?.collect();
        let mut last = "it has no address".to_string();
        for a in addrs {
            match TcpStream::connect_timeout(&a, TIMEOUT) {
                Ok(tcp) => {
                    let _ = tcp.set_read_timeout(Some(TIMEOUT));
                    let _ = tcp.set_write_timeout(Some(TIMEOUT));
                    return Ok(tcp);
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(unreachable(last))
    }
}

/// Why a mail didn't go.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Failure {
    /// No connection, a 4xx: worth trying again.
    Temporary(String),
    /// A 5xx, or a server that can't be talked to safely: not.
    Permanent(String),
}

impl Failure {
    pub(crate) fn why(&self) -> &str {
        match self {
            Failure::Temporary(w) | Failure::Permanent(w) => w,
        }
    }
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

struct Session {
    stream: Stream,
    /// What was read past the last answer line.
    buffer: Vec<u8>,
}

impl Session {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        let done = match &mut self.stream {
            Stream::Plain(s) => s.write_all(bytes).and_then(|_| s.flush()),
            Stream::Tls(s) => s.write_all(bytes).and_then(|_| s.flush()),
        };
        done.map_err(|e| Failure::Temporary(format!("the connection broke: {e}")))
    }

    fn command(&mut self, line: &str) -> Result<(u16, Vec<String>), Failure> {
        self.write(format!("{line}\r\n").as_bytes())?;
        self.answer()
    }

    /// `line` sent, and its answer `code`; the answer's lines past the first (what EHLO offers).
    fn expect(&mut self, line: &str, code: u16, what: &str) -> Result<Vec<String>, Failure> {
        self.write(Zeroizing::new(format!("{line}\r\n")).as_bytes())?;
        self.expect_answer(code, what)
    }

    fn expect_answer(&mut self, code: u16, what: &str) -> Result<Vec<String>, Failure> {
        let (got, lines) = self.answer()?;
        if got == code {
            return Ok(lines.into_iter().skip(1).collect());
        }
        let said = format!("{what}: {got} {}", lines.join(" ").chars().take(300).collect::<String>());
        Err(if (400..500).contains(&got) { Failure::Temporary(said) } else { Failure::Permanent(said) })
    }

    /// An answer: its code, and the text of each of its lines.
    fn answer(&mut self) -> Result<(u16, Vec<String>), Failure> {
        let mut lines = Vec::new();
        loop {
            let line = self.line()?;
            let bad = || Failure::Permanent(format!("the server answered something that isn't SMTP: {:?}", line.chars().take(80).collect::<String>()));
            let code = line.get(..3).and_then(|c| c.parse::<u16>().ok()).ok_or_else(bad)?;
            let more = line.as_bytes().get(3) == Some(&b'-');
            lines.push(line.get(4..).unwrap_or("").to_string());
            if !more {
                return Ok((code, lines));
            }
            if lines.len() > 100 {
                return Err(bad());
            }
        }
    }

    fn line(&mut self) -> Result<String, Failure> {
        loop {
            if let Some(end) = self.buffer.windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.buffer[..end]).into_owned();
                self.buffer.drain(..end + 2);
                return Ok(line);
            }
            if self.buffer.len() > MAX_LINE {
                return Err(Failure::Permanent("the server's answer line is too long".into()));
            }
            let mut chunk = [0u8; 1024];
            let n = match &mut self.stream {
                Stream::Plain(s) => s.read(&mut chunk),
                Stream::Tls(s) => s.read(&mut chunk),
            }
            .map_err(|e| Failure::Temporary(format!("the connection broke: {e}")))?;
            if n == 0 {
                return Err(Failure::Temporary("the server closed the connection".into()));
            }
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }

    fn quit(&mut self) {
        let _ = self.command("QUIT");
    }
}

/// A client TLS session to `host` over `tcp`, its certificate checked against the web's roots.
fn tls(host: &str, tcp: TcpStream) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>, Failure> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("ring has TLS 1.2 and 1.3")
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    });
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| Failure::Permanent(format!("{host:?} isn't a name TLS can check")))?;
    let conn = rustls::ClientConnection::new(Arc::clone(config), name).map_err(|e| Failure::Permanent(format!("TLS: {e}")))?;
    let mut stream = rustls::StreamOwned::new(conn, tcp);
    // The handshake now, so a certificate that isn't valid says so here.
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock).map_err(|e| Failure::Permanent(format!("TLS with {host}: {e}")))?;
    }
    Ok(stream)
}

/// A message as DATA sends it: a line starting with `.` given another, then the end, `.` alone.
fn dot_stuffed(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + 64);
    let mut start = true;
    for &b in message {
        if start && b == b'.' {
            out.push(b'.');
        }
        out.push(b);
        start = b == b'\n';
    }
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b".\r\n");
    out
}

/// A login's part as its URL writes it, `%XX` decoded (a `+` is a `+`).
fn unpercent(s: &str) -> Result<Zeroizing<String>, String> {
    let b = s.as_bytes();
    let mut out = Zeroizing::new(Vec::with_capacity(b.len()));
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or("the URL's login has a % that isn't %XX")?;
            out.push(hex);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out.to_vec()).map(Zeroizing::new).map_err(|_| "the URL's login isn't UTF-8".into())
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// An SMTP server for the tests, plain on loopback, keeping what it is sent: it takes any login
/// (AUTH PLAIN or LOGIN, as `auth` says to offer), refuses a recipient with `reject` in it (550)
/// and puts off one with `later` in it (451).
#[cfg(test)]
pub(crate) mod sink {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A mail it was sent: its sender, recipients, the login it was given, and the message.
    #[derive(Clone, Debug, Default)]
    pub(crate) struct Got {
        pub(crate) from: String,
        pub(crate) to: Vec<String>,
        pub(crate) login: Option<String>,
        pub(crate) data: String,
    }

    pub(crate) struct Sink {
        pub(crate) port: u16,
        pub(crate) got: Arc<Mutex<Vec<Got>>>,
    }

    pub(crate) fn start(auth: &'static str) -> Sink {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let got = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&got);
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let kept = Arc::clone(&kept);
                std::thread::spawn(move || {
                    let mut w = conn.try_clone().unwrap();
                    let mut r = BufReader::new(conn);
                    let mut say = |s: &str| w.write_all(format!("{s}\r\n").as_bytes());
                    let _ = say("220 sink ready");
                    let mut mail = Got::default();
                    let mut line = String::new();
                    loop {
                        line.clear();
                        if r.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        let l = line.trim_end().to_string();
                        let upper = l.to_ascii_uppercase();
                        if upper.starts_with("EHLO") {
                            let _ = if auth.is_empty() { say("250 sink") } else { say(&format!("250-sink\r\n250 AUTH {auth}")) };
                        } else if upper.starts_with("AUTH PLAIN ") {
                            mail.login = Some(l[11..].to_string());
                            let _ = say("235 ok");
                        } else if upper == "AUTH LOGIN" {
                            let _ = say("334 VXNlcm5hbWU6");
                            let mut user = String::new();
                            r.read_line(&mut user).unwrap();
                            let _ = say("334 UGFzc3dvcmQ6");
                            let mut password = String::new();
                            r.read_line(&mut password).unwrap();
                            mail.login = Some(format!("{}:{}", user.trim_end(), password.trim_end()));
                            let _ = say("235 ok");
                        } else if upper.starts_with("MAIL FROM:") {
                            mail.from = l[10..].trim_matches(['<', '>']).to_string();
                            let _ = say("250 ok");
                        } else if upper.starts_with("RCPT TO:") {
                            let to = l[8..].trim_matches(['<', '>']).to_string();
                            let _ = if to.contains("reject") {
                                say("550 5.1.1 no such user")
                            } else if to.contains("later") {
                                say("451 4.2.1 try again later")
                            } else {
                                mail.to.push(to);
                                say("250 ok")
                            };
                        } else if upper == "DATA" {
                            let _ = say("354 go on");
                            let mut data = String::new();
                            loop {
                                let mut l = String::new();
                                if r.read_line(&mut l).unwrap_or(0) == 0 {
                                    return;
                                }
                                if l == ".\r\n" {
                                    break;
                                }
                                data += l.strip_prefix('.').filter(|_| l.starts_with("..")).unwrap_or(&l);
                            }
                            mail.data = data;
                            kept.lock().unwrap().push(std::mem::take(&mut mail));
                            let _ = say("250 queued");
                        } else if upper == "RSET" {
                            mail = Got { login: mail.login.take(), ..Got::default() };
                            let _ = say("250 ok");
                        } else if upper == "QUIT" {
                            let _ = say("221 bye");
                            return;
                        } else {
                            let _ = say("502 not here");
                        }
                    }
                });
            }
        });
        Sink { port, got }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_says_where_and_its_login_and_never_shows_it() {
        let s = Server::parse("smtps://ops%40acme.com:p%3Ass+w@smtp.example.com").unwrap();
        assert_eq!(s.address(), "smtps://smtp.example.com:465");
        assert_eq!(s.login.as_ref().map(|(u, p)| (u.as_str(), p.as_str())), Some(("ops@acme.com", "p:ss+w")));
        assert_eq!(format!("{s:?}"), "smtps://smtp.example.com:465");
        assert_eq!(Server::parse("smtp://u:p@smtp.example.com:2525").unwrap().address(), "smtp://smtp.example.com:2525");
        assert_eq!(Server::parse("smtp://127.0.0.1:2525").unwrap().address(), "smtp://127.0.0.1:2525");
        assert_eq!(Server::parse("smtp://[::1]:25").unwrap().address(), "smtp://[::1]:25");
        assert_eq!(Server::parse("smtp://smtp.example.com").unwrap().port, 587);
        for bad in ["https://u:p@x.example", "smtp://u@x.example", "smtp://u:p@x.example:port", "smtp://u:p%zz@x.example", "smtps://u:p@"] {
            let e = Server::parse(bad).unwrap_err();
            assert!(!e.contains("u:p"), "{e}");
        }
    }

    #[test]
    fn a_message_is_sent_stuffed_and_logged_in() {
        for auth in ["PLAIN LOGIN", "LOGIN"] {
            let sink = sink::start(auth);
            let s = Server::parse(&format!("smtp://ops:s3cret@127.0.0.1:{}", sink.port)).unwrap();
            s.check().unwrap();
            s.send("treasury@acme.com", &["a@acme.com".into(), "b@acme.com".into()], b"Subject: x\r\n\r\n.hidden\r\nend").unwrap();
            let got = sink.got.lock().unwrap().clone();
            assert_eq!(got.len(), 1);
            assert_eq!((got[0].from.as_str(), got[0].to.clone()), ("treasury@acme.com", vec!["a@acme.com".to_string(), "b@acme.com".to_string()]));
            assert_eq!(got[0].data, "Subject: x\r\n\r\n.hidden\r\nend\r\n");
            let login = got[0].login.clone().unwrap();
            assert_eq!(login, if auth == "LOGIN" { format!("{}:{}", base64(b"ops"), base64(b"s3cret")) } else { base64(b"\0ops\0s3cret") });
        }
    }

    #[test]
    fn a_refused_recipient_is_permanent_a_deferred_one_temporary() {
        let sink = sink::start("");
        let s = Server::parse(&format!("smtp://127.0.0.1:{}", sink.port)).unwrap();
        assert!(matches!(s.send("t@acme.com", &["a@acme.com".into(), "reject@acme.com".into()], b"x"), Err(Failure::Permanent(w)) if w.contains("550") && w.contains("reject@acme.com")));
        assert!(matches!(s.send("t@acme.com", &["later@acme.com".into()], b"x"), Err(Failure::Temporary(w)) if w.contains("451")));
        assert!(sink.got.lock().unwrap().is_empty());
        // A login the server offers no way to give.
        let with = Server::parse(&format!("smtp://u:p@127.0.0.1:{}", sink.port)).unwrap();
        assert!(matches!(with.check(), Err(Failure::Permanent(w)) if w.contains("no AUTH")));
        // Nobody there.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(matches!(Server::parse(&format!("smtp://127.0.0.1:{closed}")).unwrap().check(), Err(Failure::Temporary(_))));
    }

    #[test]
    fn only_a_loopback_address_takes_plain_smtp() {
        for (url, loopback) in [("smtp://127.0.0.1:25", true), ("smtp://localhost:25", true), ("smtp://[::1]:25", true), ("smtp://smtp.example.com:587", false), ("smtp://10.0.0.5:25", false)] {
            assert_eq!(Server::parse(url).unwrap().is_loopback(), loopback, "{url}");
        }
    }
}
