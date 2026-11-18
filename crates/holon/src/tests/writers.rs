//! A writer whose failure the server can't simply go on from reaches an admin and `/health`
//! (failing.rs): audit R39. And whoever asked is told that it failed, never where the file is
//! or the error's own text: audit R71. A run's record names a file only as its flow does: R72; and a database: R82.
//! A file failing is an admin's warning until it goes through, and a listener held on what it
//! has seen says what to do, never where or why: R91.

use super::*;

/// Guards: each writer whose failure loses what the server was asked to keep (the deployments
/// file, the logs' sync to disk), failing for real, is an admin's warning saying why and makes
/// `GET /health` answer 503 "not ok" naming what fails, without a path or an error's text; both
/// at once are both said; and each clears as its next write goes through. A server whose writers
/// all write answers 200 "ok".
/// Why: a deploy answered "deployed" while its table couldn't be written was lost on the next
/// start, and a disk that couldn't take the logs' sync showed as one stderr line, while a load
/// balancer's health check stayed green (audit R39, EN's P5).
/// If it fails: the writer it names fails in silence, or `/health` tells anyone who asks a path of
/// the server's disk, or a writer that recovered goes on being reported.
#[test]
fn a_writer_that_fails_is_a_warning_and_health_says_not_ok_until_it_writes_again() {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("holon-writers-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("deployments.json");
    let state = test_state_at(Place::parse(&file.to_string_lossy()));
    let health = |state: &AppState| {
        let r = route(state, &req("GET", "/health", vec![]));
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
    };
    let warned = |state: &AppState| -> Vec<(String, String)> {
        crate::storage::warnings(state).into_iter().filter(|w| w.key.starts_with("write:")).map(|w| (w.key, w.says)).collect()
    };
    let slug = deploy_plain_text(&state, "import { flow, route } from \"holon\"\n\nflow(\"writers\", { port: 18391 })\n\nroute.get(\"/hi\", () => \"hi\", { log: \"lazy\" })\n");
    assert_eq!(health(&state), (200, "ok".into()), "every writer writes");
    assert_eq!(warned(&state), []);

    // The deployments file: a folder where it goes, so its write fails as a full disk's would.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(crate::persist_deployments(&state.deployments_path, &state.deployments).is_err(), "whoever asked is told too");
    let (status, says) = health(&state);
    assert_eq!((status, says.as_str()), (503, "not ok: the deployments file can't be written"));
    let warnings = warned(&state);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].0, "write:deployments");
    assert!(warnings[0].1.contains(&*file.to_string_lossy()), "the admin is told where: {warnings:?}");

    // The logs' sync, as a disk that can't flush fails it, with a run's line waiting for it.
    assert_eq!(data_call(&state, &slug, "GET", "/hi", "").0, 200);
    state.deployments.lock().unwrap()[&slug].log.with(|l| l.fail_syncs = true);
    assert!(crate::sync_to_disk(&state.deployments, &state.deployments_path).is_err());
    let (status, says) = health(&state);
    assert_eq!((status, says.as_str()), (503, "not ok: the deployments file can't be written; the run logs can't be synced to disk"), "both said");
    assert!(!says.contains(&*dir.to_string_lossy()), "no path to anyone who asks: {says}");
    let keys: Vec<String> = warned(&state).into_iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["write:deployments", "write:logs"]);
    assert!(warned(&state)[1].1.contains("a sync a test failed"), "why, to the admin");
    let other = test_state_at(Place::parse(&dir.join("other.json").to_string_lossy()));
    assert_eq!(health(&other), (200, "ok".into()), "another server in the process isn't told of these");

    // Each clears as its next write goes through.
    std::fs::remove_dir(&file).unwrap();
    assert_eq!(crate::persist_deployments(&state.deployments_path, &state.deployments), Ok(()));
    assert_eq!(health(&state), (503, "not ok: the run logs can't be synced to disk".into()));
    state.deployments.lock().unwrap()[&slug].log.with(|l| l.fail_syncs = false);
    assert_eq!(crate::sync_to_disk(&state.deployments, &state.deployments_path), Ok(()));
    assert_eq!(health(&state), (200, "ok".into()));
    assert_eq!(warned(&state), []);

    let _ = std::fs::remove_dir_all(&dir);
}

/// What an answer to whoever asked may not hold when a read or a write of the server's own files
/// fails (audit R71): any part of the folder they are in, `dir` (its path as written, or escaped
/// as JSON escapes it, its name, the temporary folder it is in), the name of a file under it, and
/// the error's own text: `whys`, and any OS error's "(os error N)". Those are an admin's alone
/// (the warnings, stderr: `failing::Failed`).
pub(super) fn neither_where_nor_why(what: &str, text: &str, dir: &std::path::Path, whys: &[&str]) {
    let found = where_or_why(text, dir, whys);
    assert!(found.is_empty(), "{what}: where on the server's disk or why ({found:?}), to whoever asked: {text}");
}

