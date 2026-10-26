//! The flow commands of `holon`: flows as text files (the language, lang/, in either of its
//! forms), checked and shown on any machine, a screen or not, through the same code as the
//! editor. Each takes the file's name and text and
//! answers what to print (and, for `fmt`, the file's new text); the binary reads and writes files.
//!
//!   holon new <file> [--sample <name>]   a starter flow (or a sample)
//!   holon check <file>                   compile it: its routes, what it may do, or each mistake
//!   holon show <file>                    an outline: routes, nodes and what feeds them
//!   holon fmt <file>                     rewrite it in the canonical form
//!
//! A mistake is printed as `<file>:<line>:<col>: <message>`, and the exit code is 1.

use holon_core::{capabilities, consent_name, MapResolver, Resolver};

use crate::composites::Names;
use crate::describe::describe_item;
use crate::flow::{derive_server, flow_unit, DerivedServer};
use crate::lang::ast::Pos;
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

impl Report {
    fn failed(err: String) -> Report {
        Report { err: vec![err], code: 1, ..Default::default() }
    }
}

pub const STARTER: &str = r#"# A flow: a route, and what it answers. Run `holon check` on it as you go.
flow my-flow
port 8200

route GET /hello/:name (req) {
  respond "Hello, {req.params.name}"
}
"#;

/// A new flow's text: the starter, or the sample `sample` (headed by its title and description).
pub fn new_flow(sample: Option<&str>) -> Result<String, String> {
    let Some(name) = sample else { return Ok(STARTER.into()) };
    let sample = samples::sample(name).ok_or_else(|| format!("no sample \"{name}\" (there are {})", sample_names()))?;
    Ok(format!("# {}: {}\n{}", sample.title, sample.description, sample.text))
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
    let compiled = lang::compile(source).map_err(|e| mistake(file, source, &e))?;
    text::restore(&compiled.doc, store, &Names::new());
    Ok(compiled)
}

fn port_of(c: &Lowered) -> &str {
    c.doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or_default()
}

/// Whether the flow deploys on a schedule: `every`, and no routes. It then runs on the clock, so
/// what it may do includes the clock (core's `Capabilities::deployed`), as the deploy will ask.
pub(crate) fn scheduled(c: &Lowered, server: &DerivedServer) -> bool {
    server.endpoints.is_empty() && c.doc.server.as_ref().is_some_and(|s| !s.interval.trim().is_empty())
}

/// What the flow derives to: its server's routes (or none), its unit, what it may do, sorted.
fn derive(c: &Lowered, store: &mut MapResolver) -> (DerivedServer, Result<(String, Vec<String>), String>) {
    let server = derive_server(&c.doc.nodes, &c.doc.edges, store, port_of(c));
    let scheduled = scheduled(c, &server);
    let unit = flow_unit(&c.doc.nodes, &c.doc.edges, store, port_of(c)).map(|hash| {
        let mut caps: Vec<String> = store.resolve(&hash).map(|u| capabilities(u, store).deployed(scheduled).iter().map(|c| consent_name(&c)).collect()).unwrap_or_default();
        caps.sort();
        (to_hex(&hash), caps)
    });
    (server, unit)
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

/// `holon check`: compiled, its routes, its unit and what it may do; each problem on stderr.
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
    let (caps, modules, mut problems) = match unit {
        Ok((hash, caps)) => {
            out.push(format!("unit {hash}"));
            // A module (a js node) computes only, so it adds no capability; what it may consume
            // per call is a limit the deploy asks for, said here as the approval will say it.
            let modules = holon_core::parse_hash(&hash).and_then(|h| store.resolve(&h)).map(|root| holon_core::module_limits_of(root, &store)).unwrap_or_default();
            (caps, modules, vec![])
        }
        Err(e) => (vec![], Default::default(), vec![e]),
    };
    out.push(if caps.is_empty() && modules.is_empty() { "it only computes: it reads and changes nothing outside" } else { "it may:" }.into());
    out.extend(caps.iter().map(|c| format!("  {}", describe_item(c))));
    if let (Some(fuel), Some(memory)) = (modules.get(holon_core::FUEL_LIMIT), modules.get(holon_core::MEMORY_LIMIT)) {
        out.push(format!("  run code it carries (a js node): up to {fuel} instructions and {} MiB per call, reaching nothing outside", memory / (1024 * 1024)));
    }
    out.extend(while_it_runs(&caps));
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
    let Ok(parsed) = lang::parse(source) else { return vec![] };
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
    let mut store = MapResolver::new();
    let c = compile(file, source, &mut store)?;
    let printed = lang::print(c.name.as_deref(), &c.doc, &mut store, &Names::new(), &c.names).map_err(|e| Report::failed(format!("{file}: {e}")))?;
    Ok(with_comments(source, &printed))
}

