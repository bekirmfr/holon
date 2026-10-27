//! HTTP node templates: `request`, `respond` and `stripe_verify`. They are not new primitives.
//! Each is a small composite of genesis leaves (field, json_parse, record, with_field, const,
//! default, …), rebuilt from the node's params whenever it is hashed, the same way a primitive
//! node is rebuilt from its catalog entry.
//!
//! - `request` (params: method, path, resume) takes the endpoint's request record on one input,
//!   hidden on the canvas: it is fed by the endpoint the route derives into (routes.rs), never by
//!   a wire. Its outputs are the request's parts: the whole record, path params, query, headers,
//!   the raw body text, and the body parsed as JSON.
//! - `respond` (params: status, missing, headers) builds the `{status, body}` envelope the data
//!   plane sends. It also takes the request (from the request node), which it doesn't read: that
//!   wire is what makes a route one connected group even when the handler never looks at the
//!   request, e.g. a listing. Status 204 has no body input. With `headers: true` it has a third
//!   input, a record of response headers (`content-type` for a page, `location` for a redirect),
//!   put in the envelope as `headers`; without it, the data plane's content type stands. With
//!   `status_input: true` its last input is the status, in place of the `status` param (a
//!   streamed reply's, its call's status). When
//!   the body input is absent (a lookup that found nothing), it answers its `missing` status
//!   instead (404 by default) with `{"error": …}`. A respond node without `missing` (placed before
//!   it existed) answers nothing, sent as a 500.
//! - `stripe_verify` (param: tolerance, seconds) checks a Stripe webhook's signature, as Stripe's
//!   own libraries do: the `stripe-signature` header (`t=<unix time>,v1=<hex HMAC>,…`) taken apart
//!   into a record (split on "," then each piece on "=", folded into a record), the HMAC-SHA256
//!   of `<t>.<raw body>` under the endpoint's signing secret compared with `v1` in constant time,
//!   and `t` no older than `tolerance` (300 s, Stripe's default) as of `now`. Inputs: the request,
//!   the secret (a `secret` node), and `now` (a `clock` node: the template stays pure, so a replay
//!   checks against the time the run saw). Outputs: `event`, the body parsed as JSON, only when
//!   verified (absent otherwise, so a `respond` with `missing` 400 refuses it); and `verified`
//!   (absent when the header is missing or malformed). With several `v1` (Stripe signs with both
//!   while a secret is being rolled), any one matching passes: each piece is compared with
//!   `v1=<expected>` in constant time, folded into whether any matched.
//!
//! A template's units are part of every flow's identity that uses it: a change here changes the
//! hash of each such flow.

use holon_core::{interface_of, Edge, Hash, MapResolver, Param, Port, Type, Unit, UnitBody, BOUNDARY};
use serde::Serialize;

use crate::wire::{frozen_value, WParam, WPort, WType};

/// The templates, in palette order.
pub const TEMPLATES: &[&str] = &["request", "respond", "stripe_verify"];

pub fn is_template(name: &str) -> bool {
    TEMPLATES.contains(&name)
}

/// What each template does, for the person placing it (core describes the primitives).
pub fn describe(name: &str) -> Option<&'static str> {
    Some(match name {
        "request" => {
            "A route: answers requests with the method and path set on the node (path params like :id). Gives the whole request, and its params, query, headers, raw body, and body parsed as JSON. With a respond it wires to, one route of the flow's server."
        }
        "respond" => {
            "Answers the route's request: body as the response, with the status set on the node (a record or list is sent as JSON). When body didn't arrive, it answers the missing status instead (404 unless set) with an error."
        }
        "stripe_verify" => {
            "Checks a Stripe webhook's signature as Stripe's own libraries do: the HMAC-SHA256 of the timestamp and raw body under the endpoint's secret, compared in constant time with each v1 signature (any one matching passes, as while a secret is rolled), the timestamp within tolerance seconds of now. Gives the event (the body parsed) only when it verifies; otherwise nothing, so a respond refuses it."
        }
        _ => return None,
    })
}

