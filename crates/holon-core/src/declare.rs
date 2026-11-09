//! Declaration units (M1, docs/PLATFORM_LAYOUT.md §5, option A): an HTTP API as ordinary units.
//!
//! - An **endpoint** is a composite (any internal graph: the handler) from a `request` record to a
//!   response, carrying frozen params `declare = "endpoint"`, `method` and `path`. Evaluating it runs the handler, so testing
//!   an endpoint is plain `eval`. Output 0 is the response; any further outputs carry effects
//!   (state writes) for the host to perform.
//! - A **clock handler** is a composite with no inputs, its outputs the effects of one tick,
//!   carrying `declare = "every"` and `seconds`: the host runs it every `seconds`
//!   (docs/L4s/triggers-L4.md §4).
//! - A **file handler** is a composite with no inputs, its outputs the effects of one file's run,
//!   carrying `declare = "on_file"`, `folder`, and `on` (`deleted`) and `matching` when given: the
//!   host runs it for each file its trigger starts. The file's handle is sourced, as a
//!   `file_arrival` leaf's is (docs/L4s/triggers-L4.md §4, as built).
//! - A **mail handler** is a composite with no inputs, its outputs the effects of one message's
//!   run, carrying `declare = "on_mail"` and `mail`: the host runs it for each message that comes
//!   to the mailbox an admin bound to that mail. The message is sourced, as a `mail_arrival`
//!   leaf's is (host-capabilities L4 §5.2.4).
//! - A **feed handler** is a composite with no inputs, its outputs the effects of one event's run,
//!   carrying `declare = "on_feed"` and `feed`: the host runs it for each event the feed an admin
//!   bound to that name sends. The event is sourced, as a `feed_event` leaf's is
//!   (host-capabilities L4 §5.9.2).
//! - A **queue handler** is a composite with no inputs, its outputs the effects of one message's
//!   run, carrying `declare = "on_queue"` and `queue`: the host runs it for each message that comes
//!   to the queue an admin bound to that name. The message is sourced, as a `queue_message` leaf's
//!   is (host-capabilities L4 §5.9.9).
//! - An **action handler** is a composite from what its caller sent to its answer (an envelope, as
//!   an endpoint's response is, whose body is the answer), then its effects, carrying
//!   `declare = "action"` and `name`: a person runs it by name through Holon, never the network.
//! - A **server** is a composite whose children are endpoints, clock handlers, file handlers, mail handlers, feed handlers and action handlers, with no wiring
//!   and no ports, carrying `declare = "server"`, and `port` when it has a route. Its hash is the
//!   hash of the whole flow.
//!
//! Nothing here changes the encoding: the markers are ordinary frozen params. This module is the
//! one place that reads them, used by the host (to listen and route) and the editor (to check a
//! server before deploying) alike.

use std::collections::BTreeMap;

use crate::{FileTrigger, Hash, Resolver, Type, Unit, UnitBody};

/// The methods an endpoint may declare.
pub const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];

/// One segment of a route path: `/todos/:id` is `[Literal("todos"), Param("id")]`, and
/// `/files/*path` is `[Literal("files"), Rest("path")]`, a rest only ever last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    Param(String),
    /// The rest of the path, one segment or more (docs/L4s/host-capabilities-L4.md §5.11.2).
    Rest(String),
}

/// How a handler's runs are recorded (docs/L4s/request-path-L4.md, C5): `strict` by default, the
/// record on disk before the run answers; `lazy`, on disk within a second; `unrecorded`, none but
/// a failure's. A handler carries it as its frozen `log` param, absent for strict, so a handler
/// written without one hashes as it always did. A handler is what its author declares, whatever
/// it does: an unrecorded one that writes, calls out, sends or waits leaves that out of the log
/// too, which the person approving it is told.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogLevel {
    #[default]
    Strict,
    Lazy,
    Unrecorded,
}

impl LogLevel {
    /// The level as written on a handler's line; strict is never written.
    pub fn word(self) -> &'static str {
        match self {
            LogLevel::Strict => "strict",
            LogLevel::Lazy => "lazy",
            LogLevel::Unrecorded => "unrecorded",
        }
    }

    /// A level from its word: `lazy` or `unrecorded`, or `strict` (or nothing) for the default.
    pub fn parse(word: &str) -> Option<LogLevel> {
        match word {
            "" | "strict" => Some(LogLevel::Strict),
            "lazy" => Some(LogLevel::Lazy),
            "unrecorded" => Some(LogLevel::Unrecorded),
            _ => None,
        }
    }
}

