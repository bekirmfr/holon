//! Outbound HTTP: the `Performer` that sends a run's `http_request`s (§4.2 of
//! docs/PLATFORM_LAYOUT.md). Every rule is enforced here, at perform time, whatever the flow
//! says:
//!
//! - only hosts the deployment was granted (its `HttpRequest(host)` capabilities), checked again
//!   on every redirect;
//! - no private, loopback, link-local or metadata addresses, checked on the addresses DNS
//!   actually returned and the connection actually uses (so DNS rebinding can't get around it);
//! - HTTPS only, with certificate checks;
//! - the request's limits: timeouts, request, response and header sizes, redirects.
//!
//! A request that breaks a rule or a limit, or fails, gets a response `{status: 0, error, message}`
//! rather than failing the run: the flow decides what to do with it.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

use holon_core::Value;
use holon_host::{Limits, Performer};

/// The `Performer` for one deployment's runs.
#[derive(Debug)]
pub(crate) struct Outbound {
    /// The `host[:port]`s it may call (lowercase).
    pub(crate) allowed: BTreeSet<String>,
    /// Hosts the host revoked since the deploy: a call to one fails as `revoked`, not
    /// `blocked_host`, so the log says why.
    pub(crate) revoked: BTreeSet<String>,
    /// Hosts it may call over plain HTTP: an exception the host grants (§4.2.1), never a default.
    pub(crate) plain_http: BTreeSet<String>,
    /// Whether private addresses are allowed: an exception too.
    pub(crate) allow_private: bool,
}

fn failure(error: &str, message: impl Into<String>) -> Value {
    Value::Record(BTreeMap::from([
        ("status".to_string(), Value::Number(0.0)),
        ("error".to_string(), Value::Text(error.to_string())),
        ("message".to_string(), Value::Text(message.into())),
    ]))
}

/// A URL split into what the rules look at.
struct Target {
    plain: bool,
    authority: String,
    rest: String,
}

/// `scheme://authority/rest`, for `http` and `https` only. The authority is lowercased; anything
/// with credentials (`user@`) is refused.
fn parse_url(url: &str) -> Option<Target> {
    let (plain, after) = if let Some(a) = url.strip_prefix("https://") {
        (false, a)
    } else if let Some(a) = url.strip_prefix("http://") {
        (true, a)
    } else {
        return None;
    };
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = after[..end].to_ascii_lowercase();
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let rest = if end < after.len() { after[end..].to_string() } else { "/".to_string() };
    Some(Target { plain, authority, rest })
}

impl Outbound {
    /// Whether `target` may be called: a granted host, over HTTPS unless granted plain HTTP.
    fn check(&self, target: &Target) -> Result<(), Value> {
        if self.revoked.contains(&target.authority) {
            return Err(failure("revoked", format!("the host revoked {} for this deployment", target.authority)));
        }
        if !self.allowed.contains(&target.authority) {
            return Err(failure("blocked_host", format!("{} wasn't granted to this deployment", target.authority)));
        }
        if target.plain && !self.plain_http.contains(&target.authority) {
            return Err(failure("blocked_scheme", format!("plain HTTP to {} wasn't granted", target.authority)));
        }
        Ok(())
    }

    fn url(&self, target: &Target) -> String {
        let scheme = if self.plain_http.contains(&target.authority) { "http" } else { "https" };
        format!("{scheme}://{}{}", target.authority, target.rest)
    }
}

