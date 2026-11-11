//! A flow's units: each node's own unit, the server its routes derive into, and the one unit the
//! flow deploys as. Each reads what core computes (the proposed boundary, the frozen composite)
//! rather than re-deciding it.

use std::collections::{BTreeMap, HashMap};

use holon_core::{
    canonical_child_order, default_spec, derive_and_freeze, parse_hash, Edge, Hash, MapResolver, Unit, UnitBody,
};
use serde::Serialize;

use crate::canvas::{Node, PlacedBody, Wire};
use crate::routes::{with_log,
    action_spec, action_unit, clock_spec, clock_unit, endpoint_unit, feed_spec, feed_unit, file_spec, file_unit, find_actions, find_clocks, find_feeds, find_files, find_mails, find_queues, find_routes, find_websockets, is_optional_input, mail_spec, mail_unit, queue_spec, queue_unit, route_spec, socket_spec, socket_unit,
    server_unit,
};
use crate::wire::WPort;
use crate::{templates, to_hex};

fn put(store: &mut MapResolver, u: Unit) -> Result<Hash, String> {
    store.insert(u).map_err(|e| format!("{e}"))
}

/// A node's own unit, stored, by hash. A primitive is rebuilt from its name, params and ports (the
/// same primitive always hashes the same, so rebuilding is referencing); a template from its
/// params; a grouped composite was frozen when it was grouped, so it is its hash (which `store`
/// must already hold to be used).
pub fn unit_for(node: &Node, store: &mut MapResolver) -> Result<Hash, String> {
    match &node.data.body {
        // A composite was frozen when grouped, a module stored when added: each is its hash.
        PlacedBody::Composite { hash } | PlacedBody::Module { hash } => parse_hash(hash).ok_or_else(|| format!("{}: bad hash {hash}", node.data.name)),
        PlacedBody::Template => {
            let unit = templates::build(&node.data.name, &node.data.params, store).map_err(|e| format!("{}: {e}", node.data.name))?;
            put(store, unit)
        }
        PlacedBody::Primitive => {
            let ports = |ps: &[WPort]| ps.iter().cloned().map(WPort::into_core).collect::<Result<Vec<_>, _>>();
            let unit = Unit {
                body: UnitBody::Primitive { name: node.data.name.clone() },
                params: node.data.params.iter().cloned().map(|p| p.into_core()).collect(),
                inputs: ports(&node.data.inputs)?,
                outputs: ports(&node.data.outputs)?,
            };
            put(store, unit)
        }
    }
}

/// A group of nodes as core children (each node's hash) plus the wires among them, by index. A
/// wire with an end outside the group is left out: from the group's side it is an open input or
/// output.
pub fn group_of(group: &[&Node], wires: &[Wire], store: &mut MapResolver) -> Result<(Vec<Hash>, Vec<Edge>), String> {
    let index: HashMap<&str, usize> = group.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let children = group.iter().map(|n| unit_for(n, store)).collect::<Result<Vec<_>, _>>()?;
    let mut edges = Vec::new();
    for w in wires {
        let (Some(&from), Some(&to)) = (index.get(w.source.as_str()), index.get(w.target.as_str())) else { continue };
        let port = |ports: &[WPort], handle: &Option<String>, side: &str, node: &Node| {
            let h = handle.as_deref().unwrap_or("");
            ports.iter().position(|p| p.name == h).map(|i| i as u32).ok_or_else(|| format!("{} has no {side} \"{h}\"", node.data.name))
        };
        edges.push(Edge {
            from_child: from as u32,
            from_port: port(&group[from].data.outputs, &w.source_handle, "output", group[from])?,
            to_child: to as u32,
            to_port: port(&group[to].data.inputs, &w.target_handle, "input", group[to])?,
        });
    }
    Ok((children, edges))
}

/// Which node each child of a frozen composite is, in the order the unit is stored (and run, and
/// timed) in: core sorts a composite's children canonically when it hashes it.
pub fn trace_of(unit: &Unit, group: &[&Node]) -> Vec<String> {
    match &unit.body {
        UnitBody::Composite { children, edges } => canonical_child_order(children, edges)
            .map(|order| order.into_iter().filter_map(|i| group.get(i).map(|n| n.id.clone())).collect())
            .unwrap_or_default(),
        UnitBody::Primitive { .. } | UnitBody::Module { .. } => vec![],
    }
}

