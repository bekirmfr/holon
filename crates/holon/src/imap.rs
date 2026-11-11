//! IMAP (RFC 9051, and RFC 3501's IMAP4rev1, which most servers still speak), as the host reads a
//! mailbox a binding names (mails.rs, inbox.rs): written here over the `rustls` and `webpki-roots`
//! SMTP uses, so the host writes every byte it sends (host-capabilities L4 §5.2.4).
//!
//! - **Where:** a URL with its login, read from the binding's source and never kept:
//!   `imaps://user:password@host` (TLS from the first byte, port 993 unless given) or
//!   `imap://user:password@host:143` (STARTTLS, required: a server that doesn't offer it is
//!   refused, so a login never crosses in clear). Plain IMAP only to a loopback address: the tests'
//!   stand-in.
//! - **The login:** `AUTHENTICATE PLAIN` when the server offers it, else `LOGIN`. A URL that names
//!   a user and no password logs in as that user with the token the binding's `--oauth` gives
//!   (oauth.rs, §5.2.6): `AUTHENTICATE XOAUTH2`.
//! - **What it does:** selects a folder (its `UIDVALIDITY`), lists its messages' UIDs
//!   (`UID SEARCH ALL`), fetches one's size and bytes (`BODY.PEEK[]`: nothing is marked read),
//!   moves one (`UID MOVE`; without `MOVE`, `UID COPY`, `\Deleted` and, with `UIDPLUS`,
//!   `UID EXPUNGE`), makes a folder, logs out.
//! - **What a failure is:** [`Failure::Temporary`] (no connection, the connection broke),
//!   [`Failure::Permanent`] (a `NO` or `BAD`, a server that can't be talked to safely). Neither says
//!   the password, nor the user.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use zeroize::Zeroizing;

pub(crate) use crate::smtp::Failure;
use crate::smtp::Login;

/// How long a connection, a command's answer or a write may take.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The longest response line read, literals apart: a folder's UIDs come on one line.
const MAX_LINE: usize = 16 * 1024 * 1024;
/// The largest literal read: a message is fetched only once its size says it fits.
const MAX_LITERAL: usize = 64 * 1024 * 1024;

/// Where a binding reads, as its URL says.
pub(crate) struct Server {
    /// TLS from the first byte (`imaps://`).
    implicit_tls: bool,
    host: String,
    port: u16,
    login: Login,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.address())
    }
}