impl Performer for Outbound {
    fn perform(&self, request: &Value, limits: &Limits) -> Value {
        let Value::Record(r) = request else { return failure("bad_request", "not a request record") };
        let (Some(Value::Text(method)), Some(Value::Text(url)), Some(Value::Record(headers)), Some(Value::Text(body))) =
            (r.get("method"), r.get("url"), r.get("headers"), r.get("body"))
        else {
            return failure("bad_request", "a request needs method, url, headers and body");
        };
        if body.len() > limits.max_request_bytes {
            return failure("too_large", format!("the request body is over {} bytes", limits.max_request_bytes));
        }
        let Some(mut target) = parse_url(url) else { return failure("bad_url", url.clone()) };

        let allow_private = self.allow_private;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(limits.connect_timeout)
            .timeout(limits.request_timeout)
            .redirects(0) // followed below, each one checked
            .resolver(move |netloc: &str| -> std::io::Result<Vec<SocketAddr>> {
                let addrs: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
                let public: Vec<SocketAddr> = addrs.iter().copied().filter(|a| allow_private || !is_private(a.ip())).collect();
                if public.is_empty() && !addrs.is_empty() {
                    return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "a private address"));
                }
                Ok(public)
            })
            .build();

        let mut method = method.clone();
        let mut body = body.clone();
        let mut redirects = 0;
        loop {
            if let Err(refused) = self.check(&target) {
                return refused;
            }
            let mut call = agent.request(&method, &self.url(&target));
            for (name, value) in headers {
                if let Value::Text(v) = value {
                    call = call.set(name, v);
                }
            }
            let sent = if body.is_empty() && method != "POST" && method != "PUT" && method != "PATCH" {
                call.call()
            } else {
                call.send_bytes(body.as_bytes())
            };
            let response = match sent {
                Ok(response) | Err(ureq::Error::Status(_, response)) => response,
                Err(ureq::Error::Transport(t)) => return transport_failure(&t),
            };
            let status = response.status();
            if (300..400).contains(&status)
                && status != 304
                && let Some(location) = response.header("location")
            {
                if redirects >= limits.max_redirects {
                    return failure("too_many_redirects", format!("more than {} redirects", limits.max_redirects));
                }
                redirects += 1;
                target = match next_target(&target, location) {
                    Some(t) => t,
                    None => return failure("bad_url", format!("redirect to {location}")),
                };
                // 303, and 301/302 after a POST, become a GET without a body, as browsers do.
                if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                    method = "GET".into();
                    body.clear();
                }
                continue;
            }
            return read_response(response, limits);
        }
    }
}

/// Where a redirect's `Location` points: absolute, or a path on the same host.
fn next_target(from: &Target, location: &str) -> Option<Target> {
    if location.starts_with('/') && !location.starts_with("//") {
        Some(Target { plain: from.plain, authority: from.authority.clone(), rest: location.to_string() })
    } else {
        parse_url(location)
    }
}

/// The response record, within the header and body limits.
fn read_response(response: ureq::Response, limits: &Limits) -> Value {
    let status = response.status();
    let mut headers: BTreeMap<String, Value> = BTreeMap::new();
    let mut header_bytes = 0;
    for name in response.headers_names() {
        let values: Vec<&str> = response.all(&name);
        header_bytes += name.len() + values.iter().map(|v| v.len()).sum::<usize>();
        headers.insert(name.to_ascii_lowercase(), Value::Text(values.join(", ")));
    }
    if header_bytes > limits.max_header_bytes {
        return failure("too_large", format!("the response headers are over {} bytes", limits.max_header_bytes));
    }
    let mut bytes = Vec::new();
    if let Err(e) = response.into_reader().take(limits.max_response_bytes as u64 + 1).read_to_end(&mut bytes) {
        return failure(if e.kind() == std::io::ErrorKind::TimedOut { "timeout" } else { "transport" }, e.to_string());
    }
    if bytes.len() > limits.max_response_bytes {
        return failure("too_large", format!("the response body is over {} bytes", limits.max_response_bytes));
    }
    Value::Record(BTreeMap::from([
        ("status".to_string(), Value::Number(status as f64)),
        ("headers".to_string(), Value::Record(headers)),
        ("body".to_string(), Value::Text(String::from_utf8_lossy(&bytes).into_owned())),
    ]))
}

