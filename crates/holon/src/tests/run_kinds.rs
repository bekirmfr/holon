//! A run of every kind shows its steps (R151): one flow with a handler of each kind the server
//! logs a run as (`verify::KINDS`), each run started as the server starts it, read by `explain`
//! with values on its handler's lines, and placed on the same nodes by the editor (`placedBy`, held
//! to this test by `editor/src/placed.fixture.json`, which it writes).

use super::*;
use std::net::TcpStream;
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;

/// A handler of every kind, each counting its runs in the state: what explain shows under it.
fn text(port: u16) -> String {
    format!(
        r#"import {{ action, event, every, flow, on, route }} from "holon"
import state from "state"
import inbox from "folder:inbox"
import remit from "mail:remit"
import prices from "feed:prices"
import events from "queue:events"

flow("kinds", {{ port: {port} }})

route.get("/hit", () => `hit ${{++state.hits}}`)

route.websocket("/ws", {{
  message: (m) => {{
    ++state.said
    return m.text
  }},
}})

export const bump = action(() => ++state.bumps)

export const later = action(async () => {{
  const go = await event("go", {{ expires: "1h" }})
  state.later = go.expired
}})

every("1s", () => {{
  ++state.ticks
}})

on.file.created(inbox, (f) => {{
  ++state.files
}})

on.mail(remit, (m) => {{
  ++state.mails
}})

on.feed(prices, (e) => {{
  ++state.feeds
}})

on.queue(events, (m) => {{
  ++state.queued
}})
"#
    )
}

