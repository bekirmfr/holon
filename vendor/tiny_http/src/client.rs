use ascii::AsciiString;

use std::io::Error as IoError;
use std::io::Result as IoResult;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use std::net::SocketAddr;
use std::str::FromStr;

use crate::common::{HTTPVersion, Method};
use crate::util::RefinedTcpStream;
use crate::util::{SequentialReader, SequentialReaderBuilder, SequentialWriterBuilder};
use crate::Request;

/// A ClientConnection is an object that will store a socket to a client
/// and return Request objects.
pub struct ClientConnection {
    // address of the client
    remote_addr: IoResult<Option<SocketAddr>>,

    // sequence of Readers to the stream, so that the data is not read in
    //  the wrong order
    source: SequentialReaderBuilder<BufReader<RefinedTcpStream>>,

    // sequence of Writers to the stream, to avoid writing response #2 before
    //  response #1
    sink: SequentialWriterBuilder<BufWriter<RefinedTcpStream>>,

    // Reader to read the next header from
    next_header_source: SequentialReader<BufReader<RefinedTcpStream>>,

    // set to true if we know that the previous request is the last one
    no_more_requests: bool,

    // true if the connection goes through SSL
    secure: bool,

    // Holon (PATCHED.md): how a response that says `Connection: close` ends the connection
    closer: Arc<Closer>,

    // Holon (PATCHED.md): when the request being read is due (`limits::Deadline`)
    deadline: Arc<crate::limits::Deadline>,

    // Holon (PATCHED.md): what the request before left of its body (`limits::Leftover`), and
    // the most of it read off for the connection to go on (`Limits::leftover`)
    leftover: Arc<crate::limits::Leftover>,
    leftover_most: usize,

    // Holon (PATCHED.md): the most of a request's head read: its request line, the head in all,
    // its header lines (`Limits::request_line`, `Limits::head`, `Limits::header_lines`)
    request_line_most: usize,
    head_most: usize,
    lines_most: usize,

    // Holon (PATCHED.md): the last request asked to upgrade: its stream is the application's
    upgraded: bool,
}

/// Holon (PATCHED.md): what a connection's requests tell it. A response sent with `Connection:
/// close` is the connection's last: no request after it is read, and the client is sent the end
/// of the stream once it is written. And whether it is idle: each request read holds a clone of
/// this until it is answered (dropped), so a connection is owed an answer while there is more
/// than one; it is idle once none is owed and the idle limit has passed since the last answer
/// ended (or since it was accepted).
pub(crate) struct Closer {
    stop: AtomicBool,
    sink: Arc<Mutex<BufWriter<RefinedTcpStream>>>,
    idle: Option<Duration>,
    last: Mutex<Instant>,
}

impl Closer {
    pub(crate) fn answered(&self) {
        if let Ok(mut last) = self.last.lock() {
            *last = Instant::now();
        }
    }

    /// Owed no answer, and none sent for the idle limit.
    fn idle(self: &Arc<Self>) -> bool {
        let owed = Arc::strong_count(self) > 1;
        let quiet = match (self.idle, self.last.lock()) {
            (Some(idle), Ok(last)) => last.elapsed() >= idle,
            _ => true,
        };
        !owed && quiet
    }

    pub(crate) fn close(&self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(mut sink) = self.sink.lock() {
            let _ = sink.flush();
            sink.get_mut().shutdown_write();
        }
    }
}

/// Holon (PATCHED.md): a connection that sent nothing of its next request within the idle limit.
#[derive(Debug)]
struct Idle;

impl std::fmt::Display for Idle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("nothing sent within the idle limit")
    }
}

impl std::error::Error for Idle {}

/// Holon (PATCHED.md): a line of a request's head past the most read, not read on.
#[derive(Debug)]
struct TooLong;

impl std::fmt::Display for TooLong {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a line of the request's head past the most read")
    }
}

impl std::error::Error for TooLong {}

