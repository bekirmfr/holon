//! Declaration units (M1, docs/PLATFORM_LAYOUT.md §5, option A): an HTTP API as ordinary units.
//!
//! - An **endpoint** is a composite (any internal graph: the handler) from a `request` record to a
//!   response, carrying frozen params `declare = "endpoint"`, `method` and `path`. Evaluating it runs the handler, so testing
//!   an endpoint is plain `eval`. Output 0 is the response; any further outputs carry effects
//!   (state writes) for the host to perform.
//! - A **server** is a composite whose children are endpoints, with no wiring and no ports,
//!   carrying `declare = "server"` and `port`. Its hash is the hash of the whole API.
//!
//! Nothing here changes the encoding: the markers are ordinary frozen params. This module is the
//! one place that reads them, used by the host (to listen and route) and the editor (to check a
//! server before deploying) alike.

use std::collections::BTreeMap;

use crate::{Hash, Resolver, Type, Unit, UnitBody};

/// The methods an endpoint may declare.
pub const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];

/// One segment of a route path: `/todos/:id` is `[Literal("todos"), Param("id")]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    Param(String),
}

/// One endpoint of a server, as declared.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub method: String,
    /// The path exactly as declared, e.g. `/todos/:id`.
    pub path: String,
    pub segments: Vec<Segment>,
    /// The endpoint unit's hash (the server's child), which is what a request evaluates.
    pub endpoint: Hash,
}

/// A server unit, read and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub port: u16,
    pub routes: Vec<Route>,
}

/// Why a unit is not a well-formed server. `child` is the index of the offending endpoint.
#[derive(Clone, Debug, PartialEq)]
pub enum DeclError {
    NotAServer,
    BadPort(String),
    ServerHasWiring,
    UnresolvedEndpoint { child: u32 },
    NotAnEndpoint { child: u32 },
    BadMethod { child: u32, method: String },
    BadPath { child: u32, path: String },
    /// An endpoint takes exactly one `Record`-compatible input and has at least one output.
    BadEndpointShape { child: u32 },
    DuplicateRoute { method: String, path: String },
}

/// The declaration a unit carries (`"server"`, `"endpoint"`), if any.
pub fn declaration_of(unit: &Unit) -> Option<&str> {
    frozen(unit, "declare")
}

fn frozen<'a>(unit: &'a Unit, name: &str) -> Option<&'a str> {
    unit.params.iter().find(|p| p.name() == name).and_then(|p| p.value())
}

/// Read a server unit and check every endpoint. Fails closed on anything malformed, so the host
/// never listens on half a declaration.
pub fn read_server(unit: &Unit, resolver: &impl Resolver) -> Result<Server, DeclError> {
    let UnitBody::Composite { children, edges } = &unit.body else {
        return Err(DeclError::NotAServer);
    };
    if declaration_of(unit) != Some("server") {
        return Err(DeclError::NotAServer);
    }
    let port_text = frozen(unit, "port").unwrap_or("");
    let port = match port_text.parse::<u16>() {
        Ok(p) if p != 0 => p,
        _ => return Err(DeclError::BadPort(port_text.to_string())),
    };
    if !edges.is_empty() || !unit.inputs.is_empty() || !unit.outputs.is_empty() {
        return Err(DeclError::ServerHasWiring);
    }

    let mut routes: Vec<Route> = Vec::with_capacity(children.len());
    for (i, hash) in children.iter().enumerate() {
        let child = i as u32;
        let endpoint = resolver.resolve(hash).ok_or(DeclError::UnresolvedEndpoint { child })?;
        let (method, path) = read_endpoint(endpoint).map_err(|e| match e {
            EndpointError::NotAnEndpoint => DeclError::NotAnEndpoint { child },
            EndpointError::BadMethod(method) => DeclError::BadMethod { child, method },
            EndpointError::BadPath(path) => DeclError::BadPath { child, path },
            EndpointError::BadShape => DeclError::BadEndpointShape { child },
        })?;
        let segments = parse_path(&path).ok_or_else(|| DeclError::BadPath { child, path: path.clone() })?;
        // Two routes clash when they would match exactly the same requests: same method, and the
        // same segments with every param treated alike (`/todos/:id` and `/todos/:key` clash).
        let shape = |s: &[Segment]| -> Vec<Option<String>> {
            s.iter()
                .map(|seg| match seg {
                    Segment::Literal(l) => Some(l.clone()),
                    Segment::Param(_) => None,
                })
                .collect()
        };
        if routes.iter().any(|r| r.method == method && shape(&r.segments) == shape(&segments)) {
            return Err(DeclError::DuplicateRoute { method, path });
        }
        routes.push(Route { method, path, segments, endpoint: *hash });
    }
    Ok(Server { port, routes })
}