fn transport_failure(t: &ureq::Transport) -> Value {
    let text = t.to_string();
    let source_says = |needle: &str| {
        let mut source: Option<&dyn std::error::Error> = std::error::Error::source(t);
        while let Some(s) = source {
            if s.to_string().contains(needle) {
                return true;
            }
            source = s.source();
        }
        text.contains(needle)
    };
    // A timeout is an I/O error of kind TimedOut (or WouldBlock, for a socket read timeout on
    // some platforms) somewhere in the chain; its text varies by OS.
    let timed_out = {
        let mut source: Option<&dyn std::error::Error> = std::error::Error::source(t);
        let mut found = false;
        while let Some(s) = source {
            if let Some(io) = s.downcast_ref::<std::io::Error>() {
                found |= matches!(io.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock);
            }
            source = s.source();
        }
        found
    };
    let error = if source_says("a private address") {
        "blocked_address"
    } else if timed_out || source_says("timed out") {
        "timeout"
    } else {
        match t.kind() {
            ureq::ErrorKind::Dns => "dns",
            ureq::ErrorKind::ConnectionFailed => "connect",
            ureq::ErrorKind::TooManyRedirects => "too_many_redirects",
            ureq::ErrorKind::InvalidUrl | ureq::ErrorKind::UnknownScheme => "bad_url",
            _ => "transport",
        }
    };
    failure(error, text)
}

