//! Where a resource is (a feed's, a queue's or a mail server's address, and the sources its URL,
//! headers and login are read from) goes only to whoever may bind resources (web console L4, Q4,
//! as a folder's path and a database's whereabouts already did), and where the server keeps its
//! data only to whoever views the host: no other caller is answered either, by any route. What a
//! binding does (whom a mail is from and to, a feed's buffer) is shown as a database's access is.
//! A summary and a check of the logs name only the deployments their caller may see (audit R51).
//! The variables an env secret can name are sources too, and the dotenv file's path is where the
//! server keeps its data (audit R94).

use super::access_table::{ask, error_of};
use super::*;
use crate::accounts::Accounts;
use crate::can::{Action, ROUTES};
use serde_json::{json, Value};

const PW: &str = "correct horse battery";

/// In the address of each feed and queue bound.
const WHERE: &str = "canary-r51";
/// In the name of each source a binding's URL is read from.
const SOURCE: &str = "CANARY_R51";
/// In the data folder's path.
const DATA: &str = "datadir-r51";
/// A deployment only some callers see.
const HIDDEN: &str = "hidden-r51";

/// Listens to a feed, a queue and a mailbox; sends to a queue and mails.
const W: &str = r#"import { flow, on } from "holon"
import prices from "feed:prices"
import inq from "queue:inq"
import outq from "queue:outq"
import ops from "mail:ops"

flow("w")

on.feed(prices, (e) => {
  outq.send({ data: e.data })
})

on.queue(inq, (m) => {
  ops.send({ to: m.data.to, subject: "s", text: "t" })
})

on.mail(ops, (m) => {
  outq.send({ data: m.subject })
})
"#;

const OTHER: &str = r#"import { action, flow } from "holon"

flow("hidden-r51")

export const ping = action(async () => 1)
"#;

/// Every key of a binding's listing that says where it is.
const WHEREABOUTS: [&str; 6] = ["url", "address", "headers", "smtp", "imap", "oauth"];

struct Server {
    state: AppState,
    ada: String,
    /// Holds every action on the host but binding, unbinding, viewing and operating the host.
    rv: String,
    /// An editor on `w` only.
    bob: String,
    /// host.check on the host, an editor on `w`.
    cy: String,
    /// The data folder, the .env file in it.
    dir: PathBuf,
    _feed: sse::stand_in::StandIn,
    _sqs: sqs::stand_in::StandIn,
    _sink: smtp::sink::Sink,
    _imap: imap::stand_in::StandIn,
    /// The feed's connection and the queue's listener, which let go when dropped.
    _connections: (subscriptions::Held, dequeue::Held),
}

