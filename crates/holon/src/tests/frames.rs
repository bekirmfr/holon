//! No page of another site may frame the control plane: every kind of answer it gives, as the
//! server sends it (`admitted`), carries the guards (http.rs `GUARDS`), and the editor's page its
//! own policy (editor.rs `PAGE_POLICY`).

use super::*;
use std::sync::atomic::AtomicUsize;

/// Every kind of answer the control plane gives goes out with the four guards: the editor's page
/// and one of its files, an API's JSON and its text, a 404 for no route and for no such thing, a
/// handler's error, a refusal at the gate (a stranger's Host, another site's Origin, a body sent
/// as a form, no sign-in), a preflight, and the two 503s `admitted` answers before routing (too
/// many at once, stopping).
///
/// Why: the editor keeps its session as a bearer token in the browser, so a hostile page that
/// frames it, signed in, can lure a click onto Approve, and only `X-Frame-Options` and
/// `frame-ancestors` stop the browser from showing it in that frame. They are added in one place,
/// around the answer (`admitted`), which a new early answer could step around, and a response
/// that names one of the headers itself keeps its own.
///
/// If it fails, the kind it names can be framed, or sniffed, or sends its address on: give it the
/// guards through `admitted`, not in its handler. The page's own policy must forbid frames too.
#[test]
fn every_kind_of_control_plane_answer_carries_the_frame_guards() {
    let dir = std::env::temp_dir().join(format!("holon-frames-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<html>holon</html>").unwrap();
    std::fs::write(dir.join("assets").join("app-1.js"), "export {}").unwrap();
    let mut state = test_state();
    state.editor = Some(crate::editor::Source::Dir(dir.clone()));
    let mut accounts = test_state();
    with_people(&mut accounts);

    let host = format!("127.0.0.1:{TEST_PORT}");
    let with = |method: &str, url: &str, headers: &[(&str, &str)]| {
        Request::fake_http(method, url, headers.iter().map(|&(k, v)| (k.to_string(), v.to_string())).collect(), vec![])
    };
    let ask = |state: &AppState, request: Request| admitted(state, &request, &AtomicUsize::new(0), 1);
    let mut answers: Vec<(&str, u16, Response)> = vec![
        ("the editor's page", 200, ask(&state, req("GET", "/", vec![]))),
        ("an editor file", 200, ask(&state, req("GET", "/assets/app-1.js", vec![]))),
        ("an API's JSON", 200, ask(&state, req("GET", "/deployments", vec![]))),
        ("an API's text", 200, ask(&state, req("GET", "/health", vec![]))),
        ("no route", 404, ask(&state, req("GET", "/no/such/route", vec![]))),
        ("no such deployment", 404, ask(&state, req("GET", "/deployments/nothing", vec![]))),
        ("a handler's error", 400, ask(&state, req("POST", "/deploy", b"{}".to_vec()))),
        ("a stranger's Host", 403, ask(&state, with("GET", "/deployments", &[("Host", "evil.example")]))),
        ("another site's Origin", 403, ask(&state, with("GET", "/deployments", &[("Host", &host), ("Origin", "https://evil.example")]))),
        ("a body sent as a form", 415, ask(&state, with("POST", "/deploy", &[("Host", &host), ("Content-Type", "text/plain")]))),
        ("no sign-in", 401, ask(&accounts, req("GET", "/deployments", vec![]))),
        ("a preflight", 204, ask(&state, with("OPTIONS", "/deploy", &[("Host", &host), ("Origin", "http://localhost:5173")]))),
        ("too many at once", 503, admitted(&state, &req("GET", "/health", vec![]), &AtomicUsize::new(1), 1)),
    ];
    state.store.1.stop();
    answers.push(("stopping", 503, ask(&state, req("GET", "/health", vec![]))));

    for (what, status, response) in &answers {
        assert_eq!(response.status_code, *status, "{what}: not the kind of answer it names");
        let values = |name: &str| response.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_ref()).collect::<Vec<_>>();
        assert_eq!(values("X-Frame-Options"), ["DENY"], "{what}");
        let policies = values("Content-Security-Policy");
        assert!(
            policies.len() == 1 && policies[0].split(';').any(|directive| directive.trim() == "frame-ancestors 'none'"),
            "{what}: its policy must forbid frames, once: {policies:?}"
        );
        assert_eq!(values("X-Content-Type-Options"), ["nosniff"], "{what}");
        assert_eq!(values("Referrer-Policy"), ["no-referrer"], "{what}");
    }
    let page = &answers[0].2;
    assert!(
        page.headers.iter().any(|(k, v)| k == "Content-Security-Policy" && v == crate::editor::PAGE_POLICY),
        "the editor's page keeps its own policy"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