impl Server {
    /// The server `url` names, with its login, or why it isn't one. The error never holds the URL:
    /// it may hold a password.
    pub(crate) fn parse(url: &str) -> Result<Server, String> {
        let (implicit_tls, rest) = match url.split_once("://") {
            Some(("imaps", rest)) => (true, rest),
            Some(("imap", rest)) => (false, rest),
            _ => return Err("the URL isn't imaps://user:password@host or imap://user:password@host:143".into()),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let (user_info, at) = authority.rsplit_once('@').ok_or("the URL has no login: imaps://user:password@host")?;
        let login = Login::of(user_info)?;
        let default = if implicit_tls { 993 } else { 143 };
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
        Ok(Server { implicit_tls, host, port, login })
    }

    /// Where it points, without its login: `imaps://imap.example.com:993`.
    pub(crate) fn address(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        format!("{}://{host}:{}", if self.implicit_tls { "imaps" } else { "imap" }, self.port)
    }

    fn is_loopback(&self) -> bool {
        self.host == "localhost" || self.host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }

    /// Whether it logs in with a token: its URL names a user and no password.
    pub(crate) fn wants_token(&self) -> bool {
        matches!(self.login, Login::Token(..))
    }

    /// It, logging in with `token` when it logs in with one.
    pub(crate) fn with_token(mut self, token: Zeroizing<String>) -> Server {
        self.login.with_token(token);
        self
    }

    /// A session, greeted, over TLS unless it is a loopback's plain one, and logged in.
    pub(crate) fn open(&self) -> Result<Session, Failure> {
        let tcp = self.connect()?;
        let stream = if self.implicit_tls { Stream::Tls(Box::new(crate::smtp::tls(&self.host, tcp)?)) } else { Stream::Plain(tcp) };
        let mut s = Session { stream, buffer: Vec::new(), tag: 0, capabilities: Vec::new() };
        let greeting = s.response()?;
        if greeting.text.starts_with("* BYE") {
            return Err(Failure::Temporary(format!("{} said goodbye: {}", self.address(), greeting.text.chars().take(200).collect::<String>())));
        }
        if !greeting.text.starts_with("* OK") && !greeting.text.starts_with("* PREAUTH") {
            return Err(Failure::Permanent(format!("{} answered something that isn't IMAP", self.address())));
        }
        s.capabilities = s.capability()?;
        if !self.implicit_tls && !self.is_loopback() {
            if !s.offers("STARTTLS") {
                return Err(Failure::Permanent(format!("{} offers no STARTTLS, and a login never crosses in clear: use imaps://, or a port that offers it", self.address())));
            }
            s.command("STARTTLS")?;
            // Nothing may come before TLS does: what did was sent in clear, by anyone between.
            if !s.buffer.is_empty() {
                return Err(Failure::Permanent("the server said more than STARTTLS asks for before TLS began".into()));
            }
            let Stream::Plain(tcp) = s.stream else { unreachable!("plain before STARTTLS") };
            s.stream = Stream::Tls(Box::new(crate::smtp::tls(&self.host, tcp)?));
            s.capabilities = s.capability()?;
        }
        if !greeting.text.starts_with("* PREAUTH") {
            let refused = |f: Failure| match f {
                Failure::Permanent(_) => Failure::Permanent(format!("{} refused the login", self.address())),
                t => t,
            };
            let (user, password) = match &self.login {
                Login::Password(user, password) => (user, password),
                Login::Token(user, token) => {
                    let token = token.as_ref().ok_or_else(|| Failure::Permanent(crate::smtp::NO_TOKEN.into()))?;
                    if !s.offers("AUTH=XOAUTH2") {
                        return Err(Failure::Permanent(format!("{} offers no AUTH=XOAUTH2, which a token logs in with", self.address())));
                    }
                    s.xoauth2(&crate::smtp::xoauth2(user, token)).map_err(refused)?;
                    s.capabilities = s.capability()?;
                    return Ok(s);
                }
            };
            if s.offers("AUTH=PLAIN") {
                let mut plain = Zeroizing::new(Vec::with_capacity(user.len() + password.len() + 2));
                plain.push(0);
                plain.extend_from_slice(user.as_bytes());
                plain.push(0);
                plain.extend_from_slice(password.as_bytes());
                let encoded = Zeroizing::new(crate::smtp::base64(&plain));
                if s.offers("SASL-IR") {
                    s.command(&Zeroizing::new(format!("AUTHENTICATE PLAIN {}", encoded.as_str()))).map_err(refused)?;
                } else {
                    s.authenticate("AUTHENTICATE PLAIN", &encoded).map_err(refused)?;
                }
            } else if s.offers("LOGINDISABLED") {
                return Err(Failure::Permanent(format!("{} takes no login IMAP can give it here (no AUTH=PLAIN)", self.address())));
            } else {
                let line = Zeroizing::new(format!("LOGIN {} {}", quoted(user)?.as_str(), quoted(password)?.as_str()));
                s.command(&line).map_err(refused)?;
            }
            // What it offers may change once logged in.
            s.capabilities = s.capability()?;
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

/// Whether `name` is a folder this IMAP writes as it is: printable ASCII, none of the characters
/// IMAP's own UTF-7 or its lists give a meaning to.
pub(crate) fn is_folder_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && name.bytes().all(|b| (0x20..0x7f).contains(&b) && !b"\"\\&%*".contains(&b))
}

/// A string as IMAP quotes it, or why it can't be (a line break, or what isn't ASCII, which only a
/// literal carries).
fn quoted(s: &str) -> Result<Zeroizing<String>, Failure> {
    if !s.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return Err(Failure::Permanent("the login holds what IMAP's LOGIN can't carry, and the server offers no AUTH=PLAIN".into()));
    }
    Ok(Zeroizing::new(format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))))
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

/// One response line: its text, each literal in it written `{L0}`, `{L1}`…, and the literals.
#[derive(Debug, Default)]
struct Response {
    text: String,
    literals: Vec<Vec<u8>>,
}

/// A logged-in session.
pub(crate) struct Session {
    stream: Stream,
    /// What was read past the last line.
    buffer: Vec<u8>,
    tag: u32,
    capabilities: Vec<String>,
}

impl Session {
    fn offers(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c.eq_ignore_ascii_case(capability))
    }

