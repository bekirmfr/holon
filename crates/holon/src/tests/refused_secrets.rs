//! What `holon check` says the host refuses of a secret, the host refuses, and only that (R165).
//! Check (and the canvas, and the deploy approval) and the host read one rule: core's
//! `refuses_a_secret` for the kinds, `SecretFed::reach` for whether a secret reaches the node,
//! on any of its inputs, its trigger too. Held here over every sink core knows, each input in
//! turn, and in runs.

use super::*;
use holon_core::taint::{refuses_a_secret, Reach, SecretFed};
use holon_core::{interface_of, Edge, MapResolver, Param, Unit, UnitBody, BOUNDARY};

/// The sink `name` as the catalog gives it alone under a root: `trim` of the secret `k` on its
/// input `port`, the root's inputs on the rest, its outputs the root's. The sink is child 2.
fn fed_on(name: &str, port: usize) -> (Unit, MapResolver) {
    let mut r = MapResolver::new();
    let unit = interface_of(name).unwrap();
    let secret = r.insert(Unit { params: vec![Param::frozen("name", "k")], ..interface_of("secret").unwrap() }).unwrap();
    let part = r.insert(interface_of("trim").unwrap()).unwrap();
    let node = r.insert(unit.clone()).unwrap();
    let e = |from_child, from_port, to_child, to_port| Edge { from_child, from_port, to_child, to_port };
    let mut edges = vec![e(0, 0, 1, 0)];
    edges.extend((0..unit.inputs.len() as u32).map(|i| if i as usize == port { e(1, 0, 2, i) } else { e(BOUNDARY, i, 2, i) }));
    edges.extend((0..unit.outputs.len() as u32).map(|o| e(2, o, BOUNDARY, o)));
    let root = Unit { body: UnitBody::Composite { children: vec![secret, part, node], edges }, params: vec![], inputs: unit.inputs.clone(), outputs: unit.outputs.clone() };
    (root, r)
}

/// The sink `name` as the catalog gives it, its inputs the root's, behind a `guard` whose
/// condition is `trim` of the secret `k`: it runs only when the secret decides (a ternary on a
/// test of it, `test ? start(x) : undefined`). The sink is child 2, the guard child 3.
fn chosen_on(name: &str) -> (Unit, MapResolver) {
    let mut r = MapResolver::new();
    let unit = interface_of(name).unwrap();
    let secret = r.insert(Unit { params: vec![Param::frozen("name", "k")], ..interface_of("secret").unwrap() }).unwrap();
    let part = r.insert(interface_of("trim").unwrap()).unwrap();
    let node = r.insert(unit.clone()).unwrap();
    let guard = r.insert(interface_of("guard").unwrap()).unwrap();
    let e = |from_child, from_port, to_child, to_port| Edge { from_child, from_port, to_child, to_port };
    let mut edges = vec![e(0, 0, 1, 0), e(1, 0, 3, 0), e(2, 0, 3, 1), e(3, 0, BOUNDARY, 0)];
    edges.extend((0..unit.inputs.len() as u32).map(|i| e(BOUNDARY, i, 2, i)));
    let outputs = interface_of("guard").unwrap().outputs;
    let root = Unit { body: UnitBody::Composite { children: vec![secret, part, node, guard], edges }, params: vec![], inputs: unit.inputs.clone(), outputs };
    (root, r)
}

/// The name of a capability's kind: `StateWrite("kept")` is `StateWrite`.
fn kind(cap: &Capability) -> String {
    let said = format!("{cap:?}");
    said.split('(').next().unwrap_or(&said).to_string()
}

