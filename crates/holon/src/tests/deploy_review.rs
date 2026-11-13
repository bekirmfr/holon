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

/// `unit` deployed as `slug` by `token`'s holder: a person approving what the review says it
/// needs, an agent approving nothing.
fn deploy_as(state: &AppState, token: &str, slug: &str, unit: &str, by_agent: bool) -> (u16, Value) {
    let needs = if by_agent { json!([]) } else { review(state, token, slug, unit).0 };
    send(state, token, "POST", "/deploy", json!({ "slug": slug, "unit": unit, "document": {}, "acknowledge": needs }))
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
/// can; once `payments` lets agents run payout, the agent deploys it.
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
    let (code, answer) = deploy_as(&state, &scout, "promo", &promo, true);
    assert_eq!(code, 200, "agents may run payout now: {answer}");
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
/// the agent reads anyway, a route that answers nothing kept, and the same code deploy. What the
/// host grants without asking is not asked again.
#[test]
fn an_agents_change_to_what_a_flow_sends_out_waits_for_a_person() {
    let (state, book, ada) = server();
    let scout = agent(&book, "scout");
    let dir = std::env::temp_dir().join(format!("holon-resent-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    state.folders.bind("notes", dir.to_str().unwrap(), crate::folders::FolderAccess::Read, crate::folders::Keep::Bytes, None, None).unwrap();
    let base = stage(&state, &ada, SYNC);
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
/// would make it), not disabled, one principal a step; whoever deploys it takes none on a host that
/// keeps them out; nor does whoever starts its run, here anyone who may run its action. Each row:
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
    bind(&book, "pat", "editor", slug("p7"));
    bind(&book, "pat", "approver", slug("p7"));
    bind(&book, "eve", "approver", slug("p8"));
    book.set_disabled("eve", true).unwrap();

    let tagged = ", { tags: [\"payments\"] }";
    let one = "{ steps: [{ role: \"approver\" }], expires: \"1h\" }";
    let two = "{ steps: [{ role: \"approver\", people: 2 }], expires: \"1h\" }";
    let none = |role: &str| format!("{role}: no person holds it here");
    // What, the flow, its settings, its approval, whether the host keeps the deployer out, and why
    // the review says it is not ready (none: it is), with which of its runs would wait there.
    let every = |why: &str| Some(("every run", why.to_string()));
    let some = |why: String| Some(("some runs", why));
    type Row<'a> = (&'a str, &'a str, &'a str, &'a str, bool, Option<(&'a str, String)>);
    let rows: [Row; 11] = [
        ("one approver, and dee holds it", "p1", "", one, false, None),
        ("two approvers, and dee alone holds it", "p2", "", two, false, every("approver: 1 person holds it here; it needs 2")),
        ("two, and the version's tag brings cy", "p3", tagged, two, false, None),
        ("two, untagged, without cy", "p3", "", two, false, every("approver: 1 person holds it here; it needs 2")),
        ("an agent's step no agent holds", "p4", "", "{ steps: [{ role: \"classifier\", kind: \"agent\" }, { role: \"approver\" }], expires: \"1h\" }", false, every("classifier: no agent holds it here")),
        ("two steps, bob alone for both", "p5", "", "{ steps: [{ role: \"reviewer\" }, { role: \"approver\" }], expires: \"1h\" }", false, every("its steps take 2 different principals, one a step, and fewer can here")),
        ("ada deploys, the one approver, who may start a run", "p6", "", one, false, some(format!("a run ada starts can't be decided, as they take no step of it: {}", none("approver")))),
        ("ada deploys, kept out", "p6", "", one, true, every(&none("approver"))),
        ("pat alone approves, and may start a run", "p7", "", one, false, some(format!("a run pat starts can't be decided, as they take no step of it: {}", none("approver")))),
        ("eve, disabled, doesn't count", "p8", "", two, false, every("approver: 1 person holds it here; it needs 2")),
        ("two people, of whom only ada decides here", "p9", "", "{ people: 2, expires: \"1h\" }", false, every("its step: 1 person may decide it here; it needs 2")),
    ];
    for (what, s, settings, asked, exclude_deployer, said) in rows {
        let (code, answer) = send(&state, &ada, "PUT", "/access/approvals", json!({ "exclude_deployer": exclude_deployer, "agents": true, "why": what }));
        assert_eq!(code, 200, "{answer}");
        let unit = stage(&state, &ada, &pay(s, settings, asked));
        let (_, _, touches) = review(&state, &ada, s, &unit);
        let row = touches.as_array().unwrap().iter().find(|r| r["kind"] == "approval").cloned().unwrap();
        assert_eq!((row["waits"].as_str(), row["ready"].as_str()), said.as_ref().map_or((None, None), |(runs, why)| (Some(*runs), Some(why.as_str()))), "{what}: {row}");
    }
}
