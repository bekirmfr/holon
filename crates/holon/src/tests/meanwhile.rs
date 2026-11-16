//! A deploy is worked out from the deployment as read before the lock that puts its version in:
//! what it needs approving, and whether it waits for a person. A change a person makes to the
//! deployment meanwhile (a revoke, a limit lowered, agents' versions no longer let deploy at once,
//! another version) isn't undone by the grant built from that copy: the deploy is refused, 409, and
//! sent again it is worked out anew (the audit register's R25, its last part).

use super::access_table::ask;
use super::*;
use crate::accounts::{Accounts, Kind};
use serde_json::{json, Value};
use std::sync::mpsc;

const PW: &str = "correct horse battery";

/// Its slug: no other test's, as a deploy of it is held while one is.
const SLUG: &str = "held-tally";

const FLOW: &str = r#"import { action, flow } from "holon"
import state from "state"

flow("held-tally")

export const keep = action((asked) => {
  state.count = asked.n
  return "kept"
})
"#;

/// What its action writes: an item of its grant.
const WRITES: &str = "StateWrite(\"count\")";

/// A version, its units stored: its unit and its document.
struct Staged {
    unit: String,
    document: Value,
}

/// `method` `path` with `body` (none when null) as the holder of `token`: its status and its body.
fn send(state: &AppState, token: &str, method: &str, path: &str, body: Value) -> (u16, Value) {
    let (code, said) = ask(state, Some(token), method, path, if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() });
    (code, serde_json::from_str(&said).unwrap_or(Value::Null))
}

/// `FLOW` answering `answer`, compiled and its units stored by `ada`.
fn stage(state: &AppState, ada: &str, answer: &str) -> Staged {
    let plan = holon_author::agent::plan(&FLOW.replace("return \"kept\"", &format!("return {answer:?}"))).unwrap_or_else(|e| panic!("{e}"));
    for unit in &plan.units {
        assert_eq!(ask(state, Some(ada), "POST", "/unit", unit.clone()).0, 200);
    }
    Staged { unit: plan.unit, document: plan.document }
}

/// `staged` deployed by `token`'s holder, approving `acknowledge`.
fn deploy(state: &AppState, token: &str, staged: &Staged, acknowledge: &Value) -> (u16, Value) {
    send(state, token, "POST", "/deploy", json!({ "slug": SLUG, "unit": staged.unit, "document": staged.document, "acknowledge": acknowledge }))
}

/// What deploying `staged` needs approving, as `token`'s holder is told.
fn needs(state: &AppState, token: &str, staged: &Staged) -> Value {
    let (code, preview) = send(state, token, "POST", "/deploy/preview", json!({ "slug": SLUG, "unit": staged.unit }));
    assert_eq!(code, 200, "{preview}");
    preview["needs_approval"].clone()
}

/// `token`'s deploy of `staged`, approving `acknowledge`, held once it is worked out while
/// `meanwhile` changes the deployment, then let go: what it answers.
fn held(state: &AppState, token: &str, staged: &Staged, acknowledge: &Value, meanwhile: impl FnOnce()) -> (u16, Value) {
    let (there, arrived) = mpsc::channel();
    let (go, waits) = mpsc::channel();
    crate::deploy::HELD.lock().unwrap().insert(SLUG.into(), (there, waits));
    std::thread::scope(|s| {
        let deploying = s.spawn(|| deploy(state, token, staged, acknowledge));
        let reached = arrived.recv_timeout(EVENTUALLY);
        crate::deploy::HELD.lock().unwrap().remove(SLUG);
        reached.expect("the deploy is worked out, and held before it is put in");
        meanwhile();
        go.send(()).unwrap();
        deploying.join().unwrap()
    })
}

/// A server with accounts, ada its admin and scout an agent, and `FLOW` deployed by ada, with
/// an agent's versions that ask for nothing more let deploy at once: the server, ada's token,
/// scout's, and what ada deployed.
fn server() -> (AppState, String, String, Staged) {
    let mut state = test_state();
    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    book.add("scout", Kind::Agent, "agent", "ada", "ada").unwrap();
    let scout = book.create_token("scout", "its tools", "ada").unwrap().0;
    state.access = access::Access::new(book, vec![], vec![]);
    let base = stage(&state, &ada, "kept");
    let (code, said) = deploy(&state, &ada, &base, &needs(&state, &ada, &base));
    assert_eq!(code, 200, "{said}");
    let (code, said) = send(&state, &ada, "POST", &format!("/deployments/{SLUG}/settings"), json!({ "agents_deploy_at_once": true }));
    assert_eq!(code, 200, "{said}");
    (state, ada, scout, base)
}

