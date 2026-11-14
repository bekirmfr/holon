//! What a binding at a flow or at an approval point lets its holder do, through the real gate and
//! handlers (docs/L4s/roles-and-bindings-plan.md, step 3b): there, and nowhere else; the lists
//! keep what its holder may view; a binding that has expired holds nothing; and what is bound is
//! listed, changed and kept as versions through its routes. A binding to a group holds for its
//! members, a role an admin defines holds what it allows, neither is removed while bound, and
//! `/auth/me/can` answers what the routes ask.

use super::access_table::{ask, deploy, error_of};
use super::*;
use crate::accounts::{Accounts, Kind};
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
    let person = |name: &str| book.accept(&book.invite(name, "viewer", None, "ada").unwrap(), PW).unwrap().0;
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

    // A unit is d's code (and e's), and he reads it; e's alone, or one no flow places, he is
    // answered as for a hash nothing has. cy, viewing every flow, reads each.
    let stage = |text: &str| {
        let plan = holon_author::agent::plan(text).unwrap();
        for u in &plan.units {
            assert_eq!(ask(&state, Some(&t.ada), "POST", "/unit", u.clone()).0, 200);
        }
        plan.unit
    };
    let only_e = stage(&PAY.replace("\"1h\"", "\"2h\""));
    assert_eq!(deploy(&state, Some(&t.ada), "e", &only_e, json!({})).0, 200);
    let staged = stage(&PAY.replace("\"1h\"", "\"3h\""));
    for (hash, his) in [(&unit, true), (&only_e, false), (&staged, false), (&"0".repeat(64), false)] {
        assert_eq!(bob("GET", &format!("/unit/{hash}"), Value::Null).0, if his { 200 } else { 404 }, "bob, {hash}");
        assert_eq!(send(&state, &t.cy, "GET", &format!("/unit/{hash}"), Value::Null).0, if hash.starts_with('0') { 404 } else { 200 }, "cy, {hash}");
    }
    let all: Value = serde_json::from_str(&send(&state, &t.cy, "GET", "/activity", Value::Null).1).unwrap();
    assert!(all["entries"].as_array().unwrap().iter().any(|e| e["target"] == "e"), "{all}");
}

