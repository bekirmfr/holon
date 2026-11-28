//! A flow travels and is kept as its text, with its hand-drawn layout beside it
//! (docs/L4s/text-deploy-plan.md): the server compiles the text, with the compiler every client
//! compiles with. The class tests of what that must keep.

use super::*;
use holon_author::{agent, kept_layout};
use serde_json::{json, Value};

/// **Guards:** a flow far past the body cap as the description deploys used to send (its canvas,
/// node by node) deploys from the command line as its text, is proposed by an agent as its text,
/// and reads back as that text and that code.
///
/// **Why:** a deploy sent the flow's whole canvas, about 700 bytes a node; a flow of some ten
/// thousand nodes (the workroom flow, 624 KB of text) came to 8 MB and was refused 413 at every
/// deploy and proposal. Its text is a fourteenth of that.
///
/// **If it fails:** a client sends a description again, or the server refuses a text it can
/// compile; large flows can't be deployed or proposed.
///
/// The flow is generated: routes of a few steps each, until its canvas is past the cap.
#[test]
#[ignore = "heavy: about 20 minutes, a flow of some 14,000 nodes compiled four times in a debug build"]
fn a_flow_past_the_old_limit_deploys_and_is_proposed_as_its_text() {
    let routes = 560;
    let mut text = String::from("import { flow, route } from \"holon\"\n\nflow(\"big\", { port: 18297 })\n");
    for i in 0..routes {
        text.push_str(&format!(
            "\nroute.get(\"/r{i}/:n\", (req) => {{\n  const a = req.params.n\n  const b = `${{a}}-{i}`\n  const c = b.toUpperCase()\n  const d = c.length > 3 ? c : `${{c}}!`\n  return {{ route: {i}, a, b, c, d }}\n}})\n"
        ));
    }
    let plan = agent::plan(&text).unwrap();
    let described = serde_json::to_vec(&plan.document).unwrap().len() as u64;
    assert!(described > crate::http::MAX_BODY_BYTES, "the flow's canvas is {described} bytes: not past the cap, the test proves nothing");
    assert!((text.len() as u64) < crate::http::MAX_BODY_BYTES / 8);

    let mut state = test_state();
    let p = with_people(&mut state);
    let (person, agent) = (Signed { state: &state, token: &p.ada }, Signed { state: &state, token: &p.claude });
    // A person deploys it as `holon deploy` does.
    let staged = crate::deploycmd::stage(&person, &plan, "big", &text).unwrap();
    assert!(crate::deploycmd::commit(&person, &plan, staged).unwrap().is_none(), "deployed, not proposed");
    let kept = crate::mcp::kept(&person, "big", "").unwrap();
    assert_eq!((kept["unit"].as_str(), kept["text"].as_str()), (Some(plan.unit.as_str()), Some(text.as_str())));

    // An agent proposes a new version as the agent tools do: it waits for a person, as its text.
    let next = text.replace("route: 0,", "route: 0, again: true,");
    let said = tool_as(&agent, "deploy", json!({ "text": next, "why": "a field more on /r0" })).unwrap();
    assert!(said.contains("waits"), "{said}");
    let proposals = crate::mcp::json_of(&person, "GET", "/proposals", None).unwrap();
    let id = proposals["proposals"][0]["id"].as_u64().unwrap();
    let proposal = crate::mcp::json_of(&person, "GET", &format!("/proposals/{id}"), None).unwrap();
    assert_eq!(proposal["text"].as_str(), Some(next.as_str()));
    assert_eq!(proposal["unit"].as_str(), Some(agent::plan(&next).unwrap().unit.as_str()));
}

/// The agent tool `name` called with `args`, through `api`, as `holon mcp` calls it.
fn tool_as(api: &(dyn crate::mcp::Api + Sync), name: &str, args: Value) -> Result<String, String> {
    use crate::mcp::{Api, Body};
    struct By<'b>(&'b (dyn Api + Sync));
    impl Api for By<'_> {
        fn server(&self) -> String {
            self.0.server()
        }
        fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
            self.0.send(method, path, body)
        }
    }
    let connect = || -> Result<Box<dyn Api + '_>, String> { Ok(Box::new(By(api))) };
    crate::mcp::call_tool(name, &args, &connect, &crate::mcp::Ctx::default())
}

/// The body of a request to `path`, as sent: plain, gzipped, or otherwise encoded.
#[derive(Clone, Copy, Debug)]
enum Sent {
    Plain,
    Gzip,
    /// Says gzip, and isn't.
    BrokenGzip,
    /// Gzip that unpacks past the cap.
    Bomb,
    /// An encoding no route takes.
    Brotli,
}

