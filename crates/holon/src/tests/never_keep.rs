//! A flow's keys never kept (`never_keep: ["cvc"]`, never_keep.rs), through a deployed flow: what
//! its runs would keep of one is refused, what it sends to a host goes, and its record holds the
//! mark where the value stood. Also: a version that drops one asks a person when an agent deploys
//! it, and a flow with one that waits isn't deployed.

use super::*;

/// A body with a CVC too short to remove by its text: `737` is in the amount `1737` too.
const BODY: &str = r#"{"amount":1737,"cvc":"737"}"#;

/// **Guards:** a deployed flow that never keeps `cvc` doesn't keep a body holding it, as a record
/// or as its JSON text, in its state or a collection, doesn't write it as a line, and doesn't hand
/// it to another flow's action (whose run would keep it); the same run's other writes land (the
/// amount alone is written), and a request to a host is sent with the CVC in it. Its record holds
/// `‹never_keep:cvc›` where the CVC stood, the amount as it was, and the CVC nowhere.
/// **Why:** each of these keeps what it is given, past the run's record; the request to the
/// payment processor is the point of taking the CVC.
/// **If it fails:** `firing.rs` performs a write that holds the key (its `never_keep::refused`
/// check), `calls.rs` hands it on, or a line holding it is written.
#[test]
fn a_flow_never_keeps_what_its_writes_would_keep() {
    use std::io::Write;
    // The far side: what it was sent, for the test to read.
    let (sent_tx, sent_rx) = std::sync::mpsc::channel::<String>();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let far = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Some((_, body)) = read_request(&mut stream) else { continue };
            let _ = sent_tx.send(String::from_utf8_lossy(&body).into_owned());
            let answer = r#"{"ok":true}"#;
            let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len()).as_bytes());
        }
    });
    let state = claims_state("");
    let keeper = "import { action, flow } from \"holon\"\nimport state from \"state\"\n\nflow(\"keeper\")\n\nexport const keep = action((x) => {\n  state.kept = x\n  return 1\n})\n";
    deploy_text(&state, keeper, &far);
    let port = free_port();
    let till = format!(
        r#"import {{ flow, route }} from "holon"
import state from "state"
import notes from "collection:notes"
import far from "host:{far}"
import {{ keep }} from "flow:keeper"

flow("till", {{ port: {port}, never_keep: ["cvc"] }})

route.post("/state", (req) => {{
  state.last = req.json
  return "ok"
}})

route.post("/text", (req) => {{
  state.body = req.body
  return "ok"
}})

route.post("/row", (req) => {{
  notes["n1"] = req.json
  return "ok"
}})

route.post("/log", (req) => {{
  console.log(req.json)
  return "ok"
}})

route.post("/hand", async (req) => {{
  const kept = await keep(req.json)
  return kept
}})

route.post("/clean", (req) => {{
  state.amount = req.json.amount
  return "ok"
}})

route.post("/send", async (req) => {{
  const sent = await far.post("/pay", {{ headers: {{ "content-type": "application/json" }}, body: req.body }})
  return sent.status
}})
"#
    );
    deploy_text(&state, &till, &far);
    let post = |path: &str| {
        let request = data::DataRequest { headers: vec![("content-type".into(), "application/json".into())], ..fake_data("POST", path, BODY.as_bytes().to_vec()) };
        let answered = serve_data(&state.store, &state.deployments, &state.deployments_path, "till", &request);
        (answered.status, String::from_utf8_lossy(&answered.body).into_owned())
    };
    for path in ["/state", "/text", "/row", "/log", "/clean"] {
        assert_eq!(post(path).0, 200, "{path}");
    }
    let (status, handed) = post("/hand");
    assert!(status == 200 && handed.contains("refused") && handed.contains("never keeps") && !handed.contains("737\""), "{status} {handed}");
    let (status, sent) = post("/send");
    assert_eq!((status, sent.as_str()), (200, "200"));
    assert_eq!(sent_rx.recv_timeout(EVENTUALLY).unwrap(), BODY, "the processor is sent the CVC");
    crate::recorder::sync_logs(&state.deployments).unwrap();

    // Nothing holding it was kept; the amount alone was.
    let till_state = body_json(route(&state, &req("GET", "/deployments/till/state", vec![])))["state"].clone();
    let names: Vec<&String> = till_state.as_object().unwrap().keys().collect();
    assert!(till_state.get("amount").is_some() && till_state.get("last").is_none() && till_state.get("body").is_none(), "{till_state}");
    assert!(!till_state.to_string().contains("\"737\""), "{names:?}");
    let keeper_state = body_json(route(&state, &req("GET", "/deployments/keeper/state", vec![])))["state"].clone();
    assert!(keeper_state.get("kept").is_none(), "{keeper_state}");
    let entries = log_entries(&state, "till");
    let performed = |path: &str| entries.iter().find(|e| e["url"] == path).map(|e| e["performed"].clone()).unwrap_or_default();
    for path in ["/state", "/text", "/row", "/log"] {
        assert_eq!(performed(path).as_array().map_or(0, Vec::len), 0, "{path}: {}", performed(path));
    }
    assert!(performed("/clean").to_string().contains("StateWrite(\\\"amount\\\")"), "{}", performed("/clean"));
    let lines = body_json(route(&state, &req("GET", "/deployments/till/logs", vec![]))).to_string();
    assert!(!lines.contains("737\\\""), "{lines}");

    // Its record: the mark where the CVC stood, under its key; the amount as it was.
    let log = serde_json::Value::Array(entries.clone()).to_string();
    assert!(!log.contains("\\\"737\\\"") && !log.contains("\"737\""), "{log}");
    assert!(log.contains("‹never_keep:cvc›") && log.contains("1737"), "{log}");
    let send = entries.iter().find(|e| e["url"] == "/send").unwrap();
    let body = send["request"].to_string();
    assert!(body.contains(r#"{\"amount\":1737,\"cvc\":\"‹never_keep:cvc›\"}"#), "the body as it was sent, but the CVC: {body}");
    // And its runs check, and replay, as themselves: the mark is a placeholder there.
    let export = body_json(route(&state, &req("GET", "/deployments/till/export", vec![])));
    assert!(!export.to_string().contains("\\\"737\\\""), "{export}");
    let report = verify::check(&serde_json::from_value(export).unwrap(), None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    let same = replays_as_it_verifies(&state, "till");
    assert!(same.contains("request"), "{same:?}");
}

/// **Guards:** a version that drops a key the flow never keeps asks a person for it when an agent
/// deploys it (`KeepAgain("cvc")`), as one that drops a tainted key asks `Untaint`; one that moves
/// a key from `taint` to `never_keep` asks nothing (it is stricter), and one that moves it back
/// asks `KeepAgain`. The deploy preview lists the keys it never keeps beside the tainted ones.
/// **Why:** values the flow never kept would be kept and recorded from then on, which an agent
/// that reads the log would see.
/// **If it fails:** `deploy.rs`'s `kept_again` or `untainted` no longer asks what it should.
#[test]
fn dropping_a_key_never_kept_asks_a_person() {
    use super::access_table::ask;
    use crate::accounts::{Accounts, Kind};
    let mut state = claims_state("");
    // Each version's unit, staged before the server has accounts.
    let settings = [", taint: [\"cvc\"]", ", never_keep: [\"cvc\"]", "", ", never_keep: [\"cvc\", \"pin\"]", ", taint: [\"number\"], never_keep: [\"cvc\", \"pin\"]", ", never_keep: [\"cvc\", \"pin\", \"number\"]"];
    let units: BTreeMap<&str, String> = settings
        .iter()
        .map(|s| (*s, to_hex(&stage_text(&state, &format!("import {{ flow, route }} from \"holon\"

flow(\"till\", {{ port: 8299{s} }})

route.get(\"/x\", () => 1)
")))))
        .collect();
    let unit = |settings: &str| units[settings].clone();
    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", "correct horse battery").unwrap();
    state.access = access::Access::new(Arc::clone(&book), vec![], vec![]);
    book.add("scout", Kind::Agent, "agent", "ada", "ada").unwrap();
    let (scout, _) = book.create_token("scout", "Scout", "ada").unwrap();
    // What an agent's version asks a person, or nothing when it deploys.
    let by_scout = |settings: &str| {
        let (code, said) = ask(&state, Some(&scout), "POST", "/deploy", serde_json::to_vec(&serde_json::json!({ "slug": "till", "unit": unit(settings), "document": {} })).unwrap());
        match code {
            200 => vec![],
            202 => serde_json::from_value::<Vec<String>>(serde_json::from_str::<serde_json::Value>(&said).unwrap()["proposal"]["needs"].clone()).unwrap(),
            _ => panic!("{settings}: {code} {said}"),
        }
    };
    let first = serde_json::json!({ "slug": "till", "unit": unit(", taint: [\"cvc\"]"), "acknowledge": ["NetworkIngress"] });
    let (code, said) = ask(&state, Some(&ada), "POST", "/deploy", serde_json::to_vec(&first).unwrap());
    assert_eq!(code, 200, "{said}");
    // taint → never_keep: stricter, asks nothing, and deploys.
    assert_eq!(by_scout(", never_keep: [\"cvc\"]"), Vec::<String>::new());
    // never_keep → none, or back to taint: asks for it.
    for settings in ["", ", taint: [\"cvc\"]"] {
        assert_eq!(by_scout(settings), ["KeepAgain(\"cvc\")"], "{settings:?}");
    }
    // One more never kept: stricter.
    assert_eq!(by_scout(", never_keep: [\"cvc\", \"pin\"]"), Vec::<String>::new());
    // And a tainted key dropped still asks Untaint, one moved to never_keep doesn't.
    assert_eq!(by_scout(", taint: [\"number\"], never_keep: [\"cvc\", \"pin\"]"), Vec::<String>::new());
    assert_eq!(by_scout(", never_keep: [\"cvc\", \"pin\", \"number\"]"), Vec::<String>::new());
    assert_eq!(by_scout(", taint: [\"number\"], never_keep: [\"cvc\", \"pin\"]"), ["KeepAgain(\"number\")"]);
    assert_eq!(holon_author::describe::describe_item("KeepAgain(\"cvc\")"), "keep the values of \"cvc\" again, which it never keeps now");
    assert_eq!(touches::part("KeepAgain(\"cvc\")"), ("other", "key never kept", "cvc".to_string(), "kept again".to_string()));
    let said = touches::settings([&[], &[]], [&[], &[]], [&["cvc".to_string()], &[]]);
    assert_eq!(said, serde_json::json!([{ "name": "never keeps", "now": "cvc", "then": "none" }]));
}

/// **Guards:** a unit whose server never keeps a key and that waits (made by hand: the language
/// refuses it where it waits) isn't deployed, and says why.
/// **Why:** a waiting run is stored whole until it goes on, and what it holds under the key with
/// it: not until a waiting run is stored without the value.
/// **If it fails:** `deploy.rs` deploys it, and a CVC lies in a waiting run's store.
#[test]
fn a_unit_that_never_keeps_a_key_and_waits_isnt_deployed() {
    let state = claims_state("");
    let text = "import { action, event, flow, route } from \"holon\"\n\nflow(\"till\", { port: 8298 })\n\nroute.post(\"/a\", (req) => pay(req.json))\n\nexport const pay = action(async (order) => {\n  const d = await event(\"paid\")\n  return d\n})\n";
    let unit = stage_text(&state, text);
    let resolver = closure(&state.store, unit).unwrap();
    let server = resolver.resolve(&unit).unwrap().clone();
    let with = holon_core::Unit { params: [server.params.clone(), vec![holon_core::Param::frozen("never_keep", "cvc")]].concat(), ..server };
    let staged = route(&state, &req("POST", "/unit", holon_core::encode_preimage(&with).unwrap()));
    assert_eq!(staged.status_code, 200);
    let hash = holon_core::hash_unit(&with).unwrap();
    let r = deploy_json(&state, "till", hash, serde_json::json!({ "acknowledge": [] }));
    assert_eq!(r.status_code, 400);
    let said = String::from_utf8_lossy(&body_bytes(r)).into_owned();
    assert!(said.contains("never keeps \\\"cvc\\\" can't wait yet") || said.contains("never keeps \"cvc\" can't wait yet"), "{said}");
}
