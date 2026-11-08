//! The flow commands of `holon`: flows as text files (the language, lang/, in TypeScript's
//! syntax, `NAME.holon.ts`), checked and shown on any machine, a screen or not, through the same
//! code as the editor. Each takes the file's name and text and answers what to print (and, for
//! `fmt`, the file's new text); the binary reads and writes files.
//!
//!   holon new <file> [--sample <name>]   a starter flow (or a sample)
//!   holon check <file>                   compile it: its routes, what it may do, or each mistake
//!   holon show <file>                    an outline: routes, nodes and what feeds them
//!   holon fmt <file>                     rewrite it in the canonical form
//!
//! A mistake is printed as `<file>:<line>:<col>: <message>`, and the exit code is 1.

use holon_core::{consent_name, MapResolver, Resolver};

use crate::composites::Names;
use crate::describe::{describe_item, describe_item_sending};
use crate::flow::{derive_server, flow_unit, DerivedServer};
use crate::lang::{self, Lowered};
use crate::text;
use crate::wire::WParam;
use crate::{samples, to_hex};

/// What a command prints: `out` to stdout, `err` to stderr, and its exit code.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub out: Vec<String>,
    pub err: Vec<String>,
    pub code: i32,
}

pub const STARTER: &str = r#"// A flow: a route, and what it answers. Run `holon check` on it as you go.
import { flow, route } from "holon"

flow("my-flow", { port: 8200 })

route.get("/hello/:name", (req) => `Hello, ${req.params.name}`)
"#;

/// A new flow's text: the starter, or the sample `sample` (its comments say what it is).
pub fn new_flow(sample: Option<&str>) -> Result<String, String> {
    let Some(name) = sample else { return Ok(STARTER.into()) };
    let sample = samples::sample(name).ok_or_else(|| format!("no sample \"{name}\" (there are {})", sample_names()))?;
    Ok(sample.text.into())
}

/// Every sample's name, for a usage line.
pub fn sample_names() -> String {
    samples::samples().iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
}

/// A mistake as the commands print one: `<file>:<line>:<col>: <message>`, then the line it is
/// on, then a caret under the span.
fn mistake(file: &str, source: &str, e: &lang::LowerError) -> Report {
    let mut err = vec![format!("{file}:{e}")];
    let (start, end) = (e.span.start, e.span.end);
    if let Some(line) = source.lines().nth((start.line as usize).saturating_sub(1)) {
        err.push(format!("  {line}"));
        let col = start.col.max(1) as usize;
        let width = if end.line == start.line && end.col > start.col { (end.col - start.col) as usize } else { 1 };
        err.push(format!("  {}{}", " ".repeat(col - 1), "^".repeat(width)));
    }
    Report { err, code: 1, ..Default::default() }
}

/// The file compiled, its units in `store`; or its mistake, said with its line and a caret.
pub(crate) fn compile(file: &str, source: &str, store: &mut MapResolver) -> Result<Lowered, Report> {
    let compiled = lang::ts::compile(source).map_err(|e| mistake(file, source, &e))?;
    text::restore(&compiled.doc, store, &Names::new());
    Ok(compiled)
}

fn port_of(c: &Lowered) -> &str {
    c.doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or_default()
}

/// Nothing would start a run of it: no route, clock, file or mail handler or action. It is a holon, placed
/// in other flows; deploy refuses it (docs/L4s/triggers-L4.md fork 14).
pub fn starts_nothing(c: &Lowered, store: &mut MapResolver) -> bool {
    let server = derive_server(&c.doc.nodes, &c.doc.edges, store, port_of(c));
    server.endpoints.is_empty() && server.clocks.is_empty() && server.files.is_empty() && server.mails.is_empty() && server.actions.is_empty()
}

/// What `check` says of a flow nothing would start.
pub const STARTS_NOTHING: &str = "nothing would start a run: add a route, a clock, a file or mail handler or an action (an action runs by hand: `export const run = action((input) => { … })`)";

/// What the flow derives to: its server's routes (or none), its unit, what it may do, sorted.
fn derive(c: &Lowered, store: &mut MapResolver) -> (DerivedServer, Result<(String, Vec<String>), String>) {
    let server = derive_server(&c.doc.nodes, &c.doc.edges, store, port_of(c));
    let unit = flow_unit(&c.doc.nodes, &c.doc.edges, store, port_of(c)).map(|hash| {
        let mut caps: Vec<String> = store.resolve(&hash).map(|u| holon_core::deployed_capabilities(u, store).iter().map(|c| consent_name(&c)).collect()).unwrap_or_default();
        caps.sort();
        (to_hex(&hash), caps)
    });
    (server, unit)
}

/// Where each secret the text reads is sent (core's `sent_secrets`, secret → hosts); nothing when
/// it doesn't compile. What the editor's hover on a secret says.
pub(crate) fn sent_of(source: &str) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    let mut store = MapResolver::new();
    let Ok(c) = compile("text", source, &mut store) else {
        return Default::default();
    };
    let (_, unit) = derive(&c, &mut store);
    unit.ok().and_then(|(hash, _)| holon_core::parse_hash(&hash)).and_then(|h| store.resolve(&h)).map(|root| holon_core::sent_secrets(root, &store)).unwrap_or_default()
}

/// What the host does around a run that the manifest doesn't say (`docs/L4s/key-claims-L4.md`
/// §3): a flow that calls out and both reads and writes a collection holds the keys it read in it
/// from its first outbound request until it commits; a table, its rows, and a folder, its files
/// (`docs/L4s/databases-L4.md` §7.2). Derived from the capabilities, as the host derives the
/// claim; nothing to approve, so it is said apart from "it may".
pub(crate) fn while_it_runs(caps: &[String]) -> Vec<String> {
    if !caps.iter().any(|c| c.starts_with("HttpRequest(")) {
        return vec![];
    }
    let named = |kind: &str| -> Vec<String> {
        caps.iter().filter_map(|c| c.strip_prefix(kind)?.strip_prefix("(\"")?.strip_suffix("\")").map(str::to_string)).collect()
    };
    let both = |read: &str, write: &str| -> String {
        let written = named(write);
        let both: Vec<String> = named(read).into_iter().filter(|n| written.contains(n)).map(|n| format!("\"{n}\"")).collect();
        both.join(" and ")
    };
    let mut lines = vec![];
    for (read, write, what, one) in [("CollectionRead", "CollectionWrite", "keys", "key"), ("DbRead", "DbWrite", "rows", "row"), ("FileRead", "FileWrite", "files", "file")] {
        let names = both(read, write);
        if !names.is_empty() {
            lines.push(format!("  holds the {what} it reads in {names} from its first outbound call until it commits, so two runs on one {one} call out once"));
        }
    }
    if lines.is_empty() {
        return lines;
    }
    lines.insert(0, "while it runs:".into());
    lines
}