/// dee, `editor` at `d`'s approval point `payout` only: decides it, and nothing else: not another
/// point, not `e`'s; of the flow's runs, only those waiting at that point are listed to dee, and
/// the flows aren't (D25). A role that holds nothing at an approval point isn't bound there.
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
    // The run waiting at the point is listed to dee; once decided, it waits at none of dee's.
    assert_eq!(listed(&state, &t.dee, "/deployments/d/runs", Some("runs"), "run_id"), std::slice::from_ref(&run));
    let (code, said) = decide("d", "payout");
    assert_eq!(code, 200, "{said}");
    assert!(said.contains(r#""outcome":"approved""#), "{said}");
    assert!(listed(&state, &t.dee, "/deployments/d/runs", Some("runs"), "run_id").is_empty());
    assert_eq!(refusal(dee("GET", "/deployments/e/runs", Value::Null)), none_on("dee", "on \"e\"", "run.view"));
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
    accounts.bind(crate::bindings::Subject::Principal("bob".into()), "editor", on("e"), Some(now - 1), "ada", None).unwrap();
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

/// A binding to a group holds for each member whose kind its role allows: bob, a person in
/// `payments`, holds editor on `d` by it, and `/auth/me` says through which group; scout, an agent
/// in it, doesn't, editor being a person's or a service's. Taken out of the group, bob holds
/// nothing by it. A group's members are principals here, and a binding is to a group here.
#[test]
fn a_groups_binding_holds_for_its_members_of_the_kinds_its_role_allows() {
    let (state, t, _, _) = server();
    let ada = |method: &str, path: &str, body: Value| send(&state, &t.ada, method, path, body);
    let accounts = Arc::clone(state.access.accounts().unwrap());
    accounts.add("scout", Kind::Agent, "viewer", "ada", "ada").unwrap();
    let (scout, _) = accounts.create_token("scout", "Scout", "ada").unwrap();

    assert_eq!(ada("PUT", "/groups/payments", json!({ "members": ["scout", "bob", "bob"], "about": "pays out" })).0, 200);
    let (code, said) = ada("POST", "/bindings", json!({ "who": { "group": "payments" }, "role": "editor", "scope": { "flows": { "slug": "d" } } }));
    assert_eq!(code, 200, "{said}");
    assert_eq!(send(&state, &t.bob, "POST", "/deployments/d/pause", Value::Null).0, 200);
    let me: Value = serde_json::from_str(&send(&state, &t.bob, "GET", "/auth/me", Value::Null).1).unwrap();
    let held: Vec<(&str, Option<&str>)> = me["bindings"].as_array().unwrap().iter().map(|b| (b["role"].as_str().unwrap(), b["group"].as_str())).collect();
    assert_eq!(held, [("viewer", None), ("editor", Some("payments"))]);
    let can: Value = serde_json::from_str(&send(&state, &scout, "GET", "/auth/me/can?actions=run.view,flow.lifecycle&flow=d", Value::Null).1).unwrap();
    assert_eq!(can, json!({ "run.view": true, "flow.lifecycle": "scout may not do that: a viewer only reads" }));
    let groups: Value = serde_json::from_str(&ada("GET", "/groups", Value::Null).1).unwrap();
    assert_eq!(groups["groups"], json!([{ "name": "payments", "about": "pays out", "members": ["bob", "scout"], "bound": 1 }]));

    assert_eq!(ada("PUT", "/groups/payments", json!({ "members": ["scout"] })).0, 200);
    assert_eq!(refusal(send(&state, &t.bob, "POST", "/deployments/d/run", Value::Null)), (403, "bob may not do that: a viewer only reads".into()));
    assert_eq!(refusal(ada("PUT", "/groups/payments", json!({ "members": ["nobody"] }))), (400, "no principal \"nobody\": a group's members are principals here".into()));
    assert_eq!(refusal(ada("POST", "/bindings", json!({ "who": { "group": "nope" }, "role": "viewer", "scope": "host" }))), (400, "no group \"nope\"".into()));
}

/// A role an admin defines holds what it allows and nothing else, to the kinds it names: dee,
/// `approver` on `d` only, decides and views its runs, and is told no role dee holds there holds
/// anything more (D16). It changes for every binding of it, and isn't changed to refuse whom it is
/// bound to. Its author holds on the host whatever it allows (D19); a built-in role's name is
/// taken.
#[test]
fn a_role_an_admin_defines_holds_what_it_allows() {
    let (state, t, run, _) = server();
    let ada = |method: &str, path: &str, body: Value| send(&state, &t.ada, method, path, body);
    let (code, said) = ada("PUT", "/roles/approver", json!({ "allows": ["run.decide", "run.view"], "kinds": ["person"], "about": "decides payouts" }));
    assert_eq!(code, 200, "{said}");
    let roles: Value = serde_json::from_str(&ada("GET", "/roles", Value::Null).1).unwrap();
    let names: Vec<(&str, bool)> = roles["roles"].as_array().unwrap().iter().map(|r| (r["name"].as_str().unwrap(), r["builtin"].as_bool().unwrap())).collect();
    assert_eq!(names, [("viewer", true), ("agent", true), ("editor", true), ("admin", true), ("approver", false)]);
    assert_eq!(roles["roles"][4], json!({ "name": "approver", "about": "decides payouts", "allows": ["run.view", "run.decide"], "kinds": ["person"], "builtin": false, "bound": 0 }));

    // Given on the host as a built-in role is: inviting or setting someone with it.
    let (code, said) = ada("POST", "/people", json!({ "name": "fay", "kind": "person", "role": "approver" }));
    assert_eq!(code, 200, "{said}");
    assert_eq!(refusal(ada("POST", "/people", json!({ "name": "bot", "kind": "agent", "role": "approver" }))), (400, "an agent may not hold approver: it is held by a person".into()));
    assert_eq!(refusal(ada("PATCH", "/people/cy", json!({ "role": "nope" }))), (400, "no role \"nope\": GET /roles lists them".into()));

    only(&state, &t.ada, "dee", "approver", json!({ "flows": { "slug": "d" } }));
    let me: Value = serde_json::from_str(&send(&state, &t.dee, "GET", "/auth/me", Value::Null).1).unwrap();
    assert_eq!(me["bindings"][0]["allows"], json!(["run.view", "run.decide"]), "what it allows, which dee reads here and not in GET /roles");
    let (code, said) = send(&state, &t.dee, "POST", &format!("/deployments/d/runs/{run}/decide"), json!({ "name": "payout", "approve": true }));
    assert_eq!(code, 200, "{said}");
    assert_eq!(send(&state, &t.dee, "GET", "/deployments/d/runs", Value::Null).0, 200);
    assert_eq!(refusal(send(&state, &t.dee, "POST", "/deployments/d/pause", Value::Null)), none_on("dee", "on \"d\"", "flow.lifecycle"));
    assert_eq!(refusal(send(&state, &t.dee, "GET", "/deployments/e/runs", Value::Null)), none_on("dee", "on \"e\"", "run.view"));

    assert_eq!(ada("PUT", "/roles/approver", json!({ "allows": ["run.view"], "kinds": ["person"] })).0, 200);
    let can: Value = serde_json::from_str(&send(&state, &t.dee, "GET", "/auth/me/can?actions=run.decide,run.view&flow=d", Value::Null).1).unwrap();
    assert_eq!(can, json!({ "run.view": true, "run.decide": "dee may not do that: no role dee holds on \"d\" holds run.decide" }));
    assert_eq!(refusal(ada("PUT", "/roles/approver", json!({ "allows": ["run.view"], "kinds": ["agent"] }))), (400, "a person may not hold approver: it is held by an agent".into()));
    assert_eq!(refusal(ada("PUT", "/roles/editor", json!({ "allows": ["run.view"], "kinds": ["person"] }))), (400, "editor is a built-in role: name yours otherwise".into()));
    assert_eq!(refusal(ada("PUT", "/roles/r", json!({ "allows": ["run.fly"], "kinds": ["person"] }))).0, 400);

    // cy defines roles, and holds on the host only a viewer's besides.
    assert_eq!(ada("PUT", "/roles/role-maker", json!({ "allows": ["role.define", "role.view"], "kinds": ["person"] })).0, 200);
    assert_eq!(ada("POST", "/bindings", json!({ "who": { "principal": "cy" }, "role": "role-maker", "scope": "host" })).0, 200);
    let cy = |body: Value| refusal(send(&state, &t.cy, "PUT", "/roles/mine", body));
    assert_eq!(cy(json!({ "allows": ["flow.deploy"], "kinds": ["person"] })), (403, "mine would allow flow.deploy, which you don't hold on this host: a role allows only what its author holds".into()));
    assert_eq!(cy(json!({ "allows": ["run.view", "role.view"], "kinds": ["person"] })).0, 200);
}

/// A role or a group still bound isn't removed (the plan's D28): the admin is told how many
/// bindings hold it, unbinds them, and then removes it; a built-in role stays. Each change is a
/// version, in words.
#[test]
fn a_bound_role_or_group_is_not_removed() {
    let (state, t, _, _) = server();
    let ada = |method: &str, path: &str, body: Value| send(&state, &t.ada, method, path, body);
    assert_eq!(ada("PUT", "/roles/watcher", json!({ "allows": ["run.view"], "kinds": ["service", "person"] })).0, 200);
    assert_eq!(ada("PUT", "/groups/ops", json!({ "members": ["bob"] })).0, 200);
    let bind = |who: Value, slug: &str| {
        let (code, said) = ada("POST", "/bindings", json!({ "who": who, "role": "watcher", "scope": { "flows": { "slug": slug } } }));
        assert_eq!(code, 200, "{said}");
        serde_json::from_str::<Value>(&said).unwrap()["binding"]["id"].as_u64().unwrap()
    };
    let ids = [bind(json!({ "group": "ops" }), "d"), bind(json!({ "principal": "cy" }), "e")];
    assert_eq!(refusal(ada("DELETE", "/roles/watcher", Value::Null)), (409, "watcher is bound 2 times: unbind them first".into()));
    assert_eq!(refusal(ada("DELETE", "/groups/ops", Value::Null)), (409, "ops is bound once: unbind it first".into()));
    for id in ids {
        assert_eq!(ada("DELETE", &format!("/bindings/{id}"), Value::Null).0, 204);
    }
    assert_eq!(ada("DELETE", "/roles/watcher?why=no%20longer%20needed", Value::Null).0, 204);
    assert_eq!(ada("DELETE", "/groups/ops", Value::Null).0, 204);
    assert_eq!(refusal(ada("DELETE", "/roles/watcher", Value::Null)), (404, "no role \"watcher\"".into()));
    assert_eq!(refusal(ada("DELETE", "/groups/ops", Value::Null)), (404, "no group \"ops\"".into()));
    assert_eq!(refusal(ada("DELETE", "/roles/editor", Value::Null)), (400, "editor is a built-in role: it stays".into()));

    let versions: Value = serde_json::from_str(&ada("GET", "/access/versions", Value::Null).1).unwrap();
    let said: Vec<(&str, Option<&str>)> = versions["versions"].as_array().unwrap().iter().take(8).map(|v| (v["what"].as_str().unwrap(), v["why"].as_str())).collect();
    assert_eq!(said, [
        ("removed the group ops", None),
        ("removed the role watcher", Some("no longer needed")),
        ("unbound watcher from cy on \"e\"", None),
        ("unbound watcher from the group ops on \"d\"", None),
        ("bound watcher to cy on \"e\"", None),
        ("bound watcher to the group ops on \"d\"", None),
        ("made the group ops: bob", None),
        ("defined the role watcher: run.view, held by a service or a person", None),
    ]);
}

/// `GET /auth/me/can` answers what each route's gate answers: for every route that asks `can` of
/// the host, the flows or a flow, and each caller, the answer for its action, on `d` when the
/// route names a flow, is true exactly when the gate lets the caller through, and else the gate's
/// words.
#[test]
fn me_can_answers_what_each_route_asks() {
    use crate::can::{On, Parts, Rule, ROUTES};
    let (state, t, _, _) = server();
    only(&state, &t.ada, "bob", "editor", json!({ "flows": { "slug": "d" } }));
    only(&state, &t.ada, "dee", "editor", json!({ "approval": { "flow": { "slug": "d" }, "name": "payout" } }));
    let accounts = state.access.accounts().map(|a| &**a);
    for token in [&t.ada, &t.bob, &t.dee, &t.cy] {
        let who = state.access.admit(&req_with("GET", "/auth/me", vec![], vec![("Authorization".to_string(), format!("Bearer {token}"))])).unwrap();
        let name = who.name.clone().unwrap();
        for route in ROUTES {
            let Rule::Can(action, on) = route.rule else { continue };
            let (query, parts) = match on {
                On::Principal => continue,
                On::Slug | On::Approvals => ("&flow=d", Parts { slug: Some("d"), name: None }),
                On::Host | On::Flows => ("", Parts::default()),
            };
            let (code, said) = send(&state, token, "GET", &format!("/auth/me/can?actions={}{query}", action.name()), Value::Null);
            assert_eq!(code, 200, "{said}");
            let answered = serde_json::from_str::<Value>(&said).unwrap()[action.name()].clone();
            let gate = crate::can::may(&who, route, &parts, accounts);
            assert_eq!(answered, gate.map_or_else(|r| json!(r.0), |_| json!(true)), "{} {} for {name}", route.method, route.path);
        }
    }
    let (code, said) = send(&state, &t.cy, "GET", "/auth/me/can?actions=flow.fly", Value::Null);
    assert_eq!((code, error_of(&said)), (400, "\"flow.fly\" isn't an action: the catalogue names them as flow.deploy, run.decide…".into()));
}

/// What the editor's buttons are held to (`editor/src/can.test.tsx`, the plan's step 7c): the
/// catalogue, what each route asks, and what `/auth/me/can` answers for every action, with no flow
/// and on `d`, to an admin (ada), an editor (bob) and a viewer (cy) on the host, someone bound at
/// `d`'s approval point alone (dee) and an agent (scout). `HOLON_BLESS=1` writes it again; until
/// then a change to any of them fails here, and the editor's table of callers and buttons then
/// says whether its buttons still follow.
#[test]
fn the_editor_is_held_to_what_can_answers() {
    use crate::can::{Action, Rule, ROUTES};
    let (state, t, _, _) = server();
    let ada = |method: &str, path: &str, body: Value| send(&state, &t.ada, method, path, body);
    assert_eq!(ada("POST", "/bindings", json!({ "who": { "principal": "bob" }, "role": "editor", "scope": "host" })).0, 200);
    only(&state, &t.ada, "dee", "editor", json!({ "approval": { "flow": { "slug": "d" }, "name": "payout" } }));
    assert_eq!(ada("POST", "/people", json!({ "name": "scout", "kind": "agent", "role": "agent" })).0, 200);
    let (code, said) = ada("POST", "/people/scout/tokens", json!({ "label": "the editor's table" }));
    assert_eq!(code, 200, "{said}");
    let scout = serde_json::from_str::<Value>(&said).unwrap()["token"].as_str().unwrap().to_string();
    let all = Action::ALL.map(Action::name).join(",");
    let answers = |token: &str| {
        let at = |query: &str| {
            let (code, said) = send(&state, token, "GET", &format!("/auth/me/can?actions={all}{query}"), Value::Null);
            assert_eq!(code, 200, "{said}");
            serde_json::from_str::<Value>(&said).unwrap()
        };
        json!({ "host": at(""), "flow": at("&flow=d") })
    };
    let routes: serde_json::Map<String, Value> = ROUTES
        .iter()
        .filter_map(|r| match r.rule {
            Rule::Can(action, _) => Some((format!("{} {}", r.method, r.path), json!(action.name()))),
            _ => None,
        })
        .collect();
    let fixture = json!({
        "actions": Action::ALL.map(Action::name),
        "routes": routes,
        "callers": { "admin": answers(&t.ada), "editor": answers(&t.bob), "viewer": answers(&t.cy), "point": answers(&t.dee), "agent": answers(&scout) },
    });
    let text = serde_json::to_string_pretty(&fixture).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../editor/src/can.fixture.json");
    let kept = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    if kept != text && std::env::var_os("HOLON_BLESS").is_some() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    assert!(kept == text, "{} is out of date: HOLON_BLESS=1 cargo test -p holon the_editor_is_held_to_what_can_answers writes it again", path.display());
}

/// No one gives more than they hold (D19), whichever route gives it: cy, holding a role that
/// manages and assigns people on the host and a viewer's besides, doesn't make an admin or an
/// editor by inviting, setting a role or adding to a group bound admin, and doesn't reset or make
/// the tokens of someone holding more, so as to act as them, nor change or remove them. What holds
/// no more than cy holds, cy gives and changes; an owner makes its service's tokens, as the design
/// has it.
#[test]
fn no_one_gives_or_changes_more_than_they_hold() {
    let (state, t, _, _) = server();
    let ada = |method: &str, path: &str, body: Value| send(&state, &t.ada, method, path, body);
    let cy = |method: &str, path: &str, body: Value| send(&state, &t.cy, method, path, body);
    assert_eq!(ada("PUT", "/roles/people-admin", json!({ "allows": ["people.view", "people.manage", "people.assign"], "kinds": ["person"] })).0, 200);
    assert_eq!(ada("POST", "/bindings", json!({ "who": { "principal": "cy" }, "role": "people-admin", "scope": "host" })).0, 200);
    let beyond = |role: &str| (403, format!("{role} holds flow.edit on this host, which you don't: a binding gives only what its author holds where it is made"));

    assert_eq!(refusal(cy("PATCH", "/people/cy", json!({ "role": "admin" }))), beyond("admin"));
    assert_eq!(refusal(cy("POST", "/people", json!({ "name": "eve", "kind": "person", "role": "editor" }))), beyond("editor"));
    assert_eq!(cy("POST", "/people", json!({ "name": "eve", "kind": "person", "role": "viewer" })).0, 200);
    assert_eq!(cy("PATCH", "/people/bob", json!({ "role": "viewer" })).0, 200);

    assert_eq!(ada("PUT", "/groups/admins", json!({ "members": ["ada"] })).0, 200);
    assert_eq!(ada("POST", "/bindings", json!({ "who": { "group": "admins" }, "role": "admin", "scope": "host" })).0, 200);
    let joined = (403, "adding cy to admins gives them admin on this host, which holds flow.edit there, which you don't: a binding gives only what its author holds where it is made".to_string());
    assert_eq!(refusal(cy("PUT", "/groups/admins", json!({ "members": ["ada", "cy"] }))), joined);
    assert_eq!(cy("PUT", "/groups/readers", json!({ "members": ["bob", "cy"] })).0, 200);

    let acts_as = |name: &str, role: &str| (403, format!("{name} holds {role} on this host, which holds flow.edit there, which you don't: you change, or act as, only someone who holds no more than you"));
    assert_eq!(refusal(cy("POST", "/people/ada/reset", Value::Null)), acts_as("ada", "admin"));
    assert_eq!(cy("POST", "/people/dee/reset", Value::Null).0, 200);
    assert_eq!(ada("POST", "/people", json!({ "name": "ci", "kind": "service", "role": "editor" })).0, 200);
    assert_eq!(refusal(cy("POST", "/people/ci/tokens", json!({ "label": "mine now" }))), acts_as("ci", "editor"));
    assert_eq!(ada("POST", "/people/ci/tokens", json!({ "label": "CI" })).0, 200);
    assert_eq!(ada("POST", "/people", json!({ "name": "bot", "kind": "service", "role": "editor", "owner": "bob" })).0, 200);
    assert_eq!(send(&state, &t.bob, "POST", "/people/bot/tokens", json!({ "label": "bob's bot" })).0, 200, "its owner, holding less");

    // Nor change someone holding more, so as to sign in as them, own them, or take from them (the
    // launch review's B4): not the admin's email, which a provider matches, nor whether they are
    // enabled or there; not a service's owner, who then makes its tokens.
    for change in [json!({ "email": "cy@example.com" }), json!({ "disabled": true }), json!({ "role": "viewer" }), json!({ "display": "Ada" })] {
        assert_eq!(refusal(cy("PATCH", "/people/ada", change.clone())), acts_as("ada", "admin"), "{change}");
    }
    assert_eq!(refusal(cy("DELETE", "/people/ada", Value::Null)), acts_as("ada", "admin"));
    assert_eq!(refusal(cy("PATCH", "/people/ci", json!({ "owner": "cy" }))), acts_as("ci", "editor"));
    assert_eq!(cy("PATCH", "/people/dee", json!({ "email": "dee@example.com", "display": "Dee" })).0, 200);
    assert_eq!(cy("PATCH", "/people/cy", json!({ "display": "Cy" })).0, 200, "themself");
}

/// Everyone `world` makes, and what it made, by name: bindings and tokens by id.
struct World {
    state: AppState,
    t: Tokens,
    ids: Vec<(&'static str, String)>,
}

/// `server()`, and cy holding `people-admin` on the host, which allows every action that changes a
/// principal or what it holds, and a viewer's besides; bob, editor on the host; fay, editor through
/// the group `editors` (bob and fay), and dee and cy in `readers`, bound viewer; the roles `ops`
/// (flow.deploy, run.view) and `watch` (run.view); and the services `ci`, editor, and `reader`,
/// viewer, each with a token, owned by ada. Then an access change that gives more waits for one
/// person besides its author, and two of ada's wait: dee made an admin, and given people-admin.
fn world() -> World {
    let (state, t, _, _) = server();
    let ada = |method: &str, path: &str, body: Value| {
        let (code, said) = send(&state, &t.ada, method, path, body);
        assert!((200..300).contains(&code), "{method} {path}: {said}");
        said
    };
    let accounts = Arc::clone(state.access.accounts().unwrap());
    accounts.accept(&accounts.invite("fay", "viewer", None, "ada").unwrap(), PW).unwrap();
    ada("PUT", "/roles/people-admin", json!({ "allows": ["people.view", "people.manage", "people.assign", "role.view", "role.define"], "kinds": ["person"] }));
    ada("POST", "/bindings", json!({ "who": { "principal": "cy" }, "role": "people-admin", "scope": "host" }));
    ada("PATCH", "/people/bob", json!({ "role": "editor" }));
    ada("PUT", "/groups/editors", json!({ "members": ["bob", "fay"] }));
    ada("POST", "/bindings", json!({ "who": { "group": "editors" }, "role": "editor", "scope": "host" }));
    ada("PUT", "/groups/readers", json!({ "members": ["cy", "dee"] }));
    ada("POST", "/bindings", json!({ "who": { "group": "readers" }, "role": "viewer", "scope": "host" }));
    ada("PUT", "/roles/ops", json!({ "allows": ["flow.deploy", "run.view"], "kinds": ["person"] }));
    ada("PUT", "/roles/watch", json!({ "allows": ["run.view"], "kinds": ["person"] }));
    let mut ids = Vec::new();
    for (service, role) in [("ci", "editor"), ("reader", "viewer")] {
        ada("POST", "/people", json!({ "name": service, "kind": "service", "role": role }));
        let made: Value = serde_json::from_str(&ada("POST", &format!("/people/{service}/tokens"), json!({ "label": "x" }))).unwrap();
        ids.push((if service == "ci" { "ci-token" } else { "reader-token" }, made["entry"]["id"].as_str().unwrap().to_string()));
    }
    let host = |name: &str| accounts.bindings_of(name).into_iter().find(|b| b.scope == crate::bindings::Scope::Host).unwrap().id.to_string();
    ids.extend([("bob-editor", host("bob")), ("dee-viewer", host("dee"))]);
    ada("PUT", "/access/approvals", json!({ "changes": 1 }));
    for (change, role) in [("admin-change", "admin"), ("assign-change", "people-admin")] {
        let waits: Value = serde_json::from_str(&ada("POST", "/bindings", json!({ "who": { "principal": "dee" }, "role": role, "scope": "host" }))).unwrap();
        ids.push((change, waits["proposal"]["id"].to_string()));
    }
    World { state, t, ids }
}

/// D19 both ways, for every route that changes a principal or what it holds (the launch review's
/// B4): the route table's every such route has a case here, or says why it changes neither. In
/// each, cy, holding `people-admin` and a viewer's besides, is refused (403) the change to someone
/// holding more than cy (their sign-in, tokens, owner, role, whether they are enabled or there),
/// or giving or taking a binding that holds more, and makes the same change to someone holding no
/// more. Setting the identity provider says who every person here is: only ada, holding what each
/// holds, sets it.
#[test]
fn every_route_that_changes_a_principal_holds_its_caller_to_what_they_hold() {
    use crate::can::{Action, Rule, ROUTES};
    type Ask = (&'static str, &'static str, Value);
    // (route, cy refused, (by ada?, the same to someone holding no more)).
    type Case = ((&'static str, &'static str), Ask, (bool, Ask));
    let cases: [Case; 12] = [
        (("POST", "/people"), ("POST", "/people", json!({ "name": "eve", "kind": "person", "role": "editor" })), (false, ("POST", "/people", json!({ "name": "eve", "kind": "person", "role": "viewer" })))),
        (("POST", "/admin/oidc"), ("POST", "/admin/oidc", json!({ "settings": null })), (true, ("POST", "/admin/oidc", json!({ "settings": null })))),
        (("POST", "/people/{name}/tokens"), ("POST", "/people/ci/tokens", json!({ "label": "mine" })), (false, ("POST", "/people/reader/tokens", json!({ "label": "mine" })))),
        (("DELETE", "/people/{name}/tokens/{id}"), ("DELETE", "/people/ci/tokens/{ci-token}", Value::Null), (false, ("DELETE", "/people/reader/tokens/{reader-token}", Value::Null))),
        (("POST", "/people/{name}/reset"), ("POST", "/people/bob/reset", Value::Null), (false, ("POST", "/people/dee/reset", Value::Null))),
        (("DELETE", "/people/{name}"), ("DELETE", "/people/bob", Value::Null), (false, ("DELETE", "/people/dee", Value::Null))),
        (("PATCH", "/people/{name}"), ("PATCH", "/people/ci", json!({ "owner": "cy" })), (false, ("PATCH", "/people/reader", json!({ "owner": "cy" })))),
        (("POST", "/bindings"), ("POST", "/bindings", json!({ "who": { "principal": "cy" }, "role": "editor", "scope": "host" })), (false, ("POST", "/bindings", json!({ "who": { "principal": "dee" }, "role": "viewer", "scope": { "flows": { "slug": "d" } } })))),
        (("DELETE", "/bindings/{id}"), ("DELETE", "/bindings/{bob-editor}", Value::Null), (false, ("DELETE", "/bindings/{dee-viewer}", Value::Null))),
        (("PUT", "/roles/{name}"), ("PUT", "/roles/ops", json!({ "allows": ["run.view"], "kinds": ["person"] })), (false, ("PUT", "/roles/watch", json!({ "allows": ["run.view"], "kinds": ["person", "service"] })))),
        (("PUT", "/groups/{name}"), ("PUT", "/groups/editors", json!({ "members": ["bob"] })), (false, ("PUT", "/groups/readers", json!({ "members": ["cy"] })))),
        // Approving an access change gives what it gives (D6): dee made an admin, which cy isn't;
        // dee given people-admin, which cy holds.
        (("POST", "/access/proposals/{id}/approve"), ("POST", "/access/proposals/{admin-change}/approve", Value::Null), (false, ("POST", "/access/proposals/{assign-change}/approve", Value::Null))),
    ];
    let neither = [
        (("PUT", "/access/approvals"), "how this host decides approvals: no one's sign-in, tokens or holdings"),
        (("PUT", "/access/local"), "whom the run file signs in as, which is admin on the host whoever it is (D45)"),
        (("DELETE", "/roles/{name}"), "refused while the role is bound (D28): it takes from no one"),
        (("DELETE", "/groups/{name}"), "refused while the group is bound (D28): it takes from no one"),
        (("POST", "/access/proposals/{id}/decline"), "a change declined is never made: it gives and takes nothing"),
    ];
    let changing: Vec<(&str, &str)> = ROUTES
        .iter()
        .filter(|r| r.method != "GET" && matches!(r.rule, Rule::Can(Action::PeopleManage | Action::PeopleAssign | Action::RoleDefine, _)))
        .map(|r| (r.method, r.path))
        .collect();
    let said: Vec<(&str, &str)> = cases.iter().map(|c| c.0).chain(neither.iter().map(|n| n.0)).collect();
    for route in &changing {
        assert!(said.contains(route), "{route:?} changes a principal or what it holds, and has no case here");
    }
    assert_eq!(said.len(), changing.len(), "a case for a route the table doesn't have: {said:?}");

    for (route, more, (by_ada, (method, path, body))) in cases {
        let w = world();
        let fill = |path: &str| w.ids.iter().fold(path.to_string(), |p, (k, v)| p.replace(&format!("{{{k}}}"), v));
        let (code, said) = refusal(send(&w.state, &w.t.cy, more.0, &fill(more.1), more.2));
        assert!(code == 403 && said.contains("which you don't"), "{route:?}, cy to more: {code} {said}");
        let (code, said) = send(&w.state, if by_ada { &w.t.ada } else { &w.t.cy }, method, &fill(path), body);
        assert!((200..300).contains(&code), "{route:?}, to no more: {code} {said}");
    }
}

/// `server()`, with bob an admin, his email bob@example.com; `classifier` (run.decide, run.view), an agent's role; the agents
/// `bot`, bound classifier on the host, and `pup`, a viewer, each owned by bob; and, when
/// `changes`, an access change that gives more waiting for one person besides its author. Answered
/// with a token of this machine's run file, tied to no account.
fn takeover_world(changes: bool) -> (AppState, Tokens, String) {
    let (state, t, _, _) = server();
    let ada = |method: &str, path: &str, body: Value| {
        let (code, said) = send(&state, &t.ada, method, path, body);
        assert!((200..300).contains(&code), "{method} {path}: {said}");
    };
    ada("PATCH", "/people/bob", json!({ "role": "admin", "email": "bob@example.com" }));
    ada("PUT", "/roles/classifier", json!({ "allows": ["run.decide", "run.view"], "kinds": ["agent"] }));
    for agent in ["bot", "pup"] {
        ada("POST", "/people", json!({ "name": agent, "kind": "agent", "role": "viewer", "owner": "bob" }));
    }
    ada("POST", "/bindings", json!({ "who": { "principal": "bot" }, "role": "classifier", "scope": "host" }));
    if changes {
        ada("PUT", "/access/approvals", json!({ "changes": 1 }));
    }
    let local = state.access.accounts().unwrap().new_local("bma");
    (state, t, local)
}

/// While an access change that gives more waits for people, no one makes a way in to someone else
/// holding what that governs, whose name would count as a second person (the access plan's D12):
/// every route that changes a principal makes a way in with a case here, or says why it makes none.
/// Each way in is refused an admin (ada), saying so, to whoever holds what waits (bob, an admin;
/// bot, an agent deciding its steps; for the provider, every person here), and made by the same
/// admin to whoever holds none of it (cy, pup), while the setting is off, through the run file,
/// and by whoever it is a way in to, or for a token its owner (bob).
#[test]
fn no_one_takes_over_someone_holding_what_waits_for_people() {
    use crate::can::{Action, Rule, ROUTES};
    // (route, method, to someone holding what waits, to someone holding none of it, body, who
    // makes it for themself, what the refusal says is done).
    type Way = ((&'static str, &'static str), &'static str, &'static str, Option<&'static str>, Value, Option<&'static str>, &'static str);
    let provider = json!({ "settings": { "issuer": "http://127.0.0.1:9", "client_id": "holon" } });
    let ways: [Way; 5] = [
        (("POST", "/people/{name}/reset"), "POST", "/people/bob/reset", Some("/people/cy/reset"), Value::Null, Some("bob"), "resets bob's password"),
        (("PATCH", "/people/{name}"), "PATCH", "/people/bob", Some("/people/cy"), json!({ "email": "someone@example.com" }), Some("bob"), "sets bob's email"),
        (("PATCH", "/people/{name}"), "PATCH", "/people/bot", Some("/people/pup"), json!({ "owner": "ada" }), None, "makes ada the owner of bot"),
        (("POST", "/people/{name}/tokens"), "POST", "/people/bot/tokens", Some("/people/pup/tokens"), json!({ "label": "mine now" }), Some("bob"), "makes a token of bot"),
        (("POST", "/admin/oidc"), "POST", "/admin/oidc", None, provider, None, "sets the identity provider, which says who each person here is,"),
    ];
    let not_ways = [
        (("POST", "/people"), "a new principal holds nothing that waits (a role that would is refused, 409): its link or owner is a way in to no one who does"),
        (("DELETE", "/people/{name}/tokens/{id}"), "takes a way in away"),
        (("DELETE", "/people/{name}"), "takes them away, and every way in to them"),
        (("POST", "/bindings"), "what is bound: given, it waits for people"),
        (("DELETE", "/bindings/{id}"), "what is bound: taken away at once"),
        (("PUT", "/roles/{name}"), "what is bound: given, it waits for people"),
        (("DELETE", "/roles/{name}"), "refused while the role is bound (D28)"),
        (("PUT", "/groups/{name}"), "what is bound: given, it waits for people"),
        (("DELETE", "/groups/{name}"), "refused while the group is bound (D28)"),
        (("POST", "/access/proposals/{id}/approve"), "makes what waited, by its author"),
        (("POST", "/access/proposals/{id}/decline"), "makes nothing"),
        (("PUT", "/access/approvals"), "how this host decides approvals: no one's way in"),
        (("PUT", "/access/local"), "the run file's own way in, which is this machine's: the root here (D11)"),
    ];
    let changing: Vec<(&str, &str)> = ROUTES
        .iter()
        .filter(|r| r.method != "GET" && matches!(r.rule, Rule::Can(Action::PeopleManage | Action::PeopleAssign | Action::RoleDefine, _)))
        .map(|r| (r.method, r.path))
        .collect();
    let mut said: Vec<(&str, &str)> = ways.iter().map(|w| w.0).chain(not_ways.iter().map(|n| n.0)).collect();
    said.dedup();
    for route in &changing {
        assert!(said.contains(route), "{route:?} changes a principal, and has no case here: is it a way in?");
    }
    assert_eq!(said.len(), changing.len(), "a case for a route the table doesn't have: {said:?}");

    for (route, method, held, none, body, themself, what) in ways {
        let made = |changes: bool, by: &str, path: &str| {
            let (state, t, local) = takeover_world(changes);
            let token = match by {
                "ada" => t.ada.clone(),
                "bob" => t.bob.clone(),
                _ => local,
            };
            let (code, said) = refusal(send(&state, &token, method, path, body.clone()));
            // The provider is let through to its discovery, which no one answers here.
            let through = (200..300).contains(&code) || (route.1 == "/admin/oidc" && code == 400);
            (through, code, said)
        };
        let (through, code, said) = made(true, "ada", held);
        let target = if route.1 == "/admin/oidc" { "bob" } else { held.trim_start_matches("/people/").split('/').next().unwrap() };
        assert!(!through && code == 403, "{route:?} by ada to {target}: {code} {said}");
        assert!(said.contains(&format!("whoever {what} could act as {target}, counted as another person")), "{route:?}: {said}");
        assert!(said.ends_with("which waits") || said.ends_with("which waits too"), "{route:?}: {said}");
        for (changes, by, path, case) in [(true, "ada", none, "to someone holding none of it"), (false, "ada", Some(held), "the setting off"), (true, "local", Some(held), "through the run file"), (true, "bob", themself.map(|_| held), "by themself, or its owner")] {
            let Some(path) = path else { continue };
            let (through, code, said) = made(changes, by, path);
            assert!(through, "{route:?} {path}, {case}: {code} {said}");
        }
    }

    // Word for word, the first.
    let (state, t, _) = takeover_world(true);
    assert_eq!(
        refusal(send(&state, &t.ada, "POST", "/people/bob/reset", Value::Null)),
        (403, "bob holds run.decide on this host, as admin, and giving that waits here for 1 person other than its giver: whoever resets bob's password could act as bob, counted as another person, so it is done only through this server's machine (its run file); or take that from bob first, which applies at once, and give it back after, which waits".into())
    );
    // Not a way in: his email as it is, or cleared, and how he is shown.
    for same in [json!({ "email": " Bob@Example.com" }), json!({ "email": "" }), json!({ "display": "Bob" })] {
        assert_eq!(send(&state, &t.ada, "PATCH", "/people/bob", same.clone()).0, 200, "{same}");
    }
    // That way, through the rules there are: taken away at once, reset, and given back, which
    // waits, its proposal saying who made bob's link.
    assert_eq!(send(&state, &t.ada, "PATCH", "/people/bob", json!({ "role": "viewer" })).0, 200);
    assert_eq!(send(&state, &t.ada, "POST", "/people/bob/reset", Value::Null).0, 200);
    let (code, said) = send(&state, &t.ada, "PATCH", "/people/bob", json!({ "role": "admin" }));
    assert_eq!(code, 202, "{said}");
    let p = &serde_json::from_str::<Value>(&said).unwrap()["proposal"];
    assert_eq!((p["to"].clone(), p["ways_in"].clone()), (json!(["bob"]), json!(["ada made bob's reset link"])));
    let people: Value = serde_json::from_str(&send(&state, &t.ada, "GET", "/people", Value::Null).1).unwrap();
    let bob = people.as_array().unwrap().iter().find(|p| p["name"] == "bob").unwrap();
    assert_eq!(bob["ways_in"], json!(["ada made bob's reset link"]), "the People page says it too");
}

/// While an access change that gives more waits for people (`approvals.changes`), each route that
/// makes one answers 202 with the proposal, saying what its version will say, and changes nothing
/// but what gives no more (a display name beside a role); a new principal with a role that would
/// wait is refused (409). The proposals are listed newest first; their author doesn't approve
/// them, another admin does, each then made as its author asked, by them, its why saying who
/// approved it; a decline needs a reason. The activity log says each. (Each gives to someone of its
/// own: one made before another that gives to the same person would change what the other gives,
/// which makes it stale, access_proposals.rs.)
#[test]
fn an_access_change_that_gives_more_waits_for_people_whichever_route_makes_it() {
    let (state, t, _, _) = server();
    let ada = |method: &str, path: &str, body: Value| send(&state, &t.ada, method, path, body);
    let bob = |method: &str, path: &str, body: Value| send(&state, &t.bob, method, path, body);
    assert_eq!(ada("PATCH", "/people/bob", json!({ "role": "admin" })).0, 200);
    assert_eq!(ada("PUT", "/roles/approver", json!({ "allows": ["run.decide", "run.view"], "kinds": ["person"] })).0, 200);
    assert_eq!(ada("PUT", "/groups/team", json!({ "members": ["cy"] })).0, 200);
    assert_eq!(ada("POST", "/bindings", json!({ "who": { "group": "team" }, "role": "approver", "scope": { "flows": { "slug": "d" } } })).0, 200);
    for name in ["gil", "hal", "ivy"] {
        assert_eq!(ada("POST", "/people", json!({ "name": name, "kind": "person", "role": "viewer" })).0, 200);
    }
    assert_eq!(ada("PUT", "/roles/keeper", json!({ "allows": ["run.view"], "kinds": ["person"] })).0, 200);
    assert_eq!(ada("POST", "/bindings", json!({ "who": { "principal": "hal" }, "role": "keeper", "scope": "host" })).0, 200);
    assert_eq!(ada("PUT", "/access/approvals", json!({ "changes": 1 })).0, 200, "turning it up is made at once");
    let accounts = Arc::clone(state.access.accounts().unwrap());
    let base = accounts.access_document().version;

    let waiting = [
        ("POST", "/bindings", json!({ "who": { "principal": "dee" }, "role": "approver", "scope": "host", "why": "on call" }), "bound approver to dee on this host"),
        ("PUT", "/groups/team", json!({ "members": ["cy", "gil"] }), "changed the group team: cy, gil"),
        ("PUT", "/roles/keeper", json!({ "allows": ["run.view", "secret.set"], "kinds": ["person"] }), "changed the role keeper: run.view, secret.set, held by a person"),
        ("PATCH", "/people/ivy", json!({ "role": "editor", "display": "Ivy" }), "bound editor to ivy on this host"),
        ("PUT", "/access/approvals", json!({ "changes": null }), "set the approvals: whoever deployed the version a run is on may decide its approvals; agents take the steps written for them; a person who started a run may take a step of it that says so; an access change applies at once"),
    ];
    for (n, (method, path, body, what)) in waiting.iter().enumerate() {
        let (code, said) = ada(method, path, body.clone());
        assert_eq!(code, 202, "{method} {path}: {said}");
        let v: Value = serde_json::from_str(&said).unwrap();
        let p = &v["proposal"];
        assert_eq!((p["id"].as_u64(), p["what"].as_str(), p["state"].as_str(), p["author"].as_str(), p["base"].as_u64()), (Some(n as u64 + 1), Some(*what), Some("waiting"), Some("ada"), Some(base)), "{said}");
    }
    assert_eq!(accounts.access_document().version, base, "nothing it gives is made");
    assert_eq!(accounts.principal("ivy").unwrap().display.as_deref(), Some("Ivy"), "a display name gives nothing");
    assert_eq!(listed(&state, &t.ada, "/access/proposals", Some("proposals"), "what"), waiting.iter().rev().map(|w| w.3.to_string()).collect::<Vec<_>>());
    let (code, said) = refusal(ada("POST", "/people", json!({ "name": "eve", "kind": "person", "role": "editor" })));
    assert_eq!(code, 409, "{said}");
    assert!(said.ends_with("invite eve as viewer, then give them editor, which waits"), "{said}");

    assert_eq!(refusal(ada("POST", "/access/proposals/1/approve", Value::Null)), (403, "ada proposed access change 1: it is approved by people other than its author".into()));
    for id in 1..=waiting.len() {
        let (code, said) = bob("POST", &format!("/access/proposals/{id}/approve"), Value::Null);
        assert_eq!(code, 200, "{id}: {said}");
        let v: Value = serde_json::from_str(&said).unwrap();
        assert_eq!((v["proposal"]["state"].as_str(), v["proposal"]["version"].as_u64()), (Some("applied"), Some(base + id as u64)), "{said}");
    }
    let document = accounts.access_document();
    assert_eq!((document.version, document.by.as_str(), document.why.as_deref()), (base + 5, "ada", Some("access change 5, approved by bob")));
    assert_eq!(accounts.access_version(base + 1).unwrap().unwrap().why.as_deref(), Some("on call; access change 1, approved by bob"));
    assert_eq!((accounts.host_role("ivy"), document.approvals.changes), (Some(crate::access::Role::Editor), None));

    // Back on: a decline needs a reason.
    assert_eq!(ada("PUT", "/access/approvals", json!({ "changes": 1 })).0, 200);
    assert_eq!(ada("POST", "/bindings", json!({ "who": { "principal": "cy" }, "role": "admin", "scope": "host" })).0, 202);
    assert_eq!(refusal(bob("POST", "/access/proposals/6/decline", json!({}))), (400, "say why it is declined: {\"reason\": \"…\"}".into()));
    let (code, said) = bob("POST", "/access/proposals/6/decline", json!({ "reason": "cy is a viewer" }));
    assert_eq!(code, 200, "{said}");
    let declined: Value = serde_json::from_str(&said).unwrap();
    assert_eq!((declined["proposal"]["state"].as_str(), declined["proposal"]["declined_by"].as_str(), declined["proposal"]["reason"].as_str()), (Some("declined"), Some("bob"), Some("cy is a viewer")));
    assert_eq!(accounts.host_role("cy"), Some(crate::access::Role::Viewer));

    let activity: Value = serde_json::from_str(&ada("GET", "/activity", Value::Null).1).unwrap();
    let done: Vec<String> = activity["entries"].as_array().unwrap().iter().map(|e| format!("{} {} {}", e["by"].as_str().unwrap_or_default(), e["action"].as_str().unwrap(), e["target"].as_str().unwrap())).collect();
    for said in [
        "ada proposed an access change 1: bound approver to dee on this host",
        "bob approved an access change (1 of 1) 1: bound approver to dee on this host",
        "bob made an access change 1: bound approver to dee on this host",
        "bob declined an access change 6: bound admin to cy on this host (cy is a viewer)",
    ] {
        assert!(done.iter().any(|d| d == said), "{said}: {done:?}");
    }
}

/// Where a database or a folder is (its file, its connection's source and address, its path) is
/// shown to whoever may bind one, not to whoever only views them (web console L4, Q4): cy, holding
/// a role that allows resource.view on the host, sees each binding, its name and kind, without
/// where it is; ada, an admin, sees it all.
#[test]
fn where_a_resource_is_shows_only_to_whoever_may_bind_one() {
    let (state, t, _, _) = server();
    let ada = |method: &str, path: &str, body: Value| send(&state, &t.ada, method, path, body);
    let dir = std::env::temp_dir().join(format!("holon-where-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let folder = dir.display().to_string();
    let file = dir.join("shop.db").display().to_string();
    assert_eq!(ada("POST", "/admin/folders", json!({ "name": "inbox", "path": folder, "access": "read" })).0, 200);
    let (code, said) = ada("POST", "/admin/databases", json!({ "name": "shop", "path": file, "schema": "create table orders (id integer primary key)" }));
    assert_eq!(code, 200, "{said}");
    assert_eq!(ada("PUT", "/roles/looker", json!({ "allows": ["resource.view"], "kinds": ["person"] })).0, 200);
    assert_eq!(ada("PATCH", "/people/cy", json!({ "role": "looker" })).0, 200);

    let listed = |token: &str, path: &str| -> Value { serde_json::from_str(&send(&state, token, "GET", path, Value::Null).1).unwrap() };
    let (folders, databases) = (listed(&t.ada, "/admin/folders"), listed(&t.ada, "/admin/databases"));
    assert_eq!((&folders[0]["name"], &folders[0]["path"]), (&json!("inbox"), &json!(folder)));
    assert_eq!((&databases[0]["name"], &databases[0]["path"]), (&json!("shop"), &json!(file)));

    let (folders, databases) = (listed(&t.cy, "/admin/folders"), listed(&t.cy, "/admin/databases"));
    assert_eq!(folders, json!([{ "name": "inbox", "deployment": null, "access": "read", "keep": "bytes", "cap": null, "shared": false, "lock": "commit" }]));
    assert_eq!(databases, json!([{ "name": "shop", "deployment": null, "engine": "sqlite", "access": "read-write", "lock": "commit" }]));
    assert_eq!(refusal(send(&state, &t.cy, "POST", "/admin/folders", json!({ "name": "x", "path": folder }))).0, 403, "cy only views");
    std::fs::remove_dir_all(&dir).ok();
}