/// A line split at its `#` comment (one outside a string): the code, and the comment if any.
fn split_comment(line: &str) -> (&str, Option<&str>) {
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '#' if !quoted => return (&line[..i], Some(&line[i..])),
            _ => {}
        }
    }
    (line, None)
}

/// `line` cut before its 1-based character column `col`.
fn split_at_col(line: &str, col: u32) -> (&str, &str) {
    let at = line.char_indices().nth((col as usize).saturating_sub(1)).map(|(i, _)| i).unwrap_or(line.len());
    line.split_at(at)
}

/// What a line of code says, whatever its spacing: how a statement is found again once printed.
fn spoken(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace()).collect()
}

/// `printed` (the canonical form of `source`) with `source`'s comments put back: each comment
/// line above the statement it was above (with a blank line between when it had one), each
/// comment after code after that line. A statement is found again by what it says, whatever its
/// spacing, in order; a comment whose statement printing wrote otherwise goes with the next one
/// found, and any left over at the end.
pub(crate) fn with_comments(source: &str, printed: &str) -> String {
    struct Anchor {
        spoken: String,
        above: Vec<String>,
        gap: bool,
        after: Option<String>,
    }
    // A js body is JavaScript, where `#` is text (a colour in a style, a private field): no line
    // of one holds a Holon comment. On its first line a comment may precede the `{`; on its last,
    // follow the `}`.
    let bodies: Vec<(Pos, Pos)> = lang::parse(source).map(|f| f.items.iter().filter_map(|i| if let lang::ast::Item::Js(j) = i { Some((j.body_span.start, j.body_span.end)) } else { None }).collect()).unwrap_or_default();
    let split = |n: u32, line: &str| -> (String, Option<String>) {
        for (start, end) in &bodies {
            if n > start.line && n < end.line {
                return (line.to_string(), None);
            }
            if n == start.line && n == end.line {
                // The body on one line: a comment only after its `}`.
                let (head, tail) = split_at_col(line, end.col);
                let (code, comment) = split_comment(tail);
                return (format!("{head}{code}"), comment.map(str::to_string));
            }
            if n == start.line {
                let (head, tail) = split_at_col(line, start.col);
                let (code, comment) = split_comment(head);
                return if comment.is_some() { (code.to_string(), comment.map(str::to_string)) } else { (format!("{head}{tail}"), None) };
            }
            if n == end.line {
                let (head, tail) = split_at_col(line, end.col);
                let (code, comment) = split_comment(tail);
                return (format!("{head}{code}"), comment.map(str::to_string));
            }
        }
        let (code, comment) = split_comment(line);
        (code.to_string(), comment.map(str::to_string))
    };
    let mut anchors: Vec<Anchor> = vec![];
    let mut above: Vec<String> = vec![];
    let mut gap = false;
    for (k, line) in source.lines().enumerate() {
        let (code, comment) = split(k as u32 + 1, line);
        let (code, comment) = (code.as_str(), comment.as_deref());
        if code.trim().is_empty() {
            match comment {
                Some(c) => {
                    above.push(c.trim_end().to_string());
                    gap = false;
                }
                None => gap = !above.is_empty(),
            }
            continue;
        }
        anchors.push(Anchor { spoken: spoken(code), above: std::mem::take(&mut above), gap, after: comment.map(|c| c.trim_end().to_string()) });
        gap = false;
    }
    if anchors.iter().all(|a| a.above.is_empty() && a.after.is_none()) && above.is_empty() {
        return printed.to_string();
    }
    let lines: Vec<&str> = printed.lines().collect();
    let mut before: Vec<Vec<String>> = vec![vec![]; lines.len()];
    let mut trailing: Vec<Option<String>> = vec![None; lines.len()];
    let mut at = 0;
    let mut carried: Vec<String> = vec![];
    for a in anchors {
        let found = (at..lines.len()).find(|&j| spoken(lines[j]) == a.spoken);
        let Some(j) = found else {
            carried.extend(a.above);
            carried.extend(a.after);
            continue;
        };
        let indent: String = lines[j].chars().take_while(|c| c.is_whitespace()).collect();
        let mut put: Vec<String> = carried.drain(..).chain(a.above).map(|c| format!("{indent}{c}")).collect();
        if a.gap && !put.is_empty() {
            put.push(String::new());
        }
        before[j].extend(put);
        if let Some(c) = a.after {
            trailing[j] = Some(c);
        }
        at = j + 1;
    }
    carried.extend(above);
    let mut out: Vec<String> = vec![];
    for (j, line) in lines.iter().enumerate() {
        out.append(&mut before[j]);
        out.push(match &trailing[j] {
            Some(c) => format!("{line} {c}"),
            None => line.to_string(),
        });
    }
    out.extend(carried);
    format!("{}\n", out.join("\n").trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = "flow hello-api\nport 8101\n\nnode req   = request { method: GET, path: \"/hello/:name\" }\nnode name  = field { name: name }\nnode hi    = const { value: \"Hello, \", type: Text }\nnode greet = concat\nnode res   = respond { status: \"200\", missing: \"404\" }\n\nreq.params  -> name.record\nhi.out      -> greet.a\nname.out    -> greet.b\nreq.request -> res.request\ngreet.out   -> res.body\n";

    /// A `#` inside a js body is JavaScript (a colour, a private field), never a Holon comment:
    /// fmt leaves the body whole, and still puts back the comments around the statement.
    #[test]
    fn fmt_leaves_a_hash_inside_a_js_body_alone() {
        let src = "flow f\nport 8203\n\n# the page\nholon page(x) -> html js {\n  return `<style>a{color:#777}</style>${x}`; // #not a holon comment\n}\n\nroute GET /p (req) {\n  respond page(req.query.x ?? \"y\") # answered\n}\n";
        let out = fmt("f.holon", src).unwrap();
        assert_eq!(out, src, "{out}");
        assert_eq!(fmt("f.holon", &out).unwrap(), out, "and again");
        // A body on one line, with a comment after its brace.
        let one = "flow f\n\nholon tag(x) -> t js { return `#${x}` } # a tag\n\nstate.k = tag(1)\n";
        assert_eq!(fmt("f.holon", one).unwrap(), one);
    }

    /// Each js body is compiled once by the engine: a syntax error is a mistake at its line in the
    /// file, with the line and a caret like any other; a body that compiles passes, whatever it
    /// would do at run time (that is the run's to say, and explain's).
    #[test]
    fn check_compiles_each_js_body_and_says_a_mistake_at_its_line() {
        let src = "flow f\nport 8202\n\nholon bad(a) -> b js {\n  const x = ;\n  return x\n}\n\nroute GET /x (req) {\n  respond bad(1)\n}\n";
        let r = check("f.holon", src);
        assert_eq!(r.code, 1);
        assert_eq!(r.err[0], "f.holon:5:13: js \"bad\": SyntaxError: unexpected token ';', primary expression");
        assert_eq!(r.err[1], "    const x = ;");
        assert_eq!(r.err[2], format!("  {}^", " ".repeat(12)));
        assert!(r.out.iter().any(|l| l.starts_with("unit ")), "it still compiles and derives: {:?}", r.out);
        let fine = check("f.holon", &src.replace("const x = ;", "const x = a.b.c;"));
        assert_eq!((fine.code, fine.err), (0, vec![]));
        // On the header line, the column is the file's.
        let one_line = check("f.holon", "flow f\n\nholon bad(a) -> b js { return a + }\nstate.k = bad(1)\n");
        assert!(one_line.err[0].starts_with("f.holon:3:"), "{:?}", one_line.err);
        assert_eq!(js_position("SyntaxError: x at line 2, col 13 (native)"), Some((2, 13)));
        assert_eq!(js_position("TypeError: y (unknown at :3:9)\n    at <main> (unknown at :3:9)"), Some((3, 9)));
        assert_eq!(js_position("the source isn't a function"), None);
    }

    #[test]
    fn check_says_routes_unit_and_what_it_may_do() {
        let r = check("hello.holon", HELLO);
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(r.out[0], "hello-api: 5 nodes, 5 wires");
        assert_eq!(r.out[1..3], ["a server on :8101", "  GET /hello/:name"]);
        assert!(r.out[3].starts_with("unit "));
        assert_eq!(r.out[4..], ["it may:", "  receive requests"]);
    }

    /// A flow that calls out and both reads and writes a collection is told, apart from what it
    /// may do, that the host holds the keys it read from its first call until it commits
    /// (docs/L4s/key-claims-L4.md §3, test 8); one that doesn't call out, or only reads, isn't.
    #[test]
    fn check_says_a_flow_on_a_schedule_runs_on_the_clock() {
        // `every` starts its runs, so it reads the clock as deployed, though nothing reads `now`.
        let r = check("beats.holon", "flow beats\nevery 5s\n\nstate.beats = (state.beats ?? 0) + 1\n");
        assert_eq!(r.code, 0, "{r:?}");
        let may = r.out.iter().position(|l| l == "it may:").unwrap_or_else(|| panic!("{:?}", r.out));
        assert_eq!(r.out[may + 1..], ["  read the clock", "  read its state \"beats\"", "  write its state \"beats\""]);
        // A server answers requests: an interval doesn't make it a clock flow.
        let r = check("hello.holon", &HELLO.replace("port 8101\n", "port 8101\nevery 5s\n"));
        assert!(!r.out.iter().any(|l| l == "  read the clock"), "{:?}", r.out);
    }

    #[test]
    fn check_names_the_held_keys() {
        let r = check("cache-aside.holon", crate::samples::sample("cache-aside").unwrap().text);
        assert_eq!(r.code, 0, "{r:?}");
        let at = r.out.iter().position(|l| l == "while it runs:").unwrap_or_else(|| panic!("{:?}", r.out));
        assert_eq!(r.out[at + 1], "  holds the keys it reads in \"ids\" from its first outbound call until it commits, so two runs on one key call out once");
        assert!(r.out[..at].iter().any(|l| l.starts_with("  call https://")), "said after what it may do: {:?}", r.out);
        let shown = show("cache-aside.holon", crate::samples::sample("cache-aside").unwrap().text);
        assert!(shown.out.iter().any(|l| l == "  while it runs:"), "{:?}", shown.out);
        let r = check("refund-once.holon", crate::samples::sample("refund-once").unwrap().text);
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

    #[test]
    fn a_mistake_names_its_file_and_line() {
        let r = check("bad.holon", "node a = field\nnode b = concat\na.nope -> b.a");
        assert_eq!(r.code, 1);
        assert_eq!(r.err, [r#"bad.holon:3:1: field has no output "nope""#, "  a.nope -> b.a", "  ^^^^^^^^^^^^^"]);
        // The caret sits under the column, in the expression form too.
        let r = check("bad.holon", "route GET /a (req) {\n  respond concat(req.body, nope)\n}");
        assert_eq!(r.err, [r#"bad.holon:2:28: unknown name "nope""#, "    respond concat(req.body, nope)", "                             ^^^^"]);
        // A flow that compiles but can't be a server: its route says why.
        let r = check("half.holon", "port 8101\nnode req = request { method: GET, path: \"/x\" }");
        assert_eq!(r.code, 1);
        assert!(r.err[0].starts_with("half.holon: "), "{r:?}");
    }

    /// Many strings in one route: each `{…}` is an `as_text`, all the same unit, told apart by
    /// where their values go. A value computed twice and never used is said, once.
    #[test]
    fn a_route_of_many_interpolations_compiles_and_unused_twins_are_said() {
        let parts: Vec<String> = (0..12).map(|i| format!("{{req.json.f{i}}}")).collect();
        let text = format!(
            "flow links
port 8106

route POST /links (req) {{
  a = \"{}\"
  b = \"{}/{}\"
  respond {{ a: a, b: b }}
}}
",
            parts.join("/"),
            parts[..6].join("/"),
            parts[6..].join("-")
        );
        let r = check("links.holon", &text);
        assert_eq!(r.code, 0, "{r:?}");
        let r = check("twins.holon", "flow t
port 8107

route POST /t (req) {
  x = lowercase(req.json.a)
  y = lowercase(req.json.a)
  respond \"ok\"
}
");
        assert_eq!(r.code, 1);
        assert_eq!(r.err, ["twins.holon: POST /t: the same value is computed twice and never used, so nothing tells the two apart: remove one, or use it"]);
    }

    #[test]
    fn show_outlines_each_node_and_what_feeds_it() {
        let r = show("hello.holon", HELLO);
        assert_eq!(r.out[..4], ["hello-api  (a server on :8101)", "  routes", "    GET /hello/:name", "  nodes"]);
        assert!(r.out.contains(&"    greet  concat".to_string()), "{:#?}", r.out);
        assert!(r.out.contains(&"             b ← name.out".to_string()), "{:#?}", r.out);
        assert!(r.out.iter().any(|l| l.contains(r#"hi     const  value="Hello, " type="Text""#)), "{:#?}", r.out);
    }

    #[test]
    fn fmt_keeps_the_comments() {
        let messy = "# says hello\n\nflow   hello-api # the flow\nport 8101\nroute GET /hello/:name (req) {\n      # the answer\n      respond   \"Hi, {req.params.name} # not a comment\"   # trailing\n}\n# the end\n";
        let once = fmt("hello.holon", messy).unwrap();
        assert_eq!(
            once,
            "# says hello\n\nflow hello-api # the flow\nport 8101\n\nroute GET /hello/:name (req) {\n  # the answer\n  respond \"Hi, {req.params.name} # not a comment\" # trailing\n}\n# the end\n"
        );
        assert_eq!(fmt("hello.holon", &once).unwrap(), once, "stable");
        // A statement printing writes otherwise: its comment goes with the next one found.
        let rewritten = fmt("hello.holon", "flow f\nport 8200\n\nroute GET /a (req) {\n  # the answer\n  respond concat(\"a\", \"b\")\n}\n").unwrap();
        assert!(rewritten.contains("# the answer"), "{rewritten}");
    }

    #[test]
    fn fmt_writes_folders_back_as_written() {
        let text = "flow files\nport 8200\nfolder inbox\nfolder out\n\nroute POST /move (req) {\n  t = inbox[req.json.name]\n  out[\"{req.json.name}.md\"] = \"# {t}\"\n  delete out[\"{req.json.name}.old\"]\n  respond inbox.list(prefix: \"2026/\", limit: 5)\n}\n";
        assert_eq!(check("files.holon", text).code, 0);
        assert_eq!(fmt("files.holon", text).unwrap(), text);
    }

    #[test]
    fn fmt_writes_channels_back_as_written() {
        let text = "flow board\nport 8200\nchannel updates\n\nroute POST /t (req) {\n  updates.send({ new: req.json.title })\n  respond { ok: true }, status: 201\n}\n\nroute GET /live (req) {\n  respond updates.subscribe()\n}\n\nroute GET /all (req) {\n  respond updates.subscribe(from: \"kept\"), headers: { \"cache-control\": \"no-store\" }\n}\n";
        assert_eq!(check("board.holon", text).code, 0, "{:?}", check("board.holon", text).err);
        assert_eq!(fmt("board.holon", text).unwrap(), text);
    }

    #[test]
    fn fmt_writes_a_streamed_reply_back_as_written() {
        let text = "flow chat\nport 8200\ncollection said\n\nroute POST /ask (req) {\n  sent = http.post(\"api.example.com\", \"/v1/chat\", body: req.json, stream: true)\n  said[as_text(now)] = sent.body\n  respond sent.body, stream: true\n}\n";
        assert_eq!(check("chat.holon", text).code, 0, "{:?}", check("chat.holon", text).err);
        assert_eq!(fmt("chat.holon", text).unwrap(), text);
        // A computed status, written as it was.
        let computed = "flow codes\nport 8200\n\nroute POST /a (req) {\n  respond { ok: false }, status: req.json.code, missing: 400\n}\n";
        assert_eq!(check("codes.holon", computed).code, 0, "{:?}", check("codes.holon", computed).err);
        assert_eq!(fmt("codes.holon", computed).unwrap(), computed);
    }

    #[test]
    fn a_streamed_call_is_forwarded_where_its_route_answers() {
        let flow = |route: &str| format!("flow chat\nport 8200\n\nroute POST /ask (req) {{\n{route}\n}}\n");
        let fails = |route: &str| check("chat.holon", &flow(route)).err.join("\n");
        assert!(fails("  sent = http.post(\"a.com\", \"/x\", stream: true)\n  respond sent.status").contains("answer with it, respond sent.body, stream: true"));
        assert!(fails("  sent = http.post(\"a.com\", \"/x\")\n  respond sent.body, stream: true").contains("only a streamed call's body can be streamed"));
        assert!(fails("  sent = http.post(\"a.com\", \"/x\", stream: true)\n  respond sent.body, stream: true, status: 201").contains("takes no status"));
        assert!(fails("  sent = http.post(\"a.com\", \"/x\", stream: true, body: 1)\n  respond sent.body, stream: true").contains("stream: comes last"));
        assert!(fails("  a = http.post(\"a.com\", \"/x\", stream: true)\n  b = http.post(\"a.com\", \"/y\", stream: true)\n  respond { a: a.body, b: b.body }").contains("one streamed call"));
        assert!(check("chat.holon", "flow chat\nevery 10s\n\nstate.x = http.get(\"a.com\", \"/x\", stream: true).body\n").err.join("\n").contains("a streamed call is a route's"));
        // stream: false is a plain call.
        assert_eq!(check("chat.holon", &flow("  sent = http.post(\"a.com\", \"/x\", stream: false)\n  respond sent.body")).code, 0);
    }

    #[test]
    fn a_piped_call_goes_to_its_channel_as_it_arrives() {
        let text = "flow room\nport 8200\ncollection said\nchannel room\n\nroute POST /ask (req) {\n  sent = room.pipe(http.post(\"api.example.com\", \"/v1/chat\", body: req.json))\n  said[as_text(now)] = sent.body\n  respond { ok: true }, status: 202\n}\n";
        let checked = check("room.holon", text);
        assert_eq!(checked.code, 0, "{:?}", checked.err);
        assert_eq!(fmt("room.holon", text).unwrap(), text);
        // Not a route's: a clock flow may pipe too.
        assert_eq!(check("room.holon", "flow room\nevery 10s\nchannel room\n\nstate.x = room.pipe(http.get(\"a.com\", \"/x\")).body\n").code, 0);
        let fails = |line: &str| check("room.holon", &format!("flow room\nport 8200\nchannel room\n\nroute POST /ask (req) {{\n{line}\n  respond {{ ok: true }}, status: 202\n}}\n")).err.join("\n");
        assert!(fails("  s = room.pipe(http.post(\"a.com\", \"/x\", stream: true))").contains("without stream:"));
        assert!(fails("  s = room.pipe(1)").contains("only a call's answer can be piped"));
        assert!(fails("  s = room.pipe()").contains("room.pipe takes one call"));
        // A piped call isn't the route's reply: it can't be streamed to the caller.
        let streamed = "flow room\nport 8200\nchannel room\n\nroute POST /ask (req) {\n  sent = room.pipe(http.post(\"a.com\", \"/x\"))\n  respond sent.body, stream: true\n}\n";
        assert!(check("room.holon", streamed).err.join("\n").contains("only a streamed call's body can be streamed"));
    }

    #[test]
    fn a_subscription_is_only_a_routes_answer() {
        let flow = |route: &str| format!("flow board\nport 8200\nchannel updates\n\nroute GET /live (req) {{\n{route}\n}}\n");
        let fails = |route: &str| check("board.holon", &flow(route)).err.join("\n");
        assert!(fails("  s = updates.subscribe()\n  respond s").contains("a subscription is a route's answer: respond updates.subscribe()"));
        assert!(fails("  respond updates.subscribe(), status: 201").contains("takes no status"));
        assert!(fails("  respond updates.subscribe(from: \"old\")").contains("from: is \"new\""));
        assert!(fails("  respond updates").contains("\"updates\" is a channel"));
        assert!(fails("  respond updates.watch()").contains("a channel has no \"watch\""));
        assert!(fails("  updates = 1\n  respond 1").contains("a binding may not shadow it"));
        assert!(check("board.holon", "flow board\nport 8200\n\nroute GET /live (req) {\n  respond updates.subscribe()\n}\n").code != 0);
    }

    #[test]
    fn fmt_is_stable_and_every_sample_round_trips_through_new() {
        let once = fmt("hello.holon", HELLO).unwrap();
        assert_eq!(fmt("hello.holon", &once).unwrap(), once);
        for s in samples::samples() {
            let text = new_flow(Some(s.name)).unwrap();
            assert_eq!(check(&format!("{}.holon", s.name), &text).code, 0, "{}", s.name);
            // The sample's own text, its names and all, under its title.
            assert!(text.ends_with(s.text), "{}", s.name);
        }
        assert_eq!(new_flow(None).unwrap(), STARTER);
        assert_eq!(check("my-flow.holon", STARTER).code, 0);
        // The starter is already canonical, its comment and all.
        assert_eq!(fmt("my-flow.holon", STARTER).unwrap(), STARTER);
        let likes = new_flow(Some("likes")).unwrap();
        assert!(likes.contains("collection likes
"), "{likes}");
        assert!(likes.contains("route POST /posts/:id/like (req) {
"), "{likes}");
        assert!(new_flow(Some("nope")).unwrap_err().starts_with("no sample \"nope\""));
    }
}
