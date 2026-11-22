//! A run tried again sends no call twice (R115, the release's R4): a try after the first holds
//! back a call its earlier tries' record doesn't answer, from a node one of them called out from,
//! and the run fails, keeping nothing.

use super::*;

const SCHEMA: &str = "create table prices (id integer primary key, price integer not null); insert into prices values (1, 5); \
     create table items (id integer primary key); insert into items values (1), (2); \
     create table sales (price integer not null)";

/// A flow that reads the database `shop`, calls `/slow` once or for each item, and writes a sale.
fn reader(host: &str, body: &str) -> String {
    format!(
        "import {{ flow, route }} from \"holon\"\nimport shop from \"database:shop\"\nimport host from \"host:{host}\"\n\nflow(\"reader\", {{ port: {} }})\n\nroute.post(\"/sell\", async () => {{\n{body}\n}})\n",
        free_port()
    )
}

/// A flow that changes the database `shop` once, by `exec`.
fn writer(exec: &str) -> String {
    format!("import {{ flow, route }} from \"holon\"\nimport shop from \"database:shop\"\n\nflow(\"writer\", {{ port: {} }})\n\nroute.post(\"/change\", () => {{\n  {exec}\n  return \"set\"\n}})\n", free_port())
}

/// The price read, sent in the call's header.
const PRICE_SENT: &str = "  const row = shop.one(\"select price from prices where id = $1\", 1)\n  const sent = (await host.get(\"/slow\", { headers: { \"x-price\": `${row.price}` } })).body\n  shop.exec(\"insert into sales (price) values ($1)\", row.price)\n  return sent";
/// The price read, the call sent without it.
const PRICE_KEPT: &str = "  const row = shop.one(\"select price from prices where id = $1\", 1)\n  const sent = (await host.get(\"/slow\", { headers: { \"x-price\": \"fixed\" } })).body\n  shop.exec(\"insert into sales (price) values ($1)\", row.price)\n  return sent";
/// A call for each item read, in order.
const EACH_ITEM: &str = "  const items = shop.query(\"select id from items order by id\")\n  const sent = items.map((item) => host.get(`/slow?id=${item.id}`).status)\n  shop.exec(\"insert into sales (price) values ($1)\", sent.length)\n  return sent";

