//! The data plane (M1): requests to a deployed server unit, on that server's own port.
//!
//! This module knows HTTP only as plain strings and bytes, so it can be tested without a socket
//! and moved to another HTTP stack later (docs/PLATFORM_LAYOUT.md §7b) without changing it.
//!
//! **The request record** a handler receives:
//! `{method, path, params, query, headers, body}`. `params` holds the route's `:name` segments
//! and `query` the query string, both as text and percent-decoded. `headers` has lower-cased
//! names. `body` is the raw body as text; a handler that expects JSON runs it through
//! `json_parse`.
//!
//! **The response**, read from the endpoint's output 0:
//! - a record with a numeric `status` is an envelope: `{status, headers?, body?}`;
//! - any other value is the body, sent with status 200;
//! - an effect (e.g. a lone `store_write`) is performed and answered 204;
//! - a subscription (`return updates.subscribe()`, a `channel_subscribe` as the body) is
//!   answered 200 with the channel's packs as server-sent events, streamed by the transport
//!   (`subscribe`; channels.rs);
//! - a file (`return site.serve(name)`, a `file_serve` as the body) is answered with its bytes,
//!   its type and its hash as its `etag`, once the host has checked it is still the handle's
//!   (`file`; listeners.rs's `serve_file`);
//! - nothing at all (the handler didn't settle) is a 500.
//!
//! A text body goes out as `text/plain`, anything else as JSON, unless the envelope sets
//! `content-type` itself.

use std::collections::BTreeMap;
use std::sync::Arc;

use holon_core::{match_route, stringify_json, Capability, Resolver, Server, Value};

/// What to send back, in stack-neutral form.
#[derive(Debug, PartialEq)]
pub struct DataResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The channel the caller subscribes to, when the answer is a subscription: the transport
    /// streams its packs after the headers.
    pub subscribe: Option<Subscribe>,
    /// The file the answer is, when it is one: its bytes are the body once the host has read them
    /// (a small one's), or the transport streams them from the file it opened (a big one's).
    pub file: Option<Served>,
}

/// A file a route answered with: what its handle said of it, and its type. Read again as it is
/// sent, and sent only if it is still the handle's.
#[derive(Clone, Debug)]
pub struct Served {
    pub folder: String,
    pub name: String,
    pub size: u64,
    /// Its BLAKE3, hex: its `etag`, and what its bytes are checked against as they are sent.
    pub hash: String,
    /// When it was last changed, in ms since the epoch, as its handle said.
    pub modified: u64,
    pub content_type: String,
    /// The file, opened and checked by the host, for the transport to stream: a big one's.
    pub opened: Option<Arc<std::fs::File>>,
}

impl PartialEq for Served {
    fn eq(&self, other: &Served) -> bool {
        (&self.folder, &self.name, self.size, &self.hash, self.modified, &self.content_type) == (&other.folder, &other.name, other.size, &other.hash, other.modified, &other.content_type)
    }
}

impl Served {
    /// How the run log and the control plane say it: `file site/app.js (148.2 KiB, blake3 7c1e2a9b…)`.
    pub(crate) fn line(&self) -> String {
        format!("file {}/{} ({}, blake3 {}…)", self.folder, self.name, crate::folders::size_text(self.size), &self.hash[..self.hash.len().min(8)])
    }
}

/// A subscription a route answered with.
#[derive(Clone, Debug, PartialEq)]
pub struct Subscribe {
    pub channel: String,
    /// The packs the channel keeps first (`from: "kept"`), not only those sent from now on.
    pub kept: bool,
}

/// Response headers a flow's answer doesn't set: how the reply is framed, which the host says.
const FRAMING: &[&str] = &["content-length", "transfer-encoding", "connection", "keep-alive", "te", "trailer", "upgrade"];