/// Error that can happen when reading a request.
#[derive(Debug)]
enum ReadError {
    WrongRequestLine,
    /// Holon (PATCHED.md): refused with this status, and why: a header line that can't be read,
    /// or can be read more than one way (`read`); a body whose length can be, or that has a
    /// `Transfer-Encoding` (`request::body_length`); an `Expect` other than `100-continue`. Upstream's
    /// `WrongHeader` and `ExpectationFailed` are among them.
    Refused(u16, &'static str),
    ReadIoError(IoError),
}

/// Holon (PATCHED.md): a CR, LF or NUL inside a line of a request's head (its lines end at CRLF):
/// a proxy that ends a line at a lone LF, or reads the rest otherwise, finds other headers than
/// this server does (RFC 9110 5.5).
fn unclear(line: &AsciiString) -> bool {
    line.as_bytes().iter().any(|&b| b == b'\r' || b == b'\n' || b == 0)
}

impl ClientConnection {
    /// Creates a new `ClientConnection` that takes ownership of the `TcpStream`.
    ///
    /// Holon (PATCHED.md): held to `limits`: the idle limit each read of it waits, the deadline
    /// each request arrives whole by, the linger of a 408.
    pub fn new(
        write_socket: RefinedTcpStream,
        mut read_socket: RefinedTcpStream,
        limits: &crate::limits::Limits,
    ) -> ClientConnection {
        let remote_addr = read_socket.peer_addr();
        let secure = read_socket.secure();
        let idle = limits.idle;
        let deadline = crate::limits::Deadline::new(limits);
        read_socket.held_to(deadline.clone(), idle);

        let mut source = SequentialReaderBuilder::new(BufReader::with_capacity(1024, read_socket));
        let first_header = source.next().unwrap();
        let sink = SequentialWriterBuilder::new(BufWriter::with_capacity(1024, write_socket));
        let closer = Arc::new(Closer {
            stop: AtomicBool::new(false),
            sink: sink.shared(),
            idle,
            last: Mutex::new(Instant::now()),
        });

        ClientConnection {
            source,
            sink,
            closer,
            remote_addr,
            next_header_source: first_header,
            no_more_requests: false,
            secure,
            deadline,
            leftover: Arc::new(crate::limits::Leftover::default()),
            leftover_most: limits.leftover,
            request_line_most: limits.request_line,
            head_most: limits.head,
            lines_most: limits.header_lines,
            upgraded: false,
        }
    }

    /// true if the connection is HTTPS
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// Reads the next line from self.next_header_source.
    ///
    /// Reads until `CRLF` is reached. The next read will start
    ///  at the first byte of the new line.
    ///
    /// Holon (PATCHED.md): when `first` (a request line) and a read waits past the idle limit
    ///  before any of it comes, the error is `Idle` if the connection is idle (`Closer::idle`);
    ///  while it is owed an answer, or its last ended less than the limit ago, it reads on (so it
    ///  is closed between one and two limits after its last answer). A request line's first byte
    ///  starts the request's deadline (`limits::Deadline`). A line past `most` bytes (its CR
    ///  aside) is not read on: the error is `TooLong`.
    fn read_next_line(&mut self, first: bool, most: usize) -> IoResult<AsciiString> {
        let mut buf = Vec::new();
        let mut prev_byte_was_cr = false;

        loop {
            let byte = self.next_header_source.by_ref().bytes().next();

            let byte = match byte {
                Some(Err(e)) if first && buf.is_empty() && crate::limits::timed_out(&e) => {
                    if self.closer.idle() {
                        return Err(IoError::new(ErrorKind::TimedOut, Idle));
                    }
                    continue;
                }
                Some(b) => b?,
                None => return Err(IoError::new(ErrorKind::ConnectionAborted, "Unexpected EOF")),
            };

            if first && buf.is_empty() {
                self.deadline.start();
            }

            if byte == b'\n' && prev_byte_was_cr {
                buf.pop(); // removing the '\r'
                return AsciiString::from_ascii(buf)
                    .map_err(|_| IoError::new(ErrorKind::InvalidInput, "Header is not in ASCII"));
            }

            prev_byte_was_cr = byte == b'\r';

            buf.push(byte);
            if buf.len() > most + 1 {
                return Err(IoError::new(ErrorKind::InvalidData, TooLong));
            }
        }
    }

