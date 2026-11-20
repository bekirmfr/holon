//! What a removed slug leaves (its keys, its files) and a deploy of the slug again don't
//! interleave. A forget of its keys asks whether it is deployed and forgets them as one step, and
//! a removal takes it out of the table and deletes its files as one, both under the deployments
//! table's lock, which a deploy of a slug new to the table takes to make its keys and open its
//! files. Each held at its step (`deploy::hold_here`), a deploy of the slug sent then waits for
//! it, and what it then seals and writes all opens, on files of its own.

use super::*;
use std::sync::mpsc;

/// How long a test waits, a step held, to see that a deploy sent then doesn't land.
const WHILE_HELD: Duration = Duration::from_millis(1500);

/// A data folder of its own, `slug`'s writer staged, deployed and run once: the server, the
/// writer's unit, what its deploy approves, and the folder's deployments file.
fn deployed_once(label: &str, slug: &str) -> (AppState, Hash, serde_json::Value, String) {
    let mut state = test_state();
    let path = unique_temp_path(label);
    state.deployments_path = path.clone().into();
    let writer = stage_writer(&state, "k");
    let ack = serde_json::json!({ "acknowledge": ["Action(\"put\")", "StateWrite(\"k\")"] });
    assert_eq!(deploy_json(&state, slug, writer, ack.clone()).status_code, 200);
    assert_eq!(put(&state, slug, "before").status_code, 200);
    (state, writer, ack, path)
}

/// `step` sent with `slug`'s step `at` held, and `deploy` sent once it is there: whether the
/// deploy landed while the step was held, then what each answered.
fn held_meanwhile(at: &'static str, slug: &str, step: impl FnOnce() -> u16 + Send, deploy: impl FnOnce() -> u16 + Send) -> (bool, u16, u16) {
    let (there, arrived) = mpsc::channel();
    let (go, waits) = mpsc::channel();
    crate::deploy::HELD.lock().unwrap().insert((at, slug.to_string()), (there, waits));
    std::thread::scope(|s| {
        let stepping = s.spawn(step);
        let reached = arrived.recv_timeout(EVENTUALLY);
        crate::deploy::HELD.lock().unwrap().remove(&(at, slug.to_string()));
        reached.expect("the step reached its hold");
        let (landed, landing) = mpsc::channel();
        let deploying = s.spawn(move || {
            let code = deploy();
            let _ = landed.send(());
            code
        });
        // not a bound: nothing is to come in it (the deploy waits for the step held)
        let landed_while_held = landing.recv_timeout(WHILE_HELD).is_ok();
        go.send(()).unwrap();
        (landed_while_held, stepping.join().unwrap(), deploying.join().unwrap())
    })
}

/// `slug` stopped and removed: its run records, opened, and checked whole.
fn removed_records(state: &AppState, slug: &str) -> verify::Export {
    assert_eq!(route(state, &req("POST", &format!("/deployments/{slug}/stop"), vec![])).status_code, 200);
    let removed = body_json(route(state, &req("DELETE", &format!("/deployments/{slug}"), vec![])));
    let kept = std::path::PathBuf::from(removed["records"].as_str().unwrap_or_else(|| panic!("where its records are: {removed}")));
    let export = verify::read_export(&kept, || Ok(std::sync::Arc::clone(crate::at_rest::keys()))).unwrap();
    let report = verify::check(&export, Some(&export.head), &BTreeMap::new());
    assert!(report.ok(), "{}", verify::describe(&report, None));
    export
}

/// The folder's files gone: its logs, the records kept beside them, its deployments file.
fn clean(path: &str, slug: &str) {
    let logs = recorder::file_for(&Place::parse(path), slug).unwrap();
    let _ = std::fs::remove_dir_all(logs.parent().unwrap());
    let _ = std::fs::remove_file(path);
}

/// **Guards:** a forget of a removed slug's keys and a deploy of the slug again don't
/// interleave: a deploy sent while the forget is held, between asking whether the slug is
/// deployed and forgetting its keys, waits for it; then everything it seals opens, its runs' lines
/// read from its file and its events.
///
/// **Why:** asked with the table's lock let go between, a deploy landing there went live under
/// the version about to be forgotten, and what it sealed until the forget no longer opened: its
/// first runs' lines, left out of its log's reads without a word.
///
/// **If it fails:** a flow deployed again under a name whose keys an admin is forgetting loses
/// its first runs' records, and nothing says so.
#[test]
fn a_deploy_waits_for_a_forget_of_its_slugs_keys_and_what_it_seals_then_opens() {
    // A slug of its own: its keys are forgotten on the keyring every test shares.
    let slug = crate::at_rest::own_slug("forgets");
    let (state, writer, ack, path) = deployed_once("forget-meanwhile", &slug);
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![])).status_code, 200);
    assert_eq!(route(&state, &req("DELETE", &format!("/deployments/{slug}"), vec![])).status_code, 200);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    let forget = || match crate::keys::forget(&state, crate::at_rest::keys(), &slug, "ada", now) {
        Ok(said) if said["forgotten"] == true => 200,
        Ok(said) => panic!("{said}"),
        Err(_) => 409,
    };
    let (landed, forgot, deployed) = held_meanwhile("forget", &slug, forget, || deploy_json(&state, &slug, writer, ack.clone()).status_code);
    assert!(!landed, "a deploy of the slug landed while its keys were being forgotten");
    assert_eq!((forgot, deployed), (200, 200));
    for body in ["one", "two", "three"] {
        assert_eq!(put(&state, &slug, body).status_code, 200);
    }
    let export = removed_records(&state, &slug);
    assert_eq!(export.lines.len(), 3, "each run's line opens");
    assert!(export.events.iter().any(|e| e.contains("\"deploy")), "its events open: {:?}", export.events);
    clean(&path, &slug);
}

/// **Guards:** a removal and a deploy of the slug again don't interleave: a deploy sent while the
/// removal is held, between taking it out of the table and deleting its files, waits for it; then
/// it starts a chain of its own, on files of its own that stay.
///
/// **Why:** with the table's lock let go between, a deploy landing there opened the old log,
/// still there, went on from its chain, then had its files deleted under it: its first runs'
/// lines, its events, its state's head, gone without a word.
///
/// **If it fails:** a flow deployed again under a name someone is removing loses its first runs'
/// records, or carries the removed one's chain on.
#[test]
fn a_deploy_waits_for_a_removal_of_its_slug_and_starts_on_files_of_its_own() {
    let slug = crate::at_rest::own_slug("removes");
    let (state, writer, ack, path) = deployed_once("remove-meanwhile", &slug);
    assert_eq!(route(&state, &req("POST", &format!("/deployments/{slug}/stop"), vec![])).status_code, 200);
    let remove = || route(&state, &req("DELETE", &format!("/deployments/{slug}"), vec![])).status_code;
    let (landed, removed, deployed) = held_meanwhile("remove", &slug, remove, || deploy_json(&state, &slug, writer, ack.clone()).status_code);
    assert!(!landed, "a deploy of the slug landed while its old files were being deleted");
    assert_eq!((removed, deployed), (200, 200));
    for body in ["one", "two"] {
        assert_eq!(put(&state, &slug, body).status_code, 200);
    }
    let fresh = log_entries(&state, &slug);
    assert_eq!((fresh.len(), fresh[0]["seq"].as_u64(), fresh[0].get("prev")), (2, Some(0), None), "a chain of its own: {fresh:?}");
    let export = removed_records(&state, &slug);
    assert_eq!(export.lines.len(), 2, "each run's line, on a file of its own");
    clean(&path, &slug);
}