/// The file `v` answers with, if it is one (`file_serve`'s effect).
fn served(v: &Value) -> Option<Served> {
    let Value::Effect { sink, payload } = v else { return None };
    let folder = sink.strip_prefix("file_serve:")?;
    let Value::Record(f) = &**payload else { return None };
    let text = |k: &str| match f.get(k) {
        Some(Value::Text(t)) => Some(t.clone()),
        _ => None,
    };
    let count = |k: &str| match f.get(k) {
        Some(Value::Number(n)) if *n >= 0.0 && n.fract() == 0.0 => Some(*n as u64),
        _ => None,
    };
    if text("folder")? != folder {
        return None;
    }
    Some(Served { folder: folder.to_string(), name: text("name")?, size: count("size")?, hash: text("hash")?, modified: count("modified")?, content_type: text("type")?, opened: None })
}

/// A file's answer: the flow's headers, its type and `cache-control` unless the flow set them,
/// and its hash as its `etag`, the host's.
fn file_answer(status: u16, mut headers: Vec<(String, String)>, file: Served) -> DataResponse {
    headers.retain(|(k, _)| k != "etag");
    if !headers.iter().any(|(k, _)| k == "content-type") {
        headers.push(("content-type".into(), file.content_type.clone()));
    }
    headers.push(("etag".into(), format!("\"{}\"", file.hash)));
    if !headers.iter().any(|(k, _)| k == "cache-control") {
        headers.push(("cache-control".into(), "no-cache".into()));
    }
    DataResponse { status, headers, body: vec![], subscribe: None, file: Some(file) }
}

/// The subscription `v` asks for, if it is one (`channel_subscribe`'s effect).
fn subscription(v: &Value) -> Option<Subscribe> {
    let Value::Effect { sink, payload } = v else { return None };
    let channel = sink.strip_prefix("channel_subscribe:")?;
    let kept = matches!(&**payload, Value::Record(f) if f.get("from") == Some(&Value::Text("kept".into())));
    Some(Subscribe { channel: channel.to_string(), kept })
}

impl DataResponse {
    pub(crate) fn json_error(status: u16, message: &str) -> DataResponse {
        let body = Value::Record([("error".to_string(), Value::Text(message.to_string()))].into());
        DataResponse {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: stringify_json(&body).unwrap_or_default().into_bytes(),
            subscribe: None,
            file: None,
        }
    }
}

/// One HTTP request as a transport hands it over, owned: what `serve_data` answers. `url` is the
/// path plus any query string; header names as sent.
#[derive(Clone, Debug, PartialEq)]
pub struct DataRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Who sent it over the control plane (`holon mcp`'s `call`, the editor's request tester):
    /// the caller's name, recorded on its run. None on the deployment's own port, where nobody
    /// signs in.
    pub by: Option<String>,
}

/// One HTTP request, as plain strings and bytes. `url` is the path plus any query string.
pub struct HttpRequest<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
}

/// Answer one request to `server`. `fire` settles the matched endpoint with the request record
/// as its input (and performs its effects); it returns the endpoint's output slots. It is also
/// told the matched route (`"GET /todos/:id"`), for the flow log.
pub fn handle(
    server: &Server,
    resolver: &impl Resolver,
    request: &HttpRequest,
    fire: impl FnOnce(&holon_core::Unit, &holon_core::Route, Value, &[(Capability, Value)]) -> Vec<Option<Value>>,
    sourced: &[(Capability, Value)],
) -> DataResponse {
    let HttpRequest { method, url, headers, body } = *request;
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let Some((route, raw_params)) = match_route(server, method, path) else {
        let other_methods: Vec<&str> = holon_core::METHODS
            .iter()
            .copied()
            .filter(|m| *m != method && match_route(server, m, path).is_some())
            .collect();
        if other_methods.is_empty() {
            return DataResponse::json_error(404, "no route for this path");
        }
        let mut r = DataResponse::json_error(405, "method not allowed for this path");
        r.headers.push(("allow".into(), other_methods.join(", ")));
        return r;
    };
    let Ok(body_text) = String::from_utf8(body.to_vec()) else {
        return DataResponse::json_error(400, "request body is not UTF-8 text");
    };
    let Some(endpoint) = resolver.resolve(&route.endpoint) else {
        return DataResponse::json_error(500, "endpoint does not resolve");
    };
    let params = raw_params.into_iter().map(|(k, v)| (k, percent_decode(&v))).collect();
    let request = request_record(method, path, params, query, headers, &body_text);
    let mut sourced = sourced.to_vec();
    sourced.push((Capability::NetworkIngress, Value::Text(body_text)));
    let outputs = fire(endpoint, route, request, &sourced);
    respond(outputs.first().and_then(Option::as_ref))
}