/// **Guards:** a run that runs again because what it read changed while its call was out sends
/// no call twice. Where the new try would call out from a node an earlier try called from, with a
/// request the record doesn't answer (the price it sends changed), the call is held back and the
/// run fails, keeping nothing; where the record answers it (the call carries none of what
/// changed), the run goes on and commits, the far side asked once. By node, item numbers out: a
/// list read that grew at its front or its end between the tries fails the run too, its items'
/// calls each sent once. The error names the step, never what it would have sent: two runs whose
/// new prices differ fail with the same words. `holon verify` runs each again to the same end.
///
/// **Why:** R115: a run tried again on a row it only reads sent its call again with the new price
/// (the far side asked with 5, then with 7): the far side asked twice to act, by one run.
///
/// **If it fails:** the row names the case: a far side asked more than once is the double send
/// back; a run failing where the record answered its calls is the rule held too wide.
#[test]
fn a_run_tried_again_sends_no_call_twice() {
    // (why, the reader's body, the writer's change while the call is out, held back, calls sent, sales)
    type Row<'a> = (&'a str, &'a str, &'a str, bool, usize, Vec<i64>);
    let rows: [Row; 5] = [
        ("R115: the call sends the price that changed", PRICE_SENT, "shop.exec(\"update prices set price = $1 where id = $2\", 7, 1, { expect: 1 })", true, 1, vec![]),
        ("the same, changed to another price", PRICE_SENT, "shop.exec(\"update prices set price = $1 where id = $2\", 8, 1, { expect: 1 })", true, 1, vec![]),
        ("the call sends none of what changed: answered from the record", PRICE_KEPT, "shop.exec(\"update prices set price = $1 where id = $2\", 7, 1, { expect: 1 })", false, 1, vec![7]),
        ("a list read grew at its end: its new item's call is held back", EACH_ITEM, "shop.exec(\"insert into items (id) values ($1)\", 3, { expect: 1 })", true, 2, vec![]),
        ("a list read grew at its front: each item's call moved", EACH_ITEM, "shop.exec(\"insert into items (id) values ($1)\", 0, { expect: 1 })", true, 2, vec![]),
    ];
    let mut errors = Vec::new();
    for (why, body, change, held, calls, sales) in rows {
        let (host, hits, gate) = gated_counting_server();
        let state = claims_state("");
        let dir = std::env::temp_dir().join(format!("holon-tried-again-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shop.db");
        let bound = serde_json::json!({ "name": "shop", "path": file.display().to_string(), "access": "read-write", "schema": SCHEMA });
        assert_eq!(route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&bound).unwrap())).status_code, 200, "{why}");
        let text = reader(&host, body);
        // A request for each item of a list is refused by the text check today (a request in a
        // list's function: the canvas can't write it back), a known refusal to lift when the
        // printer learns the form. What a run of it does still holds: it is deployed by its units,
        // past the text check, as a client sending no text may.
        let reader = match body == EACH_ITEM {
            true => {
                let refused = holon_author::lang::compile(&text).map(|_| ()).unwrap_err().message;
                assert!(refused.starts_with("the function given to map has a write, a request or a start"), "{why}: {refused}");
                deploy_read_alone(&state, &text, &host)
            }
            false => deploy_text(&state, &text, &host),
        };
        let writer = deploy_text(&state, &writer(change), &host);
        let (sold, set) = second_while_first_is_out(&gate, || data_call(&state, &reader, "POST", "/sell", ""), || data_call(&state, &writer, "POST", "/change", ""));
        assert_eq!(set.0, 200, "{why}: the writer: {}", set.1);
        let entries = log_entries(&state, &reader);
        let ran = entries.last().unwrap();
        let error = ran["error"].as_str().unwrap_or("").to_string();
        assert_eq!(ran["retries"].as_u64(), Some(1), "{why}: it ran again: {ran}");
        assert_eq!(hits_of(&hits, "/slow"), calls, "{why}: each call sent once: {ran}");
        let kept: Vec<i64> = rusqlite::Connection::open(&file).unwrap().prepare("select price from sales").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(kept, sales, "{why}: what it kept");
        match held {
            true => {
                assert!(sold.0 >= 500, "{why}: the run failed: {sold:?}");
                assert!(error.starts_with(holon_host::HELD_BACK), "{why}: {error}");
                assert!(!error.contains("slow") && !error.contains("x-price") && !error.contains("id="), "{why}: no part of the call in its words: {error}");
                errors.push(error);
            }
            false => assert_eq!((sold.0, error.as_str()), (200, ""), "{why}: {ran}"),
        }
        let export = body_json(route(&state, &req("GET", &format!("/deployments/{reader}/export"), vec![])));
        let report = verify::check(&serde_json::from_value(export).unwrap(), None, &BTreeMap::new());
        let said = verify::describe(&report, None);
        assert!(!said.contains("DIFFERS") && !said.contains("FAILED"), "{why}: {said}");
        let _ = std::fs::remove_dir_all(&dir);
    }
    // Each row deploys its own reader, on its own port and calling its own far side, both in the
    // flow's nodes: the step's path (where the request's node sorts among them) may differ
    // between rows, though it is one for every run of a deployment. Past it, two new prices give
    // the same words.
    let words = |error: &str| {
        let (head, rest) = error.split_once(" from step [").expect("the error names the step");
        format!("{head} from step [..]{}", &rest[rest.find(']').expect("the step's path ends") + 1..])
    };
    assert_eq!(words(&errors[0]), words(&errors[1]), "two new prices, the same words");
    assert!(errors[2].contains("growing at its end"), "the author is told why a list that grew fails it: {}", errors[2]);
}

/// A worker whose action `touch` marks the row `k` of `notes` once per run it makes, and a starter
/// whose route reads that row and starts `touch` with `input` (a record's fields, `row.v` read).
fn starter_and_worker(input: &str) -> (String, String) {
    let worker = format!(
        "import {{ action, flow }} from \"holon\"\nimport shared from \"database:shared\"\n\nflow(\"worker\", {{ port: {} }})\n\nexport const touch = action((asked) => {{\n  shared.exec(\"update notes set v = v || '+' where k = $1\", asked.k)\n  return \"touched\"\n}})\n",
        free_port()
    );
    let starter = format!(
        "import {{ flow, route }} from \"holon\"\nimport shared from \"database:shared\"\nimport {{ touch }} from \"flow:worker\"\n\nflow(\"starter\", {{ port: {} }})\n\nroute.post(\"/go/:k\", (req) => {{\n  const row = shared.one(\"select v from notes where k = $1\", req.params.k)\n  const started = touch({{ {input} }})\n  return {{ saw: row.v ?? \"none\", started: started.run }}\n}})\n",
        free_port()
    );
    (worker, starter)
}

