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
//! - nothing at all (the handler didn't settle) is a 500.
//!
//! A text body goes out as `text/plain`, anything else as JSON, unless the envelope sets
//! `content-type` itself.

use std::collections::BTreeMap;

use holon_core::{match_route, stringify_json, Capability, Resolver, Server, Value};

/// What to send back, in stack-neutral form.
#[derive(Debug, PartialEq)]
pub struct DataResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl DataResponse {
    pub(crate) fn json_error(status: u16, message: &str) -> DataResponse {
        let body = Value::Record([("error".to_string(), Value::Text(message.to_string()))].into());
        DataResponse {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: stringify_json(&body).unwrap_or_default().into_bytes(),
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
    fire: impl FnOnce(&holon_core::Unit, &str, Value, &[(Capability, Value)]) -> Vec<Option<Value>>,
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
    let outputs = fire(endpoint, &format!("{} {}", route.method, route.path), request, &sourced);
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

/// Turn an endpoint's output 0 into a response. See the module doc for the rules.
pub fn respond(output: Option<&Value>) -> DataResponse {
    let Some(output) = output else {
        return DataResponse::json_error(500, "the handler produced no response");
    };
    if let Value::Effect { .. } = output {
        return DataResponse { status: 204, headers: vec![], body: vec![] };
    }
    let (status, mut headers, body) = match output {
        Value::Record(fields) if matches!(fields.get("status"), Some(Value::Number(_))) => {
            let Some(Value::Number(n)) = fields.get("status") else { unreachable!() };
            if n.fract() != 0.0 || !(100.0..=599.0).contains(n) {
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
                        out.push((k.to_ascii_lowercase(), v.clone()));
                    }
                    out
                }
                Some(_) => return DataResponse::json_error(500, "response headers must be a record"),
            };
            (*n as u16, headers, fields.get("body"))
        }
        other => (200, vec![], Some(other)),
    };
    let (content_type, bytes) = match body {
        None => (None, vec![]),
        Some(Value::Text(s)) => (Some("text/plain; charset=utf-8"), s.clone().into_bytes()),
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
    DataResponse { status, headers, body: bytes }
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

        let effect = Value::Effect { sink: "store_write:todos".into(), payload: Box::new(Value::List(vec![])) };
        assert_eq!(respond(Some(&effect)).status, 204);
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
}