/// `w` and `hidden-r51` deployed on a server whose data is in a folder named for `DATA`, with a
/// feed, two queues and a mail bound at stand-ins whose addresses carry `WHERE`, each listened
/// to; then accounts.
fn server() -> Server {
    let feed = sse::stand_in::StandIn::start();
    let sqs = sqs::stand_in::StandIn::start();
    sqs.make(&format!("{WHERE}-in"));
    sqs.make(&format!("{WHERE}-out"));
    let sink = smtp::sink::start("PLAIN LOGIN");
    let imap = imap::stand_in::start("ops", "s3cret", true);
    let dir = PathBuf::from(unique_temp_path(DATA));
    std::fs::create_dir_all(&dir).unwrap();
    let env = dir.join(".env");
    let lines = [
        format!("HOLON_SECRET_{SOURCE}_FEED={}", feed.url(&format!("/{WHERE}/events?key=k"))),
        format!("HOLON_SECRET_{SOURCE}_IN={}", sqs.url(&format!("{WHERE}-in"))),
        format!("HOLON_SECRET_{SOURCE}_OUT={}", sqs.url(&format!("{WHERE}-out"))),
        format!("HOLON_SECRET_{SOURCE}_SMTP=smtp://ops:s3cret@127.0.0.1:{}", sink.port),
        format!("HOLON_SECRET_{SOURCE}_IMAP=imap://ops:s3cret@127.0.0.1:{}", imap.port),
    ];
    std::fs::write(&env, lines.join("\n") + "\n").unwrap();
    let sources = || secret_sources::Sources::new(Some(env.clone())).unwrap();
    let mut state = test_state_at(dir.join("deployments.json").to_string_lossy().to_string().into());
    // The secrets read the same .env, as a server's do: what an env secret can name, offered.
    state.secrets = Arc::new(SecretStore::memory().with_sources(sources()));
    let policy = "[grants]\nallow_plain_http = true\nallow_private_addresses = true\n";
    *state.policy.write().unwrap() = Arc::new(Policy::parse(policy).unwrap());
    state.folders = Arc::new(
        folders::Folders::memory()
            .with_feeds(feeds::Feeds::memory().with_sources(sources()))
            .with_queues(queues::Queues::memory().with_sources(sources()))
            .with_mails(mails::Mails::memory().with_sources(sources())),
    );
    state.folders.queues.serve(Arc::clone(&state.deployments));
    state.folders.mails.serve(Arc::clone(&state.deployments));
    let binds = [
        ("/admin/feeds", json!({ "name": "prices", "url": format!("env:HOLON_SECRET_{SOURCE}_FEED") })),
        ("/admin/queues", json!({ "name": "inq", "url": format!("env:HOLON_SECRET_{SOURCE}_IN") })),
        ("/admin/queues", json!({ "name": "outq", "url": format!("env:HOLON_SECRET_{SOURCE}_OUT") })),
        (
            "/admin/mails",
            json!({ "name": "ops", "smtp": format!("env:HOLON_SECRET_{SOURCE}_SMTP"), "from": "Ops <ops@acme.example>", "to": "*@acme.example", "imap": format!("env:HOLON_SECRET_{SOURCE}_IMAP"), "verdict_from": "mx.acme.example" }),
        ),
    ];
    for (path, body) in binds {
        let (code, said) = ask(&state, None, "POST", path, serde_json::to_vec(&body).unwrap());
        assert_eq!(code, 200, "{path}: {said}");
    }
    assert_eq!(deploy_text(&state, W, ""), "w");
    assert_eq!(deploy_text(&state, OTHER, ""), HIDDEN);
    assert!(run_action_named(&state, HIDDEN, "ping", json!({})).is_success());

    // Each listened to, and the deployment's events saying so.
    let mut connections = (subscriptions::Held::default(), dequeue::Held::default());
    subscriptions::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut connections.0);
    dequeue::reconcile(&state.store, &state.deployments, &state.deployments_path, &mut connections.1);
    inbox::run_mail(&state.store, &state.deployments, &state.deployments_path, &mut inbox::Looks::default());
    eventually("w listens to the feed, the queue and the mailbox", || {
        let (_, said) = ask(&state, None, "GET", "/deployments/w/events", vec![]);
        ["listens to the feed", "listens to the queue", "listens to the mailbox"].iter().all(|l| said.contains(l))
    });
    // And runs once on each: an event, a message that mails, and a mail.
    feed.send("1", r#"{"x":1}"#);
    sqs.put(&format!("{WHERE}-in"), r#"{"to":"desk@acme.example"}"#, None);
    let mail = "From: Sam <sam@acme.example>\r\nTo: ops@acme.example\r\nSubject: hello\r\nMessage-ID: <1@acme.example>\r\nDate: Mon, 16 Nov 2026 09:00:00 +0000\r\n\r\nhi\r\n";
    imap.folders.lock().unwrap().put("INBOX", mail.as_bytes());
    let mut looks = inbox::Looks::default();
    inbox::run_mail(&state.store, &state.deployments, &state.deployments_path, &mut looks);
    eventually("w runs on each", || {
        inbox::run_mail(&state.store, &state.deployments, &state.deployments_path, &mut looks);
        log_entries(&state, "w").len() == 3 && sink.got.lock().unwrap().len() == 1
    });

    let book = Arc::new(Accounts::memory());
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    let person = |name: &str| book.accept(&book.invite(name, "viewer", None, "ada").unwrap(), PW).unwrap().0;
    let (rv, bob, cy) = (person("rv"), person("bob"), person("cy"));
    state.access = access::Access::new(book, vec![], vec![]);
    let admin = |method: &str, path: &str, body: Value| {
        let (code, said) = ask(&state, Some(&ada), method, path, serde_json::to_vec(&body).unwrap());
        assert!((200..300).contains(&code), "{method} {path}: {said}");
        said
    };
    let blind = [Action::ResourceBind, Action::ResourceUnbind, Action::HostView, Action::HostOperate];
    let allows: Vec<&str> = Action::ALL.into_iter().filter(|a| !blind.contains(a)).map(Action::name).collect();
    admin("PUT", "/roles/blind", json!({ "allows": allows, "kinds": ["person"] }));
    admin("PATCH", "/people/rv", json!({ "role": "blind" }));
    admin("PUT", "/roles/checker", json!({ "allows": ["host.check"], "kinds": ["person"] }));
    admin("PATCH", "/people/cy", json!({ "role": "checker" }));
    for name in ["bob", "cy"] {
        admin("POST", "/bindings", json!({ "who": { "principal": name }, "role": "editor", "scope": { "flows": { "slug": "w" } } }));
    }
    let bob_on_host = state.access.accounts().unwrap().bindings_of("bob").into_iter().find(|b| b.scope == crate::bindings::Scope::Host).unwrap();
    admin("DELETE", &format!("/bindings/{}", bob_on_host.id), Value::Null);
    Server { state, ada, rv, bob, cy, dir, _feed: feed, _sqs: sqs, _sink: sink, _imap: imap, _connections: connections }
}

/// Every GET route the control plane answers, its path's names filled in: `{slug}` the deployment
/// `w`, any other a name of something bound or none.
fn every_get() -> Vec<String> {
    ROUTES
        .iter()
        .filter(|r| r.method == "GET" && !r.path.contains('*'))
        .map(|r| r.path.replace("{slug}", "w").replace("{name}", "ops").replace("{hash}", &"0".repeat(64)).replace("{id}", "x"))
        .collect()
}

/// Every GET route sent as `token`'s holder, none of whose answers holds any of `canaries`: how
/// many it was answered.
fn swept(state: &AppState, token: &str, who: &str, canaries: &[&str]) -> Vec<String> {
    let mut answered = Vec::new();
    for path in every_get() {
        let (code, said) = ask(state, Some(token), "GET", &path, vec![]);
        for canary in canaries {
            assert!(!said.contains(canary), "{who}: GET {path} ({code}) says {canary}: {said}");
        }
        if code == 200 {
            answered.push(path);
        }
    }
    answered
}

/// No caller who may not bind resources is told where one is, by any GET route, nor one who
/// doesn't view the host where its data is; an editor on one flow isn't told of another by the
/// summary or a check of the logs, nor is a caller who may check them but view no flow; an admin
/// is told each. Nor is any of them told the data folder's path or the .env's, as written or as
/// JSON escapes them, nor the variables an env secret can name. Deleting a deployment says where its records are kept to whoever views the host,
/// and within the data folder to anyone else.
#[test]
fn where_a_resource_or_the_data_is_goes_only_to_whoever_may_bind_or_view_the_host() {
    let s = server();
    let state = &s.state;
    // The run logs' witness, set for this thread's summaries: where it is goes to whoever views
    // the host.
    let witness = format!("https://witness-{WHERE}.example:8443");
    crate::heads::in_test::set(Some(crate::heads::Witness { address: witness.clone(), taken_ms: Some(1_763_283_600_000), waiting: 2 }));
    let get = |token: &str, path: &str| {
        let (code, said) = ask(state, Some(token), "GET", path, vec![]);
        assert_eq!(code, 200, "{path}: {said}");
        serde_json::from_str::<Value>(&said).unwrap()
    };

    // The admin is told where each is, so a sweep that finds none finds them left out.
    for (path, canary) in [("/admin/feeds", WHERE), ("/admin/queues", WHERE), ("/admin/mails", SOURCE), ("/deployments/w/feeds", WHERE), ("/deployments/w/queues", WHERE)] {
        assert!(get(&s.ada, path).to_string().contains(canary), "the admin's {path}");
    }
    let (code, said) = ask(state, Some(&s.ada), "POST", "/summary/check", vec![]);
    assert!(code == 200 && said.contains(HIDDEN), "{said}");
    assert_eq!(get(&s.ada, "/summary")["chain"]["witness"]["address"], json!(witness));
    let env = s.dir.join(".env").display().to_string();
    let offered = get(&s.ada, "/admin/secrets/env");
    assert_eq!(offered["dotenv"], json!(env), "the admin is told where the .env is: {offered}");
    assert!(offered["names"].to_string().contains(SOURCE), "and what an env secret can name: {offered}");

    // A connection's error names where it is (sse.rs's "<address> answered 404", a refused
    // connection its host and port): w's timeline noted with each place the admin is shown, whole
    // and as its host and port, as such an error is, says each binding instead; and so does the
    // error a failing feed's and queue's listing holds, as their connections set it.
    let mut places: Vec<String> = Vec::new();
    for path in ["/admin/feeds", "/admin/queues", "/admin/mails"] {
        for e in get(&s.ada, path).as_array().unwrap() {
            places.extend([&e["address"], &e["mailbox"]["address"]].into_iter().filter_map(Value::as_str).map(String::from));
        }
    }
    assert_eq!(places.len(), 5, "the feed, the two queues, the mail server and its mailbox: {places:?}");
    let authorities: Vec<String> = places.iter().map(|p| p.split_once("://").map_or(p.as_str(), |(_, rest)| rest).split('/').next().unwrap().to_string()).collect();
    for (place, authority) in places.iter().zip(&authorities) {
        crate::events::note(&state.deployments, "w", "feed", format!("couldn't reach {place}: refused; nor {authority}"), None, Default::default());
    }
    let events = get(&s.ada, "/deployments/w/events").to_string();
    for said in ["couldn't reach the feed prices: refused; nor the feed prices", "couldn't reach the queue inq: refused; nor the queue inq", "couldn't reach the mail ops: refused; nor the mail ops"] {
        assert!(events.contains(said), "{said}: {events}");
    }
    let (feed_at, inq_at) = (get(&s.ada, "/admin/feeds")[0]["address"].as_str().unwrap().to_string(), get(&s.ada, "/admin/queues")[0]["address"].as_str().unwrap().to_string());
    // The queue's listener is stopped first, and known to have stopped, so that the error faked
    // here stays as faked through rv's and bob's sweeps. A running listener receives again as
    // soon as a receive comes back, and each receive that comes back clears the error its status
    // holds (dequeue.rs, `listen`); the stand-in answers a receive within a second (sqs.rs). So a
    // listener closed, with nothing asked of the stand-in for two seconds, has had its last answer
    // and written its last status. Faked while it ran, the error was gone by bob's read whenever
    // a receive came back in between, which a busy machine made likely.
    // The feed needs none of this: its reader writes its error only as it connects or loses its
    // connection (subscriptions.rs), and the stand-in keeps its stream open for the whole test.
    drop(s._connections.1);
    let mut asked = (s._sqs.asked().len(), std::time::Instant::now());
    eventually("the queue's listener stops: the stand-in is asked nothing for two seconds", || {
        let now = s._sqs.asked().len();
        if now != asked.0 {
            asked = (now, std::time::Instant::now());
        }
        asked.1.elapsed() >= std::time::Duration::from_secs(2)
    });
    state.folders.feeds.set_live("w", "prices", |l| l.error = Some(format!("{feed_at} answered 404")));
    state.folders.queues.set_live("w", "inq", |l| l.error = Some(format!("{inq_at} wasn't reached")));
    assert_eq!(get(&s.ada, "/deployments/w/feeds")["feeds"][0]["error"], json!(format!("{feed_at} answered 404")), "the admin is told it as it is");
    let data = s.dir.display().to_string();
    let paths = [data.clone(), data.replace('\\', "\\\\"), env.clone(), env.replace('\\', "\\\\")];
    let canaries: Vec<&str> = [WHERE, SOURCE, DATA].into_iter().chain(places.iter().chain(&authorities).chain(&paths).map(String::as_str)).collect();

    // rv views every flow and every resource, and binds none.
    let answered = swept(state, &s.rv, "rv", &canaries);
    for path in ["/admin/feeds", "/admin/queues", "/admin/mails", "/admin/secrets/env", "/deployments/w/feeds", "/deployments/w/queues", "/deployments/w/events", "/summary"] {
        assert!(answered.iter().any(|p| p == path), "rv is answered {path}: {answered:?}");
    }
    for path in ["/admin/feeds", "/admin/queues", "/admin/mails"] {
        for entry in get(&s.rv, path).as_array().unwrap() {
            let mailbox = entry["mailbox"].as_object().into_iter().flat_map(|m| m.keys());
            let shown: Vec<&String> = entry.as_object().unwrap().keys().chain(mailbox).filter(|k| WHEREABOUTS.contains(&k.as_str())).collect();
            assert!(shown.is_empty(), "rv's {path} shows {shown:?}: {entry}");
        }
    }
    assert_eq!(get(&s.rv, "/summary")["chain"]["witness"], json!({ "taken_ms": 1_763_283_600_000u64, "waiting": 2 }), "that one is set, and when it took the heads");
    let mails = get(&s.rv, "/admin/mails");
    assert_eq!((mails[0]["to"].clone(), mails[0]["mailbox"]["folder"].clone()), (json!(["*@acme.example"]), json!("INBOX")), "what it does is shown: {mails}");

    // bob edits w alone: no resource, no data folder, and no other flow.
    let answered = swept(state, &s.bob, "bob", &[canaries.as_slice(), &[HIDDEN]].concat());
    for path in ["/deployments/w/feeds", "/deployments/w/queues", "/deployments/w/events", "/summary"] {
        assert!(answered.iter().any(|p| p == path), "bob is answered {path}: {answered:?}");
    }
    assert_eq!(get(&s.bob, "/deployments/w/feeds")["feeds"][0]["error"], json!("the feed prices answered 404"));
    assert_eq!(get(&s.bob, "/deployments/w/queues")["listening"][0]["error"], json!("the queue inq wasn't reached"));
    let checked = get(&s.bob, "/summary")["chain"]["checked"].clone();
    let heads: Vec<&str> = checked["heads"].as_array().unwrap().iter().map(|h| h[0].as_str().unwrap()).collect();
    assert_eq!(heads, ["w"], "{checked}");

    // cy may check the logs and views only w: her check names w alone, and counts its three runs
    // alone (the hidden flow ran once).
    let (code, said) = ask(state, Some(&s.cy), "POST", "/summary/check", vec![]);
    assert_eq!(code, 200, "{said}");
    let check: Value = serde_json::from_str(&said).unwrap();
    let heads: Vec<&str> = check["heads"].as_array().unwrap().iter().map(|h| h[0].as_str().unwrap()).collect();
    assert_eq!((heads, check["runs"].clone()), (vec!["w"], json!(3)), "{check}");
    assert!(!said.contains(HIDDEN), "{said}");

    // Deleting: bob is told where in the data folder its records are; the admin, where that is.
    for (token, slug) in [(&s.bob, "w"), (&s.ada, HIDDEN)] {
        let (code, said) = ask(state, Some(token), "POST", &format!("/deployments/{slug}/stop"), vec![]);
        assert_eq!(code, 200, "{said}");
    }
    let (code, said) = ask(state, Some(&s.bob), "DELETE", "/deployments/w", vec![]);
    assert_eq!(code, 200, "{said}");
    let records = serde_json::from_str::<Value>(&said).unwrap()["records"].as_str().map(String::from).unwrap_or_default();
    assert!(!said.contains(DATA) && records.contains("removed") && records.ends_with(".json.gz"), "{said}");
    let (code, said) = ask(state, Some(&s.ada), "DELETE", &format!("/deployments/{HIDDEN}"), vec![]);
    assert_eq!(code, 200, "{said}");
    let records = serde_json::from_str::<Value>(&said).unwrap()["records"].as_str().map(String::from).unwrap_or_default();
    assert!(records.contains(DATA) && std::path::Path::new(&records).is_file(), "{said}");
    assert_eq!(error_of(&said), "");
}
