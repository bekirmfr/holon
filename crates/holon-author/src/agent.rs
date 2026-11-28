//! What the agent tools (`holon mcp`, docs/L4s/agent-surface-L4.md) compute without a server: a
//! text compiled into what a deploy sends, a recorded run shown as the program with each
//! statement's value, a run in one line, and the language's reference. All of it reads what core
//! and the language already compute; the server side only moves it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use holon_core::{capabilities, consent_name, encode_preimage, genesis_units, MapResolver, Resolver, UnitBody};
use serde_json::{json, Value};

use crate::builder::{closure_of, FlowDocument};
use crate::commands;
use crate::composites::Names;
use crate::describe::{describe_item, describe_item_sending};
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
    pub requirements: Option<Value>,
    pub browsers: Option<String>,
    /// What it may do, as consent names (`NetworkIngress`, `StateWrite("beats")`), sorted.
    pub may: Vec<String>,
    /// Where each secret it reads is sent (core's `sent_secrets`): secret → hosts. Said beside the
    /// secret's item ([`describe_sending`]).
    pub sent: BTreeMap<String, BTreeSet<String>>,
}

/// The text compiled into a [`Plan`]; or its mistakes, as `holon check` prints them.
pub fn plan(source: &str) -> Result<Plan, String> {
    let mut store = MapResolver::new();
    let c = commands::compile("text", source, &mut store).map_err(|r| r.err.join("\n"))?;
    let name = c.name.clone().ok_or("the text needs a `flow <name>` line: the name is what it deploys as")?;
    let settings = c.doc.server.clone().unwrap_or_default();
    let server = derive_server(&c.doc.nodes, &c.doc.edges, &mut store, &settings.port, settings.unit());
    if !server.errors.is_empty() {
        return Err(server.errors.join("\n"));
    }
    let hash = flow_unit(&c.doc.nodes, &c.doc.edges, &mut store, &settings.port, settings.unit())?;
    let root = store.resolve(&hash).ok_or("the flow's unit wasn't stored")?.clone();
    // A server's handlers each have their own (a route's, a clock's, an action's…); a flow with
    // none runs whole.
    let handlers = !(server.endpoints.is_empty() && server.clocks.is_empty() && server.files.is_empty() && server.mails.is_empty() && server.feeds.is_empty() && server.queues.is_empty() && server.websockets.is_empty() && server.actions.is_empty());
    let trace = if !handlers {
        let all: Vec<_> = c.doc.nodes.iter().collect();
        BTreeMap::from([("*".to_string(), trace_of(&root, &all))])
    } else {
        server.trace.clone()
    };
    let mut may: Vec<String> = holon_core::deployed_capabilities(&root, &store).iter().map(|c| consent_name(&c)).collect();
    // A module (a js node) adds no capability; its budgets are limits the deploy asks for, and
    // the person approving sees them (the server merges them into the manifest the same way).
    may.extend(holon_core::module_limits_of(&root, &store).iter().map(|(name, n)| format!("Limit({name}={n})")));
    may.sort();
    let sent = holon_core::sent_secrets(&root, &store);
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
        requirements: settings.requirements.as_ref().and_then(|r| serde_json::to_value(r).ok()),
        browsers: settings.browsers.clone(),
        may,
        sent,
    })
}

/// The canvas a flow's code is shown on, from the text it keeps (docs/L4s/text-deploy-plan.md):
/// the text compiled, its source kept in it, with its hand-drawn `layout` (a
/// [`crate::kept_layout::Layout`], or null) put on it when it was drawn for this code; and whether
/// it was (none: no layout). Refused when the text compiles here to other code than `unit`, the
/// code it runs (another release's compiler): what is shown, and a run's steps laid on it, would be
/// other code than runs. Every reading of a kept flow's canvas comes through here, natively and in
/// the editor through the bridge.
pub fn document_of(text: &str, unit: &str, layout: Option<&Value>) -> Result<(Value, Option<crate::kept_layout::Placed>), String> {
    let (document, compiled, placed) = canvas_of(text, layout)?;
    if !unit.is_empty() && compiled != unit {
        return Err(other_code(&compiled, unit));
    }
    Ok((document, placed))
}

/// What [`document_of`] says when a kept text compiles here to other code than `unit`, the code it
/// runs.
pub fn other_code(compiled: &str, unit: &str) -> String {
    format!(
        "its text compiles here to other code ({}) than it runs ({}): this Holon is another release than the one that compiled it; update whichever is older",
        crate::first_chars(compiled, 12),
        crate::first_chars(unit, 12)
    )
}

/// A text's canvas ([`document_of`]) and the unit it compiles to here, without holding it to the
/// code it runs: the caller says which code that is, or that they differ.
pub fn canvas_of(text: &str, layout: Option<&Value>) -> Result<(Value, String, Option<crate::kept_layout::Placed>), String> {
    let plan = plan(text)?;
    let mut document = plan.document;
    document["source"] = Value::String(text.to_string());
    let layout: Option<crate::kept_layout::Layout> = layout.filter(|l| !l.is_null()).map(|l| serde_json::from_value(l.clone()).map_err(|e| format!("its layout doesn't read: {e}"))).transpose()?;
    let placed = layout.map(|l| crate::kept_layout::apply(&mut document, &plan.unit, &l));
    Ok((document, plan.unit, placed))
}

/// What a canvas sends to be kept (docs/L4s/text-deploy-plan.md): `text` (its text as typed, or the
/// canvas printed) compiled, its unit, and where the canvas's nodes are when a person moved them
/// (none when every node is where the compiler puts it). Refused when `canvas` isn't the code its
/// text compiles to (its handlers differ).
pub fn layout_of(canvas: &Value, text: &str) -> Result<(String, Option<crate::kept_layout::Layout>), String> {
    let plan = plan(text)?;
    let layout = crate::kept_layout::of_canvas(canvas, &plan.unit, &plan.document)?;
    Ok((plan.unit, layout))
}