/// A handler unit's level, from its frozen `log` param: strict without one, none for a value that
/// isn't a level.
pub fn log_level(unit: &Unit) -> Option<LogLevel> {
    LogLevel::parse(frozen(unit, "log").unwrap_or(""))
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
    /// The waiting point this route resumes (its endpoint's `resume` param), if it is a resume
    /// route: a request carrying a run's token delivers itself to that run's `await` of this name.
    pub resume: Option<String>,
    /// How its runs are recorded.
    pub log: LogLevel,
}

/// One clock handler of a server, as declared: a run every `seconds`.
#[derive(Clone, Debug, PartialEq)]
pub struct Clock {
    pub seconds: u64,
    /// How the log and the editor name it: `every 2s`.
    pub label: String,
    /// The handler unit's hash (the server's child), which a tick evaluates with no inputs.
    pub handler: Hash,
    /// How its runs are recorded.
    pub log: LogLevel,
}

/// One file handler of a server, as declared: a run for each file its trigger starts.
#[derive(Clone, Debug, PartialEq)]
pub struct FileHandler {
    pub trigger: FileTrigger,
    /// How the log and the editor name it: `on file inbox`.
    pub label: String,
    /// The handler unit's hash (the server's child), which a file evaluates with no inputs, the
    /// file's handle sourced.
    pub handler: Hash,
    /// How its runs are recorded.
    pub log: LogLevel,
}

/// One mail handler of a server, as declared: a run for each message that comes to its mail's
/// mailbox.
#[derive(Clone, Debug, PartialEq)]
pub struct MailHandler {
    /// The mail resource whose mailbox it reads: `remit`.
    pub mail: String,
    /// How the log and the editor name it: `on mail remit`.
    pub label: String,
    /// The handler unit's hash (the server's child), which a message evaluates with no inputs, the
    /// message sourced.
    pub handler: Hash,
    /// How its runs are recorded.
    pub log: LogLevel,
}

/// A mail handler's label, as the log keeps it: `on mail remit`.
pub fn mail_label(mail: &str) -> String {
    format!("on mail {mail}")
}

/// One feed handler of a server, as declared: a run for each event its feed sends.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedHandler {
    /// The feed it listens to: `prices`.
    pub feed: String,
    /// How the log and the editor name it: `on feed prices`.
    pub label: String,
    /// The handler unit's hash (the server's child), which an event evaluates with no inputs, the
    /// event sourced.
    pub handler: Hash,
    /// How its runs are recorded.
    pub log: LogLevel,
}

/// A feed handler's label, as the log keeps it: `on feed prices`.
pub fn feed_label(feed: &str) -> String {
    format!("on feed {feed}")
}

/// One queue handler of a server, as declared: a run for each message that comes to its queue.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueHandler {
    /// The queue it listens to: `payouts`.
    pub queue: String,
    /// How the log and the editor name it: `on queue payouts`.
    pub label: String,
    /// The handler unit's hash (the server's child), which a message evaluates with no inputs, the
    /// message sourced.
    pub handler: Hash,
    /// How its runs are recorded.
    pub log: LogLevel,
}

/// A queue handler's label, as the log keeps it: `on queue payouts`.
pub fn queue_label(queue: &str) -> String {
    format!("on queue {queue}")
}

/// One action handler of a server, as declared: run by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub name: String,
    /// How the log and the editor name it: `action reset`.
    pub label: String,
    /// The handler unit's hash (the server's child), which a run evaluates with what was sent.
    pub handler: Hash,
    /// How its runs are recorded.
    pub log: LogLevel,
}

/// A server unit, read and checked. `port` is there when it has a route.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub port: Option<u16>,
    pub routes: Vec<Route>,
    pub clocks: Vec<Clock>,
    pub files: Vec<FileHandler>,
    pub mails: Vec<MailHandler>,
    pub feeds: Vec<FeedHandler>,
    pub queues: Vec<QueueHandler>,
    pub actions: Vec<Action>,
}

impl Server {
    /// The labels of its handlers declared `unrecorded` (`GET /health`, `action report`), as their
    /// runs are labelled: what each asks to be approved, `Unrecorded("GET /health")`.
    pub fn unrecorded(&self) -> std::collections::BTreeSet<String> {
        self.handlers().filter(|(_, log, _)| *log == LogLevel::Unrecorded).map(|(label, _, _)| label).collect()
    }

