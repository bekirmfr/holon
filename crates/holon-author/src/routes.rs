//! Deriving a server from a flow: every connected group holding a `request` node is one route,
//! with that node's method and path. The group is grouped through core like any selection, its
//! boundary narrowed to what an endpoint needs ([`route_spec`]), and wrapped as an endpoint unit;
//! the endpoints together make the server unit (flow.rs). The shapes are the ones core's
//! declare.rs reads back, on deploy, in the host.

use std::collections::HashMap;

use holon_core::{parse_path, Edge, EncapSpec, Hash, Param, Port, Segment, Type, Unit, UnitBody, BOUNDARY, METHODS};
use serde::Serialize;

use crate::canvas::{Node, PlacedBody, Wire};
use crate::wire::WType;

/// One route of the server: its request node, and every node wired (directly or not) to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RouteGroup {
    pub method: String,
    pub path: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
    /// The waiting point this route resumes (its request node's `resume`), or "".
    pub resume: String,
}

/// The flow's connected groups of node ids (wires taken in either direction), each in the order
/// `ids` gives, the groups in the order of their first node. Only ids are read, so a replay's
/// nodes (which carry no units) group the same way.
pub fn components(ids: &[&str], wires: &[Wire]) -> Vec<Vec<String>> {
    let index: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut parent: Vec<usize> = (0..ids.len()).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        parent[x] = r;
        r
    }
    for w in wires {
        if let (Some(&s), Some(&t)) = (index.get(w.source.as_str()), index.get(w.target.as_str())) {
            let (rs, rt) = (find(&mut parent, s), find(&mut parent, t));
            parent[rs] = rt;
        }
    }
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut group_of_root: HashMap<usize, usize> = HashMap::new();
    for (i, id) in ids.iter().enumerate() {
        let root = find(&mut parent, i);
        let g = *group_of_root.entry(root).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[g].push(id.to_string());
    }
    groups
}

/// One clock handler of the server: its `every` node, its period, and every node wired to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ClockGroup {
    pub seconds: u64,
    /// `every 2s`: how the log names its runs.
    pub label: String,
    #[serde(rename = "everyId")]
    pub every_id: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

/// One file handler of the server: its `on_file` node, its line, and every node wired to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FileGroup {
    pub folder: String,
    pub deleted: bool,
    pub matching: String,
    /// `on file inbox`: how the log names its runs.
    pub label: String,
    #[serde(rename = "onFileId")]
    pub on_file_id: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

impl FileGroup {
    pub fn trigger(&self) -> holon_core::FileTrigger {
        holon_core::FileTrigger { folder: self.folder.clone(), deleted: self.deleted, matching: self.matching.clone() }
    }
}

/// What a node is the trigger of, when it is one: a route's request, a clock's tick, a file.
fn trigger_label(n: &Node) -> Option<String> {
    if n.is_template("request") {
        Some(format!("{} {}", n.param("method"), n.param("path")))
    } else if n.is_template("every") {
        Some(match crate::templates::period(n.param("seconds")) {
            Ok(s) => holon_core::clock_label(s),
            Err(_) => format!("every {}", n.param("seconds")),
        })
    } else if n.is_template("on_file") {
        Some(match crate::templates::file_trigger(&n.data.params) {
            Ok(t) => t.label(),
            Err(_) => format!("on file {}", n.param("folder")),
        })
    } else {
        None
    }
}