/// A template node's interface for a given set of params (a respond node's inputs depend on its
/// status). `hidden_inputs`: the inputs that are fed by the endpoint, not drawn as handles.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Interface {
    pub inputs: Vec<WPort>,
    pub outputs: Vec<WPort>,
    #[serde(rename = "hiddenInputs")]
    pub hidden_inputs: bool,
}

pub fn interface(name: &str, params: &[WParam]) -> Option<Interface> {
    use WType::*;
    let request = || WPort::new("request", Record);
    Some(match name {
        "request" => Interface {
            inputs: vec![request()],
            outputs: vec![
                WPort::new("request", Record),
                WPort::new("params", Record),
                WPort::new("query", Record),
                WPort::new("headers", Record),
                WPort::new("body", Text),
                WPort::new("json", Any),
            ],
            hidden_inputs: true,
        },
        "stripe_verify" => Interface {
            inputs: vec![request(), WPort::new("secret", Text), WPort::new("now", Number)],
            outputs: vec![WPort::new("event", Any), WPort::new("verified", Boolean)],
            hidden_inputs: false,
        },
        "respond" => {
            let no_body = frozen_value(params, "status") == Some("204");
            let mut inputs = if no_body { vec![request()] } else { vec![request(), WPort::new("body", Any)] };
            if frozen_value(params, "headers") == Some("true") {
                inputs.push(WPort::new("headers", Record));
            }
            if frozen_value(params, "status_input") == Some("true") {
                inputs.push(WPort::new("status", Number));
            }
            Interface { inputs, outputs: vec![WPort::new("response", Record)], hidden_inputs: false }
        }
        _ => return None,
    })
}

/// The params a freshly dropped template node starts with.
pub fn defaults(name: &str) -> Option<Vec<WParam>> {
    let f = WParam::frozen;
    Some(match name {
        "request" => vec![f("method", "GET"), f("path", "/"), f("resume", "")],
        "respond" => vec![f("status", "200"), f("missing", "404"), f("headers", "false"), f("status_input", "false")],
        "stripe_verify" => vec![f("tolerance", "300")],
        _ => return None,
    })
}

