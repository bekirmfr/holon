//! What a binding at a flow or at an approval point lets its holder do, through the real gate and
//! handlers (docs/L4s/roles-and-bindings-plan.md, step 3b): there, and nowhere else; the lists
//! keep what its holder may view; a binding that has expired holds nothing; and what is bound is
//! listed, changed and kept as versions through its routes.

use super::access_table::{ask, deploy, error_of};
use super::*;
use crate::accounts::Accounts;
use access::Role;
use serde_json::{json, Value};

const PW: &str = "correct horse battery";

/// Pays at an approval point, `payout`, that one person decides.
const PAY: &str = r#"import { action, approval, flow } from "holon"

flow("pay")

export const pay = action(async (order) => {
  const decision = await approval("payout", { expires: "1h" })
  return decision.approved
})
"#;

/// Who signs in: ada, the admin; bob, dee and cy, people who hold viewer on the host until a test
/// binds them otherwise.
struct Tokens {
    ada: String,
    bob: String,
    dee: String,
    cy: String,
}

/// A server with `d` and `e` deployed from `PAY`, each saved as a flow of its name, and a run of
/// `d` waiting at its approval point; then accounts, with `Tokens`' people. Answered with the
/// run's id and the unit's.
fn server() -> (AppState, Tokens, String, String) {
    let mut state = test_state();
    let unit = to_hex(&stage_text(&state, PAY));
    for slug in ["d", "e"] {
        assert_eq!(deploy(&state, None, slug, &unit, json!({})).0, 200);
        assert_eq!(ask(&state, None, "POST", "/flows", serde_json::to_vec(&json!({ "name": slug, "document": {} })).unwrap()).0, 200);
    }
    assert!(run_action_named(&state, "d", "pay", json!({})).is_success());
    let runs: Value = serde_json::from_str(&ask(&state, None, "GET", "/deployments/d/runs", vec![]).1).unwrap();
    let run = runs["runs"][0]["run_id"].as_str().unwrap().to_string();
    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    let person = |name: &str| book.accept(&book.invite(name, Role::Viewer, None, "ada").unwrap(), PW).unwrap().0;
    let tokens = Tokens { ada, bob: person("bob"), dee: person("dee"), cy: person("cy") };
    state.access = access::Access::new(book, vec![], vec![]);
    (state, tokens, run, unit)
}

/// `method` `path` with `body` (none when null) as the holder of `token`: its status and its body.
fn send(state: &AppState, token: &str, method: &str, path: &str, body: Value) -> (u16, String) {
    ask(state, Some(token), method, path, if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() })
}

/// `name`, holding `role` at `scope` only: bound there by ada, and unbound from the host.
fn only(state: &AppState, ada: &str, name: &str, role: &str, scope: Value) {
    let (code, said) = send(state, ada, "POST", "/bindings", json!({ "who": { "principal": name }, "role": role, "scope": scope }));
    assert_eq!(code, 200, "{said}");
    let host = state.access.accounts().unwrap().bindings_of(name).into_iter().find(|b| b.scope == crate::bindings::Scope::Host).unwrap();
    let (code, said) = send(state, ada, "DELETE", &format!("/bindings/{}", host.id), Value::Null);
    assert_eq!(code, 204, "{said}");
}

/// What `path`'s list holds, by `key` and then `field`, as `token`'s holder is answered it.
fn listed(state: &AppState, token: &str, path: &str, key: Option<&str>, field: &str) -> Vec<String> {
    let (code, said) = send(state, token, "GET", path, Value::Null);
    assert_eq!(code, 200, "{path}: {said}");
    let v: Value = serde_json::from_str(&said).unwrap();
    let list = key.map_or(&v, |k| &v[k]);
    list.as_array().unwrap().iter().map(|e| e[field].as_str().unwrap().to_string()).collect()
}

/// `(status, error)` of an answer.
fn refusal((code, said): (u16, String)) -> (u16, String) {
    (code, error_of(&said))
}

/// No role at the flow: the words a refusal says.
fn none_on(name: &str, place: &str, action: &str) -> (u16, String) {
    (403, format!("{name} may not do that: no role {name} holds {place} holds {action}"))
}