fn sent(state: &AppState, path: &str, body: &Value, how: Sent) -> Response {
    let json = serde_json::to_vec(body).unwrap();
    let (bytes, encoding) = match how {
        Sent::Plain => (json, None),
        Sent::Gzip => (holon_core::gzip(&json), Some("gzip")),
        Sent::BrokenGzip => (vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff, 1, 2, 3], Some("gzip")),
        Sent::Bomb => (holon_core::gzip(&vec![b' '; crate::http::MAX_BODY_BYTES as usize + 1]), Some("gzip")),
        Sent::Brotli => (json, Some("br")),
    };
    let more = encoding.map(|e| vec![("Content-Encoding".to_string(), e.to_string())]).unwrap_or_default();
    route(state, &req_with("POST", path, bytes, more))
}

/// **Guards:** the routes that take a flow (a deploy, its preview, a save) take it plain or
/// gzipped, alike, and refuse in their own words a gzip that doesn't unpack, one that unpacks
/// past the cap and any other encoding; every other route refuses any encoded body.
///
/// **Why:** a flow's text is sent gzipped when it is large. Unpacking is where one small request
/// becomes a large one (audit F2): it stops at the cap, as a plain body does; and a route that
/// reads its body without unpacking it would read gzip as JSON.
///
/// **If it fails:** the row names the route and the body: unpack it through `read_flow_body` (a
/// flow route) or refuse it in `read_body` (any other).
#[test]
fn a_flow_is_taken_plain_or_gzipped_and_nothing_else_is_taken_encoded() {
    let state = test_state();
    let text = writer_text("gz");
    let flow_routes: [(&str, Value); 3] = [
        ("/deploy/preview", json!({ "slug": "gz", "text": text })),
        ("/deploy", json!({ "slug": "gz", "text": text, "acknowledge": ["Action(\"put\")", "StateWrite(\"gz\")"] })),
        ("/flows", json!({ "name": "gz", "text": text })),
    ];
    for (path, body) in &flow_routes {
        for how in [Sent::Plain, Sent::Gzip] {
            let r = sent(&state, path, body, how);
            let code = r.status_code;
            assert_eq!(code, 200, "{path}, {how:?}: {}", String::from_utf8_lossy(&body_bytes(r)));
        }
        let broken = sent(&state, path, body, Sent::BrokenGzip);
        assert_eq!(broken.status_code, 400, "{path}");
        assert!(body_json(broken)["error"].as_str().unwrap().contains("doesn't unpack as gzip"), "{path}");
        let bomb = sent(&state, path, body, Sent::Bomb);
        assert_eq!(bomb.status_code, 413, "{path}");
        assert!(body_json(bomb)["error"].as_str().unwrap().contains("unpacked"), "{path}");
        assert_eq!(sent(&state, path, body, Sent::Brotli).status_code, 415, "{path}");
    }
    // What the routes keep came the same either way: one unit, one text.
    let kept = body_json(route(&state, &req("GET", "/deployments/gz/text", vec![])));
    assert_eq!(kept["text"], json!(text));

    // Each reads its body: a route that reads none has none to misread.
    for path in ["/unit", "/deployments/gz/settings"] {
        for how in [Sent::Gzip, Sent::BrokenGzip, Sent::Brotli] {
            let r = sent(&state, path, &json!({}), how);
            let code = r.status_code;
            assert_eq!(code, 415, "{path}, {how:?}: {}", String::from_utf8_lossy(&body_bytes(r)));
        }
    }
}

/// `plan`'s canvas with every node moved by `by`: as a person drags it.
fn drawn(plan: &agent::Plan, by: f64) -> kept_layout::Layout {
    let mut canvas = plan.document.clone();
    for (k, n) in canvas["nodes"].as_array_mut().unwrap().iter_mut().enumerate() {
        n["position"] = json!({ "x": by + k as f64, "y": by * 2.0 - k as f64 });
    }
    kept_layout::of_canvas(&canvas, &plan.unit, &plan.document).unwrap().expect("moved")
}