/// **Guards:** for every sink core knows (`TRIGGERED`), with something computed from a secret on
/// each of its inputs in turn, its trigger too, and behind a choice a secret decides: check marks
/// the sink whichever input it reaches, and one of a kind the host refuses a secret to that a
/// choice takes;
/// it says "the host refuses it" exactly when the host refuses that effect by the rule it refuses
/// by (`taint::refuses`: a call or a start at its node, as calls.rs asks it; the rest after the
/// run, as firing.rs does); and it says the secret only "decides" where it reaches the trigger
/// alone or a choice takes it. And every kind of effect a sink has is either one the host
/// refuses a secret to (`refuses_a_secret`) or named here with what the host does with one; and
/// every leaf that does something outside the run is a sink (`TRIGGERED`), so none escapes this.
/// **Why:** check told a person approving a flow that a start in a `try` block a secret can fail
/// "would hand the secret … to another flow …: the host refuses it", and the host made it
/// whenever the secret parsed, and not when it didn't: calls.rs refused by what a start sends
/// alone, so the other flow's log said which way the secret went. And check said a message to a
/// queue was sent with the secret, which the host refused. Two lists, kept by hand (R165). And
/// the same start spelled `test ? start(x) : undefined` was made by the host and marked by check
/// as nothing at all.
/// **If it fails:** a sink, an input or a kind of effect the host and check say two things of.
/// Make both read `refuses_a_secret` and `SecretFed::reach`; a new kind of effect goes in
/// `refuses_a_secret`, or here with what the host does with a secret.
#[test]
fn what_check_says_the_host_refuses_of_a_secret_it_refuses_on_every_input_of_every_sink() {
    let made_with_one: BTreeMap<&str, &str> = BTreeMap::from([
        ("HttpRequest", "sent where the deploy approval says the secret goes (sent_secrets): what a request with one is for"),
        ("DbRead", "a query's parameters, sent to the database the approval names, as a request's are"),
        ("Log", "the line, with what is computed from a secret withheld (firing.rs, `withholds_effect`)"),
        ("NetworkEgress", "performed in the run and acted on by nothing of the server: nothing leaves"),
    ]);
    // Every leaf core knows that does something outside the run is walked below: a sink, but
    // a route's two answers, never performed.
    for &name in holon_core::GENESIS {
        let Some(unit) = interface_of(name) else { continue };
        let performs = holon_core::capabilities_of(name, &unit.params).is_some_and(|c| c.iter().any(|c| !c.is_sourced()));
        let answers = ["channel_subscribe", "file_serve"].contains(&name);
        assert_eq!(performs && !answers, holon_core::TRIGGERED.contains(&name), "{name}: a leaf that does something outside the run is a sink, and only such a leaf");
    }
    let mut kinds: BTreeSet<String> = BTreeSet::new();
    let mut refused: BTreeSet<String> = BTreeSet::new();
    for &name in holon_core::TRIGGERED {
        let unit = interface_of(name).unwrap();
        let caps = holon_core::capabilities_of(name, &unit.params).unwrap_or_else(|| panic!("{name}: its catalog params classify it"));
        // Each input in turn, then behind a choice the secret decides.
        for port in 0..=unit.inputs.len() {
            let chosen = port == unit.inputs.len();
            let at = match chosen {
                true => format!("{name}, a choice the secret decides taking it"),
                false => format!("{name}.{}", unit.inputs[port].name),
            };
            let trigger = holon_core::takes_trigger(&unit) && port + 1 == unit.inputs.len();
            let (root, r) = if chosen { chosen_on(name) } else { fed_on(name, port) };
            // What check marks (a walk for each secret, flow.rs `secret_lines`), and what the host
            // refuses by (one walk for them all, firing.rs).
            let walk = SecretFed::of_secret(&root, &r, "k");
            let fed = SecretFed::of(&root, &r);
            let marked: BTreeSet<Capability> = walk.sinks().filter(|(path, _)| *path == [2]).map(|(_, c)| c.clone()).collect();
            let expected: BTreeSet<Capability> = match chosen {
                // Chosen, only the kinds the host refuses: a request a secret decides is another
                // question (what its host learns), not this one's.
                true => caps.iter().filter(refuses_a_secret).collect(),
                false => caps.iter().filter(|c| !c.is_sourced() || matches!(c, Capability::DbRead(_))).filter(|c| !(name == "http_stream" && matches!(c, Capability::ChannelSend(_)))).collect(),
            };
            assert_eq!(marked, expected, "{at}: check marks the sink whichever input the secret reaches");
            let reach = walk.reach(&[2]);
            let decided = trigger || chosen;
            assert_eq!(reach, Some(if decided { Reach::Decided } else { Reach::Given }), "{at}");
            assert_eq!(fed.reach(&[2]), reach, "{at}: the host's walk and check's");
            for cap in &marked {
                kinds.insert(kind(cap));
                let line = holon_author::describe::describe_secret_sink("k", cap, reach.unwrap());
                let said = line.ends_with(": the host refuses it");
                let node = holon_core::CALLS.contains(&name).then_some(&[2u32][..]);
                let host = crate::taint::refuses(&fed, cap, node);
                assert_eq!(said, host, "{at}: check says {line:?}, and the host {}", if host { "refuses it" } else { "makes it" });
                assert_eq!(line.contains("as the secret \"k\" decides"), said && decided, "{at}: {line:?}");
                if said {
                    refused.insert(kind(cap));
                }
            }
        }
    }
    // Not vacuous: the kinds this was written for are among those it held.
    for k in ["StateWrite", "CollectionWrite", "FileWrite", "DbWrite", "ChannelSend", "QueueSend", "MailSend", "RunsAction"] {
        assert!(refused.contains(k), "{k}: {refused:?}");
    }
    for k in &kinds {
        assert!(refused.contains(k) != made_with_one.contains_key(k.as_str()), "{k}: {}", if refused.contains(k) { "refused and named as made" } else { "neither refused nor named" });
    }
    for k in made_with_one.keys() {
        assert!(kinds.contains(*k), "{k} is no sink's: take it out");
        assert!(!refuses_a_secret(&match *k {
            "HttpRequest" => Capability::HttpRequest("example.com".into()),
            "DbRead" => Capability::DbRead("shop/orders".into()),
            "Log" => Capability::Log("info".into()),
            _ => Capability::NetworkEgress,
        }));
    }
}