/// What `check` finds wrong with a text that compiles: its server's mistakes, and why it has no
/// unit.
pub(crate) fn problems(c: &Lowered, store: &mut MapResolver) -> Vec<String> {
    let (server, unit) = derive(c, store);
    let mut problems = server.errors;
    problems.extend(unit.err());
    let mut seen = std::collections::BTreeSet::new();
    problems.retain(|p| seen.insert(p.clone()));
    problems
}

/// `holon check`: compiled, its routes and clocks, its unit and what it may do; each problem on
/// stderr.
pub fn check(file: &str, source: &str) -> Report {
    let mut store = MapResolver::new();
    let c = match compile(file, source, &mut store) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let (server, unit) = derive(&c, &mut store);
    let mut out = vec![format!("{}: {} nodes, {} wires", c.name.as_deref().unwrap_or(file), c.doc.nodes.len(), c.doc.edges.len())];
    if !server.endpoints.is_empty() {
        let port = port_of(&c);
        out.push(format!("a server on :{}", if port.is_empty() { "?" } else { port }));
        out.extend(server.endpoints.iter().map(|e| format!("  {} {}", e.method, e.path)));
    }
    if !server.clocks.is_empty() {
        out.push(if server.endpoints.is_empty() { "a server of clocks, listening on no port" } else { "and its clocks" }.into());
        out.extend(server.clocks.iter().map(|c| format!("  {}", c.label)));
    }
    if !server.files.is_empty() {
        let alone = server.endpoints.is_empty() && server.clocks.is_empty();
        out.push(if alone { "a server of file handlers, listening on no port" } else { "and its file handlers" }.into());
        out.extend(server.files.iter().map(|f| format!("  {}", f.label)));
    }
    if !server.mails.is_empty() {
        let alone = server.endpoints.is_empty() && server.clocks.is_empty() && server.files.is_empty();
        out.push(if alone { "a server of mail handlers, listening on no port" } else { "and its mail handlers" }.into());
        out.extend(server.mails.iter().map(|m| format!("  {}", m.label)));
    }
    if !server.actions.is_empty() {
        let alone = server.endpoints.is_empty() && server.clocks.is_empty() && server.files.is_empty() && server.mails.is_empty();
        out.push(if alone { "a server of actions, run by hand, listening on no port" } else { "and its actions, run by hand" }.into());
        out.extend(server.actions.iter().map(|a| format!("  {}", a.label)));
    }
    let (caps, modules, sent, mut problems) = match unit {
        Ok((hash, caps)) => {
            out.push(format!("unit {hash}"));
            let root = holon_core::parse_hash(&hash).and_then(|h| store.resolve(&h));
            // A module (a js node) computes only, so it adds no capability; what it may consume
            // per call is a limit the deploy asks for, said here as the approval will say it.
            let modules = root.map(|root| holon_core::module_limits_of(root, &store)).unwrap_or_default();
            // Where each secret goes, said beside it as the approval says it.
            let sent = root.map(|root| holon_core::sent_secrets(root, &store)).unwrap_or_default();
            (caps, modules, sent, vec![])
        }
        Err(e) => (vec![], Default::default(), Default::default(), vec![e]),
    };
    out.push(if caps.is_empty() && modules.is_empty() { "it only computes: it reads and changes nothing outside" } else { "it may:" }.into());
    out.extend(caps.iter().map(|c| format!("  {}", describe_item_sending(c, &sent))));
    if let (Some(fuel), Some(memory)) = (modules.get(holon_core::FUEL_LIMIT), modules.get(holon_core::MEMORY_LIMIT)) {
        out.push(format!("  run code it carries (a js node): up to {fuel} instructions and {} MiB per call, reaching nothing outside", memory / (1024 * 1024)));
    }
    // Its unrecorded handlers, each asked for at deploy, said as the approval says it (C5).
    let unrecorded = server.endpoints.iter().map(|e| (format!("{} {}", e.method, e.path), &e.log)).chain(server.clocks.iter().map(|c| (c.label.clone(), &c.log))).chain(server.files.iter().map(|f| (f.label.clone(), &f.log))).chain(server.actions.iter().map(|a| (a.label.clone(), &a.log)));
    let mut unrecorded: Vec<String> = unrecorded.filter(|(_, log)| log.as_str() == "unrecorded").map(|(label, _)| format!("  {}", describe_item(&format!("Unrecorded({label:?})")))).collect();
    unrecorded.sort();
    out.extend(unrecorded);
    out.extend(while_it_runs(&caps));
    if problems.is_empty() && starts_nothing(&c, &mut store) {
        out.push(STARTS_NOTHING.into());
    }
    problems.splice(0..0, server.errors);
    // The flow has no unit because of its routes' errors: each said once.
    let mut seen = std::collections::BTreeSet::new();
    problems.retain(|p| seen.insert(p.clone()));
    let mut err: Vec<String> = problems.iter().map(|p| format!("{file}: {p}")).collect();
    err.extend(js_mistakes(file, source));
    let code = if err.is_empty() { 0 } else { 1 };
    Report { out, err, code }
}