/// Each part of where on the server's disk `dir` is, or of why a file there failed, that `text`
/// holds ([`neither_where_nor_why`]): none, if it says neither.
pub(super) fn where_or_why(text: &str, dir: &std::path::Path, whys: &[&str]) -> Vec<String> {
    let path = dir.display().to_string();
    let temp = std::env::temp_dir().display().to_string().trim_end_matches(['\\', '/']).to_string();
    let mut parts = vec![path.clone(), path.replace('\\', "\\\\"), dir.file_name().unwrap().to_string_lossy().to_string(), temp.clone(), temp.replace('\\', "\\\\")];
    let mut under = vec![dir.to_path_buf()];
    while let Some(d) = under.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // A flow's own folder is named as the flow, which answers name; a file's name, with
            // its kind after a dot, is the server's.
            if name.contains('.') {
                parts.push(name);
            }
            if entry.path().is_dir() {
                under.push(entry.path());
            }
        }
    }
    let mut found: Vec<String> = parts.into_iter().filter(|p| !p.is_empty() && text.contains(p.as_str())).collect();
    found.extend(whys.iter().copied().chain(["os error"]).filter(|why| text.contains(why)).map(String::from));
    found.dedup();
    found
}

/// An answer read off `port` once it is whole (by its Content-Length), the connection left open
/// as a refused WebSocket's opening request leaves it.
fn answered(port: u16, request: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(EVENTUALLY)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&out).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
            if body.len() >= length {
                return text;
            }
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return text,
            Ok(n) => out.extend_from_slice(&buf[..n]),
        }
    }
}