/// bob, `editor` on `d` only: views, deploys, stops and decides `d`, and is refused on `e` in
/// words naming it; the lists keep `d`, and the activity what was done to it. A flow he saves that
/// no one deployed is his wherever he holds `flow.edit` (D18), so he drafts, opens and deletes it;
/// he doesn't save over `e`. cy, a viewer on the host, sees every flow as before.
#[test]
fn a_binding_at_a_flow_lets_its_holder_act_there_and_nowhere_else() {
    let (state, t, run, unit) = server();
    only(&state, &t.ada, "bob", "editor", json!({ "flows": { "slug": "d" } }));
    let bob = |method: &str, path: &str, body: Value| send(&state, &t.bob, method, path, body);

    assert_eq!(listed(&state, &t.bob, "/deployments", Some("deployments"), "slug"), ["d"]);
    assert_eq!(listed(&state, &t.bob, "/flows", None, "name"), ["d"]);
    assert_eq!(listed(&state, &t.bob, "/summary", Some("deployments"), "slug"), ["d"]);
    assert_eq!(bob("GET", "/deployments/d/runs", Value::Null).0, 200);
    assert_eq!(refusal(bob("GET", "/deployments/e/runs", Value::Null)), none_on("bob", "on \"e\"", "run.view"));
    assert_eq!(refusal(bob("GET", "/flows/e", Value::Null)), none_on("bob", "on \"e\"", "flow.view"));
    assert_eq!(refusal(bob("POST", "/deployments/e/stop", Value::Null)), none_on("bob", "on \"e\"", "flow.lifecycle"));
    assert_eq!(refusal(bob("POST", "/deploy", json!({ "slug": "e", "unit": unit }))), none_on("bob", "on \"e\"", "flow.propose"));
    assert_eq!(refusal(bob("POST", "/deploy/preview", json!({ "slug": "e", "unit": unit }))), none_on("bob", "on \"e\"", "flow.propose"));
    assert_eq!(refusal(bob("POST", "/deployments/e/runs/r/decide", json!({ "name": "payout", "approve": true }))), none_on("bob", "on \"e\"", "run.decide"));
    assert_eq!(refusal(bob("GET", "/bindings", Value::Null)), none_on("bob", "on this host", "people.view"));

    let (code, said) = bob("POST", &format!("/deployments/d/runs/{run}/decide"), json!({ "name": "payout", "approve": true }));
    assert_eq!(code, 200, "{said}");
    assert_eq!(deploy(&state, Some(&t.bob), "d", &unit, json!({})).0, 200);
    assert_eq!(bob("POST", "/deployments/d/stop", Value::Null).0, 200);

    // His draft, and someone else's flow.
    assert_eq!(bob("POST", "/flows", json!({ "name": "x", "document": {} })).0, 200);
    assert_eq!(listed(&state, &t.bob, "/flows", None, "name"), ["d", "x"]);
    assert_eq!(bob("GET", "/flows/x", Value::Null).0, 200);
    assert_eq!(refusal(bob("POST", "/flows", json!({ "name": "e", "document": {} }))), none_on("bob", "on \"e\"", "flow.edit"));
    assert_eq!(bob("DELETE", "/flows/x", Value::Null).0, 200);

    let activity: Value = serde_json::from_str(&bob("GET", "/activity", Value::Null).1).unwrap();
    let targets: Vec<&str> = activity["entries"].as_array().unwrap().iter().map(|e| e["target"].as_str().unwrap()).collect();
    assert!(!targets.is_empty() && targets.iter().all(|t| *t == "d" || t.starts_with("d run ")), "{targets:?}");
    assert_eq!(listed(&state, &t.cy, "/deployments", Some("deployments"), "slug"), ["d", "e"]);
    let all: Value = serde_json::from_str(&send(&state, &t.cy, "GET", "/activity", Value::Null).1).unwrap();
    assert!(all["entries"].as_array().unwrap().iter().any(|e| e["target"] == "e"), "{all}");
}