/// The record a handler receives. See the module doc for its fields.
pub fn request_record(
    method: &str,
    path: &str,
    params: BTreeMap<String, String>,
    query: &str,
    headers: &[(String, String)],
    body: &str,
) -> Value {
    let text_record = |pairs: BTreeMap<String, String>| {
        Value::Record(pairs.into_iter().map(|(k, v)| (k, Value::Text(v))).collect())
    };
    let query = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect();
    let headers = headers.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.clone())).collect();
    Value::Record(BTreeMap::from([
        ("method".to_string(), Value::Text(method.to_string())),
        ("path".to_string(), Value::Text(path.to_string())),
        ("params".to_string(), text_record(params)),
        ("query".to_string(), text_record(query)),
        ("headers".to_string(), text_record(headers)),
        ("body".to_string(), Value::Text(body.to_string())),
    ]))
}

/// The values of the tainted keys wherever a run's entry or its lines hold them, each with its key:
/// what the host keeps out of everything it records of the run (`Redactor::tainted`), its log
/// entry, its lines, its packs, its export, `holon verify` and replay, which read each as a
/// placeholder. The keys are those its flow taints (`declared`) and those every run does
/// ([`holon_core::ALWAYS_TAINTED`]: credentials), named either case. A key's value is looked for
/// wherever the entry holds what its run had ([`LogEntry::values_mut`]), whatever started it: a
/// record's field (a request's header, query or path parameter, a message's, an input's, an
/// answer's, a row's), a header's `[name, value]` pair, a JSON text at any depth (a body, a call's
/// answer), and a query or a form's `name=value` (a URL, a form body); all of it when it holds
/// more than one value. What a credential holds is tainted with it: what follows its scheme
/// (`Bearer …`), what a Basic one decodes to and its password alone, each cookie's value, a set
/// cookie's. A value shorter than a secret may be can't be told apart from ordinary text, so it
/// isn't kept out; nor is one `marked` already marks (a secret, a token), which stays marked as
/// that.
pub(crate) fn tainted(declared: &[String], entry: &mut crate::recorder::LogEntry, lines: &[(String, Value)], marked: &crate::secrets::Redactor) -> Vec<(String, String)> {
    let keys: Vec<&str> = holon_core::ALWAYS_TAINTED.iter().copied().chain(declared.iter().map(String::as_str)).collect();
    let mut found: Vec<(String, String)> = Vec::new();
    entry.values_mut(&mut |v| svalue_tainted(v, &keys, &mut found));
    for s in entry.texts_mut() {
        text_tainted(s, &keys, &mut found);
    }
    for (_, line) in lines {
        svalue_tainted(&crate::wire::SValue::from_core(line), &keys, &mut found);
    }
    let mut held = Vec::new();
    for (key, value) in &found {
        if let Some((scheme, rest)) = value.trim().split_once(' ')
            && !scheme.is_empty()
            && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            held.push((key.clone(), rest.trim().to_string()));
            if scheme.eq_ignore_ascii_case("basic")
                && let Some(decoded) = base64_text(rest.trim())
            {
                held.extend(decoded.split_once(':').map(|(_, password)| (key.clone(), password.to_string())));
                held.push((key.clone(), decoded));
            }
        }
        // Each cookie a request sends; the one a response sets, before its attributes.
        let cookies = match key.as_str() {
            "cookie" => value.split(';').collect(),
            "set-cookie" => value.split(';').take(1).collect(),
            _ => Vec::new(),
        };
        held.extend(cookies.into_iter().filter_map(|c| c.split_once('=')).map(|(_, v)| (key.clone(), v.trim().to_string())));
    }
    found.extend(held);
    found.retain(|(_, value)| value.chars().count() >= crate::secrets::MIN_SECRET_LEN && !value.contains('‹') && !marked.holds(value));
    found.sort();
    found.dedup();
    found
}