/// A flow `trier` whose routes each make one effect a secret reaches: by its trigger (a
/// statement in a `try` block that fails when the secret's first four characters aren't JSON),
/// or by what it is given. A start of the flow `worker`'s action, a write, a request to `host`.
fn trier(host: &str) -> String {
    let tried = |path: &str, statement: &str| {
        format!("route.post(\"{path}\", async (req) => {{\n  try {{\n    {statement}\n    const p = JSON.parse(k.slice(0, 4))\n  }} catch {{\n    return \"caught\"\n  }}\n  return \"done\"\n}})\n")
    };
    let given = |path: &str, statement: &str| format!("route.post(\"{path}\", async (req) => {{\n  {statement}\n  return \"done\"\n}})\n");
    [
        format!("import {{ flow, route }} from \"holon\"\nimport state from \"state\"\nimport k from \"secret:k\"\nimport api from \"host:{host}\"\nimport {{ touch }} from \"flow:worker\"\n\nflow(\"trier\", {{ port: {} }})\n", free_port()),
        tried("/start-decided", "touch({ n: 1 })"),
        given("/start-given", "touch({ n: k.slice(0, 4) })"),
        given("/start-chosen", "const s = k.startsWith(\"1\") ? touch({ n: 3, at: req.path }) : undefined"),
        given("/start-if", "if (k.startsWith(\"1\")) {\n    touch({ n: 4, at: req.path })\n  }"),
        tried("/write-decided", "state.decided = 1"),
        given("/write-given", "state.kept = k.slice(0, 4)"),
        tried("/request-decided", "api.get(\"/ping\")"),
    ]
    .join("\n")
}

