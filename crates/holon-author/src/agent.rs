//! What the agent tools (`holon mcp`, docs/L4s/agent-surface-L4.md) compute without a server: a
//! text compiled into what a deploy sends, a recorded run shown as the program with each
//! statement's value, a run in one line, and the language's reference. All of it reads what core
//! and the language already compute; the server side only moves it.

use std::collections::{BTreeMap, HashMap, HashSet};

use holon_core::{capabilities, consent_name, encode_preimage, genesis_units, MapResolver, Resolver, UnitBody};
use serde_json::{json, Value};

use crate::builder::{closure_of, FlowDocument};
use crate::commands;
use crate::composites::Names;
use crate::describe::describe_item;
use crate::flow::{derive_server, flow_unit, trace_of};
use crate::lang::print::print_lines;
use crate::wire::{WParam, WPort, WType};
use crate::{samples, templates, to_hex};

/// What deploying a text sends: its units, the unit it deploys as, the document (with the
/// `trace` view mode and `explain` read runs by, and the text's binding names), and its settings.
#[derive(Debug)]
pub struct Plan {
    /// The flow's name: what it deploys as unless a slug is given.
    pub name: String,
    /// The unit it deploys as (hex).
    pub unit: String,
    /// Every unit it needs, as preimages, children before the units that use them.
    pub units: Vec<Vec<u8>>,
    pub document: Value,
    /// Its routes, `METHOD /path`; none for a flow without routes.
    pub routes: Vec<String>,
    pub port: Option<u16>,
    pub interval_seconds: Option<u64>,
    pub requirements: Option<Value>,
    pub browsers: Option<String>,
    /// What it may do, as consent names (`NetworkIngress`, `StateWrite("beats")`), sorted.
    pub may: Vec<String>,
}

/// The text compiled into a [`Plan`]; or its mistakes, as `holon check` prints them.
pub fn plan(source: &str) -> Result<Plan, String> {
    let mut store = MapResolver::new();
    let c = commands::compile("text", source, &mut store).map_err(|r| r.err.join("\n"))?;
    let name = c.name.clone().ok_or("the text needs a `flow <name>` line: the name is what it deploys as")?;
    let settings = c.doc.server.clone().unwrap_or_default();
    let server = derive_server(&c.doc.nodes, &c.doc.edges, &mut store, &settings.port);
    if !server.errors.is_empty() {
        return Err(server.errors.join("\n"));
    }
    let hash = flow_unit(&c.doc.nodes, &c.doc.edges, &mut store, &settings.port)?;
    let root = store.resolve(&hash).ok_or("the flow's unit wasn't stored")?.clone();
    let trace = if server.endpoints.is_empty() {
        let all: Vec<_> = c.doc.nodes.iter().collect();
        BTreeMap::from([("*".to_string(), trace_of(&root, &all))])
    } else {
        server.trace.clone()
    };
    let mut may: Vec<String> = capabilities(&root, &store).deployed(commands::scheduled(&c, &server)).iter().map(|c| consent_name(&c)).collect();
    // A module (a js node) adds no capability; its budgets are limits the deploy asks for, and
    // the person approving sees them (the server merges them into the manifest the same way).
    may.extend(holon_core::module_limits_of(&root, &store).iter().map(|(name, n)| format!("Limit({name}={n})")));
    may.sort();
    let units = closure_of(&[hash], &store).iter().filter_map(|h| store.resolve(h).and_then(|u| encode_preimage(u).ok())).collect();
    let mut document = serde_json::to_value(&c.doc).map_err(|e| e.to_string())?;
    document["trace"] = json!(trace);
    if !c.names.is_empty() {
        document["bindings"] = json!(c.names);
    }
    Ok(Plan {
        name,
        unit: to_hex(&hash),
        units,
        document,
        routes: server.endpoints.iter().map(|e| format!("{} {}", e.method, e.path)).collect(),
        port: settings.port.trim().parse().ok(),
        interval_seconds: settings.interval.trim().parse().ok(),
        requirements: settings.requirements.as_ref().and_then(|r| serde_json::to_value(r).ok()),
        browsers: settings.browsers.clone(),
        may,
    })
}

/// A flow's document as the language's text: the text it was saved with (the editor's text pane
/// and the agent tools keep it, comments and all) when that still compiles, else printed from the
/// canvas, with the binding names an agent gave it.
pub fn text_of(name: &str, document: &Value) -> Result<String, String> {
    if let Some(source) = document.get("source").and_then(Value::as_str).filter(|s| !s.trim().is_empty())
        && plan(source).is_ok()
    {
        return Ok(source.to_string());
    }
    let doc: FlowDocument = serde_json::from_value(document.clone()).map_err(|e| format!("the flow's document doesn't read: {e}"))?;
    let bindings: BTreeMap<String, String> = document.get("bindings").and_then(|b| serde_json::from_value(b.clone()).ok()).unwrap_or_default();
    print_lines(Some(name), &doc, &mut MapResolver::new(), &Names::new(), &bindings).map(|(text, _)| text)
}

/// A consent name as a person reads it (`StateWrite("beats")` → write its state "beats").
pub fn describe(item: &str) -> String {
    describe_item(item)
}

// ── values ──────────────────────────────────────────────────────────────────────────────────

/// A logged value (`{"type": "Text", "value": "a"}`, the server's `SValue`) written as the
/// language writes literals, cut to a readable length.
pub fn show(v: &Value) -> String {
    let s = show_full(v);
    if s.chars().count() > 240 {
        format!("{}…", s.chars().take(240).collect::<String>())
    } else {
        s
    }
}