    /// Holon (PATCHED.md): a line of a request's head, no longer than `most`; past it, refused
    /// with `status` and `why`. Upstream read a line for as long as its bytes came.
    fn head_line(&mut self, first: bool, most: usize, status: u16, why: &'static str) -> Result<AsciiString, ReadError> {
        self.read_next_line(first, most).map_err(|e| {
            if e.get_ref().is_some_and(|e| e.is::<TooLong>()) {
                ReadError::Refused(status, why)
            } else {
                ReadError::ReadIoError(e)
            }
        })
    }

    /// Holon (PATCHED.md): what is left of the body of the request before (`limits::Leftover`),
    /// read off and thrown away with a buffer of 4 KiB: whether all of it was, so that the
    /// connection can go on. Not past `Limits::leftover` bytes, nor past the request limit from
    /// now (`Deadline`); not at all if its end can't be found.
    fn read_off(&mut self) -> bool {
        let mut left = match self.leftover.take() {
            crate::limits::Left::Nothing => return true,
            crate::limits::Left::Bytes(n) if n <= self.leftover_most => n,
            crate::limits::Left::Bytes(_) | crate::limits::Left::Unknown => return false,
        };
        self.deadline.start();
        let mut buf = [0u8; 4096];
        while left > 0 {
            let most = left.min(buf.len());
            match self.next_header_source.read(&mut buf[..most]) {
                Ok(0) | Err(_) => break,
                Ok(n) => left -= n,
            }
        }
        self.deadline.end();
        left == 0
    }

    /// Holon (PATCHED.md): a request refused before the application sees it, and the
    /// connection's end: `status`, and why in words (as JSON, as the 503 of a connection past a
    /// bound says it), with `Connection: close`, written through `writer` (the next in turn, or
    /// the refused request's own), then `end`, so that its client reads it rather than a reset.
    /// Every refusal ends its connection: where the refused request ends, and so where a next
    /// one would begin, is not known, or not to be trusted. Upstream said 400 and 417 with no
    /// words and closed at once, unread bytes and all.
    fn refuse(&mut self, writer: impl Write, status: u16, why: &str) {
        use crate::{Header, Response, StatusCode};
        let json = Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap();
        let close = Header::from_bytes(&b"Connection"[..], &b"close"[..]).unwrap();
        Response::from_data(format!("{{\"error\":\"{}\"}}", why))
            .with_status_code(StatusCode(status))
            .with_header(json)
            .with_header(close)
            .raw_print(writer, HTTPVersion(1, 1), &[], false, None)
            .ok();
        self.end();
    }

    /// Holon (PATCHED.md): the connection ends. The end of the stream is sent once its last
    /// answer is written (a request lets go of the stream when its body is read, or at once when
    /// it has none, before it is answered), and what its client still sends is read off and
    /// thrown away for the linger (`Limits::linger`, 64 KiB at most), so that the client reads
    /// its answer: a close with bytes unread is a reset, and a reset can reach the client before
    /// the answer (on Windows, it throws away what the client hasn't read yet).
    fn end(&mut self) {
        if let Some(mut after) = self.sink.next() {
            after.wait_turn();
        }
        self.closer.close();
        self.deadline.linger();
        let (mut buf, mut read) = ([0u8; 4096], 0);
        while read < crate::limits::LINGER_READS_MOST {
            match self.next_header_source.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => read += n,
            }
        }
    }