/// A flow saved as a canvas document (0.1.0-rc.37 and before) as it is kept now: its text, the
/// text the document carried when that still compiles to its canvas, else its canvas printed (as
/// the canvas named its nodes), and where its nodes were drawn, read off its canvas ([`layout_of`];
/// none when they can't be). How a server reads a saved-flows file written before flows were kept
/// as their text, so it starts on it and loses none.
pub fn kept_from_document(name: &str, document: &Value) -> Result<(String, Option<crate::kept_layout::Layout>), String> {
    let doc: FlowDocument = serde_json::from_value(document.clone()).map_err(|e| format!("the flow's document doesn't read: {e}"))?;
    let source = document.get("source").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
    let says_its_canvas = |s: &str| plan(s).is_ok_and(|said| canvas_unit(&doc).is_ok_and(|canvas| canvas == said.unit));
    let text = match source {
        Some(s) if says_its_canvas(s) => s.to_string(),
        _ => {
            let bindings: BTreeMap<String, String> = document.get("bindings").and_then(|b| serde_json::from_value(b.clone()).ok()).unwrap_or_default();
            match print_lines(Some(name), &doc, &mut MapResolver::new(), &Names::new(), &bindings) {
                Ok((text, _)) => text,
                // A draft that never compiled: its text as it was typed, kept as a draft is.
                Err(e) => source.map(str::to_string).ok_or(e)?,
            }
        }
    };
    let layout = layout_of(document, &text).ok().and_then(|(_, layout)| layout);
    Ok((text, layout))
}

/// The unit `doc`'s canvas deploys as, in hex.
fn canvas_unit(doc: &FlowDocument) -> Result<String, String> {
    let mut store = MapResolver::new();
    crate::text::restore(doc, &mut store, &Names::new());
    let port = doc.server.clone().unwrap_or_default().port;
    flow_unit(&doc.nodes, &doc.edges, &mut store, &port, doc.unit_settings()).map(|canvas| to_hex(&canvas))
}

/// A consent name as a person reads it (`StateWrite("beats")` → write its state "beats").
pub fn describe(item: &str) -> String {
    describe_item(item)
}