/// **Guards:** a run of each kind the server logs (`verify::KINDS`) shows its steps in `explain`,
/// a value on a line of the handler that ran, and the editor places its steps on the nodes explain
/// does. Each kind is a row; a kind added to KINDS with no row here fails the test until a run of
/// it is started below.
///
/// **Why:** explain and the editor placed a run by a list of kinds, and the list had no mail, feed
/// or queue: their runs read the whole flow's trace, which a flow with handlers doesn't have, and
/// showed no steps (R151). Now one rule places every kind: by its route where the flow has a trace
/// of that name, else by the whole flow's.
///
/// **If it fails:** a kind of run shows no values in explain or the editor's run view, or the two
/// place its steps on different nodes. `HOLON_BLESS=1` rewrites the fixture where only the flow
/// or the runs it records changed.
#[test]
fn a_run_of_every_kind_shows_its_steps_in_explain_and_the_editor() {
    // What the rows' triggers stand in for: a mailbox, a feed and a queue.
    let imap = imap::stand_in::start("remit", "s3cret", true);
    let feed = sse::stand_in::StandIn::start();
    let sqs = sqs::stand_in::StandIn::start();
    sqs.make("events");
    let dir = std::env::temp_dir().join(format!("holon-kinds-{}-{}", std::process::id(), rand::random::<u32>()));
    let inbox = dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    let env = dir.join(".env");
    let secrets = format!(
        "HOLON_SECRET_REMIT_IMAP=imap://remit:s3cret@127.0.0.1:{}\nHOLON_SECRET_PRICES_URL={}\nHOLON_SECRET_EVENTS_URL={}\n",
        imap.port,
        feed.url("/events"),
        sqs.url("events")
    );
    std::fs::write(&env, secrets).unwrap();
    let sources = || secret_sources::Sources::new(Some(env.clone())).unwrap();
    let mut state = claims_state("");
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let folders = folders::Folders::memory().with_mails(mails::Mails::memory().with_sources(sources())).with_feeds(feeds::Feeds::memory().with_sources(sources())).with_queues(queues::Queues::memory().with_sources(sources()));
    state.folders = Arc::new(folders);
    state.folders.mails.serve(Arc::clone(&state.deployments));
    state.folders.queues.serve(Arc::clone(&state.deployments));
    for (path, bind) in [
        ("/admin/folders", serde_json::json!({ "name": "inbox", "path": inbox.display().to_string(), "access": "read-write" })),
        ("/admin/mails", serde_json::json!({ "name": "remit", "imap": "env:HOLON_SECRET_REMIT_IMAP", "verdict_from": "mx.acme.com" })),
        ("/admin/feeds", serde_json::json!({ "name": "prices", "url": "env:HOLON_SECRET_PRICES_URL" })),
        ("/admin/queues", serde_json::json!({ "name": "events", "url": "env:HOLON_SECRET_EVENTS_URL" })),
    ] {
        let bound = route(&state, &req("POST", path, serde_json::to_vec(&bind).unwrap()));
        let status = bound.status_code;
        assert_eq!(status, 200, "{path}: {}", String::from_utf8_lossy(&body_bytes(bound)));
    }
    let port = free_port();
    let text = text(port);
    let slug = deploy_text(&state, &text, "");
    sync_listeners(&state);
    let document = holon_author::agent::plan(&text).unwrap().document;
    let ran = |kind: &str| -> Option<serde_json::Value> {
        crate::recorder::sync_logs(&state.deployments).unwrap();
        log_entries(&state, &slug).into_iter().find(|e| e["kind"] == kind && e["error"].is_null())
    };
    // Tried until a run of `kind` is on record, a bounded number of times: a pass that drives runs
    // nobody asks for may first take note of what is there.
    let start = |kind: &str, mut once: Box<dyn FnMut(u64) + '_>| {
        for pass in 1..=10 {
            once(pass);
            if ran(kind).is_some() {
                return;
            }
        }
        eventually(&format!("a {kind} run"), || ran(kind).is_some());
    };

    // Each kind, started as the server starts it.
    assert_eq!(data_call(&state, &slug, "GET", "/hit", "").0, 200);
    assert_eq!(run_action_named(&state, &slug, "bump", serde_json::json!({})).status_code, 200);
    assert_eq!(run_action_named(&state, &slug, "later", serde_json::json!({})).status_code, 202, "it waits");
    durable::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 4000).wait();
    start("tick", Box::new(|pass| scheduler::run_due_clock_ticks(&state.store, &state.deployments, &state.deployments_path, SystemTime::now() + Duration::from_secs(2 * pass))));
    arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path);
    std::fs::write(inbox.join("a.txt"), "arrived").unwrap();
    start("file", Box::new(|_| arrivals::run_arrivals(&state.store, &state.deployments, &state.deployments_path)));
    let mut looks = inbox::Looks::default();
    inbox::run_mail(&state.store, &state.deployments, &state.deployments_path, &mut looks);
    imap.folders.lock().unwrap().put("INBOX", &remittance("r1", "billing@customer.example", "mx.acme.com", "INV-1,100.00"));
    start("mail", Box::new(|_| inbox::run_mail(&state.store, &state.deployments, &state.deployments_path, &mut looks)));
    let mut held = subscriptions::Held::default();
    subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut held);
    eventually("the feed is listened to", || feed.asked().len() == 2);
    feed.send("1", r#"{"symbol":"ACME","price":10.5}"#);
    eventually("a feed run", || ran("feed").is_some());
    sqs.put("events", r#"{"type":"payout.paid"}"#, None);
    let mut dequeued = dequeue::Held::default();
    dequeue::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut dequeued);
    eventually("a queue run", || ran("queue").is_some());
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(EVENTUALLY)).unwrap();
    let (mut ws, _) = tungstenite::client(format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap(), stream).unwrap();
    ws.send(Message::text("hi")).unwrap();
    assert!(matches!(ws.read().unwrap(), Message::Text(_)), "its message answered");
    eventually("a websocket run", || ran("websocket").is_some());

    // A row a kind: the line its handler begins on (one line, or down to the `})` that ends it).
    let rows = [
        ("request", "route.get(\"/hit\""),
        ("trigger", "export const bump"),
        ("resume", "export const later"),
        ("tick", "every(\"1s\""),
        ("file", "on.file.created("),
        ("mail", "on.mail("),
        ("feed", "on.feed("),
        ("queue", "on.queue("),
        ("websocket", "route.websocket(\"/ws\""),
    ];
    let kinds: BTreeSet<&str> = rows.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(kinds, verify::KINDS.iter().copied().collect::<BTreeSet<_>>(), "a row for each kind the server logs a run as");
    let mut fixture = Vec::new();
    // Every kind that shows no value on its handler's lines, each said, so one run names them all.
    let mut unshown = Vec::new();
    for (kind, begins) in rows {
        let entry = ran(kind).unwrap_or_else(|| panic!("no {kind} run"));
        let shown = holon_author::agent::explain("kinds", &document, &entry).unwrap();
        let view = holon_author::agent::run_view("kinds", &document, &entry).unwrap();
        let first = view.text.iter().position(|l| l.starts_with(begins)).unwrap_or_else(|| panic!("{begins:?} isn't a line of:\n{}", view.text.join("\n")));
        let last = if view.text[first].ends_with(')') { first } else { first + view.text[first..].iter().position(|l| l == "})").unwrap() };
        if view.no_steps || !(first + 1..=last + 1).any(|line| view.notes.contains_key(&(line as u32))) {
            unshown.push(format!("a {kind} run ({}) shows no value on its handler's lines {}–{}:\n{shown}", entry["route"], first + 1, last + 1));
            continue;
        }
        // Where the editor places it: the steps one level down in its handler's unit, on the nodes
        // of the trace its route names.
        let route = entry["route"].as_str().unwrap_or_else(|| panic!("a {kind} run has a route: {entry}"));
        let map: Vec<String> = serde_json::from_value(document["trace"][route].clone()).unwrap_or_else(|_| panic!("the flow has a trace of {route:?}"));
        let last_round = entry["timeline"].as_array().unwrap().iter().filter_map(|t| t["round"].as_u64()).max().unwrap_or(0);
        let mut paths: Vec<Vec<u64>> = entry["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["round"].as_u64().unwrap_or(0) == last_round)
            .map(|t| t["path"].as_array().unwrap().iter().map(|p| p.as_u64().unwrap()).collect::<Vec<u64>>())
            .filter(|p| p.len() == 2 && p[0] == 0)
            .collect();
        paths.sort();
        paths.dedup();
        let nodes: BTreeSet<&String> = paths.iter().map(|p| &map[p[1] as usize]).collect();
        assert!(!nodes.is_empty(), "a {kind} run's steps sit in its handler's unit: {}", entry["timeline"]);
        fixture.push(serde_json::json!({ "kind": kind, "route": route, "paths": paths, "nodes": nodes }));
    }
    ws.close(None).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(unshown.is_empty(), "{} kind(s) show no steps:\n\n{}", unshown.len(), unshown.join("\n\n"));

    let written = serde_json::to_string_pretty(&serde_json::json!({ "trace": document["trace"], "runs": fixture })).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editor/src/placed.fixture.json");
    let kept = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    if kept != written && std::env::var_os("HOLON_BLESS").is_some() {
        std::fs::write(&path, &written).unwrap();
        return;
    }
    assert!(kept == written, "{} is out of date: HOLON_BLESS=1 cargo test -p holon --bin holon a_run_of_every_kind_shows_its_steps writes it again", path.display());
}