/// **Guards:** a layout is presentation: one text deploys as one unit whatever layout comes with
/// it, each version keeps its own layout, and the server keeps a layout only for the code its text
/// compiles to, only with a text, and only one well formed. A unit sent beside a text must be the
/// text's.
///
/// **Why:** the unit is what a person approves and what runs; positions are where a person put
/// boxes. Were a layout part of the code, moving a box would be a new version to approve; were a
/// layout kept for other code, a canvas would be shown half where it was drawn. The unit a client
/// sends is the code it showed a person: deployed as some other code, the person approved one
/// thing and another runs (R79).
///
/// **If it fails:** what `deploy.rs`'s `compiled` checks of a layout and a unit, or what a version
/// keeps (`layout_ref`), has moved.
#[test]
fn one_text_deploys_as_one_unit_whatever_its_layout_and_keeps_each_versions_layout() {
    let state = test_state();
    let text = writer_text("lay");
    let plan = agent::plan(&text).unwrap();
    let ack = json!(["Action(\"put\")", "StateWrite(\"lay\")"]);
    let deploy = |extra: Value| {
        let mut body = json!({ "slug": "lay", "text": text, "acknowledge": ack });
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        route(&state, &req("POST", "/deploy", serde_json::to_vec(&body).unwrap()))
    };
    let (a, b) = (drawn(&plan, 100.0), drawn(&plan, -400.0));
    assert_ne!(a, b);
    for layout in [&a, &b] {
        let r = deploy(json!({ "layout": layout }));
        let code = r.status_code;
        assert_eq!(code, 200, "{}", String::from_utf8_lossy(&body_bytes(r)));
    }
    let versions = body_json(route(&state, &req("GET", "/deployments/lay/versions", vec![])));
    let units: Vec<&str> = versions["versions"].as_array().unwrap().iter().filter_map(|v| v["unit"].as_str()).collect();
    assert_eq!(units, [plan.unit.as_str(), plan.unit.as_str()], "two layouts, one code");
    let layout_of = |q: &str| body_json(route(&state, &req("GET", &format!("/deployments/lay/text{q}"), vec![])))["layout"].clone();
    assert_eq!(layout_of(""), json!(b));
    assert_eq!(layout_of("?version=1"), json!(a));
    assert_eq!(layout_of("?version=2"), json!(b));

    let refused = |extra: Value, says: &str| {
        let r = deploy(extra.clone());
        assert_eq!(r.status_code, 400, "{extra}");
        let why = body_json(r)["error"].as_str().unwrap().to_string();
        assert!(why.contains(says), "{extra}: {why}");
    };
    // A layout drawn for other code.
    let other = agent::plan(&writer_text("other")).unwrap();
    refused(json!({ "layout": drawn(&other, 5.0) }), "other code than the text's");
    // One that isn't well formed.
    let mut far = a.clone();
    far.at.values_mut().next().unwrap()[0] = [2e9, 0.0];
    refused(json!({ "layout": far }), "finite numbers");
    // A unit that isn't the text's: what a person was shown isn't what would run.
    refused(json!({ "unit": other.unit }), "");
    // A layout without its text.
    let unit_only = json!({ "slug": "lay", "unit": plan.unit, "acknowledge": ack, "layout": a });
    let r = route(&state, &req("POST", "/deploy", serde_json::to_vec(&unit_only).unwrap()));
    assert_eq!(r.status_code, 400);
    assert!(body_json(r)["error"].as_str().unwrap().contains("send the text with it"));
}

/// **Guards:** the server's compile is held to the nesting bound before it reads a text: a text
/// nested far past it is refused in the count's words at a deploy and its preview, on a thread of
/// a Windows program's main stack (1 MiB), and the server answers on. Beside the compiler's own
/// entries (holon-author `lang/nesting.rs`, `no_way_of_writing_a_text_gets_it_past_the_count`),
/// the server's: it compiles what anyone may send it.
///
/// **Why:** the parser recurses; a text nested deep enough runs it off its stack, which ends the
/// process (audit R36). The server now compiles every deploy's text.
///
/// **If it fails:** the server compiles a text some other way than `agent::plan`, or the bound
/// moved behind the parser.
#[test]
fn the_servers_compile_refuses_a_text_past_the_nesting_bound() {
    let state = test_state();
    let depth = 4 * holon_author::lang::nesting::MAX_NESTING;
    let text = format!("import {{ flow, route }} from \"holon\"\n\nflow(\"deep\")\n\nroute.get(\"/a\", (req) => {}1{})\n", "(".repeat(depth), ")".repeat(depth));
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn_scoped(s, || {
                for path in ["/deploy/preview", "/deploy"] {
                    let r = route(&state, &req("POST", path, serde_json::to_vec(&json!({ "slug": "deep", "text": text, "acknowledge": [] })).unwrap()));
                    assert_eq!(r.status_code, 400, "{path}");
                    let why = body_json(r)["error"].as_str().unwrap().to_string();
                    assert!(why.contains("levels deep"), "{path}: {why}");
                }
            })
            .unwrap()
            .join()
            .unwrap();
    });
    assert_eq!(route(&state, &req("GET", "/version", vec![])).status_code, 200, "it answers on");
}