    /// Reads a request from the stream.
    /// Blocks until the header has been read.
    fn read(&mut self) -> Result<Request, ReadError> {
        // Holon (PATCHED.md): none due until its first byte, whatever came before. The end once a
        // request is read whole (below) is what holds the application's reads of its body to the
        // idle limit: this one may come after they have begun (the read of the next request
        // starts as the request is handed on), and doesn't come at all after a connection's last
        self.deadline.end();

        // Holon (PATCHED.md): the head read so far, its lines with their CRLFs (`Limits::head`)
        let mut head = 0;
        let (method, path, version, headers) = {
            // reading the request line
            let (method, path, version) = {
                let line = self.head_line(true, self.request_line_most, 414, "a request line longer than this server reads")?;
                head += line.len() + 2;
                if unclear(&line) {
                    return Err(ReadError::Refused(400, "a CR, LF or NUL inside the request line"));
                }

                parse_request_line(
                    line.as_str().trim(), // TODO: remove this conversion
                )?
            };

            // getting all headers
            let headers = {
                let mut headers = Vec::new();
                loop {
                    // Holon (PATCHED.md): a header line is held by what is left of the head
                    // (`Limits::head`), with no bound of its own: a browser sends every cookie of
                    // a host in one. Upstream read one for as long as its bytes came
                    let left = self.head_most.saturating_sub(head + 2);
                    let line = self.head_line(false, left, 431, "a head larger than this server reads")?;

                    if line.is_empty() {
                        break;
                    };
                    head += line.len() + 2;
                    // Holon (PATCHED.md): no more header lines than `Limits::header_lines`;
                    // upstream took as many as came
                    if headers.len() == self.lines_most {
                        return Err(ReadError::Refused(431, "more header lines than this server reads"));
                    }
                    // Holon (PATCHED.md): a line folded onto the one before (obs-fold), or one
                    // with a CR, LF or NUL inside, is refused (RFC 9112 5.2, RFC 9110 5.5): a
                    // proxy in front that reads it otherwise finds other headers (a
                    // `Content-Length` this server doesn't see, or one only it sees).
                    // Upstream took a folded line, trimmed, as a header of its own.
                    if line.as_bytes().first().map_or(false, |&b| b == b' ' || b == b'\t') {
                        return Err(ReadError::Refused(400, "a header line folded onto the one before"));
                    }
                    if unclear(&line) {
                        return Err(ReadError::Refused(400, "a CR, LF or NUL inside a header line"));
                    }
                    headers.push(match FromStr::from_str(line.as_str().trim()) {
                        // TODO: remove this conversion
                        Ok(h) => h,
                        _ => return Err(ReadError::Refused(400, "a header line that can't be read")),
                    });
                }

                headers
            };

            (method, path, version, headers)
        };

        // building the writer for the request
        let writer = self.sink.next().unwrap();

        // follow-up for next potential request
        let mut data_source = self.source.next().unwrap();
        std::mem::swap(&mut self.next_header_source, &mut data_source);

        // building the next reader
        let request = crate::request::new_request(
            self.secure,
            method,
            path,
            version.clone(),
            headers,
            *self.remote_addr.as_ref().unwrap(),
            data_source,
            writer,
            self.leftover.clone(),
        )
        .map_err(|e| {
            use crate::request;
            match e {
                request::RequestCreationError::CreationIoError(e) => ReadError::ReadIoError(e),
                request::RequestCreationError::ExpectationFailed => {
                    ReadError::Refused(417, "an Expect other than 100-continue")
                }
                request::RequestCreationError::Unclear(status, why) => ReadError::Refused(status, why),
            }
        })?;

        // Holon (PATCHED.md): read whole, by its deadline; what the application reads after is
        // held to the idle limit alone
        self.deadline.end();

        // Holon (PATCHED.md): its response can end the connection
        let mut request = request;
        request.closer = Some(self.closer.clone());

        // return the request
        Ok(request)
    }
}

impl Iterator for ClientConnection {
    type Item = Request;