    /// Each of its handlers: its label, as its runs are labelled, its level, and its unit.
    pub fn handlers(&self) -> impl Iterator<Item = (String, LogLevel, Hash)> + '_ {
        let routes = self.routes.iter().map(|r| (format!("{} {}", r.method, r.path), r.log, r.endpoint));
        let clocks = self.clocks.iter().map(|c| (c.label.clone(), c.log, c.handler));
        let files = self.files.iter().map(|f| (f.label.clone(), f.log, f.handler));
        let mails = self.mails.iter().map(|m| (m.label.clone(), m.log, m.handler));
        let feeds = self.feeds.iter().map(|f| (f.label.clone(), f.log, f.handler));
        let queues = self.queues.iter().map(|q| (q.label.clone(), q.log, q.handler));
        let actions = self.actions.iter().map(|a| (a.label.clone(), a.log, a.handler));
        routes.chain(clocks).chain(files).chain(mails).chain(feeds).chain(queues).chain(actions)
    }

    /// Its action named `name`, if it has one.
    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.name == name)
    }
}

/// The longest period a clock handler may have: 366 days, in seconds.
pub const MAX_PERIOD: u64 = 366 * 86_400;

/// How a period is written: `2s`, `5m`, `1h`, `1d`, the largest unit it is a whole number of.
pub fn period_text(seconds: u64) -> String {
    for (per, unit) in [(86_400, 'd'), (3_600, 'h'), (60, 'm')] {
        if seconds.is_multiple_of(per) {
            return format!("{}{unit}", seconds / per);
        }
    }
    format!("{seconds}s")
}

/// A clock handler's label, as the log keeps it: `every 2s`.
pub fn clock_label(seconds: u64) -> String {
    format!("every {}", period_text(seconds))
}

/// A clock handler's period, if `unit` is one: `declare = "every"`, whole `seconds` from 1 to
/// [`MAX_PERIOD`], a composite with no inputs.
pub fn read_clock(unit: &Unit) -> Option<u64> {
    if !matches!(unit.body, UnitBody::Composite { .. }) || declaration_of(unit) != Some("every") || !unit.inputs.is_empty() {
        return None;
    }
    frozen(unit, "seconds")?.parse::<u64>().ok().filter(|s| (1..=MAX_PERIOD).contains(s))
}

/// An action handler's name, if `unit` is one: `declare = "action"`, a name (letters, digits, `-`
/// and `_`), a composite of one input with an answer.
pub fn read_action(unit: &Unit) -> Option<String> {
    if !matches!(unit.body, UnitBody::Composite { .. }) || declaration_of(unit) != Some("action") || unit.inputs.len() != 1 || unit.outputs.is_empty() {
        return None;
    }
    crate::kind::ident(&unit.params, "name")
}

/// A file handler's trigger, if `unit` is one: `declare = "on_file"`, a folder's name, `on`
/// absent, `new` or `deleted`, `matching` absent or a glob, a composite with no inputs.
pub fn read_file_handler(unit: &Unit) -> Option<FileTrigger> {
    if !matches!(unit.body, UnitBody::Composite { .. }) || declaration_of(unit) != Some("on_file") || !unit.inputs.is_empty() {
        return None;
    }
    let folder = crate::kind::ident(&unit.params, "folder")?;
    let deleted = match frozen(unit, "on") {
        None | Some("new") => false,
        Some("deleted") => true,
        Some(_) => return None,
    };
    let matching = match frozen(unit, "matching") {
        None | Some("") => String::new(),
        Some(g) if crate::is_glob(g) => g.to_string(),
        Some(_) => return None,
    };
    Some(FileTrigger { folder, deleted, matching })
}

/// A mail handler's mail, if `unit` is one: `declare = "on_mail"`, a mail's name, a composite with
/// no inputs.
pub fn read_mail_handler(unit: &Unit) -> Option<String> {
    if !matches!(unit.body, UnitBody::Composite { .. }) || declaration_of(unit) != Some("on_mail") || !unit.inputs.is_empty() {
        return None;
    }
    crate::kind::ident(&unit.params, "mail")
}

/// A feed handler's feed, if `unit` is one: `declare = "on_feed"`, a feed's name, a composite with
/// no inputs.
pub fn read_feed_handler(unit: &Unit) -> Option<String> {
    if !matches!(unit.body, UnitBody::Composite { .. }) || declaration_of(unit) != Some("on_feed") || !unit.inputs.is_empty() {
        return None;
    }
    crate::kind::ident(&unit.params, "feed")
}

