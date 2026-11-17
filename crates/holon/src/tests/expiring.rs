//! An expired waiting run goes on in a thread of its own (the audit register's R34, durable.rs's
//! half): one slow to go on, its part sending a request the far end holds, holds no other deadline
//! and no pass after it. At most `EXPIRING_AT_ONCE` go on at once; the rest go on at the next pass.

use super::*;
use crate::durable::{self, EXPIRING_AT_ONCE};
use serde_json::json;
use std::sync::atomic::Ordering::SeqCst;

/// `a-slow`: its action `hold` waits for the event "paid", an hour at most; given it, timed out or
/// not, it asks `host`'s `/slow`, the request tied to the event so it is sent only then.
fn slow_text(host: &str) -> String {
    format!(
        r#"import {{ action, event, flow }} from "holon"
import api from "host:{host}"

flow("a-slow")

export const hold = action(async () => {{
  const paid = await event("paid", {{ expires: "1h" }})
  const sent = await api.get("/slow", {{ headers: {{ "x-paid": JSON.stringify(paid.expired ?? false) }} }})
  return sent.body
}})
"#
    )
}

/// `b-quick`: the same wait, and it finishes as soon as it is given the event.
const QUICK: &str = r#"import { action, event, flow } from "holon"

flow("b-quick")

export const hold = action(async () => {
  const paid = await event("paid", { expires: "1h" })
  return paid.expired ?? false
})
"#;

/// How many runs wait in `slug`.
fn waiting(state: &AppState, slug: &str) -> usize {
    state.deployments.lock().unwrap()[slug].parked.len()
}

/// `n` runs of `slug`'s `hold`, each waiting for its event.
fn hold(state: &AppState, slug: &str, n: usize) {
    for _ in 0..n {
        let r = run_action_named(state, slug, "hold", json!({}));
        assert!(r.is_success(), "{slug}: {}", String::from_utf8_lossy(&body_bytes(r)));
    }
}

/// The class: past their deadlines, a run slow to go on (the request its part sends is held at the
/// far end) and three others. The three go on and finish while the slow one's request is still
/// held, and the pass that set them going waited for none of them. The slow one's slug sorts
/// first, so a pass resuming them in turn would hold the three behind it.
#[test]
fn one_slow_expiry_holds_no_other_deadline() {
    let (host, hits, gate) = gated_counting_server();
    gate.shut();
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_plain_http = true\nallow_private_addresses = true\n").unwrap());
    assert_eq!(deploy_text(&state, &slow_text(&host), &host), "a-slow");
    assert_eq!(deploy_text_to(&state, QUICK, &[]), "b-quick");
    hold(&state, "a-slow", 1);
    hold(&state, "b-quick", 3);

    // A pass, as the clock scheduler makes one, on a thread of its own.
    let pass = {
        let (store, deployments, path) = (state.store.clone(), Arc::clone(&state.deployments), state.deployments_path.clone());
        std::thread::spawn(move || durable::expire_waiting(&store, &deployments, &path, epoch_now() + 4000))
    };
    assert!(gate.out_at_once(1), "the slow one's part asks the far end, which holds it");
    eventually("the three go on while the slow one's request is held", || waiting(&state, "b-quick") == 0);
    eventually("the pass waits for none of them", || pass.is_finished());
    assert_eq!((gate.out(), waiting(&state, "a-slow")), (1, 1), "the slow one still going on, its request held");

    let going = pass.join().unwrap();
    gate.open();
    going.wait();
    assert_eq!(waiting(&state, "a-slow"), 0, "the slow one went on too, once answered");
    assert_eq!(hits_of(&hits, "/slow"), 1, "its part asked once");
    assert_eq!(state.store.1.expiring.load(SeqCst), 0, "each place taken is given back");
}

/// At most `EXPIRING_AT_ONCE` go on at once: with every place taken, a pass sets none going and
/// they stay due; with one place, it sets one going, and the other goes on at the next pass.
#[test]
fn expiries_go_on_at_most_so_many_at_once() {
    let state = test_state();
    *state.policy.write().unwrap() = Arc::new(Policy::parse("[grants]\nallow_private_addresses = true\n").unwrap());
    assert_eq!(deploy_text_to(&state, QUICK, &[]), "b-quick");
    hold(&state, "b-quick", 2);
    let pass = || durable::expire_waiting(&state.store, &state.deployments, &state.deployments_path, epoch_now() + 4000).wait();

    state.store.1.expiring.store(EXPIRING_AT_ONCE, SeqCst);
    pass();
    assert_eq!(waiting(&state, "b-quick"), 2, "every place taken: none set going, both still due");
    state.store.1.expiring.store(EXPIRING_AT_ONCE - 1, SeqCst);
    pass();
    assert_eq!(waiting(&state, "b-quick"), 1, "one place: one set going");
    assert_eq!(state.store.1.expiring.load(SeqCst), EXPIRING_AT_ONCE - 1, "its place given back as it went on");
    pass();
    assert_eq!(waiting(&state, "b-quick"), 0, "the other, at the next pass");
}