/// One endpoint of the derived server: its route, and the endpoint unit's hash.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct EndpointDraft {
    pub method: String,
    pub path: String,
    pub hash: String,
    /// How its runs are recorded: `lazy`, `unrecorded`, or "" (strict).
    pub log: String,
}

/// One clock handler of the derived server: its period, its label (`every 2s`), and the clock
/// handler unit's hash.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ClockDraft {
    pub seconds: u64,
    pub label: String,
    pub hash: String,
    /// How its runs are recorded: `lazy`, `unrecorded`, or "" (strict).
    pub log: String,
}

/// One file handler: its line, the folder it watches, and its unit's hash.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FileDraft {
    pub folder: String,
    pub label: String,
    pub hash: String,
    /// How its runs are recorded: `lazy`, `unrecorded`, or "" (strict).
    pub log: String,
}

/// One mail handler: its mail, its label (`on mail remit`), and its unit's hash.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct MailDraft {
    pub mail: String,
    pub label: String,
    pub hash: String,
    /// How its runs are recorded: `lazy`, `unrecorded`, or "" (strict).
    pub log: String,
}

/// One feed handler: its feed, its label (`on feed prices`), and its unit's hash.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FeedDraft {
    pub feed: String,
    pub label: String,
    pub hash: String,
    /// How its runs are recorded: `lazy`, `unrecorded`, or "" (strict).
    pub log: String,
}

/// One queue handler: its queue, its label (`on queue payouts`), and its unit's hash.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct QueueDraft {
    pub queue: String,
    pub label: String,
    pub hash: String,
    /// How its runs are recorded: `lazy`, `unrecorded`, or "" (strict).
    pub log: String,
}

/// One websocket block: its path, what it runs on, its label (`WebSocket /room/:name message`), and
/// its unit's hash.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SocketDraft {
    pub path: String,
    pub on: String,
    pub label: String,
    pub hash: String,
    /// How its runs are recorded: `lazy`, `unrecorded`, or "" (strict).
    pub log: String,
}

/// One action: its name, its label (`action reset`), and its unit's hash.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ActionDraft {
    pub name: String,
    pub label: String,
    pub hash: String,
    /// How its runs are recorded: `lazy`, `unrecorded`, or "" (strict).
    pub log: String,
}

/// The server a flow declares: its endpoints, clocks, file, mail, feed and queue handlers and actions, why any handler can't be one, the server
/// unit's hash once every handler is sound (and the port is a port, with a route), and, for each
/// handler by its label, which node each of its stored children is.
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct DerivedServer {
    pub endpoints: Vec<EndpointDraft>,
    pub clocks: Vec<ClockDraft>,
    pub files: Vec<FileDraft>,
    pub mails: Vec<MailDraft>,
    pub feeds: Vec<FeedDraft>,
    pub queues: Vec<QueueDraft>,
    pub websockets: Vec<SocketDraft>,
    pub actions: Vec<ActionDraft>,
    pub errors: Vec<String>,
    #[serde(rename = "serverHash")]
    pub server_hash: Option<String>,
    pub trace: BTreeMap<String, Vec<String>>,
}

/// A port as a flow's settings hold it: an integer 1–65535.
fn port_of(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|&p| p >= 1)
}

