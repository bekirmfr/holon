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
    // A function given a value too large for the record to keep and a state read for the first
    // time: the note names the read, not the large value's step (whose outputs the record lacks).
    ("a read beside a value too large to record", "{S}\nreturn \"end\"", "both(req.json.pad, state.none)", "{BIG}", Some("read")),
];

/// The body a line of `ORIGINS` is sent: `{BIG}` a field past what the record keeps of a step
/// (`recorder::Timer::MAX_RECORDED_BYTES`).
fn sent(body: &str) -> String {
    body.replace("{BIG}", &format!(r#"{{"pad":"{}"}}"#, "p".repeat(2 * crate::recorder::Timer::MAX_RECORDED_BYTES)))
}

/// A flow of one route per effect and origin, `/r<effect>_<origin>`.
fn flow_of(port: u16, host: &str) -> String {
    let mut text = format!(
        "import {{ action, flow, route }} from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\nimport api from \"host:{host}\"\n\nflow(\"skips\", {{ port: {port} }})\n\nfunction checked(n) {{\n  if (n <= 0) throw new Error(\"not more than 0\")\n  return n\n}}\n\nfunction both(first, second) {{\n  return [first, second]\n}}\n\nexport const ping = action((x) => ++state.pinged)\n"
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
/// with the level of where the absence began: a read that found nothing (and which read; one
/// beside a value too large for the record to keep is still the read, the large value's step not
/// taken for one that gave nothing), a field a value lacked, a step that failed, as far as the record sees into a map or a fold;
/// and says nothing where the flow's own choice left it out (an `if` not taken, an early return,
/// a throw under its own test that its own catch took, the side of a `??` not needed, a tie to a
/// value the flow tested), or where it was made. A throw no catch takes is a failure. The same
/// for every kind of effect: a write, a row, a line, a call, an action started.
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
            data_call(&state, &slug, "POST", &format!("/r{e}_{o}"), &sent(body));
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
            // A state read is kept as the state the run began with, not among its reads: the note
            // names it by its step, which is the read's and not the large value's.
            if *origin == "a read beside a value too large to record" && notes.first().is_some_and(|n| n["leaf"] != "store_read") {
                wrong.push(format!("{effect} | {origin} | the note names another step than the state read: {notes:?}"));
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
/// the one there or not as the request says, the other a read that finds nothing. And a write the
/// host refuses for a secret without it being computed from one: what a reply that sent the
/// secret back gave (`far` answers with what it is sent).
const SECRET_PLACES: &[(&str, &str)] = &[
    ("a write of what a reply sent back", "const got = await far.post(\"/e\", { body: k })\nstate.w = got.body.slice(8, 20)"),
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

/// The flow of [`SECRET_PLACES`], one route each, `/p<i>`: `{T}` true for a secret that starts
/// `live_`, `{S}` there for such a one; `far` a host that answers with what it is sent.
fn places_flow(far: &str) -> String {
    let routes: String = SECRET_PLACES
        .iter()
        .enumerate()
        .map(|(i, (_, body))| {
            let body = body.replace("{T}", "k.startsWith(\"live_\")").replace("{S}", "(k.startsWith(\"live_\") ? \"s\" : undefined)");
            let r#async = if body.contains("await ") { "async " } else { "" };
            format!("route.post(\"/p{i}\", {async}(req) => {{\n{body}\nreturn \"end\"\n}})\n\n")
        })
        .collect();
    format!(
        "import {{ flow, route }} from \"holon\"\nimport state from \"state\"\nimport notes from \"collection:notes\"\nimport k from \"secret:k\"\nimport far from \"host:{far}\"\n\nflow(\"either\", {{ port: {} }})\n\nfunction pick(x) {{\n  const s = k.startsWith(\"live_\") ? \"s\" : undefined\n  return {{ s, v: x + \"!\" }}\n}}\n\n{routes}",
        free_port()
    )
}

/// Where two runs that did the same may still read apart, older than `skipped` and allowed: each
/// named with the decision that allows it, still compared. A difference that is one of these is
/// said, not failed; the test fails on any other, and on one of these that no pair shows any more
/// (the record changed: take it out, so the list can't outlive what it allows).
const ALLOWED: &[(&str, &str)] = &[
    (
        "a refused effect's output",
        "the route's effect output is the refused write's (withheld) in one run and null where the write wasn't reached: allowed by the owner's decision of 2026-11-15 (a run's steps show which way a choice went, a secret's included; a grant on who reads them, not scrubbing), R125 closed on SV's reading: the refused write's step is entered and timed and the unreached one is not in the timeline, so the steps already tell the two apart to the same readers, and only holders of the grant on a run's steps (run.steps) read its outputs",
    ),
    (
        "an echoed reply's length",
        "a far side that sends the secret back answers with a content-length, and the record keeps a far side's headers as they came, so the reply says how long the secret is: a known limit kept by the owner's choice (R69, 2026-11-17: the log stays as it is, the docs say so, and who may read a run's details is what is controlled; tests::a_far_sides_spelling_says_whether_a_secret_has_a_letter_past_ascii)",
    ),
];

/// Which of [`ALLOWED`] the difference `d` (as `differs` words it) is: a run's output, withheld in
/// one run and absent in the other; or a value in a recorded reply that reads as each secret's
/// length (`lengths`), as text, and nothing else.
fn allowed(d: &str, lengths: (usize, usize)) -> Option<usize> {
    let withheld_or_none = d.starts_with(".outputs[") && d.contains("‹withheld: computed from a secret›") && (d.ends_with("| null") || d.contains(": null |"));
    let length = d.starts_with(".rounds[") && d.contains(".response.") && d.ends_with(&format!(": \"{}\" | \"{}\"", lengths.0, lengths.1));
    if withheld_or_none {
        Some(0)
    } else {
        length.then_some(1)
    }
}

/// **Guards:** two runs that differ only in something computed from a secret (which way a test
/// on it went, whether a value computed from it arrived), with everything clean the same, leave
/// records that read the same but for their steps (which show which way a choice went, as the
/// owner decided) and the differences `ALLOWED` names: their `skipped` field for field, in the
/// same order. Each place the walk can meet such a step, and a write the
/// host refuses for a reply that sent the secret back (`SECRET_PLACES`), with each request, run
/// with one secret and then another.
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
    let far = echoing_server(false);
    let slug = deploy_text(&state, &places_flow(&far), &far);
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
    let lengths = (secrets[0].len(), secrets[1].len());
    for (n, (one, other)) in records[0].iter().zip(&records[1]).enumerate() {
        let (place, body) = (SECRET_PLACES[n / bodies.len()].0, bodies[n % bodies.len()]);
        let words = |e: &serde_json::Value| e["skipped"].as_array().map_or("-".to_string(), |s| s.iter().map(|n| format!("{} {}", n["effect"].as_str().unwrap_or(""), n["level"].as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
        table.push(format!("{place:42} | {body:20} | {} | {}", words(one), words(other)));
        let mut at = Vec::new();
        differs(&but_marks(one), &but_marks(other), String::new(), &mut at);
        // The steps show which way a choice went, as the owner decided.
        at.retain(|d| !d.starts_with(".timeline"));
        // A difference `ALLOWED` names is said with the decision that allows it, not failed.
        at.retain(|d| match allowed(d, lengths) {
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

/// A deployment's export, its lines ten seconds apart (none ran beside another, `verify::beside`),
/// and its lines as JSON.
fn exported(state: &AppState, slug: &str) -> (verify::Export, Vec<serde_json::Value>) {
    let export: verify::Export = serde_json::from_value(body_json(route(state, &req("GET", &format!("/deployments/{slug}/export"), vec![])))).unwrap();
    let values = export
        .lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let mut v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["at_ms"] = (v["at_ms"].as_u64().unwrap() + 10_000 * i as u64).into();
            v
        })
        .collect();
    (export, values)
}

/// `holon verify` on `values`, line `line` changed by `change` (none: as they are), chained again:
/// what each run comes to, by seq.
fn verified(export: &verify::Export, values: &[serde_json::Value], line: Option<usize>, change: &dyn Fn(&mut serde_json::Value)) -> Vec<(u64, verify::Rerun)> {
    let mut lines: Vec<String> = Vec::new();
    for (j, v) in values.iter().enumerate() {
        let mut v = v.clone();
        if Some(j) == line {
            change(&mut v);
        }
        if let Some(before) = lines.last() {
            v["prev"] = recorder::line_hash(before).into();
        }
        lines.push(serde_json::to_string(&v).unwrap());
    }
    let head = recorder::line_hash(lines.last().unwrap());
    let forged = verify::Export { lines, head, ..serde_json::from_value(serde_json::to_value(export).unwrap()).unwrap() };
    verify::check(&forged, None, &BTreeMap::new()).reruns
}

/// What `holon verify` made of each run of a flow, as rows; a run it says differs is wrong. The
/// levels of the notes of the runs it ran again to the same are counted in `levels`.
fn tally(name: &str, reruns: &[(u64, verify::Rerun)], values: &[serde_json::Value], table: &mut Vec<String>, wrong: &mut Vec<String>, levels: &mut BTreeMap<String, usize>) {
    let (mut matched, mut not_run) = (0, 0);
    for (seq, rerun) in reruns {
        let line = values.iter().find(|v| v["seq"].as_u64() == Some(*seq));
        let at = line.map_or(String::new(), |v| format!("{} {}", v["kind"].as_str().unwrap_or(""), v["url"].as_str().unwrap_or("")));
        match rerun {
            verify::Rerun::Matched => {
                matched += 1;
                for note in line.and_then(|v| v["skipped"].as_array()).into_iter().flatten() {
                    *levels.entry(note["level"].as_str().unwrap_or("?").to_string()).or_default() += 1;
                }
            }
            verify::Rerun::Differs(why) => wrong.push(format!("{name}, {at} (seq {seq}): holon verify says it differs: {why}; its notes {}", line.map_or(serde_json::Value::Null, |v| v["skipped"].clone()))),
            verify::Rerun::Skipped(why) => {
                not_run += 1;
                table.push(format!("{name}, {at}: not run again: {why}"));
            }
            _ => {}
        }
    }
    table.push(format!("{name}: {} runs, {matched} run again to the same, {not_run} not run again", reruns.len()));
}

/// **Guards:** `holon verify` works each run's `skipped` out again, by the server's walk on the
/// steps it runs again and the refusals it can tell (a never-kept value in a write or a line, a
/// write computed from a secret, one of what a reply that sent a secret back gave or of the secret
/// itself), and holds the line to it: every run of the table's flow, of the
/// secret places' flow with each secret, and of a flow whose writes and lines are refused, that
/// verify runs again comes to its notes, every level among them; and a line changed and chained
/// again differs where its notes do (a note taken out, one put in, a level, a read or a rule
/// changed, an effect moved out of `performed` into `skipped`, a refused one said made). A run
/// that computed a true or false or a number from a secret isn't run again (no mark carries one),
/// and its notes are as unchecked as the rest of its line: the rows say which.
/// **Why:** a note is the server's claim about the run: unchecked, a line could drop the note of
/// a payout whose amount never came, or say a write was refused that was made (R99 stage 3).
/// **If it fails:** the run named came out other than its line says: the walk in verify isn't
/// the server's (what it hears of an absence, the steps it reads, the reads it names), or verify
/// notes a refusal the server didn't (or not one it did). "No run … at level" means the flows no
/// longer give a run verify can run again with such a note: give one back.
#[test]
fn verify_works_out_what_each_run_didnt_make_again() {
    let state = claims_state("");
    let (host, _hits) = counting_server();
    let (mut table, mut wrong) = (Vec::new(), Vec::new());
    let mut levels = BTreeMap::new();

    // The table's flow: every effect, every way of not arriving.
    let table_slug = deploy_text(&state, &flow_of(free_port(), &host), &host);
    for (e, _) in EFFECTS.iter().enumerate() {
        for (o, (_, _, _, body, _)) in ORIGINS.iter().enumerate() {
            data_call(&state, &table_slug, "POST", &format!("/r{e}_{o}"), &sent(body));
        }
    }
    let (table_export, table_values) = exported(&state, &table_slug);
    tally("the table", &verified(&table_export, &table_values, None, &|_| {}), &table_values, &mut table, &mut wrong, &mut levels);

    // The secret places' flow, with each secret.
    let far = echoing_server(false);
    let places = deploy_text(&state, &places_flow(&far), &far);
    for secret in ["live_alpha-key", "test_bravo-another-secret-value-22"] {
        let set = req("POST", &format!("/deployments/{places}/secrets/k"), serde_json::to_vec(&serde_json::json!({ "value": secret })).unwrap());
        assert_eq!(route(&state, &set).status_code, 200);
        for i in 0..SECRET_PLACES.len() {
            for body in [r#"{}"#, r#"{"v":"y","c":true}"#, r#"{"c":false}"#, r#"{"v":"y"}"#] {
                data_call(&state, &places, "POST", &format!("/p{i}"), body);
            }
        }
    }
    let (places_export, places_values) = exported(&state, &places);
    tally("the secret places", &verified(&places_export, &places_values, None, &|_| {}), &places_values, &mut table, &mut wrong, &mut levels);

    // Refused: a write and a line holding a value the flow never keeps, a write computed from a
    // secret too short to be a mark, a write of what a reply that sent the secret back gave (a
    // piece of it, and the secret whole, which the server refuses by that rule first).
    let text = format!(
        "import {{ flow, route }} from \"holon\"\nimport state from \"state\"\nimport k from \"secret:k\"\nimport far from \"host:{far}\"\n\nflow(\"refuses\", {{ port: {}, never_keep: [\"cvc\"] }})\n\nroute.post(\"/keep\", (req) => {{\n  state.last = req.json\n  return \"ok\"\n}})\n\nroute.post(\"/said\", (req) => {{\n  console.log(req.json)\n  return \"ok\"\n}})\n\nroute.post(\"/short\", (req) => {{\n  state.n = k.length\n  return \"ok\"\n}})\n\nroute.post(\"/echoed\", async (req) => {{\n  const got = await far.post(\"/e\", {{ body: k }})\n  state.p = got.body.slice(8, 20)\n  return \"ok\"\n}})\n\nroute.post(\"/echoed-whole\", async (req) => {{\n  const got = await far.post(\"/e\", {{ body: k }})\n  state.e = got.body\n  return \"ok\"\n}})\n",
        free_port()
    );
    let refuses = deploy_text(&state, &text, &far);
    let set = req("POST", &format!("/deployments/{refuses}/secrets/k"), serde_json::to_vec(&serde_json::json!({ "value": "s3cr3t-value-for-test" })).unwrap());
    assert_eq!(route(&state, &set).status_code, 200);
    for path in ["/keep", "/said", "/short", "/echoed", "/echoed-whole"] {
        data_call(&state, &refuses, "POST", path, r#"{"amount":1737,"cvc":"737"}"#);
    }
    let (refuses_export, refuses_values) = exported(&state, &refuses);
    tally("the refusals", &verified(&refuses_export, &refuses_values, None, &|_| {}), &refuses_values, &mut table, &mut wrong, &mut levels);
    for v in refuses_values.iter().filter(|v| v["kind"] == "request") {
        table.push(format!("the refusals, {}: performed {}, skipped {}", v["url"].as_str().unwrap_or(""), v["performed"], v["skipped"]));
    }
    // Each is noted, none a reader of `skipped` can't see: refused by its rule, or withheld where
    // it may carry what is computed from a secret.
    let notes = |url: &str| refuses_values.iter().find(|v| v["url"].as_str().is_some_and(|u| u.ends_with(url))).map_or(serde_json::Value::Null, |v| v["skipped"].clone());
    let said = [
        ("/keep", "refused", Some(crate::skipped::NEVER_KEPT)),
        ("/said", "refused", Some(crate::skipped::NEVER_KEPT)),
        ("/short", "withheld", None),
        ("/echoed", "refused", Some(crate::skipped::ECHOED)),
        ("/echoed-whole", "refused", Some(crate::skipped::SECRET_VALUE)),
    ];
    for (url, level, rule) in said {
        let n = notes(url);
        if n.as_array().map(Vec::len) != Some(1) || n[0]["level"] != level || n[0]["rule"].as_str() != rule {
            wrong.push(format!("the refusals, {url}: the line's notes are {n}, not one at level {level} by {rule:?}"));
        }
    }

    // Changed lines: each differs. `/r0_1`: a write whose read found nothing; `/r0_0`: a write made.
    let line_of = |values: &[serde_json::Value], url: &str| values.iter().position(|v| v["url"].as_str().is_some_and(|u| u.ends_with(url))).unwrap_or_else(|| panic!("no line for {url}"));
    let (read_line, made_line) = (line_of(&table_values, "/r0_1"), line_of(&table_values, "/r0_0"));
    let (keep_line, echoed_line) = (line_of(&refuses_values, "/keep"), line_of(&refuses_values, "/echoed"));
    let made = table_values[made_line]["performed"].clone();
    type Change = Box<dyn Fn(&mut serde_json::Value)>;
    let changes: Vec<(&str, &verify::Export, &[serde_json::Value], usize, Change)> = vec![
        ("a note taken out", &table_export, &table_values, read_line, Box::new(|v| v["skipped"] = serde_json::json!([]))),
        ("a note's level changed", &table_export, &table_values, read_line, Box::new(|v| v["skipped"][0]["level"] = "missing".into())),
        ("a note's read changed", &table_export, &table_values, read_line, Box::new(|v| v["skipped"][0]["read"] = 7.into())),
        ("a note put in", &table_export, &table_values, made_line, Box::new(|v| v["skipped"] = serde_json::json!([{ "effect": "Log(\"info\")", "level": "refused", "rule": "not granted" }]))),
        (
            "a write made moved into what it didn't make",
            &table_export,
            &table_values,
            made_line,
            Box::new(|v| {
                v["performed"] = serde_json::json!([]);
                v["skipped"] = serde_json::json!([{ "effect": "StateWrite(\"w\")", "level": "refused", "rule": "it holds a value its flow never keeps" }]);
            }),
        ),
        ("a refused write's note taken out", &refuses_export, &refuses_values, keep_line, Box::new(|v| v["skipped"] = serde_json::json!([]))),
        ("an echoed write's note taken out", &refuses_export, &refuses_values, echoed_line, Box::new(|v| v["skipped"] = serde_json::json!([]))),
        ("a refused write's rule changed", &refuses_export, &refuses_values, keep_line, Box::new(|v| v["skipped"][0]["rule"] = "not granted".into())),
        (
            "a refused write said made",
            &refuses_export,
            &refuses_values,
            keep_line,
            Box::new(move |v| {
                v["skipped"] = serde_json::json!([]);
                v["performed"] = made.clone();
            }),
        ),
    ];
    for (what, export, values, line, change) in changes {
        let seq = values[line]["seq"].as_u64().unwrap();
        let reruns = verified(export, values, Some(line), &change);
        let said = reruns.iter().find(|(s, _)| *s == seq).map(|(_, r)| r);
        table.push(format!("{what}: {said:?}"));
        if !matches!(said, Some(verify::Rerun::Differs(_))) {
            wrong.push(format!("{what} (seq {seq}), and holon verify says {said:?}, not that it differs"));
        }
    }
    // Each level is held in a run verify ran again (a run it can't run again holds none of its
    // line): else the comparison could pass by never being made.
    table.push(format!("the notes of the runs run again to the same, by level: {levels:?}"));
    for level in ["read", "missing", "failed", "withheld", "refused"] {
        if !levels.contains_key(level) {
            wrong.push(format!("no run verify ran again to the same has a note at level {level}: the comparison isn't made for it"));
        }
    }
    println!("{}", table.join("\n"));
    assert!(wrong.is_empty(), "{}:\n{}", wrong.len(), wrong.join("\n"));
}

/// The words `holon explain` says a note of each level with (agent.rs's `skipped_why`).
const EXPLAIN_SAYS: &[(&str, &str)] =
    &[("read", "found nothing"), ("missing", "gave nothing:"), ("failed", "failed"), ("withheld", "withheld:"), ("refused", "refused:")];

/// **Guards:** `holon explain` says why an effect the run held wasn't made by the server's note of
/// it (`skipped`), once, under the line that would have made it, for every kind of effect and
/// every way of not arriving in the level table; and where it still works out why a step has no
/// value itself (a step that isn't an effect), it never takes a step whose value was too large to
/// record for one that gave nothing: a function given a large value and a state read for the
/// first time is said to have got nothing from the read, as is the write it fed.
/// **Why:** explain worked every reason out from the record, the first step upstream with no
/// recorded value taken for the one that gave nothing. A step whose value was too large to record
/// has none, so a flow on rc.36 whose write was fed two large answers and a state read for the
/// first time was told the request had given nothing (2026-11-19).
/// **If it fails:** the row printed names the effect and the way it didn't arrive that explain
/// says otherwise than the record; for the function row, the line that names the wrong step.
#[test]
fn explain_says_why_by_the_records_note() {
    let state = claims_state("");
    let (host, _hits) = counting_server();
    let mut text = flow_of(free_port(), &host);
    text.push_str("\nroute.post(\"/slim\", (req) => {\n  const s = both(req.json.pad, state.none)\n  state.g = s\n  return \"end\"\n})\n");
    let slug = deploy_text(&state, &text, &host);
    let document = holon_author::agent::plan(&text).unwrap().document;
    let mut seen = None;
    gained(&state, &slug, &mut seen);
    let (mut rows, mut wrong) = (0, Vec::new());
    for (e, (effect, _, _)) in EFFECTS.iter().enumerate() {
        for (o, (origin, _, _, body, _)) in ORIGINS.iter().enumerate() {
            data_call(&state, &slug, "POST", &format!("/r{e}_{o}"), &sent(body));
            let entry = gained(&state, &slug, &mut seen).pop().unwrap_or_else(|| panic!("{effect} | {origin}: no entry"));
            let said = holon_author::agent::explain(&slug, &document, &entry).unwrap();
            let notes = entry["skipped"].as_array().cloned().unwrap_or_default();
            let not_made: Vec<&str> = said.lines().filter(|l| l.contains("not made: ")).collect();
            rows += 1;
            if not_made.len() != notes.len() {
                wrong.push(format!("{effect} | {origin}: {} note(s), explain says \"not made\" {} time(s):\n{said}", notes.len(), not_made.len()));
                continue;
            }
            for (note, line) in notes.iter().zip(&not_made) {
                let level = note["level"].as_str().unwrap_or("?");
                let words = EXPLAIN_SAYS.iter().find(|(l, _)| *l == level).map_or("?", |(_, w)| *w);
                if !line.contains(words) {
                    wrong.push(format!("{effect} | {origin}: the note {note} is said as {line:?}, without {words:?}"));
                }
            }
            // Beside a value too large to record, only the read that found nothing is said to
            // have given nothing.
            if sent(body).len() > crate::recorder::Timer::MAX_RECORDED_BYTES {
                for line in said.lines().filter(|l| l.contains("nothing")) {
                    if !line.contains("\"none\"") {
                        wrong.push(format!("{effect} | {origin}: a step other than the read is said to have given nothing: {line:?}\n{said}"));
                    }
                }
            }
        }
    }
    // A step that isn't an effect, fed a large value and a missing state: explain works out why
    // it has no value, and names the read.
    data_call(&state, &slug, "POST", "/slim", &sent("{BIG}"));
    let entry = gained(&state, &slug, &mut seen).pop().expect("the /slim run's entry");
    let said = holon_author::agent::explain(&slug, &document, &entry).unwrap();
    println!("/slim, explained:\n{said}");
    let nothing: Vec<&str> = said.lines().filter(|l| l.contains("nothing")).collect();
    if nothing.len() != 2 || nothing.iter().any(|l| !l.contains("\"none\"")) {
        wrong.push(format!("/slim: the function's line and the write's should each name the read, and only it:\n{said}"));
    }
    println!("{rows} runs of the level table explained");
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
}

/// **Guards:** a run that went on from a wait (`resume`) is explained over its own handler's
/// steps, with the note of the effect it didn't make under that effect's line, as its first part
/// is.
/// **Why:** explain placed only a request's, a clock's, a file's and a started action's runs
/// under their handler; a resumed run was placed over the whole flow, none of its steps found, and
/// `holon runs --run N` said it "recorded no steps", so why it wrote nothing showed only on its
/// first part (rc.36, 2026-11-19).
/// **If it fails:** the explained text printed shows what was placed.
#[test]
fn explain_shows_a_resumed_runs_steps_and_notes() {
    let state = claims_state("");
    let text = format!(
        "import {{ action, event, flow, route }} from \"holon\"\nimport tasks from \"collection:tasks\"\n\nflow(\"cards\", {{ port: {} }})\n\nroute.post(\"/propose\", (req) => {{\n  const started = card({{ id: req.json.id }})\n  return `/go/${{started.waiting.go}}`\n}})\n\nexport const card = action(async (asked) => {{\n  const go = await event(\"go\", {{ expires: \"1h\" }})\n  const found = tasks[\"never\"]\n  tasks[asked.id] = {{ ...found, state: \"done\", via: go.method }}\n  return \"end\"\n}})\n\nroute.get(\"/go/:token\", () => \"thanks\", {{ resumes: \"go\" }})\n",
        free_port()
    );
    let slug = deploy_plain_text(&state, &text);
    let document = holon_author::agent::plan(&text).unwrap().document;
    let (code, link, _) = data_call(&state, &slug, "POST", "/propose", r#"{"id":"t1"}"#);
    assert_eq!(code, 200, "{link}");
    let (code, said, _) = data_call(&state, &slug, "GET", link.trim_matches('"'), "");
    assert_eq!(code, 200, "{said}");
    let log = body_json(route(&state, &req("GET", &format!("/deployments/{slug}/log?limit=50"), vec![])));
    let entries = log["entries"].as_array().cloned().unwrap_or_default();
    let resumed = entries.iter().find(|e| e["kind"] == "resume").unwrap_or_else(|| panic!("no resumed run: {log}"));
    assert_eq!(resumed["skipped"].as_array().map(Vec::len), Some(1), "the resumed run notes the write it didn't make: {resumed}");
    let said = holon_author::agent::explain(&slug, &document, resumed).unwrap();
    println!("the resumed run, explained:\n{said}");
    assert!(!said.contains("recorded no steps"), "{said}");
    let under_write = said.lines().skip_while(|l| !l.contains("tasks[asked.id] =")).nth(1).unwrap_or("");
    assert!(under_write.contains("not made: ") && under_write.contains("found nothing"), "the note stands under the write's line: {said}");
}