/// A queue handler's queue, if `unit` is one: `declare = "on_queue"`, a queue's name, a composite
/// with no inputs.
pub fn read_queue_handler(unit: &Unit) -> Option<String> {
    if !matches!(unit.body, UnitBody::Composite { .. }) || declaration_of(unit) != Some("on_queue") || !unit.inputs.is_empty() {
        return None;
    }
    crate::kind::ident(&unit.params, "queue")
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
    /// Two clock handlers with the same period.
    DuplicateClock { seconds: u64 },
    /// Two file handlers with the same folder, `on` and `matching`.
    DuplicateFileHandler { label: String },
    /// Two mail handlers of one mail.
    DuplicateMailHandler { mail: String },
    /// Two feed handlers of one feed.
    DuplicateFeedHandler { feed: String },
    /// Two queue handlers of one queue.
    DuplicateQueueHandler { queue: String },
    /// Two actions of one name.
    DuplicateAction { name: String },
    /// No route, clock, file, mail, feed or queue handler or action: nothing would start a run.
    NoHandlers,
    /// A handler's `log` param isn't a level.
    BadLogLevel { child: u32, value: String },
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
    if !edges.is_empty() || !unit.inputs.is_empty() || !unit.outputs.is_empty() {
        return Err(DeclError::ServerHasWiring);
    }

    let mut routes: Vec<Route> = Vec::with_capacity(children.len());
    let mut clocks: Vec<Clock> = Vec::new();
    let mut files: Vec<FileHandler> = Vec::new();
    let mut mails: Vec<MailHandler> = Vec::new();
    let mut feeds: Vec<FeedHandler> = Vec::new();
    let mut queues: Vec<QueueHandler> = Vec::new();
    let mut actions: Vec<Action> = Vec::new();
    for (i, hash) in children.iter().enumerate() {
        let child = i as u32;
        let endpoint = resolver.resolve(hash).ok_or(DeclError::UnresolvedEndpoint { child })?;
        let log = log_level(endpoint).ok_or_else(|| DeclError::BadLogLevel { child, value: frozen(endpoint, "log").unwrap_or("").to_string() })?;
        if declaration_of(endpoint) == Some("every") {
            let seconds = read_clock(endpoint).ok_or(DeclError::BadEndpointShape { child })?;
            if clocks.iter().any(|c| c.seconds == seconds) {
                return Err(DeclError::DuplicateClock { seconds });
            }
            clocks.push(Clock { seconds, label: clock_label(seconds), handler: *hash, log });
            continue;
        }
        if declaration_of(endpoint) == Some("on_file") {
            let trigger = read_file_handler(endpoint).ok_or(DeclError::BadEndpointShape { child })?;
            if files.iter().any(|f| f.trigger == trigger) {
                return Err(DeclError::DuplicateFileHandler { label: trigger.label() });
            }
            files.push(FileHandler { label: trigger.label(), trigger, handler: *hash, log });
            continue;
        }
        if declaration_of(endpoint) == Some("on_mail") {
            let mail = read_mail_handler(endpoint).ok_or(DeclError::BadEndpointShape { child })?;
            if mails.iter().any(|m| m.mail == mail) {
                return Err(DeclError::DuplicateMailHandler { mail });
            }
            mails.push(MailHandler { label: mail_label(&mail), mail, handler: *hash, log });
            continue;
        }
        if declaration_of(endpoint) == Some("on_feed") {
            let feed = read_feed_handler(endpoint).ok_or(DeclError::BadEndpointShape { child })?;
            if feeds.iter().any(|f| f.feed == feed) {
                return Err(DeclError::DuplicateFeedHandler { feed });
            }
            feeds.push(FeedHandler { label: feed_label(&feed), feed, handler: *hash, log });
            continue;
        }
        if declaration_of(endpoint) == Some("on_queue") {
            let queue = read_queue_handler(endpoint).ok_or(DeclError::BadEndpointShape { child })?;
            if queues.iter().any(|q| q.queue == queue) {
                return Err(DeclError::DuplicateQueueHandler { queue });
            }
            queues.push(QueueHandler { label: queue_label(&queue), queue, handler: *hash, log });
            continue;
        }
        if declaration_of(endpoint) == Some("action") {
            let name = read_action(endpoint).ok_or(DeclError::BadEndpointShape { child })?;
            if actions.iter().any(|a| a.name == name) {
                return Err(DeclError::DuplicateAction { name });
            }
            actions.push(Action { label: format!("action {name}"), name, handler: *hash, log });
            continue;
        }
        let (method, path) = read_endpoint(endpoint).map_err(|e| match e {
            EndpointError::NotAnEndpoint => DeclError::NotAnEndpoint { child },
            EndpointError::BadMethod(method) => DeclError::BadMethod { child, method },
            EndpointError::BadPath(path) => DeclError::BadPath { child, path },
            EndpointError::BadShape => DeclError::BadEndpointShape { child },
        })?;
        let segments = parse_path(&path).ok_or_else(|| DeclError::BadPath { child, path: path.clone() })?;
        if routes.iter().any(|r| r.method == method && same_requests(&r.segments, &segments)) {
            return Err(DeclError::DuplicateRoute { method, path });
        }
        let resume = match frozen(endpoint, "resume") {
            None | Some("") => None,
            Some(_) => Some(crate::kind::ident(&endpoint.params, "resume").ok_or(DeclError::BadEndpointShape { child })?),
        };
        routes.push(Route { method, path, segments, endpoint: *hash, resume, log });
    }
    if routes.is_empty() && clocks.is_empty() && files.is_empty() && mails.is_empty() && feeds.is_empty() && queues.is_empty() && actions.is_empty() {
        return Err(DeclError::NoHandlers);
    }
    // A port only for a server with a route: a flow of clocks, files and actions listens on none.
    let port = match frozen(unit, "port") {
        None if routes.is_empty() => None,
        text => match text.unwrap_or("").parse::<u16>() {
            Ok(p) if p != 0 && !routes.is_empty() => Some(p),
            _ => return Err(DeclError::BadPort(text.unwrap_or("").to_string())),
        },
    };
    Ok(Server { port, routes, clocks, files, mails, feeds, queues, actions })
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
/// `-`, `_`, `.`, `~`) or params, written `:name` or `{name}` (the same param either way), the
/// last of them perhaps a rest, `*name`. `None` if malformed, including a repeated param name and
/// a rest anywhere but last.
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
    let parts: Vec<&str> = rest.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        let param = part.strip_prefix(':').or_else(|| part.strip_prefix('{').and_then(|p| p.strip_suffix('}')));
        let rest = part.strip_prefix('*').filter(|_| i + 1 == parts.len());
        match (param, rest) {
            (Some(name), _) | (None, Some(name)) if word(name) && !names.contains(&name) => {
                names.push(name);
                segments.push(if param.is_some() { Segment::Param(name.to_string()) } else { Segment::Rest(name.to_string()) });
            }
            (None, None) if word(part) => segments.push(Segment::Literal(part.to_string())),
            _ => return None,
        }
    }
    Some(segments)
}