/// The tainted key `name` is, named either case: as its flow wrote it.
fn key_of<'k>(keys: &[&'k str], name: &str) -> Option<&'k str> {
    keys.iter().copied().find(|k| k.eq_ignore_ascii_case(name))
}

/// The values of tainted keys in `v`, at any depth.
fn svalue_tainted(v: &crate::wire::SValue, keys: &[&str], found: &mut Vec<(String, String)>) {
    use crate::wire::SValue;
    match v {
        SValue::Record(fields) => {
            for (name, x) in fields {
                if let Some(key) = key_of(keys, name) {
                    leaves(key, x, found);
                }
                svalue_tainted(x, keys, found);
            }
        }
        SValue::List(xs) => {
            // A header as a pair, `[name, value]`.
            if let [SValue::Text(name), x] = xs.as_slice()
                && let Some(key) = key_of(keys, name)
            {
                leaves(key, x, found);
            }
            xs.iter().for_each(|x| svalue_tainted(x, keys, found));
        }
        SValue::Text(s) => text_tainted(s, keys, found),
        SValue::Effect { payload, .. } | SValue::Sum { payload, .. } => svalue_tainted(payload, keys, found),
        SValue::Number(_) | SValue::Boolean(_) | SValue::Bytes(_) => {}
    }
}

/// Every value in `v`, all of it under `key`: a text as it is, a number as it is written.
fn leaves(key: &str, v: &crate::wire::SValue, found: &mut Vec<(String, String)>) {
    use crate::wire::SValue;
    match v {
        SValue::Text(t) => found.push((key.to_string(), t.clone())),
        SValue::Number(n) => found.push((key.to_string(), format!("{n}"))),
        SValue::List(xs) => xs.iter().for_each(|x| leaves(key, x, found)),
        SValue::Record(fields) => fields.iter().for_each(|(_, x)| leaves(key, x, found)),
        SValue::Effect { payload, .. } | SValue::Sum { payload, .. } => leaves(key, payload, found),
        SValue::Boolean(_) | SValue::Bytes(_) => {}
    }
}

/// The values of tainted keys in a text that is JSON (a body, a call's answer) or a query or a
/// form (`a=1&b=2`, after a URL's `?`). A text that names no key is passed by unread.
fn text_tainted(s: &str, keys: &[&str], found: &mut Vec<(String, String)>) {
    let t = s.trim_start();
    let json = t.starts_with('{') || t.starts_with('[');
    if !(json || s.contains('=')) || !keys.iter().any(|k| contains_either_case(s, k)) {
        return;
    }
    if json && let Ok(json) = serde_json::from_str::<serde_json::Value>(t) {
        json_tainted(&json, keys, found);
        return;
    }
    let query = s.split_once('?').map_or(s, |(_, q)| q);
    let query = query.split_once('#').map_or(query, |(q, _)| q);
    for pair in query.split('&') {
        if let Some((name, value)) = pair.split_once('=')
            && let Some(key) = key_of(keys, percent_decode(name.trim()).as_str())
        {
            found.push((key.to_string(), value.to_string()));
            found.push((key.to_string(), percent_decode(value)));
        }
    }
}

/// Whether `s` holds `key`, either case.
fn contains_either_case(s: &str, key: &str) -> bool {
    s.as_bytes().windows(key.len()).any(|w| w.eq_ignore_ascii_case(key.as_bytes()))
}