fn show_full(v: &Value) -> String {
    let (Some(ty), value) = (v.get("type").and_then(Value::as_str), v.get("value")) else {
        return if v.is_null() { "nothing".into() } else { v.to_string() };
    };
    let value = value.unwrap_or(&Value::Null);
    match ty {
        "Number" => match value.as_f64() {
            Some(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", n as i64),
            Some(n) => n.to_string(),
            None => value.to_string(),
        },
        "Text" => value.to_string(),
        "Boolean" => value.to_string(),
        "List" => format!("[{}]", value.as_array().map(|a| a.iter().map(show_full).collect::<Vec<_>>().join(", ")).unwrap_or_default()),
        "Record" => {
            let fields: Vec<String> = value
                .as_array()
                .map(|a| a.iter().filter_map(|kv| Some(format!("{}: {}", kv.get(0)?.as_str()?, show_full(kv.get(1)?)))).collect())
                .unwrap_or_default();
            if fields.is_empty() {
                "{}".into()
            } else {
                format!("{{ {} }}", fields.join(", "))
            }
        }
        "Effect" => format!("effect {} ← {}", value.get("sink").and_then(Value::as_str).unwrap_or("?"), value.get("payload").map(show_full).unwrap_or_default()),
        "Sum" => format!("arm {}: {}", value.get("arm").map(Value::to_string).unwrap_or_default(), value.get("payload").map(show_full).unwrap_or_default()),
        _ => v.to_string(),
    }
}

// ── runs ────────────────────────────────────────────────────────────────────────────────────

/// A run in one line: its number, what started it, its answer, how long, and why it failed.
pub fn run_line(entry: &Value) -> String {
    let s = |k: &str| entry.get(k).and_then(Value::as_str);
    let seq = entry.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let what = match s("kind") {
        Some("request") => format!("{} {}", s("method").unwrap_or("?"), s("url").unwrap_or("?")),
        Some(kind) => kind.to_string(),
        None => "?".into(),
    };
    let mut line = format!("run {seq}: {what}");
    if let Some(status) = entry.get("status").and_then(Value::as_u64) {
        line.push_str(&format!(" → {status}"));
    }
    if let Some(ms) = entry.get("duration_ms").and_then(Value::as_f64) {
        line.push_str(&format!(", {ms:.1} ms"));
    }
    // A run that ran again, or waited for a key another run held (docs/L4s/key-claims-L4.md).
    if let Some(n) = entry.get("retries").and_then(Value::as_u64).filter(|n| *n > 0) {
        line.push_str(&format!(", retried ×{n}"));
    }
    let waited: f64 = entry.get("claims").and_then(Value::as_array).into_iter().flatten().filter_map(|c| c.get("waited_ms").and_then(Value::as_f64)).sum();
    if waited >= 1.0 {
        line.push_str(&format!(", waited {waited:.0} ms for a key"));
    }
    if let Some(at) = entry.get("at_ms").and_then(Value::as_u64) {
        line.push_str(&format!(", at {}", utc(at / 1000)));
    }
    if let Some(by) = s("by") {
        line.push_str(&format!(", sent by {by}"));
    }
    if let Some(e) = s("error") {
        line.push_str(&format!(" — failed: {e}"));
    }
    line
}

/// Epoch seconds as a UTC time, `2026-10-24 12:40:53 UTC`.
pub fn utc(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// Each canvas node's recorded outputs in the run's last round, and which nodes ran at all, by
/// node id: the run's step paths read through the document's `trace` (the editor's
/// `nodeOutputs`, replay.ts).
/// What a run recorded, by the node of the text it belongs to.
struct Recorded {
    /// Each node's outputs.
    values: HashMap<String, Vec<Value>>,
    /// The nodes that ran.
    ran: HashSet<String>,
    /// The nodes of this run's part of the flow (its route's, or the whole flow's).
    part: HashSet<String>,
    /// For a node under which a module gave nothing, why: the engine's words, kept beside the
    /// module's own step, one level deeper.
    reasons: HashMap<String, String>,
}

/// The document's trace for this run's part (its route's, or the whole flow's): the node id at
/// each step index, and how deep in a step's path that index sits (a route's steps are `[0, k]`).
fn run_trace(document: &Value, entry: &Value) -> (Vec<String>, usize) {
    let request = entry.get("kind").and_then(Value::as_str) == Some("request");
    let key = if request { entry.get("route").and_then(Value::as_str).unwrap_or("") } else { "*" };
    let map: Vec<String> = document.pointer(&format!("/trace/{}", key.replace('~', "~0").replace('/', "~1"))).and_then(|m| serde_json::from_value(m.clone()).ok()).unwrap_or_default();
    (map, if request { 2 } else { 1 })
}

/// The node a recorded step path (`[0, k]`) belongs to.
fn node_of_path(map: &[String], depth: usize, path: &[u64]) -> Option<String> {
    if path.len() < depth || (depth == 2 && path[0] != 0) {
        return None;
    }
    map.get(path[depth - 1] as usize).cloned()
}

/// The claims a run took (`docs/L4s/key-claims-L4.md`), each as a sentence, by the text line of
/// the read it protects; the ones no read of the text explains (a listing) under line 0.
fn claim_notes(document: &Value, entry: &Value, lines: &BTreeMap<String, u32>) -> BTreeMap<u32, Vec<String>> {
    let (map, depth) = run_trace(document, entry);
    let text_field = |record: &Value, name: &str| -> Option<String> {
        record.get("value")?.as_array()?.iter().find(|kv| kv.get(0).and_then(Value::as_str) == Some(name))?.get(1)?.get("value")?.as_str().map(str::to_string)
    };
    // Each read's key (`seen[k2]`, or `seen[*]` for a listing) and the line of its node.
    let reads: Vec<(String, Option<u32>)> = entry
        .get("reads")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let request = r.get("request")?;
            let collection = text_field(request, "collection")?;
            let key = match text_field(request, "get") {
                Some(k) => format!("{collection}[{k}]"),
                None => format!("{collection}[*]"),
            };
            let path: Vec<u64> = r.get("path").and_then(Value::as_array).map(|p| p.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            let line = node_of_path(&map, depth, &path).and_then(|id| lines.get(&id).copied());
            Some((key, line))
        })
        .collect();
    let busy = entry.get("error").and_then(Value::as_str).is_some_and(|e| e.starts_with("busy:"));
    let retries = entry.get("retries").and_then(Value::as_u64).unwrap_or(0);
    let mut notes: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut retry = 0;
    for c in entry.get("claims").and_then(Value::as_array).into_iter().flatten() {
        let key = c.get("key").and_then(Value::as_str).unwrap_or("?");
        let waited = c.get("waited_ms").and_then(Value::as_f64).unwrap_or(0.0);
        let overtaken = c.get("overtaken").and_then(Value::as_bool).unwrap_or(false);
        let held = c.get("held_ms").and_then(Value::as_f64);
        let line = reads.iter().find(|(k, _)| k == key).and_then(|(_, l)| *l).unwrap_or(0);
        let note = if overtaken {
            retry += 1;
            format!("claimed {key} before calling out: waited {waited:.0} ms for another run, then found what it read had changed and ran again from there (retry {retry} of {retries}), having sent nothing")
        } else if let Some(held) = held {
            format!("claimed {key} before calling out: waited {waited:.0} ms, held it {held:.0} ms until it was done")
        } else if busy {
            format!("waited {waited:.0} ms for {key}, held by another run, and was turned away as busy")
        } else {
            format!("waited {waited:.0} ms for {key}, held by another run")
        };
        notes.entry(line).or_default().push(note);
    }
    notes
}

fn recorded(document: &Value, entry: &Value) -> Recorded {
    let (map, depth) = run_trace(document, entry);
    let timeline = entry.get("timeline").and_then(Value::as_array).cloned().unwrap_or_default();
    let round = |t: &Value| t.get("round").and_then(Value::as_u64).unwrap_or(0);
    let last = timeline.iter().map(round).max().unwrap_or(0);
    let (mut values, mut ran, mut reasons) = (HashMap::new(), HashSet::new(), HashMap::new());
    let part: HashSet<String> = map.iter().cloned().collect();
    for t in timeline.iter().filter(|t| round(t) == last) {
        let path: Vec<u64> = t.get("path").and_then(Value::as_array).map(|p| p.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        if path.len() < depth || (depth == 2 && path[0] != 0) {
            continue;
        }
        let Some(id) = map.get(path[depth - 1] as usize) else { continue };
        if path.len() > depth {
            // A step inside the node (a js node's engine): only why it gave nothing is of interest
            // here. The engine's own failure (out of fuel, a trap) is the step's `reason`; a body's
            // exception is the engine's answer, `{ error }`, recorded as the step's output.
            if let Some(reason) = t.get("reason").and_then(Value::as_str).map(str::to_string).or_else(|| engine_error(t)) {
                reasons.entry(id.clone()).or_insert(reason);
            }
            continue;
        }
        ran.insert(id.clone());
        if let Some(outputs) = t.get("outputs").and_then(Value::as_array) {
            values.insert(id.clone(), outputs.clone());
        }
        if let Some(reason) = t.get("reason").and_then(Value::as_str) {
            reasons.entry(id.clone()).or_insert_with(|| reason.to_string());
        }
    }
    Recorded { values, ran, part, reasons }
}

/// The error a js engine's step answered with, when its one output is the engine's record
/// (`{ error }`, or `{ value }` with no error): a body's exception, as the engine worded it.
fn engine_error(step: &Value) -> Option<String> {
    let outputs = step.get("outputs")?.as_array()?;
    let [record] = outputs.as_slice() else { return None };
    if record.get("type")?.as_str()? != "Record" {
        return None;
    }
    let fields = record.get("value")?.as_array()?;
    let keys: Vec<&str> = fields.iter().filter_map(|kv| kv.get(0)?.as_str()).collect();
    if !keys.iter().all(|k| *k == "error" || *k == "value") {
        return None;
    }
    let error = fields.iter().find(|kv| kv.get(0).and_then(Value::as_str) == Some("error"))?;
    Some(body_position(error.get(1)?.get("value")?.as_str()?))
}

/// The engine's position in a message, said as the body's: Boa writes `at line L, col C (native)`
/// for a syntax error and `(unknown at :L:C)` for a frame, both counted in the source the engine
/// was given, whose line 1 is the js statement's header line.
fn body_position(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message).trim_end_matches(" (native)");
    if let Some((head, tail)) = first.split_once(" (unknown at :") {
        let (l, c) = tail.trim_end_matches(')').split_once(':').unwrap_or((tail, ""));
        return format!("{head} (the body's line {l}, col {c})");
    }
    if let Some((head, tail)) = first.split_once(" at line ") {
        return format!("{head} (the body's line {tail})");
    }
    first.to_string()
}

/// A node as a sentence names it: its leaf, and its one telling param (`field "author"`).
fn node_name(n: &crate::canvas::Node) -> String {
    let telling = ["name", "key", "collection", "host", "value"];
    let param = telling.iter().find_map(|k| n.data.params.iter().find_map(|p| match p {
        WParam::Frozen { name, value } if name == k && !value.is_empty() => Some(value.clone()),
        _ => None,
    }));
    match param {
        Some(p) => format!("{} {}", n.data.name, serde_json::to_string(&p).unwrap_or(p)),
        None => n.data.name.clone(),
    }
}

/// One recorded run, as the flow's text with each statement's value under it: what a person (or
/// an agent) reads to see what the program did. `name` heads the text (`flow <name>`).
pub fn explain(name: &str, document: &Value, entry: &Value) -> Result<String, String> {
    let doc: FlowDocument = serde_json::from_value(document.clone()).map_err(|e| format!("the deployed document doesn't read: {e}"))?;
    let bindings: BTreeMap<String, String> = document.get("bindings").and_then(|b| serde_json::from_value(b.clone()).ok()).unwrap_or_default();
    let (text, lines) = print_lines(Some(name), &doc, &mut MapResolver::new(), &Names::new(), &bindings)?;
    // `part`: the nodes this run is made of (its route's, or the whole flow's); the rest of the
    // text is shown without values.
    let Recorded { values, ran, part, reasons } = recorded(document, entry);
    let has = |id: &str| values.get(id).is_some_and(|o| o.iter().any(|v| !v.is_null()));
    let by_id: HashMap<&str, &crate::canvas::Node> = doc.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut feeds: HashMap<&str, Vec<&str>> = HashMap::new();
    for w in &doc.edges {
        feeds.entry(w.target.as_str()).or_default().push(w.source.as_str());
    }
    let mut on_line: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
    for (id, line) in &lines {
        on_line.entry(*line).or_default().push(id.as_str());
    }
    let mut claims = claim_notes(document, entry, &lines);
    // Why a node has no value: the first node upstream that gave nothing though everything
    // feeding it had a value.
    let origin = |id: &str| -> String {
        let mut stack = vec![id];
        let mut seen = HashSet::new();
        let mut found = None;
        while let Some(n) = stack.pop() {
            if !seen.insert(n) || has(n) {
                continue;
            }
            let from = feeds.get(n).cloned().unwrap_or_default();
            if from.iter().all(|f| has(f)) {
                found = Some(n);
            }
            stack.extend(from.iter().filter(|f| !has(f)));
        }
        match found.and_then(|n| by_id.get(n)) {
            Some(n) if ran.contains(&n.id) => match reasons.get(n.id.as_str()) {
                Some(why) => format!("nothing: {} gave nothing: {why}", node_name(n)),
                None => format!("nothing: {} gave nothing", node_name(n)),
            },
            Some(n) => format!("nothing: {} didn't run", node_name(n)),
            None => "nothing".into(),
        }
    };
    let annotate = |id: &str| -> String {
        let Some(node) = by_id.get(id) else { return String::new() };
        if !part.contains(id) {
            return String::new();
        }
        // A route's request: what arrived, as JSON when it was (the run's line says the rest).
        if node.data.name == "request" {
            let json = node.data.outputs.iter().position(|p| p.name == "json").and_then(|i| values.get(id)?.get(i)).filter(|v| !v.is_null());
            return json.map(|j| format!("req.json = {}", show(j))).unwrap_or_default();
        }
        if has(id) {
            let outs = &values[id];
            if outs.len() == 1 {
                return show(&outs[0]);
            }
            let named: Vec<String> = outs.iter().zip(&node.data.outputs).filter(|(v, _)| !v.is_null()).map(|(v, p)| format!("{}: {}", p.name, show(v))).collect();
            return named.join(", ");
        }
        if ran.contains(id) && !values.contains_key(id) {
            return "(its value was too large to record)".into();
        }
        origin(id)
    };
    let mut out = vec![run_line(entry)];
    if let Some(unit) = entry.get("unit").and_then(Value::as_str).filter(|u| u.len() >= 12) {
        out[0].push_str(&format!(" (version {})", &unit[..12]));
    }
    if values.is_empty() && ran.is_empty() {
        out.push("(this run recorded no steps: the values below are left out)".into());
    }
    out.push(String::new());
    let width = text.lines().count().to_string().len();
    for (i, line) in text.lines().enumerate() {
        let n = (i + 1) as u32;
        out.push(format!("{n:>width$}  {line}"));
        let Some(ids) = on_line.get(&n) else { continue };
        let here: HashSet<&str> = ids.iter().copied().collect();
        let mut roots: Vec<&str> = ids.iter().copied().filter(|id| !doc.edges.iter().any(|w| w.source == *id && here.contains(w.target.as_str()))).collect();
        roots.sort_by_key(|id| doc.nodes.iter().position(|n| n.id == *id));
        let indent = line.len() - line.trim_start().len();
        for id in &roots {
            let note = annotate(id);
            if !note.is_empty() {
                out.push(format!("{:>width$}  {}  ⇒ {note}", "", " ".repeat(indent)));
            }
        }
        // A node inlined into the statement (a js call inside `respond { … }`) that gave nothing
        // for a recorded reason: said too, since the root's own value hides it.
        for id in ids.iter().filter(|id| !roots.contains(id)) {
            if let (Some(why), Some(node)) = (reasons.get(*id), by_id.get(id))
                && !has(id)
            {
                out.push(format!("{:>width$}  {}  ⇒ {} gave nothing: {why}", "", " ".repeat(indent), node_name(node)));
            }
        }
        for note in claims.remove(&n).unwrap_or_default() {
            out.push(format!("{:>width$}  {}  {note}", "", " ".repeat(indent)));
        }
    }
    out.push(String::new());
    for note in claims.into_values().flatten() {
        out.push(note);
    }
    if let Some(body) = entry.get("response_body").and_then(Value::as_str) {
        let body: String = if body.chars().count() > 400 { format!("{}…", body.chars().take(400).collect::<String>()) } else { body.to_string() };
        out.push(format!("answered {}: {body}", entry.get("status").and_then(Value::as_u64).map(|s| s.to_string()).unwrap_or_default()));
    }
    for p in entry.get("performed").and_then(Value::as_array).into_iter().flatten() {
        out.push(format!("performed {}: {}", p.get("capability").and_then(Value::as_str).unwrap_or("?"), p.get("payload").map(show).unwrap_or_default()));
    }
    for x in entry.get("rounds").and_then(Value::as_array).into_iter().flatten().filter_map(|r| r.get("exchanges").and_then(Value::as_array)).flatten() {
        out.push(format!("called out: {} → {}", x.get("request").map(show).unwrap_or_default(), x.get("response").map(show).unwrap_or_default()));
    }
    if let Some(e) = entry.get("error").and_then(Value::as_str) {
        out.push(format!("failed: {e}"));
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    Ok(out.join("\n"))
}

// ── the reference ───────────────────────────────────────────────────────────────────────────

const GUIDE: &str = r#"Holon: flows written as text. A flow is a small service (routes, clocks, actions), built from
pure parts. Holon derives what it may do from the text itself, runs only what is granted, and
keeps a record of every run that replays exactly.

A FILE
  flow notes-api            its name (what it deploys as)
  port 8102                 the port its routes answer on
  collection notes          a named store of items, used as notes[key]
  optional secret.key       a permission it can run without

HANDLERS (each starts runs; a flow has one or more, each a block)
  route GET /notes/:id (req) {
    respond notes[req.params.id]
  }
  req.params.id, req.query.after, req.headers.authorization, req.body (text), req.json (parsed)
  respond <body>, status: 201, missing: 400   (missing: the status when the body has no value)
  respond 204                                  (a status alone, no body)

  every 10s { state.last = now }               a clock: now is the time of the tick
  on file inbox (f) { … }                      each new file in the folder inbox, f its handle
  action reset (input) {                       run by hand, by name: input is what was sent
    state.count = input.to ?? 0                (JSON; {} for nothing), never from the network
    respond { set: input.to ?? 0 }             its answer: a value alone, or none
  }

  A block is everything tied to its trigger. Its writes and calls are tied to it, though they
  needn't read it; `state.seen = id after found` ties one to a value instead (it writes only when
  found has a value). A statement tied to nothing is an error. At the top level, beside handlers,
  only bindings of literals.
  Without handlers, the top-level statements are the flow, one run of all of it: `every 10s` or
  `on file inbox` in the header starts it. A flow nothing starts is a holon: to run it by hand,
  put it in `action run (input) { … }`.

BINDINGS AND VALUES
  name = expression          one form, everywhere; a binding used twice is one node
  1, 2.5, 5m, 1d             numbers; a duration is seconds
  "text", "Hello, {name}"    {…} interpolates any value (a record or list as JSON)
  true, false
  { title: t, "a key": v }   a record;  { ..base, extra: v } adds fields
  [a, b, c]                  a list
  r.field, r["a key"]        a field; a missing field has no value (it is not an error)

OPERATORS
  + - * / on numbers;  == != < <= > >=;  && || !
  a ?? b                     a, or b when a has no value
  a when cond                a only when cond is true, else no value
  if c then a else b
  A value that doesn't arrive makes everything built from it absent: that is how a flow says
  "no" (respond … missing: 404 answers it).

THE OUTSIDE WORLD (each is one permission the flow asks for)
  secret.api_token                         a secret, set by an admin
  state.count                              read at the start of the run
  state.count = (state.count ?? 0) + 1     written when the run finishes
  notes[key]  notes[key] = v  delete notes[key]  notes.list(prefix: p, after: a, limit: 50)
  now, random                              the run's time (epoch seconds) and one random number
  http.get(host, path, headers: h, body: b)   also post, put, patch, delete, head;
                                           host is a text literal: it is the permission
  stripe.verify(req, secret.whsec, now)    a verified Stripe event (.event)
  calls billing                            (a header) another deployed flow, by its slug
  r = billing.charge({ id: 1 })            runs its action charge with a value ({} if none):
                                           r is {answer, run}, or {error, message} when it
                                           didn't run; a permission, and never a loop
  await paid, expires: 1d                  park the run until the event arrives
  resume_token(paid)                       the token a resuming route takes
  route GET /paid/:token (req) resumes paid { … }
  json(x), parse_json(text)

CALLS
  leaf(inputs…, param: value)    positional arguments are inputs; a named one is an input when
                                 the leaf has that port, else a param
  sha256(t), hmac_sha256(secret.k, t), uppercase(t), split(t, ","), join(list, "-"),
  map(list, fn(x) -> x * 2), fold(list, 0, fn(acc, x) -> acc + x), …  (the leaves below)

HOLONS (a flow inside a flow: a function, placed as one node)
  holon slugify(text) -> slug {
    slug = join(split(lowercase(text), " "), "-")
  }
  s = slugify(req.json.title)
  A holon with several outputs is used as h(…).name.

EFFECTS
  Writes (state.x = …, notes[k] = …, delete …, http calls whose answer isn't used) happen when the
  run finishes, and only when granted. A value computed and never used is an error.

COMMENTS
  # to the end of the line

An example, end to end:

  flow likes
  port 8104
  collection likes

  route POST /posts/:id/like (req) {
    count = (likes[req.params.id] ?? 0) + 1
    likes[req.params.id] = count
    respond { likes: count, says: "liked {count} times" }
  }
"#;

/// A type as the reference writes it.
fn type_name(t: &WType) -> String {
    match t {
        WType::List(inner) => format!("List<{}>", type_name(inner)),
        WType::Product(e) if e.is_empty() => "Unit".into(),
        WType::Product(e) => format!("({})", e.iter().map(type_name).collect::<Vec<_>>().join(", ")),
        WType::Sum(e) => e.iter().map(type_name).collect::<Vec<_>>().join(" | "),
        other => format!("{other:?}"),
    }
}

fn ports(ps: &[WPort]) -> String {
    ps.iter().map(|p| format!("{}: {}", p.name, type_name(&p.ty))).collect::<Vec<_>>().join(", ")
}

/// Every leaf in the live catalog: its interface, and a line on what it does.
pub(crate) fn leaves() -> Vec<(String, String, &'static str)> {
    genesis_units()
        .iter()
        .filter_map(|u| {
            let UnitBody::Primitive { name } = &u.body else { return None };
            let w = crate::wire::WUnit::from_core(u);
            let params: Vec<String> = w
                .params
                .iter()
                .map(|p| match p {
                    WParam::Frozen { name, value } if !value.is_empty() => format!("{name}: {value}"),
                    p => p.name().to_string(),
                })
                .collect();
            let params = if params.is_empty() { String::new() } else { format!("  {{{}}}", params.join(", ")) };
            let sig = format!("{name}({}) -> {}{params}", ports(&w.inputs), ports(&w.outputs));
            Some((name.clone(), sig, holon_core::describe(name).unwrap_or("")))
        })
        .collect()
}

/// The manual's page of leaves (docs/manual/language/leaves.md): every leaf and template, as
/// [`leaves`] and the templates describe them. A test keeps the page equal to this.
pub fn leaves_page() -> String {
    let all = leaves();
    let mut out = vec![
        "# Leaves".to_string(),
        String::new(),
        "<!-- Generated from the catalog by holon_author::agent::leaves_page: don't edit it by hand.".into(),
        "     HOLON_BLESS=1 cargo test -p holon-author --test manual writes it again. -->".into(),
        String::new(),
        "Every building block a flow is made of. Each is shown as `name(inputs) -> outputs {params}`:".into(),
        "call a leaf by its name, with its inputs in order and its parameters by name (see".into(),
        "[Calls](reference.md#calls)). Many have a shorter written form (`a + b` is `add`, `x ?? y` is".into(),
        "`coalesce`, `\"{x}\"` is `concat`), and `holon fmt` writes that form; the".into(),
        "[reference](reference.md) has them all.".into(),
    ];
    // A leaf with no capability computes; the rest are where a flow meets the outside world.
    let store = MapResolver::new();
    let reaches: HashSet<String> = genesis_units()
        .iter()
        .filter_map(|u| match &u.body {
            UnitBody::Primitive { name } if !capabilities(u, &store).is_empty() => Some(name.clone()),
            _ => None,
        })
        .collect();
    let sections = [
        ("Computing", "These touch nothing outside: given the same inputs, they always give the same output.", false),
        ("The outside world", "Each of these is something a flow may do, and so a line of what it asks for when it is deployed. The language writes most of them as nouns (`req`, `now`, `secret.x`, `state.x`, `http.get`, a collection or a folder). The writes and the calls out take a `trigger` last: what starts them, the request of the route they are in, the trigger of the holon they are in, or what `after` names. The language ties it, so it is never an argument.", true),
    ];
    for (title, about, outside) in sections {
        let these: Vec<_> = all.iter().filter(|(name, ..)| reaches.contains(name) == outside).collect();
        out.extend([String::new(), format!("## {title}"), String::new(), about.to_string(), String::new()]);
        out.push(these.iter().map(|(name, ..)| format!("[`{name}`](#{name})")).collect::<Vec<_>>().join(" · "));
        for (name, sig, doc) in these {
            out.extend([String::new(), format!("### `{name}`"), String::new(), "```".into(), sig.clone(), "```".into(), String::new(), doc.to_string()]);
        }
    }
    out.extend([
        String::new(),
        "## Templates".into(),
        String::new(),
        "Templates are written through the language's own forms (a route, `respond`, `stripe.verify`),".into(),
        "not called by name.".into(),
    ]);
    for t in templates::TEMPLATES {
        out.extend([String::new(), format!("### `{t}`"), String::new(), templates::describe(t).unwrap_or("").to_string()]);
    }
    out.push(String::new());
    out.join("\n")
}

fn first_sentence(s: &str) -> &str {
    s.find(". ").map(|i| &s[..=i]).unwrap_or(s)
}

/// The reference: with no topic, the language on one page, every leaf, the templates and the
/// samples; with a leaf's, a template's or a sample's name, that one in full.
pub fn reference(topic: Option<&str>) -> Result<String, String> {
    let all = leaves();
    match topic.map(str::trim).filter(|t| !t.is_empty()) {
        None => {
            let mut out = vec![GUIDE.trim_end().to_string(), String::new(), "LEAVES (as name(inputs) -> outputs {params}; reference with a leaf's name says more)".into()];
            out.extend(all.iter().map(|(_, sig, doc)| format!("  {sig}\n      {}", first_sentence(doc))));
            out.push(String::new());
            out.push("TEMPLATES (written through the nouns above)".into());
            out.extend(templates::TEMPLATES.iter().map(|t| format!("  {t}: {}", first_sentence(templates::describe(t).unwrap_or("")))));
            out.push(String::new());
            out.push("SAMPLES (reference with a sample's name gives its text)".into());
            out.extend(samples::samples().iter().map(|s| format!("  {}: {}", s.name, s.description)));
            Ok(out.join("\n"))
        }
        Some(t) => {
            if let Some((_, sig, doc)) = all.iter().find(|(n, _, _)| n == t) {
                return Ok(format!("{sig}\n\n{doc}"));
            }
            if let Some(doc) = templates::describe(t) {
                return Ok(format!("{t} (a template)\n\n{doc}"));
            }
            if let Some(s) = samples::sample(t) {
                return Ok(format!("# {}: {}\n# try it: {}\n{}", s.title, s.description, s.try_it, s.text));
            }
            Err(format!("no leaf, template or sample is called \"{t}\"; reference with no topic lists them all"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_carries_the_unit_its_units_its_trace_and_its_names() {
        let p = plan(samples::sample("hello-api").unwrap().text).unwrap();
        assert_eq!(p.name, "hello-api");
        assert_eq!(p.port, Some(8101));
        assert_eq!(p.routes, ["GET /hello/:name"]);
        assert_eq!(p.may, ["NetworkIngress"]);
        assert_eq!(p.unit.len(), 64);
        assert!(!p.units.is_empty());
        let trace = p.document["trace"]["GET /hello/:name"].as_array().unwrap();
        assert_eq!(trace.len(), p.document["nodes"].as_array().unwrap().len(), "every node of the route, in the handler's stored order");
        // A clock flow is traced whole, and keeps its names.
        let beat = plan(samples::sample("heartbeat").unwrap().text).unwrap();
        assert_eq!(beat.interval_seconds, Some(10));
        assert!(beat.document["trace"]["*"].is_array());
        assert!(beat.document["bindings"].as_object().unwrap().values().any(|v| v == "t"));
    }

    #[test]
    fn a_plan_says_each_mistake_as_check_does() {
        let e = plan("flow f\nport 8200\n\nroute GET /a (req) {\n  respond nope\n}\n").unwrap_err();
        assert!(e.starts_with("text:5:11: unknown name \"nope\""), "{e}");
        assert!(e.contains("^^^^"), "{e}");
        assert!(plan("port 8200\n\nroute GET /a (req) {\n  respond 204\n}\n").unwrap_err().contains("flow <name>"));
    }

    /// A flow's text: as it was saved, comments and all, when that still compiles; else printed
    /// from its canvas, with the names it was written with.
    #[test]
    fn a_flows_text_is_the_saved_text_or_its_canvas_printed() {
        let text = samples::sample("slugs").unwrap().text;
        let mut document = plan(text).unwrap().document;
        let printed = text_of("slugs", &document).unwrap();
        assert!(printed.starts_with("flow slugs\n") && printed.contains("author = slugify(req.json.author)"), "{printed}");
        assert!(!printed.contains('#'), "printed from the canvas: no comments");
        document["source"] = json!(text);
        assert_eq!(text_of("slugs", &document).unwrap(), text, "the text as saved");
        document["source"] = json!("flow slugs\nroute GET /a (req) {\n  respond nope\n}\n");
        assert_eq!(text_of("slugs", &document).unwrap(), printed, "a saved text that doesn't compile gives way to the canvas");
    }

    /// A js node that gave nothing says why, in the engine's words, under the statement that
    /// placed it: the reason rides one level deeper in the timeline (the engine's own step) and
    /// is read back to the node of the text.
    #[test]
    fn explain_says_why_a_js_node_gave_nothing() {
        let p = plan(samples::sample("nearest").unwrap().text).unwrap();
        let trace: Vec<String> = serde_json::from_value(p.document["trace"]["GET /nearest/:word"].clone()).unwrap();
        let nodes = p.document["nodes"].as_array().unwrap();
        let js_id = nodes.iter().find(|n| n["data"]["name"] == "closest").unwrap()["id"].as_str().unwrap();
        let i = trace.iter().position(|t| t == js_id).unwrap();
        // Every node of the route produced something but the js node, whose engine ran out of fuel.
        let mut timeline: Vec<Value> = trace
            .iter()
            .enumerate()
            .map(|(k, _)| {
                let outputs = if k == i { json!([null, null]) } else { json!([{ "type": "Text", "value": "x" }]) };
                json!({ "path": [0, k], "round": 0, "start_us": 0.0, "end_us": 1.0, "outputs": outputs })
            })
            .collect();
        timeline.push(json!({ "path": [0, i, 2], "round": 0, "start_us": 0.0, "end_us": 1.0, "reason": "out of fuel (200000000 instructions)" }));
        let entry = json!({ "seq": 7, "kind": "request", "route": "GET /nearest/:word", "method": "GET", "url": "/nearest/kittne", "status": 404, "duration_ms": 3.0, "at_ms": 1_792_845_653_000u64, "timeline": timeline });
        let text = explain("nearest", &p.document, &entry).unwrap();
        assert!(text.contains("⇒ nothing: closest gave nothing: out of fuel (200000000 instructions)"), "{text}");
        // A body's exception is the engine's answer, `{ error }`, kept as the engine step's output.
        let mut threw = entry.clone();
        threw["timeline"].as_array_mut().unwrap().pop();
        threw["timeline"].as_array_mut().unwrap().push(json!({ "path": [0, i, 2], "round": 0, "start_us": 0.0, "end_us": 1.0,
            "outputs": [{ "type": "Record", "value": [["error", { "type": "Text", "value": "TypeError: cannot convert 'null' or 'undefined' to object" }]] }] }));
        let text = explain("nearest", &p.document, &threw).unwrap();
        assert!(text.contains("⇒ nothing: closest gave nothing: TypeError: cannot convert 'null' or 'undefined' to object"), "{text}");
        // Without a reason recorded, the statement says only that it gave nothing.
        let mut plain = entry.clone();
        plain["timeline"].as_array_mut().unwrap().pop();
        let text = explain("nearest", &p.document, &plain).unwrap();
        assert!(text.contains("⇒ nothing: closest gave nothing\n"), "{text}");
    }

    #[test]
    fn a_runs_line_says_who_sent_it() {
        let entry = json!({ "seq": 4, "kind": "request", "method": "POST", "url": "/slugs", "status": 200, "duration_ms": 1.25, "at_ms": 1_792_845_653_000u64, "by": "token:claude" });
        assert_eq!(run_line(&entry), "run 4: POST /slugs → 200, 1.2 ms, at 2026-10-24 12:40:53 UTC, sent by token:claude");
        let mut on_its_port = entry.clone();
        on_its_port.as_object_mut().unwrap().remove("by");
        assert!(!run_line(&on_its_port).contains("sent by"));
    }

    /// A run that ran again, or waited for a key another run held, says so on its line.
    #[test]
    fn a_runs_line_says_it_retried_and_waited() {
        let entry = json!({ "seq": 5, "kind": "request", "method": "POST", "url": "/charges", "status": 200, "duration_ms": 696.1, "at_ms": 1_792_845_653_000u64,
            "retries": 1, "claims": [{ "key": "charges[k2]", "waited_ms": 681.4, "overtaken": true }, { "key": "charges[k2]", "waited_ms": 0.2, "overtaken": false, "held_ms": 2.0 }] });
        assert_eq!(run_line(&entry), "run 5: POST /charges → 200, 696.1 ms, retried ×1, waited 682 ms for a key, at 2026-10-24 12:40:53 UTC");
        let quiet = json!({ "seq": 4, "kind": "request", "method": "POST", "url": "/charges", "status": 200, "duration_ms": 677.7, "claims": [{ "key": "charges[k2]", "waited_ms": 0.1, "overtaken": false, "held_ms": 676.0 }] });
        assert_eq!(run_line(&quiet), "run 4: POST /charges → 200, 677.7 ms");
    }

    /// The claims a run took are said under the statement that made the read they protect: the
    /// run that waited and ran again, and the run that held the key while it called out.
    #[test]
    fn explain_says_what_a_run_claimed_under_its_read() {
        let p = plan(samples::sample("cache-aside").unwrap().text).unwrap();
        let trace: Vec<String> = serde_json::from_value(p.document["trace"]["GET /id/:name"].clone()).unwrap();
        let nodes = p.document["nodes"].as_array().unwrap();
        let read_id = nodes.iter().find(|n| n["data"]["name"] == "collection_get").unwrap()["id"].as_str().unwrap();
        let k = trace.iter().position(|t| t == read_id).unwrap();
        let read = json!({ "path": [0, k], "request": { "type": "Record", "value": [["collection", { "type": "Text", "value": "ids" }], ["get", { "type": "Text", "value": "ada" }]] }, "answer": null });
        let waited = json!({ "seq": 5, "kind": "request", "route": "GET /id/:name", "method": "GET", "url": "/id/ada", "status": 200, "duration_ms": 696.1, "retries": 1,
            "reads": [read], "claims": [{ "key": "ids[ada]", "waited_ms": 681.4, "overtaken": true }, { "key": "ids[ada]", "waited_ms": 0.2, "overtaken": false, "held_ms": 2.0 }] });
        let text = explain("cache-aside", &p.document, &waited).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let at = lines.iter().position(|l| l.contains("id = ids[name] ??")).unwrap_or_else(|| panic!("{text}"));
        let under: Vec<&str> = lines[at + 1..].iter().take_while(|l| !l.trim_start().starts_with(|c: char| c.is_ascii_digit())).copied().collect();
        assert!(under.iter().any(|l| l.trim() == "claimed ids[ada] before calling out: waited 681 ms for another run, then found what it read had changed and ran again from there (retry 1 of 1), having sent nothing"), "{text}");
        assert!(under.iter().any(|l| l.trim() == "claimed ids[ada] before calling out: waited 0 ms, held it 2 ms until it was done"), "{text}");
        let busy = json!({ "seq": 6, "kind": "request", "route": "GET /id/:name", "method": "GET", "url": "/id/ada", "status": 503, "duration_ms": 50.3, "error": "busy: ids[ada] is held by another run",
            "reads": [read], "claims": [{ "key": "ids[ada]", "waited_ms": 50.0, "overtaken": false }] });
        let text = explain("cache-aside", &p.document, &busy).unwrap();
        assert!(text.contains("waited 50 ms for ids[ada], held by another run, and was turned away as busy"), "{text}");
    }

    #[test]
    fn times_read_in_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_792_845_653), "2026-10-24 12:40:53 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00 UTC");
    }

    #[test]
    fn values_read_as_the_language_writes_them() {
        let rec = json!({"type": "Record", "value": [["title", {"type": "Text", "value": "hi"}], ["n", {"type": "Number", "value": 3.0}]]});
        assert_eq!(show(&rec), r#"{ title: "hi", n: 3 }"#);
        assert_eq!(show(&json!({"type": "List", "value": [{"type": "Boolean", "value": true}]})), "[true]");
        assert_eq!(show(&Value::Null), "nothing");
        assert!(show(&json!({"type": "Text", "value": "x".repeat(500)})).ends_with('…'));
    }

    #[test]
    fn the_reference_lists_the_language_every_leaf_and_the_samples() {
        let all = reference(None).unwrap();
        assert!(all.contains("HANDLERS") && all.contains("  concat(") && all.contains("  hello-api: "), "{all}");
        assert!(reference(Some("coalesce")).unwrap().starts_with("coalesce("));
        assert!(reference(Some("respond")).unwrap().contains("(a template)"));
        assert!(reference(Some("likes")).unwrap().contains("collection likes"));
        assert!(reference(Some("nope")).is_err());
    }

    /// The likes example in the guide is a real flow.
    #[test]
    fn the_guides_example_compiles() {
        let example: String = GUIDE.split("An example, end to end:").nth(1).unwrap().lines().map(|l| l.strip_prefix("  ").unwrap_or(l)).collect::<Vec<_>>().join("\n");
        let p = plan(example.trim_start()).unwrap();
        assert_eq!(p.routes, ["POST /posts/:id/like"]);
    }
}