    /// Select `folder`: its `UIDVALIDITY`, which changes when the server numbers it again.
    pub(crate) fn select(&mut self, folder: &str) -> Result<u32, Failure> {
        let untagged = self.command(&format!("SELECT \"{folder}\""))?;
        let validity = untagged.iter().find_map(|r| {
            let at = r.text.find("[UIDVALIDITY ")?;
            r.text[at + 13..].split(']').next()?.trim().parse::<u32>().ok()
        });
        validity.ok_or_else(|| Failure::Permanent(format!("{folder} gave no UIDVALIDITY")))
    }

    /// The UIDs of the selected folder's messages, in order.
    pub(crate) fn uids(&mut self) -> Result<Vec<u32>, Failure> {
        let untagged = self.command("UID SEARCH ALL")?;
        let mut uids: Vec<u32> = untagged
            .iter()
            .filter_map(|r| r.text.strip_prefix("* SEARCH"))
            .flat_map(|rest| rest.split_whitespace().filter_map(|n| n.parse::<u32>().ok()).collect::<Vec<_>>())
            .collect();
        uids.sort_unstable();
        uids.dedup();
        Ok(uids)
    }

    /// The size of message `uid`, none when it is gone.
    pub(crate) fn size(&mut self, uid: u32) -> Result<Option<u64>, Failure> {
        let untagged = self.command(&format!("UID FETCH {uid} (RFC822.SIZE)"))?;
        Ok(untagged.iter().filter(|r| r.text.contains(" FETCH ")).find_map(|r| {
            let at = r.text.find("RFC822.SIZE ")?;
            r.text[at + 12..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
        }))
    }

    /// The bytes of message `uid`, none when it is gone. Nothing marks it read.
    pub(crate) fn fetch(&mut self, uid: u32) -> Result<Option<Vec<u8>>, Failure> {
        let untagged = self.command(&format!("UID FETCH {uid} (BODY.PEEK[])"))?;
        Ok(untagged.into_iter().filter(|r| r.text.contains(" FETCH ")).find_map(|mut r| {
            let at = r.text.find("BODY[]")?;
            let index: usize = r.text[at..].strip_prefix("BODY[] {L")?.split('}').next()?.parse().ok()?;
            (index < r.literals.len()).then(|| std::mem::take(&mut r.literals[index]))
        }))
    }

    /// Move message `uid` to `folder`: `UID MOVE`, else a copy, marked `\Deleted` and, with
    /// `UIDPLUS`, expunged. Whether it left the folder.
    pub(crate) fn move_to(&mut self, uid: u32, folder: &str) -> Result<bool, Failure> {
        if self.offers("MOVE") {
            self.command(&format!("UID MOVE {uid} \"{folder}\""))?;
            return Ok(true);
        }
        self.command(&format!("UID COPY {uid} \"{folder}\""))?;
        self.command(&format!("UID STORE {uid} +FLAGS.SILENT (\\Deleted)"))?;
        if self.offers("UIDPLUS") {
            self.command(&format!("UID EXPUNGE {uid}"))?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Make `folder` if it isn't there.
    pub(crate) fn create(&mut self, folder: &str) -> Result<(), Failure> {
        if self.command(&format!("CREATE \"{folder}\"")).is_ok() {
            return Ok(());
        }
        let listed = self.command(&format!("LIST \"\" \"{folder}\""))?;
        if listed.iter().any(|r| r.text.starts_with("* LIST")) {
            return Ok(());
        }
        Err(Failure::Permanent(format!("the folder {folder} isn't there and couldn't be made")))
    }

    pub(crate) fn logout(mut self) {
        let _ = self.command("LOGOUT");
    }

    fn capability(&mut self) -> Result<Vec<String>, Failure> {
        let untagged = self.command("CAPABILITY")?;
        Ok(untagged.iter().filter_map(|r| r.text.strip_prefix("* CAPABILITY ")).flat_map(|c| c.split_whitespace().map(str::to_string).collect::<Vec<_>>()).collect())
    }

    /// `line` sent under a new tag: its untagged responses, once it is answered `OK`.
    fn command(&mut self, line: &str) -> Result<Vec<Response>, Failure> {
        self.tag += 1;
        let tag = format!("a{}", self.tag);
        self.write(Zeroizing::new(format!("{tag} {line}\r\n")).as_bytes())?;
        self.answered(&tag, line)
    }

    /// `AUTHENTICATE` with its one answer sent when the server asks.
    fn authenticate(&mut self, line: &str, answer: &str) -> Result<Vec<Response>, Failure> {
        self.tag += 1;
        let tag = format!("a{}", self.tag);
        self.write(format!("{tag} {line}\r\n").as_bytes())?;
        let r = self.response()?;
        if !r.text.starts_with('+') {
            return Err(Failure::Permanent("the login wasn't asked for".into()));
        }
        self.write(Zeroizing::new(format!("{answer}\r\n")).as_bytes())?;
        self.answered(&tag, line)
    }

    /// `AUTHENTICATE XOAUTH2` with `response`: with the command when the server offers `SASL-IR`,
    /// else when it asks. A challenge after it is a refusal's status in JSON (Google's way),
    /// answered empty as the mechanism says, before the `NO`.
    fn xoauth2(&mut self, response: &str) -> Result<(), Failure> {
        let line = "AUTHENTICATE XOAUTH2";
        let tag = if self.offers("SASL-IR") {
            self.tag += 1;
            let tag = format!("a{}", self.tag);
            self.write(Zeroizing::new(format!("{tag} {line} {response}\r\n")).as_bytes())?;
            tag
        } else {
            self.tag += 1;
            let tag = format!("a{}", self.tag);
            self.write(format!("{tag} {line}\r\n").as_bytes())?;
            if !self.response()?.text.starts_with('+') {
                return Err(Failure::Permanent("the login wasn't asked for".into()));
            }
            self.write(Zeroizing::new(format!("{response}\r\n")).as_bytes())?;
            tag
        };
        loop {
            let r = self.response()?;
            if r.text.starts_with('+') {
                self.write(b"\r\n")?;
                continue;
            }
            let Some(status) = r.text.strip_prefix(tag.as_str()).and_then(|s| s.strip_prefix(' ')) else { continue };
            if status.get(..2).is_some_and(|s| s.eq_ignore_ascii_case("OK")) {
                return Ok(());
            }
            return Err(Failure::Permanent(format!("AUTHENTICATE: {}", status.chars().take(300).collect::<String>())));
        }
    }

    /// The untagged responses up to `tag`'s, which must be `OK`.
    fn answered(&mut self, tag: &str, line: &str) -> Result<Vec<Response>, Failure> {
        let mut untagged = Vec::new();
        loop {
            let r = self.response()?;
            let Some(status) = r.text.strip_prefix(tag).and_then(|s| s.strip_prefix(' ')) else {
                if r.text.starts_with("* BYE") && !line.starts_with("LOGOUT") {
                    return Err(Failure::Temporary(format!("the server said goodbye: {}", r.text.chars().take(200).collect::<String>())));
                }
                untagged.push(r);
                continue;
            };
            if status.get(..2).is_some_and(|s| s.eq_ignore_ascii_case("OK")) {
                return Ok(untagged);
            }
            // The command's first word only: a login's line holds what it sent.
            let verb = line.split_whitespace().take(if line.starts_with("UID ") { 2 } else { 1 }).collect::<Vec<_>>().join(" ");
            return Err(Failure::Permanent(format!("{verb}: {}", status.chars().take(300).collect::<String>())));
        }
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), Failure> {
        let done = match &mut self.stream {
            Stream::Plain(s) => s.write_all(bytes).and_then(|_| s.flush()),
            Stream::Tls(s) => s.write_all(bytes).and_then(|_| s.flush()),
        };
        done.map_err(|e| Failure::Temporary(format!("the connection broke: {e}")))
    }

    /// One response line, the literals in it read whole.
    fn response(&mut self) -> Result<Response, Failure> {
        let mut r = Response::default();
        loop {
            let line = self.line()?;
            // A literal: `{n}` (or `{n+}`) ending the line, then n bytes, then the line goes on.
            let literal = line.strip_suffix('}').and_then(|l| l.rfind('{').map(|at| (at, &l[at + 1..]))).and_then(|(at, n)| n.trim_end_matches('+').parse::<usize>().ok().map(|n| (at, n)));
            let Some((at, n)) = literal else {
                r.text += &line;
                return Ok(r);
            };
            if n > MAX_LITERAL {
                return Err(Failure::Permanent("the server sent more than a message may be".into()));
            }
            r.text += &line[..at];
            r.text += &format!("{{L{}}}", r.literals.len());
            r.literals.push(self.bytes(n)?);
        }
    }

    fn line(&mut self) -> Result<String, Failure> {
        let mut searched = 0;
        loop {
            if let Some(end) = self.buffer[searched..].windows(2).position(|w| w == b"\r\n").map(|p| p + searched) {
                let line = String::from_utf8_lossy(&self.buffer[..end]).into_owned();
                self.buffer.drain(..end + 2);
                return Ok(line);
            }
            searched = self.buffer.len().saturating_sub(1);
            if self.buffer.len() > MAX_LINE {
                return Err(Failure::Permanent("the server's response line is too long".into()));
            }
            self.fill()?;
        }
    }

    fn bytes(&mut self, n: usize) -> Result<Vec<u8>, Failure> {
        while self.buffer.len() < n {
            self.fill()?;
        }
        Ok(self.buffer.drain(..n).collect())
    }

    fn fill(&mut self) -> Result<(), Failure> {
        let mut chunk = [0u8; 16 * 1024];
        let n = match &mut self.stream {
            Stream::Plain(s) => s.read(&mut chunk),
            Stream::Tls(s) => s.read(&mut chunk),
        }
        .map_err(|e| Failure::Temporary(format!("the connection broke: {e}")))?;
        if n == 0 {
            return Err(Failure::Temporary("the server closed the connection".into()));
        }
        self.buffer.extend_from_slice(&chunk[..n]);
        Ok(())
    }
}

/// An IMAP server for the tests, plain on loopback, its folders in memory: it takes the login it
/// was made with (`AUTHENTICATE PLAIN` and `LOGIN`, or a token's `AUTHENTICATE XOAUTH2`), and
/// speaks what [`Session`] says, `MOVE` among it unless made without.
#[cfg(test)]
pub(crate) mod stand_in {
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use crate::sync::Mutex;

    /// Its folders: each message by UID, and the next UID.
    #[derive(Default)]
    pub(crate) struct Folders {
        pub(crate) folders: BTreeMap<String, (BTreeMap<u32, Vec<u8>>, u32)>,
        pub(crate) validity: u32,
        /// Times it was logged in to.
        pub(crate) logins: u32,
    }

    impl Folders {
        /// Put `message` in `folder`: its UID.
        pub(crate) fn put(&mut self, folder: &str, message: &[u8]) -> u32 {
            let (messages, next) = self.folders.entry(folder.to_string()).or_insert((BTreeMap::new(), 1));
            let uid = *next;
            *next += 1;
            messages.insert(uid, message.to_vec());
            uid
        }

        /// The messages in `folder`.
        pub(crate) fn of(&self, folder: &str) -> Vec<Vec<u8>> {
            self.folders.get(folder).map(|(m, _)| m.values().cloned().collect()).unwrap_or_default()
        }
    }

    pub(crate) struct StandIn {
        pub(crate) port: u16,
        pub(crate) folders: Arc<Mutex<Folders>>,
    }

    /// One that takes `user` and `password`, offering `MOVE` when `moves`.
    pub(crate) fn start(user: &str, password: &str, moves: bool) -> StandIn {
        started(user, password, false, moves)
    }

    /// One that takes `user` with `token` (`AUTHENTICATE XOAUTH2`, offered with `SASL-IR`), and
    /// refuses another token as Google does: a challenge, then `NO`.
    pub(crate) fn start_with_token(user: &str, token: &str, moves: bool) -> StandIn {
        started(user, token, true, moves)
    }

    fn started(user: &str, secret: &str, token: bool, moves: bool) -> StandIn {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let folders = Arc::new(Mutex::new(Folders { validity: 7, ..Default::default() }));
        folders.lock().unwrap().folders.insert("INBOX".into(), (BTreeMap::new(), 1));
        let (shared, user, secret) = (Arc::clone(&folders), user.to_string(), secret.to_string());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let (shared, user, secret) = (Arc::clone(&shared), user.clone(), secret.clone());
                std::thread::spawn(move || serve(conn, &shared, &user, &secret, token, moves));
            }
        });
        StandIn { port, folders }
    }

