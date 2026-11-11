//! The printer: a flow document as canonical TypeScript (`NAME.holon.ts`, docs/LANGUAGE.md). The
//! other direction of ts.rs: what `holon fmt` writes, and how a canvas is exported as text.
//!
//! The imports first (`"holon"`'s words and leaves, the state, secrets, stores, hosts, the clock,
//! other flows' actions), then `flow("name", { … })`, then every function the flow places (inner
//! before outer, each once) and each `js(…)`, then the handlers (routes, clocks, file handlers,
//! actions) in the order of their nodes. Within a handler, nodes come in topological order; a
//! node whose one output has exactly one consumer prints inline, as a nested expression, at its
//! consumer; a node with several consumers, or several outputs, gets a `const`; an effect nobody
//! consumes prints as a statement, in the assignment form where there is one (`state.k = v`,
//! `notes[k] = v`, `delete notes[k]`); a write or a call tied to a value (`after`) goes in
//! `if (x !== undefined) { … }`; the answer is the handler's `return`, last. Each leaf prints in
//! its natural form when it has one (`a + b`, `xs.map(f)`, `JSON.parse(s)`, `notes[k]`), else as a
//! call by its catalog name, imported from "holon". The text is laid out by [`format`], each `js()`
//! body as the document has it.
//!
//! What TypeScript can't write is refused, with what to change: nodes under no handler, a group
//! whose request isn't a route's, a bound `state.set` outside a function, a match the reader
//! can't take back. The printer never changes the graph: `ts::compile(print(doc))` carries the
//! same unit hashes as `doc`, and printing is idempotent through compiling.
//!
//! [`format`]: crate::lang::format

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use holon_core::{family_values, function_takes, interface_of, MapResolver, Resolver, UnitBody, BOUNDARY, CALLS, PASSES_EXTRA, TRIGGER, VARIADIC};
use oxc_allocator::Allocator;
use oxc_ast::ast as js;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};

use crate::builder::{call_outputs, call_port, FlowDocument};
use crate::canvas::{Node, PlacedBody};
use crate::composites::{inner_canvas, resolve, Names};
use crate::lang::lower::{alias, call_noun, is_optional, RESERVED, UNUSED};
use crate::lang::ts::{DATE_GETTERS, DATE_SETTERS};
use crate::routes::{components, find_actions, find_clocks, find_feeds, find_files, find_mails, find_queues, find_routes, find_websockets};
use crate::templates;
use crate::text::restore;
use crate::wire::{WParam, WPort, WType};

/// JavaScript's words a name can't be, and the ones a flow's text keeps for what it isn't: a
/// binding, a function or an import never takes one.
const JS_RESERVED: &[&str] = &[
    "break", "case", "catch", "class", "const", "continue", "debugger", "default", "delete", "do", "else", "enum", "export", "extends", "false", "finally", "for", "function", "if", "import", "in",
    "instanceof", "new", "null", "return", "super", "switch", "this", "throw", "true", "try", "typeof", "var", "void", "while", "with", "implements", "interface", "let", "package", "private",
    "protected", "public", "static", "yield", "await", "async", "undefined", "arguments", "eval", "NaN", "Infinity", "JSON", "Object",
];

/// JavaScript's globals a flow doesn't have (ts.rs names them in a mistake), and the number ones
/// it reads as leaves (`Math.round`, `parseInt`): never a name here.
const GLOBALS: &[&str] = &[
    "fetch", "Date", "Math", "setTimeout", "setInterval", "process", "globalThis", "window", "console", "Promise", "require", "Number", "parseInt", "parseFloat", "isNaN", "isFinite",
    "String", "encodeURIComponent", "encodeURI", "decodeURIComponent", "decodeURI", "btoa", "atob",
];

/// `"holon"`'s own words: the top level's, the answers', and the waits'.
const VOCAB: &[&str] = &["flow", "route", "every", "on", "action", "js", "respond", "status", "stripe", "event", "approval"];

/// Precedence of a printed expression, for parentheses: higher binds tighter (to_ts.rs's).
const COND: u8 = 1;
const COALESCE: u8 = 2;
const OR: u8 = 3;
const AND: u8 = 4;
const BITOR: u8 = 5;
const BITXOR: u8 = 6;
const BITAND: u8 = 7;
const EQ: u8 = 8;
const REL: u8 = 9;
const SHIFT: u8 = 10;
const ADD: u8 = 11;
const MUL: u8 = 12;
const POW: u8 = 13;
const UNARY: u8 = 14;
const MEMBER: u8 = 15;
const ATOM: u8 = 16;

/// The frozen param that picks a text leaf's JavaScript method, where one does.
fn family_param(leaf: &str) -> &'static str {
    match leaf {
        "code_at" => "mode",
        "text_contains" => "at",
        "trim" | "pad" => "side",
        "replace" => "count",
        _ => "",
    }
}

/// `text` in parentheses when it binds less tightly than `min`.
fn wrap((text, power): (String, u8), min: u8) -> String {
    if power < min { format!("({text})") } else { text }
}

/// Whether `s` is a JavaScript identifier, as the printer writes them (ASCII).
fn is_identifier(s: &str) -> bool {
    !s.is_empty() && !s.starts_with(|c: char| c.is_ascii_digit()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// `s` made an identifier: `postman-echo` is `postman_echo`.
fn identifier(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) { format!("_{out}") } else { out }
}

/// Whether the lowering keeps `name` for itself, a leaf's or a keyword, so a TypeScript name for
/// it is known there with an underscore after it (ts.rs `local_name`).
fn taken(name: &str) -> bool {
    RESERVED.contains(&name) || interface_of(alias(name)).is_some() || templates::is_template(name)
}

/// The name the lowering knows a TypeScript binding `ts` by.
fn local_name(ts: &str) -> String {
    if taken(ts) { format!("{ts}_") } else { ts.to_string() }
}

/// The TypeScript name for a lowering's name: `json_`, the lowering's for `json`, is `json`.
fn ts_name(name: &str) -> String {
    match name.strip_suffix('_') {
        Some(word) if taken(word) => word.to_string(),
        _ => name.to_string(),
    }
}

/// The name a function's parameter goes by for its port: the port's, but `x` for a keyword's
/// `x_` (ts.rs `port_name` adds the underscore back).
fn param_name(port: &str) -> String {
    match port.strip_suffix('_') {
        Some(word) if RESERVED.contains(&word) && word != "in" && word != "out" => word.to_string(),
        _ => port.to_string(),
    }
}

/// How many inputs a higher-order leaf takes before the values its function uses from around
/// it: `fold` its start and the list, the others the list.
fn leaf_inputs(leaf: &str) -> usize {
    interface_of(leaf).map_or(1, |u| u.inputs.len())
}

/// The port ts.rs gives a parameter named `ts`.
fn port_of(ts: &str) -> String {
    if RESERVED.contains(&ts) && ts != "in" && ts != "out" { format!("{ts}_") } else { ts.to_string() }
}

/// A text in double quotes.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A text part of a template literal.
fn template_text(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => out.push_str("\\`"),
            '\\' => out.push_str("\\\\"),
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A record's key as written: bare when a word, else quoted (`"$set"` too, as the samples write
/// a document's operators).
fn key(k: &str) -> String {
    if is_identifier(k) && !k.contains('$') { k.to_string() } else { quote(k) }
}

/// `key: value`, or `key` alone when the value is the name the key says.
fn field(k: &str, v: &str) -> String {
    if is_identifier(k) && v == k { v.to_string() } else { format!("{}: {v}", key(k)) }
}

/// Whether `text` is a number as TypeScript writes one and ts.rs reads it back to the same text:
/// decimal digits, a fraction, an exponent, and a minus first; no leading zero before more digits.
fn is_number(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (mantissa, exponent) = match digits.find(['e', 'E']) {
        Some(at) => (&digits[..at], Some(&digits[at + 1..])),
        None => (digits, None),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (mantissa, None),
    };
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let whole_ok = all_digits(whole) && !(whole.len() > 1 && whole.starts_with('0'));
    let fraction_ok = fraction.is_none_or(|f| all_digits(f) && !(whole.is_empty() && f.is_empty()));
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && all_digits(e)
    });
    !mantissa.is_empty() && (whole.is_empty() || whole_ok) && fraction_ok && exponent_ok && text.parse::<f64>().is_ok_and(f64::is_finite)
}

/// A number literal with its precedence: a negative one is a unary minus.
fn number(text: &str) -> (String, u8) {
    (text.to_string(), if text.starts_with('-') { UNARY } else { ATOM })
}

/// A param's value as a call gives it: a number or a boolean bare, as the lowering reads either
/// back to the same text, else quoted.
fn param_value(v: &str) -> String {
    if is_number(v) || v == "true" || v == "false" { v.to_string() } else { quote(v) }
}

/// A JSON text as a TypeScript literal, when the lowering writes it back to the same text (keys
/// sorted, compact, numbers as they are); `None` otherwise, or for a `null`, which TypeScript has
/// no value for here.
fn json_literal(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if serde_json::to_string(&v).ok()? != text {
        return None;
    }
    fn render(v: &serde_json::Value) -> Option<String> {
        Some(match v {
            serde_json::Value::Null => return None,
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => {
                let t = n.to_string();
                if !is_number(&t) || n.as_f64().is_none_or(|f| f.abs() >= 1e15) {
                    return None;
                }
                t
            }
            serde_json::Value::String(s) => quote(s),
            serde_json::Value::Array(items) => format!("[{}]", items.iter().map(render).collect::<Option<Vec<_>>>()?.join(", ")),
            serde_json::Value::Object(fields) if fields.is_empty() => "{}".into(),
            serde_json::Value::Object(fields) => {
                let parts = fields.iter().map(|(k, v)| Some(format!("{}: {}", key(k), render(v)?))).collect::<Option<Vec<_>>>()?;
                format!("{{ {} }}", parts.join(", "))
            }
        })
    }
    render(&v)
}

/// A `const` or `default` literal from its frozen type and value, or `None` when it has no
/// literal TypeScript reads back the same.
fn literal(ty: &str, value: &str) -> Option<(String, u8)> {
    match ty {
        "Number" => is_number(value).then(|| number(value)),
        "Text" => Some((quote(value), ATOM)),
        "Boolean" => (value == "true" || value == "false").then(|| (value.to_string(), ATOM)),
        "Json" => json_literal(value).map(|l| (l, ATOM)),
        _ => None,
    }
}

/// A limit's value as `flow(…)` takes it: a duration in quotes for one counted in milliseconds or
/// seconds, when it is a whole number of seconds, else the number.
fn limit_value(name: &str, value: u64) -> String {
    let seconds = if name.ends_with("_ms") && value.is_multiple_of(1000) && value > 0 {
        value / 1000
    } else if name.ends_with("_seconds") && value > 0 {
        value
    } else {
        return value.to_string();
    };
    quote(&duration(seconds))
}

/// A number of seconds as a duration: `90` is `90s`, `3600` is `1h`.
fn duration(seconds: u64) -> String {
    [(86400, 'd'), (3600, 'h'), (60, 'm')].iter().find(|(unit, _)| seconds.is_multiple_of(*unit)).map(|(unit, c)| format!("{}{c}", seconds / unit)).unwrap_or_else(|| format!("{seconds}s"))
}

/// A handler's options as its call's last argument: `, { resumes: "paid", log: "lazy" }`, the
/// log level only when it isn't strict, the default.
fn options(options: &[(&str, &str)]) -> String {
    let set: Vec<String> = options.iter().filter(|(_, v)| !v.is_empty()).map(|(k, v)| format!("{k}: {}", quote(v))).collect();
    if set.is_empty() { String::new() } else { format!(", {{ {} }}", set.join(", ")) }
}

/// A log level as a handler's option: nothing for strict.
fn log_option(log: &str) -> &'static str {
    match holon_core::LogLevel::parse(log) {
        Some(holon_core::LogLevel::Strict) | None => "",
        Some(level) => level.word(),
    }
}

/// The secret an optional consent names (`SecretRead("receipt_key")`), when the text can import
/// it optional; any other consent has no TypeScript form.
fn optional_secret(o: &str) -> Option<&str> {
    o.strip_prefix("SecretRead(\"").and_then(|rest| rest.strip_suffix("\")")).filter(|name| is_identifier(name))
}

/// Whether a match's pattern value prints as a literal TypeScript reads back to the same pattern.
fn pattern_literal(ty: &str, value: &str) -> Option<String> {
    match ty {
        "Number" | "Text" | "Boolean" => literal(ty, value).map(|(t, _)| t),
        _ => None,
    }
}

/// A value's source within a block.
#[derive(Clone, Debug, PartialEq)]
enum Src {
    Node { id: String, output: String },
    /// A function's input, by its port.
    In(String),
}

/// A wire within a block: its source, and the node and input it feeds.
type BlockWire = (Src, String, String);

/// A connected set of nodes printed as one block: a handler's, or a function's.
struct Group<'g> {
    nodes: Vec<&'g Node>,
    incoming: HashMap<(String, String), Src>,
    consumers: HashMap<(String, String), usize>,
    /// Boundary outputs: `(node, output, port)`.
    outs: Vec<(String, String, String)>,
    order: Vec<String>,
    /// The nodes tied to their block's trigger, which the text doesn't write.
    block_tied: HashSet<String>,
}

impl<'g> Group<'g> {
    fn new(nodes: Vec<&'g Node>, wires: Vec<BlockWire>, outs: Vec<(String, String, String)>) -> Group<'g> {
        let (nodes, wires, block_tied) = without_block_ties(nodes, wires);
        let ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        let mut incoming = HashMap::new();
        let mut consumers: HashMap<(String, String), usize> = HashMap::new();
        let mut edges: Vec<(String, String)> = Vec::new();
        for (src, to, input) in wires {
            if !ids.contains(to.as_str()) {
                continue;
            }
            if let Src::Node { id, output } = &src {
                if !ids.contains(id.as_str()) {
                    continue;
                }
                *consumers.entry((id.clone(), output.clone())).or_default() += 1;
                edges.push((id.clone(), to.clone()));
            }
            incoming.insert((to, input), src);
        }
        for (id, output, _) in &outs {
            *consumers.entry((id.clone(), output.clone())).or_default() += 1;
        }
        // Kahn's order, ties by the nodes' own order.
        let index: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
        let mut indeg = vec![0usize; nodes.len()];
        let mut succ: Vec<Vec<usize>> = vec![vec![]; nodes.len()];
        for (from, to) in &edges {
            let (f, t) = (index[from.as_str()], index[to.as_str()]);
            indeg[t] += 1;
            succ[f].push(t);
        }
        let mut ready: Vec<usize> = (0..nodes.len()).filter(|&i| indeg[i] == 0).collect();
        let mut order = Vec::new();
        while !ready.is_empty() {
            ready.sort_unstable_by(|a, b| b.cmp(a));
            let i = ready.pop().unwrap();
            order.push(nodes[i].id.clone());
            for &s in &succ[i] {
                indeg[s] -= 1;
                if indeg[s] == 0 {
                    ready.push(s);
                }
            }
        }
        Group { nodes, incoming, consumers, outs, order, block_tied }
    }

    fn node(&self, id: &str) -> &'g Node {
        self.nodes.iter().find(|n| n.id == id).expect("a node of the group")
    }

    fn consumed(&self, id: &str, output: &str) -> usize {
        self.consumers.get(&(id.to_string(), output.to_string())).copied().unwrap_or(0)
    }

    fn consumed_total(&self, node: &Node) -> usize {
        node.data.outputs.iter().map(|o| self.consumed(&node.id, &o.name)).sum()
    }

    /// Who reads `id`'s outputs: each consumer's node and input.
    fn uses(&self, id: &str) -> Vec<(&String, &String)> {
        self.incoming.iter().filter(|(_, src)| matches!(src, Src::Node { id: s, .. } if s == id)).map(|((to, input), _)| (to, input)).collect()
    }
}

/// `nodes` and `wires` without the ties the text doesn't write: a trigger fed by its block's
/// trigger (a route's request, a clock's tick, a file handler's file, an action's input, a
/// function's own `trigger`, or the pass-through the lowering places when that feeds several,
/// which goes too, with its `true`). What's left on a trigger is written in an
/// `if (x !== undefined)`. Also the nodes whose ties went.
fn without_block_ties(nodes: Vec<&Node>, wires: Vec<BlockWire>) -> (Vec<&Node>, Vec<BlockWire>, HashSet<String>) {
    let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), *n)).collect();
    // The last wire into an input is the one that counts, as `Group` keeps it: a holon's
    // boundary is listed as its node first, then as the port.
    let into = |to: &str, input: &str| wires.iter().rev().find(|(_, t, i)| t == to && i == input).map(|(s, _, _)| s);
    let own_trigger = |src: &Src| match src {
        Src::In(p) => p == TRIGGER,
        Src::Node { id, output } => id == crate::composites::BOUNDARY_IN && output == TRIGGER,
    };
    let passes_trigger = |id: &str| {
        let Some(n) = by_id.get(id) else { return false };
        let always = matches!(into(id, "cond"), Some(Src::Node { id: c, .. }) if by_id.get(c.as_str()).is_some_and(|c| c.data.name == "const" && c.param("type") == "Boolean" && c.param("value") == "true"));
        n.data.name == "guard" && always && into(id, "value").is_some_and(own_trigger)
    };
    let tie = |src: &Src, input: &str| {
        input == TRIGGER
            && (own_trigger(src)
                || matches!(src, Src::Node { id, output } if (output == "request" && by_id.get(id.as_str()).is_some_and(|n| n.is_template("request") || n.is_template("websocket"))) || (output == "now" && by_id.get(id.as_str()).is_some_and(|n| n.is_template("every"))) || (output == "file" && by_id.get(id.as_str()).is_some_and(|n| n.is_template("on_file"))) || (output == "message" && by_id.get(id.as_str()).is_some_and(|n| n.is_template("on_mail") || n.is_template("on_queue"))) || (output == "event" && by_id.get(id.as_str()).is_some_and(|n| n.is_template("on_feed"))) || (output == "input" && by_id.get(id.as_str()).is_some_and(|n| n.is_template("action"))) || passes_trigger(id)))
    };
    // A pass-through that feeds only ties goes, with the `true` it holds.
    let mut gone: HashSet<String> = HashSet::new();
    for n in &nodes {
        let feeds: Vec<&BlockWire> = wires.iter().filter(|(s, _, _)| matches!(s, Src::Node { id, .. } if *id == n.id)).collect();
        if passes_trigger(&n.id) && !feeds.is_empty() && feeds.iter().all(|(_, _, input)| input == TRIGGER) {
            gone.insert(n.id.clone());
            if let Some(Src::Node { id: c, .. }) = into(&n.id, "cond")
                && wires.iter().filter(|(s, _, _)| matches!(s, Src::Node { id, .. } if id == c)).count() == 1
            {
                gone.insert(c.clone());
            }
        }
    }
    let touches_gone = |(src, to, _): &BlockWire| gone.contains(to) || matches!(src, Src::Node { id, .. } if gone.contains(id));
    let kept: Vec<BlockWire> = wires.iter().filter(|w| !tie(&w.0, &w.2) && !touches_gone(w)).cloned().collect();
    let tied: HashSet<String> = wires.iter().filter(|w| tie(&w.0, &w.2)).map(|w| w.1.clone()).collect();
    (nodes.into_iter().filter(|n| !gone.contains(&n.id)).collect(), kept, tied)
}

/// The first input TypeScript can't write as wired: one a node needs and nothing feeds (the
/// lowering refuses one left out), or a request node that isn't the handler's own.
fn unwritable(nodes: &[&Node], incoming: &HashMap<(String, String), Src>, request: Option<&str>) -> Option<String> {
    for n in nodes {
        if request == Some(n.id.as_str()) {
            continue;
        }
        if n.is_template("request") {
            return Some(format!("the request of {} {} is wired into another handler: TypeScript writes one request per route, so give each its own nodes", n.param("method"), n.param("path")));
        }
        for p in &n.data.inputs {
            let optional = n.data.body == PlacedBody::Primitive && is_optional(&n.data.name, &p.name);
            if !(optional || p.name == TRIGGER || (n.is_template("respond") && p.name == "body") || incoming.contains_key(&(n.id.clone(), p.name.clone()))) {
                return Some(format!("the input {} of {} isn't wired: TypeScript passes every input a node needs, so wire it", p.name, n.data.name));
            }
        }
    }
    None
}

/// A statement written in a block: its text (several lines for an `if` or a `switch`), the
/// nodes it stands for, and the statements inside it, each on a line of its own.
#[derive(Clone, Debug, Default)]
struct Stmt {
    text: String,
    nodes: Vec<String>,
    kids: Vec<Stmt>,
    /// The collection or folder a write in the assignment form writes: a later read of it is
    /// written before it, as ts.rs refuses a read after one.
    writes: Option<String>,
    /// The collections and folders it reads.
    reads: BTreeSet<String>,
    /// What it waits for (`after`): it goes in `if (x !== undefined)`.
    after: Option<String>,
    /// The `let` declared before that `if` for the value it gives inside.
    declares: Option<String>,
}

/// A top-level item after `flow(…)`: a function, a `js(…)`, a handler; with its trigger's node
/// (on its first line) and its body's statements, for the lines.
struct Item {
    text: String,
    nodes: Vec<String>,
    body: Vec<Stmt>,
    /// A websocket's blocks, each its trigger and its statements, in the order written.
    blocks: Vec<(String, Vec<Stmt>)>,
}

pub struct Printer<'s> {
    store: &'s MapResolver,
    /// The flow's own name: a `flow_start` of its own action is `payment(x)`.
    own: Option<String>,
    names: Names,
    bindings: BTreeMap<String, String>,
    collections: BTreeSet<String>,
    folders: BTreeSet<String>,
    databases: BTreeSet<String>,
    channels: BTreeSet<String>,
    mails: BTreeSet<String>,
    /// The feeds its handlers listen to.
    feeds: BTreeSet<String>,
    /// The queues its handlers listen to or its runs send to.
    queues: BTreeSet<String>,
    /// The flows any `flow_call` or `flow_start` runs an action of, by slug.
    calls: BTreeSet<String>,
    secrets: BTreeSet<String>,
    state: BTreeSet<String>,
    /// The folders file handlers watch, in the handlers' order.
    watched: Vec<String>,
    /// The flow's own actions, by name.
    actions: BTreeSet<String>,
    /// Each function's or `js(…)`'s name, by hash, once written.
    def_names: HashMap<String, String>,
    /// The names the first pass gave them, so the second gives the same.
    seeded: HashMap<String, String>,
    defs: Vec<Item>,
    // What the text uses, imported at its head; gathered while writing.
    vocab: BTreeSet<String>,
    /// Leaves called by their catalog name.
    leaves: BTreeSet<String>,
    uses_state: bool,
    uses_now: bool,
    uses_random: bool,
    uses_random_bytes: bool,
    /// Hosts requested, in the order first met.
    hosts: Vec<String>,
    /// Each flow's actions called or started, by slug.
    flow_actions: BTreeMap<String, BTreeSet<String>>,
    // The names they are imported by, settled between the two passes.
    leaf_local: HashMap<String, String>,
    secret_local: HashMap<String, String>,
    host_local: HashMap<String, String>,
    action_local: HashMap<(String, String), String>,
    /// Every name a binding, a parameter or a function stays clear of.
    avoid: HashSet<String>,
    /// `"holon"`'s words the first pass found the text uses.
    vocab_used: Option<BTreeSet<String>>,
}

/// A flow document as canonical TypeScript. `known` names composites the document doesn't;
/// `bindings` names top-level nodes (a lowering's binding names, so a text keeps its names
/// through `fmt`). The document's units are put into `store` first.
pub fn print(name: Option<&str>, doc: &FlowDocument, store: &mut MapResolver, known: &Names, bindings: &BTreeMap<String, String>) -> Result<String, String> {
    print_lines(name, doc, store, known, bindings).map(|(text, _)| text)
}

/// [`print`], and the line (1-based) each top-level node is written on: a handler's trigger's is
/// its call's first line, an inline node's is its consumer's statement's, a statement's own is
/// where it starts (a `switch`'s case values, the case's). What view mode shows beside a selected
/// node.
pub fn print_lines(name: Option<&str>, doc: &FlowDocument, store: &mut MapResolver, known: &Names, bindings: &BTreeMap<String, String>) -> Result<(String, BTreeMap<String, u32>), String> {
    let names = restore(doc, store, known);
    let mut p = Printer {
        store,
        own: name.filter(|n| !n.is_empty()).map(String::from),
        names,
        bindings: bindings.clone(),
        collections: BTreeSet::new(),
        folders: BTreeSet::new(),
        databases: BTreeSet::new(),
        channels: BTreeSet::new(),
        mails: BTreeSet::new(),
        feeds: BTreeSet::new(),
        queues: BTreeSet::new(),
        calls: BTreeSet::new(),
        secrets: BTreeSet::new(),
        state: BTreeSet::new(),
        watched: Vec::new(),
        actions: BTreeSet::new(),
        def_names: HashMap::new(),
        seeded: HashMap::new(),
        defs: Vec::new(),
        vocab: BTreeSet::new(),
        leaves: BTreeSet::new(),
        uses_state: false,
        uses_now: false,
        uses_random: false,
        uses_random_bytes: false,
        hosts: Vec::new(),
        flow_actions: BTreeMap::new(),
        leaf_local: HashMap::new(),
        secret_local: HashMap::new(),
        host_local: HashMap::new(),
        action_local: HashMap::new(),
        avoid: HashSet::new(),
        vocab_used: None,
    };
    p.collect_collections(&doc.nodes)?;
    // A mail a handler reads is imported as one it sends through is.
    p.mails.extend(doc.nodes.iter().filter(|n| n.is_template("on_mail")).map(|n| n.param("mail").to_string()));
    p.feeds.extend(doc.nodes.iter().filter(|n| n.is_template("on_feed")).map(|n| n.param("feed").to_string()));
    p.queues.extend(doc.nodes.iter().filter(|n| n.is_template("on_queue")).map(|n| n.param("queue").to_string()));
    for n in doc.nodes.iter().filter(|n| n.is_template("on_file")) {
        let folder = n.param("folder").to_string();
        if !p.watched.contains(&folder) {
            p.watched.push(folder);
        }
    }
    p.actions = doc.nodes.iter().filter(|n| n.is_template("action")).map(|n| n.param("name").to_string()).collect();
    let optional: Vec<String> = doc.server.as_ref().and_then(|s| s.requirements.as_ref()).and_then(|r| r.optional.clone()).unwrap_or_default();
    if let Some(o) = optional.iter().find(|o| optional_secret(o).is_none()) {
        return Err(format!("the consent {o} has no TypeScript form: only a secret is imported optional, with {{ optional: \"true\" }}"));
    }
    let optional: BTreeSet<String> = optional.iter().filter_map(|o| optional_secret(o)).map(String::from).collect();
    p.secrets.extend(optional.iter().cloned());
    p.check_names()?;
    // The first pass names the functions and finds what is imported; the second writes it all
    // with the names the imports settle on, which every binding stays clear of.
    p.avoid = p.fixed_names();
    p.items(doc)?;
    p.seeded = std::mem::take(&mut p.def_names);
    p.vocab_used = Some(p.vocab.clone());
    p.settle_imports();
    p.defs.clear();
    p.vocab.clear();
    p.leaves.clear();
    p.hosts.clear();
    p.flow_actions.clear();
    (p.uses_state, p.uses_now, p.uses_random, p.uses_random_bytes) = (false, false, false, false);
    p.avoid = p.fixed_names();
    p.avoid.extend(p.seeded.values().cloned());
    p.avoid.extend(p.leaf_local.values().cloned());
    p.avoid.extend(p.secret_local.values().cloned());
    p.avoid.extend(p.host_local.values().cloned());
    p.avoid.extend(p.action_local.values().cloned());
    let items = p.items(doc)?;
    let settings = p.settings(doc)?;
    let defs = std::mem::take(&mut p.defs);
    let mut raw: Vec<String> = p.imports(&optional);
    raw.push(String::new());
    raw.push(settings);
    for item in defs.iter().chain(&items) {
        raw.push(String::new());
        raw.push(item.text.clone());
    }
    let raw = raw.join("\n") + "\n";
    let text = crate::lang::format::format_keeping_bodies(&raw).map_err(|e| format!("the TypeScript written for this flow doesn't read back ({e}), which is the printer's mistake:\n{raw}"))?;
    let lines = lines_of(&text, &defs, &items);
    Ok((text, lines))
}

/// Each node's line in `text`, as `items` (after the imports, `flow(…)` and `defs`) wrote them:
/// read from the text as laid out, statement by statement.
fn lines_of(text: &str, defs: &[Item], items: &[Item]) -> BTreeMap<String, u32> {
    let mut lines = BTreeMap::new();
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, SourceType::ts()).parse();
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line = |at: u32| starts.partition_point(|s| *s <= at as usize) as u32;
    let top: Vec<&js::Statement> = parsed.program.body.iter().filter(|s| !matches!(s, js::Statement::ImportDeclaration(_))).skip(1).skip(defs.len()).collect();
    for (item, s) in items.iter().zip(top) {
        let at = line(s.span().start);
        for n in &item.nodes {
            lines.entry(n.clone()).or_insert(at);
        }
        if !item.blocks.is_empty() {
            // A websocket: each block's trigger on its key's line, its statements on theirs.
            let written = socket_blocks_of(s);
            for (k, (trigger, body)) in item.blocks.iter().enumerate() {
                match written.get(k).filter(|_| written.len() == item.blocks.len()) {
                    Some((key, f)) => {
                        lines.entry(trigger.clone()).or_insert(line(*key));
                        arrow_lines(f, body, line(*key), &line, &mut lines);
                    }
                    None => {
                        lines.entry(trigger.clone()).or_insert(at);
                        body.iter().for_each(|st| place(st, at, &mut lines));
                    }
                }
            }
            continue;
        }
        let Some(handler) = handler_of(s) else { continue };
        arrow_lines(handler, &item.body, at, &line, &mut lines);
    }
    lines
}

/// Each of a handler's statements' nodes on its line in the arrow `f` as laid out.
fn arrow_lines(f: &js::ArrowFunctionExpression, body: &[Stmt], at: u32, line: &dyn Fn(u32) -> u32, lines: &mut BTreeMap<String, u32>) {
    match &f.body {
        js::ArrowFunctionBody::FunctionBody(b) => walk(body, &b.statements, at, line, lines),
        // An arrow whose body is its answer: every node is on that answer's line.
        answer => {
            let at = answer.as_expression().map(|e| line(e.span().start)).unwrap_or(at);
            for st in body {
                place(st, at, lines);
            }
        }
    }
}