/// Derive the server from the flow: each connected group holding a request node is one route
/// (routes.rs). Each group is grouped through core like a selection, narrowed to the request input
/// and the response and effect outputs, and wrapped as an endpoint. Every unit made is stored.
pub fn derive_server(nodes: &[Node], wires: &[Wire], store: &mut MapResolver, port: &str, tags: &[String]) -> DerivedServer {
    let (routes, mut errors) = find_routes(nodes, wires);
    let (socket_groups, socket_errors) = find_websockets(nodes, wires, &routes);
    let approval = |name: &str| nodes.iter().any(|n| n.data.name == "approval" && n.param("name") == name);
    for n in nodes.iter().filter(|n| n.data.name == "resume_token" && approval(n.param("await"))) {
        errors.push(format!("resume_token({:?}): {:?} is an approval, which people decide in Holon: it has no token", n.param("await"), n.param("await")));
    }
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut endpoints = Vec::new();
    let mut trace = BTreeMap::new();
    for r in routes {
        let label = format!("{} {}", r.method, r.path);
        // An approval is people's to decide, in Holon: no route delivers it, and no token is made
        // for it (below).
        if !r.resume.is_empty() && approval(&r.resume) {
            errors.push(format!("{label}: \"{}\" is an approval, which people decide in Holon: no route resumes it", r.resume));
            continue;
        }
        let group: Vec<&Node> = r.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let endpoint = (|| -> Result<Result<Hash, String>, String> {
            let (children, edges) = group_of(&group, wires, store)?;
            let proposed = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
            let spec = match route_spec(proposed, &group, &label, store) {
                Ok(spec) => spec,
                // Already says which route.
                Err(e) => return Ok(Err(e)),
            };
            let handler = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
            trace.insert(label.clone(), trace_of(&handler, &group));
            let (inputs, outputs) = (handler.inputs.clone(), handler.outputs.clone());
            let handler = put(store, handler)?;
            let unit = with_log(endpoint_unit(handler, &inputs, &outputs, &r.method, &r.path, &r.resume)?, &r.log)?;
            Ok(Ok(put(store, unit)?))
        })();
        match endpoint {
            Ok(Ok(hash)) => endpoints.push(EndpointDraft { method: r.method, path: r.path, hash: to_hex(&hash), log: r.log }),
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("{label}: {e}")),
        }
    }
    let (groups, clock_errors) = find_clocks(nodes, wires);
    errors.extend(clock_errors);
    let mut clocks = Vec::new();
    for c in groups {
        let group: Vec<&Node> = c.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let clock = (|| -> Result<Result<Hash, String>, String> {
            let (children, edges) = group_of(&group, wires, store)?;
            let proposed = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
            let spec = match clock_spec(proposed, &group, &c.label, store) {
                Ok(spec) => spec,
                Err(e) => return Ok(Err(e)),
            };
            let handler = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
            trace.insert(c.label.clone(), trace_of(&handler, &group));
            let outputs = handler.outputs.clone();
            let handler = put(store, handler)?;
            let unit = with_log(clock_unit(handler, &outputs, c.seconds), &c.log)?;
            Ok(Ok(put(store, unit)?))
        })();
        match clock {
            Ok(Ok(hash)) => clocks.push(ClockDraft { seconds: c.seconds, label: c.label, hash: to_hex(&hash), log: c.log }),
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("{}: {e}", c.label)),
        }
    }
    let (groups, file_errors) = find_files(nodes, wires);
    errors.extend(file_errors);
    let mut files = Vec::new();
    for f in groups {
        let group: Vec<&Node> = f.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let file = (|| -> Result<Result<Hash, String>, String> {
            let (children, edges) = group_of(&group, wires, store)?;
            let proposed = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
            let spec = match file_spec(proposed, &group, &f.label, store) {
                Ok(spec) => spec,
                Err(e) => return Ok(Err(e)),
            };
            let handler = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
            trace.insert(f.label.clone(), trace_of(&handler, &group));
            let outputs = handler.outputs.clone();
            let handler = put(store, handler)?;
            let unit = with_log(file_unit(handler, &outputs, &f.trigger()), &f.log)?;
            Ok(Ok(put(store, unit)?))
        })();
        match file {
            Ok(Ok(hash)) => files.push(FileDraft { folder: f.folder, label: f.label, hash: to_hex(&hash), log: f.log }),
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("{}: {e}", f.label)),
        }
    }
    let (groups, mail_errors) = find_mails(nodes, wires);
    errors.extend(mail_errors);
    let mut mails = Vec::new();
    for m in groups {
        let group: Vec<&Node> = m.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let mail = (|| -> Result<Result<Hash, String>, String> {
            let (children, edges) = group_of(&group, wires, store)?;
            let proposed = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
            let spec = match mail_spec(proposed, &group, &m.label, store) {
                Ok(spec) => spec,
                Err(e) => return Ok(Err(e)),
            };
            let handler = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
            trace.insert(m.label.clone(), trace_of(&handler, &group));
            let outputs = handler.outputs.clone();
            let handler = put(store, handler)?;
            let unit = with_log(mail_unit(handler, &outputs, &m.mail), &m.log)?;
            Ok(Ok(put(store, unit)?))
        })();
        match mail {
            Ok(Ok(hash)) => mails.push(MailDraft { mail: m.mail, label: m.label, hash: to_hex(&hash), log: m.log }),
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("{}: {e}", m.label)),
        }
    }
    let (groups, feed_errors) = find_feeds(nodes, wires);
    errors.extend(feed_errors);
    let mut feeds = Vec::new();
    for g in groups {
        let group: Vec<&Node> = g.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let feed = (|| -> Result<Result<Hash, String>, String> {
            let (children, edges) = group_of(&group, wires, store)?;
            let proposed = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
            let spec = match feed_spec(proposed, &group, &g.label, store) {
                Ok(spec) => spec,
                Err(e) => return Ok(Err(e)),
            };
            let handler = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
            trace.insert(g.label.clone(), trace_of(&handler, &group));
            let outputs = handler.outputs.clone();
            let handler = put(store, handler)?;
            let unit = with_log(feed_unit(handler, &outputs, &g.feed), &g.log)?;
            Ok(Ok(put(store, unit)?))
        })();
        match feed {
            Ok(Ok(hash)) => feeds.push(FeedDraft { feed: g.feed, label: g.label, hash: to_hex(&hash), log: g.log }),
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("{}: {e}", g.label)),
        }
    }
    let (groups, queue_errors) = find_queues(nodes, wires);
    errors.extend(queue_errors);
    let mut queues = Vec::new();
    for g in groups {
        let group: Vec<&Node> = g.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let queue = (|| -> Result<Result<Hash, String>, String> {
            let (children, edges) = group_of(&group, wires, store)?;
            let proposed = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
            let spec = match queue_spec(proposed, &group, &g.label, store) {
                Ok(spec) => spec,
                Err(e) => return Ok(Err(e)),
            };
            let handler = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
            trace.insert(g.label.clone(), trace_of(&handler, &group));
            let outputs = handler.outputs.clone();
            let handler = put(store, handler)?;
            let unit = with_log(queue_unit(handler, &outputs, &g.queue), &g.log)?;
            Ok(Ok(put(store, unit)?))
        })();
        match queue {
            Ok(Ok(hash)) => queues.push(QueueDraft { queue: g.queue, label: g.label, hash: to_hex(&hash), log: g.log }),
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("{}: {e}", g.label)),
        }
    }
    errors.extend(socket_errors);
    let mut websockets = Vec::new();
    for g in socket_groups {
        let group: Vec<&Node> = g.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let socket = (|| -> Result<Result<Hash, String>, String> {
            let (children, edges) = group_of(&group, wires, store)?;
            let proposed = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
            let spec = match socket_spec(proposed, &group, &g.label, store) {
                Ok(spec) => spec,
                Err(e) => return Ok(Err(e)),
            };
            let handler = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
            trace.insert(g.label.clone(), trace_of(&handler, &group));
            let (inputs, outputs) = (handler.inputs.clone(), handler.outputs.clone());
            let handler = put(store, handler)?;
            let unit = with_log(socket_unit(handler, &inputs, &outputs, &g.path, &g.on)?, &g.log)?;
            Ok(Ok(put(store, unit)?))
        })();
        match socket {
            Ok(Ok(hash)) => websockets.push(SocketDraft { path: g.path, on: g.on, label: g.label, hash: to_hex(&hash), log: g.log }),
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("{}: {e}", g.label)),
        }
    }
    let (groups, action_errors) = find_actions(nodes, wires);
    errors.extend(action_errors);
    let mut actions = Vec::new();
    for a in groups {
        let group: Vec<&Node> = a.node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let action = (|| -> Result<Result<Hash, String>, String> {
            let (children, edges) = group_of(&group, wires, store)?;
            let proposed = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
            let spec = match action_spec(proposed, &group, &a.label, store) {
                Ok(spec) => spec,
                Err(e) => return Ok(Err(e)),
            };
            let handler = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
            trace.insert(a.label.clone(), trace_of(&handler, &group));
            let (inputs, outputs) = (handler.inputs.clone(), handler.outputs.clone());
            let handler = put(store, handler)?;
            let unit = with_log(action_unit(handler, &inputs, &outputs, &a.name)?, &a.log)?;
            Ok(Ok(put(store, unit)?))
        })();
        match action {
            Ok(Ok(hash)) => actions.push(ActionDraft { name: a.name, label: a.label, hash: to_hex(&hash), log: a.log }),
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("{}: {e}", a.label)),
        }
    }
    // The keys the flow taints, as its handlers' triggers carry them: one taints it for every run.
    let taint = crate::routes::flow_taint(nodes);
    if let Some(bad) = taint.iter().find(|k| !holon_core::is_taint_key(k)) {
        errors.push(format!("\"{bad}\" isn't a key to taint: a name of up to 128 characters, no spaces"));
    }
    if let Some(bad) = tags.iter().find(|t| !holon_core::is_tag(t)) {
        errors.push(format!("\"{bad}\" isn't a tag: lowercase letters, digits and -, up to 40 of them"));
    }
    if tags.windows(2).any(|w| w[0] >= w[1]) {
        errors.push("a flow's tags are sorted, once each".into());
    }
    // A port only with a route or a websocket: a flow of clocks, files, mails, feeds, queues and
    // actions listens on none.
    let port = if endpoints.is_empty() && websockets.is_empty() { Some(None) } else { port_of(port).map(Some) };
    let server_hash = match port {
        Some(port) if !(endpoints.is_empty() && clocks.is_empty() && files.is_empty() && mails.is_empty() && feeds.is_empty() && queues.is_empty() && websockets.is_empty() && actions.is_empty()) && errors.is_empty() => {
            let hashes = endpoints
                .iter()
                .map(|e| &e.hash)
                .chain(clocks.iter().map(|c| &c.hash))
                .chain(files.iter().map(|f| &f.hash))
                .chain(mails.iter().map(|m| &m.hash))
                .chain(feeds.iter().map(|f| &f.hash))
                .chain(queues.iter().map(|q| &q.hash))
                .chain(websockets.iter().map(|w| &w.hash))
                .chain(actions.iter().map(|a| &a.hash))
                .map(|h| parse_hash(h).expect("hashes made here"))
                .collect();
            put(store, server_unit(hashes, port, &taint, tags)).ok().map(|h| to_hex(&h))
        }
        _ => None,
    };
    DerivedServer { endpoints, clocks, files, mails, feeds, queues, websockets, actions, errors, server_hash, trace }
}

