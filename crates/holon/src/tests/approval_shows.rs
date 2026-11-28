//! What an approval shows those who decide it, through the real gate and handlers (the launch
//! review's H6, docs/L4s/access-and-approvals-L4.md §12): what its `shows` names of the run, or else
//! the run's input; kept as the run's log keeps it, sealed on disk with the run; and listed to
//! whoever decides it, who sees nothing else of the flow's runs.

use super::access_table::ask;
use super::*;
use crate::accounts::Accounts;
use crate::bindings::{RoleText, Scope, Selector, Subject};
use serde_json::{json, Value};

const PW: &str = "correct horse battery";
const SECRET: &str = "sk_live_kept_from_deciders";
const CARD: &str = "4111111111111111";

/// A server with accounts (ada its admin), its deployments and secrets on disk: the accounts, ada's
/// token, and the folder.
fn server() -> (AppState, Arc<Accounts>, String, String) {
    let mut state = test_state();
    let dir = unique_temp_path("approval-shows");
    std::fs::create_dir_all(&dir).unwrap();
    state.deployments_path = format!("{dir}/deployments.json").into();
    let keys = crate::at_rest::Keyring::open(std::path::Path::new(&format!("{dir}/keys.json")), Some(&format!("file:{dir}/root.key")), &[]).unwrap();
    state.secrets = Arc::new(SecretStore::open(&format!("{dir}/secrets.json"), Arc::new(keys), crate::secret_sources::Sources::none()).unwrap());
    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    state.access = access::Access::new(Arc::clone(&book), vec![], vec![]);
    (state, book, ada, dir)
}

/// `method` `path` with `body` (none when null) as the holder of `token`: its status and its body.
fn send(state: &AppState, token: &str, method: &str, path: &str, body: Value) -> (u16, Value) {
    let (code, said) = ask(state, Some(token), method, path, if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() });
    (code, serde_json::from_str(&said).unwrap_or(Value::Null))
}

/// `text` deployed as ada, approving what it asks for: its slug.
fn deploy(state: &AppState, ada: &str, text: &str) -> String {
    let plan = holon_author::agent::plan(text).unwrap_or_else(|e| panic!("{e}"));
    for unit in &plan.units {
        assert_eq!(ask(state, Some(ada), "POST", "/unit", unit.clone()).0, 200);
    }
    let slug = plan.name.clone();
    let (code, preview) = send(state, ada, "POST", "/deploy/preview", json!({ "slug": slug, "unit": plan.unit }));
    assert_eq!(code, 200, "{preview}");
    let (code, said) = send(state, ada, "POST", "/deploy", json!({ "slug": slug, "unit": plan.unit, "text": text, "acknowledge": preview["needs_approval"] }));
    assert_eq!(code, 200, "{said}");
    slug
}

/// A flow whose action `pay` waits at the approval `payout` with `options`, the lines before it
/// first; tainting `card`.
fn pay(slug: &str, imports: &str, before: &str, options: &str) -> String {
    format!(
        r#"import {{ action, approval, flow }} from "holon"
{imports}
flow("{slug}", {{ taint: ["card"] }})

export const pay = action(async (order) => {{
{before}  const decision = await approval("payout", {{ expires: "1h"{options} }})
  return decision.approved
}})
"#
    )
}

/// The runs waiting in `slug`, as `token`'s holder lists them.
fn waiting(state: &AppState, token: &str, slug: &str) -> (u16, Value) {
    send(state, token, "GET", &format!("/deployments/{slug}/runs"), Value::Null)
}

