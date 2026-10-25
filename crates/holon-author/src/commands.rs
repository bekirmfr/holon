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

/// What the flow derives to: its server's routes (or none), its unit, what it may do, sorted.
fn derive(c: &Lowered, store: &mut MapResolver) -> (DerivedServer, Result<(String, Vec<String>), String>) {
    let server = derive_server(&c.doc.nodes, &c.doc.edges, store, port_of(c));
    let unit = flow_unit(&c.doc.nodes, &c.doc.edges, store, port_of(c)).map(|hash| {
        let mut caps: Vec<String> = store.resolve(&hash).map(|u| capabilities(u, store).iter().map(|c| consent_name(&c)).collect()).unwrap_or_default();
        caps.sort();
        (to_hex(&hash), caps)
    });
    (server, unit)
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
    problems.splice(0..0, server.errors);
    // The flow has no unit because of its routes' errors: each said once.
    let mut seen = std::collections::BTreeSet::new();
    problems.retain(|p| seen.insert(p.clone()));
    let err: Vec<String> = problems.iter().map(|p| format!("{file}: {p}")).collect();
    let code = if err.is_empty() { 0 } else { 1 };
    Report { out, err, code }
}

/// `holon show`: an outline, each node by its name in the text, with its params and what feeds it.
pub fn show(file: &str, source: &str) -> Report {
    let mut store = MapResolver::new();
    let c = match compile(file, source, &mut store) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let server = derive_server(&c.doc.nodes, &c.doc.edges, &mut store, port_of(&c));
    let port = port_of(&c);
    let mut out = vec![format!("{}{}", c.name.as_deref().unwrap_or(file), if port.is_empty() { String::new() } else { format!("  (a server on :{port})") })];
    if !server.endpoints.is_empty() {
        out.push("  routes".into());
        out.extend(server.endpoints.iter().map(|e| format!("    {} {}", e.method, e.path)));
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
    let mut anchors: Vec<Anchor> = vec![];
    let mut above: Vec<String> = vec![];
    let mut gap = false;
    for line in source.lines() {
        let (code, comment) = split_comment(line);
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

    #[test]
    fn check_says_routes_unit_and_what_it_may_do() {
        let r = check("hello.holon", HELLO);
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(r.out[0], "hello-api: 5 nodes, 5 wires");
        assert_eq!(r.out[1..3], ["a server on :8101", "  GET /hello/:name"]);
        assert!(r.out[3].starts_with("unit "));
        assert_eq!(r.out[4..], ["it may:", "  receive requests"]);
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