/// What the deployment is now: its unit, and what its grant holds and has lowered.
fn now(state: &AppState) -> (String, bool, Option<u64>) {
    let table = state.deployments.lock().unwrap();
    let d = &table[SLUG];
    (to_hex(&d.unit), d.grant.items.contains(WRITES), d.grant.lowered.get("keep_runs").copied())
}

/// The class: a deploy held after it is worked out, while a person changes the deployment, each
/// change in turn on a server of its own. The deploy is refused, naming why, and the change stands:
/// - a revoke, under an agent's version deployed at once and under a person's deploy, which
///   approved nothing since nothing was needed: sent again, the agent's waits for a person, asking
///   for what was revoked, and the person's is refused until they approve it;
/// - a limit lowered: sent again, the agent's version deploys at once, the limit still lowered;
/// - agents' versions no longer let deploy at once: sent again, it waits for a person;
/// - another version, deployed by a person: sent again, the agent's deploys at once over it.
#[test]
fn a_deploy_undoes_nothing_a_person_changed_while_it_was_worked_out() {
    let refused = |when: &str, (code, said): (u16, Value)| {
        assert_eq!(code, 409, "{when}: {said}");
        let why = said["error"].as_str().unwrap_or_default();
        assert!(why.contains("changed while this deploy was worked out") && why.contains("deploy again"), "{when}: {said}");
    };
    let nothing = json!([]);
    let revoke = |state: &AppState, ada: &str| {
        let (code, said) = send(state, ada, "POST", &format!("/deployments/{SLUG}/grant"), json!({ "revoke": [WRITES], "keep_running": true }));
        assert_eq!(code, 200, "{said}");
    };

    // A revoke, under an agent's version deployed at once.
    let (state, ada, scout, base) = server();
    let changed = stage(&state, &ada, "noted");
    refused("a revoke, under an agent's version", held(&state, &scout, &changed, &nothing, || revoke(&state, &ada)));
    assert_eq!(now(&state), (base.unit.clone(), false, None), "the revoke stands, the version as it was");
    let (code, said) = deploy(&state, &scout, &changed, &nothing);
    assert_eq!(code, 202, "sent again, it waits for a person: {said}");
    assert_eq!(said["proposal"]["needs"], json!([WRITES]), "asking for what was revoked: {said}");

    // A revoke, under a person's deploy that approved nothing, nothing being needed.
    let (state, ada, _, base) = server();
    let changed = stage(&state, &ada, "noted");
    assert_eq!(needs(&state, &ada, &changed), nothing);
    refused("a revoke, under a person's deploy", held(&state, &ada, &changed, &nothing, || revoke(&state, &ada)));
    assert_eq!(now(&state), (base.unit.clone(), false, None), "the revoke stands, the version as it was");
    let (code, said) = deploy(&state, &ada, &changed, &nothing);
    assert_eq!(code, 400, "sent again, they approve what was revoked: {said}");
    assert!(said["error"].as_str().unwrap().contains(WRITES), "{said}");

    // A limit lowered.
    let (state, ada, scout, base) = server();
    let changed = stage(&state, &ada, "noted");
    let lower = || assert_eq!(send(&state, &ada, "POST", &format!("/deployments/{SLUG}/settings"), json!({ "lower": { "keep_runs": 3 } })).0, 200);
    refused("a limit lowered", held(&state, &scout, &changed, &nothing, lower));
    assert_eq!(now(&state), (base.unit.clone(), true, Some(3)), "lowered, the version as it was");
    assert_eq!(deploy(&state, &scout, &changed, &nothing).0, 200, "sent again, it deploys at once");
    assert_eq!(now(&state), (changed.unit.clone(), true, Some(3)), "the limit still lowered");

    // Agents' versions no longer let deploy at once.
    let (state, ada, scout, base) = server();
    let changed = stage(&state, &ada, "noted");
    let off = || assert_eq!(send(&state, &ada, "POST", &format!("/deployments/{SLUG}/settings"), json!({ "agents_deploy_at_once": false })).0, 200);
    refused("agents' versions no longer at once", held(&state, &scout, &changed, &nothing, off));
    assert_eq!(now(&state).0, base.unit, "the version as it was");
    assert_eq!(deploy(&state, &scout, &changed, &nothing).0, 202, "sent again, it waits for a person");

    // Another version, deployed by a person.
    let (state, ada, scout, _) = server();
    let (changed, other) = (stage(&state, &ada, "noted"), stage(&state, &ada, "other"));
    let theirs = || assert_eq!(deploy(&state, &ada, &other, &needs(&state, &ada, &other)).0, 200);
    refused("another version", held(&state, &scout, &changed, &nothing, theirs));
    assert_eq!(now(&state).0, other.unit, "the person's version");
    assert_eq!(deploy(&state, &scout, &changed, &nothing).0, 200, "sent again, it deploys at once over it");
}