/// What each canvas node does with a secret, for the canvas to mark it: by node id, a line for each
/// sink a secret reaches there (core's `SecretFed`): a request it is sent in, a write or a call the
/// host refuses, a route's answer. A holon placed on the canvas is marked for what its insides do.
/// Nothing for a canvas that doesn't derive.
pub fn secret_marks(nodes: &[Node], wires: &[Wire], store: &mut MapResolver, port: &str, tags: &[String]) -> BTreeMap<String, Vec<String>> {
    use holon_core::Resolver as _;
    let derived = derive_server(nodes, wires, store, port, tags);
    // Each handler (a route's, a clock's, a file's, a message's, a websocket block's, an action's: the
    // composite of its nodes, child 0 of what wraps it), the canvas node each of its children is,
    // and whether its first output answers a caller (a route's, a websocket's open and message
    // blocks'; an action's answer goes back through Holon, withheld).
    let labelled = derived
        .endpoints
        .iter()
        .map(|e| (&e.hash, format!("{} {}", e.method, e.path), true))
        .chain(derived.clocks.iter().map(|c| (&c.hash, c.label.clone(), false)))
        .chain(derived.files.iter().map(|f| (&f.hash, f.label.clone(), false)))
        .chain(derived.mails.iter().map(|m| (&m.hash, m.label.clone(), false)))
        .chain(derived.feeds.iter().map(|f| (&f.hash, f.label.clone(), false)))
        .chain(derived.queues.iter().map(|q| (&q.hash, q.label.clone(), false)))
        .chain(derived.websockets.iter().map(|w| (&w.hash, w.label.clone(), w.on != "close")))
        .chain(derived.actions.iter().map(|a| (&a.hash, a.label.clone(), false)));
    let mut handlers: Vec<(Unit, Vec<String>, bool)> = Vec::new();
    for (hash, label, answers) in labelled {
        let handler = parse_hash(hash).and_then(|h| store.resolve(&h)).and_then(|wrapper| match &wrapper.body {
            UnitBody::Composite { children, .. } => children.first().and_then(|c| store.resolve(c)).cloned(),
            _ => None,
        });
        if let (Some(handler), Some(trace)) = (handler, derived.trace.get(&label)) {
            let nodes = nodes_of(&handler, trace);
            handlers.push((handler, nodes, answers));
        }
    }
    // A flow without handlers runs whole.
    if derived.endpoints.is_empty() && derived.clocks.is_empty() && derived.files.is_empty() && derived.mails.is_empty() && derived.feeds.is_empty() && derived.queues.is_empty() && derived.websockets.is_empty() && derived.actions.is_empty() && derived.errors.is_empty() && !nodes.is_empty() {
        let whole = flow_unit(nodes, wires, store, port, tags).ok().and_then(|h| store.resolve(&h)).cloned();
        if let Some(whole) = whole {
            let all: Vec<&Node> = nodes.iter().collect();
            let trace = trace_of(&whole, &all);
            let nodes = nodes_of(&whole, &trace);
            handlers.push((whole, nodes, false));
        }
    }
    let mut marks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut mark = |nodes: &[String], child: u32, line: String| {
        if let Some(id) = nodes.get(child as usize).filter(|id| !id.is_empty()) {
            let lines = marks.entry(id.clone()).or_default();
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
    };
    for (handler, trace, answers) in &handlers {
        // A route's answer: the first output that isn't an effect.
        let response = handler.outputs.iter().position(|p| p.ty != holon_core::Type::Effect).unwrap_or(0) as u32;
        for cap in holon_core::capabilities(handler, &*store).iter() {
            let holon_core::Capability::SecretRead(name) = cap else { continue };
            let fed = holon_core::taint::SecretFed::of_secret(handler, &*store, &name);
            for (path, sink) in fed.sinks() {
                if let Some(&child) = path.first() {
                    mark(trace, child, crate::describe::describe_secret_sink(&name, sink));
                }
            }
            // What is answered to it: the token a token exchange answers, an API's echo.
            for (path, sink) in holon_core::taint::SecretFed::answers_of(handler, &*store, &name).sinks() {
                if let Some(&child) = path.first() {
                    mark(trace, child, crate::describe::describe_answer_sink(&name, sink));
                }
            }
            if let (true, UnitBody::Composite { edges, .. }) = (*answers, &handler.body) {
                for e in edges.iter().filter(|e| e.to_child == holon_core::BOUNDARY && e.to_port == response && e.from_child != holon_core::BOUNDARY) {
                    if fed.output(&[e.from_child], e.from_port as usize) > holon_core::taint::Taint::Clean {
                        mark(trace, e.from_child, crate::describe::describe_secret_answer(&name));
                    }
                }
            }
        }
    }
    marks
}

/// Which canvas node each child of `unit` is, as it is held here: `trace` names them in core's
/// canonical order (as a deployed unit holds its children, and a run's paths count them), which
/// the unit made here may not be in yet.
fn nodes_of(unit: &Unit, trace: &[String]) -> Vec<String> {
    let UnitBody::Composite { children, edges } = &unit.body else { return vec![] };
    let Ok(order) = canonical_child_order(children, edges) else { return vec![] };
    let mut nodes = vec![String::new(); children.len()];
    for (k, i) in order.into_iter().enumerate() {
        if let (Some(slot), Some(id)) = (nodes.get_mut(i), trace.get(k)) {
            *slot = id.clone();
        }
    }
    nodes
}

/// The unit a flow deploys as, stored: its server when it has routes, else the whole flow grouped
/// as one composite. Why it can't be, otherwise.
pub fn flow_unit(nodes: &[Node], wires: &[Wire], store: &mut MapResolver, port: &str, tags: &[String]) -> Result<Hash, String> {
    let derived = derive_server(nodes, wires, store, port, tags);
    if !derived.errors.is_empty() {
        return Err(derived.errors.join("; "));
    }
    if !derived.endpoints.is_empty() || !derived.clocks.is_empty() || !derived.files.is_empty() || !derived.mails.is_empty() || !derived.feeds.is_empty() || !derived.queues.is_empty() || !derived.websockets.is_empty() || !derived.actions.is_empty() {
        let hash = derived.server_hash.ok_or_else(|| format!("\"{port}\" is not a port (set it under ⚙)"))?;
        return Ok(parse_hash(&hash).expect("a hash made here"));
    }
    if nodes.is_empty() {
        return Err("the canvas is empty".into());
    }
    // A flow without handlers deploys as itself, with no server to carry its tags.
    if !tags.is_empty() {
        return Err("tags are a server's: a flow without handlers has none".into());
    }
    let all: Vec<&Node> = nodes.iter().collect();
    let (children, edges) = group_of(&all, wires, store)?;
    let mut spec = default_spec(&children, &edges, store).map_err(|e| e.to_string())?;
    // An input a leaf works without (an http_request's headers and body, a listing's prefix and
    // after) stays unbound, as `route_spec` leaves it in a route: promoted to the boundary it
    // would be wired but absent on every run, and core would hold the leaf back for good.
    spec.promote.retain(|p| {
        let node = all[p.child as usize];
        !is_optional_input(node, node.data.inputs.get(p.port as usize).map(|i| i.name.as_str()))
    });
    let whole = derive_and_freeze(children, edges, &spec, store).map_err(|e| e.to_string())?;
    put(store, whole)
}
