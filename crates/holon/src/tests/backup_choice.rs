//! Where the runs a log lets go of go: not chosen, none, or a folder (permissions.rs `Backup`),
//! through every way the policy is written and read (R123).

use super::*;
use crate::permissions::Backup;
use crate::storage::{BACKUP_CHOOSE, BACKUP_NOT_CHOSEN};
use std::path::PathBuf;

/// A server whose policy is the file `policy`, in a data folder of its own.
struct Host {
    state: AppState,
    policy: PathBuf,
    data: PathBuf,
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

/// One with the file as `text` says (none: no file), read as a starting server reads it.
fn host(text: Option<&str>) -> Host {
    let data = PathBuf::from(unique_temp_path("backup-choice"));
    std::fs::create_dir_all(&data).unwrap();
    let policy = data.join("holon.toml");
    if let Some(text) = text {
        std::fs::write(&policy, text).unwrap();
    }
    let mut state = test_state();
    state.deployments_path = data.join("deployments.json").display().to_string().into();
    state.policy_path = policy.display().to_string().into();
    *state.policy.write().unwrap() = Arc::new(Policy::load(&state.policy_path).unwrap());
    Host { state, policy, data }
}

/// The state, as each way of reading it says it: the policy in force, `GET /permissions`,
/// `GET /admin/host` (and `holon host`'s line), `holon policy`'s line, and whether the host warns.
#[derive(Debug, PartialEq)]
struct Read {
    backup: Backup,
    permissions: serde_json::Value,
    host_chosen: bool,
    host_dir: Option<String>,
    host_line: String,
    policy_line: String,
    warned: bool,
}

fn read(h: &Host) -> Read {
    let permissions = body_json(route(&h.state, &req("GET", "/permissions", vec![])));
    let view = body_json(route(&h.state, &req("GET", "/admin/host", vec![])));
    let warnings = body_json(route(&h.state, &req("GET", "/admin/warnings", vec![])))["warnings"].clone();
    let warned = warnings.as_array().unwrap().iter().any(|w| w["key"] == "backup-choice");
    // Its warning says no place: none is chosen, and the policy's file and the data folder are
    // whereabouts (R114, whereabouts.rs). (A disk past where it warns, on the machine the test
    // runs on, is warned of beside it, by its folder: a host viewer's.)
    let places = [h.data.display().to_string(), h.data.display().to_string().replace('\\', "\\\\")];
    let text = warnings.as_array().unwrap().iter().filter(|w| w["key"] == "backup-choice").map(|w| w.to_string()).collect::<String>();
    assert!(places.iter().all(|p| !text.contains(p.as_str())), "no place in its warning: {text}");
    let line = |lines: Vec<String>, lead: &str| lines.into_iter().find(|l| l.starts_with(lead)).unwrap_or_else(|| panic!("no {lead:?} line"));
    Read {
        backup: h.state.policy().storage.backup.clone(),
        permissions: permissions["storage"]["backup"].clone(),
        host_chosen: view["backup_chosen"] == true,
        host_dir: view["backup"]["dir"].as_str().map(String::from),
        host_line: line(crate::storage::host_lines(&view), "backup "),
        policy_line: line(crate::grantcmd::policy_lines(&permissions), "backup "),
        warned,
    }
}

/// What each state reads as, every way.
fn expect(state: &Backup) -> Read {
    let example = crate::paths::EXAMPLE;
    match state {
        Backup::NotChosen => Read {
            backup: Backup::NotChosen,
            permissions: serde_json::Value::Null,
            host_chosen: false,
            host_dir: None,
            host_line: format!("backup       not chosen: the runs a log lets go of are deleted for good (holon policy --backup {example}, or --backup none)"),
            policy_line: "backup             not chosen: what logs let go of is deleted for good (holon policy --backup DIR, or --backup none)".into(),
            warned: true,
        },
        Backup::Nowhere => Read {
            backup: Backup::Nowhere,
            permissions: serde_json::json!("none"),
            host_chosen: true,
            host_dir: None,
            host_line: "backup       none, chosen: the runs a log lets go of are deleted".into(),
            policy_line: "backup             none, chosen: what logs let go of is deleted".into(),
            warned: false,
        },
        Backup::Folder(dir) => Read {
            backup: state.clone(),
            permissions: serde_json::json!(dir.display().to_string()),
            host_chosen: true,
            host_dir: Some(dir.display().to_string()),
            host_line: format!("backup       {}: 0 exports, 0 bytes", dir.display()),
            policy_line: format!("backup             {}", dir.display()),
            warned: false,
        },
    }
}

fn storage(h: &Host, body: serde_json::Value) -> (u16, String) {
    let r = route(&h.state, &req("POST", "/admin/storage", serde_json::to_vec(&body).unwrap()));
    let code = r.status_code;
    (code, String::from_utf8_lossy(&body_bytes(r)).into_owned())
}

/// Guards: where the runs a log lets go of go is one of three states, each read the same every
/// way (the policy in force, `GET /permissions`, `GET /admin/host`, `holon host`, `holon policy`,
/// the host's warnings) and kept by every write but the one that chooses: the file as found (none,
/// one without the line, `none`, a folder), the file written by hand and reloaded, and
/// `POST /admin/storage` (`holon policy --backup`, the editor's setup and Host page) choosing
/// none, a folder, or another setting. Not chosen, the host warns its admins, in words without a
/// place, and the activity keeps those words alone; a server whose policy is no file isn't
/// warned, since no choice could be written. An empty `backup` is no choice, and refused.
/// Why: an absent `backup` and `backup = "none"` read the same, so a new server deleted the runs
/// its logs let go of for good without asking anyone, and nothing said so (R123). And "not
/// chosen" must survive a write of another setting, or the first unrelated change would turn it
/// into a choice no one made.
/// If it fails: permissions.rs `Backup` or `parse_backup`, storage.rs `set_storage`, `warnings`,
/// `host_view` or `host_lines`, deploy.rs's policy view, or grantcmd.rs `policy_lines` reads or
/// writes a state as another.
#[test]
fn where_runs_go_is_not_chosen_none_or_a_folder_every_way_it_is_read_and_written() {
    let folder = || PathBuf::from(unique_temp_path("backup-folder"));
    let as_folder = |p: &PathBuf| Backup::Folder(crate::paths::HostPath::whole(&p.display().to_string()).unwrap());

    // The file as found.
    let one = folder();
    let found: [(&str, Option<String>, Backup); 5] = [
        ("no file", None, Backup::NotChosen),
        ("a file without the line", Some("[grants]\nallow_plain_http = true\n[storage]\nsweep_every = \"1h\"\n".into()), Backup::NotChosen),
        ("an empty file", Some(String::new()), Backup::NotChosen),
        ("none", Some("[storage]\nbackup = \"none\"\n".into()), Backup::Nowhere),
        ("a folder", Some(format!("[storage]\nbackup = {:?}\n", one.display().to_string())), as_folder(&one)),
    ];
    for (what, text, state) in &found {
        let h = host(text.as_deref());
        assert_eq!(read(&h), expect(state), "{what}, as found");

        // Another setting written: the state kept, its line as it was.
        let before = std::fs::read_to_string(&h.policy).ok();
        assert_eq!(storage(&h, serde_json::json!({ "warn_at": "90%" })).0, 200, "{what}");
        assert_eq!(read(&h), expect(state), "{what}, after another setting is written");
        let after = std::fs::read_to_string(&h.policy).unwrap();
        assert_eq!(after.contains("backup"), before.as_deref().is_some_and(|b| b.contains("backup")), "{what}: no backup line written or taken: {after}");
        // And read again from the file, as a restart reads it.
        assert_eq!(Policy::load(&h.state.policy_path).unwrap().storage.backup, *state, "{what}, read again");

        // An empty one is no choice: refused, the state kept.
        let (code, said) = storage(&h, serde_json::json!({ "backup": "" }));
        assert!(code == 400 && said.contains("\\\"none\\\" to delete them"), "{what}: an empty backup: {code} {said}");
        assert_eq!(read(&h), expect(state), "{what}, after an empty one is refused");

        // Each choice, from each state: written, in force, and read so every way.
        let two = folder();
        for choice in [Backup::Nowhere, as_folder(&two), Backup::Nowhere] {
            let value = match &choice {
                Backup::Folder(dir) => dir.display().to_string(),
                _ => "none".into(),
            };
            assert_eq!(storage(&h, serde_json::json!({ "backup": value })).0, 200, "{what}: {value}");
            assert_eq!(read(&h), expect(&choice), "{what}, {value} chosen");
            assert_eq!(Policy::load(&h.state.policy_path).unwrap().storage.backup, choice, "{what}, {value}, read again");
        }

        // The line taken out by hand and reloaded: not chosen again, and warned of again.
        let text = std::fs::read_to_string(&h.policy).unwrap();
        let without: String = text.lines().filter(|l| !l.trim_start().starts_with("backup")).map(|l| format!("{l}\n")).collect();
        std::fs::write(&h.policy, without).unwrap();
        assert_eq!(route(&h.state, &req("POST", "/permissions/reload", vec![])).status_code, 200, "{what}");
        assert_eq!(read(&h), expect(&Backup::NotChosen), "{what}, its line taken out and reloaded");
        // Written as `none` by hand and reloaded: chosen.
        let mut text = std::fs::read_to_string(&h.policy).unwrap();
        if !text.contains("[storage]") {
            text.push_str("[storage]\n");
        }
        std::fs::write(&h.policy, text.replace("[storage]\n", "[storage]\nbackup = \"none\"\n")).unwrap();
        assert_eq!(route(&h.state, &req("POST", "/permissions/reload", vec![])).status_code, 200, "{what}");
        assert_eq!(read(&h), expect(&Backup::Nowhere), "{what}, none written by hand and reloaded");
        let _ = std::fs::remove_dir_all(&two);
    }

    // Not chosen: the warning's words, the same to everyone who may view the host and to the
    // activity, which keeps them alone as it starts (R114), and once chosen it ends.
    let h = host(None);
    let warning = crate::storage::warnings(&h.state).into_iter().find(|w| w.key == "backup-choice").expect("warned");
    assert_eq!((warning.told, warning.says.as_str(), warning.todo.as_str()), (BACKUP_NOT_CHOSEN, BACKUP_NOT_CHOSEN, BACKUP_CHOOSE));
    h.state.sweeps.watch(&h.state);
    // (A disk past where it warns, on the machine the test runs on, is warned of beside it.)
    let told: Vec<String> = h.state.activity.newest(50, |e| e.action == crate::activity::WARNED).into_iter().map(|e| e.target).collect();
    assert_eq!(told.iter().filter(|t| *t == BACKUP_NOT_CHOSEN).count(), 1, "the activity keeps its words alone: {told:?}");
    assert_eq!(storage(&h, serde_json::json!({ "backup": "none" })).0, 200);
    assert!(crate::storage::warnings(&h.state).iter().all(|w| w.key != "backup-choice"), "chosen: no longer warned of");
    assert_eq!(h.state.activity.newest(50, |e| e.action.starts_with("set where")).len(), 1, "the choice is in the activity");

    // A server whose policy is no file (`--policy :memory:`): nothing could be written, so no
    // warning no one could end; as not chosen all the same.
    let mut h = host(None);
    h.state.policy_path = Place::Nowhere;
    assert!(crate::storage::warnings(&h.state).iter().all(|w| w.key != "backup-choice"), "a policy that is no file: not warned");
    assert_eq!(h.state.policy().storage.backup, Backup::NotChosen);
    let _ = std::fs::remove_dir_all(&one);
}