/// Why a unit is not a well-formed endpoint.
#[derive(Clone, Debug, PartialEq)]
pub enum EndpointError {
    NotAnEndpoint,
    BadMethod(String),
    BadPath(String),
    BadShape,
}

/// Read an endpoint's method and path, checking its shape.
pub fn read_endpoint(unit: &Unit) -> Result<(String, String), EndpointError> {
    if !matches!(unit.body, UnitBody::Composite { .. }) || declaration_of(unit) != Some("endpoint") {
        return Err(EndpointError::NotAnEndpoint);
    }
    let method = frozen(unit, "method").unwrap_or("").to_string();
    if !METHODS.contains(&method.as_str()) {
        return Err(EndpointError::BadMethod(method));
    }
    let path = frozen(unit, "path").unwrap_or("").to_string();
    if parse_path(&path).is_none() {
        return Err(EndpointError::BadPath(path));
    }
    let request_ok = matches!(unit.inputs.as_slice(), [p] if crate::compatible(&Type::Record, &p.ty));
    if !request_ok || unit.outputs.is_empty() {
        return Err(EndpointError::BadShape);
    }
    Ok((method, path))
}

/// Parse a route path: `/`, or `/` followed by segments that are literals (letters, digits,
/// `-`, `_`, `.`, `~`) or params, written `:name` or `{name}` (the same param either way).
/// `None` if malformed, including a repeated param name.
pub fn parse_path(path: &str) -> Option<Vec<Segment>> {
    let rest = path.strip_prefix('/')?;
    if rest.is_empty() {
        return Some(vec![]);
    }
    let word = |s: &str| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
    };
    let mut segments = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        let param = part.strip_prefix(':').or_else(|| part.strip_prefix('{').and_then(|p| p.strip_suffix('}')));
        match param {
            Some(name) if word(name) && !names.contains(&name) => {
                names.push(name);
                segments.push(Segment::Param(name.to_string()));
            }
            None if word(part) => segments.push(Segment::Literal(part.to_string())),
            _ => return None,
        }
    }
    Some(segments)
}