/// **Guards:** a run that starts another flow's action which changes the row the run read runs
/// again (its read no longer holds at its commit) and starts that action once. Its start's input
/// not read from the row: the second try's start is answered from the record and the run commits.
/// Made from the row: the second try's start is held back and the run fails, keeping nothing; the
/// action ran once.
///
/// **Why:** shown on main 32b7f29c (CR, wait-resume-design 88aaf95b, `twice-*`): with the input
/// made from the row, each try started the action again, the row changed by each start, so every
/// try found it changed: five started runs from one request, the request failing after 110 s.
///
/// **If it fails:** more than one started run in the worker's log (or more than one "+") is the
/// repeated start back. Each started run waits out the starter's hold on the row (the claim wait)
/// before it writes; a write that lost to it leaves "x" and no second try, which "ran again once"
/// says first: that is the hold, not a missing start.
#[test]
fn a_run_that_changes_what_it_read_starts_its_action_once() {
    // (why, the start's input, held back, the starter's status)
    for (why, input, held, status) in [
        ("the start's input not read from the row: answered from the record", "k: req.params.k", false, 200u16),
        ("the start's input made from the row: held back, the run fails", "k: req.params.k, was: row.v", true, 500),
    ] {
        let state = claims_state("");
        let dir = std::env::temp_dir().join(format!("holon-starts-once-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shared.db");
        let schema = "create table notes (k text primary key, v text not null); insert into notes values ('a', 'x')";
        let bound = serde_json::json!({ "name": "shared", "path": file.display().to_string(), "access": "read-write", "schema": schema });
        assert_eq!(route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&bound).unwrap())).status_code, 200, "{why}");
        let (worker, starter) = starter_and_worker(input);
        let worker = deploy_text_to(&state, &worker, &[]);
        let starter = deploy_text_to(&state, &starter, &[]);
        let (code, body, _) = data_call(&state, &starter, "POST", "/go/a", "");
        let entries = log_entries(&state, &starter);
        let ran = entries.last().unwrap();
        let error = ran["error"].as_str().unwrap_or("").to_string();
        assert_eq!(ran["retries"].as_u64(), Some(1), "{why}: it ran again once: {ran}");
        assert_eq!(code >= 500, status >= 500, "{why}: {code} {body}");
        assert_eq!(error.starts_with(holon_host::HELD_BACK), held, "{why}: {error}");
        let v: String = rusqlite::Connection::open(&file).unwrap().query_row("select v from notes where k = 'a'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "x+", "{why}: the action made once");
        assert!(soon(|| log_entries(&state, &worker).len() == 1), "{why}: one started run: {:?}", log_entries(&state, &worker).len());
        let export = body_json(route(&state, &req("GET", &format!("/deployments/{starter}/export"), vec![])));
        let report = verify::check(&serde_json::from_value(export).unwrap(), None, &BTreeMap::new());
        let said = verify::describe(&report, None);
        assert!(!said.contains("DIFFERS") && !said.contains("FAILED"), "{why}: {said}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A flow whose action reads the price, calls `/before` with it, waits for the event "paid", then
/// calls `/slow` with it and writes a sale. `needs_before`: `/slow` also carries `/before`'s status,
/// so a run that asks `/before` again asks `/slow` only in the round after.
fn waiter(host: &str, needs_before: bool) -> String {
    let before = if needs_before { ", \"x-before\": `${before}`" } else { "" };
    format!(
        r#"import {{ action, event, flow, route }} from "holon"
import shop from "database:shop"
import host from "host:{host}"

flow("waiter", {{ port: {} }})

route.post("/sell", () => {{
  const started = sell()
  return {{ token: started.waiting.paid }}
}})

export const sell = action(async () => {{
  const row = shop.one("select price from prices where id = $1", 1)
  const before = (await host.get("/before", {{ headers: {{ "x-price": `${{row.price}}` }} }})).status
  const paid = await event("paid", {{ expires: "1h" }})
  const after = (await host.get("/slow", {{ headers: {{ "x-price": `${{row.price}}`, "x-paid": JSON.stringify(paid.body){before} }} }})).body
  shop.exec("insert into sales (price) values ($1)", row.price)
  return [before, after]
}})

route.get("/paid/:token", () => "ok", {{ resumes: "paid" }})
"#,
        free_port()
    )
}

/// **Guards:** what the databases guide says of a run that waits and runs again. A call it made
/// before its wait, built from a row that changed while it waited, is made again with the new
/// value: the resumed part's earlier part isn't one of its tries (R115's resume half isn't
/// built). A row changed after it went on fails the run, the call made since held back, nothing
/// kept; the call made before the wait goes out again first only where the run asks it in an
/// earlier round than the one held back (on main nothing ties what follows an `await` to it, so
/// both are asked at the resume's first round unless one needs the other's answer).
///
/// **Why:** the guide's "Runs that wait" names each case; each sentence is a claim this test
/// holds. A first draft expected `/before` again in the second row: the run showed one round
/// held back whole, `/before` with it.
///
/// **If it fails:** a `/before` count of 1 in the first or third row means a resumed part holds
/// back its earlier part's calls (the resume half, built: the guide's sentence goes). 2 in the
/// second: either a round sends what it can before one is held back (a bug), or the event's
/// `await` now waits for the awaited call above it, as TypeScript runs two awaits in turn (the
/// wait work's host half, wait-order-lang; R7's tie of what follows a wait doesn't order the
/// wait itself, PR 2026-11-19): then 2 is right, as in the third row. A `/slow` count of 2 is
/// R115's double send back for a resumed part's own tries.
#[test]
fn a_run_that_waits_sends_again_only_what_it_sent_before_its_wait() {
    // (why, the price changed while the run waits, or while its call after the wait is out;
    //  /slow needs /before's answer; /before's calls, held back, sales)
    for (why, while_out, needs_before, before, held, sales) in [
        ("changed while it waits: its call before the wait made again", false, false, 2, false, vec![7i64]),
        ("changed while its call after the wait is out: held back, the round with it", true, false, 1, true, vec![]),
        ("the same, the call after needing the one before: that one goes out again first", true, true, 2, true, vec![]),
    ] {
        let (host, hits, gate) = gated_counting_server();
        let state = claims_state("");
        let dir = std::env::temp_dir().join(format!("holon-waits-again-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shop.db");
        let bound = serde_json::json!({ "name": "shop", "path": file.display().to_string(), "access": "read-write", "schema": SCHEMA });
        assert_eq!(route(&state, &req("POST", "/admin/databases", serde_json::to_vec(&bound).unwrap())).status_code, 200, "{why}");
        let reader = deploy_text(&state, &waiter(&host, needs_before), &host);
        let writer = deploy_text(&state, &writer("shop.exec(\"update prices set price = $1 where id = $2\", 7, 1, { expect: 1 })"), &host);
        let (status, held_body, _) = data_call(&state, &reader, "POST", "/sell", "");
        assert_eq!(status, 200, "{why}: {held_body}");
        let token = serde_json::from_str::<serde_json::Value>(&held_body).unwrap()["token"].as_str().unwrap().to_string();
        assert_eq!((hits_of(&hits, "/before"), hits_of(&hits, "/slow")), (1, 0), "{why}: it called out and waits");
        let paid = || data_call(&state, &reader, "GET", &format!("/paid/{token}"), "");
        let change = || data_call(&state, &writer, "POST", "/change", "");
        match while_out {
            false => {
                assert_eq!(change().0, 200, "{why}");
                assert_eq!(paid().0, 200, "{why}");
            }
            true => {
                let (went_on, set) = second_while_first_is_out(&gate, paid, change);
                assert_eq!((went_on.0, set.0), (200, 200), "{why}: {went_on:?} {set:?}");
            }
        }
        assert!(soon(|| log_entries(&state, &reader).iter().any(|e| e["kind"] == "resume")), "{why}: it went on");
        let entries = log_entries(&state, &reader);
        let resumed = entries.iter().find(|e| e["kind"] == "resume").unwrap();
        let error = resumed["error"].as_str().unwrap_or("").to_string();
        assert_eq!((hits_of(&hits, "/before"), hits_of(&hits, "/slow")), (before, 1), "{why}: {resumed}");
        assert_eq!(error.starts_with(holon_host::HELD_BACK), held, "{why}: {error}");
        assert_eq!(resumed["retries"].as_u64().unwrap_or(0), u64::from(while_out), "{why}: {resumed}");
        let kept: Vec<i64> = rusqlite::Connection::open(&file).unwrap().prepare("select price from sales").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(kept, sales, "{why}: what it kept");
        let export = body_json(route(&state, &req("GET", &format!("/deployments/{reader}/export"), vec![])));
        let report = verify::check(&serde_json::from_value(export).unwrap(), None, &BTreeMap::new());
        let said = verify::describe(&report, None);
        assert!(!said.contains("DIFFERS") && !said.contains("FAILED"), "{why}: {said}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
