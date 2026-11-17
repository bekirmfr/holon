//! What a version may run and send, and the deploy review that says so, through the real gate
//! and handlers (docs/L4s/access-and-approvals-L4.md §9, step 6): a flow call runs only what
//! whoever deploys it may run (the launch review's H1); an agent's change to what a flow sends out
//! of it waits for a person (H9); and an approval whose steps no one here can take is said not
//! ready.

use super::access_table::ask;
use super::*;
use crate::accounts::{Accounts, Kind};
use crate::bindings::{Scope, Selector, Subject};
use serde_json::{json, Value};

const PW: &str = "correct horse battery";

/// A server with accounts, ada its admin: the accounts, and ada's token.
fn server() -> (AppState, Arc<Accounts>, String) {
    let mut state = test_state();
    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    state.access = access::Access::new(Arc::clone(&book), vec![], vec![]);
    (state, book, ada)
}

/// A person invited as a viewer on the host: their token.
fn person(book: &Accounts, name: &str) -> String {
    book.accept(&book.invite(name, "viewer", None, "ada").unwrap(), PW).unwrap().0
}

/// An agent holding `agent` on the host: its token.
fn agent(book: &Accounts, name: &str) -> String {
    book.add(name, Kind::Agent, "agent", "ada", "ada").unwrap();
    book.create_token(name, name, "ada").unwrap().0
}

/// `role` bound to `name` on the flows `flows` selects.
fn bind(book: &Accounts, name: &str, role: &str, flows: Selector) {
    book.bind(Subject::Principal(name.into()), role, Scope::Flows(flows), None, "ada", None).unwrap();
}

/// `method` `path` with `body` (none when null) as the holder of `token`: its status and its body.
fn send(state: &AppState, token: &str, method: &str, path: &str, body: Value) -> (u16, Value) {
    let (code, said) = ask(state, Some(token), method, path, if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() });
    (code, serde_json::from_str(&said).unwrap_or(Value::Null))
}

/// `text` compiled and its units stored, as ada: its unit.
fn stage(state: &AppState, ada: &str, text: &str) -> String {
    let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
    for unit in &plan.units {
        assert_eq!(ask(state, Some(ada), "POST", "/unit", unit.clone()).0, 200);
    }
    keep_document(&plan.unit, &plan.document);
    plan.unit
}

/// What deploying `unit` as `slug` needs approving, as `token`'s holder is told, and what it
/// touches, as the review's lines say it.
fn review(state: &AppState, token: &str, slug: &str, unit: &str) -> (Value, Vec<String>, Value) {
    let (code, preview) = send(state, token, "POST", "/deploy/preview", json!({ "slug": slug, "unit": unit }));
    assert_eq!(code, 200, "{preview}");
    let lines = crate::touches::text(&crate::touches::lines(&preview["touches"], &preview["settings"]));
    (preview["needs_approval"].clone(), lines, preview["touches"].clone())
}

/// `slug`'s agent versions that ask for nothing more let deploy at once, by `token`'s holder.
fn at_once(state: &AppState, token: &str, slug: &str) {
    let (code, said) = send(state, token, "POST", &format!("/deployments/{slug}/settings"), json!({ "agents_deploy_at_once": true }));
    assert_eq!(code, 200, "{said}");
}

/// `unit` deployed as `slug` by `token`'s holder: a person approving what the review says it
/// needs, an agent approving nothing.
fn deploy_as(state: &AppState, token: &str, slug: &str, unit: &str, by_agent: bool) -> (u16, Value) {
    let needs = if by_agent { json!([]) } else { review(state, token, slug, unit).0 };
    send(state, token, "POST", "/deploy", json!({ "slug": slug, "unit": unit, "document": document_of(unit), "acknowledge": needs }))
}

const PAYMENTS: &str = r#"import { action, flow } from "holon"

flow("payments", { tags: ["payments"] })

export const payout = action((asked) => {
  return { paid: asked.amount }
})
"#;

const PROMO: &str = r#"import { action, flow } from "holon"
import { payout } from "flow:payments"

flow("promo", { tags: ["marketing"] })

export const reward = action(async (asked) => {
  const paid = await payout({ amount: asked.amount })
  return paid
})
"#;

