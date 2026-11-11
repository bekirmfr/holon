//! Deriving a server from a flow: every connected group holding a `request` node is one route,
//! with that node's method and path. The group is grouped through core like any selection, its
//! boundary narrowed to what an endpoint needs ([`route_spec`]), and wrapped as an endpoint unit;
//! the endpoints together make the server unit (flow.rs). The shapes are the ones core's
//! declare.rs reads back, on deploy, in the host.

use std::collections::HashMap;

use holon_core::{parse_hash, parse_path, same_requests, Edge, EncapSpec, Hash, Param, Port, Resolver, Type, Unit, UnitBody, BOUNDARY, CALLS, METHODS};
use serde::Serialize;

use crate::builder::call_outputs;
use crate::canvas::{Node, PlacedBody, Wire};
use crate::wire::WType;

/// One route of the server: its request node, and every node wired (directly or not) to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct RouteGroup {
    pub method: String,
    pub path: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    /// How its runs are recorded: its trigger node's `log`, `lazy` or `unrecorded`, or "" (strict).
    pub log: String,
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
    /// How its runs are recorded: its trigger node's `log`, `lazy` or `unrecorded`, or "" (strict).
    pub log: String,
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
    /// How its runs are recorded: its trigger node's `log`, `lazy` or `unrecorded`, or "" (strict).
    pub log: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

/// One mail handler of the server: its `on_mail` node, its mail, and every node wired to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct MailGroup {
    pub mail: String,
    /// `on mail remit`: how the log names its runs.
    pub label: String,
    #[serde(rename = "onMailId")]
    pub on_mail_id: String,
    /// How its runs are recorded: its trigger node's `log`, `lazy` or `unrecorded`, or "" (strict).
    pub log: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

/// One queue handler of the server: its `on_queue` node, its queue, and every node wired to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct QueueGroup {
    pub queue: String,
    /// `on queue payouts`: how the log names its runs.
    pub label: String,
    #[serde(rename = "onQueueId")]
    pub on_queue_id: String,
    /// How its runs are recorded: its trigger node's `log`, `lazy` or `unrecorded`, or "" (strict).
    pub log: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

/// One feed handler of the server: its `on_feed` node, its feed, and every node wired to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FeedGroup {
    pub feed: String,
    /// `on feed prices`: how the log names its runs.
    pub label: String,
    #[serde(rename = "onFeedId")]
    pub on_feed_id: String,
    /// How its runs are recorded: its trigger node's `log`, `lazy` or `unrecorded`, or "" (strict).
    pub log: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

/// One block of a websocket of the server: its `websocket` node, its path, what it runs on, and
/// every node wired to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SocketGroup {
    pub path: String,
    /// `open`, `message` or `close`.
    pub on: String,
    /// `WebSocket /room/:name message`: how the log names its runs.
    pub label: String,
    #[serde(rename = "socketId")]
    pub socket_id: String,
    /// How its runs are recorded: its trigger node's `log`, `lazy` or `unrecorded`, or "" (strict).
    pub log: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

/// One action of the server: its `action` node, its name, and every node wired to it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ActionGroup {
    pub name: String,
    /// `action reset`: how the log names its runs.
    pub label: String,
    #[serde(rename = "actionId")]
    pub action_id: String,
    /// How its runs are recorded: its trigger node's `log`, `lazy` or `unrecorded`, or "" (strict).
    pub log: String,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
}

impl FileGroup {
    pub fn trigger(&self) -> holon_core::FileTrigger {
        holon_core::FileTrigger { folder: self.folder.clone(), deleted: self.deleted, matching: self.matching.clone() }
    }
}

/// What a node is the trigger of, when it is one: a route's request, a clock's tick, a file, a
/// message, an action.
pub(crate) fn trigger_label(n: &Node) -> Option<String> {
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
    } else if n.is_template("on_mail") {
        Some(holon_core::mail_label(n.param("mail")))
    } else if n.is_template("on_feed") {
        Some(holon_core::feed_label(n.param("feed")))
    } else if n.is_template("on_queue") {
        Some(holon_core::queue_label(n.param("queue")))
    } else if n.is_template("websocket") {
        Some(holon_core::socket_label(n.param("path"), n.param("on")))
    } else if n.is_template("action") {
        Some(format!("action {}", n.param("name")))
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
        clocks.push(ClockGroup { seconds, label, every_id: every.id.clone(), log: every.param("log").into(), node_ids: ids });
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
        files.push(FileGroup { folder: t.folder, deleted: t.deleted, matching: t.matching, label, on_file_id: on_file.id.clone(), log: on_file.param("log").into(), node_ids: ids });
    }
    // In the order they are written.
    files.sort_by_key(|f| node_ids.iter().position(|id| *id == f.on_file_id));
    (files, errors)
}

