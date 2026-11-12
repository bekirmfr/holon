//! A flow's tags, through the real gate and handlers (docs/L4s/roles-and-bindings-plan.md, step
//! 3d): a binding on a tag covers the flows deployed with it, while they are; a version is deployed
//! only by who holds the flow as it is and as the version would make it (D22); an agent's new tag
//! waits for a person, as a proposal; and the review says who a tag lets in (D24).

use super::access_table::{ask, deploy, error_of};
use super::*;
use crate::accounts::{Accounts, Kind};
use serde_json::{json, Value};

const PW: &str = "correct horse battery";

/// scopes.rs's flow, which pays at an approval point, with `settings` as its flow(…) line writes
/// them.
fn pay(settings: &str) -> String {
    format!(
        r#"import {{ action, approval, flow }} from "holon"

flow("pay"{settings})

export const pay = action(async (order) => {{
  const decision = await approval("payout", {{ expires: "1h" }})
  return decision.approved
}})
"#
    )
}

/// A server with accounts: ada, the admin, and bob, a person bound `editor` on the tag
/// `payments` only. Answered with their tokens, and the units of `pay` untagged and tagged.
fn server() -> (AppState, String, String, String, String) {
    let mut state = test_state();
    let untagged = to_hex(&stage_text(&state, &pay("")));
    let tagged = to_hex(&stage_text(&state, &pay(", { tags: [\"payments\"] }")));
    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    let bob = book.accept(&book.invite("bob", "viewer", None, "ada").unwrap(), PW).unwrap().0;
    state.access = access::Access::new(book, vec![], vec![]);
    let (code, said) = send(&state, &ada, "POST", "/bindings", json!({ "who": { "principal": "bob" }, "role": "editor", "scope": { "flows": { "tag": "payments" } } }));
    assert_eq!(code, 200, "{said}");
    let host = state.access.accounts().unwrap().bindings_of("bob").into_iter().find(|b| b.scope == crate::bindings::Scope::Host).unwrap();
    assert_eq!(send(&state, &ada, "DELETE", &format!("/bindings/{}", host.id), Value::Null).0, 204);
    (state, ada, bob, untagged, tagged)
}

/// `method` `path` with `body` (none when null) as the holder of `token`: its status and its body.
fn send(state: &AppState, token: &str, method: &str, path: &str, body: Value) -> (u16, String) {
    ask(state, Some(token), method, path, if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() })
}

/// `(status, error)` of an answer.
fn refusal((code, said): (u16, String)) -> (u16, String) {
    (code, error_of(&said))
}

/// The slugs `token`'s holder is listed by `GET /deployments`, each with its tags.
fn deployments(state: &AppState, token: &str) -> Vec<(String, Value)> {
    let v: Value = serde_json::from_str(&send(state, token, "GET", "/deployments", Value::Null).1).unwrap();
    v["deployments"].as_array().unwrap().iter().map(|d| (d["slug"].as_str().unwrap().to_string(), d.get("tags").cloned().unwrap_or(json!([])))).collect()
}

/// Whether `token`'s holder may view `slug`'s runs, as `/auth/me/can` answers.
fn views(state: &AppState, token: &str, slug: &str) -> bool {
    let can: Value = serde_json::from_str(&send(state, token, "GET", &format!("/auth/me/can?actions=run.view&flow={slug}"), Value::Null).1).unwrap();
    can["run.view"] == true
}

