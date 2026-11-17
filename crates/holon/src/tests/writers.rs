//! A writer whose failure the server can't simply go on from reaches an admin and `/health`
//! (failing.rs): audit R39. And whoever asked is told that it failed, never where the file is
//! or the error's own text: audit R71. A run's record names a file only as its flow does: R72.

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
    for part in parts.iter().filter(|p| !p.is_empty()) {
        assert!(!text.contains(part.as_str()), "{what}: where on the server's disk ({part:?}), to whoever asked: {text}");
    }
    for why in whys.iter().copied().chain(["os error"]) {
        assert!(!text.contains(why), "{what}: why ({why:?}), to whoever asked: {text}");
    }
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
/// not the line, which can name a secret's source). The deployments file's eleven answers are
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