/// dee, `editor` at `d`'s approval point `payout` only: decides it, and nothing else: not another
/// point, not `e`'s, and she doesn't view the flow's runs or list the flows (D25). A role that
/// holds nothing at an approval point isn't bound there.
#[test]
fn a_binding_at_an_approval_point_decides_that_point_only() {
    let (state, t, run, _) = server();
    let point = json!({ "approval": { "flow": { "slug": "d" }, "name": "payout" } });
    only(&state, &t.ada, "dee", "editor", point.clone());
    let dee = |method: &str, path: &str, body: Value| send(&state, &t.dee, method, path, body);
    let decide = |slug: &str, name: &str| dee("POST", &format!("/deployments/{slug}/runs/{run}/decide"), json!({ "name": name, "approve": true }));

    assert_eq!(refusal(decide("d", "other")), none_on("dee", "for \"d\"'s approval \"other\"", "run.decide"));
    assert_eq!(refusal(decide("e", "payout")), none_on("dee", "on \"e\"", "run.decide"));
    assert_eq!(dee("POST", &format!("/deployments/d/runs/{run}/decide"), json!([])).0, 400, "a malformed body before a refusal");
    let (code, said) = decide("d", "payout");
    assert_eq!(code, 200, "{said}");
    assert!(said.contains(r#""outcome":"approved""#), "{said}");
    assert_eq!(refusal(dee("GET", "/deployments/d/runs", Value::Null)), none_on("dee", "on \"d\"", "run.view"));
    assert_eq!(refusal(dee("GET", "/deployments", Value::Null)), none_on("dee", "on any flow", "flow.view"));

    let viewer = send(&state, &t.ada, "POST", "/bindings", json!({ "who": { "principal": "cy" }, "role": "viewer", "scope": point }));
    assert_eq!(refusal(viewer), (400, "viewer holds nothing for \"d\"'s approval \"payout\": bind a role that holds something there".into()));
}

/// What is bound is listed with whether it has expired, and each change is a version with who
/// made it, what it changed and why; a binding that has expired holds nothing, and one isn't made
/// already expired; the last admin's binding isn't unbound (D15); and each change is in the
/// activity log.
#[test]
fn what_is_bound_is_listed_versioned_and_expires() {
    let (state, t, _, _) = server();
    let ada = |method: &str, path: &str, body: Value| send(&state, &t.ada, method, path, body);
    let accounts = Arc::clone(state.access.accounts().unwrap());
    let now = crate::deploy::epoch_now();
    let on = |slug: &str| crate::bindings::Scope::Flows(crate::bindings::Selector::Slug(slug.into()));

    let (code, said) = ada("POST", "/bindings", json!({ "who": { "principal": "bob" }, "role": "editor", "scope": { "flows": { "slug": "d" } }, "expires_at": now + 3600, "why": "on call this week" }));
    assert_eq!(code, 200, "{said}");
    let made: Value = serde_json::from_str(&said).unwrap();
    assert_eq!((made["binding"]["role"].as_str(), made["binding"]["expired"].as_bool()), (Some("editor"), Some(false)));
    accounts.bind("bob", Role::Editor, on("e"), Some(now - 1), "ada", None).unwrap();
    assert_eq!(send(&state, &t.bob, "POST", "/deployments/d/stop", Value::Null).0, 200, "until it expires");
    assert_eq!(refusal(send(&state, &t.bob, "POST", "/deployments/e/stop", Value::Null)), (403, "bob may not do that: a viewer only reads".into()), "expired");
    let bobs: Value = serde_json::from_str(&ada("GET", "/bindings?who=bob&role=editor", Value::Null).1).unwrap();
    let expired: Vec<(String, bool)> = bobs["bindings"].as_array().unwrap().iter().map(|b| (b["scope"]["flows"]["slug"].as_str().unwrap().to_string(), b["expired"].as_bool().unwrap())).collect();
    assert_eq!(expired, [("d".to_string(), false), ("e".to_string(), true)]);
    let past = ada("POST", "/bindings", json!({ "who": { "principal": "bob" }, "role": "editor", "scope": "host", "expires_at": now - 1 }));
    assert_eq!(refusal(past).0, 400);
    let on_e = bobs["bindings"][1]["id"].as_u64().unwrap();
    assert_eq!(ada("DELETE", &format!("/bindings/{on_e}?why=over"), Value::Null).0, 204);
    assert_eq!(ada("DELETE", &format!("/bindings/{on_e}"), Value::Null).0, 404);

    let adas = accounts.bindings_of("ada")[0].id;
    let (code, said) = ada("DELETE", &format!("/bindings/{adas}"), Value::Null);
    assert_eq!((code, error_of(&said)), (409, "that would leave no admin: make someone else an admin first".into()));

    let versions: Value = serde_json::from_str(&ada("GET", "/access/versions", Value::Null).1).unwrap();
    let said: Vec<(&str, Option<&str>)> = versions["versions"].as_array().unwrap().iter().take(3).map(|v| (v["what"].as_str().unwrap(), v["why"].as_str())).collect();
    let until = |at: u64| crate::cmdline::when(Some(at));
    assert_eq!(said, [
        ("unbound editor from bob on \"e\"", Some("over")),
        (&*format!("bound editor to bob on \"e\", until {}", until(now - 1)), None),
        (&*format!("bound editor to bob on \"d\", until {}", until(now + 3600)), Some("on call this week")),
    ]);
    let newest = versions["versions"][0]["version"].as_u64().unwrap();
    let whole: Value = serde_json::from_str(&ada("GET", &format!("/access?version={}", newest - 1), Value::Null).1).unwrap();
    assert_eq!(whole["bindings"].as_array().unwrap().len(), accounts.access_document().bindings.len() + 1, "the one unbound since");
    assert_eq!(ada("GET", "/access?version=999", Value::Null).0, 404);
    let activity: Value = serde_json::from_str(&ada("GET", "/activity", Value::Null).1).unwrap();
    let done: Vec<String> = activity["entries"].as_array().unwrap().iter().map(|e| format!("{} {}", e["action"].as_str().unwrap(), e["target"].as_str().unwrap())).collect();
    assert!(done.contains(&"unbound editor from bob on \"e\"".to_string()) && done.contains(&"bound editor to bob on \"d\"".to_string()), "{done:?}");
}