/// Each js body compiled once by the engine, without running it (docs/L4s/js-node-L4.md §7): a
/// mistake said at its line in the file, with the line and a caret, as any other mistake is. The
/// engine counts lines in the source it was given, whose line 1 is the statement's header line
/// (the body follows the `{` there), so a line of the body is that many lines down the file.
fn js_mistakes(file: &str, source: &str) -> Vec<String> {
    let Ok(parsed) = lang::ts::parse(source) else { return vec![] };
    let mut err = Vec::new();
    for item in &parsed.items {
        let lang::ast::Item::Js(def) = item else { continue };
        let Err(message) = holon_module::compile_js(&lang::lower::js_source(def)) else { continue };
        let prefix = lang::lower::js_source(def).find(") => {").map_or(0, |i| i as u32 + ") => {".len() as u32);
        let (line, col) = match js_position(&message) {
            Some((l, c)) if l <= 1 => (def.body_span.start.line, def.body_span.start.col + c.saturating_sub(prefix)),
            Some((l, c)) => (def.body_span.start.line + l - 1, c),
            None => (def.name.span.start.line, def.name.span.start.col),
        };
        // The engine's own position (in the source it was given) goes: the file's is said instead.
        let message = message.split(" at line ").next().unwrap_or(&message).split(" (unknown at ").next().unwrap_or(&message).trim_end_matches(" (native)");
        err.push(format!("{file}:{line}:{col}: js \"{}\": {message}", def.name.text));
        if let Some(text) = source.lines().nth(line.saturating_sub(1) as usize) {
            err.push(format!("  {text}"));
            err.push(format!("  {}^", " ".repeat(col.max(1) as usize - 1)));
        }
    }
    err
}

/// Where the engine places a mistake in the source it compiled: `… at line L, col C` (a syntax
/// error) or `(… at :L:C)` (a runtime error's frame).
fn js_position(message: &str) -> Option<(u32, u32)> {
    let rest = message.split(" at line ").nth(1).or_else(|| message.split(" at :").nth(1))?;
    let mut numbers = rest.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty());
    Some((numbers.next()?.parse().ok()?, numbers.next()?.parse().ok()?))
}

/// `holon show`: an outline, each node by its name in the text, with its params and what feeds it.
pub fn show(file: &str, source: &str) -> Report {
    let mut store = MapResolver::new();
    let c = match compile(file, source, &mut store) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let (server, unit) = derive(&c, &mut store);
    let port = port_of(&c);
    let mut out = vec![format!("{}{}", c.name.as_deref().unwrap_or(file), if port.is_empty() { String::new() } else { format!("  (a server on :{port})") })];
    if !server.endpoints.is_empty() {
        out.push("  routes".into());
        out.extend(server.endpoints.iter().map(|e| format!("    {} {}", e.method, e.path)));
    }
    if !server.clocks.is_empty() {
        out.push("  clocks".into());
        out.extend(server.clocks.iter().map(|c| format!("    {}", c.label)));
    }
    if !server.files.is_empty() {
        out.push("  files".into());
        out.extend(server.files.iter().map(|f| format!("    {}", f.label)));
    }
    if !server.mails.is_empty() {
        out.push("  mails".into());
        out.extend(server.mails.iter().map(|m| format!("    {}", m.label)));
    }
    if !server.actions.is_empty() {
        out.push("  actions".into());
        out.extend(server.actions.iter().map(|a| format!("    {}", a.label)));
    }
    if let Ok((_, caps)) = &unit {
        out.extend(while_it_runs(caps).iter().map(|l| format!("  {l}")));
    }
    out.push("  nodes".into());
    let name_of = |id: &str| c.names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let width = c.doc.nodes.iter().map(|n| name_of(&n.id).chars().count()).max().unwrap_or(0).max(4);
    for n in &c.doc.nodes {
        let params: Vec<String> = n
            .data
            .params
            .iter()
            .filter_map(|p| match p {
                WParam::Frozen { name, value } if !value.is_empty() => Some(format!("{name}={}", serde_json::to_string(value).unwrap_or_default())),
                _ => None,
            })
            .collect();
        let params = if params.is_empty() { String::new() } else { format!("  {}", params.join(" ")) };
        out.push(format!("    {:<width$}  {}{params}", name_of(&n.id), n.data.name));
        for e in c.doc.edges.iter().filter(|e| e.target == n.id) {
            out.push(format!("    {:<width$}    {} ← {}.{}", "", e.input(), name_of(&e.source), e.output()));
        }
    }
    Report { out, ..Default::default() }
}