/// A consent name as a person reads it, and beside a secret it reads, where the flow sends it
/// (`sent`, as [`Plan::sent`]): "read the secret "k", and send it to https://api.example.com".
pub fn describe_sending(item: &str, sent: &BTreeMap<String, BTreeSet<String>>) -> String {
    describe_item_sending(item, sent)
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
        // As the list of runs says it: its route, not its URL, which says the request's values.
        Some("request") if s("url").is_none() && s("route").is_some() => s("route").unwrap_or_default().to_string(),
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
    } else if entry.get("failed").and_then(Value::as_bool) == Some(true) {
        line.push_str(" — failed");
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

/// The document's trace for this run's part: the node id at each step index, and how deep in a
/// step's path that index sits. A run is placed by its route where the document has a trace of
/// that name: a request's route, or the label of the handler that started it (`every 2s`, `on
/// file inbox`, `on mail remit`, `WebSocket /room/:name message`, `action reset`…), one level
/// down, inside its handler unit (`[0, k]`); a run that went on from a wait (`resume`) keeps the
/// label of the run it continues. Else by the whole flow's (`"*"`), at the top. One rule, with no
/// list of kinds: a list left mail, feed and queue runs out, and they showed no steps
/// (R151). As the editor's `placedBy` does (`placed.fixture.json`, from the server's test of a
/// run of every kind).
fn run_trace(document: &Value, entry: &Value) -> (Vec<String>, usize) {
    let trace = |key: &str| -> Option<Vec<String>> { serde_json::from_value(document.get("trace")?.get(key)?.clone()).ok() };
    match entry.get("route").and_then(Value::as_str).and_then(trace) {
        Some(map) => (map, 2),
        None => (trace("*").unwrap_or_default(), 1),
    }
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
    // Each read's key (`seen[k2]`, or `seen[*]` for a listing) and the line of its node.
    let reads: Vec<(String, Option<u32>)> = entry
        .get("reads")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let key = read_key(r.get("request")?)?;
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

/// A read's key as its record keeps what it asked: `seen[k2]`, or `seen[*]` for a listing.
fn read_key(request: &Value) -> Option<String> {
    let text_field = |name: &str| -> Option<String> {
        request.get("value")?.as_array()?.iter().find(|kv| kv.get(0).and_then(Value::as_str) == Some(name))?.get(1)?.get("value")?.as_str().map(str::to_string)
    };
    let collection = text_field("collection")?;
    Some(match text_field("get") {
        Some(k) => format!("{collection}[{k}]"),
        None => format!("{collection}[*]"),
    })
}

/// Why an effect wasn't made, in words, from the server's note of it (`skipped`): the step
/// where its absence began (`from`, the node of the text it stands in), and what a read asked.
/// Read, never worked out again: the server's walk saw each step, and the record may not keep
/// one (a value too large to record), which a walk here would take for one that gave nothing.
fn skipped_why(note: &Value, from: Option<&crate::canvas::Node>, entry: &Value, reasons: &HashMap<String, String>) -> String {
    let name = from.map(node_name).unwrap_or_else(|| note.get("leaf").and_then(Value::as_str).unwrap_or("a step").to_string());
    let seen = if note.get("unseen").and_then(Value::as_bool) == Some(true) { ", as far as the record sees" } else { "" };
    match note.get("level").and_then(Value::as_str).unwrap_or("") {
        "read" => {
            let asked = note.get("read").and_then(Value::as_u64).and_then(|i| read_key(entry.get("reads")?.get(i as usize)?.get("request")?));
            match asked {
                Some(key) => format!("{name} found nothing at {key}{seen}"),
                None => format!("{name} found nothing{seen}"),
            }
        }
        "missing" => format!("{name} gave nothing: what it read lacked it{seen}"),
        "failed" => match from.and_then(|n| reasons.get(n.id.as_str())) {
            Some(why) => format!("{name} failed: {why}{seen}"),
            None => format!("{name} failed{seen}"),
        },
        "withheld" => "withheld: where it stopped may be computed from a secret".into(),
        "refused" => format!("refused: {}", note.get("rule").and_then(Value::as_str).unwrap_or("?")),
        other => other.to_string(),
    }
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

/// One recorded run laid over the flow's text: what [`explain`] prints, as parts a screen can lay
/// out (`holon editor` shows each line's notes beside it, and steps through them).
#[derive(Clone, Debug, PartialEq)]
pub struct RunView {
    /// The run in one line, with the version it ran on.
    pub head: String,
    /// The run recorded no steps, so no line has a value.
    pub no_steps: bool,
    /// The flow's text, as the deployed document prints.
    pub text: Vec<String>,
    /// By line (1-based): each statement's value there (`⇒ …`), and the claims it took.
    pub notes: BTreeMap<u32, Vec<String>>,
    /// After the text: claims no line explains, the answer, what it performed and called, why it failed.
    pub after: Vec<String>,
    /// When each statement ran, in the order they started: what a run's timeline draws.
    pub steps: Vec<Step>,
    /// The nodes whose steps the record notes and core held back (core's `held`, read off the
    /// record): read as not run. By node id, in the document's order.
    pub not_run: Vec<String>,
}

/// A statement's part of a run: its line (1-based), and when its steps began and ended, in
/// microseconds from the run's start (its last round's, for a run that called out).
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub line: u32,
    pub start_us: f64,
    pub end_us: f64,
}

/// One recorded run, as the flow's text with each statement's value under it: what a person (or
/// an agent) reads to see what the program did. `name` heads the text (`flow <name>`).
pub fn explain(name: &str, document: &Value, entry: &Value) -> Result<String, String> {
    let v = run_view(name, document, entry)?;
    let mut out = vec![v.head];
    if v.no_steps {
        out.push("(this run recorded no steps: the values below are left out)".into());
    }
    out.push(String::new());
    let width = v.text.len().to_string().len();
    // A flow's text is mostly what this run didn't touch (another handler's, the page, a law). A
    // line with a value is shown, with a few around it; a stretch of lines with none, where a
    // run recorded steps at all, is said in a line, so a run of a 15-line handler is not read
    // among 470.
    const AROUND: usize = 3;
    let shown = |i: usize| {
        v.no_steps || (i.saturating_sub(AROUND)..=(i + AROUND).min(v.text.len().saturating_sub(1))).any(|j| v.notes.contains_key(&((j + 1) as u32)))
    };
    let mut skipped = 0;
    for (i, line) in v.text.iter().enumerate() {
        if !shown(i) {
            skipped += 1;
            continue;
        }
        if skipped > 0 {
            out.push(format!("{:>width$}  … {skipped} line(s) this run didn't reach", ""));
            skipped = 0;
        }
        out.push(format!("{:>width$}  {line}", i + 1));
        let indent = line.len() - line.trim_start().len();
        for note in v.notes.get(&((i + 1) as u32)).into_iter().flatten() {
            out.push(format!("{:>width$}  {}  {note}", "", " ".repeat(indent)));
        }
    }
    if skipped > 0 {
        out.push(format!("{:>width$}  … {skipped} line(s) this run didn't reach", ""));
    }
    out.push(String::new());
    out.extend(v.after);
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    Ok(out.join("\n"))
}

/// One recorded run, as [`RunView`]'s parts. `name` heads the text (`flow <name>`).
pub fn run_view(name: &str, document: &Value, entry: &Value) -> Result<RunView, String> {
    let doc: FlowDocument = serde_json::from_value(document.clone()).map_err(|e| format!("the deployed document doesn't read: {e}"))?;
    let bindings: BTreeMap<String, String> = document.get("bindings").and_then(|b| serde_json::from_value(b.clone()).ok()).unwrap_or_default();
    let (text, lines) = print_lines(Some(name), &doc, &mut MapResolver::new(), &Names::new(), &bindings)?;
    // `part`: the nodes this run is made of (its route's, or the whole flow's); the rest of the
    // text is shown without values.
    let Recorded { mut values, mut ran, part, reasons } = recorded(document, entry);
    let by_id: HashMap<&str, &crate::canvas::Node> = doc.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    // A step core held back (core's `held`: its tie didn't arrive, or an input wired to it that it
    // needs didn't) is noted by the record and wasn't run: it reads as not run. What reached it is
    // read off the record, each input from the step that fed it; one whose value the record
    // doesn't keep (too large, or computed from a secret) counts as arrived, so a step is said held
    // only where the record shows it, and reads the same whatever such a value was.
    let arrived = |w: &crate::canvas::Wire| -> bool {
        if !ran.contains(&w.source) {
            return false;
        }
        let Some(outs) = values.get(&w.source) else { return true };
        let at = by_id.get(w.source.as_str()).and_then(|n| n.data.outputs.iter().position(|p| p.name == w.output()));
        // An output the record doesn't hold (a choice that lent its value is noted with none) is
        // taken as arrived, as above.
        at.and_then(|i| outs.get(i)).is_none_or(|v| !v.is_null())
    };
    let not_run: Vec<String> = ran
        .iter()
        .filter(|id| {
            let Some(n) = by_id.get(id.as_str()) else { return false };
            let wire = |p: usize| doc.edges.iter().find(|w| w.target == **id && n.data.inputs.get(p).is_some_and(|port| port.name == w.input()));
            let leaf = matches!(n.data.body, crate::canvas::PlacedBody::Primitive) && holon_core::holds_back(&n.data.name);
            holon_core::held(leaf, n.data.inputs.iter().map(|p| p.name.as_str()), |p| wire(p).is_some(), |p| wire(p).is_some_and(arrived))
        })
        .cloned()
        .collect();
    for id in &not_run {
        ran.remove(id);
        values.remove(id);
    }
    let has = |id: &str| values.get(id).is_some_and(|o| o.iter().any(|v| !v.is_null()));
    // What feeds each node, for why it has no value. Not its tie (its trigger): a step whose tie
    // didn't arrive wasn't reached, and didn't run itself, whatever held the tie back (a return
    // above it answered: a wait past it isn't asked).
    let mut feeds: HashMap<&str, Vec<&str>> = HashMap::new();
    for w in doc.edges.iter().filter(|w| w.input() != holon_core::TRIGGER) {
        feeds.entry(w.target.as_str()).or_default().push(w.source.as_str());
    }
    let mut on_line: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
    for (id, line) in &lines {
        on_line.entry(*line).or_default().push(id.as_str());
    }
    let mut claims = claim_notes(document, entry, &lines);
    // Each effect the run held and didn't make, as the server noted it (`skipped`, R99): its
    // reason is read from the note, never worked out again here. By the node of the text that
    // would have made it; one with no step to stand under (refused, withheld) is said after it.
    let (map, depth) = run_trace(document, entry);
    let mut held: HashMap<String, Vec<String>> = HashMap::new();
    let mut unplaced: Vec<String> = Vec::new();
    for note in entry.get("skipped").and_then(Value::as_array).into_iter().flatten() {
        let path = |k: &str| -> Vec<u64> { note.get(k).and_then(Value::as_array).map(|p| p.iter().filter_map(Value::as_u64).collect()).unwrap_or_default() };
        let from = node_of_path(&map, depth, &path("from")).and_then(|id| by_id.get(id.as_str()).copied());
        let why = skipped_why(note, from, entry, &reasons);
        match node_of_path(&map, depth, &path("at")).filter(|id| part.contains(id)) {
            Some(id) => held.entry(id).or_default().push(why),
            None => unplaced.push(format!("not made: {}: {why}", note.get("effect").and_then(Value::as_str).unwrap_or("?"))),
        }
    }
    // A node that ran and whose outputs the record didn't keep (too large to record) had a value:
    // never the one taken to have given nothing.
    let known = |id: &str| has(id) || (ran.contains(id) && !values.contains_key(id));
    // Why a node has no value: the first node upstream that gave nothing though everything
    // feeding it had a value.
    let origin = |id: &str| -> String {
        let mut stack = vec![id];
        let mut seen = HashSet::new();
        let mut found = None;
        while let Some(n) = stack.pop() {
            if !seen.insert(n) || known(n) {
                continue;
            }
            let from = feeds.get(n).cloned().unwrap_or_default();
            if from.iter().all(|f| known(f)) {
                found = Some(n);
            }
            stack.extend(from.iter().filter(|f| !known(f)));
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
    let mut head = run_line(entry);
    if let Some(unit) = entry.get("unit").and_then(Value::as_str).filter(|u| u.len() >= 12) {
        head.push_str(&format!(" (version {})", &unit[..12]));
    }
    let no_steps = values.is_empty() && ran.is_empty();
    let mut notes: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for (i, _) in text.lines().enumerate() {
        let n = (i + 1) as u32;
        let Some(ids) = on_line.get(&n) else { continue };
        let here: HashSet<&str> = ids.iter().copied().collect();
        let mut roots: Vec<&str> = ids.iter().copied().filter(|id| !doc.edges.iter().any(|w| w.source == *id && here.contains(w.target.as_str()))).collect();
        roots.sort_by_key(|id| doc.nodes.iter().position(|n| n.id == *id));
        let here = notes.entry(n).or_default();
        for id in &roots {
            // An effect the server noted as held says why by its note, below.
            if held.contains_key(*id) && !has(id) {
                continue;
            }
            let note = annotate(id);
            if !note.is_empty() {
                here.push(format!("⇒ {note}"));
            }
        }
        for why in ids.iter().filter_map(|id| held.get(*id)).flatten() {
            here.push(format!("⇒ not made: {why}"));
        }
        // A node inlined into the statement (a js call inside `respond { … }`) that gave nothing
        // for a recorded reason: said too, since the root's own value hides it.
        for id in ids.iter().filter(|id| !roots.contains(id)) {
            if let (Some(why), Some(node)) = (reasons.get(*id), by_id.get(id))
                && !has(id)
            {
                here.push(format!("⇒ {} gave nothing: {why}", node_name(node)));
            }
        }
        here.extend(claims.remove(&n).unwrap_or_default());
    }
    notes.retain(|_, v| !v.is_empty());
    let mut out = Vec::new();
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
    out.extend(unplaced);
    for x in entry.get("rounds").and_then(Value::as_array).into_iter().flatten().filter_map(|r| r.get("exchanges").and_then(Value::as_array)).flatten() {
        out.push(format!("called out: {} → {}", x.get("request").map(show).unwrap_or_default(), x.get("response").map(show).unwrap_or_default()));
    }
    if let Some(e) = entry.get("error").and_then(Value::as_str) {
        out.push(format!("failed: {e}"));
    }
    // A held step took no part in the run: its line has no step of it.
    let line_of: HashMap<&str, u32> = lines.iter().filter(|(id, _)| !not_run.contains(id)).map(|(id, l)| (id.as_str(), *l)).collect();
    let steps = steps_of(document, entry, &line_of);
    let mut not_run = not_run;
    not_run.sort_by_key(|id| doc.nodes.iter().position(|n| &n.id == id));
    Ok(RunView { head, no_steps, text: text.lines().map(str::to_string).collect(), notes, after: out, steps, not_run })
}

/// One recorded run as [`run_view`]'s parts, in JSON: what the Runs panel reads for its steps,
/// its timeline and its errors. `notes` is keyed by line (1-based) as a string.
pub fn run_json(name: &str, document: &Value, entry: &Value) -> Result<Value, String> {
    let v = run_view(name, document, entry)?;
    Ok(json!({
        "head": v.head,
        "no_steps": v.no_steps,
        "text": v.text,
        "notes": v.notes.iter().map(|(line, notes)| (line.to_string(), json!(notes))).collect::<serde_json::Map<String, Value>>(),
        "after": v.after,
        "steps": v.steps.iter().map(|s| json!({ "line": s.line, "start_us": s.start_us, "end_us": s.end_us })).collect::<Vec<_>>(),
        "not_run": v.not_run,
    }))
}

/// When each line's nodes ran: the timeline's steps at the run's own depth, by line.
fn steps_of(document: &Value, entry: &Value, line_of: &HashMap<&str, u32>) -> Vec<Step> {
    let (map, depth) = run_trace(document, entry);
    let timeline = entry.get("timeline").and_then(Value::as_array).cloned().unwrap_or_default();
    let round = |t: &Value| t.get("round").and_then(Value::as_u64).unwrap_or(0);
    let last = timeline.iter().map(round).max().unwrap_or(0);
    let mut by_line: BTreeMap<u32, Step> = BTreeMap::new();
    for t in timeline.iter().filter(|t| round(t) == last) {
        let path: Vec<u64> = t.get("path").and_then(Value::as_array).map(|p| p.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        if depth == 0 || path.len() != depth || (depth == 2 && path[0] != 0) {
            continue;
        }
        let Some(&line) = map.get(path[depth - 1] as usize).and_then(|id| line_of.get(id.as_str())) else { continue };
        let (start, end) = (t.get("start_us").and_then(Value::as_f64).unwrap_or(0.0), t.get("end_us").and_then(Value::as_f64).unwrap_or(0.0));
        let step = by_line.entry(line).or_insert(Step { line, start_us: start, end_us: end });
        step.start_us = step.start_us.min(start);
        step.end_us = step.end_us.max(end);
    }
    let mut steps: Vec<Step> = by_line.into_values().collect();
    steps.sort_by(|a, b| a.start_us.total_cmp(&b.start_us).then(a.line.cmp(&b.line)));
    steps
}

// ── the reference ───────────────────────────────────────────────────────────────────────────

const GUIDE: &str = r#"Holon: flows written as text, in TypeScript's syntax. A flow is a small service (routes, clocks,
actions), built from pure parts. Holon derives what it may do from the text itself, runs only what
is granted, and keeps a record of its runs, each of which replays exactly but for its steps with a
secret.

A FILE (NAME.holon.ts: its imports, then flow(…), then its handlers)
  import { flow, route, status } from "holon"   the vocabulary (flow, route, every, on, action, js,
                                                respond, status, stripe, event, approval) and
                                                the leaves
  import api_token from "secret:api_token"      a secret it reads, as api_token; set before it works
  import key from "secret:key" with { optional: "true" }   a secret it can run without
  import state from "state"                     its state, its keys by use: state.count
  import notes from "collection:notes"          a named store of items, used as notes[key]
  import inbox from "folder:inbox"              also "database:NAME", "channel:NAME",
                                                "mail:NAME" (a mail it sends through or reads)
                                                "feed:NAME" (a feed it listens to) and
                                                "queue:NAME" (a queue it listens or sends to)
  import httpbin from "host:httpbin.org"        a host it calls, as httpbin.get("/uuid")
  import { now } from "holon:clock"             the time; import { random } from "holon:random"
                                                (and random_bytes, the run's 32, for pgp_encrypt)
  import { charge } from "flow:billing"         another deployed flow's action, by its slug
  flow("notes-api", { port: 8102 })             its name (what it deploys as) and its settings:
                                                port, keep: { runs, days }, browsers, limits,
                                                taint: ["x-api-key"] (keys the log never holds),
                                                never_keep: ["cvc"] (keys never kept, at any length,
                                                by where they stand; a write or a send that holds
                                                one is refused; such a flow can't wait yet),
                                                tags: ["payments"] (who is bound on one acts on it)
  The imports name everything that outlives a run or comes from outside it; the body uses only
  names they gave. An import names, it grants nothing: what the flow may do is worked out from
  what it does with them.

HANDLERS (each starts runs; a flow has one or more)
  route.get("/notes/:id", (req) => notes[req.params.id] ?? status(404))
  route.post, .put, .patch, .delete, and route.get with a block: (req) => { …; return x }
  req.params.id, req.query.after, req.headers.authorization, req.body (text), req.json (parsed)
  route.get("/files/*path", (req) => …)          the rest of the path: /files/a/b.css, req.params.path a/b.css
  return x                                       answers 200 with x
  return status(201, x) ?? status(400)           another status; ?? says what an absent x answers
  return status(204)                             a status alone, no body
  return respond(x, { headers: h })              x with headers; { stream: true } answers with a
                                                 streamed call's body, as it arrives

  every("10s", (now) => { state.last = now })    a clock: now is the time of the tick
  on.file.created(inbox, (f) => { … }, { matching: "*.csv" })   each new file, f its handle;
                                                 on.file.deleted(inbox, (f) => { … }) too
  on.mail(remit, (m) => { … })                   each message to the mailbox remit is bound to
  on.feed(prices, (e) => { … })                  each event the feed prices sends: e.data (JSON
                                                 read, else text), e.id, e.event; one at a time
  on.queue(payouts, (m) => { … })                each message from the queue payouts: m.data, m.id,
                                                 m.group…; deleted once its run is over
  export const reset = action((input) => {       run by hand, by name: input is what was sent
    state.count = input.to ?? 0                  (JSON; {} for nothing), never from the network
    return { set: input.to ?? 0 }                its answer: a value alone, or none
  })
  route.get("/health", () => "ok", { log: "unrecorded" })   a handler's options come last: log
                                                 ("lazy" or "unrecorded"), resumes, matching

  A handler is everything tied to its trigger. Its writes and calls are tied to it, though they
  needn't read it; `if (found !== undefined) state.seen = id` ties one to a value instead (it
  writes only when found has a value). At the top level, beside handlers and functions, only
  consts of literals. A flow nothing starts has nothing to run: to run it by hand, put it in an
  exported action.

BINDINGS AND VALUES
  const name = expression    a binding used twice is one node; let may be assigned again
  1, 2.5                     numbers; a duration is a text where one is asked for: "10s", "1d"
  "text", `Hello, ${name}`   ${…} interpolates any value (a record or list as JSON)
  true, false, undefined     undefined is no value
  { title: t, "a key": v }   a record;  { ...base, extra: v } adds fields
  [a, b, c]                  a list
  r.field, r["a key"]        a field; a missing field has no value (it is not an error)
  const { a, b } = r         a field each; [x, y] = xs and [first, ...rest] = xs too

OPERATORS AND CHOICES
  + - * / on numbers;  === !== < <= > >=;  && || ! on booleans; texts join with `${a}${b}`
  a ?? b                     a, or b when a has no value
  c ? a : b                  c ? a : undefined is a only when c is true
  if, switch, return         chosen as JavaScript chooses; the writes of each branch merge
  A value that doesn't arrive makes everything built from it absent: that is how a flow says
  "no" (?? status(404) answers it).

THE OUTSIDE WORLD (each is one permission the flow asks for)
  api_token                                a secret, set by an admin
  state.count                              read at the start of the run
  state.count = (state.count ?? 0) + 1     written when the run finishes
  notes[key]  notes[key] = v  delete notes[key]  notes.list({ prefix: p, after: a, limit: 50 })
  now, random                              the run's time (epoch seconds) and one random number
  await httpbin.get(path, { headers: h, body: b })   also post, put, patch, delete, head: the
                                           host is its import, and the import is the permission
  stripe.verify(req, whsec, now)           a verified Stripe event (.event)
  const r = await charge({ id: 1 })        runs billing's action charge with a value ({} if
                                           none): r is {answer, run}, or {error, message} when it
                                           didn't run; a permission, and never a loop
  const started = payment(req.json)        a route starts this flow's action payment, not
                                           awaited: started is {run, waiting} once it waits,
                                           started.waiting.paid the token for paid
  const paid = await event("paid", { expires: "1d" })   in an action: park the run until the
                                           event arrives; a route answers and is done
  route.get("/paid/:token", () => "ok", { resumes: "paid" })   the route that delivers it
  const d = await approval("payout", { people: 2 })   in an action: park the run until 2
                                           different people approve it in Holon (or one
                                           declines): d is {approved, by}; no route resumes it
  approval("payout", { steps: [{ role: "reviewer" }, { role: "approver" }] })   each step in
                                           turn, by those holding its role (order: "any" for
                                           any order; kind: "agent" for an agent's step)
  ops.send({ to, subject, text })          mails through the mail ops an admin bound (cc, html
                                           and attachments too), once the run is over: quoted
                                           recipients are granted by name, computed ones within
                                           the binding's
  JSON.stringify(x), JSON.parse(text)

CALLS
  leaf(inputs…, { param: value })   positional arguments are inputs; a last record whose keys
                                    the leaf takes names inputs and params
  sha256(t), hmac_sha256(key, t), base64(t), t.toUpperCase(), t.split(","), list.join("-"),
  xs.map((x) => x * 2), xs.map((x, i) => …), xs.reduce((acc, x) => acc + x, 0),
  xs.filter((x) => …), xs.find(…), xs.some(…), xs.every(…), xs.toSorted((a, b) => a - b),
  xs.at(-1), xs.slice(1, -1), xs.includes(x), xs.flat(), [...new Set(xs)], Array.from(…), …
  (the leaves below; a string or list method is the leaf that has it, and one without a leaf
  is refused. A list never changes: xs.sort() is read as the list it gives, xs.toSorted(),
  where xs isn't read after; xs.push(x) alone is xs = [...xs, x]; pop, shift, unshift and
  splice are refused, naming what to write)
  { ...a, k: v, ...b, [key]: x }, Object.values(r), Object.entries(r), Object.fromEntries(ps),
  Object.groupBy(xs, (x) => x.kind), Object.hasOwn(r, k), k in r, typeof x, Object.is(a, b),
  JSON.stringify(x, ["id"], 2), JSON.parse(t, (k, v) => …)   (a record never changes:
  Object.assign(t, a) alone on a local is t = { ...t, ...a })
  const d = new Date(t); d.getUTCHours(), d.toISOString(), d.setUTCHours(0, 0, 0, 0);
  Date.UTC(y, m, d), Date.parse(t)   (UTC only: no local time, and new Date() is refused,
  as the time is now * 1000 from "holon:clock"; a Date is read by its methods, d.getTime())
  s.trim(), s.padStart(4, "0"), s.replaceAll("a", "b"), s.startsWith(t), s.slice(1, -1),
  s.charCodeAt(0), s.normalize(), String.fromCharCode(97), encodeURIComponent(s), btoa(s),
  "n=" + n, String(x), s.length   (positions count UTF-16 code units, as JavaScript's do;
  .length where s is known to be a text or a list, as a method's result is: else text_length(s)
  or length(xs); no regular expressions: s.match(re) and friends are done in js())

FUNCTIONS (a flow inside a flow, placed as one node)
  function slugify(text) {
    const slug = text.toLowerCase().split(" ").join("-")
    return slug
  }
  const s = slugify(req.json.title)
  Its output is named after what it returns (slug), out otherwise; one that returns { a, b } has
  two, read as const { a, b } = f(x).
  const page = js(function html(x) { … })   runs as JavaScript, in a sandbox, as written

EFFECTS
  Writes (state.x = …, notes[k] = …, delete …, requests whose answer isn't used) happen when the
  run finishes, and only when granted. A value computed and never used is an error.

COMMENTS
  // to the end of the line

An example, end to end:

  import { flow, route } from "holon"
  import likes from "collection:likes"

  flow("likes", { port: 8104 })

  route.post("/posts/:id/like", (req) => {
    const count = (likes[req.params.id] ?? 0) + 1
    likes[req.params.id] = count
    return { likes: count, says: `liked ${count} times` }
  })
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
        "call a leaf by its name, imported from `\"holon\"`, with its inputs in order and its parameters in a".into(),
        "last record (see [Calls](reference.md#calls)). Many are written another way (`a + b` is `add`,".into(),
        "`x ?? y` is `coalesce`, `` `${x}` `` is `concat`, `t.toUpperCase()` is `uppercase`); the".into(),
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
        ("The outside world", "Each of these is something a flow may do, and so a line of what it asks for when it is deployed. The language writes most of them as names it imports (`now`, a secret, `state.x`, a host's `get`, a collection or a folder) or a handler receives (`req`). The writes and the calls out take a `trigger` last: what starts them, the request of the route they are in, the trigger of the holon they are in, or the value an `if (x !== undefined)` around them waits for. The language ties it, so it is never an argument.", true),
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

/// A line of a description: its first sentence, cut at `max` characters with a mark where it was
/// cut, so a list of many stays a size an agent can take in one answer.
fn brief(s: &str, max: usize) -> String {
    let first = first_sentence(s);
    if first.chars().count() <= max {
        return first.to_string();
    }
    let cut: String = first.chars().take(max).collect();
    format!("{}…", cut.trim_end())
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
            out.push("SAMPLES (reference with a sample's name gives its text, with `samples` each one in full)".into());
            out.extend(samples::samples().iter().map(|s| format!("  {}: {}", s.name, brief(s.description, 160))));
            Ok(out.join("\n"))
        }
        Some("samples") => Ok(samples::samples().iter().map(|s| format!("{}: {}\n  shows: {}\n  try it: {}", s.name, s.description, s.shows.join(", "), s.try_it)).collect::<Vec<_>>().join("\n\n")),
        Some(t) => {
            if let Some((_, sig, doc)) = all.iter().find(|(n, _, _)| n == t) {
                return Ok(format!("{sig}\n\n{doc}"));
            }
            if let Some(doc) = templates::describe(t) {
                return Ok(format!("{t} (a template)\n\n{doc}"));
            }
            if let Some(s) = samples::sample(t) {
                return Ok(format!("// {}: {}\n// try it: {}\n{}", s.title, s.description, s.try_it, s.text));
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
        // A clock is traced as its handler, and keeps its names.
        let beat = plan(samples::sample("heartbeat").unwrap().text).unwrap();
        assert!(beat.document["trace"]["every 10s"].is_array(), "{}", beat.document["trace"]);
        assert!(beat.document["bindings"].as_object().unwrap().values().any(|v| v == "t"));
    }

    #[test]
    fn a_plan_says_each_mistake_as_check_does() {
        let e = plan("import { flow, route } from \"holon\"\n\nflow(\"f\", { port: 8200 })\n\nroute.get(\"/a\", () => nope)\n").unwrap_err();
        assert!(e.starts_with("text:5:23: unknown name \"nope\""), "{e}");
        assert!(e.contains("^^^^"), "{e}");
        assert!(plan("import { route, status } from \"holon\"\n\nroute.get(\"/a\", () => status(204))\n").unwrap_err().contains("a flow says what it is first"));
    }

    /// An action's run is placed by its label, one level inside its handler, as a route's is
    /// (a flow of actions alone too); a write is on its own line.
    #[test]
    fn an_actions_run_is_placed_by_its_label_and_a_write_is_on_its_line() {
        let text = "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"two\")\n\nexport const bump = action(() => {\n  const n = (state.n ?? 0) + 1\n  state.n = n\n  return n\n})\n\nexport const reset = action(() => {\n  state.n = 0\n  return 0\n})\n";
        let p = plan(text).unwrap();
        let n = p.document["trace"]["action bump"].as_array().expect("its own trace").len();
        let timeline: Vec<Value> = (0..n).map(|i| json!({ "path": [0, i], "start_us": i as f64, "end_us": i as f64 + 1.0 })).collect();
        let entry = json!({ "kind": "trigger", "route": "action bump", "timeline": timeline });
        let v = run_view("two", &p.document, &entry).unwrap();
        let noted: Vec<u32> = v.notes.keys().copied().collect();
        for line in [7, 8, 9] {
            assert!(noted.contains(&line), "line {line}, {:?}: {noted:?}", v.text[line as usize - 1]);
        }
        assert!(!noted.iter().any(|l| *l >= 11), "the other action didn't run: {noted:?}");
    }

    /// **Guards:** a kept flow is shown as the code that runs: its text, as written, compiled to
    /// the unit it runs, with its hand-drawn layout put back only on that code.
    ///
    /// **Why:** a flow travels and is kept as its text (docs/L4s/text-deploy-plan.md); every canvas,
    /// review and run view is that text compiled again, here or in the editor. A text that compiles
    /// to other code than runs (another release's compiler) would lay a run's steps, or a person's
    /// review, on other nodes than ran.
    ///
    /// **If it fails:** someone opening a deployed flow, a version, or a run reads one program
    /// while another runs, or sees a canvas half in its drawn places and half moved.
    ///
    /// Every sample: its text read back as its canvas holds its text as written and its own trace;
    /// held to its own unit it is shown, held to another sample's it is refused; moved by hand, its
    /// layout comes back as drawn on its own code, and on another sample's code none of it does.
    #[test]
    fn every_sample_reads_back_as_its_own_code_and_drawing() {
        let all = samples::samples();
        let plans: Vec<Plan> = all.iter().map(|s| plan(s.text).unwrap_or_else(|e| panic!("{}: {e}", s.name))).collect();
        for (i, (sample, own)) in all.iter().zip(&plans).enumerate() {
            let (doc, placed) = document_of(sample.text, &own.unit, None).unwrap_or_else(|e| panic!("{}: {e}", sample.name));
            assert_eq!(doc["source"], json!(sample.text), "{}: its text as written", sample.name);
            assert_eq!(doc["trace"], own.document["trace"], "{}: its own trace", sample.name);
            assert_eq!(placed, None, "{}: no layout, none placed", sample.name);
            let other = &plans[(i + 1) % all.len()];
            if other.unit != own.unit {
                let refused = document_of(sample.text, &other.unit, None).expect_err(sample.name);
                assert!(refused.contains("other code"), "{}: {refused}", sample.name);
            }
            // Moved by hand: every node somewhere the compiler doesn't put it.
            let mut drawn = own.document.clone();
            for (k, n) in drawn["nodes"].as_array_mut().into_iter().flatten().enumerate() {
                n["position"] = json!({ "x": 5000.0 + k as f64, "y": -3.0 * k as f64 });
            }
            let (unit, layout) = layout_of(&drawn, sample.text).unwrap_or_else(|e| panic!("{}: {e}", sample.name));
            assert_eq!(unit, own.unit);
            let layout = serde_json::to_value(layout.expect("moved by hand")).unwrap();
            let (back, placed) = document_of(sample.text, &own.unit, Some(&layout)).unwrap();
            assert_eq!(placed, Some(crate::kept_layout::Placed::AsDrawn), "{}", sample.name);
            assert_eq!(back["nodes"], drawn["nodes"], "{}: every node where it was drawn", sample.name);
            if other.unit != own.unit {
                let other_text: &str = all[(i + 1) % all.len()].text;
                let (elsewhere, _, placed) = canvas_of(other_text, Some(&layout)).unwrap();
                assert_eq!(placed, Some(crate::kept_layout::Placed::OtherCode), "{}'s layout on other code", sample.name);
                assert_eq!(elsewhere["nodes"], document_of(other_text, "", None).unwrap().0["nodes"], "{}'s layout moves nothing of other code", sample.name);
            }
        }
    }

    /// A js node that gave nothing says why, in the engine's words, under the statement that
    /// placed it: the reason rides one level deeper in the timeline (the engine's own step) and
    /// is read back to the node of the text.
    #[test]
    fn run_json_gives_the_lines_notes_and_steps_a_screen_lays_out() {
        let p = plan(samples::sample("hello-api").unwrap().text).unwrap();
        let trace: Vec<String> = serde_json::from_value(p.document["trace"]["GET /hello/:name"].clone()).unwrap();
        let timeline: Vec<Value> = (0..trace.len()).map(|k| json!({ "path": [0, k], "round": 0, "start_us": k as f64, "end_us": k as f64 + 1.0, "outputs": [{ "type": "Text", "value": "x" }] })).collect();
        let entry = json!({ "seq": 1, "kind": "request", "route": "GET /hello/:name", "method": "GET", "url": "/hello/ada", "status": 200, "duration_ms": 1.0, "at_ms": 0, "timeline": timeline });
        let v = run_json("hello-api", &p.document, &entry).unwrap();
        let lines = v["text"].as_array().unwrap();
        let steps = v["steps"].as_array().unwrap();
        assert!(!steps.is_empty(), "{v}");
        for s in steps {
            let line = s["line"].as_u64().unwrap() as usize;
            assert!(line >= 1 && line <= lines.len(), "{v}");
        }
        assert!(v["notes"].as_object().unwrap().keys().all(|k| k.parse::<usize>().is_ok()), "{v}");
    }

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
        assert!(reference(Some("likes")).unwrap().contains("\nimport likes from \"collection:likes\"\n"));
        assert!(reference(Some("nope")).is_err());
    }

    /// An answer an agent takes whole: the reference with no topic keeps each sample to a line
    /// (they were a paragraph each, nearly half of 88 KB), and `samples` has them in full.
    #[test]
    fn the_reference_keeps_each_sample_to_a_line_and_samples_gives_them_in_full() {
        let all = reference(None).unwrap();
        let listed = all.lines().filter(|l| samples::samples().iter().any(|s| l.starts_with(&format!("  {}: ", s.name)))).count();
        assert_eq!(listed, samples::samples().len());
        assert!(all.len() < 64_000, "{} bytes", all.len());
        let full = reference(Some("samples")).unwrap();
        let tidewheel = samples::sample("tidewheel").unwrap();
        assert!(full.contains(tidewheel.description) && full.contains("try it: "));
    }

    /// The likes example in the guide is a real flow.
    #[test]
    fn the_guides_example_compiles() {
        let example: String = GUIDE.split("An example, end to end:").nth(1).unwrap().lines().map(|l| l.strip_prefix("  ").unwrap_or(l)).collect::<Vec<_>>().join("\n");
        let p = plan(example.trim_start()).unwrap();
        assert_eq!(p.routes, ["POST /posts/:id/like"]);
    }

    /// **Guards:** the editor says a run's notes (`skipped`) in explain's words:
    /// `editor/src/skipped.fixture.json` is [`skipped_why`] over a table of notes (each level; a
    /// read by its key, of a listing, with no request kept, of state; as far as the record sees; a
    /// step with no node), and the editor's `heldEffects` is held to it (replay.test.ts). A
    /// failure's reason is read out of the run's recorded steps on both sides, as explain reads it
    /// ([`recorded`]): an engine's own reason, a body's syntax error and a frame's exception (the
    /// engine's `{ error }` answer, its position said as the body's, its first line only), an
    /// earlier round's, an answer that isn't the engine's, and none.
    /// **Why:** explain and the editor say one note in two places: written twice, they drift. They
    /// did: the editor showed an engine's error whole, Boa's position and further lines in, where
    /// explain says "the body's line L, col C" (the coordinator's reading of R99 stage 5).
    #[test]
    fn the_editor_says_a_runs_notes_in_explains_words() {
        let step = |name: &str, params: Value| -> crate::canvas::Node {
            serde_json::from_value(json!({ "id": "from", "data": { "name": name, "body": { "kind": "Primitive" }, "params": params } })).unwrap()
        };
        let frozen = |name: &str, value: &str| json!({ "kind": "Frozen", "name": name, "value": value });
        let text = |v: &str| json!({ "type": "Text", "value": v });
        let entry = json!({ "reads": [
            { "request": { "type": "Record", "value": [["collection", text("tasks")], ["get", text("t1")]] } },
            { "request": { "type": "Record", "value": [["collection", text("tasks")]] } },
        ] });
        let (tasks, state, field, js) = (
            step("collection_get", json!([frozen("collection", "tasks")])),
            step("store_read", json!([frozen("key", "none")])),
            step("field", json!([frozen("name", "id")])),
            step("js", json!([])),
        );
        let write = "StateWrite(\"g\")";
        // The js node's own step (`[0, 2]`) and its engine's step inside it (`[0, 2, 1]`), in a
        // round, with what they recorded.
        let own = |round: u64| json!({ "path": [0, 2], "start_us": 0, "end_us": 1, "round": round, "outputs": [null] });
        let engine = |round: u64, recorded: Value| {
            let mut step = json!({ "path": [0, 2, 1], "start_us": 0, "end_us": 1, "round": round });
            step.as_object_mut().unwrap().extend(recorded.as_object().unwrap().clone());
            step
        };
        let answered = |fields: Value| json!({ "outputs": [{ "type": "Record", "value": fields }] });
        let failed = json!({ "effect": write, "level": "failed", "at": [0, 3], "from": [0, 2], "leaf": "js" });
        let cases: [(&str, Value, Option<&crate::canvas::Node>, Option<Value>); 16] = [
            ("a read, by its key", json!({ "effect": write, "level": "read", "at": [0, 3], "from": [0, 1], "leaf": "collection_get", "read": 0 }), Some(&tasks), None),
            ("a read of a listing", json!({ "effect": write, "level": "read", "at": [0, 3], "from": [0, 1], "leaf": "collection_get", "read": 1 }), Some(&tasks), None),
            ("a read whose request the record doesn't keep", json!({ "effect": write, "level": "read", "at": [0, 3], "from": [0, 1], "leaf": "collection_get", "read": 7 }), Some(&tasks), None),
            ("a state read (not one of the entry's reads)", json!({ "effect": write, "level": "read", "at": [0, 3], "from": [0, 2], "leaf": "store_read" }), Some(&state), None),
            ("a value lacking a field", json!({ "effect": write, "level": "missing", "at": [0, 3], "from": [0, 2], "leaf": "field" }), Some(&field), None),
            ("as far as the record sees", json!({ "effect": write, "level": "missing", "at": [0, 3], "from": [0, 2, 1], "leaf": "field", "unseen": true }), Some(&field), None),
            ("a failure, the engine's own reason", failed.clone(), Some(&js), Some(json!([own(0), engine(0, json!({ "reason": "out of fuel" }))]))),
            (
                "a failure, a body's syntax error: its position as the body's, its first line",
                failed.clone(),
                Some(&js),
                Some(json!([own(0), engine(0, answered(json!([["error", text("SyntaxError: formal parameter `at` declared in lexically declared names at line 1, col 2 (native)\n    and a line the engine went on with")]])))])),
            ),
            (
                "a failure, a body's exception in a frame, in the run's last round",
                failed.clone(),
                Some(&js),
                Some(json!([engine(0, json!({ "reason": "an earlier round's" })), own(1), engine(1, answered(json!([["error", text("Error: not more than 0 (unknown at :3:11)")], ["value", null]])))])),
            ),
            ("a failure, an answer that isn't the engine's", failed.clone(), Some(&js), Some(json!([own(0), engine(0, answered(json!([["error", text("refused")], ["status", { "type": "Number", "value": 0 }]])))]))),
            ("a failure, the engine's answer with a value and no error", failed.clone(), Some(&js), Some(json!([own(0), engine(0, answered(json!([["value", text("ok")]])))]))),
            ("a failure, no reason recorded", failed.clone(), Some(&js), None),
            ("withheld", json!({ "effect": write, "level": "withheld", "at": [0, 3] }), None, None),
            ("refused", json!({ "effect": "DbWrite(\"shop/orders\")", "level": "refused", "rule": "a value read from a reply that sent a secret back isn't written" }), None, None),
            ("a step with no node of the text", json!({ "effect": write, "level": "read", "at": [0, 3], "from": [0, 9], "leaf": "collection_get", "read": 0 }), None, None),
            ("a step with no node and no leaf", json!({ "effect": write, "level": "missing", "at": [0, 3] }), None, None),
        ];
        // The run as explain reads it: a request's, placed by its route's trace, whose second and
        // third steps are the note's `from` (the notes stand at `[0, 1]` and `[0, 2]`).
        let document = json!({ "trace": { "POST /r": ["a", "from", "from"] } });
        let fixture: Vec<Value> = cases
            .iter()
            .map(|(what, note, from, timeline)| {
                let mut run = entry.clone();
                run["kind"] = json!("request");
                run["route"] = json!("POST /r");
                run["timeline"] = timeline.clone().unwrap_or(json!([]));
                let reasons = recorded(&document, &run).reasons;
                json!({
                    "what": what,
                    "note": note,
                    "from": from.map(|n| json!({ "name": n.data.name, "params": n.data.params })),
                    "timeline": timeline,
                    "says": skipped_why(note, *from, &run, &reasons),
                })
            })
            .collect();
        let text = serde_json::to_string_pretty(&json!({ "reads": entry["reads"], "trace": document["trace"], "cases": fixture })).unwrap() + "\n";
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editor/src/skipped.fixture.json");
        let kept = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        if kept != text && std::env::var_os("HOLON_BLESS").is_some() {
            std::fs::write(&path, &text).unwrap();
            return;
        }
        assert!(kept == text, "{} is out of date: HOLON_BLESS=1 cargo test -p holon-author the_editor_says_a_runs_notes_in_explains_words writes it again", path.display());
    }
}