/// Guards: a read or a write of the server's own files that fails tells whoever asked that it
/// failed, and neither where the file is nor the error's own text (`neither_where_nor_why`): a
/// run's record not synced to disk, to a route's caller, an action's, a WebSocket block's and a
/// flow calling the action; the accounts file not written, to a setup, a sign-in, an invite
/// taken, a password or preferences changed, a sign-out, and a token made or revoked; the policy
/// file not read, or not TOML holon reads, to an editor reloading it (told the line and column,
/// not the line, which can name a secret's source); a slug's keys not made, or its run log or
/// events there and not read, to whoever deploys it first; and a deployment's run records not
/// kept, to whoever removes it. The deployments file's eleven answers are
/// `what_isnt_saved_isnt_answered_as_kept`'s rows (tests.rs), held the same way.
/// Why: each answered the file's whole path on the server's disk and the OS error's text: a
/// flow's outside caller, and someone signing in, among those told. Where the server keeps its
/// data goes in full only to whoever views the host (R51), and why a write failed to the admin
/// (audit R71).
/// If it fails: the row names who was told where, or why: give the error to the admin
/// (`failing::Failed`) and the caller its fixed text.
#[test]
fn a_file_that_fails_tells_whoever_asked_neither_where_nor_why() {
    use crate::accounts::{Accounts, Kind};
    use serde_json::json;
    let dir = std::env::temp_dir().join(format!("holon-r71-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    // A file whose writes fail, as a full disk's would: a folder where it goes.
    let unwritable = |file: &std::path::Path| {
        let _ = std::fs::remove_file(file);
        std::fs::create_dir_all(file).unwrap();
    };

    // A run's record that can't be synced to disk, by every way a run is answered.
    let mut state = test_state_at(Place::parse(&dir.join("deployments.json").to_string_lossy()));
    state.listeners = Some(Mutex::new(BTreeMap::new()));
    let port = free_port();
    let kept = deploy_plain_text(
        &state,
        &format!("import {{ action, flow, route }} from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", {{ port: {port} }})\n\nroute.get(\"/hi\", () => \"hi\")\n\nroute.websocket(\"/w\", {{ open: () => \"hi\" }})\n\nexport const bump = action(() => {{\n  state.n = (state.n ?? 0) + 1\n}})\n"),
    );
    let calling = deploy_plain_text(
        &state,
        &format!("import {{ flow, route }} from \"holon\"\nimport {{ bump }} from \"flow:kept\"\n\nflow(\"calling\", {{ port: {} }})\n\nroute.post(\"/go\", async () => {{\n  const r = await bump({{}})\n  return r\n}})\n", free_port()),
    );
    sync_listeners(&state);
    state.deployments.lock().unwrap()[&kept].log.with(|l| l.fail_syncs = true);
    let opening = format!("GET /w HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n");
    type Answer<'a> = Box<dyn Fn() -> (u16, String) + 'a>;
    let unsynced: [(&str, Answer); 4] = [
        ("a route's caller", Box::new(|| { let (s, b, _) = data_call(&state, &kept, "GET", "/hi", ""); (s, b) })),
        ("an action's caller", Box::new(|| { let r = run_action_named(&state, &kept, "bump", json!({})); (r.status_code, String::from_utf8(body_bytes(r)).unwrap()) })),
        ("a flow calling the action", Box::new(|| { let (s, b, _) = data_call(&state, &calling, "POST", "/go", "{}"); (s, b) })),
        ("a WebSocket's opener", Box::new(|| { let t = answered(port, &opening); (t.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0), t) })),
    ];
    for (who, answer) in unsynced {
        let (code, text) = answer();
        assert!(text.contains(crate::recorder::UNSYNCED), "{who} ({code}): {text}");
        neither_where_nor_why(who, &text, &dir, &["a sync a test failed"]);
    }
    state.deployments.lock().unwrap()[&kept].log.with(|l| l.fail_syncs = false);

    // The accounts file that can't be written, to whoever changes what it keeps.
    let pw = "correct horse battery";
    let fresh_file = dir.join("fresh").join("accounts.json");
    std::fs::create_dir_all(fresh_file.parent().unwrap()).unwrap();
    let fresh = std::sync::Arc::new(Accounts::open(fresh_file.to_str().unwrap()).unwrap());
    let mut fresh_state = test_state();
    fresh_state.access = access::Access::new(std::sync::Arc::clone(&fresh), vec![], vec![]);
    let code = fresh.setup_code().unwrap();
    let file = dir.join("people").join("accounts.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let accounts = std::sync::Arc::new(Accounts::open(file.to_str().unwrap()).unwrap());
    let mut people = test_state();
    people.access = access::Access::new(std::sync::Arc::clone(&accounts), vec![], vec![]);
    accounts.setup(&accounts.setup_code().unwrap(), "ada", pw).unwrap();
    let invite = accounts.invite("bob", "viewer", None, "ada").unwrap();
    accounts.add("scout", Kind::Agent, "agent", "ada", "ada").unwrap();
    let (_, revoked) = accounts.create_token("scout", "live", "ada").unwrap();
    let (ada, _) = accounts.sign_in("ada", pw).unwrap();
    unwritable(&fresh_file);
    unwritable(&file);
    let call = |state: &AppState, method: &str, url: &str, token: Option<&str>, body: serde_json::Value| {
        let mut headers = vec![("Host".to_string(), "localhost:7420".to_string()), ("Content-Type".to_string(), "application/json".to_string())];
        headers.extend(token.map(|t| ("Authorization".to_string(), format!("Bearer {t}"))));
        let r = route(state, &Request::fake_http(method, url, headers, serde_json::to_vec(&body).unwrap()));
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
    };
    let unwritten: [(&str, Answer); 8] = [
        ("someone setting the server up", Box::new(|| call(&fresh_state, "POST", "/auth/setup", None, json!({ "code": &code, "name": "ada", "password": pw })))),
        ("someone signing in", Box::new(|| call(&people, "POST", "/auth/sign-in", None, json!({ "name": "ada", "password": pw })))),
        ("someone taking an invite", Box::new(|| call(&people, "POST", "/auth/invite", None, json!({ "token": &invite, "password": "another horse battery" })))),
        ("a password changed", Box::new(|| call(&people, "POST", "/auth/password", Some(&ada), json!({ "current": pw, "new": "a newer horse battery" })))),
        ("preferences changed", Box::new(|| call(&people, "POST", "/auth/prefs", Some(&ada), json!({ "theme": "dark" })))),
        ("a sign-out", Box::new(|| call(&people, "POST", "/auth/sign-out", Some(&ada), json!({})))),
        ("a token made", Box::new(|| call(&people, "POST", "/people/scout/tokens", Some(&ada), json!({ "label": "ci" })))),
        ("a token revoked", Box::new(|| call(&people, "DELETE", &format!("/people/scout/tokens/{}", revoked.id), Some(&ada), json!({})))),
    ];
    for (who, answer) in unwritten {
        let (code, text) = answer();
        assert!(code >= 400, "{who}: {code} {text}");
        neither_where_nor_why(who, &text, &dir, &[]);
    }
    assert!(call(&people, "POST", "/auth/sign-in", None, json!({ "name": "ada", "password": pw })).1.contains(crate::accounts::UNWRITTEN), "told it isn't kept");

    // The policy file, to an editor reloading it: not read; not TOML (its line names a source);
    // and TOML with a value that isn't what it takes (TOML's message quotes the value).
    let policy = dir.join("holon.toml");
    state.policy_path = policy.display().to_string().into();
    let reload = || {
        let r = route(&state, &req("POST", "/permissions/reload", vec![]));
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
    };
    unwritable(&policy);
    let (code, text) = reload();
    assert!(code == 400 && text.contains(crate::permissions::UNREAD), "{code} {text}");
    neither_where_nor_why("an editor reloading a policy that can't be read", &text, &dir, &[]);
    std::fs::remove_dir(&policy).unwrap();
    for (what, toml, canary) in [
        ("not TOML", "[limits]\nmax_requests = sk_live_canary_one\n", "sk_live_canary_one"),
        ("a value that isn't a number", "[limits]\nmax_requests = \"sk_live_canary_two\"\n", "sk_live_canary_two"),
    ] {
        std::fs::write(&policy, toml).unwrap();
        let (code, text) = reload();
        assert!(code == 400 && text.contains(crate::permissions::UNPARSED) && text.contains("(line 2, column "), "{what}: {code} {text}");
        neither_where_nor_why(what, &text, &dir, &[canary]);
    }
    std::fs::remove_file(&policy).unwrap();

    // A first deploy of a slug: its keys not made (as keys.json not written would), and its run
    // log or its events there and not read (a folder where each goes).
    let writer = stage_writer(&state, "k");
    let ack = json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    let deploy = |slug: &str| {
        let r = deploy_json(&state, slug, writer, ack.clone());
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
    };
    // Its scope made first, as another test sharing the keyring may have made it: the data
    // folder failing fails a first deploy's keys however the keyring stands.
    crate::at_rest::keys().make_scope(&crate::at_rest::deployment("keys-fail")).unwrap();
    crate::faults::fail(crate::at_rest::FAILING, false);
    let (code, text) = deploy("keys-fail");
    crate::faults::heal();
    assert!(code == 503 && text.contains("keys couldn't be made"), "{code} {text}");
    neither_where_nor_why("a deploy whose keys can't be made", &text, &dir, &["made to fail", "keys.json"]);
    assert!(!state.deployments.lock().unwrap().contains_key("keys-fail"), "not deployed");
    // Where the server keeps a deployment's run log and events (recorder.rs `file_for`).
    let logs = std::path::PathBuf::from(state.deployments_path.beside("logs").unwrap());
    for (slug, file, what) in [("logless", "logless.jsonl", "a deploy onto a run log that can't be read"), ("eventless", "eventless.events.jsonl", "a deploy onto events that can't be read")] {
        unwritable(&logs.join(file));
        let (code, text) = deploy(slug);
        assert!(code == 409 && (text.contains("is there and can't be opened") || text.contains("are there and can't be opened")), "{what}: {code} {text}");
        neither_where_nor_why(what, &text, &dir, &["can't be read"]);
        assert!(!state.deployments.lock().unwrap().contains_key(slug), "{what}: not deployed");
    }

    // A removal whose run records can't be kept (a file where their folder goes).
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{calling}/stop"), vec![])).status_code, 200);
    let removed = logs.join("removed");
    let _ = std::fs::remove_dir_all(&removed);
    std::fs::write(&removed, "").unwrap();
    let r = route(&state, &req("DELETE", &format!("/deployments/{calling}"), vec![]));
    let (code, text) = (r.status_code, String::from_utf8(body_bytes(r)).unwrap());
    assert!(code == 500 && text.contains("run records couldn't be kept, so nothing was removed"), "a removal whose run records can't be kept: {code} {text}");
    neither_where_nor_why("a removal whose run records can't be kept", &text, &dir, &[]);
    assert!(state.deployments.lock().unwrap().contains_key(&calling), "nothing removed");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Guards: a run whose read or write of a file fails says so in its record, naming a folder and a
/// file only as the flow names them, by every source of such an error: the state's head that
/// can't be written (its action's run), the server's store of texts that can't keep a big file's
/// text (a route reading it), a bound folder gone (a route listing it), and a capped folder whose
/// size can't be read (`folders::usage`, what a write to it asks first). The whole log entry is
/// held, so its error, decide's `run_error` and an unrecorded run's `last_failure` (the same
/// text) with it. The server's own files are held as R71's are (`neither_where_nor_why`: no OS
/// error's text either); a bound folder's place, wherever it is.
/// Why: each named the server's data folder or a bound folder's whole path, and the run's record
/// is read by whoever may read or decide that flow's runs; where a resource is goes only to
/// whoever may bind it (R51), why the server's own file failed to the admin (audit R72).
/// If it fails: the row says which source put a place in the record: name the folder as the flow
/// does, or give the server's own error to the admin (`failing::Failed`).
#[test]
fn a_run_whose_file_fails_names_it_only_as_the_flow_does() {
    use serde_json::json;
    // Apart: a folder inside the server's data can't be bound.
    let data = std::env::temp_dir().join(format!("holon-r72-{}-{}", std::process::id(), rand::random::<u32>()));
    let bound = data.with_extension("bound");
    for d in [data.clone(), bound.join("inbox"), bound.join("reports")] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut state = test_state_at(Place::parse(&data.join("deployments.json").to_string_lossy()));
    state.folders = std::sync::Arc::new(crate::folders::Folders::memory().with_blobs(crate::blobs::Blobs::at(data.join("blobs"))));
    for name in ["inbox", "reports"] {
        let body = json!({ "name": name, "path": bound.join(name).display().to_string(), "access": "read-write", "keep": "bytes" });
        assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200, "{name}");
    }
    let slug = deploy_plain_text(
        &state,
        "import { action, flow, route } from \"holon\"\nimport state from \"state\"\nimport inbox from \"folder:inbox\"\nimport reports from \"folder:reports\"\n\nflow(\"leaky\", { port: 8372 })\n\nroute.get(\"/read\", () => inbox[\"big.txt\"])\n\nroute.get(\"/list\", () => reports.list())\n\nexport const bump = action(() => {\n  state.n = (state.n ?? 0) + 1\n})\n",
    );
    // Where neither may be: the bound folders' place, as written or as JSON escapes it.
    let nowhere = |what: &str, text: &str| {
        for at in [&bound, &bound.join("inbox"), &bound.join("reports")] {
            let at = at.display().to_string();
            for part in [at.clone(), at.replace('\\', "\\\\")] {
                assert!(!text.contains(&part), "{what}: where a bound folder is ({part:?}), in the run's record: {text}");
            }
        }
    };
    let last = || {
        let entry = log_entries(&state, &slug).last().cloned().unwrap();
        (entry["error"].as_str().unwrap_or("").to_string(), serde_json::to_string(&entry).unwrap())
    };

    // The state's head, a folder in the way of the file.
    let head = crate::persist::state_head_for(&state.deployments_path, &slug).unwrap();
    let _ = std::fs::remove_file(&head);
    std::fs::create_dir_all(&head).unwrap();
    run_action_named(&state, &slug, "bump", json!({}));
    let (error, entry) = last();
    assert!(error.starts_with(crate::folders::WRITE_FAILED) && error.contains("the state it left"), "the head's row ran: {error}");
    neither_where_nor_why("the state's head", &entry, &data, &[]);
    nowhere("the state's head", &entry);

    // The store of texts, a file in the way of its folder: a read too big for its log line.
    let _ = std::fs::remove_dir_all(data.join("blobs"));
    std::fs::write(data.join("blobs"), b"in the way").unwrap();
    std::fs::write(bound.join("inbox").join("big.txt"), "x".repeat(2 * crate::folders::LOG_INLINE_BYTES as usize)).unwrap();
    let (status, _, _) = data_call(&state, &slug, "GET", "/read", "");
    let (error, entry) = last();
    assert!(status == 500 && error.starts_with(crate::folders::READ_FAILED) && error.contains("inbox/big.txt"), "the store's row ran ({status}): {error}");
    neither_where_nor_why("the store of texts", &entry, &data, &[]);
    nowhere("the store of texts", &entry);

    // A bound folder gone, listed.
    std::fs::remove_dir_all(bound.join("reports")).unwrap();
    let (status, _, _) = data_call(&state, &slug, "GET", "/list", "");
    let (error, entry) = last();
    assert!(status == 500 && error.starts_with(crate::folders::READ_FAILED) && error.contains("\"reports\" can't be listed"), "the listing's row ran ({status}): {error}");
    nowhere("a folder listed", &entry);
    assert!(!entry.contains(&data.display().to_string().replace('\\', "\\\\")), "a folder listed: the data folder, in the run's record: {entry}");

    // A capped folder whose size can't be read: what a write to it is told.
    let said = crate::folders::usage("reports", &bound.join("reports")).unwrap_err();
    assert!(said.contains("\"reports\" can't be read"), "the size's row ran: {said}");
    nowhere("a capped folder's size", &said);

    let _ = std::fs::remove_dir_all(&data);
    let _ = std::fs::remove_dir_all(&bound);
}

/// Guards: a run whose database can't be opened or reached says so in its record by the binding's
/// name only, on every engine, each failing for real with no server: SQLite with its file gone;
/// Postgres, MySQL and MongoDB at a closed port on this machine (MongoDB told to give up in
/// 300 ms); and a connection whose source isn't set. First use (the database opened then), by a
/// run's read and its commit; and a database reached before and gone since, by each engine's
/// read and a Postgres and MySQL commit's begin. The whole log entry is held: no address, file,
/// source, password or driver's words.
/// Why: the record is read by whoever may read or decide that flow's runs; where a database is
/// goes only to whoever may bind it (R51), and why it failed to the admin (audit R82).
/// If it fails: the row names the engine and the way that put where, or the driver's words, in
/// the record: give them to the admin (`failing::Failed`), and the run the binding's name.
#[test]
fn a_run_whose_database_fails_names_it_only_as_the_flow_does() {
    use crate::databases::{Bound, Database, Databases, READ_FAILED, UNREACHED, WRITE_FAILED};
    use std::time::Duration;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let dir = std::env::temp_dir().join(format!("holon-r82-{}-{}", std::process::id(), rand::random::<u32>()));
    std::fs::create_dir_all(&dir).unwrap();
    let gone = dir.join("shop.db");
    let pw = "s3cretpw";
    let pg = format!("postgres://holon:{pw}@127.0.0.1:{closed}/shop?sslmode=disable");
    let my = format!("mysql://holon:{pw}@127.0.0.1:{closed}/shop");
    let mongo = format!("mongodb://holon:{pw}@127.0.0.1:{closed}/notes?serverSelectionTimeoutMS=300&connectTimeoutMS=300&directConnection=true");
    let remote = |engine: &str, address: String| Database {
        engine: engine.into(),
        path: None,
        connection: Some("env:HOLON_SECRET_SHOP_URL".into()),
        address: Some(address),
        access: crate::folders::FolderAccess::ReadWrite,
        lock: crate::locks::Setting::Commit,
    };
    // What none may hold: where, the source, the password, and the drivers' words.
    let gone_at = gone.display().to_string();
    let banned: Vec<String> = [
        "127.0.0.1", "localhost", "postgres://", "mysql://", "mongodb://", pw, "HOLON_SECRET_SHOP_URL", "os error",
        "actively refused", "Connection refused", "Server selection", "Topology", "error connecting", "Could not connect", "shop.db",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([gone_at.clone(), gone_at.replace('\\', "\\\\")])
    .collect();
    let held = |what: &str, text: &str| {
        for b in &banned {
            assert!(!text.contains(b.as_str()), "{what}: {b:?} in what a run is told: {text}");
        }
    };

    // First use: each engine, through a run.
    let rows = [
        (
            "SQLite, its file gone",
            Database {
                engine: "sqlite".into(),
                path: Some(crate::paths::HostPath::new(&gone)),
                connection: None,
                address: None,
                access: crate::folders::FolderAccess::ReadWrite,
                lock: crate::locks::Setting::Commit,
            },
            None,
            "shop",
        ),
        ("Postgres, its port closed", remote("postgres", format!("postgres://127.0.0.1:{closed}/shop")), Some(pg.as_str()), "shop"),
        ("MySQL, its port closed", remote("mysql", format!("mysql://127.0.0.1:{closed}/shop")), Some(my.as_str()), "shop"),
        ("MongoDB, its port closed", remote("mongodb", format!("mongodb://127.0.0.1:{closed}/notes")), Some(mongo.as_str()), "notes"),
        ("a source not set", remote("postgres", format!("postgres://127.0.0.1:{closed}/shop")), None, "shop"),
    ];
    for (what, database, url, name) in rows {
        let file = dir.join(format!("databases-{}.json", rand::random::<u32>()));
        let bound = Bound { deployments: Default::default(), server: [(name.to_string(), database)].into() };
        crate::at_rest::write_table(&file, crate::at_rest::role::DATABASES, &serde_json::to_vec(&bound).unwrap()).unwrap();
        let vars: Vec<(&str, &str)> = url.map(|u| ("HOLON_SECRET_SHOP_URL", u)).into_iter().collect();
        let databases = Databases::open(file.to_str().unwrap(), &[]).unwrap().with_sources(crate::secret_sources::Sources::of(&vars));
        let mut state = claims_state("");
        state.folders = std::sync::Arc::new(crate::folders::Folders::memory().with_databases(databases));
        let (text, calls): (&str, &[(&str, &str, &str, &str)]) = match name {
            "notes" => (include_str!("../../../holon-author/samples/atlas-notes.holon.ts"), &[("GET", "/notes/ada", "", READ_FAILED)]),
            _ => (
                include_str!("../../../holon-author/samples/orders-api.holon.ts"),
                &[("GET", "/orders/ada", "", READ_FAILED), ("POST", "/orders", r#"{"customer":"ada","total":"1.00"}"#, WRITE_FAILED)],
            ),
        };
        let slug = deploy_text(&state, text, "");
        for (method, path, body, failed) in calls {
            let (status, _, _) = data_call(&state, &slug, method, path, body);
            let entry = log_entries(&state, &slug).last().cloned().unwrap();
            let error = entry["error"].as_str().unwrap_or("").to_string();
            let told = format!("the database {name:?} couldn't be opened");
            assert!(status >= 500 && error.starts_with(failed) && error.contains(&told), "{what}, {method} {path} ({status}): {error}");
            held(&format!("{what}, {method} {path}"), &serde_json::to_string(&entry).unwrap());
        }
    }

    // Reached before and gone since: what each engine tells a run, its handle open.
    let gather = || crate::databases::Gather::new("select 1", false, 10, 1000);
    for (what, url) in [("Postgres", &pg), ("MySQL", &my)] {
        let remote = crate::engines::Remote::open(zeroize::Zeroizing::new(url.clone())).unwrap();
        let read = remote.read("select 1", &[], &mut gather(), Duration::from_secs(5)).unwrap_err();
        let begin = remote.begin(Duration::from_secs(1), Duration::from_secs(1)).err().map(|f| f.why).unwrap_or_default();
        for (how, said) in [("read", read), ("begin", begin)] {
            assert_eq!(said, UNREACHED, "{what}, gone since, {how}");
        }
    }
    let notes = crate::mongo::Mongo::open(zeroize::Zeroizing::new(mongo.clone())).unwrap();
    let request = holon_core::Value::Record(
        [
            ("database".to_string(), holon_core::Value::Text("notes".into())),
            ("find".to_string(), holon_core::Value::Text("notes".into())),
            ("filter".to_string(), holon_core::Value::Text("{}".into())),
            ("params".to_string(), holon_core::Value::List(vec![])),
        ]
        .into(),
    );
    let found = crate::mongo::found(&request).unwrap();
    let said = notes.read(&found, &mut gather(), Duration::from_secs(5)).unwrap_err();
    assert!(said.ends_with(UNREACHED), "MongoDB, gone since, read: {said}");
    held("MongoDB, gone since, read", &said);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Guards: a file of the server's that fails (failing::note_file) is an admin's warning, saying
/// where and why, until the same told and place goes through; two places under one told are each
/// their own, so the Ok of one leaves the other; a Failed told in other words still clears, by the
/// told it was noted under; another server in the process sees none of them; and `/health` stays
/// "ok" through it.
/// Why: a file a reader couldn't read (a listener's record of what it has seen, a pack, a
/// segment) was said on stderr once, or nowhere, and the admin's page showed nothing (audit R91,
/// R92).
/// If it fails: a file's warning is lost, outlives its fix, or one place's Ok takes another's.
#[test]
fn a_file_that_fails_is_a_warning_until_it_reads_or_writes_again() {
    use crate::failing::{files_now, note_file, Failed};
    let dir = std::path::PathBuf::from(unique_temp_path("files-failing"));
    std::fs::create_dir_all(&dir).unwrap();
    let state = test_state_at(Place::parse(&dir.join("deployments.json").to_string_lossy()));
    let at = &state.deployments_path;
    let told = "the server couldn't read a test's file";
    let (one, two) = (dir.join("one.json").display().to_string(), dir.join("two.json").display().to_string());
    let failed = || Failed::new("a test's file failed", "a read a test failed");
    let warned = |state: &AppState| -> Vec<(String, String)> {
        crate::storage::warnings(state).into_iter().filter(|w| w.key.starts_with("file:")).map(|w| (w.key, w.says)).collect()
    };
    let health = |state: &AppState| {
        let r = route(state, &req("GET", "/health", vec![]));
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
    };

    note_file(at, told, &one, Err(&failed()));
    note_file(at, told, &two, Err(&failed().retold(|t| format!("in other words: {t}"))));
    let warnings = warned(&state);
    assert_eq!(warnings.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(), [format!("file:{told}:{one}"), format!("file:{told}:{two}")]);
    for ((_, says), place) in warnings.iter().zip([&one, &two]) {
        assert!(says.starts_with(told) && says.contains("a read a test failed"), "where and why, to the admin: {says}");
        assert_eq!(says.matches(place.as_str()).count(), 1, "where, once, as the place: {says}");
    }
    assert_eq!(health(&state), (200, "ok".into()), "a file failing is a warning, never /health's 503");
    let other = Place::parse(&dir.join("other.json").to_string_lossy());
    assert_eq!(files_now(&other), [], "another server in the process isn't told of these");

    note_file(at, told, &one, Ok(()));
    assert_eq!(files_now(at).into_iter().map(|(_, place, _)| place).collect::<Vec<_>>(), std::slice::from_ref(&two), "one place's Ok leaves the other");
    note_file(at, told, &two, Ok(()));
    assert_eq!(files_now(at), [], "told in other words, it clears by the told it was noted under");
    assert_eq!(warned(&state), []);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Guards: a listener's record of what it has seen that doesn't open, by each way one doesn't (a
/// key missing, sealed under another deployment, changed, plain where sealed belongs, not a record
/// once opened, the data folder failing), holds the listener in fixed words
/// (at_rest::seen_held): what it holds, and what to do, keys.json restored for a key missing and
/// otherwise the file made readable or removed; neither where nor why. The admin's warnings say
/// where and why (at_rest::note_seen) until it reads. And a folder watch deployed on one that
/// doesn't open is a warning, taken back as its deployment is removed.
/// Why: a queue's, a feed's and a mailbox's events and live status said the record's path on the
/// server's disk and the error's own text, to whoever may view the deployment (audit R91).
/// If it fails: the row names what was said where: give where and why to the warnings and stderr
/// (`note_seen`, `Failed::said`), and the deployment `seen_held`'s words.
#[test]
fn a_listener_held_on_what_it_has_seen_says_what_to_do_and_neither_where_nor_why() {
    use crate::at_rest::{deployment, keys, read_seen, role, seen_held, note_seen, RESTORE_KEYS, SEEN_UNREAD};
    let dir = std::path::PathBuf::from(unique_temp_path("seen-held"));
    std::fs::create_dir_all(dir.join("seen")).unwrap();
    let state = test_state_at(Place::parse(&dir.join("deployments.json").to_string_lossy()));
    let at = &state.deployments_path;
    let scope = deployment("watching");
    let sealed = |scope: &str, plain: &[u8]| keys().seal_file(scope, role::QUEUE_SEEN, plain).unwrap();
    let mut changed = sealed(&scope, b"{}");
    *changed.last_mut().unwrap() ^= 1;
    // Each way, with what it says, which only the admin is told.
    let rows: [(&str, Vec<u8>, bool, &str); 6] = [
        ("a key missing", crate::at_rest::testing::file_of_missing_key(&scope), true, "version 999"),
        ("sealed under another deployment", sealed(&deployment("other"), b"{}"), false, "moved from another"),
        ("changed", changed, false, "doesn't open"),
        ("plain where sealed belongs", b"{}".to_vec(), false, "not sealed"),
        ("not a record once opened", sealed(&scope, b"[1, 2"), false, "EOF"),
        ("the data folder failing", sealed(&scope, b"{}"), false, crate::at_rest::MADE_TO_FAIL),
    ];
    let whys: Vec<&str> = rows.iter().map(|r| r.3).collect();
    for (how, bytes, key_missing, why) in rows {
        let file = dir.join("seen").join(format!("{}.json", how.replace(' ', "-")));
        std::fs::write(&file, bytes).unwrap();
        if how == "the data folder failing" {
            crate::faults::fail(crate::at_rest::FAILING, false);
        }
        let read = read_seen::<serde_json::Value>(&file, &scope, role::QUEUE_SEEN);
        crate::faults::heal();
        let held = seen_held("the queue orders", read.expect_err(how));
        let told = held.to_string();
        assert!(told.starts_with("what it has seen of the queue orders can't be read: it runs nothing and marks nothing seen until it can. "), "{how}: {told}");
        assert_eq!(told.contains(RESTORE_KEYS), key_missing, "{how}: keys.json restored only for a key missing: {told}");
        assert_eq!(told.contains("remove it to start over"), !key_missing, "{how}: {told}");
        assert_eq!(format!("{held:?}"), told, "{how}: its Debug says no more");
        neither_where_nor_why(how, &told, &dir, &whys);

        note_seen(at, Some(file.as_path()), Err(&held));
        let key = format!("file:{SEEN_UNREAD}:{}", file.display());
        let warning = crate::storage::warnings(&state).into_iter().find(|w| w.key == key).unwrap_or_else(|| panic!("{how}: no warning {key}"));
        assert_eq!(warning.says.matches(&*file.display().to_string()).count(), 1, "{how}: where, to the admin, once: {}", warning.says);
        assert!(warning.says.contains(why), "{how}: why, to the admin: {}", warning.says);
        note_seen(at, Some(file.as_path()), Ok(()));
        assert!(!crate::storage::warnings(&state).iter().any(|w| w.key == key), "{how}: read again, the warning goes");
        let _ = std::fs::remove_file(&file);
    }

    // A folder watch deployed on a record that doesn't open: warned of, and the warning taken
    // back as the deployment is removed.
    let inbox = dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    let body = serde_json::json!({ "name": "inbox", "path": inbox.display().to_string(), "access": "read-write" });
    assert_eq!(route(&state, &req("POST", "/admin/folders", serde_json::to_vec(&body).unwrap())).status_code, 200);
    let record = std::path::PathBuf::from(at.beside("arrivals").unwrap()).join("watching.json");
    std::fs::create_dir_all(record.parent().unwrap()).unwrap();
    std::fs::write(&record, crate::at_rest::testing::file_of_missing_key(&scope)).unwrap();
    let slug = deploy_plain_text(&state, "import { flow, on } from \"holon\"\nimport inbox from \"folder:inbox\"\n\nflow(\"watching\")\n\non.file.created(inbox, (f) => {\n  inbox[`done/${f.name}`] = \"seen\"\n})\n");
    assert_eq!(slug, "watching");
    let key = format!("file:{SEEN_UNREAD}:{}", record.display());
    let warnings = crate::storage::warnings(&state);
    let warning = warnings.iter().find(|w| w.key == key).unwrap_or_else(|| panic!("the folder watch held: no warning {key}: {:?}", warnings.iter().map(|w| &w.key).collect::<Vec<_>>()));
    assert!(warning.says.contains("version 999"), "why, to the admin: {}", warning.says);
    assert_eq!(route(&state, &req("POST", "/deployments/watching/stop", vec![])).status_code, 200);
    assert_eq!(route(&state, &req("DELETE", "/deployments/watching", vec![])).status_code, 200);
    assert!(!crate::storage::warnings(&state).iter().any(|w| w.key == key), "removed, its warning goes with it");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Guards: keys.json that can't be written tells whoever rotated a key, or forgot a removed
/// deployment's keys, that the server couldn't write its keys, through their routes, and neither
/// where it is nor why: a keyring over a real keys.json in a folder of its own, the folder made to
/// fail (`Keyring::persist`'s write, which the sweep of every route can't reach: the tests' own
/// keyring is kept in memory, run_steps.rs's REASONS). And the next rotation, the folder well
/// again, goes through.
/// Why: a rotation answered 409 with keys.json's whole path and the OS error's text (audit R87).
/// If it fails: give `Keyring::persist`'s error to the admin (failing::Failed) and whoever asked
/// at_rest::KEYS_UNWRITTEN.
#[test]
fn keys_that_cant_be_written_tell_whoever_rotated_or_forgot_neither_where_nor_why() {
    let dir = std::path::PathBuf::from(unique_temp_path("keys-unwritten"));
    std::fs::create_dir_all(&dir).unwrap();
    // The root outside the folder made to fail: what fails is keys.json alone.
    let root = format!("file:{}", dir.with_extension("root").display());
    let keys = crate::at_rest::Keyring::open(&dir.join("keys.json"), Some(&root), &[]).unwrap();
    // A scope to rotate, and a removed deployment's to forget.
    let scope = crate::at_rest::deployment("gone");
    keys.seal_file(&scope, crate::at_rest::role::LOG, b"gone's").unwrap();
    let state = test_state_at(Place::parse(&dir.join("deployments.json").to_string_lossy()));
    let send = |path: &str, body: serde_json::Value| {
        let r = crate::keys::route_with(&state, &req("POST", path, serde_json::to_vec(&body).unwrap()), "ada", &keys).expect("a keys route");
        (r.status_code, String::from_utf8(body_bytes(r)).unwrap())
    };
    crate::at_rest::fail_folder(&dir);
    let rotated = send("/admin/keys/rotate", serde_json::json!({ "scope": scope }));
    let forgot = send("/admin/keys/forget", serde_json::json!({ "slug": "gone" }));
    crate::at_rest::heal_folder(&dir);
    for (what, (code, said)) in [("a rotation", rotated), ("a forget", forgot)] {
        assert!(said.contains(crate::at_rest::KEYS_UNWRITTEN), "{what} ({code}): {said}");
        neither_where_nor_why(what, &said, &dir, &[crate::at_rest::MADE_TO_FAIL]);
    }
    assert_eq!(send("/admin/keys/rotate", serde_json::json!({ "scope": scope })).0, 200, "written again");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(dir.with_extension("root"));
}