/// bob, `editor` on `payments`, acts on `d` while the version deployed of it is tagged so, and
/// on nothing else: a version without the tag takes `d` out of his reach, restoring the tagged
/// one brings it back, and deleting it takes it out for good. `GET /deployments` says each flow's
/// tags, and `GET /bindings?flow=d` lists the binding on the tag while it covers `d`.
#[test]
fn a_binding_on_a_tag_covers_the_flows_deployed_with_it() {
    let (state, ada, bob, untagged, tagged) = server();
    assert_eq!(deploy(&state, Some(&ada), "d", &tagged, json!({})).0, 200);
    // Tags are the unit's: ones a client sends with the requirements are no tag.
    assert_eq!(deploy(&state, Some(&ada), "e", &untagged, json!({ "tags": ["payments"] })).0, 200);
    assert_eq!(deployments(&state, &ada), [("d".to_string(), json!(["payments"])), ("e".to_string(), json!([]))]);
    assert_eq!(deployments(&state, &bob), [("d".to_string(), json!(["payments"]))]);
    assert!(views(&state, &bob, "d") && !views(&state, &bob, "e"));
    assert_eq!(send(&state, &bob, "POST", "/deployments/d/pause", Value::Null).0, 200);
    let (code, said) = send(&state, &bob, "GET", "/deployments/e/runs", Value::Null);
    assert_eq!((code, error_of(&said)), (403, "bob may not do that: no role bob holds on \"e\" holds run.view".into()));
    let on_d: Value = serde_json::from_str(&send(&state, &ada, "GET", "/bindings?flow=d&who=bob", Value::Null).1).unwrap();
    assert_eq!(on_d["bindings"].as_array().unwrap().len(), 1, "{on_d}");

    // A version without the tag: d is out of bob's reach, and the binding no longer lists on it.
    assert_eq!(deploy(&state, Some(&ada), "d", &untagged, json!({})).0, 200);
    assert!(!views(&state, &bob, "d"));
    assert!(deployments(&state, &bob).is_empty());
    let on_d: Value = serde_json::from_str(&send(&state, &ada, "GET", "/bindings?flow=d&who=bob", Value::Null).1).unwrap();
    assert_eq!(on_d["bindings"], json!([]));

    // The tagged version restored: back in, the tag approved again as anything new is.
    let restore = |dry_run: bool, acknowledge: Value| send(&state, &ada, "POST", "/deployments/d/restore", json!({ "version": 1, "code": true, "dry_run": dry_run, "acknowledge": acknowledge }));
    let (code, said) = restore(true, json!([]));
    assert_eq!(code, 200, "{said}");
    let needs = serde_json::from_str::<Value>(&said).unwrap()["needs_approval"].clone();
    assert_eq!(needs, json!(["Tag(\"payments\")"]));
    let (code, said) = restore(false, needs);
    assert_eq!(code, 200, "{said}");
    assert!(views(&state, &bob, "d"));

    // Deleted, it is on no tag.
    assert_eq!(send(&state, &ada, "POST", "/deployments/d/stop", Value::Null).0, 200);
    assert_eq!(send(&state, &ada, "DELETE", "/deployments/d", Value::Null).0, 200);
    assert!(state.access.accounts().unwrap().tagged("payments").is_empty());
    assert!(!views(&state, &bob, "d"));
}

/// D22: bob, `editor` on `payments`, deploys a new flow tagged so; he doesn't tag `payments` onto
/// a flow he holds nothing on, taking it over, nor take it off one, taking it out of his own reach
/// (and his team's). The host's admin does both.
#[test]
fn a_version_is_deployed_only_by_who_holds_it_as_it_is_and_as_it_would_be() {
    let (state, ada, bob, untagged, tagged) = server();
    assert_eq!(deploy(&state, Some(&ada), "d", &tagged, json!({})).0, 200);
    assert_eq!(deploy(&state, Some(&ada), "e", &untagged, json!({})).0, 200);

    let (code, said) = deploy(&state, Some(&bob), "n", &tagged, json!({}));
    assert_eq!(code, 200, "a new flow on his tag: {said}");
    assert!(views(&state, &bob, "n"));
    let asked = |slug: &str, unit: &str| json!({ "slug": slug, "unit": unit });
    let taken_over = (403, "bob may not do that: no role bob holds on \"e\" holds flow.propose".to_string());
    assert_eq!(refusal(send(&state, &bob, "POST", "/deploy/preview", asked("e", &tagged))), taken_over);
    assert_eq!(refusal(send(&state, &bob, "POST", "/deploy", asked("e", &tagged))), taken_over);
    let untagging = (403, "bob may not do that: this version takes \"d\" off the tag payments, which is how bob holds it; someone who holds it otherwise deploys it".to_string());
    assert_eq!(refusal(send(&state, &bob, "POST", "/deploy/preview", asked("d", &untagged))), untagging);
    assert_eq!(refusal(send(&state, &bob, "POST", "/deploy", asked("d", &untagged))), untagging);
    // An untagged flow of his own, he can't make: it would be out of his reach as it is made.
    assert_eq!(refusal(send(&state, &bob, "POST", "/deploy", asked("m", &untagged))), (403, "bob may not do that: no role bob holds on \"m\" without tags holds flow.propose".into()));

    assert_eq!(deploy(&state, Some(&ada), "e", &tagged, json!({})).0, 200);
    assert_eq!(deploy(&state, Some(&ada), "d", &untagged, json!({})).0, 200);
    assert!(views(&state, &bob, "e") && !views(&state, &bob, "d"));
}