/// Whether `ip` is somewhere a flow must not reach by default: this machine, its network, or a
/// cloud metadata service (OWASP's SSRF list).
pub(crate) fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local() // 169.254/16, cloud metadata included
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || a == 0
                || (a == 100 && (64..128).contains(&b)) // 100.64/10, carrier-grade NAT
                || (a == 192 && b == 0 && v4.octets()[2] == 0) // 192.0.0/24, protocol assignments
                || (a == 198 && (b == 18 || b == 19)) // benchmarking
                || a >= 240 // reserved
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00 // fc00::/7, unique local
                || (first & 0xffc0) == 0xfe80 // fe80::/10, link-local
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::time::Duration;

    /// A tiny HTTP server on 127.0.0.1 that answers each connection with `respond(request line)`.
    fn serve(respond: impl Fn(&str) -> String + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                // Read the whole request (head, then Content-Length bytes) before answering:
                // closing on unread data makes the OS reset the connection.
                let mut got = Vec::new();
                let mut buf = [0u8; 8192];
                let head_end = loop {
                    if let Some(i) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break None,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                };
                let Some(head_end) = head_end else { continue };
                let head = String::from_utf8_lossy(&got[..head_end]).to_string();
                let length: usize = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                    .unwrap_or(0);
                while got.len() < head_end + length {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let line = head.lines().next().unwrap_or("").to_string();
                let _ = stream.write_all(respond(&line).as_bytes());
                let _ = stream.flush();
            }
        });
        addr
    }

    fn ok(body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Seen: yes\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    fn request(method: &str, url: &str, body: &str) -> Value {
        Value::Record(BTreeMap::from([
            ("method".to_string(), Value::Text(method.into())),
            ("url".to_string(), Value::Text(url.into())),
            ("headers".to_string(), Value::Record(BTreeMap::from([("x-flow".to_string(), Value::Text("1".into()))]))),
            ("body".to_string(), Value::Text(body.into())),
        ]))
    }

    fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
        let Value::Record(r) = v else { panic!("{v:?}") };
        &r[k]
    }

    /// Allowed to call `addr` over plain HTTP, private addresses included: what a test server
    /// needs, and what the host would have to grant explicitly.
    fn local(addr: &str) -> Outbound {
        let hosts: BTreeSet<String> = [addr.to_string()].into();
        Outbound { allowed: hosts.clone(), revoked: BTreeSet::new(), plain_http: hosts, allow_private: true }
    }

    #[test]
    fn a_granted_host_answers_with_status_headers_and_body() {
        let addr = serve(|line| ok(&format!("you sent {line}")));
        let out = local(&addr).perform(&request("POST", &format!("https://{addr}/items?a=1"), "{}"), &Limits::request_run());
        assert_eq!(field(&out, "status"), &Value::Number(200.0), "{out:?}");
        assert_eq!(field(&out, "body"), &Value::Text("you sent POST /items?a=1 HTTP/1.1".into()));
        assert_eq!(field(field(&out, "headers"), "x-seen"), &Value::Text("yes".into()), "header names lowercased");
    }

    #[test]
    fn every_rule_is_enforced_at_perform_time() {
        let addr = serve(|_| ok("secret"));
        let limits = Limits::request_run();
        let error = |o: &Outbound, url: &str| field(&o.perform(&request("GET", url, ""), &limits), "error").clone();
        let text = |s: &str| Value::Text(s.into());

        // A host that wasn't granted.
        assert_eq!(error(&local(&addr), "https://evil.example.com/"), text("blocked_host"));
        // Granted, but plain HTTP wasn't.
        let https_only = Outbound { plain_http: BTreeSet::new(), ..local(&addr) };
        assert_eq!(error(&https_only, &format!("http://{addr}/")), text("blocked_scheme"));
        // Granted and plain HTTP allowed, but 127.0.0.1 is a private address.
        let public_only = Outbound { allow_private: false, ..local(&addr) };
        assert_eq!(error(&public_only, &format!("https://{addr}/")), text("blocked_address"));
        // Credentials in a URL, or another scheme, are not URLs it sends.
        assert_eq!(error(&local(&addr), &format!("https://u:p@{addr}/")), text("bad_url"));
        assert_eq!(error(&local(&addr), "ftp://a.com/"), text("bad_url"));
    }

    #[test]
    fn limits_turn_into_error_responses() {
        let big = serve(|_| ok(&"x".repeat(5000)));
        let small = Limits { max_response_bytes: 1000, ..Limits::request_run() };
        let out = local(&big).perform(&request("GET", &format!("https://{big}/"), ""), &small);
        assert_eq!(field(&out, "error"), &Value::Text("too_large".into()));

        let tight = Limits { max_request_bytes: 3, ..Limits::request_run() };
        let out = local(&big).perform(&request("POST", &format!("https://{big}/"), "four"), &tight);
        assert_eq!(field(&out, "error"), &Value::Text("too_large".into()));

        let slow = serve(|_| {
            std::thread::sleep(Duration::from_millis(400));
            ok("late")
        });
        let quick = Limits { request_timeout: Duration::from_millis(100), ..Limits::request_run() };
        let out = local(&slow).perform(&request("GET", &format!("https://{slow}/"), ""), &quick);
        assert_eq!(field(&out, "error"), &Value::Text("timeout".into()), "{out:?}");
    }

    #[test]
    fn redirects_are_followed_and_each_one_checked() {
        let target = serve(|line| ok(&format!("landed {line}")));
        let to_target = target.clone();
        let hop = serve(move |line| {
            if line.contains("/away") {
                "HTTP/1.1 302 Found\r\nLocation: https://evil.example.com/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
            } else {
                format!("HTTP/1.1 303 See Other\r\nLocation: http://{to_target}/done\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            }
        });
        let both: BTreeSet<String> = [hop.clone(), target.clone()].into();
        let o = Outbound { allowed: both.clone(), revoked: BTreeSet::new(), plain_http: both, allow_private: true };
        let out = o.perform(&request("POST", &format!("https://{hop}/start"), "{}"), &Limits::request_run());
        assert_eq!(field(&out, "body"), &Value::Text("landed GET /done HTTP/1.1".into()), "303 becomes a GET");
        let out = o.perform(&request("GET", &format!("https://{hop}/away"), ""), &Limits::request_run());
        assert_eq!(field(&out, "error"), &Value::Text("blocked_host".into()), "a redirect off the granted hosts");
        let none = Limits { max_redirects: 0, ..Limits::request_run() };
        let out = o.perform(&request("GET", &format!("https://{hop}/start"), ""), &none);
        assert_eq!(field(&out, "error"), &Value::Text("too_many_redirects".into()));
    }

    #[test]
    fn private_addresses_are_recognised() {
        for ip in ["127.0.0.1", "10.1.2.3", "172.16.0.1", "192.168.1.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fc00::1", "fe80::1", "::ffff:127.0.0.1"] {
            assert!(is_private(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "172.32.0.1", "100.128.0.1", "2606:4700::1111"] {
            assert!(!is_private(ip.parse().unwrap()), "{ip}");
        }
    }
}