/// The mail handlers the flow declares (each connected group holding an `on_mail` node and no
/// other trigger), and why any can't be one.
pub fn find_mails(nodes: &[Node], wires: &[Wire]) -> (Vec<MailGroup>, Vec<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let (mut mails, mut errors): (Vec<MailGroup>, Vec<String>) = (Vec::new(), Vec::new());
    for ids in components(&node_ids, wires) {
        let triggers: Vec<&Node> = ids.iter().map(|id| by_id[id.as_str()]).filter(|n| trigger_label(n).is_some()).collect();
        let [on_mail] = triggers.as_slice() else { continue };
        if !on_mail.is_template("on_mail") {
            continue;
        }
        let mail = match crate::templates::mail_name(&on_mail.data.params) {
            Ok(m) => m,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let label = holon_core::mail_label(&mail);
        if mails.iter().any(|m| m.mail == mail) {
            errors.push(format!("{label} is written twice: one handler per mail"));
            continue;
        }
        mails.push(MailGroup { mail, label, on_mail_id: on_mail.id.clone(), log: on_mail.param("log").into(), node_ids: ids });
    }
    // In the order they are written.
    mails.sort_by_key(|m| node_ids.iter().position(|id| *id == m.on_mail_id));
    (mails, errors)
}

/// The feed handlers the flow declares (each connected group holding an `on_feed` node and no
/// other trigger), and why any can't be one.
pub fn find_feeds(nodes: &[Node], wires: &[Wire]) -> (Vec<FeedGroup>, Vec<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let (mut feeds, mut errors): (Vec<FeedGroup>, Vec<String>) = (Vec::new(), Vec::new());
    for ids in components(&node_ids, wires) {
        let triggers: Vec<&Node> = ids.iter().map(|id| by_id[id.as_str()]).filter(|n| trigger_label(n).is_some()).collect();
        let [on_feed] = triggers.as_slice() else { continue };
        if !on_feed.is_template("on_feed") {
            continue;
        }
        let feed = match crate::templates::feed_name(&on_feed.data.params) {
            Ok(f) => f,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let label = holon_core::feed_label(&feed);
        if feeds.iter().any(|f| f.feed == feed) {
            errors.push(format!("{label} is written twice: one handler per feed"));
            continue;
        }
        feeds.push(FeedGroup { feed, label, on_feed_id: on_feed.id.clone(), log: on_feed.param("log").into(), node_ids: ids });
    }
    // In the order they are written.
    feeds.sort_by_key(|f| node_ids.iter().position(|id| *id == f.on_feed_id));
    (feeds, errors)
}

/// The queue handlers the flow declares (each connected group holding an `on_queue` node and no
/// other trigger), and why any can't be one.
pub fn find_queues(nodes: &[Node], wires: &[Wire]) -> (Vec<QueueGroup>, Vec<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let (mut queues, mut errors): (Vec<QueueGroup>, Vec<String>) = (Vec::new(), Vec::new());
    for ids in components(&node_ids, wires) {
        let triggers: Vec<&Node> = ids.iter().map(|id| by_id[id.as_str()]).filter(|n| trigger_label(n).is_some()).collect();
        let [on_queue] = triggers.as_slice() else { continue };
        if !on_queue.is_template("on_queue") {
            continue;
        }
        let queue = match crate::templates::queue_name(&on_queue.data.params) {
            Ok(q) => q,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let label = holon_core::queue_label(&queue);
        if queues.iter().any(|q| q.queue == queue) {
            errors.push(format!("{label} is written twice: one handler per queue"));
            continue;
        }
        queues.push(QueueGroup { queue, label, on_queue_id: on_queue.id.clone(), log: on_queue.param("log").into(), node_ids: ids });
    }
    // In the order they are written.
    queues.sort_by_key(|q| node_ids.iter().position(|id| *id == q.on_queue_id));
    (queues, errors)
}

/// The websocket blocks the flow declares (each connected group holding a `websocket` node and no
/// other trigger), and why any can't be one: a block written twice, or a path that takes the
/// requests another websocket's or one of `routes`' GET routes does.
pub fn find_websockets(nodes: &[Node], wires: &[Wire], routes: &[RouteGroup]) -> (Vec<SocketGroup>, Vec<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let clash = |a: &str, b: &str| parse_path(a).zip(parse_path(b)).is_some_and(|(a, b)| same_requests(&a, &b));
    let (mut sockets, mut errors): (Vec<SocketGroup>, Vec<String>) = (Vec::new(), Vec::new());
    for ids in components(&node_ids, wires) {
        let triggers: Vec<&Node> = ids.iter().map(|id| by_id[id.as_str()]).filter(|n| trigger_label(n).is_some()).collect();
        let [socket] = triggers.as_slice() else { continue };
        if !socket.is_template("websocket") {
            continue;
        }
        let (path, on) = match crate::templates::socket_block(&socket.data.params) {
            Ok(b) => b,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let label = holon_core::socket_label(&path, on);
        if sockets.iter().any(|s| s.path == path && s.on == on) {
            errors.push(format!("{label} is written twice: one block of each per websocket"));
            continue;
        }
        if let Some(other) = sockets.iter().find(|s| s.path != path && clash(&s.path, &path)) {
            errors.push(format!("WebSocket {path} clashes with WebSocket {}", other.path));
            continue;
        }
        if let Some(r) = routes.iter().find(|r| r.method == "GET" && clash(&r.path, &path)) {
            errors.push(format!("WebSocket {path} clashes with GET {}: a path is a page or a connection", r.path));
            continue;
        }
        sockets.push(SocketGroup { path, on: on.into(), label, socket_id: socket.id.clone(), log: socket.param("log").into(), node_ids: ids });
    }
    // In the order they are written.
    sockets.sort_by_key(|s| node_ids.iter().position(|id| *id == s.socket_id));
    (sockets, errors)
}

/// The actions the flow declares (each connected group holding an `action` node and no other
/// trigger), and why any can't be one.
pub fn find_actions(nodes: &[Node], wires: &[Wire]) -> (Vec<ActionGroup>, Vec<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let node_ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let (mut actions, mut errors): (Vec<ActionGroup>, Vec<String>) = (Vec::new(), Vec::new());
    for ids in components(&node_ids, wires) {
        let triggers: Vec<&Node> = ids.iter().map(|id| by_id[id.as_str()]).filter(|n| trigger_label(n).is_some()).collect();
        let [action] = triggers.as_slice() else { continue };
        if !action.is_template("action") {
            continue;
        }
        let name = match crate::templates::action_name(&action.data.params) {
            Ok(n) => n,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let label = format!("action {name}");
        if actions.iter().any(|a| a.name == name) {
            errors.push(format!("{label} is written twice: one block per name"));
            continue;
        }
        actions.push(ActionGroup { name, label, action_id: action.id.clone(), log: action.param("log").into(), node_ids: ids });
    }
    // In the order they are written.
    actions.sort_by_key(|a| node_ids.iter().position(|id| *id == a.action_id));
    (actions, errors)
}

/// The routes the flow declares, and why any group can't be one. Groups without a request node
/// are not part of the server (helpers, scratch work) and are skipped silently.
pub fn find_routes(nodes: &[Node], wires: &[Wire]) -> (Vec<RouteGroup>, Vec<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut routes: Vec<RouteGroup> = Vec::new();
    let mut errors = Vec::new();
    let label = |n: &Node| format!("{} {}", n.param("method"), n.param("path"));
    // Two routes clash when they'd match the same requests, the rule core's read_server enforces.
    let clash = |a: &str, b: &str| parse_path(a).zip(parse_path(b)).is_some_and(|(a, b)| same_requests(&a, &b));
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
            errors.push(format!("\"{path}\" is not a route path (use /todos, /todos/:id or /files/*path)"));
            continue;
        }
        if let Some(clash) = routes.iter().find(|r| r.method == method && clash(&r.path, path)) {
            errors.push(format!("{method} {path} clashes with {} {}", clash.method, clash.path));
            continue;
        }
        routes.push(RouteGroup {
            method: method.into(),
            path: path.into(),
            request_id: req.id.clone(),
            node_ids: ids,
            resume: req.param("resume").into(),
            log: req.param("log").into(),
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
        "flow_call" | "flow_start" => &["input"],
        "collection_list" | "file_list" => &["prefix", "after"],
        "file_lines" => &["from"],
        "to_number" => &["radix"],
        "number_format" => &["digits"],
        "slice" => &["end"],
        "to_spliced" => &["count", "items"],
        "fill" | "copy_within" => &["start", "end"],
        "index_of" | "includes" | "last_index_of" => &["from"],
        "text_contains" => &["position"],
        "split" => &["limit"],
        "pad" => &["fill"],
        "csv_format" => &["columns"],
        "date_part" | "date_text" | "date_add" => &["offset"],
        "business_day_add" | "business_days_between" | "is_business_day" => &["holidays"],
        "xml_text" => &["path"],
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
pub fn route_spec(spec: EncapSpec, group: &[&Node], label: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
    // A route answers and is done: what waits is an action it starts (docs/L4s/typescript-syntax-L4.md §9).
    if let Some(n) = group.iter().find(|n| n.data.body == PlacedBody::Primitive && matches!(n.data.name.as_str(), "await" | "approval" | "resume_token")) {
        return Err(format!("{label}: a route answers and is done, so it doesn't wait ({}): start an action that waits with a flow_start, and answer with what it says", n.data.name));
    }
    caller_spec(spec, group, label, "request", store)
}

/// A websocket block's boundary, as a route's: the one input left open is its `websocket` node's
/// (the record), the outputs its respond's (the answer) and its effects. It answers and is done,
/// as a route does.
pub fn socket_spec(spec: EncapSpec, group: &[&Node], label: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
    if let Some(n) = group.iter().find(|n| n.data.body == PlacedBody::Primitive && matches!(n.data.name.as_str(), "await" | "resume_token")) {
        return Err(format!("{label}: a block answers and is done, so it doesn't wait ({}): start an action that waits with a flow_start", n.data.name));
    }
    caller_spec(spec, group, label, "websocket", store)
}

/// An action's boundary, as a route's: the one input left open is its `action` node's (what was
/// sent), the outputs its respond's (the answer) and its effects.
pub fn action_spec(spec: EncapSpec, group: &[&Node], label: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
    caller_spec(spec, group, label, "action", store)
}

/// A handler with a caller to answer, whose `trigger` node takes what the caller sent.
fn caller_spec(spec: EncapSpec, group: &[&Node], label: &str, trigger: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
    let input_name = |child: u32, port: u32| group[child as usize].data.inputs.get(port as usize).map(|p| p.name.as_str());
    let promote: Vec<_> =
        spec.promote.into_iter().filter(|p| !is_optional_input(group[p.child as usize], input_name(p.child, p.port))).collect();
    for p in &promote {
        let n = group[p.child as usize];
        if !n.is_template(trigger) {
            let port = input_name(p.child, p.port).map(String::from).unwrap_or_else(|| p.port.to_string());
            return Err(format!("{label}: {} · {port} is not wired", n.data.name));
        }
    }
    let is_respond = |child: u32| group[child as usize].is_template("respond");
    let expose: Vec<_> = spec.expose.into_iter().filter(|x| is_respond(x.child) || is_effect(group, x.child, x.port, store)).collect();
    let (responses, effects): (Vec<_>, Vec<_>) = expose.into_iter().partition(|x| is_respond(x.child));
    let (calls, writes): (Vec<_>, Vec<_>) = effects.into_iter().partition(|x| is_call(group, x.child, x.port, store));
    if responses.len() > 1 {
        return Err(format!("{label}: more than one respond node"));
    }
    if responses.is_empty() && writes.is_empty() {
        return Err(match (trigger, calls.is_empty()) {
            ("action", true) => format!("{label}: it does nothing: respond with something, or write something (state, a collection, a channel)"),
            ("action", false) => format!("{label}: a call's answer is the far side's, not the action's: respond with something"),
            (_, true) => format!("{label}: add a respond node (or an effect, answered 204)"),
            (_, false) => format!("{label}: a call's answer is the far side's, not the route's: add a respond node"),
        });
    }
    // Output 0 is the answer: the respond's, or without one a write's (answered 204), never a
    // call's. The calls come last, as effects the endpoint keeps no answer of.
    Ok(EncapSpec { promote, expose: responses.into_iter().chain(writes).chain(calls).collect() })
}

/// Whether output `port` of node `child` answers a call ([`CALLS`]), made for what it does,
/// whatever it answers: the call's own, or a function's that makes one (`call_outputs`).
fn is_call(group: &[&Node], child: u32, port: u32, store: &impl Resolver) -> bool {
    let n = group[child as usize];
    match &n.data.body {
        PlacedBody::Composite { hash } => parse_hash(hash)
            .and_then(|h| store.resolve(&h))
            .is_some_and(|u| u.outputs.get(port as usize).is_some_and(|p| call_outputs(u, store).contains(&p.name))),
        _ => CALLS.contains(&n.data.name.as_str()),
    }
}

/// Whether output `port` of node `child` is an effect the host performs: an Effect value, or a
/// call's answer, kept so the call is made whether or not anything reads it.
fn is_effect(group: &[&Node], child: u32, port: u32, store: &impl Resolver) -> bool {
    is_call(group, child, port, store) || group[child as usize].data.outputs.get(port as usize).is_some_and(|p| p.ty == WType::Effect)
}

/// Narrow core's proposed boundary for one clock's group to what a clock handler is: no inputs
/// (a tick has none: every input a node needs must be wired) and its effects as outputs.
pub fn clock_spec(spec: EncapSpec, group: &[&Node], label: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
    handler_spec(spec, group, label, "a clock", store)
}

/// A file handler's boundary, as a clock's: no inputs (the file's handle is its `on_file` node's)
/// and its effects as outputs.
pub fn file_spec(spec: EncapSpec, group: &[&Node], label: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
    handler_spec(spec, group, label, "a file's run", store)
}

/// A mail handler's boundary, as a file's: no inputs (the message is its `on_mail` node's) and its
/// effects as outputs.
pub fn mail_spec(spec: EncapSpec, group: &[&Node], label: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
    handler_spec(spec, group, label, "a message's run", store)
}

/// A feed handler's boundary, as a mail's: no inputs (the event is its `on_feed` node's) and its
/// effects as outputs.
pub fn feed_spec(spec: EncapSpec, group: &[&Node], label: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
    handler_spec(spec, group, label, "an event's run", store)
}

/// A queue handler's boundary, as a feed's: no inputs (the message is its `on_queue` node's) and
/// its effects as outputs.
pub fn queue_spec(spec: EncapSpec, group: &[&Node], label: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
    handler_spec(spec, group, label, "a message's run", store)
}

/// A handler with no caller to answer: no inputs, its effects as outputs. `who` says which kind.
fn handler_spec(spec: EncapSpec, group: &[&Node], label: &str, who: &str, store: &impl Resolver) -> Result<EncapSpec, String> {
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
    let expose: Vec<_> = spec.expose.into_iter().filter(|x| is_effect(group, x.child, x.port, store)).collect();
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

/// Wrap a mail handler's handler as a mail handler unit (core's `read_mail_handler`): no inputs,
/// each of its effects an output, `declare = "on_mail"` and `mail`.
pub fn mail_unit(handler: Hash, outputs: &[Port], mail: &str) -> Unit {
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    Unit {
        body: UnitBody::Composite {
            children: vec![handler],
            edges: (0..outputs.len() as u32).map(|k| Edge { from_child: 0, from_port: k, to_child: BOUNDARY, to_port: k }).collect(),
        },
        params: vec![frozen("declare", "on_mail"), frozen("mail", mail)],
        inputs: vec![],
        outputs: outputs.to_vec(),
    }
}

/// Wrap a feed handler's handler as a feed handler unit (core's `read_feed_handler`): no inputs,
/// each of its effects an output, `declare = "on_feed"` and `feed`.
pub fn feed_unit(handler: Hash, outputs: &[Port], feed: &str) -> Unit {
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    Unit {
        body: UnitBody::Composite {
            children: vec![handler],
            edges: (0..outputs.len() as u32).map(|k| Edge { from_child: 0, from_port: k, to_child: BOUNDARY, to_port: k }).collect(),
        },
        params: vec![frozen("declare", "on_feed"), frozen("feed", feed)],
        inputs: vec![],
        outputs: outputs.to_vec(),
    }
}

/// Wrap a queue handler's handler as a queue handler unit (core's `read_queue_handler`): no
/// inputs, each of its effects an output, `declare = "on_queue"` and `queue`.
pub fn queue_unit(handler: Hash, outputs: &[Port], queue: &str) -> Unit {
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    Unit {
        body: UnitBody::Composite {
            children: vec![handler],
            edges: (0..outputs.len() as u32).map(|k| Edge { from_child: 0, from_port: k, to_child: BOUNDARY, to_port: k }).collect(),
        },
        params: vec![frozen("declare", "on_queue"), frozen("queue", queue)],
        inputs: vec![],
        outputs: outputs.to_vec(),
    }
}

/// `unit` (a handler unit: an endpoint, a clock's, a file's, a message's or an action's) with its log level
/// (docs/L4s/request-path-L4.md, C5): its frozen `log` param, set only when it isn't strict, so a
/// handler written without one hashes as it always did. An error for a word that isn't a level.
pub fn with_log(mut unit: Unit, log: &str) -> Result<Unit, String> {
    match holon_core::LogLevel::parse(log) {
        None => Err(format!("\"{log}\" is not a log level (lazy or unrecorded; strict is the default)")),
        Some(holon_core::LogLevel::Strict) => Ok(unit),
        Some(level) => {
            unit.params.push(Param::Frozen { name: "log".into(), value: level.word().into() });
            Ok(unit)
        }
    }
}

/// A waiting point's name: what a `resume` route may name.
fn is_wait_name(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Wrap a handler as an endpoint unit. The endpoint's one input is the request record, wired to
/// the handler's input if it has one (a handler with no input, e.g. a plain listing, simply
/// ignores the request). Output 0 is the response: the handler's first output, its respond's or,
/// without one, its first write's (which the data plane answers with 204), as [`route_spec`]
/// orders them. Every other effect output follows, so the host performs it. An error message when the handler can't be an
/// endpoint.
pub fn endpoint_unit(handler: Hash, inputs: &[Port], outputs: &[Port], method: &str, path: &str, resume: &str) -> Result<Unit, String> {
    if !resume.is_empty() && !is_wait_name(resume) {
        return Err(format!("\"{resume}\" is not a waiting point's name (letters, digits, - and _)"));
    }
    if parse_path(path).is_none() {
        return Err(format!("\"{path}\" is not a route path (use /todos, /todos/:id or /files/*path)"));
    }
    if inputs.len() > 1 {
        return Err("a handler takes at most one input: the request record".into());
    }
    if outputs.is_empty() {
        return Err("a handler needs an output: the response".into());
    }
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    let mut params = vec![frozen("declare", "endpoint"), frozen("method", method), frozen("path", path)];
    // Only when set, so an ordinary route's endpoint hashes as it always did.
    if !resume.is_empty() {
        params.push(frozen("resume", resume));
    }
    Ok(caller_unit(handler, inputs, outputs, params, Port { name: "request".into(), ty: Type::Record }, "response"))
}

/// Wrap a websocket block's handler as its unit (core's `read_websocket`), as an endpoint is
/// wrapped: its one input the block's record, output 0 its answer, then its effects;
/// `declare = "websocket"`, `path` and `on`.
pub fn socket_unit(handler: Hash, inputs: &[Port], outputs: &[Port], path: &str, on: &str) -> Result<Unit, String> {
    if parse_path(path).is_none() {
        return Err(format!("\"{path}\" is not a route path (use /chat, /room/:name or /files/*path)"));
    }
    if inputs.len() > 1 {
        return Err("a handler takes at most one input: the block's record".into());
    }
    if outputs.is_empty() {
        return Err("a handler needs an output: its answer or an effect".into());
    }
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    let params = vec![frozen("declare", "websocket"), frozen("path", path), frozen("on", on)];
    Ok(caller_unit(handler, inputs, outputs, params, Port { name: "request".into(), ty: Type::Record }, "response"))
}

/// Wrap an action's handler as an action handler unit (core's `read_action`), as an endpoint is
/// wrapped: its one input what was sent, output 0 the answer (the respond's envelope, whose body
/// the host answers with), then its effects; `declare = "action"` and `name`.
pub fn action_unit(handler: Hash, inputs: &[Port], outputs: &[Port], name: &str) -> Result<Unit, String> {
    if inputs.len() > 1 {
        return Err("a handler takes at most one input: what was sent".into());
    }
    if outputs.is_empty() {
        return Err("a handler needs an output: its answer or an effect".into());
    }
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    let params = vec![frozen("declare", "action"), frozen("name", name)];
    Ok(caller_unit(handler, inputs, outputs, params, Port { name: "input".into(), ty: Type::Any }, "answer"))
}

/// A handler wrapped for its caller: `input` fed to the handler's input if it has one, output 0
/// (named `first`) its first output (its answer), then every other effect.
fn caller_unit(handler: Hash, inputs: &[Port], outputs: &[Port], params: Vec<Param>, input: Port, first: &str) -> Unit {
    let order: Vec<usize> = std::iter::once(0).chain((1..outputs.len()).filter(|&i| outputs[i].ty == Type::Effect)).collect();
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
    Unit {
        body: UnitBody::Composite { children: vec![handler], edges },
        params,
        inputs: vec![input],
        outputs: order
            .iter()
            .enumerate()
            .map(|(k, &i)| Port { name: if k == 0 { first.into() } else { outputs[i].name.clone() }, ty: outputs[i].ty.clone() })
            .collect(),
    }
}

/// The keys a flow taints: those its handlers' triggers carry (`taint`, joined by spaces), each
/// once, in the order first met. A key one trigger carries is tainted for every run.
pub fn flow_taint(nodes: &[Node]) -> Vec<&str> {
    let mut keys: Vec<&str> = Vec::new();
    for n in nodes.iter().filter(|n| crate::templates::HANDLERS.iter().any(|h| n.is_template(h))) {
        for key in n.param("taint").split_whitespace() {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

/// A server unit over the given endpoints, clock handlers, file handlers and actions, in that order, with `port` when it
/// has a route, and the keys the flow taints (joined by single spaces, set only when there is one,
/// so a flow without hashes as it always did; core reads them, `Server::taint`), and its tags,
/// the same way (`Server::tags`). Core's `read_server` re-checks everything on deploy.
pub fn server_unit(handlers: Vec<Hash>, port: Option<u16>, taint: &[&str], tags: &[String]) -> Unit {
    let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
    let mut params = vec![frozen("declare", "server")];
    if let Some(port) = port {
        params.push(frozen("port", &port.to_string()));
    }
    if !taint.is_empty() {
        params.push(frozen("taint", &taint.join(" ")));
    }
    if !tags.is_empty() {
        params.push(frozen("tags", &tags.join(" ")));
    }
    Unit { body: UnitBody::Composite { children: handlers, edges: vec![] }, params, inputs: vec![], outputs: vec![] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::NodeData;
    use crate::templates;
    use crate::wire::{WParam, WPort};
    use holon_core::{Expose, MapResolver, Promote};

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
                "\"no-slash\" is not a route path (use /todos, /todos/:id or /files/*path)",
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
        let got = route_spec(spec, &group, "POST /todos", &MapResolver::new()).unwrap();
        assert_eq!(got.promote, [promote(0, 0, "request")]);
        assert_eq!(got.expose, [expose(3, 0, "response"), expose(2, 0, "write")]);

        let base = || EncapSpec { promote: vec![promote(0, 0, "request")], expose: vec![] };
        let unwired = EncapSpec { promote: vec![promote(0, 0, "request"), promote(1, 0, "a")], ..base() };
        assert_eq!(route_spec(unwired, &group, "POST /todos", &MapResolver::new()).unwrap_err(), "POST /todos: field · a is not wired");
        let two = EncapSpec { expose: vec![expose(3, 0, "r1"), expose(3, 0, "r2")], ..base() };
        assert_eq!(route_spec(two, &group, "POST /todos", &MapResolver::new()).unwrap_err(), "POST /todos: more than one respond node");
        assert_eq!(
            route_spec(base(), &group, "POST /todos", &MapResolver::new()).unwrap_err(),
            "POST /todos: add a respond node (or an effect, answered 204)"
        );
    }

    /// A route answers and is done: one with an `await` or a `resume_token` is refused, the
    /// canvas's as the text's (docs/L4s/typescript-syntax-L4.md §9).
    #[test]
    fn a_route_doesnt_wait() {
        for leaf in ["await", "resume_token"] {
            let g = [req("r", "POST", "/a"), plain("w", leaf), res("s")];
            let group: Vec<&Node> = g.iter().collect();
            let spec = EncapSpec { promote: vec![promote(0, 0, "request")], expose: vec![expose(2, 0, "response")] };
            let e = route_spec(spec, &group, "POST /a", &MapResolver::new()).unwrap_err();
            assert!(e.starts_with("POST /a: a route answers and is done, so it doesn't wait"), "{e}");
        }
    }

    #[test]
    fn a_call_is_kept_as_an_effect_and_never_answers() {
        let g = [req("r", "POST", "/hook"), plain("h", "http_request"), plain("w", "store_write"), res("s"), plain("c", "flow_call")];
        let group: Vec<&Node> = g.iter().collect();
        let spec = |expose: Vec<Expose>| EncapSpec { promote: vec![promote(0, 0, "request")], expose };
        // Read by nothing, each call is kept: after the answer and the writes.
        let all = vec![expose(1, 0, "sent"), expose(2, 0, "write"), expose(3, 0, "response"), expose(4, 0, "paid")];
        let got = route_spec(spec(all), &group, "POST /hook", &MapResolver::new()).unwrap();
        assert_eq!(got.expose, [expose(3, 0, "response"), expose(2, 0, "write"), expose(1, 0, "sent"), expose(4, 0, "paid")]);
        // Without a respond a write answers (204), not the call before it.
        let got = route_spec(spec(vec![expose(1, 0, "sent"), expose(2, 0, "write")]), &group, "POST /hook", &MapResolver::new()).unwrap();
        assert_eq!(got.expose, [expose(2, 0, "write"), expose(1, 0, "sent")]);
        // A call alone isn't the route's answer, nor an action's, but is a clock's work.
        assert_eq!(
            route_spec(spec(vec![expose(1, 0, "sent")]), &group, "POST /hook", &MapResolver::new()).unwrap_err(),
            "POST /hook: a call's answer is the far side's, not the route's: add a respond node"
        );
        let mut act = g.clone();
        act[0] = node("r", "action", vec![WParam::frozen("name", "notify")], true);
        let group: Vec<&Node> = act.iter().collect();
        assert_eq!(
            action_spec(spec(vec![expose(1, 0, "sent")]), &group, "action notify", &MapResolver::new()).unwrap_err(),
            "action notify: a call's answer is the far side's, not the action's: respond with something"
        );
        let tick = [plain("t", "every"), plain("h", "http_request")];
        let group: Vec<&Node> = tick.iter().collect();
        let clock = EncapSpec { promote: vec![], expose: vec![expose(1, 0, "sent")] };
        assert_eq!(clock_spec(clock, &group, "every 1h", &MapResolver::new()).unwrap().expose, [expose(1, 0, "sent")]);
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
        assert_eq!(route_spec(spec(), &group, "GET /fetch", &MapResolver::new()).unwrap().promote, [promote(0, 0, "request")]);
        let mut unwired = spec();
        unwired.promote.push(promote(1, 0, "path"));
        assert_eq!(route_spec(unwired, &group, "GET /fetch", &MapResolver::new()).unwrap_err(), "GET /fetch: http_request · path is not wired");
    }

    fn port(name: &str, ty: Type) -> Port {
        Port { name: name.into(), ty }
    }
    fn edge(from_child: u32, from_port: u32, to_child: u32, to_port: u32) -> Edge {
        Edge { from_child, from_port, to_child, to_port }
    }

    #[test]
    fn an_endpoint_answers_with_the_first_output_and_wires_the_request_in() {
        let h = [7u8; 32];
        // As route_spec orders them: the answer, the writes, then a call's answer, which the
        // handler keeps and the endpoint doesn't answer with.
        let outputs = [port("item", Type::Any), port("write", Type::Effect), port("log", Type::Effect), port("sent", Type::Record)];
        let u = endpoint_unit(h, &[port("record", Type::Record)], &outputs, "POST", "/todos", "").unwrap();
        let got: Vec<_> = u.outputs.iter().map(|p| (p.name.as_str(), p.ty.clone())).collect();
        assert_eq!(got, [("response", Type::Any), ("write", Type::Effect), ("log", Type::Effect)]);
        let b = BOUNDARY;
        let edges = vec![edge(b, 0, 0, 0), edge(0, 0, b, 0), edge(0, 1, b, 1), edge(0, 2, b, 2)];
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

    /// A handler's log level is a frozen `log` param only when it isn't strict, so a handler
    /// written without a word is the unit it always was (docs/L4s/request-path-L4.md, C5).
    #[test]
    fn a_handlers_log_level_is_in_its_unit_only_when_it_isnt_strict() {
        let h = [7u8; 32];
        let out = [port("o", Type::Any)];
        let plain = endpoint_unit(h, &[], &out, "GET", "/notes", "").unwrap();
        assert_eq!(with_log(plain.clone(), "").unwrap(), plain);
        assert!(!plain.params.iter().any(|p| matches!(p, Param::Frozen { name, .. } if name == "log")));
        for word in ["lazy", "unrecorded"] {
            let u = with_log(plain.clone(), word).unwrap();
            assert_eq!(u.params.last(), Some(&Param::Frozen { name: "log".into(), value: word.into() }));
            assert_eq!(holon_core::log_level(&u), holon_core::LogLevel::parse(word));
        }
        assert!(with_log(plain, "verbose").unwrap_err().contains("not a log level"));
    }

    #[test]
    fn a_server_is_a_wireless_composite_of_endpoints_with_its_port() {
        let u = server_unit(vec![[1; 32], [2; 32]], Some(8080), &[], &[]);
        assert_eq!(u.body, UnitBody::Composite { children: vec![[1; 32], [2; 32]], edges: vec![] });
        let frozen = |name: &str, value: &str| Param::Frozen { name: name.into(), value: value.into() };
        assert_eq!(u.params, [frozen("declare", "server"), frozen("port", "8080")]);
        assert!(u.inputs.is_empty() && u.outputs.is_empty());
    }
}