/// What a run's approval shows those who decide it. Each row: a flow (what its approval names to
/// show, what it reads first), what its run is started with, and what the run's one approval
/// lists: `shows` (named) or `input` (else), and its value. A second approval, the run having been
/// given the first, shows what it names as of then.
#[test]
fn an_approval_shows_its_deciders_what_the_run_names_or_its_input_as_the_log_keeps_it() {
    let (state, book, ada, dir) = server();
    book.define_role(RoleText { name: "approver".into(), about: None, allows: vec!["run.decide".into()], kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    let dee = book.accept(&book.invite("dee", "viewer", None, "ada").unwrap(), PW).unwrap().0;
    let secret = "import key from \"secret:api_key\"\n";
    let big = "x".repeat(20_000);
    let cut = format!("{}‹… {} more bytes›", "x".repeat(1024), big.len() - 1024);
    // Past the bound even cut down: 64 texts of 1000 bytes.
    let many = json!({ "blob": vec!["x".repeat(1000); 64] });
    let too_big = format!("‹{} bytes, more than an approval shows (16 KiB)›", many.to_string().len());
    type Row<'a> = (&'a str, &'a str, &'a str, &'a str, Value, &'a str, Value);
    let rows: [Row; 8] = [
        ("what its shows names", "", "", ", shows: { amount: order.amount, payee: order.payee }", json!({ "amount": 30, "payee": "acme", "memo": "q3" }), "shows", json!({ "amount": 30, "payee": "acme" })),
        ("the run's input, naming none", "", "", "", json!({ "amount": 30, "payee": "acme" }), "input", json!({ "amount": 30, "payee": "acme" })),
        ("a key the flow taints, marked in its input", "", "", "", json!({ "amount": 30, "card": CARD }), "input", json!({ "amount": 30, "card": "‹taint:card›" })),
        ("and in what it names", "", "", ", shows: { amount: order.amount, card: order.card }", json!({ "amount": 30, "card": CARD }), "shows", json!({ "amount": 30, "card": "‹taint:card›" })),
        ("computed from a secret, withheld whole", secret, "", ", shows: { amount: order.amount, check: key.length }", json!({ "amount": 30 }), "shows", json!(crate::taint::WITHHELD)),
        ("more than it shows, cut down", "", "", "", json!({ "blob": big }), "input", json!({ "blob": cut })),
        ("more than it shows cut down, its size", "", "", "", many, "input", json!(too_big)),
        ("a value the run computed before it", "", "  const fee = order.amount / 10\n", ", shows: { amount: order.amount, fee }", json!({ "amount": 30 }), "shows", json!({ "amount": 30, "fee": 3 })),
    ];
    for (n, (what, imports, before, options, input, field, shown)) in rows.into_iter().enumerate() {
        let slug = deploy(&state, &ada, &pay(&format!("p{n}"), imports, before, options));
        if !imports.is_empty() {
            state.secrets.set(&slug, "api_key", SECRET, Some("ada")).unwrap();
        }
        assert_eq!(send(&state, &ada, "POST", &format!("/trigger/{slug}/pay"), input).0, 202, "{what}: it waits");
        let (code, listed) = waiting(&state, &ada, &slug);
        assert_eq!(code, 200, "{what}: {listed}");
        let point = &listed["runs"][0]["waiting"][0];
        let other = if field == "shows" { "input" } else { "shows" };
        assert_eq!((&point[field], &point[other]), (&shown, &Value::Null), "{what}: {point}");
        assert!(!point.to_string().contains(SECRET) && !point.to_string().contains(CARD), "{what}: {point}");
    }

    // A run waiting at two approvals in turn: the second, once the first is given to it, shows
    // what it names then.
    let two = deploy(
        &state,
        &ada,
        r#"import { action, approval, flow } from "holon"

flow("two")

export const pay = action(async (order) => {
  const first = await approval("check", { expires: "1h", shows: { amount: order.amount } })
  const second = await approval("payout", { expires: "1h", shows: { amount: order.amount, checked: first.approved } })
  return second.approved
})
"#,
    );
    book.bind(Subject::Principal("dee".into()), "approver", Scope::Flows(Selector::Slug(two.clone())), None, "ada", None).unwrap();
    assert_eq!(send(&state, &ada, "POST", "/trigger/two/pay", json!({ "amount": 30 })).0, 202);
    let listed = waiting(&state, &ada, "two").1;
    let run = listed["runs"][0]["run_id"].as_str().unwrap().to_string();
    assert_eq!(listed["runs"][0]["waiting"][0]["shows"], json!({ "amount": 30 }), "{listed}");
    let (code, said) = send(&state, &dee, "POST", &format!("/deployments/two/runs/{run}/decide"), json!({ "name": "check", "approve": true }));
    assert_eq!((code, &said["outcome"]), (200, &json!("approved")), "{said}");
    let listed = waiting(&state, &ada, "two").1;
    assert_eq!((&listed["runs"][0]["waiting"][0]["name"], &listed["runs"][0]["waiting"][0]["shows"]), (&json!("payout"), &json!({ "amount": 30, "checked": true })), "{listed}");

    // Sealed on disk with the run, and read back after a restart.
    // Looked for outside the sealed fields: in their ciphertext a short word turns up by chance
    // (`outside_sealed`, R135).
    let on_disk = crate::at_rest::opened(format!("{dir}/deployments.json"), crate::at_rest::role::DEPLOYMENTS);
    // Each version's text, its code (flow two's names `checked`), is kept in the store by hash,
    // not in the deployments file: the word is in it only where a run shows it.
    let clear = outside_sealed(&on_disk);
    assert!(!clear.contains("acme") && !clear.contains("checked"), "the deployments file holds what a run shows: {on_disk}");
    assert!(on_disk.contains("\"sealed\""), "the runs are sealed: {on_disk}");
    let mut reloaded = load_deployments(&state.deployments_path, &state.store, &state.policy()).unwrap();
    for (slug, dep) in reloaded.iter_mut() {
        dep.secrets = state.secrets.vault(slug);
        let keys = dep.secrets.lock().unwrap().keys.clone();
        for run in dep.parked.values_mut() {
            run.open(slug, keys.as_deref()).unwrap();
        }
    }
    *state.deployments.lock().unwrap() = reloaded;
    assert_eq!(waiting(&state, &ada, "p0").1["runs"][0]["waiting"][0]["shows"], json!({ "amount": 30, "payee": "acme" }), "after a restart");
    assert_eq!(waiting(&state, &ada, "two").1["runs"][0]["waiting"][0]["shows"], json!({ "amount": 30, "checked": true }), "after a restart");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The class (R42): what a run's starter sends can't hide the rest of what its approval shows.
/// Past the bound, a field padded shows its head and how many more bytes, the field beside it
/// whole; a record of so many fields its first 64 and how many more; and a secret a cut would run
/// through is redacted first, none of it left in clear.
#[test]
fn padding_hides_nothing_beside_it_of_what_an_approval_shows() {
    let (state, _, ada, dir) = server();
    let padded = format!("acme{}x", " ".repeat(20_000));
    let fields: serde_json::Map<String, Value> = (0..200).map(|i| (format!("f{i:03}"), json!("y".repeat(100)))).collect();
    let across = format!("{}{SECRET}{}", "x".repeat(1010), "y".repeat(20_000));
    let cases = [("padded", json!({ "payee": padded, "amount": 3000 })), ("fields", Value::Object(fields)), ("across", json!({ "note": across, "amount": 3000 }))];
    let mut shown = BTreeMap::new();
    for (name, input) in cases {
        let slug = deploy(&state, &ada, &pay(&format!("shown-{name}"), "import key from \"secret:api_key\"\n", "", ""));
        state.secrets.set(&slug, "api_key", SECRET, Some("ada")).unwrap();
        assert_eq!(send(&state, &ada, "POST", &format!("/trigger/{slug}/pay"), input).0, 202, "{name}: it waits");
        let (code, listed) = waiting(&state, &ada, &slug);
        assert_eq!(code, 200, "{name}: {listed}");
        shown.insert(name, listed["runs"][0]["waiting"][0]["input"].clone());
    }
    let payee = shown["padded"]["payee"].as_str().unwrap();
    assert!(payee.starts_with("acme   ") && payee.ends_with(&format!("‹… {} more bytes›", padded.len() - 1024)), "{payee}");
    assert_eq!(shown["padded"]["amount"], 3000, "the field beside it, whole: {}", shown["padded"]);
    let kept = shown["fields"].as_object().unwrap();
    assert_eq!((kept.len(), &kept["‹more›"], kept.contains_key("f063"), kept.contains_key("f064")), (65, &json!("‹136 more fields›"), true, false), "{}", shown["fields"]);
    let note = shown["across"]["note"].as_str().unwrap();
    assert!(note.starts_with(&"x".repeat(1010)) && !note.contains("sk_live"), "a secret across the cut, redacted before it: {note}");
    assert_eq!(shown["across"]["amount"], 3000);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Who sees a flow's waiting runs, and what of them. Whoever views its runs sees each, every point
/// it waits at with what it shows; whoever only decides one of its approvals (a role holding
/// run.decide alone) sees the runs waiting at an approval they may decide, those points alone, and
/// what they show, and not what was delivered to its events; whoever does neither is refused.
/// Each row: who, the flow, and the status with each run's points as they list them (name, what
/// it shows, the steps theirs) and what it lists as delivered.
#[test]
fn whoever_decides_an_approval_sees_what_it_shows_and_nothing_else_of_the_flows_runs() {
    let (state, book, ada, dir) = server();
    for (name, allows) in [("approver", vec!["run.decide"]), ("reader", vec!["flow.view"])] {
        book.define_role(RoleText { name: name.into(), about: None, allows: allows.into_iter().map(String::from).collect(), kinds: vec![crate::accounts::Kind::Person] }, "ada", None).unwrap();
    }
    let person = |name: &str, role: &str| book.accept(&book.invite(name, role, None, "ada").unwrap(), PW).unwrap().0;
    let (cy, dee, rex) = (person("cy", "viewer"), person("dee", "reader"), person("rex", "reader"));
    // pay's runs wait at its approval, or at an event and an approval at once, the event
    // delivered to one of them; tab's only at an event.
    let pay = deploy(
        &state,
        &ada,
        r#"import { action, approval, event, flow, route } from "holon"

flow("pay", { port: 18093 })

export const pay = action(async (order) => {
  const decision = await approval("payout", { expires: "1h", shows: { amount: order.amount } })
  return decision.approved
})

route.post("/hold", (req) => {
  const started = hold(req.json)
  return started.waiting.paid
})

export const hold = action(async (order) => {
  const paid = await event("paid", { expires: "1h" })
  const decision = await approval("release", { expires: "1h" })
  return [paid, decision.approved]
})

route.get("/paid/:token", () => "ok", { resumes: "paid" })
"#,
    );
    let tab = deploy(
        &state,
        &ada,
        r#"import { action, event, flow } from "holon"

flow("tab")

export const pay = action(async (order) => {
  const paid = await event("paid", { expires: "1h" })
  return paid
})
"#,
    );
    for slug in [&pay, &tab] {
        book.bind(Subject::Principal("dee".into()), "approver", Scope::Flows(Selector::Slug(slug.clone())), None, "ada", None).unwrap();
    }
    assert_eq!(send(&state, &ada, "POST", "/trigger/pay/pay", json!({ "amount": 30 })).0, 202);
    let route = |method: &str, url: &str| {
        let r = crate::listeners::serve_data(&state.store, &state.deployments, &state.deployments_path, "pay", &fake_data(method, url, b"{}".to_vec()));
        assert_eq!(r.status, 200, "{method} {url}");
        String::from_utf8(r.body).unwrap()
    };
    let paid = route("POST", "/hold");
    route("POST", "/hold");
    assert_eq!(route("GET", &format!("/paid/{paid}")), "ok");
    assert_eq!(send(&state, &ada, "POST", "/trigger/tab/pay", json!({})).0, 202);
    let pays = deploy(&state, &ada, "import { action, approval, flow } from \"holon\"\n\nflow(\"other\")\n\nexport const pay = action(async (order) => {\n  const d = await approval(\"payout\", { expires: \"1h\" })\n  return d.approved\n})\n");
    assert_eq!(send(&state, &ada, "POST", &format!("/trigger/{pays}/pay"), json!({ "amount": 5 })).0, 202);

    let token = |who: &str| match who {
        "ada" => ada.clone(),
        "cy" => cy.clone(),
        "dee" => dee.clone(),
        _ => rex.clone(),
    };
    // Each run's points, as `who` lists them (the point, what it shows, the steps theirs), and
    // what it lists as delivered.
    type Run = (Vec<(String, Value, Value)>, Value);
    let points = |listed: &Value| -> Vec<Run> {
        let mut runs: Vec<Run> = listed["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["waiting"].as_array().unwrap().iter().map(|w| (w["name"].as_str().unwrap().to_string(), w["shows"].clone(), w["yours"].clone())).collect(), r["delivered"].clone()))
            .collect();
        runs.sort_by_key(|(points, delivered)| (points.iter().map(|p| p.0.clone()).collect::<Vec<_>>(), delivered.to_string()));
        runs
    };
    let shown = json!({ "amount": 30 });
    let p = |name: &str, shows: &Value, yours: Value| (name.to_string(), shows.clone(), yours);
    let (none, paid) = (json!([]), json!(["paid"]));
    // The runs a viewer lists: the payout (ada started it, so it isn't ada's to decide), and the
    // two held, one waiting at the event and the release, one only at the release, paid.
    let viewed = |yours: Value| -> Vec<Run> {
        vec![
            (vec![p("paid", &Value::Null, Value::Null), p("release", &Value::Null, yours.clone())], none.clone()),
            (vec![p("payout", &shown, json!([]))], none.clone()),
            (vec![p("release", &Value::Null, yours)], paid.clone()),
        ]
    };
    type Row<'a> = (&'a str, &'a str, u16, Vec<Run>);
    let rows: Vec<Row> = vec![
        ("ada", "pay", 200, viewed(json!([0]))),
        ("cy, a viewer", "pay", 200, viewed(json!([]))),
        (
            "dee, who decides payout and release",
            "pay",
            200,
            vec![(vec![p("payout", &shown, json!([0]))], none.clone()), (vec![p("release", &Value::Null, json!([0]))], none.clone()), (vec![p("release", &Value::Null, json!([0]))], none.clone())],
        ),
        ("dee, where nothing waits at an approval", "tab", 200, vec![]),
        ("dee, who decides nothing there", "other", 403, vec![]),
        ("rex, who reads the flow", "pay", 403, vec![]),
    ];
    for (who, slug, code, want) in rows {
        let (got, listed) = waiting(&state, &token(who.split(',').next().unwrap()), slug);
        assert_eq!(got, code, "{who} on {slug}: {listed}");
        if code == 200 {
            assert_eq!(points(&listed), want, "{who} on {slug}: {listed}");
        }
    }
    // Across flows, for each of them: exactly what every flow's runs list them, each run with its
    // flow's slug; nothing of a flow that refuses them.
    let key = |r: &Value| (r["slug"].as_str().unwrap_or_default().to_string(), r["run_id"].as_str().unwrap_or_default().to_string());
    for who in ["ada", "cy", "dee", "rex"] {
        let mut each = Vec::new();
        for slug in [&pay, &tab, &pays] {
            let (code, listed) = waiting(&state, &token(who), slug);
            if code == 200 {
                each.extend(listed["runs"].as_array().unwrap().iter().cloned().map(|mut run| {
                    run["slug"] = json!(slug);
                    run
                }));
            }
        }
        let (code, across) = send(&state, &token(who), "GET", "/runs/waiting", Value::Null);
        assert_eq!(code, 200, "{who}: {across}");
        let mut across = across["runs"].as_array().unwrap().clone();
        across.sort_by_key(key);
        each.sort_by_key(key);
        assert_eq!(across, each, "{who}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