    /// Blocks until the next Request is available.
    /// Returns None when no new Requests will come from the client.
    fn next(&mut self) -> Option<Request> {
        // Holon (PATCHED.md): an upgraded stream is the application's from its request on
        if self.upgraded {
            return None;
        }
        // Holon (PATCHED.md): once the request before has let go of the stream, what is left of
        // its body is read off (`read_off`), unless a response closed the connection. The
        // connection ends, its client reading its answer first (`end`), when a response closed
        // it, when the rest of a body can't be read off within its bounds, or when the request
        // before was its last: it said `Connection: close`, or is HTTP 1.0 without keep-alive.
        // Upstream ended at once on the last, and read the rest of a body off as the request was
        // dropped (`util::EqualReader`).
        self.next_header_source.wait_turn();
        if self.closer.stop.load(Ordering::Acquire) || !self.read_off() || self.no_more_requests {
            self.end();
            return None;
        }

        loop {
            let rq = match self.read() {
                Err(ReadError::WrongRequestLine) => {
                    let writer = self.sink.next().unwrap();
                    self.refuse(writer, 400, "a request line that can't be read");
                    return None; // we don't know where the next request would start,
                                 // se we have to close
                }

                Err(ReadError::Refused(status, why)) => {
                    let writer = self.sink.next().unwrap();
                    self.refuse(writer, status, why);
                    return None; // we don't know where the next request would start,
                                 // se we have to close
                }

                // Holon (PATCHED.md): nothing of a next request within the idle limit: closed
                // without an answer, as a kept-alive connection a server no longer wants is
                Err(ReadError::ReadIoError(ref err))
                    if err.get_ref().map_or(false, |e| e.is::<Idle>()) =>
                {
                    return None
                }

                Err(ReadError::ReadIoError(ref err)) if crate::limits::timed_out(err) => {
                    // request timeout
                    // Holon (PATCHED.md): said in words, as the 503 of a connection past a bound
                    // is, and read before the close: what the client still sends is read off and
                    // thrown away until it closes, or for the linger (`Limits::linger`). Closed with bytes
                    // unread, the connection is reset, and a reset can reach the client before
                    // the 408 (a client cut off is still sending, as a rule).
                    let why = self.deadline.why();
                    let writer = self.sink.next().unwrap();
                    self.refuse(writer, 408, &why);
                    return None; // closing the connection
                }

                Err(ReadError::ReadIoError(_)) => return None,

                Ok(rq) => rq,
            };

            // checking HTTP version
            // Holon (PATCHED.md): the 505 is written through the request's own writer, and the
            // connection ends. Upstream wrote it through a new writer, which waits for the
            // request's to be dropped, while the request was held right here: the write, and the
            // connection's thread, waited for ever, its place held after its client had gone.
            if *rq.http_version() > (1, 1) {
                self.refuse(rq.into_writer(), 505, "this server only supports HTTP versions 1.0 and 1.1");
                return None;
            }

            // updating the status of the connection
            let connection_header = rq
                .headers()
                .iter()
                .find(|h| h.field.equiv("Connection"))
                .map(|h| h.value.as_str());

            let lowercase = connection_header.map(|h| h.to_ascii_lowercase());

            match lowercase {
                Some(ref val) if val.contains("close") => self.no_more_requests = true,
                Some(ref val) if val.contains("upgrade") => {
                    self.no_more_requests = true;
                    self.upgraded = true;
                }
                Some(ref val)
                    if !val.contains("keep-alive") && *rq.http_version() == HTTPVersion(1, 0) =>
                {
                    self.no_more_requests = true
                }
                None if *rq.http_version() == HTTPVersion(1, 0) => self.no_more_requests = true,
                _ => (),
            };

            // returning the request
            return Some(rq);
        }
    }
}

/// Parses a "HTTP/1.1" string.
fn parse_http_version(version: &str) -> Result<HTTPVersion, ReadError> {
    let (major, minor) = match version {
        "HTTP/0.9" => (0, 9),
        "HTTP/1.0" => (1, 0),
        "HTTP/1.1" => (1, 1),
        "HTTP/2.0" => (2, 0),
        "HTTP/3.0" => (3, 0),
        _ => return Err(ReadError::WrongRequestLine),
    };

    Ok(HTTPVersion(major, minor))
}

/// Parses the request line of the request.
/// eg. GET / HTTP/1.1
fn parse_request_line(line: &str) -> Result<(Method, String, HTTPVersion), ReadError> {
    let mut parts = line.split(' ');

    let method = parts.next().and_then(|w| w.parse().ok());
    let path = parts.next().map(ToOwned::to_owned);
    let version = parts.next().and_then(|w| parse_http_version(w).ok());

    method
        .and_then(|method| Some((method, path?, version?)))
        .ok_or(ReadError::WrongRequestLine)
}

#[cfg(test)]
mod test {
    #[test]
    fn test_parse_request_line() {
        let (method, path, ver) = super::parse_request_line("GET /hello HTTP/1.1").unwrap();

        assert!(method == crate::Method::Get);
        assert!(path == "/hello");
        assert!(ver == crate::common::HTTPVersion(1, 1));

        assert!(super::parse_request_line("GET /hello").is_err());
        assert!(super::parse_request_line("qsd qsd qsd").is_err());
    }
}
