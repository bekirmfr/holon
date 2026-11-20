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

/// Error that can happen when reading a request.
#[derive(Debug)]
enum ReadError {
    WrongRequestLine,
    WrongHeader(HTTPVersion),
    /// the client sent an unrecognized `Expect` header
    ExpectationFailed(HTTPVersion),
    ReadIoError(IoError),
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
    ///  starts the request's deadline (`limits::Deadline`).
    fn read_next_line(&mut self, first: bool) -> IoResult<AsciiString> {
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

        let (method, path, version, headers) = {
            // reading the request line
            let (method, path, version) = {
                let line = self.read_next_line(true).map_err(ReadError::ReadIoError)?;

                parse_request_line(
                    line.as_str().trim(), // TODO: remove this conversion
                )?
            };

            // getting all headers
            let headers = {
                let mut headers = Vec::new();
                loop {
                    let line = self.read_next_line(false).map_err(ReadError::ReadIoError)?;

                    if line.is_empty() {
                        break;
                    };
                    headers.push(match FromStr::from_str(line.as_str().trim()) {
                        // TODO: remove this conversion
                        Ok(h) => h,
                        _ => return Err(ReadError::WrongHeader(version)),
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
        )
        .map_err(|e| {
            use crate::request;
            match e {
                request::RequestCreationError::CreationIoError(e) => ReadError::ReadIoError(e),
                request::RequestCreationError::ExpectationFailed => {
                    ReadError::ExpectationFailed(version)
                }
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
        use crate::{Response, StatusCode};

        // the client sent a "connection: close" header in this previous request
        //  or is using HTTP 1.0, meaning that no new request will come
        if self.no_more_requests || self.closer.stop.load(Ordering::Acquire) {
            return None;
        }

        loop {
            let rq = match self.read() {
                Err(ReadError::WrongRequestLine) => {
                    let writer = self.sink.next().unwrap();
                    let response = Response::new_empty(StatusCode(400));
                    response
                        .raw_print(writer, HTTPVersion(1, 1), &[], false, None)
                        .ok();
                    return None; // we don't know where the next request would start,
                                 // se we have to close
                }

                Err(ReadError::WrongHeader(ver)) => {
                    let writer = self.sink.next().unwrap();
                    let response = Response::new_empty(StatusCode(400));
                    response.raw_print(writer, ver, &[], false, None).ok();
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
                    let body = format!("{{\"error\":\"{}\"}}", self.deadline.why());
                    let json = crate::Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap();
                    let close = crate::Header::from_bytes(&b"Connection"[..], &b"close"[..]).unwrap();
                    let response = Response::from_data(body)
                        .with_status_code(StatusCode(408))
                        .with_header(json)
                        .with_header(close);
                    let writer = self.sink.next().unwrap();
                    response
                        .raw_print(writer, HTTPVersion(1, 1), &[], false, None)
                        .ok();
                    self.closer.close();
                    self.deadline.linger();
                    let (mut buf, mut read) = ([0u8; 4096], 0);
                    while read < crate::limits::LINGER_READS_MOST {
                        match self.next_header_source.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => read += n,
                        }
                    }
                    return None; // closing the connection
                }

                Err(ReadError::ExpectationFailed(ver)) => {
                    let writer = self.sink.next().unwrap();
                    let response = Response::new_empty(StatusCode(417));
                    response.raw_print(writer, ver, &[], true, None).ok();
                    return None; // TODO: should be recoverable, but needs handling in case of body
                }

                Err(ReadError::ReadIoError(_)) => return None,

                Ok(rq) => rq,
            };

            // checking HTTP version
            if *rq.http_version() > (1, 1) {
                let writer = self.sink.next().unwrap();
                let response = Response::from_string(
                    "This server only supports HTTP versions 1.0 and 1.1".to_owned(),
                )
                .with_status_code(StatusCode(505));
                response
                    .raw_print(writer, HTTPVersion(1, 1), &[], false, None)
                    .ok();
                continue;
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
                Some(ref val) if val.contains("upgrade") => self.no_more_requests = true,
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