/// Whether two route paths match exactly the same requests, so two routes of one method on them
/// clash: the same segments, every param alike (`/todos/:id` and `/todos/:key`), a rest alike
/// only a rest (`/a/*x` and `/a/*y`, never `/a/:x`).
pub fn same_requests(a: &[Segment], b: &[Segment]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|pair| match pair {
            (Segment::Literal(x), Segment::Literal(y)) => x == y,
            (Segment::Param(_), Segment::Param(_)) | (Segment::Rest(_), Segment::Rest(_)) => true,
            _ => false,
        })
}

/// Which of `paths` a request goes to, by its index, with its path params as the request wrote
/// them (a rest's segments joined by `/`): a path without a rest beats any path with one, then
/// the one with the most literal segments wins, so `/todos/new` beats `/todos/:id` and
/// `/docs/*page` beats `/*path`; ties go to the earlier path. `request_path` is the path only,
/// without the query string.
pub fn pick_path<'s>(paths: impl IntoIterator<Item = &'s [Segment]>, request_path: &str) -> Option<(usize, BTreeMap<String, String>)> {
    let parts: Vec<&str> = match request_path.strip_prefix('/')? {
        "" => vec![],
        rest => rest.split('/').collect(),
    };
    // How close a match is: no rest first, then the literals.
    type Closeness = (bool, usize);
    let mut best: Option<(usize, BTreeMap<String, String>, Closeness)> = None;
    for (i, segments) in paths.into_iter().enumerate() {
        let rest = matches!(segments.last(), Some(Segment::Rest(_)));
        let fits = if rest { parts.len() >= segments.len() } else { parts.len() == segments.len() };
        if !fits {
            continue;
        }
        let mut params = BTreeMap::new();
        let mut literals = 0;
        let matched = segments.iter().enumerate().all(|(at, seg)| match seg {
            Segment::Literal(l) => {
                literals += 1;
                l == parts[at]
            }
            Segment::Param(name) => {
                params.insert(name.clone(), parts[at].to_string());
                !parts[at].is_empty()
            }
            Segment::Rest(name) => {
                params.insert(name.clone(), parts[at..].join("/"));
                parts[at..].iter().all(|part| plain(part))
            }
        });
        let closeness = (!rest, literals);
        if matched && best.as_ref().is_none_or(|(_, _, c)| closeness > *c) {
            best = Some((i, params, closeness));
        }
    }
    best.map(|(i, params, _)| (i, params))
}