/// H1: a call runs the other flow's action under that flow's grant with what the caller sends, so
/// whoever deploys the caller may run that action themselves. bob, editor on `marketing` only,
/// deploys no version of `promo` that runs `payments`' payout, first or granted already, and his
/// review says why; pat, editor on both, and ada do. An agent's version that runs an action the
/// agent may not run waits for a person who may, as a proposal, which bob can't approve and pat
/// can; once `payments` lets agents run payout, the agent deploys it, where a person let `promo`
/// take an agent's versions at once.
#[test]
fn a_flow_call_runs_only_what_whoever_deploys_it_may_run() {
    let (state, book, ada) = server();
    let (bob, pat) = (person(&book, "bob"), person(&book, "pat"));
    let scout = agent(&book, "scout");
    bind(&book, "bob", "editor", Selector::Tag("marketing".into()));
    bind(&book, "pat", "editor", Selector::Tag("marketing".into()));
    bind(&book, "pat", "editor", Selector::Tag("payments".into()));
    let payments = stage(&state, &ada, PAYMENTS);
    let promo = stage(&state, &ada, PROMO);
    assert_eq!(deploy_as(&state, &ada, "payments", &payments, false).0, 200);

    let bobs = "this version runs \"payments\"'s action \"payout\", which bob may not run (a viewer only reads): a call runs it under \"payments\"'s grant with what this flow sends, so someone who may run it deploys this version";
    // Who deploys `promo`, as it stands then, and what comes of it.
    let rows: [(&str, &str, u16, &str); 4] = [
        ("bob, a first version", &bob, 403, bobs),
        ("pat", &pat, 200, ""),
        ("bob, the call granted already", &bob, 403, bobs),
        ("ada", &ada, 200, ""),
    ];
    for (what, token, code, said) in rows {
        let (got, answer) = deploy_as(&state, token, "promo", &promo, false);
        assert_eq!((got, answer["error"].as_str().unwrap_or_default()), (code, said), "{what}: {answer}");
    }
    let (_, lines, _) = review(&state, &bob, "promo", &promo);
    assert!(lines.contains(&"  ! flow payments: run payout — bob may not run it: someone who may deploys this version".to_string()), "{lines:#?}");

    // scout, the same version again: payments doesn't let agents run payout.
    let (code, answer) = deploy_as(&state, &scout, "promo", &promo, true);
    assert_eq!((code, &answer["proposal"]["needs"]), (202, &json!(["RunsAction(\"payments/payout\")"])), "{answer}");
    let (needs, lines, _) = review(&state, &scout, "promo", &promo);
    assert_eq!(needs, json!(["RunsAction(\"payments/payout\")"]));
    assert!(lines.contains(&"  ! flow payments: run payout — asked again: approved by confirming; scout may not run it: it waits for someone who may, as a proposal".to_string()), "{lines:#?}");
    let approve = json!({ "slug": "promo", "unit": promo, "document": {}, "acknowledge": ["RunsAction(\"payments/payout\")"] });
    let (code, answer) = send(&state, &bob, "POST", "/deploy", approve.clone());
    assert_eq!((code, answer["error"].as_str().unwrap_or_default()), (403, bobs), "bob doesn't approve it");
    assert_eq!(send(&state, &pat, "POST", "/deploy", approve).0, 200, "pat does");

    let agents_may = json!({ "slug": "payments", "unit": payments, "requirements": { "agents": ["payout"] }, "acknowledge": ["AgentRuns(\"payout\")"] });
    assert_eq!(send(&state, &ada, "POST", "/deploy", agents_may).0, 200);
    at_once(&state, &pat, "promo");
    let (code, answer) = deploy_as(&state, &scout, "promo", &promo, true);
    assert_eq!(code, 200, "agents may run payout now: {answer}");
}

const TALLY: &str = r#"import { action, flow } from "holon"
import state from "state"

flow("tally")

export const keep = action((asked) => {
  state.count = asked.n
  return "kept"
})
"#;