/// `holon fmt`: the file's text in the canonical form, its comments kept, or its mistake.
pub fn fmt(file: &str, source: &str) -> Result<String, Report> {
    lang::format::format(source).map_err(|e| mistake(file, source, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = "import { flow, route } from \"holon\"\n\nflow(\"hello-api\", { port: 8101 })\n\nroute.get(\"/hello/:name\", (req) => {\n  const name = req.params.name\n  const hi = \"Hello, \"\n  const greet = `${hi}${name}`\n  return greet\n})\n";

    /// A js body is JavaScript, laid out as such: fmt keeps a `#` in it (a colour, a private
    /// field) and its comments, and the comments around it.
    #[test]
    fn fmt_keeps_a_hash_inside_a_js_body() {
        let src = "import { flow, js, route } from \"holon\"\n\nflow(\"f\", { port: 8203 })\n\n// the page\nconst page = js(function html(x) {\n  return `<style>a{color:#777}</style>${x}` // #not a holon comment\n})\n\nroute.get(\"/p\", (req) => {\n  return page(req.query.x ?? \"y\")\n  // answered\n})\n";
        let out = fmt("f.holon.ts", src).unwrap();
        assert_eq!(out, src, "{out}");
        assert_eq!(fmt("f.holon.ts", &out).unwrap(), out, "and again");
    }

    /// Each js body is compiled once by the engine: a mistake TypeScript's syntax lets through
    /// (`import.meta` outside a module) is a mistake at its line in the file, with the line and a caret like
    /// any other; a body that compiles passes, whatever it would do at run time (that is the
    /// run's to say, and explain's).
    #[test]
    fn check_compiles_each_js_body_and_says_a_mistake_at_its_line() {
        let src = "import { flow, js, route } from \"holon\"\n\nflow(\"f\", { port: 8202 })\n\nconst bad = js(function out(a) {\n  const x = a\n  return import.meta.url\n})\n\nroute.get(\"/x\", () => bad(1))\n";
        let r = check("f.holon.ts", src);
        assert_eq!(r.code, 1);
        assert!(r.err[0].starts_with("f.holon.ts:7:"), "{:?}", r.err);
        assert!(r.err[0].contains(": js \"bad\": SyntaxError: "), "{:?}", r.err);
        assert_eq!(r.err[1], "    return import.meta.url");
        assert_eq!(r.err[2], format!("  {}^", " ".repeat(9)));
        assert!(r.out.iter().any(|l| l.starts_with("unit ")), "it still compiles and derives: {:?}", r.out);
        let fine = check("f.holon.ts", &src.replace("import.meta.url", "x"));
        assert_eq!((fine.code, fine.err), (0, vec![]));
        // On the function's own line, the column is the file's.
        let one_line = check("f.holon.ts", "import { flow, js, route } from \"holon\"\n\nflow(\"f\", { port: 8202 })\n\nconst bad = js(function out(a) { return import.meta })\n\nroute.get(\"/x\", () => bad(1))\n");
        assert!(one_line.err[0].starts_with("f.holon.ts:5:"), "{:?}", one_line.err);
        assert_eq!(js_position("SyntaxError: x at line 2, col 13 (native)"), Some((2, 13)));
        assert_eq!(js_position("TypeError: y (unknown at :3:9)\n    at <main> (unknown at :3:9)"), Some((3, 9)));
        assert_eq!(js_position("the source isn't a function"), None);
    }

    /// A flow's clocks (its `every` blocks) are listed after its routes, and a flow of clocks alone
    /// says it listens on no port; `show` lists them too.
    #[test]
    fn check_and_show_list_its_clocks() {
        let r = check("t.holon.ts", "import { every, flow, route, status } from \"holon\"\nimport state from \"state\"\n\nflow(\"t\", { port: 8080 })\n\nevery(\"2s\", () => {\n  state.n = 1\n})\n\nroute.get(\"/\", () => status(1))\n");
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(r.out[1..5], ["a server on :8080", "  GET /", "and its clocks", "  every 2s"]);
        let r = check("b.holon.ts", "import { every, flow } from \"holon\"\nimport state from \"state\"\nimport { now } from \"holon:clock\"\n\nflow(\"b\")\n\nevery(\"1h\", () => {\n  state.n = now\n})\n");
        assert_eq!(r.out[1..3], ["a server of clocks, listening on no port", "  every 1h"]);
        assert!(show("b.holon.ts", "import { every, flow } from \"holon\"\nimport state from \"state\"\nimport { now } from \"holon:clock\"\n\nflow(\"b\")\n\nevery(\"1h\", () => {\n  state.n = now\n})\n").out[1..3] == ["  clocks", "    every 1h"]);
    }

    #[test]
    fn check_says_routes_unit_and_what_it_may_do() {
        let r = check("hello.holon.ts", HELLO);
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(r.out[0], "hello-api: 7 nodes, 7 wires");
        assert_eq!(r.out[1..3], ["a server on :8101", "  GET /hello/:name"]);
        assert!(r.out[3].starts_with("unit "));
        assert_eq!(r.out[4..], ["it may:", "  receive requests"]);
    }

    /// A route that answers with a secret may do that, said apart from reading it; one that only
    /// signs with a secret doesn't answer with it.
    #[test]
    fn check_says_a_route_answers_with_a_secret() {
        let text = "import { base64, flow, hmac_sha256, route, status } from \"holon\"\nimport key from \"secret:key\"\nimport token from \"secret:token\"\n\nflow(\"leaky\", { port: 8102 })\n\nroute.get(\"/plain\", () => base64(token))\n\nroute.post(\"/sign\", (req) => hmac_sha256(key, req.body) ?? status(400))\n";
        let r = check("leaky.holon.ts", text);
        assert_eq!(r.code, 0, "{r:?}");
        let may = r.out.iter().position(|l| l == "it may:").unwrap_or_else(|| panic!("{:?}", r.out));
        assert_eq!(
            r.out[may + 1..],
            ["  receive requests", "  answer its callers with the secret \"token\", or with what is computed from it", "  read the secret \"key\"", "  read the secret \"token\""]
        );
    }

    /// The canvas marks every sink a secret reaches, by node: the request it is sent in, the state
    /// write the host refuses, the route's answer; a node that only computes with it (stripe's
    /// verify, an hmac inside) isn't marked.
    #[test]
    fn the_canvas_marks_each_sink_a_secret_reaches() {
        let text = "import { flow, route } from \"holon\"\nimport state from \"state\"\nimport k from \"secret:k\"\nimport example from \"host:api.example.com\"\n\nflow(\"marks\", { port: 8199 })\n\nroute.get(\"/x\", async () => {\n  const sent = await example.get(\"/v1\", { headers: { authorization: `Bearer ${k}` } })\n  state.kept = k\n  return { key: k, status: sent.status }\n})\n";
        let marks_of = |text: &str| {
            let mut store = MapResolver::new();
            let c = compile("t.holon.ts", text, &mut store).unwrap_or_else(|r| panic!("{r:?}"));
            let marks = crate::flow::secret_marks(&c.doc.nodes, &c.doc.edges, &mut store, port_of(&c));
            let kind = |id: &str| c.doc.nodes.iter().find(|n| n.id == id).map(|n| n.data.name.clone()).unwrap_or_default();
            marks.into_iter().map(|(id, lines)| (c.names.get(&id).cloned().unwrap_or_else(|| kind(&id)), lines)).collect::<std::collections::BTreeMap<String, Vec<String>>>()
        };
        let marks = marks_of(text);
        assert_eq!(marks.get("sent"), Some(&vec!["sends the secret \"k\" to https://api.example.com".to_string()]), "{marks:?}");
        assert_eq!(marks.get("store_write"), Some(&vec!["would keep the secret \"k\" (write its state \"kept\"): the host refuses it".to_string()]), "{marks:?}");
        assert_eq!(marks.get("respond"), Some(&vec!["answers its caller with the secret \"k\" (approved apart, at deploy)".to_string()]), "{marks:?}");
        assert_eq!(marks.len(), 3, "nothing else: {marks:?}");

        // Its request is inside toSlack, so the mark is on the function's node (`posted`).
        let stripe = marks_of(include_str!("../samples/stripe-to-slack.holon.ts"));
        assert_eq!(stripe.get("posted"), Some(&vec!["sends the secret \"slack_webhook_path\" to https://hooks.slack.com".to_string()]), "{stripe:?}");
        assert_eq!(stripe.len(), 1, "verifying with the Stripe secret marks nothing: {stripe:?}");

        // A token exchange: the secret to the OAuth server, what is answered to it to the API.
        let exchange = marks_of(include_str!("../samples/client-credentials.holon.ts"));
        assert_eq!(exchange.get("got"), Some(&vec!["sends the secret \"client_secret\" to https://demo.duendesoftware.com".to_string()]), "{exchange:?}");
        assert_eq!(exchange.get("called"), Some(&vec!["sends what is answered to the secret \"client_secret\" to https://httpbin.dev".to_string()]), "{exchange:?}");
        assert_eq!(exchange.len(), 2, "{exchange:?}");
        let echoed = marks_of(include_str!("../samples/echoed-key.holon.ts"));
        assert_eq!(echoed.get("passed"), Some(&vec!["sends what is answered to the secret \"demo_key\" to https://postman-echo.com".to_string()]), "{echoed:?}");

        // A database read the key is a parameter of: the key goes to the database.
        let license = marks_of(include_str!("../samples/license-check.holon.ts"));
        assert_eq!(license.into_values().collect::<Vec<_>>(), vec![vec!["sends the secret \"license_key\" to the database \"vendor\", reading its table \"licenses\"".to_string()]]);
    }

    /// Beside each secret, where the flow sends it: stripe-to-slack posts its Slack path to Slack,
    /// and only verifies a signature with the Stripe secret, which sends it nowhere.
    #[test]
    fn check_says_where_each_secret_is_sent() {
        let r = check("stripe-to-slack.holon.ts", include_str!("../samples/stripe-to-slack.holon.ts"));
        assert_eq!(r.code, 0, "{r:?}");
        assert!(r.out.iter().any(|l| l == "  read the secret \"slack_webhook_path\", and send it to https://hooks.slack.com"), "{:?}", r.out);
        assert!(r.out.iter().any(|l| l == "  read the secret \"stripe_webhook_secret\""), "{:?}", r.out);
    }

    /// A flow that calls out and both reads and writes a collection is told, apart from what it
    /// may do, that the host holds the keys it read from its first call until it commits
    /// (docs/L4s/key-claims-L4.md §3, test 8); one that doesn't call out, or only reads, isn't.
    #[test]
    fn check_says_a_flow_on_a_schedule_runs_on_the_clock() {
        // `every` starts its runs, so it reads the clock as deployed, though nothing reads `now`.
        let r = check("beats.holon.ts", "import { every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"beats\")\n\nevery(\"5s\", () => {\n  state.beats = (state.beats ?? 0) + 1\n})\n");
        assert_eq!(r.code, 0, "{r:?}");
        let may = r.out.iter().position(|l| l == "it may:").unwrap_or_else(|| panic!("{:?}", r.out));
        assert_eq!(r.out[may + 1..], ["  read the clock", "  read its state \"beats\"", "  write its state \"beats\""]);
    }

    #[test]
    fn check_names_the_held_keys() {
        let r = check("cache-aside.holon.ts", crate::samples::sample("cache-aside").unwrap().text);
        assert_eq!(r.code, 0, "{r:?}");
        let at = r.out.iter().position(|l| l == "while it runs:").unwrap_or_else(|| panic!("{:?}", r.out));
        assert_eq!(r.out[at + 1], "  holds the keys it reads in \"ids\" from its first outbound call until it commits, so two runs on one key call out once");
        assert!(r.out[..at].iter().any(|l| l.starts_with("  call https://")), "said after what it may do: {:?}", r.out);
        let shown = show("cache-aside.holon.ts", crate::samples::sample("cache-aside").unwrap().text);
        assert!(shown.out.iter().any(|l| l == "  while it runs:"), "{:?}", shown.out);
        let r = check("refund-once.holon.ts", crate::samples::sample("refund-once").unwrap().text);
        let at = r.out.iter().position(|l| l == "while it runs:").unwrap_or_else(|| panic!("{:?}", r.out));
        assert_eq!(
            r.out[at + 1],
            "  holds the rows it reads in \"shop/orders\" and \"shop/refunds\" from its first outbound call until it commits, so two runs on one row call out once"
        );
        for name in ["likes", "basket", "fetch-json", "orders-api"] {
            let r = check(name, crate::samples::sample(name).unwrap().text);
            assert!(!r.out.iter().any(|l| l.contains("while it runs")), "{name}: {:?}", r.out);
        }
    }

    const BOARD: &str = "import { action, flow, route } from \"holon\"\nimport notes from \"collection:notes\"\n\nflow(\"status-board\", { port: 8170 })\n\n// Answers the load balancer.\nroute.get(\"/health\", () => \"ok\", { log: \"unrecorded\" })\n\nroute.get(\"/notes\", () => notes.list({ limit: 50 }), { log: \"lazy\" })\n\nroute.post(\"/notes\", (req) => {\n  notes[req.json.id] = req.json.text\n  return \"saved\"\n})\n\nexport const report = action(() => notes.list({ limit: 50 }), { log: \"unrecorded\" })\n";

    /// `lazy` and `unrecorded` after a handler's head (docs/L4s/request-path-L4.md, C5): fmt
    /// writes them back where they were, they are names everywhere else, and any handler may have
    /// them, whatever it does: the level is what its author writes.
    #[test]
    fn a_handlers_log_word_is_kept_as_written() {
        assert_eq!(fmt("b.holon.ts", BOARD).unwrap(), BOARD);
        let r = check("b.holon.ts", BOARD);
        assert_eq!((r.code, r.err.clone()), (0, vec![]), "{r:?}");
        // What the approval will ask, said as it says it.
        for label in ["GET /health", "action report"] {
            assert!(r.out.contains(&format!("  keep runs of {label} out of its log, failed or not (they are counted)")), "{:?}", r.out);
        }
        // A word that isn't before a block is a name.
        let names = "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"n\")\n\nexport const a = action((unrecorded) => {\n  state.lazy = unrecorded\n  return state.lazy\n})\n";
        assert_eq!(fmt("n.holon.ts", names).unwrap(), names);
        assert_eq!(check("n.holon.ts", names).code, 0);
        // The word changes the handler's unit: each level is its own.
        let unit = |text: &str| check("u.holon.ts", text).out.into_iter().find(|l| l.starts_with("unit ")).unwrap();
        let strict = "import { flow, route, status } from \"holon\"\n\nflow(\"u\", { port: 8172 })\n\nroute.get(\"/x\", () => status(1))\n";
        let lazy = strict.replace("status(1))", "status(1), { log: \"lazy\" })");
        assert_ne!(unit(strict), unit(&lazy));
        assert_ne!(unit(&lazy), unit(&strict.replace("status(1))", "status(1), { log: \"unrecorded\" })")));
        // A handler is as its author writes it, whatever it does: a route that writes compiles
        // unrecorded, a clock that writes its state lazy, an action that calls out lazy, a file
        // handler that writes unrecorded; and fmt keeps the word on every kind of handler.
        let r = check("b.holon.ts", &BOARD.replace("  return \"saved\"
})", "  return \"saved\"
}, { log: \"unrecorded\" })"));
        assert_eq!((r.code, r.err.clone()), (0, vec![]), "{r:?}");
        let r = check("t.holon.ts", "import { every, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"t\")\n\nevery(\"1m\", () => {\n  state.n = (state.n ?? 0) + 1\n}, { log: \"lazy\" })\n");
        assert_eq!((r.code, r.err.clone()), (0, vec![]), "{r:?}");
        let r = check("o.holon.ts", "import { action, flow } from \"holon\"\nimport httpbin from \"host:httpbin.org\"\n\nflow(\"o\")\n\nexport const ping = action(async () => (await httpbin.get(\"/uuid\")).body, { log: \"lazy\" })\n");
        assert_eq!((r.code, r.err.clone()), (0, vec![]), "{r:?}");
        let files = "import { every, flow, on } from \"holon\"\nimport seen from \"collection:seen\"\nimport inbox from \"folder:inbox\"\nimport { now } from \"holon:clock\"\n\nflow(\"f\")\n\non.file.created(\n  inbox,\n  (f) => {\n    seen[f.name] = f.size\n  },\n  { log: \"unrecorded\" },\n)\n\nevery(\n  \"1m\",\n  () => {\n    seen[\"tick\"] = now\n  },\n  { log: \"lazy\" },\n)\n";
        assert_eq!(fmt("f.holon.ts", files).unwrap(), files);
        let r = check("f.holon.ts", files);
        assert_eq!((r.code, r.err.clone()), (0, vec![]), "{r:?}");
    }

    #[test]
    fn a_mistake_names_its_file_and_line() {
        let r = check("bad.holon.ts", "import { concat, flow, route } from \"holon\"\n\nflow(\"bad\", { port: 8101 })\n\nroute.get(\"/a\", (req) => concat(req.body, nope))\n");
        assert_eq!(r.code, 1);
        assert_eq!(r.err, [r#"bad.holon.ts:5:43: unknown name "nope""#, r#"  route.get("/a", (req) => concat(req.body, nope))"#, "                                            ^^^^"]);
        // A flow that compiles but can't be a server: its route says why.
        let r = check("half.holon.ts", "import { flow, route } from \"holon\"\n\nflow(\"half\", { port: 8101 })\n\nroute.get(\"/x/:a\", () => 1)\n\nroute.get(\"/x/:b\", () => 2)\n");
        assert_eq!(r.code, 1);
        assert_eq!(r.err, ["half.holon.ts: GET /x/:b clashes with GET /x/:a"]);
    }

    /// Many strings in one route: each `{…}` is an `as_text`, all the same unit, told apart by
    /// where their values go. A value computed twice and never used is said, once.
    #[test]
    fn a_route_of_many_interpolations_compiles_and_unused_twins_are_said() {
        let parts: Vec<String> = (0..12).map(|i| format!("{{req.json.f{i}}}")).collect();
        let text = format!(
            "import {{ flow, route }} from \"holon\"\n\nflow(\"links\", {{ port: 8106 }})\n\nroute.post(\"/links\", () => {{\n  const a = \"{}\"\n  const b = \"{}/{}\"\n  return {{ a, b }}\n}})\n",
            parts.join("/"),
            parts[..6].join("/"),
            parts[6..].join("-")
        );
        let r = check("links.holon.ts", &text);
        assert_eq!(r.code, 0, "{r:?}");
        let r = check("twins.holon.ts", "import { flow, route } from \"holon\"\n\nflow(\"t\", { port: 8107 })\n\nroute.post(\"/t\", (req) => {\n  const x = req.json.a.toLowerCase()\n  const y = req.json.a.toLowerCase()\n  return \"ok\"\n})\n");
        assert_eq!(r.code, 1);
        assert_eq!(r.err, ["twins.holon.ts: POST /t: the same value is computed twice and never used, so nothing tells the two apart: remove one, or use it"]);
    }

    #[test]
    fn show_outlines_each_node_and_what_feeds_it() {
        let r = show("hello.holon.ts", HELLO);
        assert_eq!(r.out[..4], ["hello-api  (a server on :8101)", "  routes", "    GET /hello/:name", "  nodes"]);
        assert!(r.out.iter().any(|l| l.split_whitespace().eq(["greet", "concat"])), "{:#?}", r.out);
        assert!(r.out.iter().any(|l| l.trim() == "record ← req.params"), "{:#?}", r.out);
        assert!(r.out.iter().any(|l| l.split_whitespace().eq(["hi", "const", r#"value="Hello,"#, r#"""#, r#"type="Text""#])), "{:#?}", r.out);
    }

    #[test]
    fn fmt_writes_folders_back_as_written() {
        let text = "import { flow, route } from \"holon\"\nimport inbox from \"folder:inbox\"\nimport out from \"folder:out\"\n\nflow(\"files\", { port: 8200 })\n\nroute.post(\"/move\", (req) => {\n  const t = inbox[req.json.name]\n  out[`${req.json.name}.md`] = `# ${t}`\n  delete out[`${req.json.name}.old`]\n  return inbox.list({ prefix: \"2026/\", limit: 5 })\n})\n";
        assert_eq!(check("files.holon.ts", text).code, 0);
        assert_eq!(fmt("files.holon.ts", text).unwrap(), text);
    }

    #[test]
    fn fmt_writes_channels_back_as_written() {
        let text = "import { flow, respond, route, status } from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"board\", { port: 8200 })\n\nroute.post(\"/t\", (req) => {\n  updates.send({ new: req.json.title })\n  return status(201, { ok: true })\n})\n\nroute.get(\"/live\", () => updates.subscribe())\n\nroute.get(\"/all\", () =>\n  respond(updates.subscribe({ from: \"kept\" }), { headers: { \"cache-control\": \"no-store\" } }),\n)\n";
        assert_eq!(check("board.holon.ts", text).code, 0, "{:?}", check("board.holon.ts", text).err);
        assert_eq!(fmt("board.holon.ts", text).unwrap(), text);
    }

    #[test]
    fn a_mail_is_written_back_as_written_its_recipients_in_its_grant() {
        let text = "import { flow, route, status } from \"holon\"\nimport ops from \"mail:ops\"\n\nflow(\"payouts\", { port: 8200 })\n\nroute.post(\"/ask\", (req) => {\n  ops.send({\n    to: \"approvals@acme.com\",\n    cc: [\"audit@acme.com\", \"Approvals@acme.com\"],\n    subject: `Payout ${req.json.id}`,\n    text: \"Approve it\",\n    attachments: { \"payout.csv\": req.text },\n  })\n  return status(202, { ok: true })\n})\n\nroute.post(\"/tell\", (req) => {\n  ops.send({ to: req.json.who, subject: \"Approved\", text: \"It went out\" })\n  return status(202, { ok: true })\n})\n";
        let checked = check("payouts.holon.ts", text);
        assert_eq!(checked.code, 0, "{:?}", checked.err);
        assert_eq!(fmt("payouts.holon.ts", text).unwrap(), text);
        // Each recipient written in quotes, once; a computed one is the binding's to hold.
        let may: Vec<&String> = checked.out.iter().filter(|l| l.contains("send mail")).collect();
        assert_eq!(
            may,
            [
                "  send mail through \"ops\" to an address it computes, within its binding's",
                "  send mail through \"ops\" to approvals@acme.com",
                "  send mail through \"ops\" to audit@acme.com",
            ]
        );
        let flow = |body: &str| format!("import {{ flow, route, status }} from \"holon\"\nimport ops from \"mail:ops\"\n\nflow(\"payouts\", {{ port: 8200 }})\n\nroute.post(\"/ask\", (req) => {{\n{body}\n  return status(202, {{ ok: true }})\n}})\n");
        let fails = |body: &str| check("payouts.holon.ts", &flow(body)).err.join("\n");
        assert!(fails("  ops.send({ to: \"approvals\", subject: \"s\", text: \"t\" })").contains("\"approvals\" isn't an address a mail goes to"));
        assert!(fails("  ops.send({ to: \"a@acme.com\", bcc: \"b@acme.com\", subject: \"s\", text: \"t\" })").contains("a mail has no \"bcc\""));
        assert!(fails("  ops.send({ to: \"a@acme.com\", text: \"t\" })").contains("a mail needs its subject"));
        assert!(fails("  ops.send({ to: [], subject: \"s\", text: \"t\" })").contains("one recipient at least"));
        assert!(fails("  ops.send(req.json, req.json)").contains("ops.send takes one message"));
        assert!(fails("  ops.post(req.json)").contains("a mail has no \"post\""));
        assert!(check("payouts.holon.ts", "import { flow, route } from \"holon\"\nimport billing from \"mail:ops\"\n\nflow(\"p\", { port: 8200 })\n\nroute.get(\"/\", () => 1)\n").err.join("\n").contains("a mail is imported under its own name"));
    }

    #[test]
    fn fmt_writes_a_streamed_reply_back_as_written() {
        let text = "import { as_text, flow, respond, route } from \"holon\"\nimport said from \"collection:said\"\nimport example from \"host:api.example.com\"\nimport { now } from \"holon:clock\"\n\nflow(\"chat\", { port: 8200 })\n\nroute.post(\"/ask\", async (req) => {\n  const sent = await example.post(\"/v1/chat\", { body: req.json, stream: true })\n  said[as_text(now)] = sent.body\n  return respond(sent.body, { stream: true })\n})\n";
        assert_eq!(check("chat.holon.ts", text).code, 0, "{:?}", check("chat.holon.ts", text).err);
        assert_eq!(fmt("chat.holon.ts", text).unwrap(), text);
        // A computed status, written as it was.
        let computed = "import { flow, route, status } from \"holon\"\n\nflow(\"codes\", { port: 8200 })\n\nroute.post(\"/a\", (req) => status(req.json.code, { ok: false }) ?? status(400))\n";
        assert_eq!(check("codes.holon.ts", computed).code, 0, "{:?}", check("codes.holon.ts", computed).err);
        assert_eq!(fmt("codes.holon.ts", computed).unwrap(), computed);
    }

    #[test]
    fn a_streamed_call_is_forwarded_where_its_route_answers() {
        let flow = |body: &str| format!("import {{ flow, respond, route }} from \"holon\"\nimport a from \"host:a.com\"\n\nflow(\"chat\", {{ port: 8200 }})\n\nroute.post(\"/ask\", async (req) => {{\n{body}\n}})\n");
        let fails = |body: &str| check("chat.holon.ts", &flow(body)).err.join("\n");
        assert!(fails("  const sent = await a.post(\"/x\", { stream: true })\n  return sent.status").contains("answer with it"));
        assert!(fails("  const sent = await a.post(\"/x\")\n  return respond(sent.body, { stream: true })").contains("only a streamed call's body can be streamed"));
        assert!(fails("  const sent = await a.post(\"/x\", { stream: true })\n  return respond(sent.body, { stream: true, status: 201 })").contains("takes no status"));
        assert!(fails("  const sent = await a.post(\"/x\", { stream: true, body: 1 })\n  return respond(sent.body, { stream: true })").contains("comes last"));
        assert!(fails("  const x = await a.post(\"/x\", { stream: true })\n  const y = await a.post(\"/y\", { stream: true })\n  return { a: x.body, b: y.body }").contains("one streamed call"));
        let clock = "import { every, flow } from \"holon\"\nimport state from \"state\"\nimport a from \"host:a.com\"\n\nflow(\"chat\")\n\nevery(\"10s\", async () => {\n  state.x = (await a.get(\"/x\", { stream: true })).body\n})\n";
        assert!(check("chat.holon.ts", clock).err.join("\n").contains("a streamed call is a route's"));
        // stream: false is a plain call.
        assert_eq!(check("chat.holon.ts", &flow("  const sent = await a.post(\"/x\", { stream: false })\n  return sent.body")).code, 0);
    }

    #[test]
    fn a_piped_call_goes_to_its_channel_as_it_arrives() {
        let text = "import { as_text, flow, route, status } from \"holon\"\nimport said from \"collection:said\"\nimport room from \"channel:room\"\nimport example from \"host:api.example.com\"\nimport { now } from \"holon:clock\"\n\nflow(\"room\", { port: 8200 })\n\nroute.post(\"/ask\", (req) => {\n  const sent = room.pipe(example.post(\"/v1/chat\", { body: req.json }))\n  said[as_text(now)] = sent.body\n  return status(202, { ok: true })\n})\n";
        let checked = check("room.holon.ts", text);
        assert_eq!(checked.code, 0, "{:?}", checked.err);
        assert_eq!(fmt("room.holon.ts", text).unwrap(), text);
        // Not a route's: a clock flow may pipe too.
        assert_eq!(check("room.holon.ts", "import { every, flow } from \"holon\"\nimport state from \"state\"\nimport room from \"channel:room\"\nimport a from \"host:a.com\"\n\nflow(\"room\")\n\nevery(\"10s\", () => {\n  state.x = room.pipe(a.get(\"/x\")).body\n})\n").code, 0);
        let room = |body: &str| format!("import {{ flow, respond, route, status }} from \"holon\"\nimport room from \"channel:room\"\nimport a from \"host:a.com\"\n\nflow(\"room\", {{ port: 8200 }})\n\nroute.post(\"/ask\", (req) => {{\n{body}\n}})\n");
        let fails = |line: &str| check("room.holon.ts", &room(&format!("{line}\n  return status(202, {{ ok: true }})"))).err.join("\n");
        assert!(fails("  const s = room.pipe(a.post(\"/x\", { stream: true }))").contains("without stream"));
        assert!(fails("  const s = room.pipe(1)").contains("only a call's answer can be piped"));
        assert!(fails("  const s = room.pipe()").contains("room.pipe takes one call"));
        // A piped call isn't the route's reply: it can't be streamed to the caller.
        let streamed = room("  const sent = room.pipe(a.post(\"/x\"))\n  return respond(sent.body, { stream: true })");
        assert!(check("room.holon.ts", &streamed).err.join("\n").contains("only a streamed call's body can be streamed"));
    }

    #[test]
    fn a_subscription_is_only_a_routes_answer() {
        let flow = |body: &str| format!("import {{ flow, route, status }} from \"holon\"\nimport updates from \"channel:updates\"\n\nflow(\"board\", {{ port: 8200 }})\n\nroute.get(\"/live\", (req) => {{\n{body}\n}})\n");
        let fails = |body: &str| check("board.holon.ts", &flow(body)).err.join("\n");
        assert!(fails("  const s = updates.subscribe()\n  return s").contains("a subscription is a route's answer"));
        assert!(fails("  return status(201, updates.subscribe())").contains("takes no status"));
        assert!(fails("  return updates.subscribe({ from: \"old\" })").contains("from: is \"new\""));
        assert!(fails("  return updates").contains("\"updates\" is a channel"));
        assert!(fails("  return updates.watch()").contains("a channel has no \"watch\""));
        assert!(check("board.holon.ts", "import { flow, route } from \"holon\"\n\nflow(\"board\", { port: 8200 })\n\nroute.get(\"/live\", () => updates.subscribe())\n").code != 0);
    }

    #[test]
    fn fmt_is_stable_and_every_sample_round_trips_through_new() {
        let once = fmt("hello.holon.ts", HELLO).unwrap();
        assert_eq!(fmt("hello.holon.ts", &once).unwrap(), once);
        for s in samples::samples() {
            let text = new_flow(Some(s.name)).unwrap();
            assert_eq!(check(&format!("{}.holon.ts", s.name), &text).code, 0, "{}", s.name);
            // The sample's own text, its names and comments and all.
            assert_eq!(text, s.text, "{}", s.name);
        }
        assert_eq!(new_flow(None).unwrap(), STARTER);
        assert_eq!(check("my-flow.holon.ts", STARTER).code, 0);
        // The starter is already canonical, its comment and all.
        assert_eq!(fmt("my-flow.holon.ts", STARTER).unwrap(), STARTER);
        let likes = new_flow(Some("likes")).unwrap();
        assert!(likes.contains("\nimport likes from \"collection:likes\"\n"), "{likes}");
        assert!(likes.contains("\nroute.post(\"/posts/:id/like\", (req) => {\n"), "{likes}");
        assert!(new_flow(Some("nope")).unwrap_err().starts_with("no sample \"nope\""));
    }
}
