//! A request a run made is answered from its record when the run asks it again with another value
//! of a secret it holds (R113): set while the run waited, or between two tries of one part. The
//! server tells requests apart with each secret's value written as its name (`secrets::Named`),
//! keeps that key with each exchange it sends (a waiting run's record), and `holon verify` and a
//! replay ask the record by the same key, each secret's mark by its name alone.

use super::*;

/// A made-up key: random, a test's own, never a real one.
fn made_up_key() -> String {
    format!("sk_test_{:032x}", rand::random::<u128>())
}

/// `paid-key`: its action `charge` authorizes with the key as a Bearer token (`/authorize`), or,
/// `basic`, with a Basic credential, base64 of the key computed in the flow (`/lookup`); then it
/// waits for the event "paid" and captures with the key as it then is.
fn charge_text(host: &str, basic: bool) -> String {
    let (path, credential, leaves) = match basic {
        true => ("/lookup", "`Basic ${base64(api_key)}`", "action, base64, event, flow, route"),
        false => ("/authorize", "`Bearer ${api_key}`", "action, event, flow, route"),
    };
    format!(
        r#"import {{ {leaves} }} from "holon"
import api_key from "secret:api_key"
import host from "host:{host}"

flow("paid-key", {{ port: 18241 }})

route.post("/hold", () => {{
  const started = charge()
  return {{ token: started.waiting.paid }}
}})

export const charge = action(async () => {{
  const authorized = await host.post("{path}", {{
    headers: {{ authorization: {credential} }},
    body: {{ amount: 100 }},
  }})
  const paid = await event("paid", {{ expires: "1h" }})
  const captured = await host.post("/capture", {{
    headers: {{ authorization: `Bearer ${{api_key}}`, "x-paid": JSON.stringify(paid.body) }},
    body: {{ amount: 100, authorized: authorized.status }},
  }})
  return captured.status
}})

route.get("/paid/:token", () => "ok", {{ resumes: "paid" }})
"#
    )
}

/// A run of `paid-key` (`basic` as for [`charge_text`]) that waits, the key set anew while it
/// waits, then given its event: its state and slug, and the counting far side's hits.
fn charged_across_a_new_key(basic: bool) -> (AppState, String, Hits) {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let slug = deploy_text(&state, &charge_text(&host, basic), &host);
    set_key(&state, &slug, &made_up_key());
    let (status, held, _) = data_call(&state, &slug, "POST", "/hold", "");
    assert_eq!(status, 200, "{held}");
    let token = serde_json::from_str::<serde_json::Value>(&held).unwrap()["token"].as_str().unwrap().to_string();
    let first = if basic { "/lookup" } else { "/authorize" };
    assert_eq!((hits_of(&hits, first), hits_of(&hits, "/capture")), (1, 0), "it called out and waits");
    set_key(&state, &slug, &made_up_key());
    let (status, body, _) = data_call(&state, &slug, "GET", &format!("/paid/{token}"), "");
    assert_eq!(status, 200, "{body}");
    assert_eq!(hits_of(&hits, "/capture"), 1);
    (state, slug, hits)
}