/// R25: every version an agent deploys waits for a person, as a proposal, whatever it changes: the
/// same code, a change that asks for nothing more, one that asks for more. Someone who may approve
/// the flow's deploys lets it take an agent's versions at once (its settings), where the host lets
/// them: then a version that asks for nothing more deploys from the agent, and one that asks for
/// more still waits (the rule before, now the floor). No one else turns it on: not the agent, not a
/// person who may only deploy the flow, not anyone where the policy says `[agents] deploy_at_once
/// = false`, which holds a flow that has it on too. A person who may only deploy it deploys what
/// they wrote as before: a version that asks for nothing more. Whoever changes its settings turns
/// it off. It
/// is carried across a person's redeploy and a restart, said on the flow's timeline, its
/// deployment and what it is granted; each time, the preview says what the deploy does. A host
/// that takes no proposals refuses each agent version, saying so.
///
/// **Why:** an agent's version that asked for nothing new deployed at once, so code no person had
/// read ran (the audit's R25). The owner's decision: every change to a deployed flow waits for a
/// person with all it changes, unless one grants otherwise, for that flow.
#[test]
fn every_agent_version_waits_for_a_person_unless_one_lets_the_flow_take_it() {
    let path = std::env::temp_dir().join(format!("holon-at-once-{}-{}.json", std::process::id(), rand::random::<u32>()));
    let mut state = test_state_at(Place::parse(path.to_str().unwrap()));
    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    state.access = access::Access::new(Arc::clone(&book), vec![], vec![]);
    let scout = agent(&book, "scout");
    let cy = person(&book, "cy");
    let deployer = ["flow.view", "flow.edit", "flow.propose", "flow.deploy"].map(String::from).to_vec();
    book.define_role(crate::bindings::RoleText { name: "deployer".into(), about: None, allows: deployer, kinds: vec![Kind::Person] }, "ada", None).unwrap();
    bind(&book, "cy", "deployer", Selector::Slug("tally".into()));
    let base = stage(&state, &ada, TALLY);
    let changed = stage(&state, &ada, &TALLY.replace("return \"kept\"", "return \"noted\""));
    let more = stage(&state, &ada, &format!("{TALLY}\nexport const push = action(async () => {{\n  await crm.post(\"/rows\", {{ body: JSON.stringify(state.count) }})\n}})\n").replace("import state from \"state\"", "import state from \"state\"\nimport crm from \"host:crm.example.com\""));
    assert_eq!(deploy_as(&state, &ada, "tally", &base, false).0, 200);
    let policy = |text: &str| *state.policy.write().unwrap() = Arc::new(Policy::parse(text).unwrap());
    let entry = || send(&state, &ada, "GET", "/deployments", Value::Null).1["deployments"].as_array().unwrap().iter().find(|d| d["slug"] == "tally").cloned().unwrap();
    // What the agent's deploy of each version does, the deployed one as ada left it: `waits`, a
    // proposal (what it asks for a person to approve: nothing for the first two); or deployed. Its
    // preview says so first.
    let versions = |when: &str, waits: [bool; 3]| {
        for ((what, unit), waits) in [("the same code", &base), ("a change asking for nothing more", &changed), ("a change asking for more", &more)].into_iter().zip(waits) {
            assert_eq!(deploy_as(&state, &ada, "tally", &base, false).0, 200, "{when}, {what}");
            let (code, preview) = send(&state, &scout, "POST", "/deploy/preview", json!({ "slug": "tally", "unit": unit }));
            assert_eq!((code, &preview["proposed"]), (200, &json!(waits)), "{when}, {what}: the preview says it: {preview}");
            let (code, said) = deploy_as(&state, &scout, "tally", unit, true);
            let asks = unit == &more;
            match waits {
                true => {
                    assert_eq!(code, 202, "{when}, {what}: waits for a person: {said}");
                    assert_eq!(said["proposal"]["needs"].as_array().unwrap().is_empty(), !asks, "{when}, {what}: {said}");
                }
                false => assert_eq!(code, 200, "{when}, {what}: deploys at once: {said}"),
            }
        }
    };
    versions("as deployed", [true, true, true]);
    assert_eq!(entry()["agents_deploy_at_once"], false, "off unless a person turns it on");

    // Who turns it on.
    let turn = |token: &str, on: bool| send(&state, token, "POST", "/deployments/tally/settings", json!({ "agents_deploy_at_once": on }));
    assert_eq!(turn(&scout, true).0, 403, "not the agent");
    let (code, said) = turn(&cy, true);
    assert_eq!(code, 403, "{said}");
    assert!(said["error"].as_str().unwrap().ends_with("letting an agent's versions of \"tally\" deploy without a person is for someone who may approve its deploys"), "{said}");
    policy("[agents]\ndeploy_at_once = false\n");
    let (code, said) = turn(&ada, true);
    assert_eq!((code, said["error"].as_str()), (403, Some("this host lets no flow's agent versions deploy without a person (holon.toml: agents.deploy_at_once = false): each version of \"tally\" an agent deploys waits for one")));
    assert_eq!(entry()["agents_deploy_at_once"], false, "what is refused changes nothing");
    policy("");
    assert_eq!(turn(&ada, true).0, 200, "ada may approve its deploys");
    assert_eq!(entry()["agents_deploy_at_once"], true);
    assert_eq!(send(&state, &ada, "GET", "/deployments/tally/grant", Value::Null).1["agents_deploy_at_once"], true, "in what it is granted");
    let said_on = state.deployments.lock().unwrap()["tally"].events.entries().iter().any(|e| e.kind == "settings" && e.by.as_deref() == Some("ada") && e.what.starts_with("an agent's version that asks for nothing more now deploys without a person"));
    assert!(said_on, "on its timeline, with who");

    versions("let deploy at once", [false, false, true]);
    // Carried across a person's redeploy (each row's), and kept through a restart.
    assert_eq!(entry()["agents_deploy_at_once"], true);
    assert!(load_deployments(&state.deployments_path, &state.store, &state.policy()).unwrap()["tally"].agents_deploy_at_once, "kept");
    // The host's policy holds it off where a flow has it on.
    policy("[agents]\ndeploy_at_once = false\n");
    versions("on, but the host lets no flow", [true, true, true]);
    policy("[agents]\npropose = false\n");
    assert_eq!(deploy_as(&state, &ada, "tally", &base, false).0, 200);
    let (code, said) = deploy_as(&state, &scout, "tally", &more, true);
    assert_eq!(code, 403, "{said}");
    assert!(said["error"].as_str().unwrap().starts_with("this host takes no proposals from agents (holon.toml: agents.propose = false): each version of \"tally\" an agent deploys waits for a person, and this one needs "), "{said}");
    policy("");
    // Whoever changes its settings turns it off.
    assert_eq!(turn(&cy, false).0, 200, "cy may change its settings");
    versions("turned off again", [true, true, true]);
    let said_off = state.deployments.lock().unwrap()["tally"].events.entries().iter().any(|e| e.kind == "settings" && e.by.as_deref() == Some("cy") && e.what == "each agent's version waits for a person again");
    assert!(said_off);
    // A person who may only deploy it: what they wrote that asks for nothing more deploys, as
    // before; what asks for more waits for someone who approves.
    assert_eq!(deploy_as(&state, &ada, "tally", &base, false).0, 200);
    let (code, said) = deploy_as(&state, &cy, "tally", &changed, true);
    assert_eq!(code, 200, "{said}");
    let (code, said) = deploy_as(&state, &cy, "tally", &more, true);
    assert_eq!((code, said["proposal"]["agent"].as_bool()), (202, Some(false)), "{said}");
    let _ = std::fs::remove_file(&path);
}