/// A websocket's blocks as laid out, `route.websocket(path, { open: (req) => … })`: where each
/// key starts, and its arrow.
fn socket_blocks_of<'a>(s: &'a js::Statement<'a>) -> Vec<(u32, &'a js::ArrowFunctionExpression<'a>)> {
    let js::Statement::ExpressionStatement(e) = s else { return vec![] };
    let js::Expression::CallExpression(c) = &e.expression else { return vec![] };
    let Some(js::Expression::ObjectExpression(o)) = c.arguments.get(1).and_then(|a| a.as_expression()) else { return vec![] };
    o.properties
        .iter()
        .filter_map(|p| match p {
            js::ObjectPropertyKind::ObjectProperty(p) => match &p.value {
                js::Expression::ArrowFunctionExpression(f) => Some((p.span.start, &**f)),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The function a handler's statement passes: its last argument but the options, an arrow.
fn handler_of<'a>(s: &'a js::Statement<'a>) -> Option<&'a js::ArrowFunctionExpression<'a>> {
    let call = match s {
        js::Statement::ExpressionStatement(e) => match &e.expression {
            js::Expression::CallExpression(c) => c,
            _ => return None,
        },
        js::Statement::ExportDeclaration(e) => match &e.declaration {
            js::Declaration::VariableDeclaration(v) => match v.declarations.first().and_then(|d| d.init.as_ref()) {
                Some(js::Expression::CallExpression(c)) => c,
                _ => return None,
            },
            _ => return None,
        },
        _ => return None,
    };
    call.arguments.iter().find_map(|a| match a.as_expression() {
        Some(js::Expression::ArrowFunctionExpression(f)) => Some(&**f),
        _ => None,
    })
}

/// Each of `stmts`' nodes on its statement's line, and those inside on theirs, `statements` being
/// the text's; where the two don't pair up, on `parent`'s line.
fn walk(stmts: &[Stmt], statements: &[js::Statement], parent: u32, line: &dyn Fn(u32) -> u32, lines: &mut BTreeMap<String, u32>) {
    if stmts.len() != statements.len() {
        for s in stmts {
            place(s, parent, lines);
        }
        return;
    }
    for (s, st) in stmts.iter().zip(statements) {
        let at = line(st.span().start);
        for n in &s.nodes {
            lines.entry(n.clone()).or_insert(at);
        }
        match st {
            js::Statement::IfStatement(i) => {
                let inside: &[js::Statement] = match &i.consequent {
                    js::Statement::BlockStatement(b) => &b.body,
                    one => std::slice::from_ref(one),
                };
                walk(&s.kids, inside, at, line, lines);
            }
            js::Statement::SwitchStatement(w) => {
                let cases: Vec<&js::SwitchCase> = w.cases.iter().filter(|c| !c.consequent.is_empty()).collect();
                if cases.len() != s.kids.len() {
                    s.kids.iter().for_each(|k| place(k, at, lines));
                    continue;
                }
                for (k, c) in s.kids.iter().zip(cases) {
                    let at = line(c.consequent[0].span().start);
                    place(k, at, lines);
                }
            }
            _ => s.kids.iter().for_each(|k| place(k, at, lines)),
        }
    }
}

/// `s`'s nodes, and its statements', all on line `at`.
fn place(s: &Stmt, at: u32, lines: &mut BTreeMap<String, u32>) {
    for n in &s.nodes {
        lines.entry(n.clone()).or_insert(at);
    }
    for k in &s.kids {
        place(k, at, lines);
    }
}

/// Whether `unit` is a `js` node's composite: one child is a module (the engine), another the
/// `const` holding its source.
pub(crate) fn is_js_node(store: &MapResolver, unit: &holon_core::Unit) -> bool {
    js_parts(store, unit).is_some()
}

/// A `js` node's engine unit and source text, when `unit` is one.
fn js_parts<'s>(store: &'s MapResolver, unit: &holon_core::Unit) -> Option<(&'s holon_core::Unit, String)> {
    let UnitBody::Composite { children, .. } = &unit.body else { return None };
    let mut engine: Option<&holon_core::Unit> = None;
    let mut source: Option<String> = None;
    for c in children {
        let child = store.resolve(c)?;
        match &child.body {
            UnitBody::Module { .. } => engine = Some(child),
            UnitBody::Primitive { name } if name == "const" => {
                let is_text = child.params.iter().any(|p| matches!(p, holon_core::Param::Frozen { name, value } if name == "type" && value == "Text"));
                if is_text {
                    source = child.params.iter().find_map(|p| match p {
                        holon_core::Param::Frozen { name, value } if name == "value" => Some(value.clone()),
                        _ => None,
                    });
                }
            }
            _ => {}
        }
    }
    let (engine, source) = (engine?, source?);
    source.contains(") => {").then_some((engine, source))
}

/// A js node's body and budgets as `js(function out(a, b) { … }, { fuel, memory })` writes them:
/// the body verbatim, each budget only where it differs from the default.
fn js_parts_ts(store: &MapResolver, unit: &holon_core::Unit) -> Option<(String, Option<u64>, Option<u64>)> {
    let (engine, source) = js_parts(store, unit)?;
    let open = source.find(") => {")?;
    let body = source[open + ") => {".len()..source.len().saturating_sub(1)].to_string();
    let budget = |name: &str, default: u64| -> Option<u64> {
        engine.params.iter().find_map(|p| match p {
            holon_core::Param::Frozen { name: n, value } if n == name => value.parse::<u64>().ok().filter(|v| *v != default),
            _ => None,
        })
    };
    Some((body, budget(holon_core::FUEL_PARAM, holon_module::JS_DEFAULT_FUEL), budget(holon_core::MEMORY_PARAM, holon_module::JS_DEFAULT_MEMORY)))
}

impl<'s> Printer<'s> {
    /// Every collection, folder, database, channel, called flow, secret, state key and mail any
    /// leaf names, the functions' leaves included.
    fn collect_collections(&mut self, nodes: &[Node]) -> Result<(), String> {
        /// The param naming what a leaf reads or writes: a collection's, a folder's, a
        /// database's or a channel's (a piped call's `pipe`), a flow's, a secret's, a state key, or
        /// a mail's.
        fn named_by(leaf: &str) -> Option<&'static str> {
            if leaf == "secret" {
                Some("name")
            } else if leaf == "store_read" || leaf == "store_write" {
                Some("key")
            } else if leaf.starts_with("collection_") {
                Some("collection")
            } else if leaf.starts_with("file_") && leaf != "file_arrival" {
                Some("folder")
            } else if leaf.starts_with("db_") {
                Some("database")
            } else if leaf.starts_with("channel_") {
                Some("channel")
            } else if leaf == "mail_send" {
                Some("mail")
            } else if leaf == "queue_send" {
                Some("queue")
            } else if leaf == "http_stream" {
                // A piped call's channel (empty when it isn't piped).
                Some("pipe")
            } else if leaf == "flow_call" || leaf == "flow_start" {
                Some("flow")
            } else {
                None
            }
        }
        type Found = (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>);
        fn found(out: &mut Found, param: &str, name: String) {
            if !name.is_empty() {
                match param {
                    "collection" => &mut out.0,
                    "folder" => &mut out.1,
                    "database" => &mut out.2,
                    "flow" => &mut out.4,
                    "name" => &mut out.5,
                    "key" => &mut out.6,
                    "mail" => &mut out.7,
                    "queue" => &mut out.8,
                    _ => &mut out.3,
                }
                .insert(name);
            }
        }
        fn walk(store: &MapResolver, hash: &holon_core::Hash, seen: &mut HashSet<holon_core::Hash>, out: &mut Found) {
            if !seen.insert(*hash) {
                return;
            }
            let Some(unit) = store.resolve(hash) else { return };
            match &unit.body {
                UnitBody::Primitive { name } if named_by(name).is_some() => {
                    let param = named_by(name).expect("checked");
                    if let Some(c) = unit.params.iter().find_map(|p| match p {
                        holon_core::Param::Frozen { name, value } if name == param => Some(value.clone()),
                        _ => None,
                    }) {
                        found(out, param, c);
                    }
                }
                UnitBody::Composite { children, .. } => {
                    for c in children {
                        walk(store, c, seen, out);
                    }
                }
                _ => {}
            }
        }
        let mut seen = HashSet::new();
        let mut out: Found = Default::default();
        for n in nodes {
            match &n.data.body {
                PlacedBody::Primitive if named_by(&n.data.name).is_some() => {
                    let param = named_by(&n.data.name).expect("checked");
                    found(&mut out, param, n.param(param).to_string());
                }
                PlacedBody::Composite { hash } => {
                    let (h, _) = resolve(self.store, hash)?;
                    walk(self.store, &h, &mut seen, &mut out);
                }
                _ => {}
            }
        }
        (self.collections, self.folders, self.databases, self.channels, self.calls, self.secrets, self.state, self.mails, self.queues) = out;
        // A start of its own action calls no other flow.
        if let Some(own) = &self.own {
            self.calls.remove(own);
        }
        Ok(())
    }

    /// What the text imports by name must be a name TypeScript writes: each store, secret, state
    /// key and action.
    fn check_names(&self) -> Result<(), String> {
        let stores = [("collection", &self.collections), ("folder", &self.folders), ("database", &self.databases), ("channel", &self.channels), ("mail", &self.mails), ("feed", &self.feeds), ("queue", &self.queues)];
        for (kind, names) in stores {
            for n in names.iter().chain(if kind == "folder" { self.watched.iter().collect::<Vec<_>>() } else { vec![] }) {
                if !is_identifier(n) || JS_RESERVED.contains(&n.as_str()) || VOCAB.contains(&n.as_str()) || ["state", "now", "random", "random_bytes"].contains(&n.as_str()) {
                    return Err(format!("the {kind} \"{n}\" has no TypeScript form: TypeScript imports it by its name, so name it as a word (letters, digits, _)"));
                }
            }
        }
        for s in &self.secrets {
            if !is_identifier(s) {
                return Err(format!("the secret \"{s}\" has no TypeScript form: name it as a word (letters, digits, _)"));
            }
        }
        for k in &self.state {
            if !is_identifier(k) {
                return Err(format!("the state key \"{k}\" has no TypeScript form: state.key takes a word (letters, digits, _)"));
            }
        }
        for a in &self.actions {
            if !is_identifier(a) || JS_RESERVED.contains(&a.as_str()) {
                return Err(format!("the action \"{a}\" has no TypeScript form: an action is exported by its name, a word (letters, digits, _)"));
            }
        }
        Ok(())
    }

    /// The names a binding, a parameter or a function never takes, whatever is imported: what
    /// JavaScript keeps, `"holon"`'s words, the stores, the flow's actions.
    fn fixed_names(&self) -> HashSet<String> {
        // `"holon"`'s words the file imports, once the first pass has found which (a function
        // never takes one, whichever).
        let vocab = VOCAB.iter().filter(|w| **w == "flow" || self.vocab_used.as_ref().is_some_and(|used| used.contains(**w)));
        let mut out: HashSet<String> = JS_RESERVED.iter().chain(GLOBALS).chain(vocab).chain(&["state", "now", "random", "random_bytes"]).map(|s| s.to_string()).collect();
        out.extend(self.collections.iter().chain(&self.folders).chain(&self.databases).chain(&self.channels).chain(&self.mails).chain(&self.feeds).chain(&self.queues).chain(&self.watched).cloned());
        out.extend(self.actions.iter().cloned());
        out.extend(self.calls.iter().map(|f| call_noun(f)));
        out
    }

    /// The names the imports go by, once the first pass found them: each its own, but where it
    /// would clash with another name of the file (a function's, a store's, another import's).
    fn settle_imports(&mut self) {
        let mut taken = self.fixed_names();
        taken.extend(self.seeded.values().cloned());
        let claim = |candidates: Vec<String>, taken: &mut HashSet<String>| -> String {
            let mut name = candidates.iter().find(|c| !taken.contains(*c) && !JS_RESERVED.contains(&c.as_str())).cloned().unwrap_or_else(|| candidates[candidates.len() - 1].clone());
            while taken.contains(&name) || JS_RESERVED.contains(&name.as_str()) {
                name.push('_');
            }
            taken.insert(name.clone());
            name
        };
        for leaf in self.leaves.clone() {
            let local = claim(vec![leaf.clone(), format!("{leaf}_")], &mut taken);
            self.leaf_local.insert(leaf, local);
        }
        for s in self.secrets.clone() {
            let local = claim(vec![s.clone(), format!("{s}_secret")], &mut taken);
            self.secret_local.insert(s, local);
        }
        for h in self.hosts.clone() {
            let (base, last) = host_names(&h);
            let local = claim(vec![base.clone(), format!("{base}_{last}")], &mut taken);
            self.host_local.insert(h, local);
        }
        for (slug, actions) in self.flow_actions.clone() {
            for a in actions {
                let local = claim(vec![a.clone(), format!("{}_{a}", call_noun(&slug))], &mut taken);
                self.action_local.insert((slug.clone(), a), local);
            }
        }
    }

    /// The name leaf `leaf` is called by, imported from "holon".
    fn leaf(&mut self, leaf: &str) -> String {
        self.leaves.insert(leaf.to_string());
        self.leaf_local.get(leaf).cloned().unwrap_or_else(|| if JS_RESERVED.contains(&leaf) { format!("{leaf}_") } else { leaf.to_string() })
    }

    /// The name secret `name` is read by.
    fn secret(&mut self, name: &str) -> Result<String, String> {
        if !self.secrets.contains(name) {
            return Err(format!("the secret \"{name}\" has no TypeScript form: name it as a word (letters, digits, _)"));
        }
        Ok(self.secret_local.get(name).cloned().unwrap_or_else(|| name.to_string()))
    }

    /// The name host `host` is called through.
    fn host(&mut self, host: &str) -> String {
        if !self.hosts.iter().any(|h| h == host) {
            self.hosts.push(host.to_string());
        }
        self.host_local.get(host).cloned().unwrap_or_else(|| host_names(host).0)
    }

    /// The name another flow's action is called by.
    fn action(&mut self, flow: &str, action: &str) -> String {
        self.flow_actions.entry(flow.to_string()).or_default().insert(action.to_string());
        self.action_local.get(&(flow.to_string(), action.to_string())).cloned().unwrap_or_else(|| action.to_string())
    }

    /// The imports of what the text uses and declares, in the order the converter writes them.
    fn imports(&self, optional: &BTreeSet<String>) -> Vec<String> {
        let mut holon: Vec<String> = self.vocab.iter().cloned().collect();
        holon.push("flow".into());
        for leaf in &self.leaves {
            let local = &self.leaf_local[leaf];
            holon.push(if local == leaf { leaf.clone() } else { format!("{leaf} as {local}") });
        }
        holon.sort();
        holon.dedup();
        let mut out = vec![format!("import {{ {} }} from \"holon\"", holon.join(", "))];
        if self.uses_state || !self.state.is_empty() {
            out.push("import state from \"state\"".into());
        }
        for s in &self.secrets {
            let local = &self.secret_local[s];
            let with = if optional.contains(s) { " with { optional: \"true\" }" } else { "" };
            out.push(format!("import {local} from \"secret:{s}\"{with}"));
        }
        for c in &self.collections {
            out.push(format!("import {c} from \"collection:{c}\""));
        }
        for f in self.folders.iter().filter(|f| !self.watched.contains(f)) {
            out.push(format!("import {f} from \"folder:{f}\""));
        }
        for d in &self.databases {
            out.push(format!("import {d} from \"database:{d}\""));
        }
        for c in &self.channels {
            out.push(format!("import {c} from \"channel:{c}\""));
        }
        for m in &self.mails {
            out.push(format!("import {m} from \"mail:{m}\""));
        }
        for f in &self.feeds {
            out.push(format!("import {f} from \"feed:{f}\""));
        }
        for q in &self.queues {
            out.push(format!("import {q} from \"queue:{q}\""));
        }
        for f in &self.watched {
            out.push(format!("import {f} from \"folder:{f}\""));
        }
        for h in &self.hosts {
            out.push(format!("import {} from \"host:{h}\"", self.host_local[h]));
        }
        if self.uses_now {
            out.push("import { now } from \"holon:clock\"".into());
        }
        match (self.uses_random, self.uses_random_bytes) {
            (true, true) => out.push("import { random, random_bytes } from \"holon:random\"".into()),
            (true, false) => out.push("import { random } from \"holon:random\"".into()),
            (false, true) => out.push("import { random_bytes } from \"holon:random\"".into()),
            (false, false) => {}
        }
        for (slug, actions) in &self.flow_actions {
            let named: Vec<String> = actions
                .iter()
                .map(|a| {
                    let local = &self.action_local[&(slug.clone(), a.clone())];
                    if local == a { a.clone() } else { format!("{a} as {local}") }
                })
                .collect();
            out.push(format!("import {{ {} }} from \"flow:{slug}\"", named.join(", ")));
        }
        out
    }

    /// `flow("name", { port, keep, limits, browsers, taint })`, from the document's settings and
    /// its handlers' triggers.
    fn settings(&self, doc: &FlowDocument) -> Result<String, String> {
        let name = self.own.clone().unwrap_or_default();
        let mut fields = Vec::new();
        if let Some(s) = &doc.server {
            if !s.port.is_empty() {
                if !s.port.bytes().all(|b| b.is_ascii_digit()) || !is_number(&s.port) {
                    return Err(format!("the port \"{}\" has no TypeScript form: a port is a number, port: 8080", s.port));
                }
                fields.push(format!("port: {}", s.port));
            }
            // How long the log keeps runs, then each other limit it asks for, a duration where its
            // unit is one.
            let limits = s.requirements.as_ref().and_then(|r| r.extra.get("limits"));
            let limit = |name: &str| limits.and_then(|l| l.get(name)).and_then(serde_json::Value::as_u64).filter(|n| *n >= 1);
            let keep: Vec<String> = [limit("keep_runs").map(|n| format!("runs: {n}")), limit("keep_run_days").map(|n| format!("days: {n}"))].into_iter().flatten().collect();
            if !keep.is_empty() {
                fields.push(format!("keep: {{ {} }}", keep.join(", ")));
            }
            let mut set = Vec::new();
            for name in crate::LIMIT_NAMES.iter().filter(|n| !n.starts_with("keep_") && !n.starts_with("module_")) {
                let Some(n) = limits.and_then(|l| l.get(*name)).and_then(serde_json::Value::as_u64) else { continue };
                set.push(format!("{name}: {}", limit_value(name, n)));
            }
            if !set.is_empty() {
                fields.push(format!("limits: {{ {} }}", set.join(", ")));
            }
            if let Some(b) = s.browsers.as_ref().filter(|b| !b.is_empty()) {
                let origins: Vec<String> = b.split(|c: char| c.is_whitespace() || c == ',').filter(|o| !o.is_empty()).map(quote).collect();
                fields.push(format!("browsers: {}", if origins.len() == 1 { origins[0].clone() } else { format!("[{}]", origins.join(", ")) }));
            }
        }
        // The keys it taints, from its handlers' triggers: one taints a key for every run.
        let taint = crate::routes::flow_taint(&doc.nodes);
        if !taint.is_empty() {
            fields.push(format!("taint: [{}]", taint.into_iter().map(quote).collect::<Vec<_>>().join(", ")));
        }
        Ok(if fields.is_empty() { format!("flow({})", quote(&name)) } else { format!("flow({}, {{ {} }})", quote(&name), fields.join(", ")) })
    }

    /// The handlers (routes, clocks, file and mail handlers, actions) in the order of their nodes; the
    /// functions they place go to `defs` as they are met.
    fn items(&mut self, doc: &FlowDocument) -> Result<Vec<Item>, String> {
        let (routes, mut errors) = find_routes(&doc.nodes, &doc.edges);
        let (clocks, clock_errors) = find_clocks(&doc.nodes, &doc.edges);
        errors.extend(clock_errors);
        let (files, file_errors) = find_files(&doc.nodes, &doc.edges);
        errors.extend(file_errors);
        let (mails, mail_errors) = find_mails(&doc.nodes, &doc.edges);
        errors.extend(mail_errors);
        let (feeds, feed_errors) = find_feeds(&doc.nodes, &doc.edges);
        errors.extend(feed_errors);
        let (queues, queue_errors) = find_queues(&doc.nodes, &doc.edges);
        errors.extend(queue_errors);
        let (actions, action_errors) = find_actions(&doc.nodes, &doc.edges);
        errors.extend(action_errors);
        let (sockets, socket_errors) = find_websockets(&doc.nodes, &doc.edges, &routes);
        errors.extend(socket_errors);
        if !errors.is_empty() {
            return Err(format!("{}: TypeScript writes each handler with its own nodes", errors.join("; ")));
        }
        let ids: Vec<&str> = doc.nodes.iter().map(|n| n.id.as_str()).collect();
        let by_id: HashMap<&str, &Node> = doc.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let position: HashMap<&str, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let first = |node_ids: &[String]| node_ids.iter().map(|id| position[id.as_str()]).min().unwrap_or(usize::MAX);
        enum Handler<'r> {
            Route(&'r crate::routes::RouteGroup),
            Clock(&'r crate::routes::ClockGroup),
            File(&'r crate::routes::FileGroup),
            Mail(&'r crate::routes::MailGroup),
            Feed(&'r crate::routes::FeedGroup),
            Queue(&'r crate::routes::QueueGroup),
            Action(&'r crate::routes::ActionGroup),
            /// A websocket's blocks on one path, in the order their nodes are.
            Socket(&'r str, Vec<&'r crate::routes::SocketGroup>),
        }
        let mut paths: Vec<&str> = Vec::new();
        for s in &sockets {
            if !paths.contains(&s.path.as_str()) {
                paths.push(&s.path);
            }
        }
        let socket_handlers = paths.into_iter().map(|path| {
            let mut blocks: Vec<&crate::routes::SocketGroup> = sockets.iter().filter(|s| s.path == path).collect();
            blocks.sort_by_key(|s| first(&s.node_ids));
            (blocks.iter().map(|s| first(&s.node_ids)).min().unwrap_or(usize::MAX), Handler::Socket(path, blocks))
        });
        let mut handlers: Vec<(usize, Handler)> = routes
            .iter()
            .map(|r| (first(&r.node_ids), Handler::Route(r)))
            .chain(clocks.iter().map(|c| (first(&c.node_ids), Handler::Clock(c))))
            .chain(files.iter().map(|f| (first(&f.node_ids), Handler::File(f))))
            .chain(mails.iter().map(|m| (first(&m.node_ids), Handler::Mail(m))))
            .chain(feeds.iter().map(|f| (first(&f.node_ids), Handler::Feed(f))))
            .chain(queues.iter().map(|q| (first(&q.node_ids), Handler::Queue(q))))
            .chain(actions.iter().map(|a| (first(&a.node_ids), Handler::Action(a))))
            .chain(socket_handlers)
            .collect();
        handlers.sort_by_key(|(at, _)| *at);
        let handled: HashSet<&String> = routes
            .iter()
            .flat_map(|r| &r.node_ids)
            .chain(clocks.iter().flat_map(|c| &c.node_ids))
            .chain(files.iter().flat_map(|f| &f.node_ids))
            .chain(mails.iter().flat_map(|m| &m.node_ids))
            .chain(feeds.iter().flat_map(|f| &f.node_ids))
            .chain(queues.iter().flat_map(|q| &q.node_ids))
            .chain(actions.iter().flat_map(|a| &a.node_ids))
            .chain(sockets.iter().flat_map(|s| &s.node_ids))
            .collect();
        let stray: Vec<Vec<String>> = components(&ids, &doc.edges).into_iter().filter(|g| !g.iter().any(|id| handled.contains(id))).collect();
        if !stray.is_empty() {
            let what = if doc.nodes.iter().any(|n| n.data.name == "file_arrival") {
                " (a file that arrives is a file handler's parameter now: on.file.created(folder, (f) => …))"
            } else {
                ""
            };
            return Err(format!("nothing starts this flow: add a route, a clock, a file, mail, feed or queue handler or an action{what}"));
        }
        let mut items = Vec::new();
        for (_, handler) in handlers {
            let item = match handler {
                Handler::Route(r) => {
                    let request = by_id[r.request_id.as_str()];
                    if !holon_core::METHODS.contains(&r.method.as_str()) {
                        return Err(format!("{} {}: TypeScript has no route.{}(…)", r.method, r.path, r.method.to_ascii_lowercase()));
                    }
                    let opts = [("resumes", r.resume.as_str()), ("log", log_option(&r.log))];
                    let head = format!("route.{}({}, ", r.method.to_ascii_lowercase(), quote(&r.path));
                    self.vocab.insert("route".into());
                    self.handler(doc, &r.node_ids, request, Kind::Route, head, options(&opts), &format!("{} {}", r.method, r.path))?
                }
                Handler::Clock(c) => {
                    let tick = by_id[c.every_id.as_str()];
                    self.vocab.insert("every".into());
                    let head = format!("every({}, ", quote(&holon_core::period_text(c.seconds)));
                    self.handler(doc, &c.node_ids, tick, Kind::Every, head, options(&[("log", log_option(&c.log))]), &c.label)?
                }
                Handler::File(f) => {
                    let on = by_id[f.on_file_id.as_str()];
                    self.vocab.insert("on".into());
                    let head = format!("on.file.{}({}, ", if f.deleted { "deleted" } else { "created" }, f.folder);
                    self.handler(doc, &f.node_ids, on, Kind::File, head, options(&[("matching", f.matching.as_str()), ("log", log_option(&f.log))]), &f.label)?
                }
                Handler::Mail(m) => {
                    let on = by_id[m.on_mail_id.as_str()];
                    self.vocab.insert("on".into());
                    let head = format!("on.mail({}, ", m.mail);
                    self.handler(doc, &m.node_ids, on, Kind::Mail, head, options(&[("log", log_option(&m.log))]), &m.label)?
                }
                Handler::Feed(f) => {
                    let on = by_id[f.on_feed_id.as_str()];
                    self.vocab.insert("on".into());
                    let head = format!("on.feed({}, ", f.feed);
                    self.handler(doc, &f.node_ids, on, Kind::Feed, head, options(&[("log", log_option(&f.log))]), &f.label)?
                }
                Handler::Queue(q) => {
                    let on = by_id[q.on_queue_id.as_str()];
                    self.vocab.insert("on".into());
                    let head = format!("on.queue({}, ", q.queue);
                    self.handler(doc, &q.node_ids, on, Kind::Queue, head, options(&[("log", log_option(&q.log))]), &q.label)?
                }
                Handler::Action(a) => {
                    let input = by_id[a.action_id.as_str()];
                    self.vocab.insert("action".into());
                    let head = format!("export const {} = action(", a.name);
                    self.handler(doc, &a.node_ids, input, Kind::Action, head, options(&[("log", log_option(&a.log))]), &a.label)?
                }
                Handler::Socket(path, blocks) => self.websocket(doc, path, &blocks)?,
            };
            items.push(item);
        }
        Ok(items)
    }

    /// A handler: `head`, its function, `options`, `)`; its trigger is `trigger`.
    #[allow(clippy::too_many_arguments)]
    fn handler(&mut self, doc: &FlowDocument, node_ids: &[String], trigger: &Node, kind: Kind, head: String, options: String, label: &str) -> Result<Item, String> {
        let fallback = match kind {
            Kind::Route => "req",
            Kind::Action => "input",
            Kind::File => "f",
            Kind::Mail => "m",
            Kind::Feed => "e",
            Kind::Queue => "m",
            _ => "",
        };
        let (function, body) = self.handler_function(doc, node_ids, trigger, kind, fallback, label)?;
        Ok(Item { text: format!("{head}{function}{options})"), nodes: vec![trigger.id.clone()], body, blocks: vec![] })
    }

    /// `route.websocket(path, { open: …, message: …, close: … }, { log })`: each block's function
    /// as a route's (`open`) or an action's (`message`, and `close`, whose only answer is the
    /// nothing of one that calls alone), in that order.
    fn websocket(&mut self, doc: &FlowDocument, path: &str, blocks: &[&crate::routes::SocketGroup]) -> Result<Item, String> {
        let by_id: HashMap<&str, &Node> = doc.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        self.vocab.insert("route".into());
        let (mut fields, mut written) = (Vec::new(), Vec::new());
        for g in blocks {
            let trigger = by_id[g.socket_id.as_str()];
            let (kind, fallback) = match g.on.as_str() {
                "open" => (Kind::Route, "req"),
                "message" => (Kind::Action, "m"),
                _ => (Kind::Action, "c"),
            };
            if g.on == "close"
                && let Some(r) = g.node_ids.iter().map(|id| by_id[id.as_str()]).find(|n| n.is_template("respond"))
                && (r.param("status") != "204" || doc.edges.iter().any(|w| w.target == r.id && w.input() != "request"))
            {
                return Err(format!("{}: a close block answers nobody", g.label));
            }
            let (function, body) = self.handler_function(doc, &g.node_ids, trigger, kind, fallback, &g.label)?;
            fields.push(format!("{}: {function}", g.on));
            written.push((trigger.id.clone(), body));
        }
        let levels: Vec<(&str, &str)> = blocks.iter().map(|g| (g.on.as_str(), log_option(&g.log))).collect();
        let options = match levels.first() {
            Some((_, first)) if !first.is_empty() && levels.iter().all(|(_, l)| l == first) => format!(", {{ log: {} }}", quote(first)),
            _ if levels.iter().any(|(_, l)| !l.is_empty()) => {
                let each: Vec<String> = levels.iter().filter(|(_, l)| !l.is_empty()).map(|(on, l)| format!("{on}: {}", quote(l))).collect();
                format!(", {{ log: {{ {} }} }}", each.join(", "))
            }
            _ => String::new(),
        };
        Ok(Item { text: format!("route.websocket({}, {{ {} }}{options})", quote(path), fields.join(", ")), nodes: vec![], body: vec![], blocks: written })
    }

    /// A handler's function, `(req) => …`, and its statements; `fallback` names its parameter
    /// when the text gave it none it can keep.
    fn handler_function(&mut self, doc: &FlowDocument, node_ids: &[String], trigger: &Node, kind: Kind, fallback: &str, label: &str) -> Result<(String, Vec<Stmt>), String> {
        let by_id: HashMap<&str, &Node> = doc.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let nodes: Vec<&Node> = node_ids.iter().map(|id| by_id[id.as_str()]).collect();
        let ws = doc.edges.iter().map(|w| (Src::Node { id: w.source.clone(), output: w.output().to_string() }, w.target.clone(), w.input().to_string())).collect();
        let group = Group::new(nodes, ws, vec![]);
        if let Some(why) = unwritable(&group.nodes, &group.incoming, Some(&trigger.id)) {
            return Err(format!("{label}: {why}"));
        }
        let param = if kind == Kind::Every { String::new() } else { self.handle_name(&trigger.id, fallback) };
        let request = (kind != Kind::Every).then(|| (trigger.id.clone(), param.clone()));
        let mut block = Block::new(self, &group, kind, request);
        if kind == Kind::Every {
            block.tick = Some(trigger.id.clone());
        }
        let body = block.body()?;
        let (read, awaits) = (block.param_read, block.awaits);
        let params = if kind == Kind::Every || (!read && param == fallback) { "()".to_string() } else { format!("({param})") };
        let asy = if awaits { "async " } else { "" };
        // A route or action that only answers is its answer; an object alone, in parentheses.
        if let [only] = body.as_slice()
            && matches!(kind, Kind::Route | Kind::Action)
            && let Some(value) = only.text.strip_prefix("return ")
            && only.kids.is_empty()
            && only.after.is_none()
        {
            let object = value.starts_with('{') && is_one_group(value);
            if object || !value.starts_with('{') {
                let value = if object { format!("({value})") } else { value.to_string() };
                return Ok((format!("{asy}{params} => {value}"), body));
            }
        }
        let text = body.iter().map(|s| s.text.clone()).collect::<Vec<_>>().join("\n");
        Ok((format!("{asy}{params} => {{\n{text}\n}}"), body))
    }

    /// The name a handler's parameter goes by: the text's, or `fallback` when it has none it
    /// can keep.
    fn handle_name(&self, id: &str, fallback: &str) -> String {
        let n = self.bindings.get(id).map(|n| ts_name(n)).unwrap_or_else(|| fallback.into());
        let free = is_identifier(&n) && !self.avoid.contains(&n) && local_name(&n) == self.bindings.get(id).cloned().unwrap_or_else(|| local_name(&n));
        if free { n } else { fallback.into() }
    }

    /// A function's name, by hash, taken once: the document's, or one made up from `hint`; the
    /// function is written the first time it is asked for.
    fn function_for(&mut self, hash: &str, hint: &str) -> Result<String, String> {
        if let Some(known) = self.def_names.get(hash) {
            return Ok(known.clone());
        }
        let hname = match self.seeded.get(hash) {
            Some(n) => n.clone(),
            None => {
                let base = identifier(self.names.get(hash).map(String::as_str).unwrap_or(hint));
                let mut hname = base.clone();
                while self.avoid.contains(&hname) || VOCAB.contains(&hname.as_str()) || taken(&hname) || self.def_names.values().any(|n| *n == hname) {
                    hname.push('_');
                }
                hname
            }
        };
        self.def_names.insert(hash.into(), hname.clone());
        let item = self.function(hash, &hname)?;
        self.defs.push(item);
        Ok(hname)
    }

    /// `function name(a, b) { … return … }`, or a js node's `const name = js(…)`: the unit's
    /// children as nodes, its edges as wires, its boundary as the parameters and what it returns.
    /// Functions inside are written first.
    fn function(&mut self, hash: &str, hname: &str) -> Result<Item, String> {
        let (_, unit) = resolve(self.store, hash)?;
        let unit = unit.clone();
        if is_js_node(self.store, &unit) {
            return self.js_def(&unit, hname);
        }
        let opened = self.open(hash, hname, &unit)?;
        let group = Group::new(opened.children.iter().collect(), opened.wires.clone(), opened.outs.clone());
        let ins = params_of(&group, &unit, hname)?;
        let calls = call_outputs(&unit, self.store);
        let mut block = Block::new(self, &group, Kind::Function, None);
        block.inputs = ins.iter().cloned().collect();
        block.calls = calls;
        for (_, ts) in &ins {
            block.claim(ts);
        }
        let body = block.body()?;
        let params: Vec<String> = ins.iter().map(|(_, ts)| ts.clone()).collect();
        let text = body.iter().map(|s| s.text.clone()).collect::<Vec<_>>().join("\n");
        Ok(Item { text: format!("function {hname}({}) {{\n{text}\n}}", params.join(", ")), nodes: vec![], body: vec![], blocks: vec![] })
    }

    /// A function's unit opened: its children, its wires, its boundary's inputs as `Src::In` and
    /// its outputs as `(node, output, port)`.
    fn open(&self, hash: &str, hname: &str, unit: &holon_core::Unit) -> Result<Opened, String> {
        let ports = |ps: &[holon_core::Port]| ps.iter().map(WPort::from_core).collect::<Vec<_>>();
        let target = Node::placed("@holon", Default::default(), crate::canvas::NodeData::new(hname, PlacedBody::Composite { hash: hash.into() }, ports(&unit.inputs), ports(&unit.outputs), vec![]));
        let opened = inner_canvas(&target, std::slice::from_ref(&target), &[], self.store, &self.names).map_err(|e| format!("the function \"{hname}\" can't be opened: {e}"))?;
        let mut wires: Vec<BlockWire> = opened.wires.iter().map(|w| (Src::Node { id: w.source.clone(), output: w.output().to_string() }, w.target.clone(), w.input().to_string())).collect();
        let mut outs: Vec<(String, String, String)> = Vec::new();
        if let UnitBody::Composite { edges, .. } = &unit.body {
            for e in edges {
                if e.from_child == BOUNDARY {
                    if let Some(c) = opened.children.get(e.to_child as usize) {
                        let input = c.data.inputs.get(e.to_port as usize).map(|p| p.name.clone()).unwrap_or_default();
                        let port = unit.inputs.get(e.from_port as usize).map(|p| p.name.clone()).unwrap_or_default();
                        wires.push((Src::In(port), c.id.clone(), input));
                    }
                } else if e.to_child == BOUNDARY
                    && let Some(c) = opened.children.get(e.from_child as usize)
                {
                    let output = c.data.outputs.get(e.from_port as usize).map(|p| p.name.clone()).unwrap_or_default();
                    let port = unit.outputs.get(e.to_port as usize).map(|p| p.name.clone()).unwrap_or_default();
                    outs.push((c.id.clone(), output, port));
                }
            }
        }
        // In the order of the unit's outputs, as the function returns them.
        outs.sort_by_key(|(_, _, port)| unit.outputs.iter().position(|p| p.name == *port));
        // A sink left open inside is tied to nothing, which a function's body can't say: it ties
        // each to the function's trigger.
        if opened.children.iter().any(|c| c.data.inputs.iter().any(|p| p.name == TRIGGER) && !wires.iter().any(|(_, to, input)| *to == c.id && input == TRIGGER)) {
            return Err(format!("the function \"{hname}\" has a write or a call tied to nothing: TypeScript ties each to the call of the function, so wire its trigger"));
        }
        Ok(Opened { children: opened.children, wires, outs })
    }
    /// `const name = js(function out(a, b) { … }, { fuel, memory, outputs })`: the body verbatim.
    fn js_def(&mut self, unit: &holon_core::Unit, hname: &str) -> Result<Item, String> {
        let (body, fuel, memory) = js_parts_ts(self.store, unit).ok_or_else(|| format!("the js node \"{hname}\" can't be read back"))?;
        let ins: Vec<&str> = unit.inputs.iter().map(|p| p.name.as_str()).collect();
        let outs: Vec<&str> = unit.outputs.iter().map(|p| p.name.as_str()).collect();
        for n in ins.iter().chain(&outs) {
            if !is_identifier(n) || JS_RESERVED.contains(n) {
                return Err(format!("the js node \"{hname}\"'s port \"{n}\" has no TypeScript form: name it as a word"));
            }
        }
        self.vocab.insert("js".into());
        let mut opts = Vec::new();
        if let Some(f) = fuel {
            opts.push(format!("fuel: {f}"));
        }
        if let Some(m) = memory {
            opts.push(format!("memory: {m}"));
        }
        let fn_name = match outs.as_slice() {
            [one] => one.to_string(),
            _ => {
                opts.push(format!("outputs: [{}]", outs.iter().map(|o| quote(o)).collect::<Vec<_>>().join(", ")));
                hname.to_string()
            }
        };
        let opts = if opts.is_empty() { String::new() } else { format!(", {{ {} }}", opts.join(", ")) };
        Ok(Item { text: format!("const {hname} = js(function {fn_name}({}) {{{body}}}{opts})", ins.join(", ")), nodes: vec![], body: vec![], blocks: vec![] })
    }

    /// A node's params that differ from its leaf's or template's defaults (a literal's always).
    fn changed_params(&self, n: &Node) -> Vec<(String, String)> {
        let d = &n.data;
        let defaults: Vec<WParam> = match d.body {
            PlacedBody::Template => templates::defaults(&d.name).unwrap_or_default(),
            PlacedBody::Primitive => interface_of(&d.name).map(|u| u.params.iter().map(WParam::from_core).collect()).unwrap_or_default(),
            PlacedBody::Composite { .. } | PlacedBody::Module { .. } => vec![],
        };
        d.params
            .iter()
            .filter_map(|p| match p {
                WParam::Frozen { name, value } => {
                    let keep = d.name == "const"
                        || d.name == "default"
                        || match defaults.iter().find(|x| x.name() == name) {
                            Some(WParam::Frozen { value: v, .. }) => v != value,
                            _ => true,
                        };
                    keep.then(|| (name.clone(), value.clone()))
                }
                WParam::Open { .. } => None,
            })
            .collect()
    }
}

/// The names a host is imported by: its first label that says something (`api.anthropic.com` is
/// `anthropic`), and its last label, added when two hosts would share one.
fn host_names(host: &str) -> (String, String) {
    let labels: Vec<&str> = host.split('.').collect();
    let meaningful: Vec<&str> = labels.iter().take(labels.len().saturating_sub(1).max(1)).copied().filter(|l| !matches!(*l, "api" | "www")).collect();
    let base = identifier(meaningful.first().copied().unwrap_or(host));
    (base, identifier(labels.last().copied().unwrap_or("host")))
}

/// Whether `text` is one bracketed group, an object literal `{ … }` or a list's `[ … ]`, its
/// brackets matched only at its ends.
fn is_one_group(text: &str) -> bool {
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => quote = Some(c),
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => {
                depth -= 1;
                if depth == 0 && i + 1 < text.len() {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

/// A function's unit opened ([`Printer::open`]).
struct Opened {
    children: Vec<Node>,
    wires: Vec<BlockWire>,
    outs: Vec<(String, String, String)>,
}

/// A function's parameters: each port and the name it goes by. Its trigger, last and read by
/// nothing but ties, is the one it gets without declaring it.
fn params_of(group: &Group, unit: &holon_core::Unit, hname: &str) -> Result<Vec<(String, String)>, String> {
    if let Some(why) = unwritable(&group.nodes, &group.incoming, None) {
        return Err(format!("the function \"{hname}\": {why}"));
    }
    let mut ins: Vec<String> = unit.inputs.iter().map(|p| p.name.clone()).collect();
    if ins.last().is_some_and(|p| p == TRIGGER) && !group.incoming.values().any(|s| matches!(s, Src::In(p) if p == TRIGGER)) {
        ins.pop();
    }
    ins.into_iter()
        .map(|port| {
            let ts = param_name(&port);
            if !is_identifier(&ts) || JS_RESERVED.contains(&ts.as_str()) || port_of(&ts) != port {
                return Err(format!("the function \"{hname}\"'s input \"{port}\" has no TypeScript form: a parameter is a word"));
            }
            Ok((port, ts))
        })
        .collect()
}

/// What a block is the body of.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Route,
    Action,
    Every,
    File,
    Mail,
    Feed,
    Queue,
    Function,
    /// A function written in place, `(x) => …`: one expression, no statements.
    Lambda,
}

/// The shape a `match` lowers to (match-L4.md §4), as found in a group: a `switch` writes it.
#[derive(Clone)]
struct MatchShape<'g> {
    subject: Src,
    /// Each arm's pattern constants, and the guard holding its value.
    arms: Vec<(Vec<&'g Node>, &'g Node)>,
    /// The guard holding the `default`'s value.
    els: Option<&'g Node>,
    /// Every node the `match` folds in, besides its subject and its values.
    parts: Vec<&'g Node>,
}

/// The statements of one block, and the expressions within them.
struct Block<'p, 's, 'g> {
    p: &'p mut Printer<'s>,
    g: &'g Group<'g>,
    kind: Kind,
    /// The handler's trigger node and its parameter's name (a route's request, a file handler's
    /// file, an action's input).
    request: Option<(String, String)>,
    /// A clock's tick node: it is `now` there.
    tick: Option<String>,
    /// A function's inputs (a function written in place's captured values too): each port and
    /// what it is read as.
    inputs: HashMap<String, String>,
    /// Each node bound, and its name.
    bound: HashMap<String, String>,
    /// The names the block declares, and the lowering's names for them.
    used: HashSet<String>,
    lowered: HashSet<String>,
    /// The nodes that must be bound under a name: a function's output, a value a function
    /// written in place uses from around it.
    forced: HashMap<String, String>,
    /// The nodes bound under a name of their own though used once: a literal `??` or `||` would
    /// read otherwise, a call that `if (x !== undefined)` would tie with its statement.
    held: HashSet<String>,
    /// Bindings from the text not kept: a switch's values, named after the `let` they give.
    ignored: HashSet<String>,
    /// The matches written as a `switch`, by their top node.
    matches: HashMap<String, MatchShape<'g>>,
    /// The match a route answers, written as a `switch` of returns.
    switch_answer: Option<String>,
    /// The nodes a `switch` folds in (its comparisons, guards and chain), and a subject only its
    /// comparisons use.
    absorbed: HashSet<String>,
    /// A function of several outputs read whole, by the last link of the record of its outputs:
    /// the call, then the record and its links, which print as the call.
    wholes: HashMap<String, (&'g Node, Vec<&'g Node>)>,
    /// A function's outputs written as statements, by node, in the order of its outputs: the
    /// lowering names them after their leaves.
    auto: Vec<String>,
    /// A function's outputs that answer a call tied to its trigger (`call_outputs`).
    calls: Vec<String>,
    /// A function's other outputs: each port and where its value comes from.
    declared: Vec<(String, Src)>,
    /// The nodes rendered into the statement being written, and the stores it reads.
    touched: Vec<String>,
    reads: BTreeSet<String>,
    /// Whether the body reads the handler's parameter, and awaits something.
    param_read: bool,
    awaits: bool,
    /// The parts of what the handler received (and a clock's tick) a function written in place
    /// uses, by the names their `const`s give them; and those `const`s, first in the body.
    captured: HashMap<(String, String), String>,
    aliases: Vec<Stmt>,
}

impl<'p, 's, 'g> Block<'p, 's, 'g> {
    fn new(p: &'p mut Printer<'s>, g: &'g Group<'g>, kind: Kind, request: Option<(String, String)>) -> Block<'p, 's, 'g> {
        let mut used = HashSet::new();
        let mut lowered = HashSet::new();
        if let Some((_, name)) = &request {
            used.insert(name.clone());
            lowered.insert(local_name(name));
        }
        Block {
            p,
            g,
            kind,
            request,
            tick: None,
            inputs: HashMap::new(),
            bound: HashMap::new(),
            used,
            lowered,
            forced: HashMap::new(),
            held: HashSet::new(),
            ignored: HashSet::new(),
            matches: HashMap::new(),
            switch_answer: None,
            absorbed: HashSet::new(),
            wholes: HashMap::new(),
            auto: Vec::new(),
            calls: Vec::new(),
            declared: Vec::new(),
            touched: Vec::new(),
            reads: BTreeSet::new(),
            param_read: false,
            awaits: false,
            captured: HashMap::new(),
            aliases: Vec::new(),
        }
    }

    fn handler(&self) -> bool {
        matches!(self.kind, Kind::Route | Kind::Action | Kind::Every | Kind::File | Kind::Mail | Kind::Feed | Kind::Queue)
    }

    // ── what each node is ────────────────────────────────────────────────────────────────────

    fn is_request(&self, id: &str) -> bool {
        self.request.as_ref().is_some_and(|(r, _)| r == id)
    }

    fn is_tick(&self, id: &str) -> bool {
        self.tick.as_deref() == Some(id)
    }

    /// Whether `node` keeps the name the text gave it. A function written in place gave none: its
    /// nodes' ids are its own canvas's, which a top-level node's name may share.
    fn named(&self, node: &Node) -> bool {
        self.kind != Kind::Lambda && self.p.bindings.get(&node.id).is_some_and(|b| !hidden(b)) && !self.ignored.contains(&node.id)
    }

    /// Whether `node` may fold into an expression: no name of its own, and one value.
    fn free(&self, node: &Node) -> bool {
        !(self.is_request(&node.id) || self.is_tick(&node.id) || node.is_template("respond") || matches!(node.data.name.as_str(), "await" | "approval") || self.bound.contains_key(&node.id) || self.named(node)) && self.one_value(node)
    }

    /// Whether `node`'s one output prints inline at its one consumer.
    fn inline(&self, node: &Node) -> bool {
        self.free(node)
            && (self.g.consumed_total(node) == 1 || self.absorbed.contains(&node.id))
            && !self.tied_elsewhere(node)
            && !self.awaited(node)
            && !self.made_before_a_branch(node)
            && !self.forced.contains_key(&node.id)
            && !self.held.contains(&node.id)
            && !self.matches.contains_key(&node.id)
    }

    /// Whether `node` is a call its block makes when reached (tied to the block's trigger) that,
    /// inline, would be written in a branch, where the lowering makes a call only when the branch
    /// needs it: it keeps its binding, a statement of its own.
    fn made_before_a_branch(&self, node: &Node) -> bool {
        CALLS.contains(&node.data.name.as_str()) && self.g.block_tied.contains(&node.id) && self.lands_in_a_branch(node)
    }

    /// Whether `node`'s one use, followed through the nodes that print inline, is a branch of a
    /// choice: a `guard`'s value or what a `coalesce` falls back to.
    fn lands_in_a_branch(&self, node: &Node) -> bool {
        let Some((to, input)) = self.g.uses(&node.id).into_iter().next() else { return false };
        let next = self.g.node(to);
        match (next.data.name.as_str(), input.as_str()) {
            ("guard", "value") | ("coalesce", "b") => true,
            _ => self.inline(next) && self.lands_in_a_branch(next),
        }
    }

    /// Whether `node`'s trigger is fed by something other than its block's trigger: it goes in
    /// `if (x !== undefined)`.
    fn tied_elsewhere(&self, node: &Node) -> bool {
        self.g.incoming.contains_key(&(node.id.clone(), TRIGGER.to_string()))
    }

    /// Whether a trigger waits for `node`: it is a statement of its own, before what waits.
    fn awaited(&self, node: &Node) -> bool {
        self.g.incoming.iter().any(|((_, input), src)| input == TRIGGER && matches!(src, Src::Node { id, .. } if *id == node.id))
    }

    /// The call `head` reads whole, with the record and links before it, when `head` ends the
    /// record lower.rs `whole` makes of a function's outputs: an empty `record`, then one
    /// `with_field` for each value output, in their order and named after it, each its only
    /// read, and no link read but by the next. A call the text named is its `const`.
    fn whole_call(&self, head: &Node) -> Option<(&'g Node, Vec<&'g Node>)> {
        let link = |n: &Node| n.data.name == "with_field" && n.data.body == PlacedBody::Primitive;
        let extended = self.g.uses(&head.id).iter().any(|(to, input)| *input == "record" && link(self.g.node(to)));
        if !link(head) || extended {
            return None;
        }
        let mut links = vec![self.g.node(&head.id)];
        let base = loop {
            let Some(Src::Node { id, .. }) = self.g.incoming.get(&(links[links.len() - 1].id.clone(), "record".to_string())) else { return None };
            let next = self.g.node(id);
            if !link(next) {
                break next;
            }
            if self.g.consumed_total(next) != 1 {
                return None;
            }
            links.push(next);
        };
        if base.data.name != "record" || base.data.body != PlacedBody::Primitive || self.g.consumed_total(base) != 1 {
            return None;
        }
        links.reverse();
        let Some(Src::Node { id, .. }) = self.g.incoming.get(&(links[0].id.clone(), "value".to_string())) else { return None };
        let call = self.g.node(id);
        let calls = self.answers(call);
        let values: Vec<&str> = call.data.outputs.iter().enumerate().filter(|(k, o)| o.ty != WType::Effect && !calls.contains(k)).map(|(_, o)| o.name.as_str()).collect();
        if !matches!(call.data.body, PlacedBody::Composite { .. }) || values.len() != links.len() || self.g.consumed_total(call) != values.len() {
            return None;
        }
        for (l, out) in links.iter().zip(&values) {
            let reads = matches!(self.g.incoming.get(&(l.id.clone(), "value".to_string())), Some(Src::Node { id, output }) if *id == call.id && output == out);
            if l.param("name") != *out || !reads {
                return None;
            }
        }
        if self.named(call) || self.tied_elsewhere(call) || self.awaited(call) {
            return None;
        }
        links.push(base);
        Some((call, links))
    }

    /// The input a `guard(true, input)` passes through, when `node` is one: the lowering places it
    /// for an input used twice, and it prints as the input itself.
    fn pass_through(&self, node: &Node) -> Option<String> {
        if node.data.name != "guard" {
            return None;
        }
        let always = self.inline_source(node, "cond", "const").is_some_and(|c| c.param("type") == "Boolean" && c.param("value") == "true");
        match self.g.incoming.get(&(node.id.clone(), "value".into())) {
            Some(Src::In(port)) if always => Some(port.clone()),
            _ => None,
        }
    }

    /// The node on `node`'s `input`, when it is a leaf `leaf` with no name, used `uses` times.
    fn part(&self, node: &Node, input: &str, leaf: &str, uses: usize) -> Option<&'g Node> {
        match self.g.incoming.get(&(node.id.clone(), input.to_string()))? {
            Src::Node { id, .. } => {
                let n = self.g.node(id);
                (n.data.name == leaf && n.data.body == PlacedBody::Primitive && self.free(n) && self.g.consumed_total(n) == uses).then_some(n)
            }
            Src::In(_) => None,
        }
    }

    /// The node wired to `node`'s `input`, by id.
    fn source_id(&self, node: &Node, input: &str) -> Option<String> {
        match self.g.incoming.get(&(node.id.clone(), input.to_string()))? {
            Src::Node { id, .. } => Some(id.clone()),
            Src::In(_) => None,
        }
    }

    /// The node wired to `node`'s `input`, when it prints inline and is a leaf `leaf`.
    fn inline_source(&self, node: &Node, input: &str, leaf: &str) -> Option<&'g Node> {
        match self.g.incoming.get(&(node.id.clone(), input.to_string()))? {
            Src::Node { id, .. } => {
                let n = self.g.node(id);
                (n.data.name == leaf && !matches!(n.data.body, PlacedBody::Composite { .. }) && self.inline(n)).then_some(n)
            }
            Src::In(_) => None,
        }
    }

    /// The `match` that `node` is the top of, if it is one: a `coalesce` chain of guards, each on
    /// its arm's patterns equalling one shared subject, the last perhaps the `default`'s, on none
    /// of them matching.
    fn match_shape(&self, node: &'g Node) -> Option<MatchShape<'g>> {
        if node.data.name != "coalesce" || node.data.body != PlacedBody::Primitive {
            return None;
        }
        let mut parts = vec![];
        let mut links = vec![];
        let mut cur = node;
        loop {
            links.push(self.part(cur, "b", "guard", 1)?);
            match self.part(cur, "a", "coalesce", 1) {
                Some(inner) => {
                    parts.push(inner);
                    cur = inner;
                }
                None => {
                    links.push(self.part(cur, "a", "guard", 1)?);
                    break;
                }
            }
        }
        links.reverse();
        // The default: the last guard, on `not` of the arms' conditions (their `or`, for several).
        let last = *links.last().expect("two links at least");
        let conds: Vec<String> = links[..links.len() - 1].iter().map(|g| self.source_id(g, "cond")).collect::<Option<_>>()?;
        let mut els = None;
        if let Some(not) = self.part(last, "cond", "not", 1) {
            let any = if conds.len() == 1 {
                (self.source_id(not, "v") == Some(conds[0].clone())).then_some(None)
            } else {
                self.part(not, "v", "or", 1).filter(|or| or.data.inputs.iter().map(|p| self.source_id(or, &p.name)).collect::<Option<Vec<_>>>() == Some(conds.clone())).map(Some)
            };
            if let Some(or) = any {
                parts.push(not);
                parts.extend(or);
                els = Some(last);
            }
        }
        let arm_links = if els.is_some() { &links[..links.len() - 1] } else { &links[..] };
        let uses = 1 + usize::from(els.is_some());
        let mut subject: Option<Src> = None;
        let mut seen: Vec<(&str, &str)> = vec![];
        let mut arms = vec![];
        for g in arm_links {
            parts.push(g);
            let eqs = match self.part(g, "cond", "equals", uses) {
                Some(eq) => vec![eq],
                None => {
                    let or = self.part(g, "cond", "or", uses)?;
                    parts.push(or);
                    or.data.inputs.iter().map(|p| self.part(or, &p.name, "equals", 1)).collect::<Option<Vec<_>>>()?
                }
            };
            let mut consts = vec![];
            for eq in eqs {
                let s = self.g.incoming.get(&(eq.id.clone(), "a".into()))?;
                if subject.get_or_insert_with(|| s.clone()) != s {
                    return None;
                }
                let c = self.part(eq, "b", "const", 1)?;
                let (ty, value) = (c.param("type"), c.param("value"));
                pattern_literal(ty, value)?;
                // Two equal patterns would not read back.
                let same = |(t, v): &(&str, &str)| *t == ty && (*v == value || (ty == "Number" && v.parse::<f64>().ok() == value.parse::<f64>().ok()));
                if seen.iter().any(same) {
                    return None;
                }
                seen.push((ty, value));
                parts.push(eq);
                parts.push(c);
                consts.push(c);
            }
            arms.push((consts, *g));
        }
        if let Some(e) = els {
            parts.push(e);
        }
        Some(MatchShape { subject: subject?, arms, els, parts })
    }

    /// The `c ? a : b` that `node` is the top of, if it is one (lower.rs `ExprKind::If`): `guard(c,
    /// a) ?? guard(!(c ?? false), b)`, one `c` on both sides. Its guards, the `not` and the `default`.
    fn ternary(&self, node: &Node) -> Option<[&'g Node; 4]> {
        if node.data.name != "coalesce" || node.data.body != PlacedBody::Primitive {
            return None;
        }
        let then = self.part(node, "a", "guard", 1)?;
        let taken = self.part(node, "b", "guard", 1)?;
        let not = self.part(taken, "cond", "not", 1)?;
        let held = self.part(not, "v", "default", 1).filter(|d| d.param("type") == "Boolean" && d.param("value") == "false")?;
        let c = self.g.incoming.get(&(then.id.clone(), "cond".to_string()))?;
        (self.g.incoming.get(&(held.id.clone(), "value".to_string())) == Some(c)).then_some([then, taken, not, held])
    }

    /// Whether a `switch` reads `m` back as the same values: ts.rs drops an arm whose value is the
    /// default's (the same name, or the same literal), so such a match stays the expressions it is.
    fn switch_reads_back(&self, m: &MatchShape) -> bool {
        let Some(e) = m.els else { return true };
        let value = |g: &Node| self.g.incoming.get(&(g.id.clone(), "value".to_string())).cloned();
        let literal_of = |src: &Option<Src>| match src {
            Some(Src::Node { id, .. }) => {
                let n = self.g.node(id);
                (n.data.name == "const" && matches!(n.param("type"), "Number" | "Text" | "Boolean")).then(|| (n.param("type").to_string(), n.param("value").to_string()))
            }
            _ => None,
        };
        let els = value(e);
        m.arms.iter().all(|(_, g)| {
            let v = value(g);
            v != els && (literal_of(&v).is_none() || literal_of(&v) != literal_of(&els))
        })
    }

    /// Whether `node` is a value TypeScript writes as a literal ts.rs reads as one (`0`, `"x"`,
    /// `{ k: [1] }`): after `??` it would make a `default`, after `||` or `+` it is refused.
    fn literal_shaped(&self, node: &Node) -> bool {
        if !self.inline(node) || node.data.body != PlacedBody::Primitive {
            return false;
        }
        let input_literal = |input: &str| match self.g.incoming.get(&(node.id.clone(), input.to_string())) {
            Some(Src::Node { id, .. }) => self.literal_shaped(self.g.node(id)),
            _ => false,
        };
        match node.data.name.as_str() {
            "const" => literal(node.param("type"), node.param("value")).is_some() && (node.param("type") != "Json" || node.param("value") == "[]"),
            "record" => true,
            "with_field" => {
                let base = matches!(self.g.incoming.get(&(node.id.clone(), "record".to_string())), Some(Src::Node { id, .. }) if matches!(self.g.node(id).data.name.as_str(), "record" | "with_field"));
                base && input_literal("record") && input_literal("value")
            }
            "list" => node.data.inputs.len() >= 2 && node.data.inputs.iter().all(|p| input_literal(&p.name)),
            "append" => self.inline_source(node, "list", "const").is_some_and(|c| c.param("type") == "Json" && c.param("value") == "[]") && input_literal("item"),
            _ => false,
        }
    }

    // ── before writing ───────────────────────────────────────────────────────────────────────

    /// What the block needs settled before its statements: its switches, the values bound for
    /// how they would read inline, a function's outputs, the names its functions written in
    /// place use from around them.
    fn prepare(&mut self) -> Result<(), String> {
        // Matches, written as a `switch` where ts.rs reads one back the same.
        if self.kind != Kind::Lambda {
            // A chain's inner links are matches too: only the outermost is one.
            let found: Vec<(&'g Node, MatchShape<'g>)> = self.g.nodes.iter().filter_map(|n| self.match_shape(n).map(|m| (*n, m))).collect();
            let inner: HashSet<String> = found.iter().flat_map(|(_, m)| m.parts.iter().map(|p| p.id.clone())).collect();
            for (n, m) in found {
                if inner.contains(&n.id) || !self.switch_reads_back(&m) {
                    continue;
                }
                self.absorbed.extend(m.parts.iter().map(|n| n.id.clone()));
                let patterns: usize = m.arms.iter().map(|(ps, _)| ps.len()).sum();
                if let Src::Node { id, .. } = &m.subject {
                    let s = self.g.node(id);
                    if !self.is_request(id) && !self.is_tick(id) && self.pass_through(s).is_none() && self.free(s) && self.g.consumed_total(s) == patterns {
                        self.absorbed.insert(id.clone());
                    }
                }
                // The values a `let`'s cases give it are named after it in the text read back.
                let base = self.p.bindings.get(&n.id).map(|b| ts_name(strip_version(b)));
                for g in m.arms.iter().map(|(_, g)| *g).chain(m.els) {
                    if let Some(Src::Node { id, .. }) = self.g.incoming.get(&(g.id.clone(), "value".to_string()))
                        && let Some(b) = self.p.bindings.get(id)
                        && base.as_deref().is_some_and(|base| ts_name(strip_version(b)) == base)
                    {
                        self.ignored.insert(id.clone());
                    }
                }
                if self.p.bindings.contains_key(&n.id) {
                    self.ignored.insert(n.id.clone());
                }
                self.matches.insert(n.id.clone(), m);
            }
        }
        // A function of several outputs read whole is its call: `summary(x)`, not a `const`
        // whose fields make the record again.
        let g = self.g;
        for n in &g.nodes {
            if let Some(whole) = self.whole_call(n) {
                self.wholes.insert(n.id.clone(), whole);
            }
        }
        // A ternary's condition, read by both its sides, is written once, in place.
        for n in &g.nodes {
            let Some([then, ..]) = self.ternary(n) else { continue };
            if let Some(Src::Node { id, .. }) = g.incoming.get(&(then.id.clone(), "cond".to_string())) {
                let c = g.node(id);
                if self.pass_through(c).is_none() && self.free(c) && g.consumed_total(c) == 2 {
                    self.absorbed.insert(id.clone());
                }
            }
        }
        // A route answering a match: a `switch` whose cases return.
        if self.kind == Kind::Route
            && let Some(respond) = self.g.nodes.iter().find(|n| n.is_template("respond"))
            && respond.param("headers") != "true"
            && respond.param("status_input") != "true"
            && let Some(top) = self.source_id(respond, "body")
            && self.matches.contains_key(&top)
            && self.g.consumed_total(self.g.node(&top)) == 1
            && literal("Number", respond.param("status")).is_some()
            && literal("Number", respond.param("missing")).is_some()
        {
            self.switch_answer = Some(top);
        }
        // A function's outputs: those written as statements, and the others.
        if matches!(self.kind, Kind::Function | Kind::Lambda) {
            self.outputs()?;
        }
        // What a `coalesce` falls back to, written as a literal, would read as a `default`
        // (`x ?? 0`), parentheses or not: it gets a name.
        for n in &g.nodes {
            if n.data.name != "coalesce" || self.matches.contains_key(&n.id) || self.absorbed.contains(&n.id) {
                continue;
            }
            if let Some(Src::Node { id, .. }) = g.incoming.get(&(n.id.clone(), "b".to_string()))
                && self.literal_shaped(g.node(id))
            {
                self.held.insert(id.clone());
            }
        }
        // A store's read written after the run writes to it is refused (a write is made when the
        // run ends): one the answer reads gets a name, a statement the writes go after.
        let written: HashSet<String> = g
            .nodes
            .iter()
            .filter(|n| matches!(n.data.name.as_str(), "collection_put" | "collection_delete" | "file_write" | "file_delete" | "file_copy") && g.consumed_total(n) == 0)
            .map(|n| self.store_of(n))
            .collect();
        if !written.is_empty() {
            let mut stack: Vec<&'g Node> = Vec::new();
            if let Some(r) = g.nodes.iter().find(|n| n.is_template("respond")) {
                stack.push(r);
            }
            for (id, _, _) in &g.outs {
                stack.push(g.node(id));
            }
            let mut seen = HashSet::new();
            while let Some(cur) = stack.pop() {
                if !seen.insert(cur.id.clone()) {
                    continue;
                }
                for p in &cur.data.inputs {
                    let Some(Src::Node { id, .. }) = g.incoming.get(&(cur.id.clone(), p.name.clone())) else { continue };
                    let s = g.node(id);
                    let reads = matches!(s.data.name.as_str(), "collection_get" | "collection_list" | "file_read" | "file_read_bytes" | "file_list" | "file_lines" | "file_slice");
                    if reads && written.contains(&self.store_of(s)) {
                        self.held.insert(s.id.clone());
                    } else if self.inline(s) || self.matches.contains_key(&s.id) || self.absorbed.contains(&s.id) {
                        stack.push(s);
                    }
                }
            }
        }
        // The values a function written in place uses from around it: each bound under the name
        // the function's input has, so the lowering passes the same.
        let maps: Vec<&'g Node> = self.g.nodes.iter().copied().filter(|n| PASSES_EXTRA.contains(&n.data.name.as_str()) && n.data.body == PlacedBody::Primitive && !n.param("fn").is_empty()).collect();
        for n in maps {
            let (own, base) = (function_takes(&n.data.name), leaf_inputs(&n.data.name));
            if n.data.inputs.len() <= base {
                continue;
            }
            let (_, unit) = resolve(self.p.store, n.param("fn"))?;
            let ports: Vec<String> = unit.inputs.iter().skip(own).map(|p| p.name.clone()).collect();
            if ports.windows(2).any(|w| w[0] >= w[1]) || ports.len() != n.data.inputs.len() - base {
                return Err(format!("{}: the values its function uses from around it aren't in the order TypeScript passes them", n.data.name));
            }
            for (port, input) in ports.iter().zip(&n.data.inputs[base..]) {
                match self.g.incoming.get(&(n.id.clone(), input.name.clone())) {
                    Some(Src::In(p)) if p == port => {}
                    Some(Src::Node { id, output }) if self.is_request(id) && (output == "request" || output == "file" || output == "message" || output == "event" || output == "input") && local_name(&self.request.as_ref().expect("checked").1) == *port => {}
                    // A part of what the handler received, or a clock's tick: no node, so a name
                    // for it is a `const` of its own, `const r = req.json`.
                    Some(Src::Node { id, output }) if self.is_request(id) || self.is_tick(id) => {
                        let key = (id.clone(), output.clone());
                        let ts = ts_name(port);
                        match self.captured.get(&key) {
                            Some(t) if *t == ts => {}
                            Some(_) => return Err(format!("{}: a value its function uses has two names", n.data.name)),
                            None => {
                                if local_name(&ts) != *port || !self.ok(&ts) {
                                    return Err(format!("{}: the value its function uses as \"{port}\" has no name TypeScript gives it", n.data.name));
                                }
                                let source = self.value(&Src::Node { id: id.clone(), output: output.clone() })?.0;
                                self.claim(&ts);
                                self.aliases.push(Stmt { text: format!("const {ts} = {source}"), ..Default::default() });
                                self.captured.insert(key, ts);
                            }
                        }
                    }
                    Some(Src::Node { id, .. }) if !self.is_request(id) && !self.is_tick(id) && self.g.node(id).data.outputs.len() == 1 => {
                        let ts = ts_name(port);
                        if local_name(&ts) != *port || self.forced.get(id).is_some_and(|t| *t != ts) {
                            return Err(format!("{}: the value its function uses as \"{port}\" has no name TypeScript gives it", n.data.name));
                        }
                        self.force(id, &ts)?;
                    }
                    _ => return Err(format!("{}: the value its function uses as \"{port}\" isn't one TypeScript can name", n.data.name)),
                }
            }
        }
        // A call inside a statement that waits for a value would wait with it: bound on its own
        // when it doesn't.
        let tied: Vec<&'g Node> = self.g.nodes.iter().copied().filter(|n| self.tied_elsewhere(n)).collect();
        for n in tied {
            let tie = self.g.incoming.get(&(n.id.clone(), TRIGGER.to_string())).cloned();
            let mut stack: Vec<&'g Node> = vec![n];
            while let Some(cur) = stack.pop() {
                for p in &cur.data.inputs {
                    let Some(Src::Node { id, .. }) = self.g.incoming.get(&(cur.id.clone(), p.name.clone())) else { continue };
                    let s = self.g.node(id);
                    if !self.inline(s) {
                        continue;
                    }
                    if s.data.inputs.iter().any(|q| q.name == TRIGGER) && self.g.incoming.get(&(s.id.clone(), TRIGGER.to_string())).cloned() != tie {
                        self.held.insert(s.id.clone());
                    } else {
                        stack.push(s);
                    }
                }
            }
        }
        Ok(())
    }

    /// Node `id` bound under `ts`, or why it can't be.
    fn force(&mut self, id: &str, ts: &str) -> Result<(), String> {
        if self.forced.get(id).is_some_and(|t| t == ts) {
            return Ok(());
        }
        if !self.ok(ts) {
            return Err(format!("the name \"{ts}\" is taken where TypeScript needs it for a value"));
        }
        self.claim(ts);
        self.forced.insert(id.to_string(), ts.to_string());
        Ok(())
    }

    /// A function's outputs: the last ones written as statements where the lowering names them so
    /// after their leaves, the others returned, each bound under its name where it can be.
    fn outputs(&mut self) -> Result<(), String> {
        let mut outs = self.g.outs.clone();
        for (_, _, port) in &outs {
            if outs.iter().filter(|(_, _, p)| p == port).count() > 1 {
                return Err(format!("the output \"{port}\" is given twice"));
            }
        }
        // The answers of the calls tied to its trigger leave last, named by the lowering, which
        // names them again (`call_port`): they aren't returned.
        if self.kind == Kind::Function {
            let names = |k: usize| {
                let mut taken: Vec<String> = outs[..k].iter().map(|(_, _, p)| p.clone()).collect();
                for _ in k..outs.len() {
                    let p = call_port(|n| taken.iter().any(|t| t == n) || self.inputs.contains_key(n));
                    taken.push(p);
                }
                taken.split_off(k)
            };
            let called = |k: usize| {
                outs[k..].iter().all(|(id, output, port)| self.calls.contains(port) && !outs[..k].iter().any(|(i, o, _)| i == id && o == output))
                    && names(k) == outs[k..].iter().map(|(_, _, p)| p.clone()).collect::<Vec<_>>()
            };
            if let Some(k) = (1..outs.len()).find(|&k| called(k)) {
                outs.truncate(k);
            }
        }
        let statementable = |b: &Block, id: &str| {
            let n = b.g.node(id);
            n.data.outputs.len() == 1 && (n.data.outputs[0].ty == WType::Effect || CALLS.contains(&n.data.name.as_str())) && b.g.consumed_total(n) == 1 && n.data.body == PlacedBody::Primitive
        };
        let mut split = outs.len();
        if self.kind == Kind::Function {
            for k in (1..outs.len()).rev() {
                let tail = &outs[k..];
                if !tail.iter().all(|(id, _, _)| statementable(self, id)) {
                    continue;
                }
                let declared: Vec<String> = outs[..k].iter().map(|(_, _, p)| p.clone()).collect();
                let leaves: Vec<String> = tail.iter().map(|(id, _, _)| self.g.node(id).data.name.clone()).collect();
                if auto_names(&declared, &leaves) == tail.iter().map(|(_, _, p)| p.clone()).collect::<Vec<_>>() {
                    split = k;
                    break;
                }
            }
        }
        self.auto = outs[split..].iter().map(|(id, _, _)| id.clone()).collect();
        self.declared = outs[..split].iter().map(|(id, output, port)| (port.clone(), Src::Node { id: id.clone(), output: output.clone() })).collect();
        if self.declared.is_empty() {
            return Err("a function returns its value: it has no output TypeScript returns".into());
        }
        // Each output's value bound under the output's name, where it is a node of one output
        // used there alone: `const slug = …; return slug`.
        let declared = self.declared.clone();
        for (port, src) in &declared {
            let Src::Node { id, .. } = src else { continue };
            let n = self.g.node(id);
            let ts = ts_name(port);
            let alone = n.data.outputs.len() == 1 && self.g.consumed_total(n) == 1 && !self.is_request(id) && !self.is_tick(id);
            let nameable = port != "out" && is_identifier(&ts) && local_name(&ts) == *port && !self.forced.contains_key(id);
            if self.kind == Kind::Function && nameable && (alone || self.matches.contains_key(id)) && self.ok(&ts) {
                self.force(id, &ts)?;
            }
        }
        Ok(())
    }

    // ── statements ───────────────────────────────────────────────────────────────────────────

    /// The block's statements, its answer last.
    fn body(&mut self) -> Result<Vec<Stmt>, String> {
        self.prepare()?;
        let mut stmts: Vec<Stmt> = std::mem::take(&mut self.aliases);
        let mut autos: HashMap<String, Stmt> = HashMap::new();
        let mut respond: Option<&'g Node> = None;
        let order = self.g.order.clone();
        for id in &order {
            let node = self.g.node(id);
            if self.is_request(id) || self.is_tick(id) || self.pass_through(node).is_some() {
                continue;
            }
            if node.is_template("respond") {
                if !matches!(self.kind, Kind::Route | Kind::Action) {
                    return Err("an answer outside a route or an action: a clock and a file handler answer nobody".into());
                }
                respond = Some(node);
                continue;
            }
            if self.switch_answer.as_deref() == Some(id.as_str()) {
                continue;
            }
            if self.auto.contains(id) {
                self.begin();
                let mut s = self.effect_statement(node)?;
                self.after_of(node, &mut s)?;
                autos.insert(id.clone(), s);
                continue;
            }
            if self.matches.contains_key(id) {
                let name = self.bind(node);
                let mut lets = self.let_switch(node, &name)?;
                stmts.append(&mut lets);
                continue;
            }
            if self.inline(node) || self.wholes.values().any(|(call, _)| call.id == *id) {
                continue;
            }
            self.begin();
            let total = self.g.consumed_total(node);
            let single = self.one_value(node);
            let effect = single && node.data.outputs[0].ty == WType::Effect;
            let call = single && CALLS.contains(&node.data.name.as_str());
            let statement = total == 0 && (effect || call || (!single && node.data.outputs.iter().any(|o| o.ty == WType::Effect)));
            if statement {
                if !self.handler() {
                    return Err(format!("the function's {} leaves through no output, which TypeScript can't say: wire it to one", node.data.name));
                }
                let mut s = if effect {
                    self.effect_statement(node)?
                } else {
                    let text = self.render(node, "")?.0;
                    self.statement(text)
                };
                self.after_of(node, &mut s)?;
                stmts.push(s);
                continue;
            }
            if node.data.name == "store_write" {
                return Err(format!("state.set({}, …)'s answer is used, which TypeScript can't say: state.{} = value is a statement; wait for the write with if (…) in a later version", node.param("key"), node.param("key")));
            }
            // A binding. Rendered first: a function is written (and its name taken) on first use.
            let (text, _) = self.render(node, if single { &node.data.outputs[0].name } else { "" })?;
            let name = self.bind(node);
            if self.tied_elsewhere(node) {
                let mut s = self.statement(format!("{name} = {text}"));
                s.declares = Some(name);
                self.after_of(node, &mut s)?;
                stmts.push(s);
            } else {
                stmts.push(self.statement(format!("const {name} = {text}")));
            }
        }
        // The answer, last.
        let mut answer = Vec::new();
        match self.kind {
            Kind::Route | Kind::Action => {
                if let Some(r) = respond {
                    answer = self.answer(r)?;
                }
            }
            Kind::Function | Kind::Lambda => answer = self.returns()?,
            Kind::Every | Kind::File | Kind::Mail | Kind::Feed | Kind::Queue => {}
        }
        if self.handler() {
            reads_first(&mut stmts);
        }
        for id in self.auto.clone() {
            stmts.push(autos.remove(&id).expect("written"));
        }
        let mut out = self.ifs(stmts);
        out.extend(answer);
        if self.kind == Kind::Function && self.awaits {
            return Err("a function computes: what it waits for (a request, another flow's action) is a handler's".into());
        }
        Ok(out)
    }

    /// A new statement starts: nothing rendered into it yet.
    fn begin(&mut self) {
        self.touched.clear();
        self.reads.clear();
    }

    /// The statement being written, `text`, with the nodes rendered into it.
    fn statement(&mut self, text: String) -> Stmt {
        Stmt { text, nodes: std::mem::take(&mut self.touched), reads: std::mem::take(&mut self.reads), ..Default::default() }
    }

    /// What `s`, `node`'s statement, waits for, when its trigger is fed by a value: the value as
    /// the text reads it.
    fn after_of(&mut self, node: &Node, s: &mut Stmt) -> Result<(), String> {
        if !self.tied_elsewhere(node) {
            return Ok(());
        }
        let assigned = s.declares.is_some() || s.writes.is_some() || s.text.starts_with("state.");
        if !assigned && !self.sink(node) {
            return Err(format!("{} waits for a value, which TypeScript says only of a write, a request, a send or another flow's action: bind it, or wait for its value with ??", node.data.name));
        }
        let saved = std::mem::take(&mut self.touched);
        let x = self.arg(node, TRIGGER)?.map(|v| wrap(v, REL)).expect("tied");
        s.nodes.append(&mut self.touched);
        self.touched = saved;
        s.after = Some(x);
        Ok(())
    }

    /// Whether `node`, written as a statement, is one ts.rs ties to what an `if (x !== undefined)`
    /// waits for: a request, another flow's action, a send, a write by method.
    fn sink(&self, node: &Node) -> bool {
        let name = node.data.name.as_str();
        matches!(name, "http_request" | "http_upload" | "http_stream" | "flow_call" | "channel_send" | "mail_send" | "queue_send" | "log" | "collection_put" | "collection_delete" | "db_exec" | "db_insert" | "db_update" | "db_delete")
            || (name.starts_with("file_") && !matches!(name, "file_list" | "file_lines" | "file_slice" | "file_read" | "file_read_bytes" | "file_serve"))
    }

    /// The statements in order, those waiting for one value together in one
    /// `if (x !== undefined)`: one short statement on the `if`'s line, else in braces, a binding
    /// given its value inside declared before it with `let`.
    fn ifs(&self, stmts: Vec<Stmt>) -> Vec<Stmt> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < stmts.len() {
            let Some(x) = stmts[i].after.clone() else {
                out.push(stmts[i].clone());
                i += 1;
                continue;
            };
            let mut group = vec![stmts[i].clone()];
            i += 1;
            // Several share one `if` only when what they wait for is a name: ts.rs names anything
            // else once per `if`.
            while is_identifier(&x) && i < stmts.len() && stmts[i].after.as_deref() == Some(x.as_str()) {
                group.push(stmts[i].clone());
                i += 1;
            }
            let lets: Vec<String> = group.iter().filter_map(|s| s.declares.clone()).collect();
            for l in &lets {
                out.push(Stmt { text: format!("let {l}"), ..Default::default() });
            }
            if let [one] = group.as_slice()
                && lets.is_empty()
                && !one.text.contains('\n')
                && one.text.len() + x.len() < 80
            {
                out.push(Stmt { text: format!("if ({x} !== undefined) {}", one.text), kids: group, ..Default::default() });
                continue;
            }
            let text = format!("if ({x} !== undefined) {{\n{}\n}}", group.iter().map(|s| s.text.clone()).collect::<Vec<_>>().join("\n"));
            out.push(Stmt { text, kids: group, ..Default::default() });
        }
        out
    }

    /// A name for `node`: one it must have, the one it had in a text, else one from what it is.
    fn bind(&mut self, node: &Node) -> String {
        if let Some(name) = self.bound.get(&node.id) {
            return name.clone();
        }
        if let Some(name) = self.forced.get(&node.id).cloned() {
            self.bound.insert(node.id.clone(), name.clone());
            return name;
        }
        let suggested = match self.p.bindings.get(&node.id).filter(|b| !hidden(b)) {
            Some(b) if self.matches.contains_key(&node.id) => ts_name(strip_version(b)),
            Some(b) if !self.ignored.contains(&node.id) => ts_name(b),
            _ => self.suggestion(node),
        };
        let base = identifier(&suggested);
        let mut name = base.clone();
        let mut n = 2;
        while !self.ok(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        self.claim(&name);
        self.bound.insert(node.id.clone(), name.clone());
        name
    }

    /// Whether the block may declare `ts`: a name TypeScript writes, none of the file's, none the
    /// block has, and one the lowering knows by a name nothing else here has, which reads back as
    /// `ts` again.
    fn ok(&self, ts: &str) -> bool {
        let lowered = local_name(ts);
        is_identifier(ts) && !self.p.avoid.contains(ts) && !self.used.contains(ts) && !self.lowered.contains(&lowered) && !self.p.avoid.contains(&lowered) && ts_name(&lowered) == ts && !ts.starts_with('$')
    }

    fn claim(&mut self, ts: &str) {
        self.used.insert(ts.to_string());
        self.lowered.insert(local_name(ts));
    }

    /// A name from what `node` is.
    fn suggestion(&self, node: &Node) -> String {
        if let Some((call, _)) = self.wholes.get(&node.id) {
            return self.suggestion(call);
        }
        let d = &node.data;
        let s = match d.name.as_str() {
            "field" | "secret" | "await" | "approval" => node.param("name"),
            "store_read" => node.param("key"),
            "collection_get" => "item",
            "collection_list" => "page",
            "collection_put" => "saved",
            "collection_delete" => "removed",
            "file_read" => "text",
            "file_read_bytes" => "bytes",
            "file_list" => "files",
            "file_handle" => "file",
            "file_serve" => "served",
            "file_lines" => "lines",
            "file_slice" => "part",
            "file_write" => "written",
            "http_request" | "http_upload" | "http_stream" => "response",
            "channel_send" => "sent",
            "mail_send" => "mailed",
            "log" => "logged",
            "queue_send" => "queued",
            "flow_call" => node.param("action"),
            "flow_start" => "started",
            "stripe_verify" => "verified",
            "clock" => "t",
            "const" => "c",
            "coalesce" if self.matches.contains_key(&node.id) => "picked",
            _ => match &d.body {
                PlacedBody::Composite { hash } => return self.p.def_names.get(hash).or(self.p.seeded.get(hash)).cloned().map(|n| format!("{n}_result")).unwrap_or_else(|| d.name.clone()),
                _ => d.name.as_str(),
            },
        };
        if s.is_empty() { "v".into() } else { s.to_string() }
    }

    /// An effect nobody reads, as a statement: in the assignment form where there is one.
    fn effect_statement(&mut self, node: &Node) -> Result<Stmt, String> {
        let d = &node.data;
        let (text, writes) = match d.name.as_str() {
            "store_write" => {
                self.p.uses_state = true;
                (format!("state.{} = {}", node.param("key"), self.required(node, "value", 0)?), None)
            }
            "collection_put" => {
                let c = node.param("collection").to_string();
                (format!("{c}[{}] = {}", self.required(node, "key", 0)?, self.required(node, "value", 0)?), Some(c))
            }
            "collection_delete" => {
                let c = node.param("collection").to_string();
                (format!("delete {c}[{}]", self.required(node, "key", 0)?), Some(c))
            }
            "file_write" => {
                let f = node.param("folder").to_string();
                (format!("{f}[{}] = {}", self.required(node, "name", 0)?, self.required(node, "text", 0)?), Some(f))
            }
            "file_delete" => {
                let f = node.param("folder").to_string();
                (format!("delete {f}[{}]", self.required(node, "name", 0)?), Some(f))
            }
            // A copy reads as a write when what it copies is a handle read here (the lowering
            // makes it a copy again), else as the method.
            "file_copy" if self.copies_a_handle(node) => {
                let f = node.param("folder").to_string();
                (format!("{f}[{}] = {}", self.required(node, "name", 0)?, self.required(node, "from", 0)?), Some(f))
            }
            _ => (self.render(node, "")?.0, None),
        };
        self.touched.push(node.id.clone());
        let mut s = self.statement(text);
        s.writes = writes;
        Ok(s)
    }

    /// Whether `node`'s `from` is a file's handle, straight from `file_handle` or the file a
    /// handler was given.
    fn copies_a_handle(&self, node: &Node) -> bool {
        match self.g.incoming.get(&(node.id.clone(), "from".to_string())) {
            Some(Src::Node { id, .. }) => self.g.node(id).data.name == "file_handle" || self.g.node(id).is_template("on_file"),
            _ => false,
        }
    }

    /// A match bound by name: `let name`, then a `switch` whose cases each give it its value.
    fn let_switch(&mut self, top: &Node, name: &str) -> Result<Vec<Stmt>, String> {
        let m = self.matches[&top.id].clone();
        self.begin();
        self.touched.push(top.id.clone());
        for n in &m.parts {
            self.touched.push(n.id.clone());
        }
        let subject = self.subject(&m)?;
        let mut head = self.statement(String::new());
        let mut kids = Vec::new();
        let mut lines = vec![format!("switch ({subject}) {{")];
        let count = m.arms.len() + usize::from(m.els.is_some());
        let mut k = 0;
        for (consts, guard) in m.arms.iter().map(|(c, g)| (Some(c), *g)).chain(m.els.map(|e| (None, e))) {
            match consts {
                Some(consts) => lines.extend(consts.iter().map(|c| format!("case {}:", pattern_literal(c.param("type"), c.param("value")).expect("a literal pattern")))),
                None => lines.push("default:".into()),
            }
            self.begin();
            let value = self.required(guard, "value", 0)?;
            lines.push(format!("{name} = {value}"));
            k += 1;
            if k < count {
                lines.push("break".into());
            }
            kids.push(self.statement(String::new()));
        }
        lines.push("}".into());
        head.text = lines.join("\n");
        head.kids = kids;
        Ok(vec![Stmt { text: format!("let {name}"), ..Default::default() }, head])
    }

    /// A switch's subject, as its `switch (…)` reads it.
    fn subject(&mut self, m: &MatchShape) -> Result<String, String> {
        Ok(self.value(&m.subject)?.0)
    }

    /// What the route or action answers, as its `return`, last.
    fn answer(&mut self, node: &'g Node) -> Result<Vec<Stmt>, String> {
        self.begin();
        self.touched.push(node.id.clone());
        let (status, missing) = (node.param("status").to_string(), node.param("missing").to_string());
        if let Some(top) = self.switch_answer.clone() {
            return self.switch_returns(node, &top, &status, &missing);
        }
        let body = self.arg(node, "body")?;
        let headers = if node.param("headers") == "true" { self.arg(node, "headers")? } else { None };
        let wired = if node.param("status_input") == "true" { self.arg(node, "status")? } else { None };
        // The body of a streamed call: the reply forwards it, with the call's status.
        let streamed = self.source_id(node, "body").map(|id| self.g.node(&id)).is_some_and(|f| {
            f.data.name == "field" && f.param("name") == "body" && self.source_id(f, "record").is_some_and(|c| self.g.node(&c).data.name == "http_stream" && self.g.node(&c).param("pipe").is_empty())
        });
        let calls_alone = self.g.nodes.iter().any(|n| CALLS.contains(&n.data.name.as_str())) && !self.g.nodes.iter().any(|n| n.data.outputs.iter().any(|p| p.ty == WType::Effect));
        let nothing = body.is_none() && headers.is_none() && wired.is_none() && status == "204" && missing == "404";
        if self.kind == Kind::Action {
            if nothing && calls_alone {
                return Ok(vec![]);
            }
            let Some(body) = body.filter(|_| headers.is_none() && wired.is_none() && status == "200" && missing == "404" && !streamed) else {
                return Err("an action answers with a value it returns: a status, headers or an answer for nothing have no TypeScript form there".into());
            };
            return Ok(vec![self.statement(format!("return {}", body.0))]);
        }
        if nothing && calls_alone {
            return Ok(vec![]);
        }
        let value = match body {
            None => {
                if headers.is_some() {
                    return Err("an answer with headers and no body has no TypeScript form: respond(body, { headers })".into());
                }
                self.p.vocab.insert("status".into());
                (format!("status({})", wired.map(|w| w.0).unwrap_or_else(|| status.clone())), MEMBER)
            }
            Some(b) if headers.is_some() || streamed => {
                self.p.vocab.insert("respond".into());
                let mut opts = Vec::new();
                if let Some(h) = headers {
                    opts.push(format!("headers: {}", h.0));
                }
                if streamed {
                    opts.push("stream: true".into());
                    if let Some(w) = &wired {
                        let own = b.0.strip_suffix(".body").is_some_and(|call| w.0 == format!("{call}.status"));
                        if !own {
                            return Err(format!("a streamed reply answers with its call's status, not {}", w.0));
                        }
                    }
                } else if let Some(w) = wired {
                    opts.push(format!("status: {}", w.0));
                }
                if !streamed && node.param("status_input") != "true" && status != "200" {
                    opts.push(format!("status: {status}"));
                }
                (format!("respond({}, {{ {} }})", b.0, opts.join(", ")), MEMBER)
            }
            Some(b) => match wired {
                Some(w) => {
                    self.p.vocab.insert("status".into());
                    (format!("status({}, {})", w.0, b.0), MEMBER)
                }
                None if status != "200" => {
                    self.p.vocab.insert("status".into());
                    (format!("status({status}, {})", b.0), MEMBER)
                }
                None => b,
            },
        };
        let text = if missing != "404" { self.missing(value, &missing) } else { value.0 };
        Ok(vec![self.statement(format!("return {text}"))])
    }

    /// `value ?? status(missing)`: the answer when the value is absent.
    fn missing(&mut self, value: (String, u8), missing: &str) -> String {
        self.p.vocab.insert("status".into());
        let left = if value.1 == COALESCE || value.1 >= BITOR { value.0 } else { format!("({})", value.0) };
        format!("{left} ?? status({missing})")
    }

    /// A route answering a match: `switch (s) { case p: return v … }`, each case returning its
    /// arm's answer, then the answer when no case matched.
    fn switch_returns(&mut self, respond: &Node, top: &str, status: &str, missing: &str) -> Result<Vec<Stmt>, String> {
        let m = self.matches[top].clone();
        self.touched.push(top.to_string());
        for n in &m.parts {
            self.touched.push(n.id.clone());
        }
        let _ = respond;
        let subject = self.subject(&m)?;
        let mut head = self.statement(String::new());
        let mut kids = Vec::new();
        let mut lines = vec![format!("switch ({subject}) {{")];
        for (consts, guard) in m.arms.iter().map(|(c, g)| (Some(c), *g)).chain(m.els.map(|e| (None, e))) {
            match consts {
                Some(consts) => lines.extend(consts.iter().map(|c| format!("case {}:", pattern_literal(c.param("type"), c.param("value")).expect("a literal pattern")))),
                None => lines.push("default:".into()),
            }
            self.begin();
            let value = self.arg(guard, "value")?.ok_or("a match's arm has no value")?;
            let value = if status != "200" {
                self.p.vocab.insert("status".into());
                (format!("status({status}, {})", value.0), MEMBER)
            } else {
                value
            };
            let text = if missing != "404" { self.missing(value, missing) } else { value.0 };
            lines.push(format!("return {text}"));
            kids.push(self.statement(String::new()));
        }
        lines.push("}".into());
        head.text = lines.join("\n");
        head.kids = kids;
        let mut out = vec![head];
        if m.els.is_none() && missing != "404" {
            self.p.vocab.insert("status".into());
            out.push(Stmt { text: format!("return status({missing})"), ..Default::default() });
        }
        Ok(out)
    }

    /// What a function returns: its declared outputs, `return x`, `return { a, b: … }`.
    fn returns(&mut self) -> Result<Vec<Stmt>, String> {
        self.begin();
        let declared = self.declared.clone();
        let mut fields = Vec::new();
        for (port, src) in &declared {
            if let Src::Node { id, .. } = src
                && self.is_request(id)
            {
                return Err(format!("the output \"{port}\" is the request passed straight through: put a node between"));
            }
            let value = self.value(src)?;
            fields.push((port.clone(), value));
        }
        let text = match fields.as_slice() {
            [(port, value)] if port == "out" => {
                let bound = matches!(&declared[0].1, Src::Node { id, .. } if self.bound.contains_key(id));
                if bound { format!("return {{ out: {} }}", value.0) } else { format!("return {}", value.0) }
            }
            [(port, value)] if matches!(&declared[0].1, Src::Node { id, .. } if self.forced.get(id).is_some_and(|t| local_name(t) == *port && *t == value.0)) => format!("return {}", value.0),
            _ => format!("return {{ {} }}", fields.iter().map(|(k, v)| field(k, &v.0)).collect::<Vec<_>>().join(", ")),
        };
        Ok(vec![self.statement(text)])
    }
}

/// Whether `name` is one ts.rs makes for a value the text doesn't name (`cond_1`, `subject_2`,
/// `parts_3`): the value is written where it was, and the name made again.
fn hidden(name: &str) -> bool {
    ["cond_", "subject_", "parts_"].iter().any(|p| name.strip_prefix(p).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
}

/// `name` without the `_2` a new value of it was given: `r_4` is `r`.
fn strip_version(name: &str) -> &str {
    match name.rsplit_once('_') {
        Some((stem, n)) if !stem.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => stem,
        _ => name,
    }
}

/// The names the lowering gives a function's effects written as statements, after `declared`:
/// each after its leaf, a second one numbered, and an underscore after one that clashes.
fn auto_names(declared: &[String], leaves: &[String]) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut names: Vec<String> = declared.to_vec();
    let mut out = Vec::new();
    for leaf in leaves {
        let base = match leaf.as_str() {
            "collection_put" => "put",
            "collection_delete" => "delete",
            "file_write" => "written",
            "file_copy" => "copied",
            "file_create" | "file_create_copy" => "created",
            "file_delete" => "deleted",
            "store_write" => "write",
            "egress" | "channel_send" => "sent",
            "mail_send" => "mailed",
            "log" => "logged",
            "queue_send" => "queued",
            _ => "effect",
        };
        let n = counts.entry(base).or_insert(0);
        *n += 1;
        let mut name = if *n == 1 { base.to_string() } else { format!("{base}_{n}") };
        while names.contains(&name) {
            name.push('_');
        }
        names.push(name.clone());
        out.push(name);
    }
    out
}

/// The statements in the text's order, but a write to a collection or folder moved after the
/// later ones that read it: a write is made when the run ends, so a read anywhere reads what was
/// there before it, and ts.rs, which reads in order, refuses a read after it.
fn reads_first(stmts: &mut Vec<Stmt>) {
    let mut i = 0;
    let mut moved = 0;
    while i < stmts.len() {
        if let Some(store) = stmts[i].writes.clone()
            && let Some(last) = (i + 1..stmts.len()).rev().find(|&j| stmts[j].reads.contains(&store))
            && moved < stmts.len() * stmts.len()
        {
            let w = stmts.remove(i);
            stmts.insert(last, w);
            moved += 1;
            continue;
        }
        i += 1;
    }
}

impl<'p, 's, 'g> Block<'p, 's, 'g> {
    // ── expressions ──────────────────────────────────────────────────────────────────────────

    /// Whether node `n` gives one value, its first output: it has one, or it is a function whose
    /// others answer the calls it makes.
    fn one_value(&self, n: &Node) -> bool {
        let outs = n.data.outputs.len();
        outs == 1 || outs > 1 && {
            let calls = self.answers(n);
            outs - calls.len() == 1 && !calls.contains(&0)
        }
    }

    /// The outputs of node `n`, a function placed, that answer a call it makes (`call_outputs`),
    /// by index: neither its value nor an effect.
    fn answers(&self, n: &Node) -> Vec<usize> {
        let PlacedBody::Composite { hash } = &n.data.body else { return vec![] };
        let Ok((_, unit)) = resolve(self.p.store, hash) else { return vec![] };
        let calls = call_outputs(unit, self.p.store);
        unit.outputs.iter().enumerate().filter(|(_, p)| calls.contains(&p.name)).map(|(k, _)| k).collect()
    }

    fn touch(&mut self, n: &Node) {
        self.touched.push(n.id.clone());
    }

    /// What feeds `node`'s `input`, as text, if anything does.
    fn arg(&mut self, node: &Node, input: &str) -> Result<Option<(String, u8)>, String> {
        let Some(src) = self.g.incoming.get(&(node.id.clone(), input.to_string())).cloned() else { return Ok(None) };
        Ok(Some(self.value(&src)?))
    }

    /// What feeds `node`'s `input`, in parentheses where it binds less tightly than `min`.
    fn required(&mut self, node: &Node, input: &str, min: u8) -> Result<String, String> {
        match self.arg(node, input)? {
            Some(v) => Ok(wrap(v, min)),
            None => Err(format!("{}: its {input} isn't wired", node.data.name)),
        }
    }

    /// A source as text: a parameter, the handler's parameter or a part of it, a name, or the node
    /// written in place.
    fn value(&mut self, src: &Src) -> Result<(String, u8), String> {
        match src {
            Src::In(port) => match self.inputs.get(port) {
                Some(t) => Ok((t.clone(), ATOM)),
                None => Err(format!("the function's input \"{port}\" is read where TypeScript has no name for it")),
            },
            Src::Node { id, output } => {
                if let Some(name) = self.captured.get(&(id.clone(), output.clone())) {
                    return Ok((name.clone(), ATOM));
                }
                if let Some((req, name)) = &self.request
                    && req == id
                {
                    let name = name.clone();
                    self.param_read = true;
                    return Ok(if ["request", "file", "message", "event", "input"].contains(&output.as_str()) { (name, ATOM) } else { (format!("{name}.{output}"), MEMBER) });
                }
                if self.is_tick(id) {
                    self.p.uses_now = true;
                    return Ok(("now".into(), ATOM));
                }
                if let Some(name) = self.bound.get(id).cloned() {
                    let node = self.g.node(id);
                    return Ok(if self.one_value(node) && node.data.outputs[0].name == *output { (name, ATOM) } else { (format!("{name}.{output}"), MEMBER) });
                }
                let node = self.g.node(id);
                if let Some(port) = self.pass_through(node) {
                    self.touch(node);
                    if let Some(c) = self.inline_source(node, "cond", "const") {
                        self.touch(c);
                    }
                    return self.value(&Src::In(port));
                }
                self.render(node, output)
            }
        }
    }

    /// `node` as an expression, with its precedence; `output` is which, for one of several.
    fn render(&mut self, node: &Node, output: &str) -> Result<(String, u8), String> {
        self.touch(node);
        let d = &node.data;
        match &d.body {
            PlacedBody::Composite { hash } => {
                let hname = self.p.function_for(hash, &d.name)?;
                let mut args = Vec::new();
                for p in d.inputs.iter().filter(|p| p.name != TRIGGER) {
                    args.push(self.required(node, &p.name, COND)?);
                }
                let call = format!("{hname}({})", args.join(", "));
                let one = self.one_value(node) && d.outputs[0].name == output;
                return Ok(if !one && !output.is_empty() { (format!("{call}.{output}"), MEMBER) } else { (call, MEMBER) });
            }
            PlacedBody::Module { .. } => return Err(format!("the module node {} has no TypeScript form: write it as js(…)", d.name)),
            PlacedBody::Template => {
                return match d.name.as_str() {
                    "stripe_verify" => {
                        self.p.vocab.insert("stripe".into());
                        let mut args = Vec::new();
                        for input in ["request", "secret", "now"] {
                            args.push(self.required(node, input, COND)?);
                        }
                        let tolerance = node.param("tolerance");
                        if tolerance != "300" && !tolerance.is_empty() {
                            args.push(format!("{{ tolerance: {} }}", param_value(tolerance)));
                        }
                        let call = format!("stripe.verify({})", args.join(", "));
                        Ok(if output.is_empty() { (call, MEMBER) } else { (format!("{call}.{output}"), MEMBER) })
                    }
                    other => Err(format!("a {other} node where TypeScript has no place for it")),
                };
            }
            PlacedBody::Primitive => {}
        }
        let atom = |s: String| Ok((s, ATOM));
        let member = |s: String| Ok((s, MEMBER));
        let function = matches!(self.kind, Kind::Function | Kind::Lambda);
        match d.name.as_str() {
            "const" => match literal(node.param("type"), node.param("value")).filter(|_| node.param("type") != "Json" || node.param("value") == "[]") {
                Some(l) => Ok(l),
                None => self.generic(node),
            },
            "clock" if self.kind == Kind::Every => self.generic(node),
            "clock" => {
                self.p.uses_now = true;
                atom("now".into())
            }
            "random_bytes" => {
                self.p.uses_random_bytes = true;
                atom("random_bytes".into())
            }
            "random" => {
                self.p.uses_random = true;
                atom("random".into())
            }
            "secret" => atom(self.p.secret(node.param("name"))?),
            "store_read" => {
                self.p.uses_state = true;
                member(format!("state.{}", node.param("key")))
            }
            "field" if is_identifier(node.param("name")) && node.param("name") != "length" => {
                let name = node.param("name").to_string();
                let base = self.receiver(node, "record")?;
                // A field of the request record named as one of the request's parts.
                let clash = matches!(self.g.incoming.get(&(node.id.clone(), "record".into())), Some(Src::Node { id, output }) if self.is_request(id) && output == "request" && self.g.node(id).data.outputs.iter().any(|p| p.name == name))
                    && REQUEST_PARTS.contains(&name.as_str());
                member(if clash { format!("{base}.request.{name}") } else { format!("{base}.{name}") })
            }
            "lookup" => {
                let base = self.required(node, "record", MEMBER)?;
                let key = self.required(node, "key", COND)?;
                member(format!("{base}[{key}]"))
            }
            // `xs[i]`, where `i` is known to be a number and `xs` isn't known to be a text or a
            // record, as the lowering reads it back to `get`; else by its name.
            "get" if self.known(node, "index", WType::Number) && !self.known(node, "list", WType::Text) && !self.known(node, "list", WType::Record) => {
                let base = self.required(node, "list", MEMBER)?;
                let i = self.required(node, "index", COND)?;
                member(format!("{base}[{i}]"))
            }
            "json_parse" if node.param("fn").is_empty() => member(format!("JSON.parse({})", self.required(node, "text", COND)?)),
            "json_parse" => {
                let t = self.required(node, "text", COND)?;
                let f = self.function_text(node)?;
                member(format!("JSON.parse({t}, {f})"))
            }
            "json_stringify" => self.json_stringify(node),
            "keys" => member(format!("Object.keys({})", self.required(node, "record", COND)?)),
            "values" => member(format!("Object.values({})", self.required(node, "record", COND)?)),
            "from_entries" => member(format!("Object.fromEntries({})", self.required(node, "entries", COND)?)),
            "has" => {
                let r = self.required(node, "record", COND)?;
                let k = self.required(node, "key", COND)?;
                member(format!("Object.hasOwn({r}, {k})"))
            }
            "same_value" => {
                let a = self.required(node, "a", COND)?;
                let b = self.required(node, "b", COND)?;
                member(format!("Object.is({a}, {b})"))
            }
            "group_by" => {
                let xs = self.required(node, "list", COND)?;
                let f = self.function_text(node)?;
                member(format!("Object.groupBy({xs}, {f})"))
            }
            "date_part" | "date_text" | "date_set" => self.date_form(node),
            "date_utc" if self.every_wired(node) => {
                let ports: Vec<String> = d.inputs.iter().map(|p| p.name.clone()).collect();
                member(format!("Date.UTC({})", self.texts(node, &ports)?.join(", ")))
            }
            "date_parse" => member(format!("Date.parse({})", self.required(node, "text", COND)?)),
            "uppercase" | "lowercase" => {
                let s = self.receiver(node, "s")?;
                member(format!("{s}.{}()", if d.name == "uppercase" { "toUpperCase" } else { "toLowerCase" }))
            }
            "split" | "join" | "index_of" if self.p.changed_params(node).is_empty() => {
                let (subject, arg, method, then) = match d.name.as_str() {
                    "split" => ("text", "sep", "split", Some("limit")),
                    "join" => ("list", "sep", "join", None),
                    _ => ("haystack", "needle", "indexOf", Some("from")),
                };
                if self.arg_wired(node, subject) && self.arg_wired(node, arg) {
                    let s = self.receiver(node, subject)?;
                    let a = self.required(node, arg, COND)?;
                    let more = match then.filter(|t| self.arg_wired(node, t)) {
                        Some(t) => format!(", {}", self.required(node, t, COND)?),
                        None => String::new(),
                    };
                    return member(format!("{s}.{method}({a}{more})"));
                }
                self.generic(node)
            }
            // `.length`, where what it is read on is known to be a text, a list or bytes, as the
            // lowering picks the leaf by it.
            "text_length" if self.known(node, "s", WType::Text) => member(format!("{}.length", self.receiver(node, "s")?)),
            "length" if self.known_by(node, "list", |t| matches!(t, WType::List(_))) => member(format!("{}.length", self.receiver(node, "list")?)),
            "bytes_length" if self.known(node, "bytes", WType::Bytes) => member(format!("{}.length", self.receiver(node, "bytes")?)),
            "text_at" | "code_at" | "from_codes" | "text_contains" | "trim" | "pad" | "repeat" | "replace" | "normalize" | "uri_encode" | "uri_decode" | "is_well_formed" => self.text_form(node),
            "base64" if node.param("bytes") == "latin1" && node.param("alphabet") == "standard" && self.arg_wired(node, "text") => {
                let f = if node.param("mode") == "decode" { "atob" } else { "btoa" };
                member(format!("{f}({})", self.required(node, "text", COND)?))
            }
            "map" | "fold" | "any" | "all" | "filter" | "find" | "find_index" | "find_last" | "find_last_index" | "sort_by" => self.map(node, None),
            "at" | "indices" | "entries" | "includes" | "last_index_of" | "reverse" | "sort" | "to_spliced" | "with_item" | "fill" | "copy_within" | "flat" | "is_list" | "unique" => self.list_form(node),
            "slice" if matches!(node.param("mode"), "end" | "substring") => self.list_form(node),
            "math" | "max" | "min" | "aggregate" | "atan2" | "power" | "remainder" | "bitwise" | "number_test" | "to_number" | "number_format" => self.number_form(node),
            "concat" => self.concat(node),
            "as_text" => atom(format!("`${{{}}}`", self.required(node, "value", 0)?)),
            "record" => atom("{}".into()),
            "with_field" if let Some((call, links)) = self.wholes.get(&node.id).cloned() => {
                for l in links {
                    self.touch(l);
                }
                self.render(call, "")
            }
            "with_field" => self.record(node),
            "merge" | "insert" if self.every_wired(node) => self.record(node),
            "list" if d.inputs.len() >= 2 => {
                let mut items = Vec::new();
                for p in &d.inputs {
                    items.push(self.required(node, &p.name, COND)?);
                }
                atom(format!("[{}]", items.join(", ")))
            }
            "append" => {
                let item = self.required(node, "item", COND)?;
                if let Some(c) = self.inline_source(node, "list", "const").filter(|c| c.param("type") == "Json" && c.param("value") == "[]") {
                    self.touch(c);
                    return atom(format!("[{item}]"));
                }
                let list = self.required(node, "list", COND)?;
                atom(format!("[...{list}, {item}]"))
            }
            // `typeof x`: what `type_of` says, "undefined" where nothing arrives.
            "default"
                if node.param("type") == "Text"
                    && node.param("value") == "undefined"
                    && let Some(t) = self.inline_source(node, "value", "type_of").filter(|t| self.arg_wired(t, "value")) =>
            {
                self.touch(t);
                Ok((format!("typeof {}", self.required(t, "value", UNARY)?), UNARY))
            }
            "default" => match literal(node.param("type"), node.param("value")) {
                Some((l, _)) => {
                    let v = self.coalesce_left(node, "value")?;
                    Ok((format!("{v} ?? {l}"), COALESCE))
                }
                None => self.generic(node),
            },
            "coalesce" => {
                if let Some(parts) = self.ternary(node) {
                    for p in parts {
                        self.touch(p);
                    }
                    let [then, taken, ..] = parts;
                    let c = self.condition(then, "cond", COALESCE)?;
                    let t = self.required(then, "value", COND)?;
                    let e = self.required(taken, "value", COND)?;
                    return Ok((format!("{c} ? {t} : {e}"), COND));
                }
                let a = self.coalesce_left(node, "a")?;
                let b = self.required(node, "b", BITOR)?;
                Ok((format!("{a} ?? {b}"), COALESCE))
            }
            "guard" => {
                let c = self.condition(node, "cond", COALESCE)?;
                let v = self.required(node, "value", COND)?;
                Ok((format!("{c} ? {v} : undefined"), COND))
            }
            "add" | "multiply" | "and" | "or" if d.inputs.len() >= 2 => {
                let (op, prec) = match d.name.as_str() {
                    "add" => ("+", ADD),
                    "multiply" => ("*", MUL),
                    "and" => ("&&", AND),
                    _ => ("||", OR),
                };
                let mut parts = Vec::new();
                for (i, p) in d.inputs.iter().enumerate() {
                    let same = matches!(self.g.incoming.get(&(node.id.clone(), p.name.clone())), Some(Src::Node { id, .. }) if !self.bound.contains_key(id) && self.g.node(id).data.name == d.name && self.inline(self.g.node(id)));
                    let v = self.arg(node, &p.name)?.ok_or_else(|| format!("{}: its {} isn't wired", d.name, p.name))?;
                    let min = if i == 0 && !same { prec } else { prec + 1 };
                    let mut text = wrap(v, min);
                    // ts.rs refuses a literal after `||` or `&&`: in parentheses, it is read the same.
                    let refused = matches!(d.name.as_str(), "and" | "or") && i > 0 && (text.starts_with(['"', '`', '{', '[']) || text.starts_with(|c: char| c.is_ascii_digit()));
                    if refused {
                        text = format!("({text})");
                    }
                    parts.push(text);
                }
                Ok((parts.join(&format!(" {op} ")), prec))
            }
            "subtract" | "divide" => {
                let (op, prec) = if d.name == "subtract" { ("-", ADD) } else { ("/", MUL) };
                let a = self.required(node, "a", prec)?;
                let b = self.required(node, "b", prec + 1)?;
                Ok((format!("{a} {op} {b}"), prec))
            }
            "compare" if ["<", "<=", ">", ">="].contains(&node.param("op")) => {
                let a = self.required(node, "a", REL)?;
                let b = self.required(node, "b", REL + 1)?;
                Ok((format!("{a} {} {b}", node.param("op")), REL))
            }
            "equals" => {
                let a = self.required(node, "a", EQ)?;
                let b = self.required(node, "b", EQ + 1)?;
                Ok((format!("{a} === {b}"), EQ))
            }
            "not" => {
                if let Some(eq) = self.inline_source(node, "v", "equals") {
                    self.touch(eq);
                    let a = self.required(eq, "a", EQ)?;
                    let b = self.required(eq, "b", EQ + 1)?;
                    return Ok((format!("{a} !== {b}"), EQ));
                }
                let v = self.condition(node, "v", UNARY)?;
                Ok((format!("!{v}"), UNARY))
            }
            // The stores, by their names: `notes[k]`, `notes.list({ limit: 50 })`, `files.put(n, t)`.
            "collection_get" | "file_read" => {
                let store = self.store_of(node);
                self.reads.insert(store.clone());
                let key = self.required(node, if d.name == "file_read" { "name" } else { "key" }, COND)?;
                member(format!("{store}[{key}]"))
            }
            // A file's bytes: `inbox.bytes(name)`.
            "file_read_bytes" => {
                let store = self.store_of(node);
                self.reads.insert(store.clone());
                let name = self.required(node, "name", COND)?;
                member(format!("{store}.bytes({name})"))
            }
            "collection_list" | "file_list" => {
                let store = self.store_of(node);
                self.reads.insert(store.clone());
                let mut opts = Vec::new();
                for input in ["prefix", "after"] {
                    if let Some((t, _)) = self.arg(node, input)? {
                        opts.push(field(input, &t));
                    }
                }
                if node.param("limit") != "100" {
                    opts.push(format!("limit: {}", param_value(node.param("limit"))));
                }
                member(format!("{store}.list({})", if opts.is_empty() { String::new() } else { object(&opts) }))
            }
            "collection_put" | "file_write" | "file_create" => {
                let store = self.store_of(node);
                let (k, v) = match d.name.as_str() {
                    "collection_put" => ("key", "value"),
                    _ => ("name", "text"),
                };
                let key = self.required(node, k, COND)?;
                let value = self.required(node, v, COND)?;
                let method = if d.name == "file_create" { "create" } else { "put" };
                member(format!("{store}.{method}({key}, {value})"))
            }
            "collection_delete" | "file_delete" | "file_handle" => {
                let store = self.store_of(node);
                let key = self.required(node, if d.name == "collection_delete" { "key" } else { "name" }, COND)?;
                member(format!("{store}.{}({key})", if d.name == "file_handle" { "file" } else { "delete" }))
            }
            // A file as a route's answer: `site.serve(name)`, the handle its own.
            "file_serve" => {
                let store = self.store_of(node);
                match self.inline_source(node, "file", "file_handle").filter(|h| h.param("folder") == store) {
                    Some(h) => {
                        self.touch(h);
                        let name = self.required(h, "name", COND)?;
                        member(format!("{store}.serve({name})"))
                    }
                    None => self.generic(node),
                }
            }
            "file_copy" | "file_create_copy" => {
                let store = self.store_of(node);
                let name = self.required(node, "name", COND)?;
                let from = self.required(node, "from", COND)?;
                member(format!("{store}.{}({name}, {from})", if d.name == "file_copy" { "copy" } else { "create_copy" }))
            }
            "file_lines" => {
                let store = self.store_of(node);
                self.reads.insert(store.clone());
                let name = self.required(node, "name", COND)?;
                let mut opts = Vec::new();
                if let Some((t, _)) = self.arg(node, "from")? {
                    opts.push(field("from", &t));
                }
                if node.param("limit") != "100" {
                    opts.push(format!("limit: {}", param_value(node.param("limit"))));
                }
                let opts = if opts.is_empty() { String::new() } else { format!(", {}", object(&opts)) };
                member(format!("{store}.lines({name}{opts})"))
            }
            "file_slice" => {
                let store = self.store_of(node);
                self.reads.insert(store.clone());
                let name = self.required(node, "name", COND)?;
                let offset = self.required(node, "offset", COND)?;
                let length = self.required(node, "length", COND)?;
                member(format!("{store}.slice({name}, {})", object(&[field("offset", &offset), field("length", &length)])))
            }
            "store_write" => Err(format!("state.set({}, …) is a write only a statement makes: state.{} = value", node.param("key"), node.param("key"))),
            "db_query" | "db_one" | "db_exec" => match self.db_values(node)? {
                Some(values) => {
                    let mut args = vec![quote(node.param("sql"))];
                    args.extend(values);
                    if d.name == "db_exec" && !node.param("expect").is_empty() {
                        args.push(format!("{{ expect: {} }}", node.param("expect")));
                    } else if !node.param("expect").is_empty() {
                        return self.generic(node);
                    }
                    member(format!("{}.{}({})", node.param("database"), &d.name["db_".len()..], args.join(", ")))
                }
                None => self.generic(node),
            },
            "db_find" | "db_find_one" | "db_insert" | "db_update" | "db_delete" => match self.document_args(node)? {
                Some(args) => member(format!("{}.{}({})", node.param("database"), &d.name["db_".len()..], args.join(", "))),
                None => self.generic(node),
            },
            "log" => member(format!("console.{}({})", node.param("level"), self.required(node, "message", COND)?)),
            "channel_send" => member(format!("{}.send({})", node.param("channel"), self.required(node, "pack", COND)?)),
            "mail_send" => member(format!("{}.send({})", node.param("mail"), self.required(node, "message", COND)?)),
            "queue_send" => member(format!("{}.send({})", node.param("queue"), self.required(node, "message", COND)?)),
            "channel_subscribe" => match node.param("from") {
                "kept" => member(format!("{}.subscribe({{ from: \"kept\" }})", node.param("channel"))),
                _ => member(format!("{}.subscribe()", node.param("channel"))),
            },
            "http_request" | "http_upload" | "http_stream" => {
                let method = node.param("method").to_ascii_lowercase();
                if !["get", "post", "put", "patch", "delete", "head"].contains(&method.as_str()) {
                    return Err(format!("a request with the method {} has no TypeScript form: get, post, put, patch, delete or head", node.param("method")));
                }
                let host = self.p.host(node.param("host"));
                let path = self.required(node, "path", COND).map_err(|_| format!("a request to {} without a path has no TypeScript form: wire its path", node.param("host")))?;
                let mut opts = Vec::new();
                for input in ["headers", "body", "file"] {
                    if let Some((t, _)) = self.arg(node, input)? {
                        opts.push(field(input, &t));
                    }
                }
                let pipe = node.param("pipe").to_string();
                let piped = d.name == "http_stream" && !pipe.is_empty();
                if d.name == "http_stream" && !piped {
                    opts.push("stream: true".into());
                }
                let opts = if opts.is_empty() { String::new() } else { format!(", {}", object(&opts)) };
                let call = format!("{host}.{method}({path}{opts})");
                if piped {
                    return member(format!("{pipe}.pipe({call})"));
                }
                if function {
                    return member(call);
                }
                self.awaits = true;
                Ok((format!("await {call}"), UNARY))
            }
            "flow_call" | "flow_start" => {
                if function {
                    return Err(format!("a function computes: {} another flow's action is a handler's", if d.name == "flow_call" { "calling" } else { "starting" }));
                }
                let (flow, action) = (node.param("flow").to_string(), node.param("action").to_string());
                let input = match self.arg(node, "input")? {
                    Some(v) => wrap(v, COND),
                    None => String::new(),
                };
                let own = self.p.own.as_deref() == Some(flow.as_str());
                if d.name == "flow_call" {
                    if own {
                        return Err(format!("await {action}(…) would wait for this flow's own action, which a flow can't yet: start it, {action}(x)"));
                    }
                    let local = self.p.action(&flow, &action);
                    self.awaits = true;
                    return Ok((format!("await {local}({input})"), UNARY));
                }
                let local = if own {
                    if !self.p.actions.contains(&action) {
                        return Err(format!("this flow starts its own action \"{action}\", which it doesn't have"));
                    }
                    action
                } else {
                    self.p.action(&flow, &action)
                };
                member(format!("{local}({input})"))
            }
            "await" => {
                if function {
                    return Err("a function computes: waiting for an event is a handler's".into());
                }
                if self.kind == Kind::Route {
                    return Err("a route answers and is done, so it doesn't wait for an event: start an action that waits".into());
                }
                self.p.vocab.insert("event".into());
                let expires = node.param("expires");
                let options = match expires.parse::<u64>() {
                    Ok(604800) => String::new(),
                    Ok(s) if s > 0 => format!(", {{ expires: {} }}", quote(&duration(s))),
                    _ => return Err(format!("an event's expiry of {expires} seconds has no TypeScript form")),
                };
                self.awaits = true;
                Ok((format!("await event({}{options})", quote(node.param("name"))), UNARY))
            }
            "approval" => {
                if function {
                    return Err("a function computes: waiting for people to decide is a handler's".into());
                }
                if self.kind == Kind::Route {
                    return Err("a route answers and is done, so it doesn't wait for an approval: start an action that waits".into());
                }
                self.p.vocab.insert("approval".into());
                let expires = node.param("expires");
                let mut options = vec![];
                match expires.parse::<u64>() {
                    Ok(604800) => {}
                    Ok(s) if s > 0 => options.push(format!("expires: {}", quote(&duration(s)))),
                    _ => return Err(format!("an approval's expiry of {expires} seconds has no TypeScript form")),
                }
                match node.param("people") {
                    "1" => {}
                    n if n.parse::<u32>().is_ok_and(|n| (2..=5).contains(&n)) => options.push(format!("people: {n}")),
                    n => return Err(format!("an approval by {n} people has no TypeScript form: 1 to 5")),
                }
                let options = if options.is_empty() { String::new() } else { format!(", {{ {} }}", options.join(", ")) };
                self.awaits = true;
                Ok((format!("await approval({}{options})", quote(node.param("name"))), UNARY))
            }
            _ => self.generic(node),
        }
    }

    /// Whether `node`'s `input` is wired.
    fn arg_wired(&self, node: &Node, input: &str) -> bool {
        self.g.incoming.contains_key(&(node.id.clone(), input.to_string()))
    }

    /// The left of `??`, in parentheses unless it is a `??` itself or binds tighter than `&&`,
    /// which TypeScript refuses beside it.
    fn coalesce_left(&mut self, node: &Node, input: &str) -> Result<String, String> {
        let (text, prec) = self.arg(node, input)?.ok_or_else(|| format!("{}: its {input} isn't wired", node.data.name))?;
        Ok(if prec == COALESCE || prec >= BITOR { text } else { format!("({text})") })
    }

    /// Whether every input `node` names is wired.
    fn all_wired(&self, node: &Node, inputs: &[&str]) -> bool {
        inputs.iter().all(|i| self.arg_wired(node, i))
    }

    /// Whether what feeds `node`'s `input` reads back as a value the lowering knows is a number
    /// (lower.rs `leaf_form`): a node's `Number` output, by its name or written in place.
    fn known_number(&self, node: &Node, input: &str) -> bool {
        self.known(node, input, WType::Number)
    }

    /// Whether what feeds `node`'s `input` is a node's output of type `ty`, by its name or
    /// written in place.
    /// What feeds `node`'s `input`, as a condition reads it: an `is_truthy` written in place is
    /// the value it reads (`!x` for `!is_truthy(x)`), as ts.rs reads a condition that isn't a
    /// Boolean; one around a Boolean, which the reading wouldn't add, is written.
    fn condition(&mut self, node: &Node, input: &str, prec: u8) -> Result<String, String> {
        if let Some(t) = self.inline_source(node, input, "is_truthy").filter(|t| self.arg_wired(t, "value") && !self.boolean(t, "value")) {
            self.touch(t);
            return self.required(t, "value", prec);
        }
        self.required(node, input, prec)
    }

    /// Whether what feeds `node`'s `input` is a Boolean as the lowering knows one: a node's output
    /// of that type, or a choice among Booleans (`a ?? b`, `c ? a : b`).
    fn boolean(&self, node: &Node, input: &str) -> bool {
        if self.known(node, input, WType::Boolean) {
            return true;
        }
        let Some(Src::Node { id, .. }) = self.g.incoming.get(&(node.id.clone(), input.to_string())) else { return false };
        let src = self.g.node(id);
        let chosen: &[&str] = match src.data.name.as_str() {
            "coalesce" => &["a", "b"],
            "guard" => &["value"],
            "select" => &["if_true", "if_false"],
            _ => return false,
        };
        chosen.iter().all(|p| self.boolean(src, p))
    }

    fn known(&self, node: &Node, input: &str, ty: WType) -> bool {
        self.known_by(node, input, |t| *t == ty)
    }

    /// Whether what feeds `node`'s `input` is a node's output whose type `is` takes, by its name
    /// or written in place.
    fn known_by(&self, node: &Node, input: &str, is: impl Fn(&WType) -> bool) -> bool {
        let Some(Src::Node { id, output }) = self.g.incoming.get(&(node.id.clone(), input.to_string())) else { return false };
        if self.captured.contains_key(&(id.clone(), output.clone())) || self.request.as_ref().is_some_and(|(r, _)| r == id) || self.is_tick(id) {
            return false;
        }
        let src = self.g.node(id);
        let port = src.data.outputs.iter().find(|p| p.name == *output).or_else(|| if src.data.outputs.len() == 1 { src.data.outputs.first() } else { None });
        self.pass_through(src).is_none() && port.is_some_and(|p| is(&p.ty))
    }

    /// A number method's receiver: a number literal in parentheses, as `5.toFixed()` doesn't
    /// parse.
    fn receiver(&mut self, node: &Node, input: &str) -> Result<String, String> {
        let x = self.required(node, input, MEMBER)?;
        Ok(if x.starts_with(|c: char| c.is_ascii_digit() || c == '.') { format!("({x})") } else { x })
    }

    /// A number leaf in its JavaScript form (ts.rs reads each back to its leaf), or by its name
    /// where it has none: a family value with no form, an input left out.
    fn number_form(&mut self, node: &Node) -> Result<(String, u8), String> {
        let d = &node.data;
        let member = |s: String| Ok((s, MEMBER));
        match d.name.as_str() {
            "math" if self.arg_wired(node, "x") && family_values("math", "fn").is_some_and(|v| v.contains(&node.param("fn"))) => {
                let x = self.required(node, "x", COND)?;
                member(format!("Math.{}({x})", node.param("fn")))
            }
            "max" | "min" if d.inputs.iter().all(|p| self.arg_wired(node, &p.name)) => {
                let mut args = Vec::new();
                for p in &d.inputs {
                    args.push(self.required(node, &p.name, COND)?);
                }
                member(format!("Math.{}({})", d.name, args.join(", ")))
            }
            "aggregate" if self.arg_wired(node, "xs") => {
                let f = node.param("fn");
                match f {
                    "sum_precise" => member(format!("Math.sumPrecise({})", self.required(node, "xs", COND)?)),
                    "max" | "min" | "hypot" => {
                        // `Math.hypot(a, b)` built its list in place.
                        if f == "hypot"
                            && let Some(list) = self.inline_source(node, "xs", "list").filter(|l| l.data.inputs.len() >= 2 && l.data.inputs.iter().all(|p| self.arg_wired(l, &p.name)))
                        {
                            self.touch(list);
                            let mut items = Vec::new();
                            for p in &list.data.inputs {
                                items.push(self.required(list, &p.name, COND)?);
                            }
                            return member(format!("Math.hypot({})", items.join(", ")));
                        }
                        member(format!("Math.{f}(...{})", self.required(node, "xs", COND)?))
                    }
                    _ => self.generic(node),
                }
            }
            "atan2" if self.all_wired(node, &["y", "x"]) => {
                let y = self.required(node, "y", COND)?;
                let x = self.required(node, "x", COND)?;
                member(format!("Math.atan2({y}, {x})"))
            }
            "power" if self.all_wired(node, &["a", "b"]) => {
                // Right-associative, and its left can't be a unary expression (`(-2) ** x`).
                let a = self.required(node, "a", MEMBER)?;
                let b = self.required(node, "b", POW)?;
                Ok((format!("{a} ** {b}"), POW))
            }
            "remainder" if self.all_wired(node, &["a", "b"]) => {
                let a = self.required(node, "a", MUL)?;
                let b = self.required(node, "b", MUL + 1)?;
                Ok((format!("{a} % {b}"), MUL))
            }
            "bitwise" if self.all_wired(node, &["a", "b"]) => {
                let (op, prec) = match node.param("op") {
                    "and" => ("&", BITAND),
                    "or" => ("|", BITOR),
                    "xor" => ("^", BITXOR),
                    "shl" => ("<<", SHIFT),
                    "shr" => (">>", SHIFT),
                    "ushr" => (">>>", SHIFT),
                    "imul" => {
                        let a = self.required(node, "a", COND)?;
                        let b = self.required(node, "b", COND)?;
                        return member(format!("Math.imul({a}, {b})"));
                    }
                    _ => return self.generic(node),
                };
                let a = self.required(node, "a", prec)?;
                let b = self.required(node, "b", prec + 1)?;
                Ok((format!("{a} {op} {b}"), prec))
            }
            "number_test" if self.arg_wired(node, "x") => {
                let call = match (node.param("test"), node.param("coerce")) {
                    ("finite", "none") => "Number.isFinite",
                    ("integer", "none") => "Number.isInteger",
                    ("safe_integer", "none") => "Number.isSafeInteger",
                    ("nan", "number") => "isNaN",
                    ("finite", "number") => "isFinite",
                    _ => return self.generic(node),
                };
                member(format!("{call}({})", self.required(node, "x", COND)?))
            }
            "to_number" if self.arg_wired(node, "x") => {
                let radix = self.arg_wired(node, "radix");
                let call = match (node.param("grammar"), radix) {
                    ("number", false) => "Number",
                    ("float", false) => "parseFloat",
                    ("int", _) => "parseInt",
                    _ => return self.generic(node),
                };
                let x = self.required(node, "x", COND)?;
                let r = if radix { format!(", {}", self.required(node, "radix", COND)?) } else { String::new() };
                member(format!("{call}({x}{r})"))
            }
            "number_format" if self.arg_wired(node, "x") => {
                let digits = self.arg_wired(node, "digits");
                let method = match node.param("style") {
                    "fixed" => "toFixed",
                    "precision" => "toPrecision",
                    "exponential" => "toExponential",
                    "string" if digits || self.known_number(node, "x") => "toString",
                    _ => return self.generic(node),
                };
                let x = self.receiver(node, "x")?;
                let d = if digits { self.required(node, "digits", COND)? } else { String::new() };
                member(format!("{x}.{method}({d})"))
            }
            _ => self.generic(node),
        }
    }

    /// A text leaf in its JavaScript form (ts.rs reads each back to its leaf): `s.charAt(i)`,
    /// `s.startsWith(t, p)`, `s.trim()`, `s.padStart(n, f)`, `s.replaceAll(a, b)`,
    /// `s.normalize("NFD")`, `encodeURIComponent(s)`, `String.fromCharCode(a, b)`, …; by name
    /// where its inputs aren't wired as JavaScript passes them, in order with none left out
    /// before one given.
    fn text_form(&mut self, node: &Node) -> Result<(String, u8), String> {
        let d = &node.data;
        let wired: Vec<bool> = d.inputs.iter().map(|p| self.arg_wired(node, &p.name)).collect();
        let given = wired.iter().take_while(|w| **w).count();
        let complete = d.inputs.iter().skip(given).all(|p| is_optional(&d.name, &p.name)) && !wired[given..].iter().any(|w| *w);
        if given == 0 || !complete {
            return self.generic(node);
        }
        let ports: Vec<String> = d.inputs.iter().take(given).map(|p| p.name.clone()).collect();
        let call = |f: &str, args: Vec<String>| Ok((format!("{f}({})", args.join(", ")), MEMBER));
        match (d.name.as_str(), node.param("mode"), node.param("set")) {
            ("from_codes", "unit", _) => return call("String.fromCharCode", self.texts(node, &ports)?),
            ("from_codes", "point", _) => return call("String.fromCodePoint", self.texts(node, &ports)?),
            ("uri_encode", _, "component") => return call("encodeURIComponent", self.texts(node, &ports)?),
            ("uri_encode", _, "uri") => return call("encodeURI", self.texts(node, &ports)?),
            ("uri_decode", _, "component") => return call("decodeURIComponent", self.texts(node, &ports)?),
            ("uri_decode", _, "uri") => return call("decodeURI", self.texts(node, &ports)?),
            _ => {}
        }
        let method = match (d.name.as_str(), node.param(family_param(&d.name))) {
            ("text_at", _) => "charAt",
            ("code_at", "unit") => "charCodeAt",
            ("code_at", "point") => "codePointAt",
            ("text_contains", "start") => "startsWith",
            ("text_contains", "end") => "endsWith",
            ("trim", "both") => "trim",
            ("trim", "start") => "trimStart",
            ("trim", "end") => "trimEnd",
            ("pad", "start") => "padStart",
            ("pad", "end") => "padEnd",
            ("repeat", _) => "repeat",
            ("replace", "first") => "replace",
            ("replace", "all") => "replaceAll",
            ("normalize", _) => "normalize",
            ("is_well_formed", _) => "isWellFormed",
            _ => return self.generic(node),
        };
        let s = self.receiver(node, &ports[0])?;
        let mut args = self.texts(node, &ports[1..])?;
        if d.name == "normalize" && node.param("form") != "NFC" {
            args.push(quote(node.param("form")));
        }
        Ok((format!("{s}.{method}({})", args.join(", ")), MEMBER))
    }

    /// The collection, folder or database a store's leaf names.
    fn store_of(&self, node: &Node) -> String {
        let d = &node.data;
        let param = if d.name.starts_with("collection_") {
            "collection"
        } else if d.name.starts_with("db_") {
            "database"
        } else {
            "folder"
        };
        node.param(param).to_string()
    }

    /// A leaf called by its catalog name, imported from "holon": its inputs in order up to the
    /// first one unwired, the others and the params it changes in a last object.
    fn generic(&mut self, node: &Node) -> Result<(String, u8), String> {
        let local = self.p.leaf(&node.data.name);
        let mut args = Vec::new();
        let mut named = Vec::new();
        let mut gap = false;
        for p in node.data.inputs.iter().filter(|p| p.name != TRIGGER) {
            match self.arg(node, &p.name)? {
                Some(v) if !gap => args.push(wrap(v, COND)),
                Some(v) => {
                    if VARIADIC.contains(&node.data.name.as_str()) {
                        return Err(format!("{}: its {} is wired after one that isn't, which TypeScript can't pass", node.data.name, p.name));
                    }
                    named.push(field(&p.name, &wrap(v, COND)));
                }
                None => gap = true,
            }
        }
        // A `map`'s or `fold`'s function is written in order; a family's `fn` (`math`) by name.
        for (k, v) in self.p.changed_params(node) {
            if k != "fn" || !PASSES_EXTRA.contains(&node.data.name.as_str()) {
                named.push(format!("{}: {}", key(&k), param_value(&v)));
            }
        }
        if named.is_empty() {
            // A last record would be read as the call's options: in parentheses, it is passed.
            if let Some(last) = args.last_mut()
                && last.starts_with('{')
                && last != "{}"
                && is_one_group(last)
            {
                *last = format!("({last})");
            }
        } else {
            args.push(object(&named));
        }
        Ok((format!("{local}({})", args.join(", ")), MEMBER))
    }

    /// A higher-order leaf as its method: `xs.map(f)`, `xs.reduce(f, init)` (`xs.reduceRight(f,
    /// init)` over the list reversed in place), `xs.filter(f)`, `xs.some(f)`, `xs.toSorted(f)`, …;
    /// `method` names another (`flatMap`). The function is written in place when the text didn't
    /// name it, the values it uses from around it by their names.
    fn map(&mut self, node: &Node, method: Option<&str>) -> Result<(String, u8), String> {
        let d = &node.data;
        if node.param("fn").is_empty() {
            return Err(format!("{} without a function has no TypeScript form", d.name));
        }
        let fold = d.name == "fold";
        let reversed = if fold { self.inline_source(node, "list", "reverse").filter(|r| self.arg_wired(r, "list")) } else { None };
        let list = match reversed {
            Some(r) => {
                self.touch(r);
                self.receiver(r, "list")?
            }
            None => self.receiver(node, "list")?,
        };
        let init = if fold { Some(self.required(node, "init", COND)?) } else { None };
        let f = self.function_text(node)?;
        let method = method.unwrap_or(match d.name.as_str() {
            "fold" if reversed.is_some() => "reduceRight",
            "fold" => "reduce",
            "any" => "some",
            "all" => "every",
            "find_index" => "findIndex",
            "find_last" => "findLast",
            "find_last_index" => "findLastIndex",
            "sort_by" => "toSorted",
            other => other,
        });
        Ok(match init {
            Some(init) => (format!("{list}.{method}({f}, {init})"), MEMBER),
            None => (format!("{list}.{method}({f})"), MEMBER),
        })
    }

    /// A list leaf in its JavaScript form: `xs.at(i)`, `[...xs.keys()]`, `xs.toSorted()`,
    /// `xs.slice(s, e)`, `[...xs].fill(v)`, `xs.flat(Infinity)`, `[...new Set(xs)]`, …; by name
    /// where its inputs aren't wired as JavaScript passes them, in order with none left out
    /// before one given.
    fn list_form(&mut self, node: &Node) -> Result<(String, u8), String> {
        let d = &node.data;
        let wired: Vec<bool> = d.inputs.iter().map(|p| self.arg_wired(node, &p.name)).collect();
        let given = wired.iter().take_while(|w| **w).count();
        let complete = d.inputs.iter().skip(given).all(|p| is_optional(&d.name, &p.name)) && !wired[given..].iter().any(|w| *w);
        if given == 0 || !complete || (d.name != "flat" && !self.p.changed_params(node).iter().all(|(k, _)| k == "mode")) {
            return self.generic(node);
        }
        let ports: Vec<String> = d.inputs.iter().take(given).map(|p| p.name.clone()).collect();
        let (atom, member) = (|s: String| Ok((s, ATOM)), |s: String| Ok((s, MEMBER)));
        match d.name.as_str() {
            // A record's pairs where it is known to be one, `Object.entries(r)`; else a list's.
            "entries" if self.known(node, &ports[0], WType::Record) => member(format!("Object.entries({})", self.required(node, &ports[0], COND)?)),
            "indices" | "entries" => {
                let xs = self.receiver(node, &ports[0])?;
                atom(format!("[...{xs}.{}()]", if d.name == "indices" { "keys" } else { "entries" }))
            }
            "unique" => atom(format!("[...new Set({})]", self.required(node, &ports[0], COND)?)),
            "is_list" => member(format!("Array.isArray({})", self.required(node, &ports[0], COND)?)),
            "flat" => {
                let depth = match node.param("depth") {
                    "1" => String::new(),
                    "all" => "Infinity".into(),
                    // A number as written, which reads back as written.
                    d if d.strip_prefix('-').unwrap_or(d).starts_with(|c: char| c.is_ascii_digit()) && d.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')) && d.parse::<f64>().is_ok_and(f64::is_finite) => d.to_string(),
                    _ => return self.generic(node),
                };
                // `xs.flatMap(f)`: a map spread one level, written in place.
                if depth.is_empty()
                    && let Some(m) = self.inline_source(node, "list", "map").filter(|m| !m.param("fn").is_empty())
                {
                    self.touch(m);
                    return self.map(m, Some("flatMap"));
                }
                let xs = self.receiver(node, &ports[0])?;
                member(format!("{xs}.flat({depth})"))
            }
            // A change in place, of a copy: `[...xs]` is the list itself, read back.
            "fill" | "copy_within" => {
                let xs = self.required(node, &ports[0], COND)?;
                let args = self.texts(node, &ports[1..])?;
                member(format!("[...{xs}].{}({})", if d.name == "fill" { "fill" } else { "copyWithin" }, args.join(", ")))
            }
            "to_spliced" => {
                let xs = self.receiver(node, &ports[0])?;
                let mut args = self.texts(node, &ports[1..given.min(3)])?;
                if given == 4 {
                    // The items as written, `a, b`, from the list they make, else spread.
                    let items = self.required(node, "items", COND)?;
                    match items.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
                        Some(inner) if !inner.trim().is_empty() && is_one_group(&items) => args.push(inner.to_string()),
                        _ => args.push(format!("...{items}")),
                    }
                }
                member(format!("{xs}.toSpliced({})", args.join(", ")))
            }
            other => {
                let method = match other {
                    "at" => "at",
                    "includes" => "includes",
                    "last_index_of" => "lastIndexOf",
                    "reverse" => "toReversed",
                    "sort" => "toSorted",
                    "with_item" => "with",
                    _ if node.param("mode") == "substring" => "substring",
                    _ => "slice",
                };
                let xs = self.receiver(node, &ports[0])?;
                let args = self.texts(node, &ports[1..])?;
                member(format!("{xs}.{method}({})", args.join(", ")))
            }
        }
    }

    /// Whether what feeds `node`'s `input` is written as a text in quotes or a template, as a
    /// literal, a `concat` or an `as_text` in place can be: a text's `concat` reads such an
    /// argument as itself, where it reads anything else as `String(x)` writes it.
    fn text_written(&self, node: &Node, input: &str) -> bool {
        self.inline_source(node, input, "const").is_some_and(|c| c.param("type") == "Text") || self.inline_source(node, input, "concat").is_some() || self.inline_source(node, input, "as_text").is_some()
    }

    /// What feeds each of `node`'s `inputs`, as arguments.
    fn texts(&mut self, node: &Node, inputs: &[String]) -> Result<Vec<String>, String> {
        inputs.iter().map(|i| self.required(node, i, COND)).collect()
    }

    /// A `concat` of texts and values, each written in place, as a template: `` `${a}-${b}` ``.
    /// One of values only, or with two texts side by side, is a call, as a template would read
    /// otherwise.
    fn concat(&mut self, node: &Node) -> Result<(String, u8), String> {
        let ins: Vec<String> = node.data.inputs.iter().map(|p| p.name.clone()).collect();
        // `s.toWellFormed()`: the text and "", which is the text for a text and nothing else.
        if let [s, e] = ins.as_slice()
            && self.arg_wired(node, s)
            && let Some(c) = self.inline_source(node, e, "const").filter(|c| c.param("type") == "Text" && c.param("value").is_empty())
        {
            self.touch(c);
            return Ok((format!("{}.toWellFormed()", self.receiver(node, s)?), MEMBER));
        }
        // `s.concat(a, …)` on a text known to be one, each argument as `String(x)` writes it or a
        // text in quotes: what the lowering reads back as these nodes.
        if ins.len() >= 2
            && self.known(node, &ins[0], WType::Text)
            && !self.text_written(node, &ins[0])
            && ins[1..].iter().all(|i| self.inline_source(node, i, "const").is_some_and(|c| c.param("type") == "Text") || self.inline_source(node, i, "as_text").is_some_and(|a| !self.text_written(a, "value")))
        {
            let s = self.receiver(node, &ins[0])?;
            let mut args = Vec::new();
            for i in &ins[1..] {
                if let Some(a) = self.inline_source(node, i, "as_text") {
                    self.touch(a);
                    args.push(self.required(a, "value", COND)?);
                } else {
                    args.push(self.required(node, i, COND)?);
                }
            }
            return Ok((format!("{s}.concat({})", args.join(", ")), MEMBER));
        }
        let mut pieces = Vec::new();
        let mut touched = Vec::new();
        let mut values = 0;
        let mut last_text = false;
        for p in &node.data.inputs {
            if let Some(c) = self.inline_source(node, &p.name, "const")
                && c.param("type") == "Text"
                && !c.param("value").is_empty()
                && !last_text
            {
                touched.push(c);
                pieces.push(Err(template_text(c.param("value"))));
                last_text = true;
            } else if let Some(a) = self.inline_source(node, &p.name, "as_text") {
                touched.push(a);
                pieces.push(Ok(a));
                values += 1;
                last_text = false;
            } else {
                return self.generic(node);
            }
        }
        if values == 0 || pieces.len() < 2 {
            return self.generic(node);
        }
        for t in touched {
            self.touch(t);
        }
        let mut out = String::from("`");
        for piece in pieces {
            match piece {
                Err(text) => out.push_str(&text),
                Ok(a) => out.push_str(&format!("${{{}}}", self.required(a, "value", 0)?)),
            }
        }
        out.push('`');
        Ok((out, ATOM))
    }

    /// A higher-order leaf's function as its method takes it: written in place when the text
    /// didn't name it, the values it uses from around it by their names; else by its name.
    fn function_text(&mut self, node: &Node) -> Result<String, String> {
        let d = &node.data;
        let (own, base) = (function_takes(&d.name), leaf_inputs(&d.name));
        let hash = node.param("fn").to_string();
        if hash.is_empty() {
            return Err(format!("{} without a function has no TypeScript form", d.name));
        }
        let (_, unit) = resolve(self.p.store, &hash)?;
        let ports: Vec<String> = unit.inputs.iter().skip(own).map(|p| p.name.clone()).collect();
        let mut captures = Vec::new();
        for (port, input) in ports.iter().zip(d.inputs.iter().skip(base)) {
            let text = self.required(node, &input.name, ATOM)?;
            captures.push((port.clone(), text));
        }
        match self.p.lambda_text(&hash, &captures, own)? {
            Some(f) => Ok(f),
            None if captures.is_empty() => self.p.function_for(&hash, "f"),
            None => Err(format!("{}'s function uses values from around it and is more than one expression, which TypeScript can't write in place: give the values to it some other way", d.name)),
        }
    }

    /// `JSON.stringify(x)`, with the replacer and the indent where they are set: the function,
    /// the keys as a list, or `null`; the indent as a number of spaces, or a text.
    fn json_stringify(&mut self, node: &Node) -> Result<(String, u8), String> {
        let (keys, f, indent) = (node.param("keys"), node.param("fn"), node.param("indent"));
        if !keys.is_empty() && !f.is_empty() {
            return Err("json_stringify with both keys and a function has no TypeScript form: JavaScript's replacer is one or the other".into());
        }
        // The keys as ts.rs writes them, a list of texts, read back the same.
        let listed = match keys {
            "" => None,
            k => match serde_json::from_str::<Vec<String>>(k) {
                Ok(list) if serde_json::to_string(&list).is_ok_and(|s| s == k) => Some(format!("[{}]", list.iter().map(|k| quote(k)).collect::<Vec<_>>().join(", "))),
                _ => return self.generic(node),
            },
        };
        let mut args = vec![self.required(node, "value", COND)?];
        let replacer = if f.is_empty() { listed } else { Some(self.function_text(node)?) };
        let space = match indent {
            "" => None,
            s if s.len() <= 10 && s.bytes().all(|b| b == b' ') => Some(s.len().to_string()),
            s => Some(quote(s)),
        };
        if replacer.is_some() || space.is_some() {
            args.push(replacer.unwrap_or_else(|| "null".into()));
        }
        args.extend(space);
        Ok((format!("JSON.stringify({})", args.join(", ")), MEMBER))
    }

    /// A date leaf as a method of `new Date(t)`: `new Date(t).getUTCHours()`,
    /// `new Date(t).toISOString()`, `new Date(t).setUTCHours(h, m)`, and `new Date(t).getTime()`
    /// for the time value; by name where JavaScript has no such method or call, a date read at an
    /// offset among them.
    fn date_form(&mut self, node: &Node) -> Result<(String, u8), String> {
        let d = &node.data;
        let given = d.inputs.len() - 1;
        let method = match (d.name.as_str(), node.param("part"), node.param("format")) {
            ("date_part", part, _) => DATE_GETTERS.iter().find(|(_, p)| *p == part).map(|(m, _)| *m),
            ("date_text", _, "iso") => Some("toISOString"),
            ("date_text", _, "utc") => Some("toUTCString"),
            ("date_set", part, _) => DATE_SETTERS.iter().find(|(_, p, takes)| *p == part && given <= *takes).map(|(m, ..)| *m),
            _ => None,
        };
        let utc = d.name == "date_set" || !self.arg_wired(node, "offset");
        let Some(method) = method.filter(|_| utc && self.arg_wired(node, "time") && (d.name != "date_set" || self.every_wired(node))) else { return self.generic(node) };
        let t = self.required(node, "time", COND)?;
        let ports: Vec<String> = if d.name == "date_set" { d.inputs.iter().skip(1).map(|p| p.name.clone()).collect() } else { Vec::new() };
        let args = self.texts(node, &ports)?;
        Ok((format!("new Date({t}).{method}({})", args.join(", ")), MEMBER))
    }


    /// A record built in place as the items of the literal ts.rs reads back as it: a chain of
    /// `with_field`s its fields, an `insert` its `[k]: v`, a `merge` its spreads, each on what it
    /// builds on when that is built in place too (`{ ...a, ...b, k: v }`), else spread.
    fn built(&mut self, node: &Node) -> Result<Vec<String>, String> {
        match node.data.name.as_str() {
            "record" => Ok(Vec::new()),
            "merge" => {
                let mut items = Vec::new();
                for (i, p) in node.data.inputs.iter().enumerate() {
                    // The first, built in place with something in it, is read back as its items.
                    match self.builder_source(node, &p.name, &["with_field", "insert"]) {
                        Some(b) if i == 0 => {
                            self.touch(b);
                            items.extend(self.built(b)?);
                        }
                        _ => items.push(format!("...{}", self.required(node, &p.name, COND)?)),
                    }
                }
                Ok(items)
            }
            "insert" => {
                let mut items = self.base_items(node)?;
                let k = self.required(node, "key", COND)?;
                let v = self.required(node, "value", COND)?;
                items.push(format!("[{k}]: {v}"));
                Ok(items)
            }
            _ => self.fields_of(node),
        }
    }

    /// What a `with_field` or an `insert` builds on, as items: none for `{}`, a `merge`'s or an
    /// `insert`'s own, else the record spread.
    fn base_items(&mut self, node: &Node) -> Result<Vec<String>, String> {
        if let Some(b) = self.builder_source(node, "record", &["record", "merge", "insert"]) {
            self.touch(b);
            return self.built(b);
        }
        Ok(vec![format!("...{}", self.required(node, "record", COND)?)])
    }

    /// The node feeding `input`, written in place, when it is one of `leaves` with every input
    /// wired.
    fn builder_source(&self, node: &Node, input: &str, leaves: &[&str]) -> Option<&'g Node> {
        leaves.iter().find_map(|l| self.inline_source(node, input, l)).filter(|b| self.every_wired(b))
    }

    fn every_wired(&self, node: &Node) -> bool {
        node.data.inputs.iter().all(|p| self.arg_wired(node, &p.name))
    }

    /// A record built in place as a record literal: `{ k: v, … }` from a `record()`, `{ ...base,
    /// k: v, … }` from anything else.
    fn record(&mut self, node: &Node) -> Result<(String, u8), String> {
        Ok((object(&self.built(node)?), ATOM))
    }

    /// A chain of `with_field`s as items: what it builds on, then its fields.
    fn fields_of(&mut self, node: &Node) -> Result<Vec<String>, String> {
        let mut chain: Vec<&Node> = vec![self.g.node(&node.id)];
        let mut keys: Vec<String> = vec![node.param("name").to_string()];
        let base = loop {
            let cur = *chain.last().expect("one at least");
            match self.inline_source(cur, "record", "with_field") {
                // A key given twice is two records: the inner one the base.
                Some(inner) if !keys.iter().any(|k| k == inner.param("name")) => {
                    keys.push(inner.param("name").to_string());
                    chain.push(inner);
                }
                _ => break cur,
            }
        };
        chain.reverse();
        for c in &chain[1..] {
            self.touch(c);
        }
        let mut items = self.base_items(base)?;
        for c in &chain {
            let value = self.required(c, "value", COND)?;
            items.push(field(c.param("name"), &value));
        }
        Ok(items)
    }

    /// A database call's values, one for each `$n` of its SQL, when they are a list written in
    /// place (`[]`, one appended to it, or a `list`) and as many as the SQL takes: as the text
    /// writes them after the SQL. `None`: it is called as the leaf it is.
    fn db_values(&mut self, node: &Node) -> Result<Option<Vec<String>>, String> {
        let write = node.data.name == "db_exec";
        let Ok(tables) = holon_core::sql::tables(node.param("sql"), write) else { return Ok(None) };
        if tables.reads.is_empty() && tables.writes.is_empty() {
            return Ok(None);
        }
        self.listed_values(node, tables.params)
    }

    /// A document call's arguments as the text writes them: the collection, its records with
    /// their holes filled by the values listed in place (`$1` the first, and each once, in the
    /// order the records are written), then `{ sort, limit, expect }`. `None`: it is called as
    /// the leaf it is.
    fn document_args(&mut self, node: &Node) -> Result<Option<Vec<String>>, String> {
        use holon_core::documents::{parse, Tpl};
        let name = node.data.name.as_str();
        let collection = node.param("collection");
        if !holon_core::documents::is_collection_name(collection) {
            return Ok(None);
        }
        let parts: &[&str] = match name {
            "db_insert" => &["document"],
            "db_update" => &["filter", "update"],
            _ => &["filter"],
        };
        let mut templates = Vec::new();
        for part in parts {
            let Ok(t) = parse(node.param(part)) else { return Ok(None) };
            templates.push(t);
        }
        let sort = match node.param("sort") {
            "" => None,
            text => match parse(text) {
                Ok(t) => Some(t),
                Err(_) => return Ok(None),
            },
        };
        // The holes in the order written: as the lowering numbers them, else it is a leaf.
        fn holes(t: &Tpl, out: &mut Vec<usize>) {
            match t {
                Tpl::Hole(n) => out.push(*n),
                Tpl::List(items) => items.iter().for_each(|i| holes(i, out)),
                Tpl::Object(fields) => fields.iter().for_each(|(_, v)| holes(v, out)),
                _ => {}
            }
        }
        let mut order = Vec::new();
        templates.iter().chain(&sort).for_each(|t| holes(t, &mut order));
        if order.iter().enumerate().any(|(i, n)| *n != i + 1) || sort.as_ref().is_some_and(|t| !t.holes().is_empty()) {
            return Ok(None);
        }
        // Each template must be one the text writes: no null, numbers as TypeScript reads them.
        fn written(t: &Tpl, values: &[String]) -> Option<String> {
            Some(match t {
                Tpl::Null => return None,
                Tpl::Bool(b) => b.to_string(),
                Tpl::Number(n) => is_number(n).then(|| n.clone())?,
                Tpl::Text(t) => quote(t),
                Tpl::Hole(n) => values.get(n - 1)?.clone(),
                Tpl::List(items) => format!("[{}]", items.iter().map(|i| written(i, values)).collect::<Option<Vec<_>>>()?.join(", ")),
                Tpl::Object(fields) => object(&fields.iter().map(|(k, v)| Some(format!("{}: {}", key(k), written(v, values)?))).collect::<Option<Vec<_>>>()?),
            })
        }
        let placeholders: Vec<String> = (0..order.len()).map(|_| "x".to_string()).collect();
        if templates.iter().chain(&sort).any(|t| written(t, &placeholders).is_none()) {
            return Ok(None);
        }
        let Some(values) = self.listed_values(node, order.len())? else { return Ok(None) };
        // A value the lowering would read as a constant of the record, not a value in it.
        let constant = |v: &str| v.starts_with(['"', '[', '{', '-']) || v.starts_with(|c: char| c.is_ascii_digit()) || v == "true" || v == "false";
        if values.iter().any(|v| constant(v)) {
            return Ok(None);
        }
        let mut opts = Vec::new();
        if let Some(t) = &sort {
            opts.push(format!("sort: {}", written(t, &values).expect("checked")));
        }
        for named in ["limit", "expect"] {
            if !node.param(named).is_empty() {
                opts.push(format!("{named}: {}", node.param(named)));
            }
        }
        let mut args = vec![quote(collection)];
        for (i, t) in templates.iter().enumerate() {
            // A find's filter of everything is left out, unless options follow it.
            if i == 0 && matches!(name, "db_find" | "db_find_one") && matches!(t, Tpl::Object(f) if f.is_empty()) && opts.is_empty() {
                continue;
            }
            args.push(written(t, &values).expect("checked"));
        }
        if !opts.is_empty() {
            args.push(object(&opts));
        }
        Ok(Some(args))
    }

    /// The values a call lists in place for its `count` holes: `[]`, one appended to it, or a
    /// `list` of exactly `count`. `None` when they aren't written so.
    fn listed_values(&mut self, node: &Node, count: usize) -> Result<Option<Vec<String>>, String> {
        let empty = |c: &Node| c.param("type") == "Json" && c.param("value") == "[]";
        let none = self.inline_source(node, "params", "const").filter(|c| empty(c));
        let one = self.inline_source(node, "params", "append").filter(|a| self.inline_source(a, "list", "const").is_some_and(empty));
        let several = self.inline_source(node, "params", "list").filter(|l| l.data.inputs.len() >= 2);
        let given = match (none, one, several) {
            (Some(_), _, _) => 0,
            (_, Some(_), _) => 1,
            (_, _, Some(l)) => l.data.inputs.len(),
            _ => return Ok(None),
        };
        if given != count {
            return Ok(None);
        }
        let mut values = Vec::new();
        if let Some(c) = none {
            self.touch(c);
        } else if let Some(a) = one {
            let c = self.inline_source(a, "list", "const").expect("checked");
            self.touch(a);
            self.touch(c);
            values.push(self.required(a, "item", COND)?);
        } else if let Some(l) = several {
            self.touch(l);
            for p in &l.data.inputs {
                values.push(self.required(l, &p.name, COND)?);
            }
        }
        Ok(Some(values))
    }
}

impl<'s> Printer<'s> {
    /// A function the text wrote in place (one it never named: `fn_1`, `fn_2`…, or none) whose
    /// body is one expression: `(x) => …`, the values it uses from around it read by the names
    /// `captures` gives them. `None` for a named function, or one that needs statements.
    fn lambda_text(&mut self, hash: &str, captures: &[(String, String)], own: usize) -> Result<Option<String>, String> {
        let anonymous = match self.names.get(hash) {
            None => true,
            Some(n) => n.strip_prefix("fn_").is_some_and(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit())),
        };
        if !anonymous || self.def_names.contains_key(hash) {
            return Ok(None);
        }
        let (_, unit) = resolve(self.store, hash)?;
        let unit = unit.clone();
        if is_js_node(self.store, &unit) || unit.outputs.len() != 1 || unit.inputs.len() != own + captures.len() {
            return Ok(None);
        }
        let Ok(opened) = self.open(hash, "fn", &unit) else { return Ok(None) };
        // A parameter the function doesn't use is held by a node nothing reads (lower.rs
        // `UNUSED`, or a `list` holding two), which the body leaves out.
        let own_param = |src: &Src| matches!(src, Src::In(p) if unit.inputs.iter().take(own).any(|q| q.name == *p));
        let held: HashSet<&str> = opened
            .children
            .iter()
            .filter(|c| {
                let inside = |src: &Src| match src {
                    Src::In(_) => true,
                    Src::Node { id, .. } => opened.children.iter().any(|k| k.id == *id),
                };
                let fed: Vec<&Src> = opened.wires.iter().filter(|(src, to, _)| *to == c.id && inside(src)).map(|(src, _, _)| src).collect();
                (c.data.name == UNUSED || c.data.name == "list" && fed.len() >= 2)
                    && c.data.body == PlacedBody::Primitive
                    && fed.len() == c.data.inputs.len()
                    && fed.iter().all(|src| own_param(src))
                    && !opened.wires.iter().any(|(src, _, _)| matches!(src, Src::Node { id, .. } if *id == c.id))
                    && !opened.outs.iter().any(|(id, _, _)| *id == c.id)
            })
            .map(|c| c.id.as_str())
            .collect();
        let children = opened.children.iter().filter(|c| !held.contains(c.id.as_str())).collect();
        let wires = opened.wires.iter().filter(|(_, to, _)| !held.contains(to.as_str())).cloned().collect();
        let group = Group::new(children, wires, opened.outs.clone());
        if unwritable(&group.nodes, &group.incoming, None).is_some() {
            return Ok(None);
        }
        let mut params = Vec::new();
        for p in unit.inputs.iter().take(own) {
            let ts = param_name(&p.name);
            if !is_identifier(&ts) || JS_RESERVED.contains(&ts.as_str()) || port_of(&ts) != p.name {
                return Ok(None);
            }
            params.push((p.name.clone(), ts));
        }
        let mut block = Block::new(self, &group, Kind::Lambda, None);
        for (port, ts) in &params {
            block.inputs.insert(port.clone(), ts.clone());
            block.claim(ts);
        }
        for (port, text) in captures {
            block.inputs.insert(port.clone(), text.clone());
        }
        let Ok(body) = block.body() else { return Ok(None) };
        let [only] = body.as_slice() else { return Ok(None) };
        let Some(expr) = only.text.strip_prefix("return ") else { return Ok(None) };
        if !only.kids.is_empty() || expr.starts_with("{ out:") {
            return Ok(None);
        }
        let expr = if expr.starts_with('{') { format!("({expr})") } else { expr.to_string() };
        let params: Vec<String> = params.into_iter().map(|(_, ts)| ts).collect();
        Ok(Some(format!("({}) => {expr}", params.join(", "))))
    }
}

/// An object literal of `items`: `{ a, b: 1 }`, or `{}`.
fn object(items: &[String]) -> String {
    if items.is_empty() { "{}".into() } else { format!("{{ {} }}", items.join(", ")) }
}

/// A route's request's parts, each an output of its node: a field of the request record of one of
/// these names is read through `req.request`.
const REQUEST_PARTS: &[&str] = &["request", "params", "query", "headers", "body", "json"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::FlowBuilder;
    use crate::flow::flow_unit;
    use crate::lang::compile;
    use crate::{samples, to_hex};

    fn hash_of(doc: &FlowDocument) -> String {
        let mut store = MapResolver::new();
        restore(doc, &mut store, &Names::new());
        let port = doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or("");
        to_hex(&flow_unit(&doc.nodes, &doc.edges, &mut store, port).unwrap_or_else(|e| panic!("{e}")))
    }

    fn printed(name: Option<&str>, doc: &FlowDocument, bindings: &BTreeMap<String, String>) -> String {
        print(name, doc, &mut MapResolver::new(), &Names::new(), bindings).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Why `doc` has no TypeScript form.
    fn refused(doc: &FlowDocument) -> String {
        print(Some("f"), doc, &mut MapResolver::new(), &Names::new(), &BTreeMap::new()).expect_err("printed")
    }

    /// The round trip: printed, compiled again, the same unit; printed again, the same text.
    fn round_trips(name: &str, doc: &FlowDocument, bindings: &BTreeMap<String, String>) -> String {
        let text = printed(Some(name), doc, bindings);
        let back = compile(&text).unwrap_or_else(|e| panic!("{name}: {e}\n{text}"));
        assert_eq!(hash_of(&back.doc), hash_of(doc), "{name}: the printed text lowers to another unit:\n{text}");
        let again = printed(Some(name), &back.doc, &back.names);
        assert_eq!(again, text, "{name}: printing isn't idempotent");
        text
    }

    /// `src` compiled and printed back as it is written.
    fn prints_back(src: &str) -> String {
        let l = compile(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        let text = round_trips(l.name.as_deref().unwrap_or_default(), &l.doc, &l.names);
        assert_eq!(text, src, "fmt keeps it as written");
        text
    }

    /// A number index (`e[0]`, a list's item) prints back as written, and `get` by its name where
    /// the index isn't known to be a number.
    #[test]
    fn a_number_index_prints_as_written() {
        prints_back("import { action, flow } from \"holon\"
import state from \"state\"

flow(\"f\")

function g(entries) {
  return entries.map((e) => [e[0], e[1] * 2])
}

export const t = action((x) => {
  state.x = x.xs[0]
  state.y = g(x.b)
})
");
        prints_back("import { action, flow, get } from \"holon\"
import state from \"state\"

flow(\"f\")

export const t = action((x) => {
  state.x = get(x.xs, x.i)
})
");
    }

    /// A log line prints back as written, in the standard library's form: console.log and the
    /// rest, each level its own method, and its one text (several values written to it are the
    /// template that reads as one).
    #[test]
    fn a_log_line_prints_as_written() {
        prints_back("import { flow, route } from \"holon\"

flow(\"f\", { port: 8300 })

route.get(\"/hi/:name\", (req) => {
  console.log(`hello ${req.params.name}`)
  console.warn(\"careful\")
  console.debug(\"quiet\")
  return `Hello, ${req.params.name}`
})
");
    }

    /// A function returning a record lowers to an output per key, and read whole to the record
    /// of them: it prints as its call, in place too (a map's function using values from around
    /// it), and a call the text named keeps its `const`.
    #[test]
    fn a_function_read_whole_prints_as_its_call() {
        prints_back("import { action, flow } from \"holon\"
import state from \"state\"

flow(\"f\")

function both(x) {
  const a = x + 1
  const b = x * 2
  return { a, b }
}

export const t = action((x) => {
  state.x = x.ys.map((y) => [y, both(x.k + y)])
  state.y = both(x.k)
  const p = both(x.j)
  state.z = { a: p.a, b: p.b }
})
");
    }

    /// A date read in UTC prints as `new Date(t)`'s method; read at an offset, or as its date
    /// alone, it prints by name, its offset after the date.
    #[test]
    fn a_date_at_an_offset_prints_by_name() {
        prints_back("import { action, date_part, date_text, flow } from \"holon\"
import state from \"state\"

flow(\"f\")

export const t = action((x) => {
  state.a = new Date(x.t).getUTCHours()
  state.b = new Date(x.t).toISOString()
  state.c = date_part(x.t, \"+01:00\", { part: \"hours\" })
  state.d = date_text(x.t, x.offset)
  state.e = date_text(x.t, { format: \"date\" })
})
");
    }

    /// `src` compiled and printed back, round-tripping.
    fn reprinted(src: &str) -> String {
        let l = compile(src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        round_trips(l.name.as_deref().unwrap_or_default(), &l.doc, &l.names)
    }

    /// Each sample, compiled from its text and printed, compiles to the same unit and prints the
    /// same text again; the names it gives survive.
    #[test]
    fn every_sample_round_trips_from_its_text() {
        for (name, src) in crate::lang::lower::tests::sample_texts() {
            let first = compile(src).unwrap_or_else(|e| panic!("{name}: {e}"));
            let text = round_trips(name, &first.doc, &first.names);
            if name == "guarded-api" {
                assert!(text.contains("const ok = "), "{text}");
                assert!(text.contains("const receipt = "), "{text}");
            }
        }
    }

    /// Each sample as the builder makes it, with no names: the printer makes them up.
    #[test]
    fn every_sample_round_trips_from_its_builder() {
        for s in samples::samples() {
            let doc = samples::build(s).unwrap();
            round_trips(s.name, &doc, &BTreeMap::new());
        }
    }

    /// A function that places `stripe.verify` keeps the template's unit, and prints it back as
    /// `stripe.verify(…)`, at its own tolerance too, never as a function of its own.
    #[test]
    fn a_function_placing_a_template_prints_it_back() {
        for opts in ["", ", { tolerance: 60 }"] {
            let src = format!(
                "import {{ flow, route, status, stripe }} from \"holon\"\nimport key from \"secret:key\"\nimport {{ now }} from \"holon:clock\"\n\nflow(\"v\", {{ port: 8000 }})\n\nfunction check(req) {{\n  const verified = stripe.verify(req, key, now{opts})\n  return verified.event\n}}\n\nroute.post(\"/v\", (req) => check(req) ?? status(400))\n"
            );
            let text = reprinted(&src);
            assert!(text.contains("stripe.verify(") && text.contains(opts), "{text}");
            assert_eq!(text.matches("function ").count(), 1, "{text}");
        }
    }

    #[test]
    fn the_canonical_form_of_two_samples() {
        let hello = samples::build(samples::sample("hello-api").unwrap()).unwrap();
        assert_eq!(
            printed(Some("hello-api"), &hello, &BTreeMap::new()),
            "import { flow, route } from \"holon\"\n\nflow(\"hello-api\", { port: 8101 })\n\nroute.get(\"/hello/:name\", (req) => `Hello, ${req.params.name}`)\n"
        );
        let heartbeat = samples::build(samples::sample("heartbeat").unwrap()).unwrap();
        assert_eq!(
            printed(Some("heartbeat"), &heartbeat, &BTreeMap::new()),
            "import { every, flow } from \"holon\"\nimport state from \"state\"\nimport times from \"collection:times\"\nimport { now } from \"holon:clock\"\n\nflow(\"heartbeat\")\n\nevery(\"10s\", () => {\n  state.last_beat = now\n  const default_2 = state.beats ?? 0\n  state.beats = default_2 + 1\n  times[`${default_2}`] = now\n  state.first_beat = state.first_beat ?? now\n})\n"
        );
        let likes = compile(samples::sample("likes").unwrap().text).unwrap();
        let text = printed(Some("likes"), &likes.doc, &likes.names);
        assert!(text.contains("import likes from \"collection:likes\"\n"), "{text}");
        assert!(text.contains("  likes[id] = n\n"), "{text}");
        assert!(text.contains("(req) => `${likes[req.params.id] ?? 0} likes`)"), "{text}");
        assert!(text.contains("route.get(\"/likes\", () => likes.list())"), "{text}");
    }

    /// The number leaves called by name: a family's `fn` is a name, where a `map`'s is its
    /// function; an optional radix or digits left out is left unwired. Printed, each is written
    /// in its JavaScript form.
    #[test]
    fn the_number_leaves_called_by_name_round_trip() {
        let src = "import { action, aggregate, flow, math, max, number_format, power, to_number } from \"holon\"\n\nflow(\"f\")\n\nexport const t = action((input) => {\n  const x = to_number(input.text, { grammar: \"int\" })\n  const y = to_number(input.text, 16, { grammar: \"int\" })\n  return { text: number_format(math(power(max(x, y, 3), 2), { fn: \"sqrt\" }), 2, { style: \"fixed\" }), h: aggregate([x, y], { fn: \"hypot\" }) }\n})\n";
        let lowered = compile(src).unwrap_or_else(|e| panic!("{e}"));
        let math = lowered.doc.nodes.iter().find(|n| n.data.name == "math").expect("a math");
        assert_eq!(math.param("fn"), "sqrt");
        let reads: Vec<usize> = lowered.doc.nodes.iter().filter(|n| n.data.name == "to_number").map(|n| lowered.doc.edges.iter().filter(|w| w.target == n.id).count()).collect();
        assert_eq!(reads, [1, 2], "the radix wired only where it is given");
        let text = reprinted(src);
        assert!(text.contains("Math.sqrt(Math.max(x, y, 3) ** 2).toFixed(2)"), "{text}");
        assert!(text.contains("parseInt(input.text, 16)"), "{text}");
        assert!(text.contains("Math.hypot(x, y)"), "{text}");
        assert!(!text.contains("from \"holon\"\nimport") && text.starts_with("import { action, flow } from \"holon\"\n"), "no leaf is imported:\n{text}");
    }

    /// A file served prints back as the folder's `serve`, its handle its own.
    #[test]
    fn a_served_file_prints_back_as_serve() {
        let src = "import { flow, respond, route, status } from \"holon\"\nimport site from \"folder:site\"\n\nflow(\"f\", { port: 8080 })\n\nroute.get(\"/assets/*file\", (req) => site.serve(`assets/${req.params.file}`))\n\nroute.get(\"/gone\", () => status(410, site.serve(\"410.html\")))\n\nroute.get(\"/down\", () =>\n  respond(site.serve(\"a.zip\"), { headers: { \"content-disposition\": \"attachment\" } }),\n)\n\nroute.get(\"/*path\", (req) => site.serve(req.params.path) ?? site.serve(\"index.html\"))\n";
        prints_back(src);
    }

    /// Each record, JSON and date form prints back as written: `fmt` keeps the forms.
    #[test]
    fn the_record_and_date_leaves_print_as_javascript_writes_them() {
        let src = "import { action, flow } from \"holon\"\n\nflow(\"f\")\n\nexport const t = action((input) => {\n  const r = input.r\n  const made = { ...r, k: 1 }\n  return {\n    values: Object.values(r),\n    entries: Object.entries(made),\n    pairs: Object.fromEntries(input.pairs),\n    groups: Object.groupBy(input.xs, (x) => x.kind),\n    merged: { ...r, ...input.s, k: 1 },\n    first: { a: 1, ...r, ...input.s },\n    keyed: { ...made, [input.k]: 2, b: 3 },\n    has: Object.hasOwn(r, input.k),\n    same: Object.is(r, input.s),\n    kind: typeof r,\n    parsed: JSON.parse(input.t, (k, v) => [k, v]),\n    text: JSON.stringify(r, [\"id\", \"name\"], 2),\n    tabbed: JSON.stringify(r, null, \"\\t\"),\n    replaced: JSON.stringify(r, (k, v) => [k, v]),\n    time: new Date(input.t).getTime(),\n    hours: new Date(input.t).getUTCHours(),\n    iso: new Date(input.t).toISOString(),\n    utc: new Date(input.t).toUTCString(),\n    midnight: new Date(input.t).setUTCHours(0, 0, 0, 0),\n    built: Date.UTC(2000, 1, 29),\n    read: Date.parse(input.t),\n  }\n})\n";
        prints_back(src);
    }

    /// Records built by name in shapes a literal reads otherwise print as literals that read back
    /// as the same nodes: a merge of a merge spreads it whole, an empty first record is spread.
    #[test]
    fn the_record_leaves_called_by_name_round_trip() {
        let src = "import { action, flow, insert, merge, type_of } from \"holon\"\n\nflow(\"f\")\n\nexport const t = action((input) => {\n  const r = input.r\n  return { a: merge(merge(r, input.s), input.u), b: merge({}, r), c: insert(insert(r, input.k, 1), input.j, 2), d: type_of(r), e: { ...merge(r, input.s), k: 1 } }\n})\n";
        let text = reprinted(src);
        assert!(text.contains("a: { ...{ ...r, ...input.s }, ...input.u }"), "{text}");
        assert!(text.contains("b: { ...{}, ...r }"), "{text}");
        assert!(text.contains("c: { ...r, [input.k]: 1, [input.j]: 2 }"), "{text}");
        assert!(text.contains("d: type_of(r)"), "{text}");
        assert!(text.contains("e: { ...r, ...input.s, k: 1 }"), "{text}");
    }

    /// Each number leaf in its JavaScript form prints back as written: `fmt` keeps the forms.
    #[test]
    fn the_number_leaves_print_as_javascript_writes_them() {
        let src = "import { action, flow } from \"holon\"\n\nflow(\"f\")\n\nexport const t = action((input) => {\n  const a = parseFloat(input.a)\n  const b = parseInt(input.b, 16)\n  const n = Number(input.n)\n  return {\n    root: Math.sqrt(a),\n    big: Math.max(a, b, n),\n    most: Math.max(...input.xs),\n    h: Math.hypot(a, b),\n    sum: Math.sumPrecise(input.xs),\n    angle: Math.atan2(a, b),\n    p: a ** (b ** 2),\n    q: (a ** b) ** 2,\n    m: a % b,\n    bits: ((((a & b) | (n ^ 3)) << 2) >> 1) >>> 0,\n    neg: a * -1,\n    mul: Math.imul(a, b),\n    finite: Number.isFinite(a),\n    whole: Number.isInteger(a),\n    safe: Number.isSafeInteger(a),\n    nan: isNaN(input.s),\n    fin: isFinite(input.s),\n    fixed: a.toFixed(2),\n    prec: b.toPrecision(3),\n    exp: n.toExponential(),\n    text: a.toString(),\n    hex: b.toString(16),\n    lit: (5).toFixed(1),\n    pi: 3.141592653589793,\n  }\n})\n";
        prints_back(src);
    }

    /// A number written as a method's receiver keeps its parentheses, as `(5).toFixed(1)` does:
    /// printed `5.at(0)`, it read as `5.` and then a name, which doesn't parse.
    #[test]
    fn a_number_receiver_keeps_its_parentheses() {
        let src = "import { action, flow } from \"holon\"\n\nflow(\"f\")\n\nexport const t = action((input) => {\n  return { a: (5).at(0), s: (5).slice(1), i: (5).includes(5) }\n})\n";
        let text = reprinted(src);
        for form in ["(5).at(0)", "(5).slice(1)", "(5).includes(5)"] {
            assert!(text.contains(form), "{form}: {text}");
        }
    }

    /// Each text leaf in its JavaScript form prints back as written: `fmt` keeps the forms,
    /// `.length` where the value is known to be a text or a list, and `+` beside a text.
    #[test]
    fn the_text_leaves_print_as_javascript_writes_them() {
        let src = r#"import { action, flow } from "holon"

flow("f")

export const t = action((input) => {
  const s = input.s.trim()
  const n = parseFloat(input.n)
  return {
    len: s.length,
    pieces: s.split(",", 2).length,
    c: s.charAt(1),
    code: s.charCodeAt(0),
    point: s.codePointAt(2),
    made: String.fromCharCode(97, n),
    emoji: String.fromCodePoint(128512),
    starts: s.startsWith("a"),
    ends: s.endsWith("z", 3),
    has: s.includes("b", 1),
    at: s.indexOf("b", 2),
    last: s.lastIndexOf("b", 4),
    mid: s.slice(1, -1),
    sub: s.substring(1, 3),
    left: s.trimStart(),
    right: s.trimEnd(),
    padded: s.padStart(5, "0"),
    wide: s.padEnd(8),
    again: s.repeat(3),
    once: s.replace("a", "b"),
    all: s.replaceAll("a", "$&$&"),
    nfc: s.normalize(),
    nfd: s.normalize("NFD"),
    ok: s.isWellFormed(),
    whole: s.toWellFormed(),
    joined: s.concat("-", n),
    enc: encodeURIComponent(s),
    uri: encodeURI(s),
    dec: decodeURIComponent(s),
    full: decodeURI(s),
    b64: btoa(s),
    text: atob(s),
    label: "n=" + n + s,
    first: s < input.t,
  }
})
"#;
        prints_back(src);
    }

    /// The text leaves called by name print in their JavaScript forms where they have one, and
    /// by name where the form would read back otherwise: `text_length` of a value not known to be
    /// a text, `slice`'s `length` mode.
    #[test]
    fn the_text_leaves_called_by_name_round_trip() {
        let src = "import { action, code_at, flow, pad, replace, slice, text_contains, text_length, trim, uri_encode } from \"holon\"\n\nflow(\"f\")\n\nexport const t = action((input) => {\n  return { a: trim(input.s, { side: \"start\" }), b: pad(input.s, 4), c: text_contains(input.s, \"x\", { at: \"end\" }), d: replace(input.s, \"a\", \"b\", { count: \"all\" }), e: text_length(input.s), f: slice(input.s, 1, 2, { mode: \"length\" }), g: uri_encode(input.s, { set: \"uri\" }), h: code_at(input.s, 1, { mode: \"point\" }) }\n})\n";
        let text = reprinted(src);
        for form in ["a: input.s.trimStart()", "b: input.s.padStart(4)", "c: input.s.endsWith(\"x\")", "d: input.s.replaceAll(\"a\", \"b\")", "e: text_length(input.s)", "f: slice(input.s, 1, 2)", "g: encodeURI(input.s)", "h: input.s.codePointAt(1)"] {
            assert!(text.contains(form), "{form}: {text}");
        }
    }

    /// Each list leaf in its JavaScript form prints back as written: the methods that take a
    /// function, written in place with the values they use from around it, the value forms of
    /// the methods that change a list in place, and a parameter a function doesn't use.
    #[test]
    fn the_list_leaves_print_as_javascript_writes_them() {
        let src = r#"import { action, flow, range } from "holon"

flow("f")

export const t = action((input) => {
  const xs = input.xs
  const min = input.min
  const who = input.who
  const scores = xs.map((x) => x.score)
  return {
    kept: xs.filter((x) => x.score >= min),
    sorted: xs.toSorted((a, b) => b.score - a.score),
    names: [...new Set(xs.map((x) => x.name))],
    top: xs.find((x) => x.score > 90),
    at: xs.findIndex((x) => x.name === who),
    any: xs.some((x) => x.score > 99),
    all: xs.every((x) => x.score > 0),
    fl: xs.findLast((x) => x.score > 1),
    fli: xs.findLastIndex((x) => x.score > 1),
    last: scores.at(-1),
    has: scores.includes(50),
    li: scores.lastIndexOf(50),
    i: scores.indexOf(50),
    rev: scores.toReversed(),
    ordered: scores.toSorted(),
    part: scores.slice(1, -1),
    rest: scores.slice(1),
    spl: scores.toSpliced(1, 1, 7, 8),
    cut: scores.toSpliced(1),
    put: scores.toSpliced(0, 0, ...input.more),
    w: scores.with(0, 1),
    filled: [...scores].fill(0, 1),
    cw: [...scores].copyWithin(0, 1),
    list: Array.isArray(xs),
    keys: [...scores.keys()],
    ent: [...scores.entries()],
    fm: scores.flatMap((x) => [x, x]),
    deep: input.nested.flat(Infinity),
    two: input.nested.flat(2),
    one: input.nested.flat(),
    right: scores.reduceRight((acc, x) => acc + x, 0),
    zeros: range(3).map((_) => 0),
    pairs: scores.toSorted((_, b) => b - 1),
    kept_order: scores.toSorted((_, __) => 0),
  }
})
"#;
        prints_back(src);
    }

    /// The forms the reader lowers to others print as what they lower to, and read back the
    /// same: an index as `indices`, `reduce` without a start over `head` and `tail`,
    /// `Array.from` and `Array.of` as the list, a change in place of a list of the handler's
    /// own as its value form.
    #[test]
    fn the_list_forms_lowered_print_as_what_they_lower_to() {
        let src = r#"import { action, flow } from "holon"

flow("f")

export const t = action((input) => {
  const xs = input.xs
  const own = [3, 1, 2]
  const sorted = own.sort((a, b) => a - b)
  return {
    ranked: xs.map((x, i) => x + i),
    places: xs.map((x, i) => i),
    same: xs.map((x) => x),
    total: xs.reduce((a, b) => a + b),
    counted: Array.from({ length: 3 }, (_, i) => i * 2),
    zeros: Array.from({ length: 2 }, () => 0),
    upto: Array.from({ length: 4 }, (_, i) => i),
    of: Array.of(1, 2),
    doubled: Array.from(xs, (x) => x * 2),
    once: Array.from(new Set(xs)),
    copy: Array.from(xs),
    values: [...xs.values()],
    joined: xs.join(),
    copied: [...xs].sort(),
    sorted,
    all: xs.slice(),
  }
})
"#;
        let text = reprinted(src);
        for form in [
            "ranked: [...xs.keys()].map((i) => get(xs, i) + i)",
            "places: [...xs.keys()]",
            "same: xs,",
            "total: tail(xs).reduce((a, b) => a + b, head(xs))",
            "counted: range(3).map((i) => i * 2)",
            "zeros: range(2).map((_) => 0)",
            "upto: range(4)",
            "of: [1, 2]",
            "doubled: xs.map((x) => x * 2)",
            "once: [...new Set(xs)]",
            "copy: xs,",
            "values: xs,",
            "joined: xs.join(\",\")",
            "copied: xs.toSorted()",
            "const sorted = own.toSorted((a, b) => a - b)",
            "all: xs,",
        ] {
            assert!(text.contains(form), "{form}:\n{text}");
        }
    }

    /// How long the log keeps runs, and the other limits, in `flow(…)`: in the order the limits
    /// are listed, whole seconds as a duration.
    #[test]
    fn keep_and_limits_print_in_the_flow_s_settings() {
        let clock = |settings: &str| format!("import {{ every, flow }} from \"holon\"\nimport state from \"state\"\n\nflow(\"f\"{settings})\n\nevery(\"1s\", () => {{\n  state.n = 1\n}})\n");
        for keep in [", { keep: { runs: 5, days: 1 } }", ", { keep: { runs: 500 } }", ", { keep: { days: 30 } }", ""] {
            prints_back(&clock(keep));
        }
        let mut doc = compile(&clock(", { keep: { days: 2 } }")).unwrap().doc;
        let requirements = doc.server.as_mut().unwrap().requirements.as_mut().unwrap();
        requirements.extra["limits"]["max_requests"] = 80.into();
        requirements.extra["limits"]["run_budget_ms"] = 60000.into();
        requirements.extra["limits"]["claim_wait_ms"] = 1500.into();
        requirements.extra["limits"]["max_await_seconds"] = 7200.into();
        let text = printed(Some("f"), &doc, &BTreeMap::new());
        assert!(text.contains("  keep: { days: 2 },\n"), "{text}");
        for limit in ["max_requests: 80", "run_budget_ms: \"1m\"", "claim_wait_ms: 1500", "max_await_seconds: \"2h\""] {
            assert!(text.contains(limit), "{limit}: {text}");
        }
        let back = compile(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(back.doc.server.and_then(|s| s.requirements).map(|r| r.extra["limits"].clone()), doc.server.and_then(|s| s.requirements).map(|r| r.extra["limits"].clone()));
    }

    /// A js node (docs/L4s/js-node-L4.md): its body prints back verbatim, a `}` in a string or a
    /// template notwithstanding, its budgets only where they differ from the defaults; the printed
    /// text lowers to the same units; and the node runs, through the engine, as the body says.
    #[test]
    fn a_js_node_prints_back_verbatim_lowers_to_the_same_units_and_runs() {
        let src = "import { flow, js, route } from \"holon\"\n\nflow(\"f\", { port: 8201 })\n\nconst score = js(\n  function score(estimates, tags) {\n  // a } in a comment, \"}\" in a string, `}` in a template: none ends the body\n  const total = estimates.reduce((a, b) => a + b, 0);\n  const label = `${tags.length}:${total > 10 ? \"heavy\" : \"light\"}`;\n  return { total, weight: label };\n},\n  { fuel: 50000000, outputs: [\"total\", \"weight\"] },\n)\n\nconst shout = js(function said(s) {\n  return s.toUpperCase() + \"!\";\n})\n\nroute.post(\"/x\", (req) => {\n  const s = score(req.json.estimates, req.json.tags)\n  return { total: s.total, weight: s.weight, said: shout(req.json.title) }\n})\n";
        let lowered = compile(src).unwrap_or_else(|e| panic!("{e}"));
        let text = round_trips("f", &lowered.doc, &lowered.names);
        assert!(text.contains("  const total = estimates.reduce((a, b) => a + b, 0);\n"), "the body verbatim: {text}");
        assert!(text.contains("{ fuel: 50000000, outputs: [\"total\", \"weight\"] }"), "{text}");
        assert!(text.contains("const shout = js(function said(s) {"), "one output, named by the function: {text}");
        assert!(!text.contains("memory"), "the default memory isn't written: {text}");
        // The node runs: its unit, from the document restored, through a resolver with the engine.
        let mut store = MapResolver::new();
        restore(&lowered.doc, &mut store, &Names::new());
        let names = lowered.doc.names.clone().unwrap_or_default();
        let hash_named = |want: &str| holon_core::parse_hash(names.iter().find(|(_, n)| n.as_str() == want).map(|(h, _)| h.as_str()).unwrap()).unwrap();
        let engine = holon_module::Engine::new();
        let with = holon_module::WithEngine::new(&store, &engine);
        let n = |x: f64| holon_core::Value::Number(x);
        let score = store.resolve(&hash_named("score")).unwrap().clone();
        assert_eq!(score.outputs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["total", "weight"]);
        let estimates = holon_core::Value::List(vec![n(4.0), n(8.0)]);
        let tags = holon_core::Value::List(vec![holon_core::Value::Text("a".into())]);
        let outs = holon_core::eval(&score, &[Some(estimates), Some(tags)], &with);
        assert_eq!(outs.as_slice(), &[Some(n(12.0)), Some(holon_core::Value::Text("1:heavy".into()))], "{:?}", with.take_failure());
        let shout = store.resolve(&hash_named("shout")).unwrap().clone();
        let said = holon_core::eval(&shout, &[Some(holon_core::Value::Text("hi".into()))], &with);
        assert_eq!(said.as_slice(), &[Some(holon_core::Value::Text("HI!".into()))], "{:?}", with.take_failure());
        assert!(holon_core::module_limits_of(&score, &store)[holon_core::FUEL_LIMIT] == 50_000_000);
    }

    /// A call bound as a statement is made when the run reaches it, so a branch reading it keeps
    /// it a statement; one written in a branch is made only when the branch needs it, and stays
    /// there. Two units, each printed as written.
    #[test]
    fn a_call_stays_where_it_was_written_beside_a_branch() {
        let head = "import { flow, route } from \"holon\"\nimport a from \"host:a.example\"\n\nflow(\"f\", { port: 8080 })\n\n";
        let reached = format!("{head}route.get(\"/a\", async (req) => {{\n  const page = await a.get(\"/x\")\n  return req.json.go ? page.body : undefined\n}})\n");
        let in_branch = format!("{head}route.get(\"/a\", async (req) => (req.json.go ? (await a.get(\"/x\")).body : undefined))\n");
        let mut hashes = Vec::new();
        for src in [&reached, &in_branch] {
            prints_back(src);
            hashes.push(hash_of(&compile(src).unwrap().doc));
        }
        assert_ne!(hashes[0], hashes[1], "made when reached, or when the branch needs it");
    }

    /// A function's call bound on a statement is made when the function is entered, whatever
    /// reads its answer, so the answer leaves its boundary as `called`, and a function placing it
    /// gives that on as its own; one made in a branch doesn't. The printer leaves them out of
    /// what each returns, and the lowering names them again.
    #[test]
    fn a_functions_call_leaves_by_a_port_it_doesnt_return() {
        let head = "import { flow, route } from \"holon\"\nimport api from \"host:api.example\"\n\nflow(\"f\", { port: 8080 })\n\n";
        let route = "route.post(\"/a\", async (req) => {\n  const got = await ask2(\"a\")\n  return req.json.c ? got : \"no\"\n})\n";
        let ask2 = "async function ask2(x) {\n  const got = await ask(x)\n  return got ?? 0\n}\n";
        let reached = format!("{head}async function ask(x) {{\n  const sent = await api.post(\"/in\", {{ body: x }})\n  return x === \"a\" ? sent.status : 0\n}}\n\n{ask2}\n{route}");
        let in_branch = format!("{head}async function ask(x) {{\n  return x === \"a\" ? (await api.post(\"/in\", {{ body: x }})).status : 0\n}}\n\n{ask2}\n{route}");
        let outputs = |src: &str, f: &str| {
            let l = compile(src).unwrap();
            let mut store = MapResolver::new();
            restore(&l.doc, &mut store, &Names::new());
            let hash = l.doc.nodes.iter().find(|n| n.data.name == "ask2").and_then(|n| match &n.data.body {
                PlacedBody::Composite { hash } => Some(hash.clone()),
                _ => None,
            });
            let (_, ask2) = resolve(&store, &hash.expect("ask2 placed")).unwrap();
            let unit = if f == "ask2" { ask2.clone() } else { inner(&store, ask2, "ask") };
            unit.outputs.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
        };
        fn inner(store: &MapResolver, outer: &holon_core::Unit, name: &str) -> holon_core::Unit {
            let UnitBody::Composite { children, .. } = &outer.body else { panic!("a composite") };
            children.iter().filter_map(|h| store.resolve(h)).find(|u| matches!(u.body, UnitBody::Composite { .. }) && u.inputs.iter().any(|p| p.name == "x")).cloned().unwrap_or_else(|| panic!("{name} placed"))
        }
        // A function's request prints in the printer's form (§11): the same unit, not the same text.
        let text = reprinted(&reached);
        assert!(text.contains("return x === \"a\" ? response.status : 0\n}") && text.contains("return ask_result ?? 0\n}"), "returned as written, the call aside:\n{text}");
        assert_eq!(outputs(&reached, "ask"), ["out", "called"], "ask's request leaves");
        assert_eq!(outputs(&reached, "ask2"), ["out", "called"], "and ask2 gives it on");
        assert_eq!(outputs(&in_branch, "ask"), ["out"], "made in a branch, when the branch needs it");
        assert_eq!(outputs(&in_branch, "ask2"), ["out"]);
    }

    /// Another flow's action awaited is called, one not awaited started, the flow's own by its
    /// name; what waits for a value goes in `if (x !== undefined)`.
    #[test]
    fn calls_and_starts_print_as_actions_and_round_trip() {
        prints_back(
            "import { flow, route } from \"holon\"\nimport c from \"collection:c\"\nimport { charge, refund, reset } from \"flow:billing-v2\"\n\nflow(\"shop\", { port: 8180 })\n\nroute.post(\"/orders\", async (req) => {\n  const charged = await charge({ amount: req.json.total })\n  const found = c[req.json.id]\n  if (found !== undefined) await reset()\n  await refund(1)\n  return charged.answer\n})\n",
        );
        prints_back(
            "import { action, event, flow, route } from \"holon\"\n\nflow(\"f\", { port: 8080 })\n\nroute.post(\"/a\", (req) => {\n  const started = pay(req.json)\n  return started.waiting.paid\n})\n\nexport const pay = action(async (order) => {\n  const paid = await event(\"paid\", { expires: \"1h\" })\n  return paid.method\n})\n",
        );
        prints_back(
            "import { flow, route } from \"holon\"\nimport { charge } from \"flow:billing\"\n\nflow(\"f\", { port: 8080 })\n\nroute.post(\"/a\", (req) => {\n  charge(req.json)\n})\n",
        );
    }

    /// An approval prints as it is written, its options only where they aren't the defaults.
    #[test]
    fn approvals_print_back() {
        for options in ["", ", { people: 2 }", ", { expires: \"1d\" }", ", { expires: \"1h\", people: 3 }"] {
            prints_back(&format!(
                "import {{ action, approval, flow, route }} from \"holon\"\n\nflow(\"f\", {{ port: 8080 }})\n\nroute.post(\"/a\", (req) => pay(req.json))\n\nexport const pay = action(async (order) => {{\n  const payout = await approval(\"payout\"{options})\n  return payout.approved\n}})\n"
            ));
        }
    }

    /// The ties the lowering makes aren't written: a route's writes, a function's own trigger;
    /// what waits for a value is (docs/L4s/triggers-L4.md).
    #[test]
    fn ties_print_as_the_text_wrote_them_and_round_trip() {
        let text = reprinted(
            "import { flow, route } from \"holon\"\nimport state from \"state\"\nimport c from \"collection:c\"\n\nflow(\"f\", { port: 8100 })\n\nfunction keep(v) {\n  const put = c.put(\"k\", v)\n  state.last = v\n  return put\n}\n\nroute.post(\"/k\", (req) => {\n  const id = req.params.id\n  const r = keep(id)\n  if (r.put !== undefined) state.seen = id\n  state.hits = (state.hits ?? 0) + 1\n  return id\n})\n",
        );
        assert!(text.contains("function keep(v) {"), "its trigger isn't declared: {text}");
        assert!(text.contains("  if (r.put !== undefined) state.seen = id\n"), "{text}");
        assert!(text.contains("  state.hits = (state.hits ?? 0) + 1\n"), "tied to the request, nothing written: {text}");
        assert!(!text.contains("trigger"), "{text}");
    }

    /// File handlers print back as written, in order, each with its file's name; a handle
    /// written to a folder is a copy.
    #[test]
    fn file_handlers_print_back_as_written() {
        prints_back(
            "import { flow, on, route } from \"holon\"\nimport state from \"state\"\nimport done from \"folder:done\"\nimport drop from \"folder:drop\"\n\nflow(\"sort\", { port: 8080 })\n\non.file.created(\n  drop,\n  (sheet) => {\n    done[sheet.name] = sheet\n    drop[`seen/${sheet.name}`] = \"seen\"\n  },\n  { matching: \"*.csv\" },\n)\n\non.file.deleted(drop, (f) => {\n  state.last = f.name\n})\n\nroute.get(\"/last\", () => state.last)\n",
        );
    }

    /// Mail handlers print back as written, the mail imported as one sent through is.
    #[test]
    fn mail_handlers_print_back_as_written() {
        prints_back(
            "import { flow, on, route } from \"holon\"\nimport state from \"state\"\nimport remit from \"mail:remit\"\n\nflow(\"remit\", { port: 8080 })\n\non.mail(remit, (message) => {\n  state.last = message.subject\n})\n\nroute.get(\"/last\", () => state.last)\n",
        );
        prints_back("import { flow, on } from \"holon\"\nimport state from \"state\"\nimport remit from \"mail:remit\"\n\nflow(\"remit\")\n\non.mail(\n  remit,\n  (m) => {\n    state.from = m.from\n  },\n  { log: \"lazy\" },\n)\n");
    }

    /// Feed handlers print back as written, the feed imported.
    #[test]
    fn feed_handlers_print_back_as_written() {
        prints_back(
            "import { flow, on, route } from \"holon\"\nimport state from \"state\"\nimport prices from \"feed:prices\"\n\nflow(\"ticks\", { port: 8080 })\n\non.feed(prices, (event) => {\n  state.last = event.data\n})\n\nroute.get(\"/last\", () => state.last)\n",
        );
        prints_back("import { flow, on } from \"holon\"\nimport state from \"state\"\nimport prices from \"feed:prices\"\n\nflow(\"ticks\")\n\non.feed(\n  prices,\n  (e) => {\n    state.name = e.event\n  },\n  { log: \"lazy\" },\n)\n");
    }

    /// Queue handlers print back as written, a queue sent to imported as one listened to is.
    #[test]
    fn queue_handlers_print_back_as_written() {
        prints_back(
            "import { flow, on, route } from \"holon\"\nimport state from \"state\"\nimport payouts from \"queue:payouts\"\nimport retries from \"queue:retries\"\n\nflow(\"payouts\", { port: 8080 })\n\non.queue(payouts, (message) => {\n  state.last = message.data\n  retries.send({ data: message.data, group: message.group })\n})\n\nroute.get(\"/last\", () => state.last)\n",
        );
        prints_back("import { flow, on } from \"holon\"\nimport state from \"state\"\nimport payouts from \"queue:payouts\"\n\nflow(\"payouts\")\n\non.queue(\n  payouts,\n  (m) => {\n    state.count = m.count\n  },\n  { log: \"lazy\" },\n)\n");
    }

    /// A websocket prints back as one call, its blocks in order, each an arrow; its levels as
    /// one when they are one, else by block.
    #[test]
    fn websockets_print_back_as_written() {
        prints_back(
            "import { flow, route } from \"holon\"\nimport people from \"collection:people\"\nimport room from \"channel:room\"\n\nflow(\"room\", { port: 8132 })\n\nroute.websocket(\"/room/:name\", {\n  open: (req) => {\n    people[req.conn] = req.params.name\n    return room.subscribe()\n  },\n  message: (m) => {\n    room.send({ from: people[m.conn], text: m.text })\n  },\n  close: (c) => {\n    delete people[c.conn]\n  },\n})\n",
        );
        prints_back("import { flow, route } from \"holon\"\n\nflow(\"echo\", { port: 8132 })\n\nroute.websocket(\"/echo\", { message: (m) => m.json.n + 1 }, { log: \"lazy\" })\n");
        prints_back(
            "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"echo\", { port: 8132 })\n\nroute.websocket(\n  \"/echo\",\n  {\n    message: (m) => m.text,\n    close: (c) => {\n      state.last = c.code\n    },\n  },\n  { log: { message: \"unrecorded\" } },\n)\n",
        );
        // A method is written as an arrow, the blocks kept in the order written; a level by block
        // that is every block's, as one.
        let text = reprinted("import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"echo\", { port: 8132 })\n\nroute.websocket(\"/echo\", {\n  close(c) {\n    state.last = c.code\n  },\n  message(m) {\n    return m.text\n  },\n}, { log: { message: \"lazy\", close: \"lazy\" } })\n");
        assert_eq!(
            text,
            "import { flow, route } from \"holon\"\nimport state from \"state\"\n\nflow(\"echo\", { port: 8132 })\n\nroute.websocket(\n  \"/echo\",\n  {\n    close: (c) => {\n      state.last = c.code\n    },\n    message: (m) => m.text,\n  },\n  { log: \"lazy\" },\n)\n"
        );
    }

    /// Actions print back as written, each with the name of what was sent.
    #[test]
    fn actions_print_back_as_written() {
        prints_back(
            "import { action, every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"ticker\")\n\nevery(\"2s\", () => {\n  state.ticks = (state.ticks ?? 0) + 1\n})\n\nexport const reset = action((input) => {\n  state.ticks = input.to ?? 0\n  return { was: state.ticks ?? 0 }\n})\n\nexport const ping = action((sent) => sent)\n",
        );
    }

    /// A document call's records are written in the call, its values in place.
    #[test]
    fn document_calls_print_their_records_with_their_values_in_place() {
        let text = reprinted(
            "import { flow, route } from \"holon\"\nimport notes from \"database:notes\"\n\nflow(\"notes\", { port: 8160 })\n\nroute.post(\"/n/:who\", (req) => {\n  const mine = notes.find(\"notes\", { owner: req.params.who, \"state.now\": { \"$in\": [\"open\", req.json.also] } }, { sort: { at: -1 }, limit: 20 })\n  const everything = notes.find_one(\"notes\")\n  notes.insert(\"notes\", { owner: req.params.who, text: `${req.json.text}!`, tags: [\"new\"], pinned: false })\n  notes.update(\"notes\", { _id: { \"$oid\": req.json.id } }, { \"$set\": { text: req.json.text } }, { expect: 1 })\n  notes.delete(\"notes\", { owner: req.params.who, done: true })\n  return { mine, everything }\n})\n",
        );
        for written in [
            "{ owner: req.params.who, \"state.now\": { \"$in\": [\"open\", req.json.also] } },",
            "{ sort: { at: -1 }, limit: 20 },",
            "notes.find_one(\"notes\")",
            "text: `${req.json.text}!`,",
            "{ \"$set\": { text: req.json.text } },",
            "{ expect: 1 },",
            "notes.delete(\"notes\", { owner: req.params.who, done: true })",
        ] {
            assert!(text.contains(written), "{written}\n{text}");
        }
    }

    /// Without the author's names (a document the editor opens from a deploy), each value used
    /// once is written where it is used, a template inside a template too.
    #[test]
    fn a_template_inside_a_part_round_trips() {
        let src = "import { flow, route, sha256, slice } from \"holon\"\nimport { now } from \"holon:clock\"\nimport { random } from \"holon:random\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/x/:id\", (req) => {\n  const seed = sha256(`${random}:${now}:${req.params.id}`)\n  const by = req.query.by ?? \"someone\"\n  return `${req.params.id}-${slice(seed, 0, 6)} via ${by}`\n})\n";
        let first = compile(src).unwrap_or_else(|e| panic!("{e}"));
        let text = round_trips("f", &first.doc, &BTreeMap::new());
        assert!(text.contains("`${req.params.id}-${slice(sha256(`${random}:${now}:${req.params.id}`), 0, 6)} via ${req.query.by ?? \"someone\"}`"), "{text}");
    }

    /// A one-expression function the text wrote in place prints back in place, and a value it
    /// uses from around it as the name it is.
    #[test]
    fn functions_written_in_place_print_back_in_place() {
        let text = reprinted(
            "import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nfunction priced(items, rate) {\n  return items.map((p) => p * rate)\n}\n\nroute.post(\"/x\", (req) => {\n  const rate = req.query.rate ?? 2\n  const total = req.json.prices.reduce((acc, p) => acc + p * rate, 0)\n  const r = req.json\n  const values = Object.keys(r).map((k) => r[k])\n  return { total, values, held: priced([1, 2], 3), doubled: [1].map((x) => x * 2) }\n})\n",
        );
        for line in [
            "  return items.map((p) => p * rate)\n",
            // A part of the request a function uses is named by a const, first; read by it after.
            "(req) => {\n  const r = req.json\n",
            "  const total = r.prices.reduce((acc, p) => acc + p * rate, 0)\n",
            "  const values = Object.keys(r).map((k) => r[k])\n",
            "doubled: [1].map((x) => x * 2)",
        ] {
            assert!(text.contains(line), "missing {line:?} in\n{text}");
        }
        assert!(!text.contains("function fn"), "no function written in place printed as one:\n{text}");
    }

    #[test]
    fn the_natural_spellings_print_as_written() {
        let text = reprinted(
            "import { action, event, flow, route, status } from \"holon\"\nimport state from \"state\"\nimport tok from \"secret:tok\"\nimport notes from \"collection:notes\"\nimport h from \"host:h.example\"\n\nflow(\"f\", { port: 8200 })\n\nfunction twice(n) {\n  return n + n\n}\n\nroute.post(\"/x\", async (req) => {\n  const item = notes[req.params.id] ?? { k: [1, \"a\"] }\n  const ok = req.headers.authorization === `Bearer ${tok}` && !item.done\n  const sent = await h.post(\"/p\", { body: JSON.stringify({ ...item, n: twice(item.n) }) })\n  notes[req.params.id] = ok ? item : {}\n  return status(201, ok ? { status: sent.status } : undefined) ?? status(401)\n})\n\nexport const pay = action(async (input) => {\n  const paid = await event(\"paid\", { expires: \"2h\" })\n  state.last = { ...input, paid_via: paid.method }\n})\n",
        );
        for line in [
            "function twice(n) {\n  return n + n\n}",
            "  const item = notes[req.params.id] ?? { k: [1, \"a\"] }\n",
            "  const ok = req.headers.authorization === `Bearer ${tok}` && !item.done\n",
            "  const sent = await h.post(\"/p\", { body: JSON.stringify({ ...item, n: twice(item.n) }) })\n",
            "  const paid = await event(\"paid\", { expires: \"2h\" })\n",
            "  notes[req.params.id] = ok ? item : {}\n",
            "  state.last = { ...input, paid_via: paid.method }\n",
            "status(201, ok ? { status: sent.status } : undefined) ?? status(401)",
        ] {
            assert!(text.contains(line), "missing {line:?} in\n{text}");
        }
    }

    /// A canvas grouped by hand: a function grouped by proposal, placed twice; a variadic leaf;
    /// a function of several outputs with an effect; a lookup with a dynamic key.
    #[test]
    fn a_hand_grouped_canvas_round_trips_with_its_proposed_names_and_odd_shapes() {
        let mut b = FlowBuilder::new();
        b.begin_holon("shout");
        let up = b.node("uppercase", &[], None).unwrap();
        let bang = b.node("const", &[("value".into(), "!".into()), ("type".into(), "Text".into())], None).unwrap();
        let cat = b.node("concat", &[], None).unwrap();
        b.wire(&up, "out", &cat, "a").unwrap();
        b.wire(&bang, "out", &cat, "b").unwrap();
        let shout = b.end_holon("shout", &["words".into()], &["said".into()]).unwrap();
        let req = b.node("request", &[("method".into(), "POST".into()), ("path".into(), "/x".into())], None).unwrap();
        let s1 = b.place(&shout, None);
        let s2 = b.place(&shout, None);
        let three = b.node("concat", &[], None).unwrap();
        b.arity(&three, 3).unwrap();
        b.wire(&req, "body", &s1, "words").unwrap();
        b.wire(&req, "body", &s2, "words").unwrap();
        b.wire(&s1, "said", &three, "a").unwrap();
        b.wire(&s2, "said", &three, "b").unwrap();
        b.wire(&req, "body", &three, "c").unwrap();
        let look = b.node("lookup", &[], None).unwrap();
        b.wire(&req, "json", &look, "record").unwrap();
        b.wire(&three, "out", &look, "key").unwrap();
        let respond = b.node("respond", &[], None).unwrap();
        b.wire(&req, "request", &respond, "request").unwrap();
        b.wire(&look, "out", &respond, "body").unwrap();
        let doc = b.finish(crate::builder::Settings { port: "8080".into(), ..Default::default() });
        let text = round_trips("canvas", &doc, &BTreeMap::new());
        assert!(text.contains("function shout(s) {\n  return s.toUpperCase().concat(\"!\")\n}"), "its input as core proposed it: {text}");
        assert!(text.contains("concat(shout(req.body), shout(req.body), req.body)"), "{text}");
        assert!(text.contains("req.json[concat("), "{text}");
    }

    /// What TypeScript can't write is refused, saying what to change.
    #[test]
    fn what_typescript_cant_write_is_refused() {
        // Two routes' requests wired into one node.
        let mut b = FlowBuilder::new();
        let r1 = b.node("request", &[("method".into(), "GET".into()), ("path".into(), "/a".into())], None).unwrap();
        let r2 = b.node("request", &[("method".into(), "GET".into()), ("path".into(), "/b".into())], None).unwrap();
        let c = b.node("concat", &[], None).unwrap();
        b.wire(&r1, "body", &c, "a").unwrap();
        b.wire(&r2, "body", &c, "b").unwrap();
        let why = refused(&b.finish(Default::default()));
        assert!(why.contains("GET /a") && why.contains("GET /b"), "{why}");
        // Nodes under no handler.
        let mut b = FlowBuilder::new();
        b.node("uppercase", &[], None).unwrap();
        assert_eq!(refused(&b.finish(Default::default())), "nothing starts this flow: add a route, a clock, a file, mail, feed or queue handler or an action");
        // A state write whose answer is used, in a route.
        let mut b = FlowBuilder::new();
        let req = b.node("request", &[("method".into(), "POST".into()), ("path".into(), "/k".into())], None).unwrap();
        let w = b.node("store_write", &[("key".into(), "last".into())], None).unwrap();
        let respond = b.node("respond", &[], None).unwrap();
        b.wire(&req, "body", &w, "value").unwrap();
        b.wire(&req, "request", &respond, "request").unwrap();
        b.wire(&w, "out", &respond, "body").unwrap();
        let why = refused(&b.finish(Default::default()));
        assert!(why.contains("state.last = value"), "{why}");
    }

    #[test]
    fn each_node_knows_its_line() {
        let src = "import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/x\", (req) => {\n  const n = req.params.id\n  return `${n} likes`\n})\n";
        let first = compile(src).unwrap();
        let (text, lines) = print_lines(Some("f"), &first.doc, &mut MapResolver::new(), &Names::new(), &first.names).unwrap();
        assert_eq!(text, src);
        let line_of = |leaf: &str| first.doc.nodes.iter().filter(|n| n.data.name == leaf).map(|n| lines[&n.id]).collect::<Vec<_>>();
        assert_eq!(line_of("request"), [5], "the request is the route's call");
        assert_eq!(line_of("field"), [6]);
        assert_eq!(line_of("as_text"), [7], "an inline node is its consumer's statement");
        assert_eq!(line_of("respond"), [7]);
        assert_eq!(line_of("concat"), [7]);
        // A route that only answers: every node on its line.
        let src = "import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/x\", (req) => `${req.params.id} likes`)\n";
        let first = compile(src).unwrap();
        let (_, lines) = print_lines(Some("f"), &first.doc, &mut MapResolver::new(), &Names::new(), &first.names).unwrap();
        assert!(first.doc.nodes.iter().all(|n| lines[&n.id] == 5), "{lines:?}");
    }

    #[test]
    fn literals_quote_and_duration_as_written() {
        assert_eq!(quote("a \"b\" ${c}\n"), "\"a \\\"b\\\" ${c}\\n\"");
        assert_eq!(template_text("a `b` ${c} $d\n"), "a \\`b\\` \\${c} $d\\n");
        assert_eq!(duration(86400), "1d");
        assert_eq!(duration(300), "5m");
        assert_eq!(duration(45), "45s");
        assert_eq!(limit_value("run_budget_ms", 60000), "\"1m\"");
        assert_eq!(limit_value("run_budget_ms", 1500), "1500");
        assert_eq!(limit_value("max_requests", 60), "60");
        // Keys sorted, as the lowering writes a literal: that is the text that writes back the same.
        assert_eq!(json_literal("{\"done\":false,\"k\":[1,\"x\"]}"), Some("{ done: false, k: [1, \"x\"] }".into()));
        assert_eq!(json_literal("{\"k\": 1}"), None, "a text that doesn't write back the same stays a const");
        assert_eq!(json_literal("{\"k\":[1,\"x\"],\"done\":false}"), None, "unsorted keys don't either");
        assert_eq!(json_literal("{\"k\":null}"), None, "TypeScript has no null here");
        for (text, number) in [("1", true), ("-2.5", true), ("1e3", true), (".5", true), ("007", false), ("1.", true), ("x", false), ("", false)] {
            assert_eq!(is_number(text), number, "{text}");
        }
        assert_eq!(optional_secret("SecretRead(\"receipt_key\")"), Some("receipt_key"));
        assert_eq!(optional_secret("SecretRead(\"a key\")"), None);
        assert_eq!(optional_secret("HttpRequest(\"h\")"), None);
        // A TypeScript name and the lowering's: `json` is known there as `json_`.
        assert_eq!(local_name("json"), "json_");
        assert_eq!(ts_name("json_"), "json");
        assert_eq!(ts_name("total"), "total");
    }

    /// A match bound to a name prints as a `let` given its value in each case of a `switch`; one
    /// a route answers, as a `switch` whose cases return; the guards it lowers to, written by
    /// hand, print as the switch they are.
    #[test]
    fn a_match_prints_back_as_a_switch() {
        let route = "\n\nroute.post(\"/x\", (req) => pick(req.json.op, req.json.v))\n";
        prints_back(&format!(
            "import {{ flow, route }} from \"holon\"\n\nflow(\"f\", {{ port: 8200 }})\n\nfunction pick(op, v) {{\n  let r\n  switch (op) {{\n    case \"inv\":\n      r = 1 / v\n      break\n    case \"neg\":\n    case \"minus\":\n      r = 0 - v\n      break\n    default:\n      r = v\n  }}\n  return r\n}}{route}"
        ));
        let text = reprinted(&format!(
            "import {{ flow, route }} from \"holon\"\n\nflow(\"f\", {{ port: 8200 }})\n\nfunction pick(op, v) {{\n  return (op === \"inv\" ? 1 / v : undefined) ?? (op === \"neg\" ? 0 - v : undefined)\n}}{route}"
        ));
        assert!(text.contains("  switch (op) {\n    case \"inv\":\n      picked = 1 / v\n      break\n    case \"neg\":\n      picked = 0 - v\n  }\n  return { out: picked }\n"), "{text}");
        prints_back(
            "import { flow, route, status } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.post(\"/x\", (req) => {\n  switch (req.json.op) {\n    case \"a\":\n      return 1 ?? status(400)\n    default:\n      return 2 ?? status(400)\n  }\n})\n",
        );
        // A subject the comparisons alone use folds in; one used elsewhere keeps its name.
        prints_back(
            "import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.post(\"/x\", (req) => {\n  const op = req.json.op\n  let n\n  switch (op) {\n    case \"a\":\n      n = 1\n      break\n    case \"b\":\n      n = 2\n  }\n  return { op, n }\n})\n",
        );
    }

    /// What isn't the exact shape prints as the expressions it is, never as a `switch`.
    #[test]
    fn near_misses_of_a_match_print_as_what_they_are() {
        // A guard that falls back to another isn't a ternary either (its else isn't guarded by the
        // first's condition failing): it stays the `??` it is.
        for (body, want) in [
            // Two subjects.
            ("(op === \"a\" ? 1 : undefined) ?? (v === \"b\" ? 2 : undefined)", "(op === \"a\" ? 1 : undefined) ?? (v === \"b\" ? 2 : undefined)"),
            // The pattern on the left.
            ("(\"a\" === op ? v : undefined) ?? (op === \"b\" ? 2 : undefined)", "(\"a\" === op ? v : undefined) ?? (op === \"b\" ? 2 : undefined)"),
            // A ternary, its else not guarded by a pattern.
            ("op === \"a\" ? 1 : v", "op === \"a\" ? 1 : v"),
            // One arm alone.
            ("op === \"a\" ? v : undefined", "op === \"a\" ? v : undefined"),
            // A pattern that isn't a literal.
            ("(op === v ? 1 : undefined) ?? (op === \"b\" ? 2 : undefined)", "(op === v ? 1 : undefined) ?? (op === \"b\" ? 2 : undefined)"),
            // Ternaries chained.
            ("op === \"a\" ? 1 : op === \"b\" ? v : 3", "op === \"a\" ? 1 : op === \"b\" ? v : 3"),
        ] {
            let src = format!("import {{ flow, route }} from \"holon\"\n\nflow(\"f\", {{ port: 8200 }})\n\nfunction pick(op, v) {{\n  return {body}\n}}\n\nroute.post(\"/x\", (req) => pick(req.json.op, req.json.v))\n");
            let text = reprinted(&src);
            assert!(text.contains(&format!("  return {want}\n}}")), "{body}:\n{text}");
        }
    }

    /// The sample named by HOLON_DUMP, printed: `cargo test dump -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump() {
        let name = std::env::var("HOLON_DUMP").unwrap_or_default();
        let s = samples::sample(&name).unwrap();
        let first = compile(s.text).unwrap();
        println!("{}", print(Some(&name), &first.doc, &mut MapResolver::new(), &Names::new(), &first.names).unwrap_or_else(|e| e));
    }
}