/// **Guards:** in runs, under a secret whose first four characters are JSON and one whose aren't:
/// an effect check says "the host refuses it" of is made in neither run, whether the secret is
/// in what it is given or only decides whether it runs (a statement in a `try` block it can
/// fail, in `if (test)` or a ternary on a test of it); one check says nothing of refusing is made
/// where the secret lets it (a request is approved to go where the secret goes). And check's line
/// says which: the secret kept, handed on, or deciding.
/// **Why:** a start in a `try` block a secret can fail was made in one run and not the other,
/// so the other flow's log said which way the secret went, while check told the approval the
/// host refuses it (R165).
/// **If it fails:** the host and check say two things of one effect: see
/// `what_check_says_the_host_refuses_of_a_secret_it_refuses_on_every_input_of_every_sink`.
#[test]
fn an_effect_check_says_the_host_refuses_is_made_in_no_run_whatever_the_secret() {
    let state = claims_state("");
    let (host, hits) = counting_server();
    let worker = format!("import {{ action, flow }} from \"holon\"\n\nflow(\"worker\", {{ port: {} }})\n\nexport const touch = action((asked) => {{\n  return \"touched\"\n}})\n", free_port());
    let text = trier(&host);
    let checked = holon_author::commands::check("trier.holon.ts", &text);
    assert_eq!(checked.code, 0, "{checked:?}");
    let worker = deploy_text_to(&state, &worker, &[]);
    let trier = deploy_text_to(&state, &text, &[host.as_str()]);
    let kept = |key: &str| !body_json(route(&state, &req("GET", &format!("/deployments/{trier}/state"), vec![])))["state"][key].is_null();
    let runs = || log_entries(&state, &worker).len();
    // Each route: the item its line in check names, and how many of its effects there are now.
    type Count<'a> = Box<dyn Fn() -> usize + 'a>;
    let sent = format!("sends the secret \"k\" to https://{host}");
    let rows: Vec<(&str, &str, &str, Count)> = vec![
        ("/start-decided", "run the action touch of the flow worker as the secret \"k\" decides: the host refuses it", "decided", Box::new(runs)),
        ("/start-chosen", "run the action touch of the flow worker as the secret \"k\" decides: the host refuses it", "decided", Box::new(runs)),
        ("/start-if", "run the action touch of the flow worker as the secret \"k\" decides: the host refuses it", "decided", Box::new(runs)),
        ("/start-given", "would hand the secret \"k\" to another flow (run the action touch of the flow worker): the host refuses it", "given", Box::new(runs)),
        // A write in a `try` block is given its value through the block's choice (a `guard` on
        // how it ends), so what it would keep is computed from the secret: kept, as check says.
        ("/write-decided", "would keep the secret \"k\" (write its state \"decided\"): the host refuses it", "given", Box::new(|| kept("decided") as usize)),
        ("/write-given", "would keep the secret \"k\" (write its state \"kept\"): the host refuses it", "given", Box::new(|| kept("kept") as usize)),
        ("/request-decided", &sent, "made", Box::new(|| hits_of(&hits, "/ping"))),
    ];
    let lines: Vec<&String> = checked.out.iter().skip_while(|l| *l != "with its secrets:").collect();
    // How many of each route's effects its own calls made.
    let mut made: BTreeMap<&str, usize> = BTreeMap::new();
    // One whose first four characters parse as JSON and start with "1", one whose don't: dummy
    // values.
    for secret in ["1234-dummy-value", "live-dummy-value"] {
        let set = req("POST", &format!("/deployments/{trier}/secrets/k"), serde_json::to_vec(&serde_json::json!({ "value": secret })).unwrap());
        assert_eq!(route(&state, &set).status_code, 200);
        for (path, _, _, count) in &rows {
            let before = count();
            let (s, b, _) = data_call(&state, &trier, "POST", path, "");
            assert_eq!(s, 200, "{path}: {b}");
            *made.entry(path).or_default() += count() - before;
        }
    }
    for (path, said, how, _) in &rows {
        let line = lines.iter().find(|l| l.contains(&format!("POST {path}, line ")) && l.ends_with(said)).unwrap_or_else(|| panic!("{path}: check says {said:?}: {lines:#?}"));
        let refused = line.ends_with(": the host refuses it");
        assert_eq!(made[path] == 0, refused, "{path}: made {} times, and check says {line:?}", made[path]);
        assert_eq!(*how == "made", !refused, "{path}");
    }
    assert_eq!(made["/request-decided"], 1, "a request the secret decides goes where it lets it: {made:?}");
}

