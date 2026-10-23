//! The flow commands of `holon`: flows as text files (text.rs), checked and shown on any machine,
//! a screen or not, through the same code as the editor. Each takes the file's name and text and
//! answers what to print (and, for `fmt`, the file's new text); the binary reads and writes files.
//!
//!   holon new <file> [--sample <name>]   a starter flow (or a sample)
//!   holon check <file>                   compile it: its routes, what it may do, or each mistake
//!   holon show <file>                    an outline: routes, nodes and what feeds them
//!   holon fmt <file>                     rewrite it in the canonical form
//!
//! A mistake is printed as `<file>:<line>: <message>`, and the exit code is 1.

use holon_core::{capabilities, consent_name, MapResolver, Resolver};

use crate::composites::Names;
use crate::describe::describe_item;
use crate::flow::{derive_server, flow_unit, DerivedServer};
use crate::text::{self, Compiled};
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

pub const STARTER: &str = r#"# A flow: one statement a line. Run `holon check` on it as you go.
flow my-flow
port 8200

node req   = request { method: GET, path: "/hello/:name" }
node name  = field { name: name }
node hi    = const { value: "Hello, ", type: Text }
node greet = concat
node res   = respond { status: "200", missing: "404" }

req.params  -> name.record
hi.out      -> greet.a
name.out    -> greet.b
req.request -> res.request
greet.out   -> res.body
"#;

/// A new flow's text: the starter, or the sample `sample` (headed by its title and description).
pub fn new_flow(sample: Option<&str>) -> Result<String, String> {
    let Some(name) = sample else { return Ok(STARTER.into()) };
    let sample = samples::sample(name).ok_or_else(|| format!("no sample \"{name}\" (there are {})", sample_names()))?;
    let doc = samples::build(sample)?;
    let text = text::print(Some(sample.name), &doc, &mut MapResolver::new(), &Names::new())?;
    Ok(format!("# {}: {}\n{text}", sample.title, sample.description))
}

/// Every sample's name, for a usage line.
pub fn sample_names() -> String {
    samples::samples().iter().map(|s| s.name).collect::<Vec<_>>().join(", ")
}

/// The file compiled, its units in `store`; or its mistake, as `<file>:<line>: …`.
fn compile(file: &str, source: &str, store: &mut MapResolver) -> Result<Compiled, Report> {
    let compiled = text::compile(source).map_err(|e| Report::failed(format!("{file}:{}: {}", e.line, e.message)))?;
    text::restore(&compiled.doc, store, &Names::new());
    Ok(compiled)
}

fn port_of(c: &Compiled) -> &str {
    c.doc.server.as_ref().map(|s| s.port.as_str()).unwrap_or_default()
}

/// What the flow derives to: its server's routes (or none), its unit, what it may do, sorted.
fn derive(c: &Compiled, store: &mut MapResolver) -> (DerivedServer, Result<(String, Vec<String>), String>) {
    let server = derive_server(&c.doc.nodes, &c.doc.edges, store, port_of(c));
    let unit = flow_unit(&c.doc.nodes, &c.doc.edges, store, port_of(c)).map(|hash| {
        let mut caps: Vec<String> = store.resolve(&hash).map(|u| capabilities(u, store).iter().map(|c| consent_name(&c)).collect()).unwrap_or_default();
        caps.sort();
        (to_hex(&hash), caps)
    });
    (server, unit)
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
    let (caps, mut problems) = match unit {
        Ok((hash, caps)) => {
            out.push(format!("unit {hash}"));
            (caps, vec![])
        }
        Err(e) => (vec![], vec![e]),
    };
    out.push(if caps.is_empty() { "it only computes: it reads and changes nothing outside" } else { "it may:" }.into());
    out.extend(caps.iter().map(|c| format!("  {}", describe_item(c))));
    problems.splice(0..0, server.errors);
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

/// `holon fmt`: the file's text in the canonical form, or its mistake.
pub fn fmt(file: &str, source: &str) -> Result<String, Report> {
    let mut store = MapResolver::new();
    let c = compile(file, source, &mut store)?;
    text::print(c.name.as_deref(), &c.doc, &mut store, &Names::new()).map_err(|e| Report::failed(format!("{file}: {e}")))
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
        assert_eq!(r, Report { err: vec![r#"bad.holon:3: field has no output "nope""#.into()], code: 1, ..Default::default() });
        // A flow that compiles but can't be a server: its route says why.
        let r = check("half.holon", "port 8101\nnode req = request { method: GET, path: \"/x\" }");
        assert_eq!(r.code, 1);
        assert!(r.err[0].starts_with("half.holon: "), "{r:?}");
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
    fn fmt_is_stable_and_every_sample_round_trips_through_new() {
        let once = fmt("hello.holon", HELLO).unwrap();
        assert_eq!(fmt("hello.holon", &once).unwrap(), once);
        for s in samples::samples() {
            let text = new_flow(Some(s.name)).unwrap();
            assert_eq!(check(&format!("{}.holon", s.name), &text).code, 0, "{}", s.name);
        }
        assert_eq!(new_flow(None).unwrap(), STARTER);
        assert_eq!(check("my-flow.holon", STARTER).code, 0);
        assert!(new_flow(Some("nope")).unwrap_err().starts_with("no sample \"nope\""));
    }
}
