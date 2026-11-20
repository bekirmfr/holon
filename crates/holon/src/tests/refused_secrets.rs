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
/// alone or a choice takes it. And every kind of effect a sink has is either one the host refuses a secret to
/// (`refuses_a_secret`) or named here with what the host does with one.
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
/// fail, a ternary on a test of it); one check says nothing of refusing is
/// made where the secret lets it (a request is approved to go where the secret goes). And check's
/// line says which: the secret kept, handed on, or deciding.
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