    fn serve(conn: std::net::TcpStream, folders: &Mutex<Folders>, user: &str, secret: &str, token: bool, moves: bool) {
        let mut out = conn.try_clone().unwrap();
        let mut reader = BufReader::new(conn);
        if out.write_all(b"* OK stand-in ready\r\n").is_err() {
            return;
        }
        let mut selected: Option<String> = None;
        let password = if token { "" } else { secret };
        let plain = crate::smtp::base64(format!("\0{user}\0{password}").as_bytes());
        let bearer = crate::smtp::xoauth2(user, secret);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let line = line.trim_end().to_string();
            let (tag, rest) = line.split_once(' ').unwrap_or((&line, ""));
            let words: Vec<&str> = rest.split_whitespace().collect();
            let verb = words.first().map(|w| w.to_ascii_uppercase()).unwrap_or_default();
            let unquote = |s: &str| s.trim_matches('"').to_string();
            let ok = format!("{tag} OK done\r\n");
            let no = format!("{tag} NO not that\r\n");
            if verb == "AUTHENTICATE" && token {
                let xoauth2 = words.get(1).is_some_and(|m| m.eq_ignore_ascii_case("XOAUTH2"));
                let taken = xoauth2 && words.get(2) == Some(&bearer.as_str());
                if xoauth2 && !taken {
                    let mut empty = String::new();
                    if out.write_all(b"+ eyJzdGF0dXMiOiI0MDAifQ==\r\n").is_err() || reader.read_line(&mut empty).unwrap_or(0) == 0 {
                        return;
                    }
                }
                folders.lock().unwrap().logins += u32::from(taken);
                if out.write_all(if taken { ok.as_bytes() } else { no.as_bytes() }).is_err() {
                    return;
                }
                continue;
            }
            if verb == "AUTHENTICATE" {
                let given = match words.get(2) {
                    Some(initial) => initial.to_string(),
                    None => {
                        let mut answer = String::new();
                        if out.write_all(b"+ \r\n").is_err() || reader.read_line(&mut answer).unwrap_or(0) == 0 {
                            return;
                        }
                        answer.trim().to_string()
                    }
                };
                let taken = given == plain;
                folders.lock().unwrap().logins += u32::from(taken);
                if out.write_all(if taken { ok.as_bytes() } else { no.as_bytes() }).is_err() {
                    return;
                }
                continue;
            }
            let mut f = folders.lock().unwrap();
            let reply: Vec<u8> = match verb.as_str() {
                "CAPABILITY" => format!("* CAPABILITY IMAP4rev1 AUTH=PLAIN{}{}\r\n{ok}", if token { " AUTH=XOAUTH2 SASL-IR" } else { "" }, if moves { " MOVE UIDPLUS" } else { " UIDPLUS" }).into_bytes(),
                "LOGIN" if !token && words.get(1).map(|u| unquote(u)).as_deref() == Some(user) && words.get(2).map(|p| unquote(p)).as_deref() == Some(password) => {
                    f.logins += 1;
                    ok.into_bytes()
                }
                "SELECT" => {
                    let name = unquote(words.get(1).unwrap_or(&""));
                    match f.folders.get(&name).map(|(m, _)| m.len()) {
                        Some(count) => {
                            selected = Some(name);
                            format!("* {count} EXISTS\r\n* OK [UIDVALIDITY {}] UIDs valid\r\n{tag} OK [READ-WRITE] selected\r\n", f.validity).into_bytes()
                        }
                        None => no.into_bytes(),
                    }
                }
                "CREATE" => {
                    let name = unquote(words.get(1).unwrap_or(&""));
                    match f.folders.entry(name) {
                        std::collections::btree_map::Entry::Occupied(_) => format!("{tag} NO [ALREADYEXISTS] it is there\r\n").into_bytes(),
                        std::collections::btree_map::Entry::Vacant(e) => {
                            e.insert((BTreeMap::new(), 1));
                            ok.into_bytes()
                        }
                    }
                }
                "LIST" => {
                    let name = unquote(words.get(2).unwrap_or(&""));
                    if f.folders.contains_key(&name) { format!("* LIST () \"/\" \"{name}\"\r\n{ok}").into_bytes() } else { ok.into_bytes() }
                }
                "UID" => {
                    let sub = words.get(1).map(|w| w.to_ascii_uppercase()).unwrap_or_default();
                    let folder = selected.clone().unwrap_or_default();
                    let uid: u32 = words.get(2).and_then(|u| u.parse().ok()).unwrap_or(0);
                    let message = f.folders.get(&folder).and_then(|(m, _)| m.get(&uid)).cloned();
                    match (sub.as_str(), message) {
                        ("SEARCH", _) => {
                            let uids: Vec<String> = f.folders.get(&folder).map(|(m, _)| m.keys().map(|u| u.to_string()).collect()).unwrap_or_default();
                            format!("* SEARCH {}\r\n{ok}", uids.join(" ")).into_bytes()
                        }
                        ("FETCH", None) => ok.into_bytes(),
                        ("FETCH", Some(m)) if rest.contains("RFC822.SIZE") => format!("* 1 FETCH (UID {uid} RFC822.SIZE {})\r\n{ok}", m.len()).into_bytes(),
                        ("FETCH", Some(m)) => {
                            let mut reply = format!("* 1 FETCH (UID {uid} BODY[] {{{}}}\r\n", m.len()).into_bytes();
                            reply.extend_from_slice(&m);
                            reply.extend_from_slice(format!(" FLAGS ())\r\n{ok}").as_bytes());
                            reply
                        }
                        ("MOVE" | "COPY", Some(m)) if f.folders.contains_key(&unquote(words.get(3).unwrap_or(&""))) => {
                            f.put(&unquote(words.get(3).unwrap_or(&"")), &m);
                            if sub == "MOVE" {
                                f.folders.get_mut(&folder).map(|(m, _)| m.remove(&uid));
                            }
                            ok.into_bytes()
                        }
                        ("MOVE" | "COPY", _) => no.into_bytes(),
                        ("STORE", _) => ok.into_bytes(),
                        ("EXPUNGE", _) => {
                            f.folders.get_mut(&folder).map(|(m, _)| m.remove(&uid));
                            ok.into_bytes()
                        }
                        _ => format!("{tag} BAD what\r\n").into_bytes(),
                    }
                }
                "LOGOUT" => {
                    let _ = out.write_all(format!("* BYE bye\r\n{ok}").as_bytes());
                    return;
                }
                _ => format!("{tag} BAD what\r\n").into_bytes(),
            };
            drop(f);
            if out.write_all(&reply).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_says_where_and_its_login_and_never_shows_it() {
        let s = Server::parse("imaps://remit%40acme.com:p%3Ass@imap.example.com").unwrap();
        assert_eq!(s.address(), "imaps://imap.example.com:993");
        assert!(matches!(&s.login, Login::Password(u, p) if u.as_str() == "remit@acme.com" && p.as_str() == "p:ss"));
        let token = Server::parse("imaps://remit%40acme.com@outlook.office365.com").unwrap();
        assert!(token.wants_token() && !s.wants_token());
        assert_eq!(format!("{s:?}"), "imaps://imap.example.com:993");
        assert_eq!(Server::parse("imap://u:p@127.0.0.1").unwrap().address(), "imap://127.0.0.1:143");
        for bad in ["imaps://imap.example.com", "smtp://u:p@x.example", "imap://u:p@x.example:port", "imaps://u:p@"] {
            let e = Server::parse(bad).unwrap_err();
            assert!(!e.contains("u:p"), "{e}");
        }
        assert!(is_folder_name("INBOX") && is_folder_name("Processed/2026"));
        assert!(!is_folder_name("") && !is_folder_name("Réglé") && !is_folder_name("a\"b") && !is_folder_name("a&b"));
    }

    /// A folder read and a message moved, with `MOVE` and without, logged in either way; a wrong
    /// login refused without saying it.
    #[test]
    fn a_folder_is_read_and_a_message_moved() {
        for moves in [true, false] {
            let imap = stand_in::start("remit", "s3cret", moves);
            let first = imap.folders.lock().unwrap().put("INBOX", b"Subject: one\r\n\r\nhi\r\n");
            imap.folders.lock().unwrap().put("INBOX", b"Subject: two\r\n\r\n{5}\r\n");
            let server = Server::parse(&format!("imap://remit:s3cret@127.0.0.1:{}", imap.port)).unwrap();
            let mut s = server.open().unwrap();
            s.create("Processed").unwrap();
            s.create("Processed").unwrap();
            assert_eq!(s.select("INBOX").unwrap(), 7);
            let uids = s.uids().unwrap();
            assert_eq!(uids.len(), 2);
            assert_eq!(s.size(first).unwrap(), Some(20));
            assert_eq!(s.fetch(first).unwrap().unwrap(), b"Subject: one\r\n\r\nhi\r\n");
            assert_eq!(s.fetch(uids[1]).unwrap().unwrap(), b"Subject: two\r\n\r\n{5}\r\n", "a message that looks like a literal");
            assert_eq!(s.fetch(99).unwrap(), None);
            assert!(s.move_to(first, "Processed").unwrap());
            assert_eq!(s.uids().unwrap(), [uids[1]]);
            s.logout();
            assert_eq!(imap.folders.lock().unwrap().of("Processed").len(), 1);
            let wrong = Server::parse(&format!("imap://remit:nope@127.0.0.1:{}", imap.port)).unwrap();
            assert!(matches!(wrong.open(), Err(Failure::Permanent(w)) if w.contains("refused the login") && !w.contains("nope")));
        }
    }

    /// A user without a password logs in with the binding's token (`AUTHENTICATE XOAUTH2`); one
    /// refused with a challenge is answered and refused without saying the token; none given, or a
    /// server that takes none, is refused.
    #[test]
    fn a_token_logs_in_with_xoauth2() {
        let imap = stand_in::start_with_token("remit@acme.com", "t0ken", true);
        let url = format!("imap://remit%40acme.com@127.0.0.1:{}", imap.port);
        let mut s = Server::parse(&url).unwrap().with_token(Zeroizing::new("t0ken".into())).open().unwrap();
        assert_eq!(s.select("INBOX").unwrap(), 7);
        s.logout();
        assert_eq!(imap.folders.lock().unwrap().logins, 1);
        let expired = Server::parse(&url).unwrap().with_token(Zeroizing::new("expired".into()));
        assert!(matches!(expired.open(), Err(Failure::Permanent(w)) if w.contains("refused the login") && !w.contains("expired")));
        assert!(matches!(Server::parse(&url).unwrap().open(), Err(Failure::Permanent(w)) if w == crate::smtp::NO_TOKEN));
        let passwords = stand_in::start("remit@acme.com", "s3cret", true);
        let without = Server::parse(&format!("imap://remit%40acme.com@127.0.0.1:{}", passwords.port)).unwrap().with_token(Zeroizing::new("t0ken".into()));
        assert!(matches!(without.open(), Err(Failure::Permanent(w)) if w.contains("offers no AUTH=XOAUTH2")));
    }

    #[test]
    fn only_a_loopback_address_takes_plain_imap() {
        for (url, loopback) in [("imap://u:p@127.0.0.1", true), ("imap://u:p@localhost", true), ("imap://u:p@imap.example.com", false)] {
            assert_eq!(Server::parse(url).unwrap().is_loopback(), loopback, "{url}");
        }
    }
}