/// Whether a request's segment may be part of a rest: not empty, not `.` or `..` (`%2e` read as
/// `.`), and nothing in it that decodes to a separator (`%2f`, `%5c`, `\`, the last one's on
/// Windows), so a rest names only what the request's path said.
fn plain(part: &str) -> bool {
    let lower = part.to_ascii_lowercase();
    let dots = lower.replace("%2e", ".");
    !part.is_empty() && dots != "." && dots != ".." && !lower.contains("%2f") && !lower.contains("%5c") && !part.contains('\\')
}

/// The route a request should go to, with its path params, as [`pick_path`] picks among the
/// routes of its method. `request_path` is the path only, without the query string.
pub fn match_route<'a>(
    server: &'a Server,
    method: &str,
    request_path: &str,
) -> Option<(&'a Route, BTreeMap<String, String>)> {
    let routes: Vec<&Route> = server.routes.iter().filter(|r| r.method == method).collect();
    let (i, params) = pick_path(routes.iter().map(|r| r.segments.as_slice()), request_path)?;
    Some((routes[i], params))
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

    /// An endpoint over `handler`, with `log` as its log level param.
    fn endpoint_logged(r: &mut MapResolver, path: &str, handler: Unit, log: &str) -> Hash {
        let h = r.insert(handler).unwrap();
        r.insert(Unit {
            body: UnitBody::Composite {
                children: vec![h],
                edges: vec![
                    Edge { from_child: BOUNDARY, from_port: 0, to_child: 0, to_port: 0 },
                    Edge { from_child: 0, from_port: 0, to_child: BOUNDARY, to_port: 0 },
                ],
            },
            params: vec![Param::frozen("declare", "endpoint"), Param::frozen("method", "GET"), Param::frozen("path", path), Param::frozen("log", log)],
            inputs: vec![Port { name: "request".into(), ty: Type::Record }],
            outputs: vec![Port { name: "response".into(), ty: Type::Any }],
        })
        .unwrap()
    }

    #[test]
    fn a_handlers_log_level_is_read_as_written() {
        let mut r = MapResolver::new();
        let plain = endpoint(&mut r, "GET", "/a");
        assert_eq!(read_server(&server("8080", vec![plain]), &r).unwrap().routes[0].log, LogLevel::Strict, "strict without a param");
        for (word, level) in [("lazy", LogLevel::Lazy), ("unrecorded", LogLevel::Unrecorded), ("strict", LogLevel::Strict)] {
            let e = endpoint_logged(&mut r, "/a", handler(), word);
            assert_eq!(read_server(&server("8080", vec![e]), &r).unwrap().routes[0].log, level, "{word}");
        }
        let bad = endpoint_logged(&mut r, "/a", handler(), "verbose");
        assert_eq!(read_server(&server("8080", vec![bad]), &r), Err(DeclError::BadLogLevel { child: 0, value: "verbose".into() }));

        // A handler that writes state is as lazy as its author says: the level is authored, and
        // what an unrecorded one leaves out is the approval's to say.
        let writes = Unit { body: UnitBody::Primitive { name: "store_write".into() }, params: vec![Param::frozen("key", "k")], ..handler() };
        for (word, level) in [("lazy", LogLevel::Lazy), ("unrecorded", LogLevel::Unrecorded)] {
            let e = endpoint_logged(&mut r, "/a", writes.clone(), word);
            assert_eq!(read_server(&server("8080", vec![e]), &r).unwrap().routes[0].log, level, "{word}");
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
        assert_eq!(s.port, Some(8080));
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
        for path in ["a", "/a//b", "/a/:", "/a/:x/:x", "/a b", "/a?b", "/a/*", "/a/*x/b", "/*x/*y", "/:x/*x", "*x", "/a/**x"] {
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
    fn a_rest_takes_the_rest_of_the_path_and_gives_way_to_any_other() {
        let rest = |name: &str| Segment::Rest(name.into());
        assert_eq!(parse_path("/*path"), Some(vec![rest("path")]));
        assert_eq!(parse_path("/docs/:v/*page"), Some(vec![Segment::Literal("docs".into()), Segment::Param("v".into()), rest("page")]));

        let mut r = MapResolver::new();
        let paths = ["/api/hello", "/api/:name", "/api/*rest", "/docs/*page", "/*path", "/"];
        let hashes: Vec<Hash> = paths.iter().map(|p| endpoint(&mut r, "GET", p)).collect();
        let s = read_server(&server("80", hashes.clone()), &r).unwrap();
        let picked = |path: &str| match_route(&s, "GET", path).map(|(route, params)| (route.path.clone(), params.into_iter().collect::<Vec<_>>()));
        let one = |path: &str, name: &str, value: &str| Some((path.to_string(), vec![(name.to_string(), value.to_string())]));
        // Any route without a rest first, then the longest prefix.
        assert_eq!(picked("/api/hello"), Some(("/api/hello".into(), vec![])));
        assert_eq!(picked("/api/other"), one("/api/:name", "name", "other"));
        assert_eq!(picked("/api/a/b"), one("/api/*rest", "rest", "a/b"));
        assert_eq!(picked("/docs/a/b"), one("/docs/*page", "page", "a/b"));
        assert_eq!(picked("/docs"), one("/*path", "path", "docs"));
        assert_eq!(picked("/"), Some(("/".into(), vec![])));
        // As written: the host decodes a rest as it does every param.
        assert_eq!(picked("/css/site%20a.css"), one("/*path", "path", "css/site%20a.css"));
        // Never empty, and never a part that would mean another path: those match nothing here.
        for not in ["/docs/", "/docs//a", "/docs/a/", "/docs/..", "/docs/a/./b", "/docs/%2e%2E", "/docs/a%2Fb", "/docs/a%5cb", "/docs/a\\b", "//"] {
            assert_eq!(picked(not), None, "{not}: nor by /*path");
        }
        assert_eq!(picked("/.."), None);
        assert_eq!(picked("/docs/...").map(|(p, _)| p), Some("/docs/*page".into()), "three dots are a name");

        // Two rest routes clash on the same prefix, a rest never with a param.
        let x = endpoint(&mut r, "GET", "/a/*x");
        let y = endpoint(&mut r, "GET", "/a/*y");
        let param = endpoint(&mut r, "GET", "/a/:x");
        assert!(matches!(read_server(&server("80", vec![x, y]), &r), Err(DeclError::DuplicateRoute { .. })));
        assert!(read_server(&server("80", vec![x, param]), &r).is_ok());
        let other = endpoint(&mut r, "GET", "/:b/*x");
        assert!(read_server(&server("80", vec![x, other]), &r).is_ok());
    }

    fn clock(r: &mut MapResolver, seconds: &str) -> Hash {
        let h = r.insert(handler()).unwrap();
        r.insert(Unit {
            body: UnitBody::Composite { children: vec![h], edges: vec![] },
            params: vec![Param::frozen("declare", "every"), Param::frozen("seconds", seconds)],
            inputs: vec![],
            outputs: vec![],
        })
        .unwrap()
    }

    /// A server's clocks (its `every` blocks) are children beside its endpoints; a server of
    /// clocks alone has no port and listens for nothing (docs/L4s/triggers-L4.md §2, §4).
    #[test]
    fn a_server_holds_clocks_and_one_of_clocks_alone_has_no_port() {
        let mut r = MapResolver::new();
        let (two, hourly) = (clock(&mut r, "2"), clock(&mut r, "3600"));
        let alone = Unit { params: vec![Param::frozen("declare", "server")], ..server("", vec![two, hourly]) };
        let s = read_server(&alone, &r).unwrap();
        assert_eq!((s.port, s.routes.len()), (None, 0));
        assert_eq!(s.clocks.iter().map(|c| (c.seconds, c.label.as_str(), c.handler)).collect::<Vec<_>>(), [(2, "every 2s", two), (3600, "every 1h", hourly)]);
        assert!(!crate::capabilities(&alone, &r).contains(crate::Capability::NetworkIngress), "a server of clocks listens for nothing");

        let e = endpoint(&mut r, "GET", "/a");
        let both = server("8080", vec![e, two]);
        let s = read_server(&both, &r).unwrap();
        assert_eq!((s.port, s.routes.len(), s.clocks.len()), (Some(8080), 1, 1));
        assert!(crate::capabilities(&both, &r).contains(crate::Capability::NetworkIngress));

        // A port is a route's; one period per clock; a period is whole seconds, 1s to 366d.
        assert_eq!(read_server(&server("8080", vec![two]), &r), Err(DeclError::BadPort("8080".into())));
        assert_eq!(read_server(&server("80", vec![e, two, two]), &r), Err(DeclError::DuplicateClock { seconds: 2 }));
        for bad in ["0", "1.5", "x", "31622401"] {
            let c = clock(&mut r, bad);
            assert_eq!(read_server(&server("80", vec![e, c]), &r), Err(DeclError::BadEndpointShape { child: 1 }), "{bad}");
        }
        assert_eq!(clock_label(MAX_PERIOD), "every 366d");
        assert_eq!(read_server(&Unit { params: vec![Param::frozen("declare", "server")], ..server("", vec![]) }, &r), Err(DeclError::NoHandlers));
    }

    fn file_handler(r: &mut MapResolver, params: &[(&str, &str)]) -> Hash {
        let h = r.insert(handler()).unwrap();
        let mut ps = vec![Param::frozen("declare", "on_file")];
        ps.extend(params.iter().map(|(k, v)| Param::frozen(*k, *v)));
        r.insert(Unit { body: UnitBody::Composite { children: vec![h], edges: vec![] }, params: ps, inputs: vec![], outputs: vec![] }).unwrap()
    }

    fn action(r: &mut MapResolver, name: &str) -> Hash {
        let h = r.insert(handler()).unwrap();
        r.insert(Unit {
            body: UnitBody::Composite { children: vec![h], edges: vec![] },
            params: vec![Param::frozen("declare", "action"), Param::frozen("name", name)],
            inputs: vec![Port { name: "input".into(), ty: Type::Any }],
            outputs: vec![Port { name: "answer".into(), ty: Type::Record }],
        })
        .unwrap()
    }

    /// A server's actions (its `action` blocks) are children beside its endpoints, run by name;
    /// a server of actions alone has no port, and each action is a capability of its own.
    #[test]
    fn a_server_holds_actions_each_a_capability_of_its_own() {
        let mut r = MapResolver::new();
        let (reset, reindex) = (action(&mut r, "reset"), action(&mut r, "reindex"));
        let alone = Unit { params: vec![Param::frozen("declare", "server")], ..server("", vec![reset, reindex]) };
        let s = read_server(&alone, &r).unwrap();
        assert_eq!((s.port, s.routes.len()), (None, 0));
        assert_eq!(s.actions.iter().map(|a| (a.name.as_str(), a.label.as_str(), a.handler)).collect::<Vec<_>>(), [("reset", "action reset", reset), ("reindex", "action reindex", reindex)]);
        assert_eq!(s.action("reindex").map(|a| a.handler), Some(reindex));
        let caps = crate::capabilities(&alone, &r);
        assert!(caps.contains(crate::Capability::Action("reset".into())) && caps.contains(crate::Capability::Action("reindex".into())));
        assert!(!caps.contains(crate::Capability::NetworkIngress), "an action is never the network's");
        let e = endpoint(&mut r, "GET", "/a");
        assert_eq!(read_server(&server("80", vec![e, reset, reset]), &r), Err(DeclError::DuplicateAction { name: "reset".into() }));
        let bad = action(&mut r, "../x");
        assert_eq!(read_server(&server("80", vec![e, bad]), &r), Err(DeclError::BadEndpointShape { child: 1 }));
    }

    /// A server's file handlers (its `on file` blocks) are children beside its endpoints, each
    /// named by its line; one alone has no port (docs/L4s/triggers-L4.md §4).
    #[test]
    fn a_server_holds_file_handlers_each_named_by_its_line() {
        let mut r = MapResolver::new();
        let drop = file_handler(&mut r, &[("folder", "drop")]);
        let gone = file_handler(&mut r, &[("folder", "drop"), ("on", "deleted")]);
        let csv = file_handler(&mut r, &[("folder", "drop"), ("matching", "*.csv")]);
        let alone = Unit { params: vec![Param::frozen("declare", "server")], ..server("", vec![drop, gone, csv]) };
        let s = read_server(&alone, &r).unwrap();
        assert_eq!((s.port, s.routes.len(), s.clocks.len()), (None, 0, 0));
        let labels: Vec<&str> = s.files.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["on file drop", "on file drop deleted", "on file drop matching \"*.csv\""]);
        assert_eq!(s.files[1].trigger, FileTrigger { folder: "drop".into(), deleted: true, matching: String::new() });
        assert_eq!(s.files[0].handler, drop);

        let e = endpoint(&mut r, "GET", "/a");
        assert_eq!(read_server(&server("80", vec![e, drop]), &r).unwrap().files.len(), 1);
        // One handler per line; a folder is a name, `on` new or deleted, `matching` a glob.
        assert_eq!(read_server(&server("80", vec![e, drop, drop]), &r), Err(DeclError::DuplicateFileHandler { label: "on file drop".into() }));
        for bad in [&[("folder", "../x")][..], &[], &[("folder", "a"), ("on", "moved")], &[("folder", "a"), ("matching", "a**")]] {
            let f = file_handler(&mut r, bad);
            assert_eq!(read_server(&server("80", vec![e, f]), &r), Err(DeclError::BadEndpointShape { child: 1 }), "{bad:?}");
        }
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