/// The route a request should go to, with its path params. Among routes that match, the one
/// with the most literal segments wins, so `/todos/new` beats `/todos/:id`; ties go to the
/// earlier route. `request_path` is the path only, without the query string.
pub fn match_route<'a>(
    server: &'a Server,
    method: &str,
    request_path: &str,
) -> Option<(&'a Route, BTreeMap<String, String>)> {
    let parts: Vec<&str> = match request_path.strip_prefix('/')? {
        "" => vec![],
        rest => rest.split('/').collect(),
    };
    let mut best: Option<(&Route, BTreeMap<String, String>, usize)> = None;
    for route in server.routes.iter().filter(|r| r.method == method && r.segments.len() == parts.len()) {
        let mut params = BTreeMap::new();
        let mut literals = 0;
        let matched = route.segments.iter().zip(&parts).all(|(seg, part)| match seg {
            Segment::Literal(l) => {
                literals += 1;
                l == part
            }
            Segment::Param(name) => {
                params.insert(name.clone(), part.to_string());
                !part.is_empty()
            }
        });
        if matched && best.as_ref().is_none_or(|(_, _, n)| literals > *n) {
            best = Some((route, params, literals));
        }
    }
    best.map(|(route, params, _)| (route, params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hash_unit, Edge, MapResolver, Param, Port, BOUNDARY};

    fn handler() -> Unit {
        Unit {
            body: UnitBody::Primitive { name: "field".into() },
            params: vec![Param::frozen("name", "body")],
            inputs: vec![Port { name: "record".into(), ty: Type::Record }],
            outputs: vec![Port { name: "out".into(), ty: Type::Any }],
        }
    }

    fn endpoint(r: &mut MapResolver, method: &str, path: &str) -> Hash {
        let h = r.insert(handler()).unwrap();
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

    fn server(port: &str, children: Vec<Hash>) -> Unit {
        Unit {
            body: UnitBody::Composite { children, edges: vec![] },
            params: vec![Param::frozen("declare", "server"), Param::frozen("port", port)],
            inputs: vec![],
            outputs: vec![],
        }
    }

    #[test]
    fn reads_a_server_and_routes_requests() {
        let mut r = MapResolver::new();
        let list = endpoint(&mut r, "GET", "/todos");
        let one = endpoint(&mut r, "GET", "/todos/:id");
        let new = endpoint(&mut r, "GET", "/todos/new");
        let create = endpoint(&mut r, "POST", "/todos");
        let s = read_server(&server("8080", vec![list, one, new, create]), &r).unwrap();
        assert_eq!(s.port, 8080);
        assert_eq!(s.routes.len(), 4);

        let (route, params) = match_route(&s, "GET", "/todos/42").unwrap();
        assert_eq!((route.endpoint, params.get("id").map(String::as_str)), (one, Some("42")));
        assert_eq!(match_route(&s, "GET", "/todos/new").unwrap().0.endpoint, new, "literal beats param");
        assert_eq!(match_route(&s, "POST", "/todos").unwrap().0.endpoint, create);
        assert_eq!(match_route(&s, "GET", "/todos").unwrap().0.endpoint, list);
        assert!(match_route(&s, "DELETE", "/todos/1").is_none());
        assert!(match_route(&s, "GET", "/todos/1/extra").is_none());
        assert!(match_route(&s, "GET", "/todos/").is_none(), "an empty segment is not an id");
    }

    #[test]
    fn a_param_is_written_colon_or_braces_alike() {
        let p = |name: &str| Some(vec![Segment::Literal("todos".into()), Segment::Param(name.into())]);
        assert_eq!(parse_path("/todos/{title}"), p("title"));
        assert_eq!(parse_path("/todos/:title"), p("title"));
        for bad in ["/todos/{}", "/todos/{title", "/todos/title}", "/todos/{a b}", "/a/{x}/:x", "/a/{{x}}"] {
            assert_eq!(parse_path(bad), None, "{bad}");
        }
        // A route matches the same either way, and the two spellings clash on one server.
        let mut r = MapResolver::new();
        let braces = endpoint(&mut r, "GET", "/todos/{title}");
        let colon = endpoint(&mut r, "GET", "/todos/:title");
        let s = read_server(&server("80", vec![braces]), &r).unwrap();
        let (_, params) = match_route(&s, "GET", "/todos/buy%20milk").unwrap();
        assert_eq!(params.get("title").map(String::as_str), Some("buy%20milk"), "raw here; the host decodes");
        assert!(matches!(read_server(&server("80", vec![braces, colon]), &r), Err(DeclError::DuplicateRoute { .. })));
    }

    #[test]
    fn rejects_malformed_servers() {
        let mut r = MapResolver::new();
        let ok = endpoint(&mut r, "GET", "/a");
        for port in ["0", "70000", "http", ""] {
            assert_eq!(read_server(&server(port, vec![ok]), &r), Err(DeclError::BadPort(port.into())));
        }
        let clash = endpoint(&mut r, "GET", "/a/:x");
        let clash2 = endpoint(&mut r, "GET", "/a/:y");
        assert!(matches!(read_server(&server("80", vec![clash, clash2]), &r), Err(DeclError::DuplicateRoute { .. })));
        let bad_method = endpoint(&mut r, "FETCH", "/a");
        assert!(matches!(read_server(&server("80", vec![bad_method]), &r), Err(DeclError::BadMethod { child: 0, .. })));
        for path in ["a", "/a//b", "/a/:", "/a/:x/:x", "/a b", "/a?b"] {
            let e = endpoint(&mut r, "GET", path);
            assert!(matches!(read_server(&server("80", vec![e]), &r), Err(DeclError::BadPath { .. })), "{path}");
        }
        let plain = r.insert(handler()).unwrap();
        assert_eq!(read_server(&server("80", vec![plain]), &r), Err(DeclError::NotAnEndpoint { child: 0 }));
        assert_eq!(
            read_server(&server("80", vec![[7; 32]]), &r),
            Err(DeclError::UnresolvedEndpoint { child: 0 })
        );
        assert_eq!(read_server(&handler(), &r), Err(DeclError::NotAServer));
        // Parse the root path, which has no segments.
        assert_eq!(parse_path("/"), Some(vec![]));
    }

    #[test]
    fn an_endpoint_evaluates_as_its_handler_and_a_server_listens() {
        let mut r = MapResolver::new();
        let e = endpoint(&mut r, "POST", "/echo");
        let request = crate::Value::Record([("body".to_string(), crate::Value::Text("hi".into()))].into());
        let out = crate::eval(r.resolve(&e).unwrap(), &[Some(request)], &r);
        assert_eq!(out.into_vec(), vec![Some(crate::Value::Text("hi".into()))]);

        let caps = crate::capabilities(&server("80", vec![e]), &r);
        assert!(caps.contains(crate::Capability::NetworkIngress), "a server's manifest says it listens");
        assert!(crate::capabilities(r.resolve(&e).unwrap(), &r).is_pure(), "an endpoint alone does not");
    }

    #[test]
    fn a_server_is_an_ordinary_hashable_unit() {
        // The whole API is one hash: changing a route changes it.
        let mut r = MapResolver::new();
        let a = endpoint(&mut r, "GET", "/a");
        let b = endpoint(&mut r, "GET", "/b");
        assert_ne!(hash_unit(&server("80", vec![a])).unwrap(), hash_unit(&server("80", vec![b])).unwrap());
        assert_ne!(hash_unit(&server("80", vec![a])).unwrap(), hash_unit(&server("81", vec![a])).unwrap());
    }
}