/// The flow `worker`, whose action `touch` answers what it was asked.
fn worker() -> String {
    format!("import {{ action, flow }} from \"holon\"\n\nflow(\"worker\", {{ port: {} }})\n\nexport const touch = action((asked) => {{\n  return \"touched\"\n}})\n", free_port())
}

/// **Guards:** a signature checked with a secret (`secure_equals` against `hmac_sha256` of the
/// secret whole, what a webhook does) isn't a test of the secret: nothing of it comes out of a
/// one-way leaf, so the steps it decides are made and written with a good signature, none of
/// them with a bad one, and check marks nothing. Three spellings: a `return` of 401 above the
/// steps, the steps in `if (verified)`, a start in a ternary on it.
/// **Why:** what a secret decides is refused (see the tests above); a webhook's verified steps
/// aren't that, and refusing them would break every webhook.
/// **If it fails:** a one-way leaf's answer counts as computed from the secret
/// (holon-core taint.rs, `ONE_WAY`), or a step a choice takes is refused whatever decides it.
#[test]
fn a_signature_checked_with_a_secret_decides_its_steps_and_none_is_refused() {
    let state = claims_state("");
    let text = format!(
        r#"import {{ flow, hmac_sha256, route, secure_equals, status }} from "holon"
import state from "state"
import key from "secret:signing_key"
import {{ touch }} from "flow:worker"

flow("hook", {{ port: {} }})

route.post("/returns", async (req) => {{
  if (!secure_equals(req.headers["x-signature"], hmac_sha256(key, req.body))) return status(401)
  touch({{ n: 1, at: req.path }})
  state.paid = req.path
  return "done"
}})

route.post("/inside", async (req) => {{
  if (secure_equals(req.headers["x-signature"], hmac_sha256(key, req.body))) {{
    touch({{ n: 2, at: req.path }})
    state.seen = req.path
  }}
  return "done"
}})

route.post("/picked", async (req) => {{
  const started = secure_equals(req.headers["x-signature"], hmac_sha256(key, req.body)) ? touch({{ n: 3, at: req.path }}) : undefined
  return "done"
}})
"#,
        free_port()
    );
    let checked = holon_author::commands::check("hook.holon.ts", &text);
    assert_eq!(checked.code, 0, "{checked:?}");
    assert!(!checked.out.iter().any(|l| l.contains("refuses") || l.contains("decides")), "check marks nothing of a signature's test: {:#?}", checked.out);
    let worker = deploy_text_to(&state, &worker(), &[]);
    let hook = deploy_text_to(&state, &text, &[]);
    let key = "whsec-dummy-signing-value";
    let set = req("POST", &format!("/deployments/{hook}/secrets/signing_key"), serde_json::to_vec(&serde_json::json!({ "value": key })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);
    const BODY: &str = r#"{"order":"o1"}"#;
    let good = match holon_core::primitives::compute("hmac_sha256", &[&Value::Text(key.into()), &Value::Text(BODY.into())], &[Param::frozen("encoding", "hex")]) {
        Some(Value::Text(hex)) => hex,
        other => panic!("{other:?}"),
    };
    let bad = "0".repeat(good.len());
    let post = |path: &str, signature: &str| {
        let request = data::DataRequest { headers: vec![("x-signature".into(), signature.into())], ..fake_data("POST", path, BODY.as_bytes().to_vec()) };
        let answered = serve_data(&state.store, &state.deployments, &state.deployments_path, &hook, &request);
        (answered.status, String::from_utf8_lossy(&answered.body).into_owned())
    };
    let runs = || log_entries(&state, &worker).len();
    let kept = |key: &str| !body_json(route(&state, &req("GET", &format!("/deployments/{hook}/state"), vec![])))["state"][key].is_null();
    // A bad signature first: nothing made, nothing written. Then a good one: each made and written.
    for (signature, made, status) in [(bad.as_str(), 0, [401, 200, 200]), (good.as_str(), 1, [200, 200, 200])] {
        for ((path, key), status) in [("/returns", Some("paid")), ("/inside", Some("seen")), ("/picked", None)].into_iter().zip(status) {
            let before = runs();
            let (s, b) = post(path, signature);
            assert_eq!(s, status, "{path}: {b}");
            assert_eq!(runs() - before, made, "{path}: started with a signature that is {}", if made == 1 { "good" } else { "bad" });
            if let Some(key) = key {
                assert_eq!(kept(key), made == 1, "{path}: wrote {key}");
            }
        }
    }
}

/// **Guards:** two starts of one action in one route, one a secret decides (in a `try` block it
/// can fail) and one it doesn't: the one it doesn't is made in every run and recorded as made,
/// the other in none. The host refuses a call or a start by its own node, never by its action:
/// a start is a request the host answers between rounds (eval.rs `Request::FlowStart`), never an
/// effect performed after the run, where a write is refused by its kind for the whole flow.
/// **Why:** a refusal by the action (`derives_effect(RunsAction(..))`) would refuse every start of
/// it once one is decided by a secret.
/// **If it fails:** a start is refused, or recorded as refused, by another node's secret.
#[test]
fn a_start_no_secret_decides_is_made_beside_one_a_secret_decides() {
    let state = claims_state("");
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport k from \"secret:k\"\nimport {{ touch }} from \"flow:worker\"\n\nflow(\"two\", {{ port: {} }})\n\nroute.post(\"/two\", async (req) => {{\n  touch({{ n: 6, at: req.path }})\n  try {{\n    touch({{ n: 7 }})\n    const p = JSON.parse(k.slice(0, 4))\n  }} catch {{\n    return \"caught\"\n  }}\n  return \"done\"\n}})\n",
        free_port()
    );
    let worker = deploy_text_to(&state, &worker(), &[]);
    let two = deploy_text_to(&state, &text, &[]);
    for (secret, refused) in [("1234-dummy-value", 1), ("live-dummy-value", 0)] {
        let set = req("POST", &format!("/deployments/{two}/secrets/k"), serde_json::to_vec(&serde_json::json!({ "value": secret })).unwrap());
        assert_eq!(route(&state, &set).status_code, 200);
        let before = log_entries(&state, &worker);
        let (s, b, _) = data_call(&state, &two, "POST", "/two", "");
        assert_eq!(s, 200, "{b}");
        let after = log_entries(&state, &worker);
        let made: Vec<String> = after[before.len()..].iter().map(|e| e.to_string()).collect();
        assert_eq!(made.len(), 1, "one start made: {made:?}");
        let n = |n: u8| format!("[\"n\",{{\"type\":\"Number\",\"value\":{n}.0}}]");
        assert!(made[0].contains(&n(6)) && !made[0].contains(&n(7)), "the one no secret decides: {made:?}");
        // Its own record: the clean start answered with the run it made; the other refused where
        // the run reached it (the secret parsed), not asked where it didn't.
        let entry = log_entries(&state, &two).into_iter().rfind(|e| e["kind"] == "request").unwrap();
        let exchanges: Vec<&serde_json::Value> = entry["rounds"].as_array().unwrap().iter().flat_map(|r| r["exchanges"].as_array().unwrap()).collect();
        // An answer is a typed record: `{ type: "Record", value: [[key, { type, value }], ..] }`.
        let field = |answer: &serde_json::Value, key: &str| {
            answer["value"].as_array().and_then(|pairs| pairs.iter().find(|p| p[0] == key)).map(|p| p[1]["value"].clone())
        };
        let answered = |key: &str, is: &dyn Fn(&serde_json::Value) -> bool| {
            exchanges.iter().filter(|x| field(&x["response"], key).is_some_and(|v| is(&v))).count()
        };
        assert_eq!(answered("run", &|_| true), 1, "{exchanges:?}");
        assert_eq!(answered("error", &|v| v == "refused"), refused, "{exchanges:?}");
    }
}
