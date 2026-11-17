//! A writer whose failure the server can't simply go on from reaches an admin and `/health`
//! (failing.rs): audit R39.

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
    crate::persist_deployments(&state.deployments_path, &state.deployments);
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
    crate::persist_deployments(&state.deployments_path, &state.deployments);
    assert_eq!(health(&state), (503, "not ok: the run logs can't be synced to disk".into()));
    state.deployments.lock().unwrap()[&slug].log.with(|l| l.fail_syncs = false);
    assert_eq!(crate::sync_to_disk(&state.deployments, &state.deployments_path), Ok(()));
    assert_eq!(health(&state), (200, "ok".into()));
    assert_eq!(warned(&state), []);

    let _ = std::fs::remove_dir_all(&dir);
}