/// An agent's version that adds a tag is kept as a proposal, the tag among what a person
/// approves: the flow isn't on the tag until one does. An agent that acknowledges what a version
/// needs is refused that first, whatever the version, stored or not.
#[test]
fn an_agents_new_tag_is_a_proposal() {
    let (state, ada, _, untagged, tagged) = server();
    assert_eq!(deploy(&state, Some(&ada), "e", &untagged, json!({})).0, 200);
    let accounts = Arc::clone(state.access.accounts().unwrap());
    accounts.add("scout", Kind::Agent, "agent", "ada", "ada").unwrap();
    let (scout, _) = accounts.create_token("scout", "Scout", "ada").unwrap();
    let (code, said) = send(&state, &scout, "POST", "/deploy", json!({ "slug": "e", "unit": tagged, "document": {}, "why": "on the payments team's flows" }));
    assert_eq!(code, 202, "{said}");
    let proposal: Value = serde_json::from_str(&said).unwrap();
    assert_eq!(proposal["proposal"]["needs"], json!(["Tag(\"payments\")"]));
    assert_eq!(deployments(&state, &ada), [("e".to_string(), json!([]))]);
    assert!(accounts.tagged("payments").is_empty());
    let approving = "an agent doesn't approve what a flow may do: deploy without \"acknowledge\"; what needs approval waits for a person, as a proposal";
    for unit in [tagged.clone(), "0".repeat(64)] {
        let asked = json!({ "slug": "e", "unit": unit, "document": {}, "acknowledge": ["Tag(\"payments\")"] });
        assert_eq!(refusal(send(&state, &scout, "POST", "/deploy", asked)), (403, approving.to_string()), "{unit}");
    }
}

/// D24: the review of a version that joins a tag says who the tag lets act on the flow now, in
/// its own group; one that leaves it, who no longer may. The policy never grants a tag.
#[test]
fn the_preview_says_who_a_tag_lets_in() {
    let (state, ada, _, untagged, tagged) = server();
    assert_eq!(send(&state, &ada, "PUT", "/groups/ops", json!({ "members": ["bob"] })).0, 200);
    let (code, said) = send(&state, &ada, "POST", "/bindings", json!({ "who": { "group": "ops" }, "role": "viewer", "scope": { "flows": { "tag": "payments" } } }));
    assert_eq!(code, 200, "{said}");
    assert_eq!(deploy(&state, Some(&ada), "e", &untagged, json!({})).0, 200);
    let review = |unit: &str| {
        let (code, said) = send(&state, &ada, "POST", "/deploy/preview", json!({ "slug": "e", "unit": unit }));
        assert_eq!(code, 200, "{said}");
        let preview: Value = serde_json::from_str(&said).unwrap();
        (preview["needs_approval"].clone(), crate::touches::text(&crate::touches::lines(&preview["touches"], &preview["settings"])))
    };
    let (needs, lines) = review(&tagged);
    assert_eq!(needs, json!(["Tag(\"payments\")"]));
    let at = lines.iter().position(|l| l == "Who may act on it").expect("the group");
    assert_eq!(lines[at + 1], "  + joins payments: bob as editor, the group ops as viewer — approved by confirming");
    assert_eq!(deploy(&state, Some(&ada), "e", &tagged, json!({})).0, 200);
    let (_, lines) = review(&untagged);
    assert!(lines.contains(&"  - leaves payments: bob as editor, the group ops as viewer — no longer".to_string()), "{lines:?}");
}
