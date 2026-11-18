//! A flow's answers are taken as the type they say: every kind the data plane sends, as its own
//! port sends it, carries `X-Content-Type-Options: nosniff`, one a flow sets is kept as it is, and
//! no place but `with_headers` builds an answer (audit R51).

use super::*;

/// Answers each kind: JSON echoing its query, a page, a header of its own, a body taken, a
/// subscription and a WebSocket.
const FLOW: &str = r#"import { flow, respond, route } from "holon"
import ticks from "channel:ticks"

flow("sniff", { port: PORT })

route.get("/json", (req) => ({ echoed: req.query.q }))

route.get("/page", () => respond("<p>hi</p>", { headers: { "content-type": "text/html; charset=utf-8" } }))

route.get("/own", () => respond("x", { headers: { "x-content-type-options": "nosniff" } }))

route.post("/take", (req) => ({ got: req.body }))

route.get("/live", () => ticks.subscribe())

route.websocket("/ws", {
  message: (m) => {
    ticks.send({ text: m.text })
  },
})
"#;

/// The head of the answer to `request` on `port`: what comes before its body, which a
/// subscription doesn't end.
fn head_of(port: u16, request: &[u8]) -> String {
    use std::io::Write;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(EVENTUALLY)).unwrap();
    stream.write_all(request).unwrap();
    let (mut got, mut buf) = (Vec::new(), [0u8; 4096]);
    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&got).into_owned();
    text.split("\r\n\r\n").next().unwrap_or_default().to_string()
}

/// Every kind of answer a flow's port gives goes out with `X-Content-Type-Options: nosniff`, once:
/// a handler's JSON (which may echo what its caller sent, `<script>` included), a page, an answer
/// that sets the header itself, no route, a Host it wasn't given (421, which says the name back),
/// a body over its limit, a WebSocket opened without the upgrade, and a subscription.
///
/// Why: without it a browser may guess a type from the bytes, and run JSON that echoes a caller's
/// text as a page or a script. It is added in one place (dataplane.rs `with_headers`), which the
/// next test holds every answer to.
#[test]
fn every_kind_of_data_plane_answer_is_sent_nosniff() {
    let mut state = claims_state("");
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    state.data_limits.max_body_bytes = 1024;
    let port = free_port();
    let slug = deploy_text(&state, &FLOW.replace("PORT", &port.to_string()), "");
    sync_listeners(&state);

    let get = |path: &str, host: &str| format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    let kinds = [
        ("JSON", "200", get("/json?q=%3Cscript%3E", "127.0.0.1")),
        ("a page", "200", get("/page", "127.0.0.1")),
        ("a header of its own", "200", get("/own", "127.0.0.1")),
        ("no route", "404", get("/nope", "127.0.0.1")),
        ("a Host it wasn't given", "421", get("/json", "evil.example")),
        ("a body over its limit", "413", "POST /take HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n".to_string()),
        ("a WebSocket without the upgrade", "426", get("/ws", "127.0.0.1")),
        ("a subscription", "200", get("/live", "127.0.0.1")),
    ];
    for (what, status, request) in kinds {
        let head = head_of(port, request.as_bytes());
        assert!(head.starts_with(&format!("HTTP/1.1 {status}")), "{what}: {head}");
        let values: Vec<&str> = head.lines().filter_map(|l| l.split_once(':')).filter(|(k, _)| k.eq_ignore_ascii_case("x-content-type-options")).map(|(_, v)| v.trim()).collect();
        assert_eq!(values, ["nosniff"], "{what}: {head}");
    }
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![])).status_code, 200);
}

/// No place in the server but dataplane.rs's `with_headers` builds a data-plane answer, so none
/// goes out without what it adds. If it fails, a new place builds one: send it through
/// `with_headers`.
#[test]
fn only_with_headers_builds_a_data_plane_answer() {
    const BUILDS: [&str; 4] = ["Response::builder(", "hyper::Response::new(", "http::Response::new(", "Response::from_parts("];
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.clone()];
    let mut found = Vec::new();
    while let Some(at) = files.pop() {
        if at.is_dir() {
            // The tests name what they look for.
            if at != src.join("tests") {
                files.extend(std::fs::read_dir(&at).unwrap().map(|e| e.unwrap().path()));
            }
            continue;
        }
        if at.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&at).unwrap();
        let within = text.find("fn with_headers(").map(|start| start..start + text[start..].find("\n}\n").unwrap());
        for build in BUILDS {
            for (i, _) in text.match_indices(build) {
                let place = format!("{}:{}", at.strip_prefix(&src).unwrap().display(), text[..i].lines().count());
                let inside = at.ends_with("dataplane.rs") && within.as_ref().is_some_and(|w| w.contains(&i));
                found.push((place, inside));
            }
        }
    }
    let outside: Vec<&String> = found.iter().filter(|(_, inside)| !inside).map(|(p, _)| p).collect();
    assert!(outside.is_empty(), "built outside with_headers: {outside:?}");
    assert_eq!(found.len(), 2, "with_headers builds it, and its fallback: {found:?}");
}