const SHOWN: &str = r#"import { flow, route } from "holon"

flow("shown", { port: 18447, browsers: ["https://a.example"], taint: ["card"], never_keep: ["cvc"], tags: ["t1"] })

route.get("/a", () => "a")

route.get("/b", () => "b")

route.get("/hi", () => "hi")
"#;

/// R25, R24: a version's review shows all it changes beside the version deployed now, to whoever
/// reads it (a person who approves it, the agent that proposes it): its code, handler by handler
/// (one changed, one added, one removed), and its settings (the sites whose pages it answers, the
/// keys it keeps out of its records and those it never keeps, its port, a handler's log level),
/// an agent's asking to record or keep a key again among what it needs approved, its tags among what it
/// touches. A first deploy has nothing to set beside, and the same version again changes none of
/// it. And what a person reads is what deploys: a proposal whose document builds another unit,
/// whose browsers line isn't the browsers sent, or that doesn't read, is refused and nothing
/// proposed; so is an agent's version deployed at once with one, which the next review reads.
///
/// **Why:** the review said nothing of a version's port or log levels, nor per handler, a person
/// reviewing in the editor never saw the sites it answers (its preview sent none, R24), and the
/// document shown was never checked against the unit approved (R25).
#[test]
fn a_review_shows_all_a_version_changes_and_what_it_shows_deploys() {
    let (state, book, ada) = server();
    let scout = agent(&book, "scout");
    // A person who may deploy it, not approve its deploys: what asks for more, they propose.
    let cy = person(&book, "cy");
    let deployer = ["flow.view", "flow.edit", "flow.propose", "flow.deploy"].map(String::from).to_vec();
    book.define_role(crate::bindings::RoleText { name: "deployer".into(), about: None, allows: deployer, kinds: vec![Kind::Person] }, "ada", None).unwrap();
    bind(&book, "cy", "deployer", Selector::Slug("shown".into()));
    let base = stage(&state, &ada, SHOWN);
    let next = stage(
        &state,
        &ada,
        &SHOWN
            .replace(r#"port: 18447, browsers: ["https://a.example"], taint: ["card"], never_keep: ["cvc"], tags: ["t1"]"#, r#"port: 18448, browsers: ["https://a.example", "https://b.example"], tags: ["t2"]"#)
            .replace(r#"() => "a")"#, r#"() => "A")"#)
            .replace(r#"route.get("/b", () => "b")"#, r#"route.get("/c", () => "c")"#)
            .replace(r#"route.get("/hi", () => "hi")"#, r#"route.get("/hi", () => "hi", { log: "lazy" })"#),
    );
    let (one, both) = (json!(["https://a.example"]), json!(["https://a.example", "https://b.example"]));
    let preview = |token: &str, unit: &str, browsers: &Value| {
        let (code, preview) = send(&state, token, "POST", "/deploy/preview", json!({ "slug": "shown", "unit": unit, "browsers": browsers }));
        assert_eq!(code, 200, "{preview}");
        preview
    };
    let lines = |preview: &Value| crate::touches::text(&crate::touches::lines(&preview["touches"], &preview["settings"]));
    let beside = |lines: &[String]| lines.iter().any(|l| l == "Its handlers" || l == "Its settings");
    let first = preview(&ada, &base, &one);
    assert!(!beside(&lines(&first)), "a first deploy: {:#?}", lines(&first));
    let deployed = json!({ "slug": "shown", "unit": base, "document": document_of(&base), "browsers": one, "acknowledge": first["needs_approval"] });
    assert_eq!(send(&state, &ada, "POST", "/deploy", deployed).0, 200);

    for (who, token) in [("ada", &ada), ("scout", &scout)] {
        let lines = lines(&preview(token, &next, &both));
        for want in [
            "Its handlers",
            "  ~ handler GET /a",
            "  + handler GET /c — new",
            "  - handler GET /b — no longer",
            "Its settings",
            "  ~ answers pages from: https://a.example → https://a.example https://b.example",
            "  ~ keeps out of its records: card → none of its own",
            "  ~ never keeps: cvc → none",
            "  ~ port: 18447 → 18448",
            "  ~ GET /hi logs its runs: strict → lazy",
        ] {
            assert!(lines.iter().any(|l| l == want), "{who}: {want:?} in {lines:#?}");
        }
        assert!(lines.iter().any(|l| l.contains("joins t2")) && lines.iter().any(|l| l.contains("leaves t1")), "{who}: {lines:#?}");
    }
    // An agent's version that keeps a key again asks a person for it, as one that records a
    // tainted key again does: an item it needs approved, said in words, beside its settings' line.
    let asked = preview(&scout, &next, &both);
    for (item, said) in [("KeepAgain(\"cvc\")", "keep the values of \"cvc\" again, which it never keeps now"), ("Untaint(\"card\")", "record the values of \"card\" again, which it keeps out of its records now")] {
        assert!(asked["needs_approval"].as_array().unwrap().contains(&json!(item)), "{item}: {asked}");
        assert_eq!(holon_author::describe::describe_item(item), said);
    }
    let same = lines(&preview(&ada, &base, &one));
    assert!(!beside(&same), "the same version: {same:#?}");

    // What a person reads is what deploys: an agent's proposal, and a person's.
    let propose = |token: &str, document: Value, browsers: &Value| send(&state, token, "POST", "/deploy", json!({ "slug": "shown", "unit": next, "document": document, "browsers": browsers, "why": "two sites" }));
    let rows: [(&str, &str, Value, &Value, &str); 4] = [
        ("another version's document", &scout, document_of(&base), &both, "the document sent deploys as "),
        ("a browsers line that isn't the browsers sent", &scout, document_of(&next), &one, "the document's browsers line (https://a.example https://b.example) isn't the browsers sent (https://a.example)"),
        ("a document that doesn't read", &scout, json!({ "nodes": 3 }), &both, "the flow's document doesn't read"),
        ("a person's proposal with another version's document", &cy, document_of(&base), &both, "the document sent deploys as "),
    ];
    for (what, token, document, browsers, said) in rows {
        let (code, answer) = propose(token, document, browsers);
        assert_eq!(code, 400, "{what}: {answer}");
        assert!(answer["error"].as_str().unwrap().starts_with(said), "{what}: {answer}");
    }
    assert!(state.proposals.all().iter().all(|p| p.slug != "shown"), "nothing proposed");
    let (code, answer) = propose(&scout, document_of(&next), &both);
    assert_eq!(code, 202, "with its own document: {answer}");
    // Deployed at once, the agent's version is held to it too.
    at_once(&state, &ada, "shown");
    let again = |document: Value| send(&state, &scout, "POST", "/deploy", json!({ "slug": "shown", "unit": base, "document": document, "browsers": one }));
    let (code, answer) = again(document_of(&next));
    assert_eq!(code, 400, "{answer}");
    assert_eq!(again(document_of(&base)).0, 200);
}

/// R22: a flow whose canvas has two approvals named alike, asking for other steps (the language
/// refuses them where they are written; a canvas can build them), is refused: its preview says why,
/// and its deploy is 400, saying the same. Named apart, it deploys.
///
/// **Why:** a run keeps one decision a name, so the second was answered with the first's: a review
/// showed a CFO step, and one approval by someone without the role completed the run.
#[test]
fn a_canvas_with_two_approvals_named_alike_is_refused() {
    let (state, _book, ada) = server();
    let text = r#"import { action, approval, flow } from "holon"

flow("twice")

export const pay = action(async (order) => {
  const a = await approval("payout")
  const b = await approval("cfo_check", { steps: [{ role: "cfo" }] })
  return a.approved && b.approved
})
"#;
    let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
    let alike: Value = serde_json::from_str(&plan.document.to_string().replace("\"cfo_check\"", "\"payout\"")).unwrap();
    for (what, document, refused) in [
        ("two approvals named alike", alike, Some("an approval asking for a person and an approval asking for a person holding cfo are both named \"payout\": a run keeps one decision a name, so the second would be answered with the first's, and what it asks for never asked. Name them apart")),
        ("named apart", plan.document.clone(), None),
    ] {
        let (unit, units) = holon_author::agent::canvas_units(&document).unwrap();
        for u in units {
            assert_eq!(ask(&state, Some(&ada), "POST", "/unit", u).0, 200, "{what}");
        }
        let (code, preview) = send(&state, &ada, "POST", "/deploy/preview", json!({ "slug": "twice", "unit": unit }));
        assert_eq!((code, preview["refused"].as_str()), (200, refused), "{what}: {preview}");
        let (code, said) = send(&state, &ada, "POST", "/deploy", json!({ "slug": "twice", "unit": unit, "document": document, "acknowledge": preview["needs_approval"] }));
        match refused {
            Some(why) => assert_eq!((code, said["error"].as_str()), (400, Some(why)), "{what}"),
            None => assert_eq!(code, 200, "{what}: {said}"),
        }
    }
}

const SYNC: &str = r#"import { action, every, flow, route } from "holon"
import state from "state"
import crm from "host:crm.example.com"
import notes from "folder:notes"

flow("sync", { port: 18431 })

every("1h", () => {
  state.summary = 3
})

export const push = action(async () => {
  await crm.post("/rows", { body: JSON.stringify(state.summary) })
})

route.get("/stats", () => {
  return state.summary
})

route.get("/notes", () => {
  return notes["today.txt"]
})

route.get("/hello", () => {
  return "hello"
})
"#;

/// H9: what an agent's version sends out of a flow, through code it changed or code fed by it,
/// waits for a person, as a secret it reads does: a person let the old code send there, not what
/// the agent put in its place. Each row changes one handler of `sync` as deployed by ada: what
/// `push` sends the CRM; what the clock keeps, which `push` sends; what `/notes` answers its
/// callers from a folder the host binds. What `/stats` answers from the flow's own state, which
/// the agent reads anyway, a route that answers nothing kept, and the same code deploy, where a
/// person let it take an agent's versions at once. What the host grants without asking is not
/// asked again.
#[test]
fn an_agents_change_to_what_a_flow_sends_out_waits_for_a_person() {
    let (state, book, ada) = server();
    let scout = agent(&book, "scout");
    let dir = std::env::temp_dir().join(format!("holon-resent-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    state.folders.bind("notes", dir.to_str().unwrap(), crate::folders::FolderAccess::Read, crate::folders::Keep::Bytes, None, None).unwrap();
    let base = stage(&state, &ada, SYNC);
    assert_eq!(deploy_as(&state, &ada, "sync", &base, false).0, 200);
    at_once(&state, &ada, "sync");
    let crm = "HttpRequest(\"crm.example.com\")";
    let rows: [(&str, &str, &str, &[&str]); 6] = [
        ("what push sends", "JSON.stringify(state.summary)", "JSON.stringify({ all: state.summary })", &[crm]),
        ("what the clock keeps, which push sends", "state.summary = 3", "state.summary = 4", &[crm]),
        ("what /notes answers from the folder", "return notes[\"today.txt\"]", "return notes.list({ limit: 50 })", &["NetworkIngress"]),
        ("what /stats answers from the state", "return state.summary", "return { summary: state.summary }", &[]),
        ("a route that answers nothing kept", "return \"hello\"", "return \"hi\"", &[]),
        ("the same code", "", "", &[]),
    ];
    for (what, from, to, needs) in rows {
        assert_eq!(deploy_as(&state, &ada, "sync", &base, false).0, 200, "{what}");
        let unit = stage(&state, &ada, &SYNC.replace(from, to));
        assert_eq!(review(&state, &scout, "sync", &unit).0, json!(needs), "{what}");
        let (code, answer) = deploy_as(&state, &scout, "sync", &unit, true);
        match needs {
            [] => assert_eq!(code, 200, "{what}: {answer}"),
            _ => assert_eq!((code, &answer["proposal"]["needs"]), (202, &json!(needs)), "{what}: {answer}"),
        }
    }
    let unit = stage(&state, &ada, &SYNC.replace("JSON.stringify(state.summary)", "JSON.stringify({ all: state.summary })"));
    let (_, lines, _) = review(&state, &scout, "sync", &unit);
    assert!(lines.contains(&"  + host crm.example.com: call — asked again: approved by confirming".to_string()), "{lines:#?}");

    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nauto = [\"HttpRequest(*)\"]\n").unwrap());
    assert_eq!(deploy_as(&state, &ada, "sync", &base, false).0, 200);
    let unit = stage(&state, &ada, &SYNC.replace("JSON.stringify(state.summary)", "JSON.stringify({ all: state.summary })"));
    assert_eq!(review(&state, &scout, "sync", &unit).0, json!([]), "the host grants any host without asking");
    assert_eq!(deploy_as(&state, &scout, "sync", &unit, true).0, 200);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `slug`'s flow, which pays at an approval point `payout` asked `{asked}`, with `settings` as
/// its flow(…) line writes them.
fn pay(slug: &str, settings: &str, asked: &str) -> String {
    format!(
        r#"import {{ action, approval, flow }} from "holon"

flow("{slug}"{settings})

export const pay = action(async (order) => {{
  const decision = await approval("payout", {asked})
  return decision.approved
}})
"#
    )
}

/// §9: the review says an approval is not ready when its steps can't all be taken here: a step
/// takes a principal of its kind bound to its role where the approval is (as the version's tags
/// would make it), not disabled, one principal a step and one party a step (an agent is one party
/// with whoever owns it or made its token, the audit's R18); whoever deploys it, and their party,
/// take none on a host that keeps them out; nor do whoever starts its run, here anyone who may run
/// its action, and their party, but for a step that says `starter: true` on a host that lets one
/// (then refusing the version), which the starter's party fills, one principal each. Each row:
/// a flow, who is bound to what on it, and what the review says of its approval as ada: why it is
/// not ready, and which of its runs would wait there undecided (every run, or some runs: those its
/// starter takes no step of, which the editor's deploy doesn't ask about again).
#[test]
fn an_approval_whose_steps_no_one_here_can_take_is_not_ready() {
    let (state, book, ada) = server();
    for name in ["bob", "dee", "cy", "pat", "eve"] {
        person(&book, name);
    }
    agent(&book, "scout");
    // kit, fay's agent, its token hers: one party with her.
    person(&book, "fay");
    book.add("kit", Kind::Agent, "agent", "fay", "ada").unwrap();
    book.create_token("kit", "kit", "fay").unwrap();
    for (role, kind) in [("approver", "person"), ("reviewer", "person"), ("classifier", "agent")] {
        let (code, said) = send(&state, &ada, "PUT", &format!("/roles/{role}"), json!({ "allows": ["run.decide"], "kinds": [kind] }));
        assert_eq!(code, 200, "{said}");
    }
    let slug = |s: &str| Selector::Slug(s.into());
    bind(&book, "cy", "approver", Selector::Tag("payments".into()));
    for s in ["p1", "p2", "p3", "p4", "p8"] {
        bind(&book, "dee", "approver", slug(s));
    }
    bind(&book, "bob", "reviewer", slug("p5"));
    bind(&book, "bob", "approver", slug("p5"));
    bind(&book, "ada", "approver", slug("p6"));
    bind(&book, "ada", "approver", slug("p10"));
    bind(&book, "pat", "editor", slug("p7"));
    bind(&book, "pat", "approver", slug("p7"));
    bind(&book, "eve", "approver", slug("p8"));
    book.set_disabled("eve", true).unwrap();
    for s in ["p11", "p12", "p13"] {
        bind(&book, "kit", "classifier", slug(s));
    }
    bind(&book, "fay", "approver", slug("p11"));
    bind(&book, "dee", "approver", slug("p12"));
    bind(&book, "fay", "editor", slug("p12"));
    bind(&book, "fay", "approver", slug("p13"));
    bind(&book, "fay", "editor", slug("p13"));
    bind(&book, "scout", "classifier", slug("p14"));

    let tagged = ", { tags: [\"payments\"] }";
    let one = "{ steps: [{ role: \"approver\" }], expires: \"1h\" }";
    let two = "{ steps: [{ role: \"approver\", people: 2 }], expires: \"1h\" }";
    let none = |role: &str| format!("{role}: no person holds it here");
    let theirs = "{ steps: [{ role: \"approver\", starter: true }], expires: \"1h\" }";
    let classified = "{ steps: [{ role: \"classifier\", kind: \"agent\" }, { role: \"approver\" }], expires: \"1h\" }";
    let both_theirs = "{ steps: [{ role: \"classifier\", kind: \"agent\", starter: true }, { role: \"approver\", starter: true }], expires: \"1h\" }";
    let class = "{ steps: [{ role: \"classifier\", kind: \"agent\" }], expires: \"1h\" }";
    let parties = "its steps take 2 principals of different parties, one a step, and fewer can here (an agent or a service is one party with whoever owns it or made its token)";
    // What, the flow, its settings, its approval, whether the host keeps the deployer out, and
    // lets a step take whoever started the run, and why the review says it is not ready (none: it
    // is), with which of its runs would wait there.
    let every = |why: &str| Some(("every run", why.to_string()));
    let some = |why: String| Some(("some runs", why));
    type Row<'a> = (&'a str, &'a str, &'a str, &'a str, bool, bool, Option<(&'a str, String)>);
    let rows: [Row; 18] = [
        ("one approver, and dee holds it", "p1", "", one, false, true, None),
        ("two approvers, and dee alone holds it", "p2", "", two, false, true, every("approver: 1 person holds it here; it needs 2")),
        ("two, and the version's tag brings cy", "p3", tagged, two, false, true, None),
        ("two, untagged, without cy", "p3", "", two, false, true, every("approver: 1 person holds it here; it needs 2")),
        ("an agent's step no agent holds", "p4", "", "{ steps: [{ role: \"classifier\", kind: \"agent\" }, { role: \"approver\" }], expires: \"1h\" }", false, true, every("classifier: no agent holds it here")),
        ("two steps, bob alone for both", "p5", "", "{ steps: [{ role: \"reviewer\" }, { role: \"approver\" }], expires: \"1h\" }", false, true, every("its steps take 2 different principals, one a step, and fewer can here")),
        ("ada deploys, the one approver, who may start a run", "p6", "", one, false, true, some(format!("a run ada starts can't be decided, as they take no step of it: {}", none("approver")))),
        ("ada deploys, kept out", "p6", "", one, true, true, every(&none("approver"))),
        ("pat alone approves, and may start a run", "p7", "", one, false, true, some(format!("a run pat starts can't be decided, as they take no step of it: {}", none("approver")))),
        ("eve, disabled, doesn't count", "p8", "", two, false, true, every("approver: 1 person holds it here; it needs 2")),
        ("two people, of whom only ada decides here", "p9", "", "{ people: 2, expires: \"1h\" }", false, true, every("its step: 1 person may decide it here; it needs 2")),
        ("ada, the one approver, may take a step that says so", "p10", "", theirs, false, true, None),
        ("not on a host that lets no step take whoever started it", "p10", "", theirs, false, false, some(format!("a run ada starts can't be decided, as they take no step of it: {}", none("approver")))),
        ("kit classifies, and fay, its owner, alone approves: one party", "p11", "", classified, false, true, every(parties)),
        ("fay may start a run, and kit, hers, alone classifies", "p12", "", classified, false, true, some("a run fay starts can't be decided, as they and kit take no step of it: classifier: no agent holds it here".into())),
        ("fay's party fills both steps that take it, and only in her runs", "p13", "", both_theirs, false, true, some(format!("a run ada starts can't be decided, as they take no step of it: {parties}"))),
        ("scout, ada's, alone classifies, and ada may start a run", "p14", "", class, false, true, some("a run ada starts can't be decided, as they and scout take no step of it: classifier: no agent holds it here".into())),
        ("scout, ada's, with ada kept out as the deployer", "p14", "", class, true, true, every(&none("approver").replace("approver: no person", "classifier: no agent"))),
    ];
    for (what, s, settings, asked, exclude_deployer, starter, said) in rows {
        let (code, answer) = send(&state, &ada, "PUT", "/access/approvals", json!({ "exclude_deployer": exclude_deployer, "agents": true, "starter": starter, "why": what }));
        assert_eq!(code, 200, "{answer}");
        let unit = stage(&state, &ada, &pay(s, settings, asked));
        let (_, _, touches) = review(&state, &ada, s, &unit);
        let row = touches.as_array().unwrap().iter().find(|r| r["kind"] == "approval").cloned().unwrap();
        assert_eq!((row["waits"].as_str(), row["ready"].as_str()), said.as_ref().map_or((None, None), |(runs, why)| (Some(*runs), Some(why.as_str()))), "{what}: {row}");
        // A step that lets the starter in, on a host that lets none: refused, saying how to let it.
        let (_, preview) = send(&state, &ada, "POST", "/deploy/preview", json!({ "slug": s, "unit": unit }));
        let refused = (!starter && asked == theirs).then_some(
            "this host lets no step take whoever started the run, and \"payout\" asks for a person holding approver, who may be whoever started the run, or acting for them (holon access approvals --starter on)",
        );
        assert_eq!(preview["refused"].as_str(), refused, "{what}");
    }
}