/// The values under tainted keys anywhere in JSON: a text as it is, a number as it is written,
/// all of one that holds more.
fn json_tainted(json: &serde_json::Value, keys: &[&str], found: &mut Vec<(String, String)>) {
    fn all(key: &str, v: &serde_json::Value, found: &mut Vec<(String, String)>) {
        match v {
            serde_json::Value::String(t) => found.push((key.to_string(), t.clone())),
            serde_json::Value::Number(n) => found.push((key.to_string(), n.to_string())),
            serde_json::Value::Array(xs) => xs.iter().for_each(|x| all(key, x, found)),
            serde_json::Value::Object(fields) => fields.values().for_each(|x| all(key, x, found)),
            serde_json::Value::Bool(_) | serde_json::Value::Null => {}
        }
    }
    match json {
        serde_json::Value::Object(fields) => {
            for (name, value) in fields {
                if let Some(key) = key_of(keys, name) {
                    all(key, value, found);
                }
                json_tainted(value, keys, found);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| json_tainted(v, keys, found)),
        _ => {}
    }
}

/// A Basic credential's text: standard base64, padded or not, read as UTF-8.
fn base64_text(s: &str) -> Option<String> {
    let (mut bits, mut held, mut out) = (0u32, 0, Vec::new());
    for c in s.trim_end_matches('=').bytes() {
        let six = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(six);
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push((bits >> held) as u8);
            bits &= (1 << held) - 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Turn an endpoint's output 0 into a response. See the module doc for the rules.
pub fn respond(output: Option<&Value>) -> DataResponse {
    let Some(output) = output else {
        return DataResponse::json_error(500, "the handler produced no response");
    };
    if let Some(subscribe) = subscription(output) {
        return DataResponse { status: 200, headers: event_stream(vec![]), body: vec![], subscribe: Some(subscribe), file: None };
    }
    if let Some(file) = served(output) {
        return file_answer(200, vec![], file);
    }
    if let Value::Effect { .. } = output {
        return DataResponse { status: 204, headers: vec![], body: vec![], subscribe: None, file: None };
    }
    let (status, mut headers, body) = match output {
        Value::Record(fields) if matches!(fields.get("status"), Some(Value::Number(_))) => {
            let Some(Value::Number(n)) = fields.get("status") else { unreachable!() };
            // Not 1xx: an interim answer (101 switches the connection to another protocol).
            if n.fract() != 0.0 || !(200.0..=599.0).contains(n) {
                return DataResponse::json_error(500, "the handler returned an invalid status");
            }
            let headers = match fields.get("headers") {
                None => vec![],
                Some(Value::Record(h)) => {
                    let mut out = Vec::new();
                    for (k, v) in h {
                        let Value::Text(v) = v else {
                            return DataResponse::json_error(500, "response header values must be text");
                        };
                        // How the reply is framed is the host's to say: a length that isn't the
                        // body's would make the rest of the body read as another response. An
                        // upstream's own (a proxy passing its headers on) is dropped, not refused.
                        let k = k.to_ascii_lowercase();
                        if !FRAMING.contains(&k.as_str()) {
                            out.push((k, v.clone()));
                        }
                    }
                    out
                }
                Some(_) => return DataResponse::json_error(500, "response headers must be a record"),
            };
            if let Some(subscribe) = fields.get("body").and_then(subscription) {
                return DataResponse { status: *n as u16, headers: event_stream(headers), body: vec![], subscribe: Some(subscribe), file: None };
            }
            // A status the flow chose stays; a 204 or a 304 has no body, a file's or any.
            if let Some(file) = fields.get("body").and_then(served)
                && !matches!(*n as u16, 204 | 304)
            {
                return file_answer(*n as u16, headers, file);
            }
            (*n as u16, headers, fields.get("body"))
        }
        other => (200, vec![], Some(other)),
    };
    // A 204 or a 304 has no body (RFC 9110 §15.3.5, §15.4.5): one a computed status chose, as a
    // status alone's empty text, isn't sent, nor its content type.
    if matches!(status, 204 | 304) {
        return DataResponse { status, headers, body: vec![], subscribe: None, file: None };
    }
    let (content_type, bytes) = match body {
        None => (None, vec![]),
        Some(Value::Text(s)) => (Some("text/plain; charset=utf-8"), s.clone().into_bytes()),
        Some(Value::Bytes(b)) => (Some("application/octet-stream"), b.clone()),
        Some(v) => match stringify_json(v) {
            Some(s) => (Some("application/json"), s.into_bytes()),
            None => return DataResponse::json_error(500, "the response body has no JSON form"),
        },
    };
    if let Some(ct) = content_type
        && !headers.iter().any(|(k, _)| k == "content-type")
    {
        headers.push(("content-type".into(), ct.into()));
    }
    DataResponse { status, headers, body: bytes, subscribe: None, file: None }
}

/// A subscription's headers: the flow's, and an event stream nothing caches or buffers.
fn event_stream(mut headers: Vec<(String, String)>) -> Vec<(String, String)> {
    for (k, v) in [("content-type", "text/event-stream"), ("cache-control", "no-cache"), ("x-accel-buffering", "no")] {
        if !headers.iter().any(|(h, _)| h == k) {
            headers.push((k.into(), v.into()));
        }
    }
    headers
}

/// Decode `%XX` escapes and `+` (as a space, the query-string convention). Invalid escapes are
/// kept as written, and the result is lossily UTF-8.
pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit() => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'%'));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_core::{read_server, Edge, Hash, MapResolver, Param, Port, Type, Unit, UnitBody, BOUNDARY};

    fn txt(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn rec(pairs: &[(&str, Value)]) -> Value {
        Value::Record(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    #[test]
    fn request_record_has_every_field_decoded() {
        let r = request_record(
            "GET",
            "/todos/a%20b",
            [("id".to_string(), "a b".to_string())].into(),
            "q=buy+milk&tag=%2Furgent&flag",
            &[("Content-Type".into(), "application/json".into())],
            "{}",
        );
        assert_eq!(
            r,
            rec(&[
                ("method", txt("GET")),
                ("path", txt("/todos/a%20b")),
                ("params", rec(&[("id", txt("a b"))])),
                ("query", rec(&[("q", txt("buy milk")), ("tag", txt("/urgent")), ("flag", txt(""))])),
                ("headers", rec(&[("content-type", txt("application/json"))])),
                ("body", txt("{}")),
            ])
        );
    }

    #[test]
    fn respond_maps_values_envelopes_effects_and_absence() {
        let plain = respond(Some(&rec(&[("id", Value::Number(1.0))])));
        assert_eq!((plain.status, plain.body.as_slice()), (200, &b"{\"id\":1}"[..]));
        assert_eq!(plain.headers, vec![("content-type".to_string(), "application/json".to_string())]);

        let text = respond(Some(&txt("hello")));
        assert_eq!(text.headers[0].1, "text/plain; charset=utf-8");

        let env = respond(Some(&rec(&[
            ("status", Value::Number(201.0)),
            ("headers", rec(&[("Location", txt("/todos/1"))])),
            ("body", rec(&[("id", Value::Number(1.0))])),
        ])));
        assert_eq!(env.status, 201);
        assert!(env.headers.contains(&("location".to_string(), "/todos/1".to_string())));

        let no_body = respond(Some(&rec(&[("status", Value::Number(404.0))])));
        assert_eq!((no_body.status, no_body.body.len()), (404, 0));

        // Bytes are sent as they are, as octet-stream unless the flow says otherwise.
        let raw = respond(Some(&Value::Bytes(vec![0x89, 0x50, 0xff])));
        assert_eq!((raw.status, raw.body.as_slice()), (200, &[0x89, 0x50, 0xff][..]));
        assert_eq!(raw.headers, vec![("content-type".to_string(), "application/octet-stream".to_string())]);
        let png = respond(Some(&rec(&[("status", Value::Number(200.0)), ("headers", rec(&[("content-type", txt("image/png"))])), ("body", Value::Bytes(vec![1]))])));
        assert_eq!((png.body.as_slice(), png.headers.clone()), (&[1u8][..], vec![("content-type".to_string(), "image/png".to_string())]));

        // A 204 a computed status chose sends no body and no content type, whatever the body.
        for body in [txt(""), txt("x")] {
            let none = respond(Some(&rec(&[("status", Value::Number(204.0)), ("body", body)])));
            assert_eq!((none.status, none.body.len(), none.headers.len()), (204, 0, 0));
        }

        let effect = Value::Effect { sink: "store_write:todos".into(), payload: Box::new(Value::List(vec![])) };
        assert_eq!(respond(Some(&effect)).status, 204);

        let ask = |from: &str| Value::Effect { sink: "channel_subscribe:updates".into(), payload: Box::new(rec(&[("channel", txt("updates")), ("from", txt(from))])) };
        let sub = respond(Some(&rec(&[("status", Value::Number(200.0)), ("headers", rec(&[("Cache-Control", txt("no-store"))])), ("body", ask("kept"))])));
        assert_eq!((sub.status, sub.subscribe), (200, Some(Subscribe { channel: "updates".into(), kept: true })));
        assert!(sub.headers.contains(&("content-type".to_string(), "text/event-stream".to_string())));
        assert!(sub.headers.contains(&("cache-control".to_string(), "no-store".to_string())), "the flow's header stands");
        assert_eq!(respond(Some(&ask("new"))).subscribe, Some(Subscribe { channel: "updates".into(), kept: false }));

        // A file: its type, its hash as the etag, no-cache; a status and headers the flow chose stay.
        let hash = "ab".repeat(32);
        let handle = rec(&[("folder", txt("site")), ("name", txt("a.js")), ("size", Value::Number(3.0)), ("hash", txt(&hash)), ("modified", Value::Number(7.0)), ("type", txt("text/javascript; charset=utf-8"))]);
        let serve = Value::Effect { sink: "file_serve:site".into(), payload: Box::new(handle) };
        let served = respond(Some(&serve));
        let file = Served { folder: "site".into(), name: "a.js".into(), size: 3, hash: hash.clone(), modified: 7, content_type: "text/javascript; charset=utf-8".into(), opened: None };
        assert_eq!((served.status, served.file.as_ref(), served.body.len()), (200, Some(&file), 0));
        let header = |r: &DataResponse, k: &str| r.headers.iter().find(|(h, _)| h == k).map(|(_, v)| v.clone());
        assert_eq!(header(&served, "content-type").as_deref(), Some("text/javascript; charset=utf-8"));
        assert_eq!(header(&served, "etag"), Some(format!("\"{hash}\"")));
        assert_eq!(header(&served, "cache-control").as_deref(), Some("no-cache"));
        let headers = rec(&[("cache-control", txt("public, max-age=31536000, immutable")), ("etag", txt("\"mine\"")), ("content-type", txt("text/plain"))]);
        let chosen = respond(Some(&rec(&[("status", Value::Number(404.0)), ("headers", headers), ("body", serve.clone())])));
        assert_eq!((chosen.status, chosen.file.is_some()), (404, true));
        assert_eq!(header(&chosen, "cache-control").as_deref(), Some("public, max-age=31536000, immutable"));
        assert_eq!(header(&chosen, "content-type").as_deref(), Some("text/plain"), "the flow's type stands");
        assert_eq!(header(&chosen, "etag"), Some(format!("\"{hash}\"")), "the etag is the host's");
        let none = respond(Some(&rec(&[("status", Value::Number(204.0)), ("body", serve)])));
        assert_eq!((none.status, none.file), (204, None));
        assert_eq!(file.line(), format!("file site/a.js (3 bytes, blake3 abababab…)"));
        assert_eq!(respond(None).status, 500);
        assert_eq!(respond(Some(&rec(&[("status", Value::Number(99.0))]))).status, 500);
        assert_eq!(respond(Some(&rec(&[("status", Value::Number(200.5))]))).status, 500);
        assert_eq!(respond(Some(&Value::Number(f64::NAN))).status, 500);
    }

    /// An endpoint whose handler returns the request's `params` record.
    fn params_endpoint(r: &mut MapResolver, method: &str, path: &str) -> Hash {
        let h = r
            .insert(Unit {
                body: UnitBody::Primitive { name: "field".into() },
                params: vec![Param::frozen("name", "params")],
                inputs: vec![Port { name: "record".into(), ty: Type::Record }],
                outputs: vec![Port { name: "out".into(), ty: Type::Any }],
            })
            .unwrap();
        r.insert(Unit {
            body: UnitBody::Composite {
                children: vec![h],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                ],
            },
            params: vec![
                Param::frozen("declare", "endpoint"),
                Param::frozen("method", method),
                Param::frozen("path", path),
            ],
            inputs: vec![Port { name: "request".into(), ty: Type::Record }],
            outputs: vec![Port { name: "response".into(), ty: Type::Any }],
        })
        .unwrap()
    }

    #[test]
    fn handle_routes_and_reports_404_and_405() {
        let mut r = MapResolver::new();
        let e = params_endpoint(&mut r, "GET", "/todos/:id");
        let server_unit = Unit {
            body: UnitBody::Composite { children: vec![e], edges: vec![] },
            params: vec![Param::frozen("declare", "server"), Param::frozen("port", "8080")],
            inputs: vec![],
            outputs: vec![],
        };
        let server = read_server(&server_unit, &r).unwrap();
        let run = |method: &str, url: &str| {
            let http = HttpRequest { method, url, headers: &[], body: b"" };
            handle(&server, &r, &http, |unit, _, request, _| holon_core::eval(unit, &[Some(request)], &r).into_vec(), &[])
        };
        let ok = run("GET", "/todos/7?x=1");
        assert_eq!((ok.status, ok.body.as_slice()), (200, &b"{\"id\":\"7\"}"[..]));
        assert_eq!(run("GET", "/nope").status, 404);
        let wrong = run("DELETE", "/todos/7");
        assert_eq!(wrong.status, 405);
        assert!(wrong.headers.contains(&("allow".to_string(), "GET".to_string())));
    }

    #[test]
    fn a_rest_is_the_rest_of_the_path_decoded_and_counts_for_405() {
        let mut r = MapResolver::new();
        let files = params_endpoint(&mut r, "GET", "/files/*path");
        let post = params_endpoint(&mut r, "POST", "/*all");
        let server_unit = Unit {
            body: UnitBody::Composite { children: vec![files, post], edges: vec![] },
            params: vec![Param::frozen("declare", "server"), Param::frozen("port", "8080")],
            inputs: vec![],
            outputs: vec![],
        };
        let server = read_server(&server_unit, &r).unwrap();
        let run = |method: &str, url: &str| {
            let http = HttpRequest { method, url, headers: &[], body: b"" };
            handle(&server, &r, &http, |unit, _, request, _| holon_core::eval(unit, &[Some(request)], &r).into_vec(), &[])
        };
        let ok = run("GET", "/files/css/site%20a.css?v=2");
        assert_eq!((ok.status, String::from_utf8_lossy(&ok.body).into_owned()), (200, "{\"path\":\"css/site a.css\"}".to_string()));
        // A part that would name another path is no rest's, of any method.
        for other in ["/files/a%2Fb", "/files/..", "/files/"] {
            assert_eq!(run("GET", other).status, 404, "{other}");
        }
        let wrong = run("PUT", "/files/a");
        assert_eq!(wrong.status, 405);
        assert!(wrong.headers.contains(&("allow".to_string(), "GET, POST".to_string())));
    }
}
