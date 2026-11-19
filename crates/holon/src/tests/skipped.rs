//! What a run's record says of an effect it held and didn't make (`skipped`, skipped.rs): every
//! kind of effect, crossed with every way its value can fail to arrive, through a deployed flow.
//!
//! To add a case: a new kind of effect is a line of `EFFECTS`, a new way for a value to be absent
//! a line of `ORIGINS` with the level its note takes (none for the flow's own choice). None needs
//! its answer written per effect.

use super::*;

/// The kinds of effect a handler makes, with `{V}` the value it is made from, and the start of
/// the name `performed` and `skipped` give it.
const EFFECTS: &[(&str, &str, &str)] = &[
    ("a write", "state.w = {V}", "StateWrite(\"w\")"),
    ("a row", "notes[\"k\"] = {V}", "CollectionWrite(\"notes\")"),
    ("a line", "console.log({V})", "Log("),
    ("a call", "const sent = await api.post(\"/hit\", { body: {V} })", "Http"),
    ("an action started", "const started = ping({V})", ""),
];

/// The ways a value may not arrive, as a handler's body (`{S}` the effect's statement, `{V}` its
/// value), the request that makes it so, and the level of the note: none where the flow's own
/// choice left it out, or where it was made.
const ORIGINS: &[(&str, &str, &str, &str, Option<&str>)] = &[
    ("made", "{S}\nreturn \"end\"", "req.json.v", r#"{"v":"x"}"#, None),
    ("a read that found nothing", "{S}\nreturn \"end\"", "notes[\"none\"]", r#"{}"#, Some("read")),
    ("a field the request lacks", "{S}\nreturn \"end\"", "req.json.v", r#"{}"#, Some("missing")),
    ("a field of a field", "{S}\nreturn \"end\"", "req.json.a.b", r#"{"a":{}}"#, Some("missing")),
    ("a body that isn't JSON", "{S}\nreturn \"end\"", "req.json.v", "not json", Some("missing")),
    ("an if not taken", "if (req.json.c) {\n{S}\n}\nreturn \"end\"", "\"x\"", r#"{"c":false}"#, None),
    ("an if on nothing", "if (req.json.c) {\n{S}\n}\nreturn \"end\"", "\"x\"", r#"{}"#, Some("missing")),
    ("after an early return", "if (req.json.c) return \"stop\"\n{S}\nreturn \"end\"", "\"x\"", r#"{"c":true}"#, None),
    ("the other side of ??", "const got = req.json.v ?? \"x\"\n{S}\nreturn \"end\"", "got", r#"{"v":"y"}"#, None),
    ("tied to a value not there", "if (req.json.v !== undefined) {\n{S}\n}\nreturn \"end\"", "req.json.v", r#"{}"#, None),
    ("a throw its own catch took", "try {\nif (req.json.n <= 0) throw new Error(\"no\")\n{S}\n} catch {\nreturn \"bad\"\n}\nreturn \"end\"", "req.json.n", r#"{"n":0}"#, None),
    ("a missing field in a try", "try {\nconst n = req.json.n + 1\n{S}\n} catch {\nreturn \"bad\"\n}\nreturn \"end\"", "n", r#"{}"#, Some("missing")),
    ("a function that throws", "{S}\nreturn \"end\"", "checked(req.json.n)", r#"{"n":0}"#, Some("failed")),
    ("a map, as far as the record sees", "{S}\nreturn \"end\"", "req.json.items.map((i) => i.n)", r#"{"items":[{"m":1}]}"#, Some("missing")),
    ("a fold, as far as the record sees", "{S}\nreturn \"end\"", "req.json.items.reduce((a, i) => a + i.n, 0)", r#"{"items":[{"m":1}]}"#, Some("missing")),
];

/// A flow of one route per effect and origin, `/r<effect>_<origin>`.
fn flow_of(port: u16, host: &str) -> String {
    let mut text = format!(
        "import {{ action, flow, route }} from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\nimport api from \"host:{host}\"\n\nflow(\"skips\", {{ port: {port} }})\n\nfunction checked(n) {{\n  if (n <= 0) throw new Error(\"not more than 0\")\n  return n\n}}\n\nexport const ping = action((x) => ++state.pinged)\n"
    );
    for (e, (_, statement, _)) in EFFECTS.iter().enumerate() {
        for (o, (_, body, value, _, _)) in ORIGINS.iter().enumerate() {
            let statement = statement.replace("{V}", value);
            let body = body.replace("{S}", &statement);
            let r#async = if body.contains("await ") { "async " } else { "" };
            text.push_str(&format!("\nroute.post(\"/r{e}_{o}\", {async}(req) => {{\n{body}\n}})\n"));
        }
    }
    text
}

/// The request entries `slug`'s log gained after `seen`.
fn gained(state: &AppState, slug: &str, seen: &mut Option<u64>) -> Vec<serde_json::Value> {
    let after = seen.map(|seq| format!("after={seq}&")).unwrap_or_default();
    let got = body_json(route(state, &req("GET", &format!("/deployments/{slug}/log?{after}limit=50"), vec![])));
    let entries = got["entries"].as_array().unwrap().clone();
    for e in &entries {
        *seen = (*seen).max(e["seq"].as_u64());
    }
    entries.into_iter().filter(|e| e["kind"] == "request").collect()
}

/// **Guards:** a run that held an effect and didn't make it says so in its record (`skipped`),
/// with the level of where the absence began: a read that found nothing (and which read), a field
/// a value lacked, a step that failed, as far as the record sees into a map or a fold; and says
/// nothing where the flow's own choice left it out (an `if` not taken, an early return, a throw
/// under its own test that its own catch took, the side of a `??` not needed, a tie to a value the
/// flow tested), or where it was made. A throw no catch takes is a failure. The same for
/// every kind of effect: a write, a row, a line, a call, an action started.
/// **Why:** an effect whose value didn't arrive was dropped without a word: the run answered 200,
/// `performed` was empty, and a person reading the record couldn't tell "the write was never
/// meant" from "the write was meant and its value never came" (register item R99).
/// **If it fails:** `skipped::of` misses an effect kind (`effect_of`), or `absence::origin` names
/// the wrong level for one way of not arriving; the table printed says which.
#[test]
fn a_run_says_which_effects_it_held_and_didnt_make() {
    let state = claims_state("");
    let (host, _hits) = counting_server();
    let slug = deploy_text(&state, &flow_of(free_port(), &host), &host);
    let mut seen = None;
    gained(&state, &slug, &mut seen);
    let mut table = Vec::new();
    let mut wrong = Vec::new();
    for (e, (effect, _, name)) in EFFECTS.iter().enumerate() {
        for (o, (origin, _, _, body, level)) in ORIGINS.iter().enumerate() {
            data_call(&state, &slug, "POST", &format!("/r{e}_{o}"), body);
            let entries = gained(&state, &slug, &mut seen);
            let entry = entries.last().unwrap_or_else(|| panic!("{effect} | {origin}: no entry"));
            let notes = entry["skipped"].as_array().cloned().unwrap_or_default();
            let said: Vec<String> = notes.iter().map(|n| {
                let mut s = n["level"].as_str().unwrap_or("?").to_string();
                if n.get("read").is_some() {
                    s.push_str(" (a read)");
                }
                if n["unseen"] == true {
                    s.push_str(" (as far as the record sees)");
                }
                s
            }).collect();
            let named = notes.iter().all(|n| n["effect"].as_str().is_some_and(|x| x.starts_with(name)));
            let got = notes.first().and_then(|n| n["level"].as_str());
            table.push(format!("{effect:18} | {origin:34} | {}", if said.is_empty() { "-".to_string() } else { said.join(", ") }));
            if got != *level || notes.len() > 1 || !named {
                wrong.push(format!("{effect} | {origin} | expected {level:?}, the record says {notes:?}"));
            }
            if *origin == "a read that found nothing" && notes.first().is_some_and(|n| n.get("read").is_none()) {
                wrong.push(format!("{effect} | {origin} | the note doesn't name the read"));
            }
            if *origin == "made" && *effect == "a write" && entry["performed"].as_array().is_none_or(|p| p.is_empty()) {
                wrong.push(format!("{effect} | made | performed is empty for a run that wrote: {entry}"));
            }
        }
    }
    println!("{}", table.join("\n"));
    assert!(wrong.is_empty(), "{} of {} rows:\n{}", wrong.len(), table.len(), wrong.join("\n"));
}

/// The places the walk can meet something computed from a secret, as handler bodies: `{T}` a
/// test on the secret (true with one secret, false with the other), `{S}` a value computed from it
/// that arrives with one secret and not the other; `req.json.v` and `notes["none"]` are clean,
/// the one there or not as the request says, the other a read that finds nothing.
const SECRET_PLACES: &[(&str, &str)] = &[
    ("a trigger's guard", "if ({T}) {\nstate.w = req.json.v\n}"),
    ("a trigger's guard, nothing clean missing", "if ({T}) {\nstate.w = \"x\"\n}"),
    ("a guard's value", "state.w = req.json.c ? {S} : undefined"),
    ("a guard's condition", "state.w = {T} ? req.json.v : undefined"),
    ("a guard's condition beside a clean one", "if ({T} && req.json.c) {\nstate.w = req.json.v\n}"),
    ("the left of ??", "state.w = {S} ?? req.json.v"),
    ("the right of ??", "state.w = req.json.v ?? {S}"),
    ("the left of &&", "if ({T} && req.json.c) {\nstate.w = 1\n}"),
    ("the right of &&", "if (req.json.c && {T}) {\nstate.w = 1\n}"),
    ("the left of ||", "if ({T} || req.json.c) {\nstate.w = 1\n}"),
    ("the right of ||", "if (req.json.c || {T}) {\nstate.w = 1\n}"),
    ("a try", "try {\nconst got = {S}\nstate.w = got\n} catch {\nreturn \"bad\"\n}"),
    ("a try beside a clean step", "try {\nconst got = {S}\nconst v = req.json.v\nstate.w = { got, v }\n} catch {\nreturn \"bad\"\n}"),
    ("an input before a clean absent one", "state.w = { s: {S}, n: notes[\"none\"] }"),
    ("an input after a clean absent one", "state.w = { n: notes[\"none\"], s: {S} }"),
    ("an input before a clean one", "state.w = { s: {S}, v: req.json.v }"),
    ("inside a function", "state.w = pick(req.json.v)"),
    ("under map", "state.w = [\"live_\", \"test_\"].map((p) => (k.startsWith(p) ? p : undefined))"),
    ("a row", "if ({T}) {\nnotes[\"k\"] = req.json.v\n}"),
];

/// Where two runs that did the same may still read apart, older than `skipped` and allowed: each
/// named with the decision that allows it, still compared. A difference that is one of these is
/// said, not failed; the test fails on any other, and on one of these that no pair shows any more
/// (the record changed: take it out, so the list can't outlive what it allows).
const ALLOWED: &[(&str, &str)] = &[(
    "a refused effect's output",
    "the route's effect output is the refused write's (withheld) in one run and null where the write wasn't reached: allowed by the owner's decision of 2026-11-15 (a run's steps show which way a choice went, a secret's included; a grant on who reads them, not scrubbing), R125 closed on SV's reading: the refused write's step is entered and timed and the unreached one is not in the timeline, so the steps already tell the two apart to the same readers, and only holders of the grant on a run's steps (run.steps) read its outputs",
)];

/// Which of [`ALLOWED`] the difference `d` (as `differs` words it) is: a run's output, withheld in
/// one run and absent in the other.
fn allowed(d: &str) -> Option<usize> {
    let withheld_or_none = d.starts_with(".outputs[") && d.contains("‹withheld: computed from a secret›") && (d.ends_with("| null") || d.contains(": null |"));
    withheld_or_none.then_some(0)
}

/// **Guards:** two runs that differ only in something computed from a secret (which way a test
/// on it went, whether a value computed from it arrived), with everything clean the same, leave
/// records that read the same but for their steps (which show which way a choice went, as the
/// owner decided) and the differences `ALLOWED` names: their `skipped` field for field, in
/// the same order. Each place the walk can meet such a step (`SECRET_PLACES`), with each request,
/// run with one secret and then another.
/// **Why:** otherwise a run's record answers yes or no about the secret, one guess a run: the
/// walk read whether a step computed from a secret gave anything before it asked whether it may,
/// and a write a secret decided read "refused" where it was made and the host refused it, and
/// "withheld" where it wasn't.
/// **If it fails:** the place and the fields named differ between the two runs: the walk reads a
/// step `Seen` says is withheld, or a refusal of an effect that may be computed from a secret is
/// said otherwise than its absence.
#[test]
fn a_runs_skipped_effects_read_the_same_whatever_its_secret() {
    let state = claims_state("");
    let routes: String = SECRET_PLACES
        .iter()
        .enumerate()
        .map(|(i, (_, body))| {
            let body = body.replace("{T}", "k.startsWith(\"live_\")").replace("{S}", "(k.startsWith(\"live_\") ? \"s\" : undefined)");
            format!("route.post(\"/p{i}\", (req) => {{\n{body}\nreturn \"end\"\n}})\n\n")
        })
        .collect();
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\nimport k from \"secret:k\"\n\nflow(\"either\", {{ port: {} }})\n\nfunction pick(x) {{\n  const s = k.startsWith(\"live_\") ? \"s\" : undefined\n  return {{ s, v: x + \"!\" }}\n}}\n\n{routes}",
        free_port()
    );
    let slug = deploy_plain_text(&state, &text);
    let bodies = [r#"{}"#, r#"{"v":"y","c":true}"#, r#"{"c":false}"#, r#"{"v":"y"}"#];
    let secrets = ["live_alpha-key", "test_bravo-another-secret-value-22"];
    let mut seen = None;
    let mut records: Vec<Vec<serde_json::Value>> = Vec::new();
    for secret in secrets {
        let set = req("POST", &format!("/deployments/{slug}/secrets/k"), serde_json::to_vec(&serde_json::json!({ "value": secret })).unwrap());
        assert_eq!(route(&state, &set).status_code, 200);
        gained(&state, &slug, &mut seen);
        let mut these = Vec::new();
        for i in 0..SECRET_PLACES.len() {
            for body in bodies {
                data_call(&state, &slug, "POST", &format!("/p{i}"), body);
                let entries = gained(&state, &slug, &mut seen);
                these.push(entries.last().cloned().unwrap_or_else(|| panic!("/p{i} {body}: no entry")));
            }
        }
        records.push(these);
    }
    let (mut wrong, mut table) = (Vec::new(), Vec::new());
    let mut shown = vec![0; ALLOWED.len()];
    for (n, (one, other)) in records[0].iter().zip(&records[1]).enumerate() {
        let (place, body) = (SECRET_PLACES[n / bodies.len()].0, bodies[n % bodies.len()]);
        let words = |e: &serde_json::Value| e["skipped"].as_array().map_or("-".to_string(), |s| s.iter().map(|n| format!("{} {}", n["effect"].as_str().unwrap_or(""), n["level"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
        table.push(format!("{place:42} | {body:20} | {} | {}", words(one), words(other)));
        let mut at = Vec::new();
        differs(&but_marks(one), &but_marks(other), String::new(), &mut at);
        // The steps show which way a choice went, as the owner decided.
        at.retain(|d| !d.starts_with(".timeline"));
        // A difference `ALLOWED` names is said with the decision that allows it, not failed.
        at.retain(|d| match allowed(d) {
            Some(k) => {
                shown[k] += 1;
                false
            }
            None => true,
        });
        if !at.is_empty() {
            wrong.push(format!("{place} | {body}:\n  {}", at.join("\n  ")));
        }
    }
    println!("{}", table.join("\n"));
    for (k, (what, why)) in ALLOWED.iter().enumerate() {
        println!("allowed, {what}, in {} pairs: {why}", shown[k]);
        if shown[k] == 0 {
            wrong.push(format!("{what}: no pair reads apart there any more; take it out of ALLOWED"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} pairs of runs read differently:\n{}", wrong.len(), table.len(), wrong.join("\n"));
}

/// **Guards:** a call that carries a secret as it is, or computed from it (a Basic header), to a
/// host it is granted, and whose body a read found nothing for, isn't sent; its record names the
/// read, with either secret. The rule withholds what the walk would read of a secret, never "a
/// secret near the effect".
/// **Why:** a payout not sent because its amount wasn't found is the case this record is for.
/// **If it fails:** the walk withholds a call's origin for its credential.
#[test]
fn a_call_with_its_credential_still_names_the_read_it_waited_on() {
    let state = claims_state("");
    let (host, _hits) = counting_server();
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport notes from \"collection:notes\"\nimport k from \"secret:k\"\nimport api from \"host:{host}\"\n\nflow(\"payout\", {{ port: {} }})\n\nroute.post(\"/as-is\", async (req) => {{\n  const sent = await api.post(\"/pay\", {{ headers: {{ authorization: k }}, body: notes[\"none\"] }})\n  return \"end\"\n}})\n\nroute.post(\"/basic\", async (req) => {{\n  const sent = await api.post(\"/pay\", {{ headers: {{ authorization: `Basic ${{btoa(`user:${{k}}`)}}` }}, body: notes[\"none\"] }})\n  return \"end\"\n}})\n",
        free_port()
    );
    let slug = deploy_text(&state, &text, &host);
    let mut seen = None;
    for secret in ["live_alpha-key", "test_bravo-another-secret-value-22"] {
        let set = req("POST", &format!("/deployments/{slug}/secrets/k"), serde_json::to_vec(&serde_json::json!({ "value": secret })).unwrap());
        assert_eq!(route(&state, &set).status_code, 200);
        gained(&state, &slug, &mut seen);
        for path in ["/as-is", "/basic"] {
            data_call(&state, &slug, "POST", path, "{}");
            let entries = gained(&state, &slug, &mut seen);
            let notes = entries.last().unwrap()["skipped"].clone();
            println!("{path}: {notes}");
            assert_eq!(notes.as_array().map(Vec::len), Some(1), "{path}: {notes}");
            assert_eq!(notes[0]["level"], "read", "{path}: {notes}");
            assert!(notes[0].get("read").is_some(), "{path}: {notes}");
        }
    }
}

/// **Guards:** an effect the host refuses is said in `skipped` at level "refused", naming the rule
/// in fixed words and holding no value; and an absence that may be computed from a secret (a read
/// keyed by one) is said at level "withheld", with nothing of where it began.
/// **Why:** a refusal was said on stderr alone; and where an absence began can say something of a
/// secret (whether a row keyed by it exists).
/// **If it fails:** firing.rs's refusals don't push their note, or `skipped::of` names a step fed
/// by a secret.
#[test]
fn a_refused_effect_and_a_secrets_absence_are_said_without_their_value() {
    let state = claims_state("");
    let port = free_port();
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\nimport k from \"secret:k\"\n\nflow(\"refuses\", {{ port: {port}, never_keep: [\"cvc\"] }})\n\nroute.post(\"/keep\", (req) => {{\n  state.last = req.json\n  return \"ok\"\n}})\n\nroute.post(\"/keyed\", (req) => {{\n  state.found = notes[k]\n  return \"ok\"\n}})\n"
    );
    let slug = deploy_plain_text(&state, &text);
    let set = req("POST", &format!("/deployments/{slug}/secrets/k"), serde_json::to_vec(&serde_json::json!({ "value": "s3cr3t-value-for-test" })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);
    let mut seen = None;
    gained(&state, &slug, &mut seen);
    data_call(&state, &slug, "POST", "/keep", r#"{"amount":1737,"cvc":"737"}"#);
    let kept = gained(&state, &slug, &mut seen);
    let notes = kept.last().unwrap()["skipped"].clone();
    println!("refused: {notes}");
    assert_eq!(notes, serde_json::json!([{ "effect": "StateWrite(\"last\")", "level": "refused", "rule": "it holds a value its flow never keeps" }]));
    data_call(&state, &slug, "POST", "/keyed", "{}");
    let keyed = gained(&state, &slug, &mut seen);
    let notes = keyed.last().unwrap()["skipped"].clone();
    println!("withheld: {notes}");
    assert_eq!(notes[0]["level"], "withheld", "{notes}");
    assert!(notes[0].get("from").is_none() && notes[0].get("leaf").is_none() && notes[0].get("read").is_none(), "{notes}");
    assert!(!notes.to_string().contains("s3cr3t"), "{notes}");
}