/// `s` is `min..=max` ASCII digits.
fn digits(s: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

fn frozen(name: &str, value: &str) -> Param {
    Param::Frozen { name: name.into(), value: value.into() }
}

fn e(from_child: u32, from_port: u32, to_child: u32, to_port: u32) -> Edge {
    Edge { from_child, from_port, to_child, to_port }
}

fn put(store: &mut MapResolver, u: Unit) -> Result<Hash, String> {
    store.insert(u).map_err(|e| format!("{e}"))
}

/// A genesis leaf as the catalog declares it, with `params` (and, for a variadic one, `inputs`).
fn add_leaf(store: &mut MapResolver, prim: &str, params: Vec<Param>, inputs: Option<Vec<Port>>) -> Result<Hash, String> {
    let mut u = interface_of(prim).ok_or_else(|| format!("\"{prim}\" is not in the catalog"))?;
    u.params = params;
    if let Some(inputs) = inputs {
        u.inputs = inputs;
    }
    put(store, u)
}

fn composite(children: Vec<Hash>, edges: Vec<Edge>, inputs: Vec<Port>, outputs: Vec<Port>) -> Unit {
    Unit { body: UnitBody::Composite { children, edges }, params: vec![], inputs, outputs }
}

/// The composite a template node stands for, its children stored in `store`. Each leaf is the
/// genesis catalog's interface with the given params, so it hashes exactly like the same leaf
/// dropped from the palette. An error message for params the template can't build from.
pub fn build(name: &str, params: &[WParam], store: &mut MapResolver) -> Result<Unit, String> {
    let iface = interface(name, params).ok_or_else(|| format!("\"{name}\" is not a template"))?;
    let ports = |ps: Vec<WPort>| ps.into_iter().map(WPort::into_core).collect::<Result<Vec<_>, _>>();
    let (inputs, outputs) = (ports(iface.inputs)?, ports(iface.outputs)?);
    const B: u32 = BOUNDARY;

    match name {
        "request" => {
            let mut leaf = |prim: &str, p: Vec<Param>| add_leaf(store, prim, p, None);
            let children = vec![
                leaf("field", vec![frozen("name", "params")])?,
                leaf("field", vec![frozen("name", "query")])?,
                leaf("field", vec![frozen("name", "headers")])?,
                leaf("field", vec![frozen("name", "body")])?,
                leaf("json_parse", vec![])?,
            ];
            let edges = vec![
                e(B, 0, 0, 0),
                e(B, 0, 1, 0),
                e(B, 0, 2, 0),
                e(B, 0, 3, 0),
                e(3, 0, 4, 0),
                e(B, 0, B, 0), // request: the record itself
                e(0, 0, B, 1),
                e(1, 0, B, 2),
                e(2, 0, B, 3),
                e(3, 0, B, 4),
                e(4, 0, B, 5),
            ];
            Ok(composite(children, edges, inputs, outputs))
        }

        "stripe_verify" => {
            let tolerance = frozen_value(params, "tolerance").unwrap_or("");
            if !digits(tolerance, 1, 6) || tolerance.parse::<u32>().unwrap_or(0) < 1 {
                return Err(format!("tolerance \"{tolerance}\" is not a number of seconds (1–999999)"));
            }
            let mut leaf = |prim: &str, p: Vec<Param>, ins: Option<Vec<Port>>| add_leaf(store, prim, p, ins);
            let text = |v: &str| vec![frozen("value", v), frozen("type", "Text")];
            let number = |v: &str| vec![frozen("value", v), frozen("type", "Number")];
            let acc_item = || vec![Port { name: "acc".into(), ty: Type::Record }, Port { name: "item".into(), ty: Type::Text }];
            let acc = || vec![Port { name: "acc".into(), ty: Type::Record }];
            // (record, "k=v") → record with k set to v: what the header's pieces fold into.
            let pair_children = vec![
                leaf("split", vec![], None)?,
                leaf("const", text("="), None)?,
                leaf("get", vec![], None)?,
                leaf("get", vec![], None)?,
                leaf("const", number("0"), None)?,
                leaf("const", number("1"), None)?,
                leaf("insert", vec![], None)?,
            ];
            let pair = composite(
                pair_children,
                vec![
                    e(B, 1, 0, 0),
                    e(1, 0, 0, 1),
                    e(0, 0, 2, 0),
                    e(4, 0, 2, 1),
                    e(0, 0, 3, 0),
                    e(5, 0, 3, 1),
                    e(B, 0, 6, 0),
                    e(2, 0, 6, 1),
                    e(3, 0, 6, 2),
                    e(6, 0, B, 0),
                ],
                acc_item(),
                acc(),
            );
            // ({want, ok}, piece) → ok once any piece equals want (`v1=<expected>`), each compared
            // in constant time: what the header's pieces fold into for the signature check.
            let any_children = vec![
                leaf("field", vec![frozen("name", "want")], None)?, // 0
                leaf("secure_equals", vec![], None)?,               // 1
                leaf("field", vec![frozen("name", "ok")], None)?,   // 2
                leaf("or", vec![], None)?,                          // 3
                leaf("with_field", vec![frozen("name", "ok")], None)?, // 4
            ];
            let any_match = composite(
                any_children,
                vec![
                    e(B, 0, 0, 0),
                    e(B, 1, 1, 0),
                    e(0, 0, 1, 1),
                    e(B, 0, 2, 0),
                    e(2, 0, 3, 0),
                    e(1, 0, 3, 1),
                    e(B, 0, 4, 0),
                    e(3, 0, 4, 1),
                    e(4, 0, B, 0),
                ],
                acc_item(),
                acc(),
            );
            let pair = put(store, pair)?;
            let any_match = put(store, any_match)?;
            let fn_of = |h: &Hash| frozen("fn", &crate::to_hex(h));
            let three = vec![
                Port { name: "a".into(), ty: Type::Text },
                Port { name: "b".into(), ty: Type::Text },
                Port { name: "c".into(), ty: Type::Text },
            ];
            let mut leaf = |prim: &str, p: Vec<Param>, ins: Option<Vec<Port>>| add_leaf(store, prim, p, ins);
            let children = vec![
                leaf("field", vec![frozen("name", "headers")], None)?, // 0
                leaf("field", vec![frozen("name", "stripe-signature")], None)?, // 1
                leaf("const", text(","), None)?,                       // 2
                leaf("split", vec![], None)?,                          // 3: the header's pieces
                leaf("record", vec![], None)?,                         // 4
                leaf("fold", vec![fn_of(&pair)], None)?,               // 5: … as a record
                leaf("field", vec![frozen("name", "t")], None)?,       // 6
                leaf("const", text("v1="), None)?,                     // 7
                leaf("field", vec![frozen("name", "body")], None)?,    // 8: the raw body, exactly as sent
                leaf("const", text("."), None)?,                       // 9
                leaf("concat", vec![], Some(three))?,                  // 10: t.body, what Stripe signed
                leaf("hmac_sha256", vec![frozen("encoding", "hex")], None)?, // 11
                leaf("field", vec![frozen("name", "ok")], None)?,      // 12: a v1 signature matches
                leaf("json_parse", vec![], None)?,                     // 13: t as a number
                leaf("subtract", vec![], None)?,                       // 14: its age
                leaf("const", number(tolerance), None)?,               // 15
                leaf("compare", vec![frozen("op", "<=")], None)?,      // 16: recent enough
                leaf("and", vec![], None)?,                            // 17: verified
                leaf("json_parse", vec![], None)?,                     // 18: the event
                leaf("guard", vec![], None)?,                          // 19: … only when verified
                leaf("concat", vec![], None)?,                         // 20: v1=<expected>
                leaf("record", vec![], None)?,                         // 21
                leaf("with_field", vec![frozen("name", "want")], None)?, // 22
                leaf("const", vec![frozen("value", "false"), frozen("type", "Boolean")], None)?, // 23
                leaf("with_field", vec![frozen("name", "ok")], None)?, // 24: {want, ok: false}
                leaf("fold", vec![fn_of(&any_match)], None)?,          // 25: … over every piece of the header
            ];
            let edges = vec![
                e(B, 0, 0, 0),
                e(0, 0, 1, 0),
                e(1, 0, 3, 0),
                e(2, 0, 3, 1),
                e(4, 0, 5, 0),
                e(3, 0, 5, 1),
                e(5, 0, 6, 0),
                e(5, 0, 7, 0),
                e(B, 0, 8, 0),
                e(6, 0, 10, 0),
                e(9, 0, 10, 1),
                e(8, 0, 10, 2),
                e(B, 1, 11, 0),
                e(10, 0, 11, 1),
                e(7, 0, 20, 0),
                e(11, 0, 20, 1),
                e(21, 0, 22, 0),
                e(20, 0, 22, 1),
                e(22, 0, 24, 0),
                e(23, 0, 24, 1),
                e(24, 0, 25, 0),
                e(3, 0, 25, 1),
                e(25, 0, 12, 0),
                e(6, 0, 13, 0),
                e(B, 2, 14, 0),
                e(13, 0, 14, 1),
                e(14, 0, 16, 0),
                e(15, 0, 16, 1),
                e(12, 0, 17, 0),
                e(16, 0, 17, 1),
                e(8, 0, 18, 0),
                e(17, 0, 19, 0),
                e(18, 0, 19, 1),
                e(19, 0, B, 0),
                e(17, 0, B, 1),
            ];
            Ok(composite(children, edges, inputs, outputs))
        }

        // respond
        _ => {
            let status = frozen_value(params, "status").unwrap_or("");
            let n: u32 = status.parse().unwrap_or(0);
            if !digits(status, 3, 3) || !(200..=599).contains(&n) {
                return Err(format!("status \"{status}\" is not an HTTP status a reply can have (200–599)"));
            }
            let missing = frozen_value(params, "missing").unwrap_or("");
            let m: u32 = missing.parse().unwrap_or(0);
            if !missing.is_empty() && (!digits(missing, 3, 3) || !(400..=599).contains(&m)) {
                return Err(format!("missing \"{missing}\" is not an error status (400–599)"));
            }
            let with_headers = frozen_value(params, "headers") == Some("true");
            let has_body = inputs.iter().any(|p| p.name == "body");
            let mut leaf = |prim: &str, p: Vec<Param>| add_leaf(store, prim, p, None);
            // The status: the param's, or (`status_input: true`) the last input's, so a streamed
            // reply answers with its call's status.
            let wired = inputs.iter().position(|p| p.name == "status").map(|i| i as u32);
            let mut children = vec![leaf("record", vec![])?];
            let mut edges = Vec::new();
            match wired {
                Some(port) => {
                    children.push(leaf("with_field", vec![frozen("name", "status")])?);
                    edges.extend([e(0, 0, 1, 0), e(B, port, 1, 1)]);
                }
                None => {
                    children.push(leaf("const", vec![frozen("value", status), frozen("type", "Number")])?);
                    children.push(leaf("with_field", vec![frozen("name", "status")])?);
                    edges.extend([e(0, 0, 2, 0), e(1, 0, 2, 1)]);
                }
            }
            // Input 0, the request, is deliberately left unread (see the module doc). The envelope
            // grows field by field: status, the body when there is one, the headers when asked.
            let mut last = children.len() as u32 - 1;
            if has_body {
                children.push(leaf("with_field", vec![frozen("name", "body")])?);
                let k = children.len() as u32 - 1;
                edges.extend([e(last, 0, k, 0), e(B, 1, k, 1)]);
                last = k;
            }
            if with_headers {
                children.push(leaf("with_field", vec![frozen("name", "headers")])?);
                let k = children.len() as u32 - 1;
                let port = if has_body { 2 } else { 1 };
                edges.extend([e(last, 0, k, 0), e(B, port, k, 1)]);
                last = k;
            }
            if has_body && !missing.is_empty() {
                // No body, no envelope: `default` stands in the error envelope (it fires on an
                // absent input).
                let error = if m == 404 { "not found" } else { "no value to answer with" };
                let fallback = format!(r#"{{"status":{m},"body":{{"error":"{error}"}}}}"#);
                children.push(leaf("default", vec![frozen("value", &fallback), frozen("type", "Json")])?);
                let k = children.len() as u32 - 1;
                edges.extend([e(last, 0, k, 0)]);
                last = k;
            }
            edges.push(e(last, 0, B, 0));
            Ok(composite(children, edges, inputs, outputs))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_core::Resolver;

    fn names(ps: &[Port]) -> Vec<&str> {
        ps.iter().map(|p| p.name.as_str()).collect()
    }
    fn parts(u: &Unit) -> (&[Hash], &[Edge]) {
        match &u.body {
            UnitBody::Composite { children, edges } => (children, edges),
            UnitBody::Primitive { .. } | UnitBody::Module { .. } => (&[], &[]),
        }
    }
    fn prim_name(store: &MapResolver, h: &Hash) -> String {
        match &store.resolve(h).unwrap().body {
            UnitBody::Primitive { name } => name.clone(),
            UnitBody::Composite { .. } | UnitBody::Module { .. } => String::new(),
        }
    }

    #[test]
    fn request_splits_the_record_and_passes_it_through() {
        let mut store = MapResolver::new();
        let u = build("request", &defaults("request").unwrap(), &mut store).unwrap();
        assert_eq!(names(&u.outputs), ["request", "params", "query", "headers", "body", "json"]);
        assert!(parts(&u).1.contains(&e(BOUNDARY, 0, BOUNDARY, 0)));
        assert!(u.params.is_empty(), "the route lives on the endpoint, not in the handler's identity");
    }

    #[test]
    fn respond_with_and_without_a_body_and_not_a_bad_status() {
        let mut store = MapResolver::new();
        let f = WParam::frozen;
        assert_eq!(names(&build("respond", &[f("status", "201")], &mut store).unwrap().inputs), ["request", "body"]);
        assert_eq!(names(&build("respond", &[f("status", "204")], &mut store).unwrap().inputs), ["request"]);
        // `headers: true` adds a third input (a second, for a 204), and nothing else changes: the
        // respond without it is the unit it always was.
        assert_eq!(names(&build("respond", &[f("status", "200"), f("missing", "404"), f("headers", "true")], &mut store).unwrap().inputs), ["request", "body", "headers"]);
        assert_eq!(names(&build("respond", &[f("status", "204"), f("headers", "true")], &mut store).unwrap().inputs), ["request", "headers"]);
        let plain = build("respond", &[f("status", "200"), f("missing", "404")], &mut store).unwrap();
        let no_flag = build("respond", &[f("status", "200"), f("missing", "404"), f("headers", "false")], &mut store).unwrap();
        assert_eq!(plain, no_flag);
        // `status_input: true` adds the status as the last input, in place of the param's const;
        // without it (or false), the unit it always was.
        let wired = build("respond", &[f("status", "200"), f("missing", "404"), f("headers", "true"), f("status_input", "true")], &mut store).unwrap();
        assert_eq!(names(&wired.inputs), ["request", "body", "headers", "status"]);
        let (children, edges) = parts(&wired);
        assert!(children.iter().all(|c| prim_name(&store, c) != "const"), "the status is the input's, not a const");
        assert!(edges.contains(&e(BOUNDARY, 3, 1, 1)), "the status input feeds the first with_field");
        let unflagged = build("respond", &[f("status", "200"), f("missing", "404"), f("headers", "false"), f("status_input", "false")], &mut store).unwrap();
        assert_eq!(plain, unflagged);
        let with = build("respond", &[f("status", "303"), f("missing", "400"), f("headers", "true")], &mut store).unwrap();
        let (children, _) = parts(&with);
        assert_eq!(children.len(), 6, "record, status, with_field status, with_field body, with_field headers, the fallback");
        for bad in ["99", "2xx", "600", "2000"] {
            assert!(build("respond", &[f("status", bad)], &mut store).unwrap_err().contains("not an HTTP status"), "{bad}");
        }
        let err = build("respond", &[f("status", "200"), f("missing", "200")], &mut store).unwrap_err();
        assert!(err.contains("not an error status"));
    }

    #[test]
    fn respond_ends_in_a_default_answering_the_missing_status() {
        let mut store = MapResolver::new();
        let f = WParam::frozen;
        let u = build("respond", &[f("status", "200"), f("missing", "404")], &mut store).unwrap();
        let (children, edges) = parts(&u);
        assert_eq!(prim_name(&store, &children[4]), "default");
        assert_eq!(
            store.resolve(&children[4]).unwrap().params,
            [frozen("value", r#"{"status":404,"body":{"error":"not found"}}"#), frozen("type", "Json")]
        );
        assert!(edges.contains(&e(3, 0, 4, 0)));
        let out: Vec<_> = edges.iter().filter(|x| x.to_child == BOUNDARY).cloned().collect();
        assert_eq!(out, [e(4, 0, BOUNDARY, 0)]);
        // Without the param (an older node), or for a 204 (no body to miss), no fallback.
        for params in [vec![f("status", "200")], vec![f("status", "204"), f("missing", "404")]] {
            let u = build("respond", &params, &mut store).unwrap();
            assert!(parts(&u).0.iter().all(|c| prim_name(&store, c) != "default"));
        }
        assert_eq!(defaults("respond").unwrap(), [f("status", "200"), f("missing", "404"), f("headers", "false"), f("status_input", "false")]);
    }

    #[test]
    fn stripe_verify_refuses_a_tolerance_that_isnt_seconds() {
        let mut store = MapResolver::new();
        for bad in ["0", "", "1234567", "5s"] {
            assert!(build("stripe_verify", &[WParam::frozen("tolerance", bad)], &mut store).is_err(), "{bad}");
        }
        assert!(build("stripe_verify", &defaults("stripe_verify").unwrap(), &mut store).is_ok());
    }

    #[test]
    fn every_template_has_an_interface_defaults_and_words() {
        for t in TEMPLATES {
            let params = defaults(t).unwrap();
            assert!(interface(t, &params).is_some() && describe(t).is_some(), "{t}");
        }
        assert!(interface("add", &[]).is_none() && !is_template("add"));
    }
}