/// The clock handlers the flow declares (each connected group holding an `every` node and no
/// request), and why any can't be one. A group holding two triggers is `find_routes`'s to say.
pub fn find_clocks(nodes: &[Node], wires: &[Wire]) -> (Vec<ClockGroup>, Vec<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let (mut clocks, mut errors): (Vec<ClockGroup>, Vec<String>) = (Vec::new(), Vec::new());
    for ids in components(&node_ids, wires) {
        let triggers: Vec<&Node> = ids.iter().map(|id| by_id[id.as_str()]).filter(|n| trigger_label(n).is_some()).collect();
        let [every] = triggers.as_slice() else { continue };
        if !every.is_template("every") {
            continue;
        }
        let seconds = match crate::templates::period(every.param("seconds")) {
            Ok(s) => s,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let label = holon_core::clock_label(seconds);
        if clocks.iter().any(|c| c.seconds == seconds) {
            errors.push(format!("{label} is written twice: one clock per period"));
            continue;
        }
        clocks.push(ClockGroup { seconds, label, every_id: every.id.clone(), node_ids: ids });
    }
    (clocks, errors)
}

/// The file handlers the flow declares (each connected group holding an `on_file` node and no
/// other trigger), and why any can't be one.
pub fn find_files(nodes: &[Node], wires: &[Wire]) -> (Vec<FileGroup>, Vec<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let (mut files, mut errors): (Vec<FileGroup>, Vec<String>) = (Vec::new(), Vec::new());
    for ids in components(&node_ids, wires) {
        let triggers: Vec<&Node> = ids.iter().map(|id| by_id[id.as_str()]).filter(|n| trigger_label(n).is_some()).collect();
        let [on_file] = triggers.as_slice() else { continue };
        if !on_file.is_template("on_file") {
            continue;
        }
        let t = match crate::templates::file_trigger(&on_file.data.params) {
            Ok(t) => t,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let label = t.label();
        if files.iter().any(|f| f.trigger() == t) {
            errors.push(format!("{label} is written twice: one block per line"));
            continue;
        }
        files.push(FileGroup { folder: t.folder, deleted: t.deleted, matching: t.matching, label, on_file_id: on_file.id.clone(), node_ids: ids });
    }
    // In the order they are written.
    files.sort_by_key(|f| node_ids.iter().position(|id| *id == f.on_file_id));
    (files, errors)
}

/// The routes the flow declares, and why any group can't be one. Groups without a request node
/// are not part of the server (helpers, scratch work) and are skipped silently.
pub fn find_routes(nodes: &[Node], wires: &[Wire]) -> (Vec<RouteGroup>, Vec<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut routes: Vec<RouteGroup> = Vec::new();
    let mut errors = Vec::new();
    let label = |n: &Node| format!("{} {}", n.param("method"), n.param("path"));
    // Two routes clash when they'd match the same requests, the rule core's read_server enforces.
    let shape = |p: &str| parse_path(p).map(|segs| segs.into_iter().map(|s| match s {
        Segment::Literal(l) => Some(l),
        Segment::Param(_) => None,
    }).collect::<Vec<_>>());
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    for ids in components(&node_ids, wires) {
        let requests: Vec<&Node> = ids.iter().map(|id| by_id[id.as_str()]).filter(|n| n.is_template("request")).collect();
        // Two triggers in one group: a node can't belong to two handlers.
        let triggers: Vec<String> = ids.iter().filter_map(|id| trigger_label(by_id[id.as_str()])).collect();
        if triggers.len() > 1 && requests.len() < triggers.len() {
            errors.push(format!("{} are wired together: one trigger per handler", triggers.join(" and ")));
            continue;
        }
        let req = match requests.as_slice() {
            [] => continue,
            [one] => *one,
            many => {
                errors.push(format!(
                    "{} are wired together: one request node per route",
                    many.iter().map(|n| label(n)).collect::<Vec<_>>().join(" and ")
                ));
                continue;
            }
        };
        let (method, path) = (req.param("method"), req.param("path"));
        if !METHODS.contains(&method) {
            errors.push(format!("\"{method}\" is not a method ({})", METHODS.join(", ")));
            continue;
        }
        if parse_path(path).is_none() {
            errors.push(format!("\"{path}\" is not a route path (use /todos or /todos/:id)"));
            continue;
        }
        if let Some(clash) = routes.iter().find(|r| r.method == method && shape(&r.path) == shape(path)) {
            errors.push(format!("{method} {path} clashes with {} {}", clash.method, clash.path));
            continue;
        }
        routes.push(RouteGroup {
            method: method.into(),
            path: path.into(),
            request_id: req.id.clone(),
            node_ids: ids,
            resume: req.param("resume").into(),
        });
    }
    (routes, errors)
}

/// Inputs a node works without, left unwired: core treats them as none (its
/// `has_optional_ports`). An http_request's headers and body; a collection's or folder's listing's
/// prefix and after; a file's lines' first line.
pub(crate) fn is_optional_input(n: &Node, port: Option<&str>) -> bool {
    // A sink's trigger, or a holon's: left open, it runs with the unit around it (core's
    // `open_trigger`). The lowering ties it wherever there is a trigger.
    if port == Some(holon_core::TRIGGER) {
        return true;
    }
    let optional: &[&str] = match n.data.name.as_str() {
        "http_request" => &["headers", "body"],
        "http_upload" => &["headers"],
        "http_stream" => &["headers", "body"],
        "collection_list" | "file_list" => &["prefix", "after"],
        "file_lines" => &["from"],
        _ => &[],
    };
    n.data.body == PlacedBody::Primitive && port.is_some_and(|p| optional.contains(&p))
}

/// Narrow core's proposed boundary for one route's group to what an endpoint needs. The only input
/// left open must be the request node's (the endpoint feeds it); the outputs kept are the respond
/// node's (the response) and every effect (state writes, which the host performs). Anything else
/// dangling is dropped from the endpoint, not an error: an unused `query` output is normal. An
/// input that may be left empty (an `http_request`'s `headers` or `body`) stays unbound, so it is
/// absent when the endpoint runs, rather than refusing the route.
/// `group` is in the same order as the children the spec was proposed over.
pub fn route_spec(spec: EncapSpec, group: &[&Node], label: &str) -> Result<EncapSpec, String> {
    let input_name = |child: u32, port: u32| group[child as usize].data.inputs.get(port as usize).map(|p| p.name.as_str());
    let promote: Vec<_> =
        spec.promote.into_iter().filter(|p| !is_optional_input(group[p.child as usize], input_name(p.child, p.port))).collect();
    for p in &promote {
        let n = group[p.child as usize];
        if !n.is_template("request") {
            let port = input_name(p.child, p.port).map(String::from).unwrap_or_else(|| p.port.to_string());
            return Err(format!("{label}: {} · {port} is not wired", n.data.name));
        }
    }
    let is_respond = |child: u32| group[child as usize].is_template("respond");
    let is_effect =
        |child: u32, port: u32| group[child as usize].data.outputs.get(port as usize).is_some_and(|p| p.ty == WType::Effect);
    let expose: Vec<_> = spec.expose.into_iter().filter(|x| is_respond(x.child) || is_effect(x.child, x.port)).collect();
    let (responses, effects): (Vec<_>, Vec<_>) = expose.into_iter().partition(|x| is_respond(x.child));
    if responses.len() > 1 {
        return Err(format!("{label}: more than one respond node"));
    }
    if responses.is_empty() && effects.is_empty() {
        return Err(format!("{label}: add a respond node (or an effect, answered 204)"));
    }
    // The response first: the endpoint takes the first non-effect output as output 0.
    Ok(EncapSpec { promote, expose: responses.into_iter().chain(effects).collect() })
}

/// Narrow core's proposed boundary for one clock's group to what a clock handler is: no inputs
/// (a tick has none: every input a node needs must be wired) and its effects as outputs.
pub fn clock_spec(spec: EncapSpec, group: &[&Node], label: &str) -> Result<EncapSpec, String> {
    handler_spec(spec, group, label, "a clock")
}

/// A file handler's boundary, as a clock's: no inputs (the file's handle is its `on_file` node's)
/// and its effects as outputs.
pub fn file_spec(spec: EncapSpec, group: &[&Node], label: &str) -> Result<EncapSpec, String> {
    handler_spec(spec, group, label, "a file's run")
}

/// A handler with no caller to answer: no inputs, its effects as outputs. `who` says which kind.
fn handler_spec(spec: EncapSpec, group: &[&Node], label: &str, who: &str) -> Result<EncapSpec, String> {
    let input_name = |child: u32, port: u32| group[child as usize].data.inputs.get(port as usize).map(|p| p.name.as_str());
    let promote: Vec<_> =
        spec.promote.into_iter().filter(|p| !is_optional_input(group[p.child as usize], input_name(p.child, p.port))).collect();
    if let Some(p) = promote.first() {
        let n = group[p.child as usize];
        let port = input_name(p.child, p.port).map(String::from).unwrap_or_else(|| p.port.to_string());
        return Err(format!("{label}: {} · {port} is not wired", n.data.name));
    }
    if let Some(n) = group.iter().find(|n| n.is_template("respond") || n.is_template("request")) {
        return Err(format!("{label}: {} is a route's; {who} answers nobody", n.data.name));
    }
    let is_effect =
        |child: u32, port: u32| group[child as usize].data.outputs.get(port as usize).is_some_and(|p| p.ty == WType::Effect);
    let expose: Vec<_> = spec.expose.into_iter().filter(|x| is_effect(x.child, x.port)).collect();
    if expose.is_empty() {
        return Err(format!("{label}: it does nothing: write something (state, a collection, a channel)"));
    }
    Ok(EncapSpec { promote, expose })
}

/// Wrap a clock's handler as a clock handler unit (core's `read_clock`): no inputs, each of its
/// effects an output, `declare = "every"` and `seconds`.
pub fn clock_unit(handler: Hash, outputs: &[Port], seconds: u64) -> Unit {
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    Unit {
        body: UnitBody::Composite {
            children: vec![handler],
            edges: (0..outputs.len() as u32).map(|k| Edge { from_child: 0, from_port: k, to_child: BOUNDARY, to_port: k }).collect(),
        },
        params: vec![frozen("declare", "every"), frozen("seconds", &seconds.to_string())],
        inputs: vec![],
        outputs: outputs.to_vec(),
    }
}

/// Wrap a file handler's handler as a file handler unit (core's `read_file_handler`): no inputs,
/// each of its effects an output, `declare = "on_file"`, `folder`, and `on` and `matching` when
/// given.
pub fn file_unit(handler: Hash, outputs: &[Port], trigger: &holon_core::FileTrigger) -> Unit {
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    let mut params = vec![frozen("declare", "on_file"), frozen("folder", &trigger.folder)];
    if trigger.deleted {
        params.push(frozen("on", "deleted"));
    }
    if !trigger.matching.is_empty() {
        params.push(frozen("matching", &trigger.matching));
    }
    Unit {
        body: UnitBody::Composite {
            children: vec![handler],
            edges: (0..outputs.len() as u32).map(|k| Edge { from_child: 0, from_port: k, to_child: BOUNDARY, to_port: k }).collect(),
        },
        params,
        inputs: vec![],
        outputs: outputs.to_vec(),
    }
}

/// A waiting point's name: what a `resume` route may name.
fn is_wait_name(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Wrap a handler as an endpoint unit. The endpoint's one input is the request record, wired to
/// the handler's input if it has one (a handler with no input, e.g. a plain listing, simply
/// ignores the request). Output 0 is the response: the handler's first non-effect output, or, if
/// it only has effects, its first effect (which the data plane answers with 204). Every other
/// effect output follows, so the host performs it. An error message when the handler can't be an
/// endpoint.
pub fn endpoint_unit(handler: Hash, inputs: &[Port], outputs: &[Port], method: &str, path: &str, resume: &str) -> Result<Unit, String> {
    if !resume.is_empty() && !is_wait_name(resume) {
        return Err(format!("\"{resume}\" is not a waiting point's name (letters, digits, - and _)"));
    }
    if parse_path(path).is_none() {
        return Err(format!("\"{path}\" is not a route path (use /todos or /todos/:id)"));
    }
    if inputs.len() > 1 {
        return Err("a handler takes at most one input: the request record".into());
    }
    if outputs.is_empty() {
        return Err("a handler needs an output: the response".into());
    }
    let is_effect = |p: &Port| p.ty == Type::Effect;
    let response = outputs.iter().position(|p| !is_effect(p)).unwrap_or(0);
    let order: Vec<usize> =
        std::iter::once(response).chain((0..outputs.len()).filter(|&i| i != response && is_effect(&outputs[i]))).collect();
    let mut edges: Vec<Edge> = Vec::new();
    if inputs.len() == 1 {
        edges.push(Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 });
    }
    edges.extend(order.iter().enumerate().map(|(to, &from)| Edge {
        from_child: 0,
        from_port: from as u32,
        to_child: BOUNDARY,
        to_port: to as u32,
    }));
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    let mut params = vec![frozen("declare", "endpoint"), frozen("method", method), frozen("path", path)];
    // Only when set, so an ordinary route's endpoint hashes as it always did.
    if !resume.is_empty() {
        params.push(frozen("resume", resume));
    }
    Ok(Unit {
        body: UnitBody::Composite { children: vec![handler], edges },
        params,
        inputs: vec![Port { name: "request".into(), ty: Type::Record }],
        outputs: order
            .iter()
            .enumerate()
            .map(|(k, &i)| Port { name: if k == 0 { "response".into() } else { outputs[i].name.clone() }, ty: outputs[i].ty.clone() })
            .collect(),
    })
}

/// A server unit over the given endpoints, clock handlers and file handlers, in that order, with `port` when it
/// has a route. Core's `read_server` re-checks everything on deploy.
pub fn server_unit(handlers: Vec<Hash>, port: Option<u16>) -> Unit {
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    let mut params = vec![frozen("declare", "server")];
    if let Some(port) = port {
        params.push(frozen("port", &port.to_string()));
    }
    Unit { body: UnitBody::Composite { children: handlers, edges: vec![] }, params, inputs: vec![], outputs: vec![] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::NodeData;
    use crate::templates;
    use crate::wire::{WParam, WPort};
    use holon_core::{Expose, Promote};

    fn node(id: &str, name: &str, params: Vec<WParam>, template: bool) -> Node {
        let (inputs, outputs) = if template {
            let i = templates::interface(name, &params).unwrap();
            (i.inputs, i.outputs)
        } else {
            let out = if name == "store_write" { WType::Effect } else { WType::Any };
            (vec![WPort::new("a", WType::Any)], vec![WPort::new("out", out)])
        };
        let body = if template { PlacedBody::Template } else { PlacedBody::Primitive };
        Node::placed(id, Default::default(), NodeData::new(name, body, inputs, outputs, params))
    }
    fn plain(id: &str, name: &str) -> Node {
        node(id, name, vec![], false)
    }
    fn req(id: &str, method: &str, path: &str) -> Node {
        node(id, "request", vec![WParam::frozen("method", method), WParam::frozen("path", path)], true)
    }
    fn res(id: &str) -> Node {
        node(id, "respond", vec![WParam::frozen("status", "200")], true)
    }
    fn wire(source: &str, target: &str) -> Wire {
        Wire { source_handle: None, target_handle: None, ..Wire::new(source, "", target, "") }
    }
    fn promote(child: u32, port: u32, name: &str) -> Promote {
        Promote { child, port, name: name.into() }
    }
    fn expose(child: u32, port: u32, name: &str) -> Expose {
        Expose { child, port, name: name.into() }
    }

    #[test]
    fn components_group_by_wires_either_way_in_node_order() {
        assert_eq!(components(&["a", "b", "c", "d"], &[wire("b", "a"), wire("c", "b")]), [vec!["a", "b", "c"], vec!["d"]]);
    }

    #[test]
    fn one_route_per_group_with_a_request_node() {
        let ns = [req("r1", "GET", "/todos"), res("s1"), req("r2", "POST", "/todos"), res("s2"), plain("helper", "add")];
        let (routes, errors) = find_routes(&ns, &[wire("r1", "s1"), wire("r2", "s2")]);
        assert!(errors.is_empty());
        let got: Vec<_> =
            routes.iter().map(|r| (r.method.as_str(), r.path.as_str(), r.request_id.as_str(), r.node_ids.join(","))).collect();
        assert_eq!(got, [("GET", "/todos", "r1", "r1,s1".to_string()), ("POST", "/todos", "r2", "r2,s2".to_string())]);
    }

    #[test]
    fn every_group_that_cant_be_a_route_says_why() {
        let ns = [
            req("a", "GET", "/x"),
            req("b", "GET", "/y"), // wired together
            req("c", "FETCH", "/z"),
            req("d", "GET", "no-slash"),
            req("e", "GET", "/t/:id"),
            req("f", "GET", "/t/:key"), // clash
        ];
        let (routes, errors) = find_routes(&ns, &[wire("a", "b")]);
        assert_eq!(routes.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["/t/:id"]);
        assert_eq!(
            errors,
            [
                "GET /x and GET /y are wired together: one request node per route",
                "\"FETCH\" is not a method (GET, POST, PUT, PATCH, DELETE)",
                "\"no-slash\" is not a route path (use /todos or /todos/:id)",
                "GET /t/:key clashes with GET /t/:id",
            ]
        );
    }

    #[test]
    fn a_route_keeps_the_response_first_and_every_effect() {
        let g = [req("r", "POST", "/todos"), plain("f", "field"), plain("w", "store_write"), res("s")];
        let group: Vec<&Node> = g.iter().collect();
        let spec = EncapSpec {
            promote: vec![promote(0, 0, "request")],
            expose: vec![expose(0, 2, "query"), expose(2, 0, "write"), expose(3, 0, "response")],
        };
        let got = route_spec(spec, &group, "POST /todos").unwrap();
        assert_eq!(got.promote, [promote(0, 0, "request")]);
        assert_eq!(got.expose, [expose(3, 0, "response"), expose(2, 0, "write")]);

        let base = || EncapSpec { promote: vec![promote(0, 0, "request")], expose: vec![] };
        let unwired = EncapSpec { promote: vec![promote(0, 0, "request"), promote(1, 0, "a")], ..base() };
        assert_eq!(route_spec(unwired, &group, "POST /todos").unwrap_err(), "POST /todos: field · a is not wired");
        let two = EncapSpec { expose: vec![expose(3, 0, "r1"), expose(3, 0, "r2")], ..base() };
        assert_eq!(route_spec(two, &group, "POST /todos").unwrap_err(), "POST /todos: more than one respond node");
        assert_eq!(
            route_spec(base(), &group, "POST /todos").unwrap_err(),
            "POST /todos: add a respond node (or an effect, answered 204)"
        );
    }

    #[test]
    fn an_http_requests_optional_inputs_stay_unbound_but_not_its_path() {
        let mut http = plain("h", "http_request");
        http.data.inputs = ["path", "headers", "body"].iter().map(|n| WPort::new(n, WType::Any)).collect();
        let g = [req("r", "GET", "/fetch"), http, res("s")];
        let group: Vec<&Node> = g.iter().collect();
        let spec = || EncapSpec {
            promote: vec![promote(0, 0, "request"), promote(1, 1, "headers"), promote(1, 2, "body")],
            expose: vec![expose(2, 0, "response")],
        };
        assert_eq!(route_spec(spec(), &group, "GET /fetch").unwrap().promote, [promote(0, 0, "request")]);
        let mut unwired = spec();
        unwired.promote.push(promote(1, 0, "path"));
        assert_eq!(route_spec(unwired, &group, "GET /fetch").unwrap_err(), "GET /fetch: http_request · path is not wired");
    }

    fn port(name: &str, ty: Type) -> Port {
        Port { name: name.into(), ty }
    }
    fn edge(from_child: u32, from_port: u32, to_child: u32, to_port: u32) -> Edge {
        Edge { from_child, from_port, to_child, to_port }
    }

    #[test]
    fn an_endpoint_puts_the_response_first_and_wires_the_request_in() {
        let h = [7u8; 32];
        let outputs = [port("write", Type::Effect), port("item", Type::Any), port("log", Type::Effect)];
        let u = endpoint_unit(h, &[port("record", Type::Record)], &outputs, "POST", "/todos", "").unwrap();
        let got: Vec<_> = u.outputs.iter().map(|p| (p.name.as_str(), p.ty.clone())).collect();
        assert_eq!(got, [("response", Type::Any), ("write", Type::Effect), ("log", Type::Effect)]);
        let b = BOUNDARY;
        let edges = vec![edge(b, 0, 0, 0), edge(0, 1, b, 0), edge(0, 0, b, 1), edge(0, 2, b, 2)];
        assert_eq!(u.body, UnitBody::Composite { children: vec![h], edges });
        assert_eq!(u.inputs, [port("request", Type::Record)]);
        let values: Vec<_> = u
            .params
            .iter()
            .map(|p| match p {
                Param::Frozen { value, .. } => value.as_str(),
                Param::Open { .. } => "",
            })
            .collect();
        assert_eq!(values, ["endpoint", "POST", "/todos"]);
    }

    #[test]
    fn a_handler_may_ignore_the_request_and_some_cant_be_endpoints() {
        let h = [7u8; 32];
        let listing = endpoint_unit(h, &[], &[port("out", Type::Any)], "GET", "/todos", "").unwrap();
        assert!(matches!(&listing.body, UnitBody::Composite { edges, .. } if edges.len() == 1));
        let two_inputs = [port("a", Type::Any), port("b", Type::Any)];
        assert!(endpoint_unit(h, &two_inputs, &[port("o", Type::Any)], "GET", "/x", "").unwrap_err().contains("at most one input"));
        assert!(endpoint_unit(h, &[], &[], "GET", "/x", "").unwrap_err().contains("needs an output"));
        assert!(endpoint_unit(h, &[], &[port("o", Type::Any)], "GET", "x", "").unwrap_err().contains("not a route path"));
    }

    #[test]
    fn a_resume_route_names_its_waiting_point_only_when_it_has_one() {
        let h = [7u8; 32];
        let out = [port("o", Type::Any)];
        let plain = endpoint_unit(h, &[], &out, "GET", "/paid/:token", "").unwrap();
        let resume = endpoint_unit(h, &[], &out, "GET", "/paid/:token", "paid").unwrap();
        assert_eq!(plain.params.len(), 3);
        assert_eq!(resume.params.last(), Some(&Param::Frozen { name: "resume".into(), value: "paid".into() }));
        assert!(endpoint_unit(h, &[], &out, "GET", "/x", "a b").unwrap_err().contains("not a waiting point's name"));
    }

    #[test]
    fn a_server_is_a_wireless_composite_of_endpoints_with_its_port() {
        let u = server_unit(vec![[1; 32], [2; 32]], Some(8080));
        assert_eq!(u.body, UnitBody::Composite { children: vec![[1; 32], [2; 32]], edges: vec![] });
        let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
        assert_eq!(u.params, [frozen("declare", "server"), frozen("port", "8080")]);
        assert!(u.inputs.is_empty() && u.outputs.is_empty());
    }
}