/// **Guards:** a saved-flows file written before flows were kept as their text (each flow a canvas
/// document, 0.1.0-rc.37 and before) loads: every sample saved that way, with the text the editor
/// kept in it or without one, and with its boxes moved by hand, reads back as text that compiles
/// to the same code, its boxes where they were drawn.
///
/// **Why:** the server reads that file as it starts, and refuses to start on one it can't read (the
/// next save would write over every flow). Read as this release's records only, a server upgraded
/// with a single flow saved didn't start.
///
/// **If it fails:** `wire::StoredFlow` or `agent::kept_from_document` stopped reading the old
/// shape: an upgraded server refuses to start, or a saved flow opens as other code.
#[test]
fn a_flow_saved_as_a_canvas_by_an_earlier_release_loads_as_its_text_and_layout() {
    let dir = unique_temp_path("old-flows");
    std::fs::create_dir_all(&dir).unwrap();
    // One file for each way a flow was saved, each sample under its own name, as the editor saved it.
    for how in ["source", "canvas", "moved"] {
        let path = format!("{dir}/{how}.json");
        let mut old = Vec::new();
        for sample in holon_author::samples::samples() {
            let plan = agent::plan(sample.text).unwrap();
            let mut document = plan.document.clone();
            if how != "canvas" {
                document["source"] = Value::String(sample.text.to_string());
            }
            if how == "moved" {
                for (i, n) in document["nodes"].as_array_mut().unwrap().iter_mut().enumerate() {
                    n["position"] = json!({ "x": 7.0 + i as f64, "y": 3.0 - i as f64 });
                }
            }
            old.push(json!({ "name": sample.name, "document": document, "saved_at_epoch_seconds": 1 }));
        }
        crate::at_rest::write_table(std::path::Path::new(&path), crate::at_rest::role::FLOWS, &serde_json::to_vec(&old).unwrap()).unwrap();
        let loaded = load_flows(&Place::parse(&path)).expect("an earlier release's saved flows load");
        for sample in holon_author::samples::samples() {
            let record = &loaded[sample.name];
            let layout = record.layout.as_ref().map(|l| serde_json::to_value(l).unwrap());
            let (_, compiled, placed) = agent::canvas_of(&record.text, layout.as_ref()).unwrap_or_else(|e| panic!("{} ({how}): {e}", sample.name));
            assert_eq!(compiled, agent::plan(sample.text).unwrap().unit, "{} ({how}): its text is its code", sample.name);
            match how {
                "source" => assert_eq!(record.text, sample.text, "{}: the text it was saved with", sample.name),
                "moved" => assert_eq!(placed, Some(kept_layout::Placed::AsDrawn), "{}: its boxes where they were drawn", sample.name),
                _ => {}
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Whether `line` cuts a value at a fixed length by bytes against its own length:
/// `&x[..x.len().min(12)]` or `&x[..12.min(x.len())]`.
fn cuts_by_bytes(line: &str) -> bool {
    let mut from = 0;
    while let Some(at) = line[from..].find("[..").map(|k| from + k) {
        from = at + 3;
        let Some(end) = line[from..].find(']').map(|k| from + k) else { break };
        let inside = &line[from..end];
        let before = &line[..at];
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let cut = match inside.split_once(".len().min(") {
            Some((x, n)) if digits(n.trim_end_matches(')')) => Some(x),
            _ => inside.split_once(".min(").filter(|(n, _)| digits(n)).and_then(|(_, x)| x.strip_suffix(".len())")),
        };
        if cut.is_some_and(|x| !x.is_empty() && before.ends_with(x)) {
            return true;
        }
    }
    false
}

/// **Guards:** no text is cut at a fixed length by bytes in the server or the translator: a value
/// shortened for a message (a unit, a run id, a hash) is cut with `holon_author::first_chars`.
///
/// **Why:** `&unit[..unit.len().min(12)]` panics when byte 12 is inside a character, and the
/// value can come from outside: a `?unit=` of `a€€€€` answered 500 with the deployments' lock held
/// (review of #161, run on a scratch server).
///
/// **If it fails:** use `holon_author::first_chars(&x, n)` at the line it names.
#[test]
fn no_text_is_cut_by_bytes_where_a_character_may_be() {
    let mut files = Vec::new();
    for dir in ["crates/holon/src", "crates/holon-author/src"] {
        super::heavy::rust_files(&super::heavy::root().join(dir), &mut files);
    }
    let found: Vec<String> = files
        .iter()
        .flat_map(|f| {
            let text = std::fs::read_to_string(f).unwrap();
            text.lines().enumerate().filter(|(_, l)| cuts_by_bytes(l) && !l.trim_start().starts_with("//")).map(|(n, _)| format!("{}:{}", f.display(), n + 1)).collect::<Vec<_>>()
        })
        .collect();
    assert!(found.is_empty(), "cut by bytes: {found:#?}");
    // Each written in two pieces, so this file's own lines aren't found.
    for (line, cuts) in [(concat!("&unit[", "..unit.len().min(12)]"), true), (concat!("&h[", "..8.min(h.len())]"), true), (concat!("&b[", "..at.min(b.len())]"), false), (concat!("&plan.unit[", "..12]"), false)] {
        assert_eq!(cuts_by_bytes(line), cuts, "{line}");
    }
}