/// Its record, exported, checks offline.
fn verifies(state: &AppState, slug: &str) {
    let export: verify::Export = serde_json::from_value(body_json(route(state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
}

fn set_key(state: &AppState, slug: &str, value: &str) {
    let body = serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap();
    assert_eq!(route(state, &req("POST", &format!("/deployments/{slug}/secrets/api_key"), body)).status_code, 200);
}

/// The class, across a wait: a run authorizes with the key and waits; the key is set anew; the run
/// is given its event. Its resumed part runs from its start again, asking the authorization with
/// the new key: it is answered from its record, not sent again, and the capture goes out once,
/// with the new key. Its record checks offline (`holon verify`) and replays as it verifies.
/// If it fails: a count of 2 for `/authorize` is the double send R113 named: the resumed part's
/// requests were told apart by the key's value (`run_rounds` asked `prior` by the whole request,
/// or the exchanges kept no key, or `firing::run_attempt` passed `Whole`). A verify or a replay
/// that fails reads the record by the whole request (`Marks` as `Keys`).
#[test]
fn a_resumed_run_asking_with_a_new_value_of_its_key_sends_nothing_again() {
    let (state, slug, hits) = charged_across_a_new_key(false);
    assert_eq!(hits_of(&hits, "/authorize"), 1, "answered from the record");
    let entries = log_entries(&state, &slug);
    let resumed = entries.iter().find(|e| e["kind"] == "resume").unwrap_or_else(|| panic!("{entries:#?}"));
    assert!(resumed["error"].is_null(), "{resumed}");
    let sent = resumed["rounds"].to_string();
    assert!(sent.contains("/capture") && !sent.contains("/authorize"), "the resumed part sent the capture alone: {sent}");
    assert!(sent.contains("‹secret:api_key v2 #"), "with the key as it then was: {sent}");
    verifies(&state, &slug);
    assert!(replays_as_it_verifies(&state, &slug).contains("resume"));
}

/// The limit the fix states (`secrets::Named`): a request that holds a value computed from the key
/// (a Basic credential, base64 of it, computed in the flow) is told apart by that value, so asked
/// again with the key set anew it is another request, and sent again. The record marks such a
/// value by a print of its own, which `holon verify` can't put a name to: the server doesn't
/// either, so the record still checks offline. (A replay can't tell such a run at all: it
/// computes with the key's value.)
/// If it fails: a count of 1 means a computed value is now named too. The record must then mark it
/// so `holon verify` keys it by name, or the check fails (a call its line records that no node
/// makes run again); and the docs that state the limit change with it (durable-runs.md,
/// LANGUAGE.md, the manual's secrets page).
#[test]
fn a_value_computed_from_a_key_set_anew_is_another_request() {
    let (state, slug, hits) = charged_across_a_new_key(true);
    assert_eq!(hits_of(&hits, "/lookup"), 2, "sent again: the stated limit");
    verifies(&state, &slug);
}

/// The class, the same bytes named another way: a request is answered from the record when it is
/// the same as one the run sent, byte for byte, whatever its key now says. A far side sends the key
/// back (`/ascii` answers with what it was sent), and the run builds its next request from that
/// reply, both before it waits; the key is set anew while it waits. The resumed part asks the
/// first again with the new key (answered by its key) and gets the reply as it was, the old value
/// in it, so it asks the second again the same byte for byte: sent before, the value was the
/// key's, and its key named it; now it is an older value. It is answered from the record, the far
/// side is hit once, and the record checks offline (`holon verify` names each of the key's values
/// by its mark, so it answers it from the record too: a call sent again would be "a call its line
/// records that no node makes run again").
/// (A run started with the key's value in its input, a gateway passing its caller's credential on
/// to an action, is refused before it starts: a secret isn't sent to another flow.)
/// If it fails: a count of 2 is the double send main never made: `run_rounds` answers a request
/// only by its key, and not also by the request as it was sent.
#[test]
fn a_request_asked_again_the_same_byte_for_byte_is_answered_from_the_record() {
    let (host, hits) = counting_server();
    let state = claims_state("");
    let text = format!(
        r#"import {{ action, event, flow, route }} from "holon"
import api_key from "secret:api_key"
import host from "host:{host}"

flow("echo-key", {{ port: 18243 }})

route.post("/hold", () => {{
  const started = forward()
  return {{ token: started.waiting.paid }}
}})

export const forward = action(async () => {{
  const echoed = await host.post("/ascii", {{
    headers: {{ authorization: `Bearer ${{api_key}}` }},
    body: api_key,
  }})
  const looked = await host.post("/lookup", {{
    headers: {{ authorization: `Bearer ${{echoed.body}}` }},
    body: {{ amount: 100 }},
  }})
  const paid = await event("paid", {{ expires: "1h" }})
  const captured = await host.post("/capture", {{
    headers: {{ authorization: `Bearer ${{api_key}}`, "x-paid": JSON.stringify(paid.body) }},
    body: {{ amount: 100, looked: looked.status }},
  }})
  return captured.status
}})

route.get("/paid/:token", () => "ok", {{ resumes: "paid" }})
"#
    );
    let slug = deploy_text(&state, &text, &host);
    set_key(&state, &slug, &made_up_key());
    let (status, held, _) = data_call(&state, &slug, "POST", "/hold", "");
    assert_eq!(status, 200, "{held}");
    let token = serde_json::from_str::<serde_json::Value>(&held).unwrap()["token"].as_str().unwrap().to_string();
    assert_eq!((hits_of(&hits, "/ascii"), hits_of(&hits, "/lookup"), hits_of(&hits, "/capture")), (1, 1, 0), "it called out and waits");
    set_key(&state, &slug, &made_up_key());
    let (status, body, _) = data_call(&state, &slug, "GET", &format!("/paid/{token}"), "");
    assert_eq!(status, 200, "{body}");
    assert_eq!((hits_of(&hits, "/ascii"), hits_of(&hits, "/capture")), (1, 1), "the first answered by its key; the capture sent");
    // Both said at once: the count, and what the check finds.
    let export: verify::Export = serde_json::from_value(body_json(route(&state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let report = verify::check(&export, None, &BTreeMap::new());
    let lookups = hits_of(&hits, "/lookup");
    assert!(lookups == 1 && report.ok(), "/lookup hit {lookups} times (once: the same bytes, answered from the record); the check: {}", verify::describe(&report, None));
}

/// The key a request is told apart by holds nothing of the secret's value: two runs that differ
/// only in the key's value, each waiting, keep the same keys with their requests, wherever the
/// key is written (the waiting run's record, as the deployments file keeps it).
#[test]
fn what_a_request_is_told_apart_by_is_the_same_for_any_value_of_its_secret() {
    let (host, _) = counting_server();
    let state = claims_state("");
    let slug = deploy_text(&state, &charge_text(&host, false), &host);
    let (mut kept, mut seen) = (Vec::new(), BTreeSet::new());
    // Of several lengths: the key doesn't say how long the value is either.
    for value in [made_up_key(), format!("{}{}", made_up_key(), made_up_key()), "sk_test_short".to_string()] {
        set_key(&state, &slug, &value);
        let (status, held, _) = data_call(&state, &slug, "POST", "/hold", "");
        assert_eq!(status, 200, "{held}");
        let parked = state.deployments.lock().unwrap()[&slug].parked.values().find(|p| !seen.contains(&p.run_id)).cloned().unwrap();
        seen.insert(parked.run_id.clone());
        let keys: Vec<Option<String>> = parked.rounds.iter().flatten().map(|x| x.key.clone()).collect();
        assert!(matches!(keys.as_slice(), [Some(_)]), "the call before the wait keeps its key: {keys:?}");
        kept.push(keys);
    }
    assert!(kept.windows(2).all(|w| w[0] == w[1]), "one key for any value: {kept:?}");
}

/// The class, between two tries of one part: a run reads a file, calls out with the key, and
/// while its call is out the key is set anew and the file changed; its commit finds the file
/// changed, and it runs again, asking the call with the new key. It is answered from the first
/// try's record, not sent again.
/// If it fails: a count of 2 is the double send: the second try's request was told apart by the
/// key's value (`run_attempt` keyed it with `Whole`, or the first try's exchanges kept no key).
#[test]
fn a_run_tried_again_with_a_new_value_of_its_key_sends_nothing_again() {
    let (host, hits, gate) = gated_counting_server();
    let state = claims_state("");
    let dir = std::env::temp_dir().join(format!("holon-key-lines-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lines.txt"), "first\n").unwrap();
    let body = serde_json::json!({ "name": "ledger", "path": dir.display().to_string(), "access": "read-write" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let text = format!(
        r#"import {{ flow, route }} from "holon"
import ledger from "folder:ledger"
import api_key from "secret:api_key"
import host from "host:{host}"

flow("key-lines", {{ port: 18242 }})

route.post("/add", async () => {{
  const before = ledger["lines.txt"] ?? ""
  const later = (await host.get("/slow", {{
    headers: {{ authorization: `Bearer ${{api_key}}` }},
  }})).body
  ledger["lines.txt"] = `${{before}}${{later}}\n`
  return "ok"
}})
"#
    );
    let slug = deploy_text(&state, &text, &host);
    set_key(&state, &slug, &made_up_key());
    let (added, ()) = second_while_first_is_out(&gate, || data_call(&state, &slug, "POST", "/add", ""), || {
        set_key(&state, &slug, &made_up_key());
        std::fs::write(dir.join("lines.txt"), "first\nsecond\n").unwrap();
    });
    assert_eq!(added.0, 200, "{added:?}");
    assert_eq!(std::fs::read_to_string(dir.join("lines.txt")).unwrap(), "first\nsecond\nslow\n", "it ran again on the file as it now is");
    assert_eq!(hits_of(&hits, "/slow"), 1, "its request went out once");
    let entries = log_entries(&state, &slug);
    let ran = entries.last().unwrap();
    assert_eq!(ran["retries"], 1, "{ran}");
    let _ = std::fs::remove_dir_all(&dir);
}
