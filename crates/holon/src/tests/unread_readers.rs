//! What is there and can't be read is an error at every reader of a deployment's runs, never fewer
//! runs or none (release audit R92). One table: the sources (the run log's file, a sealed segment,
//! the file locked as it is read, the segments' folder or a segment as the log opens, the store's
//! packs as the store opens, and a segment let go of, which isn't there and is none) against the
//! readers (the runs page and list reaching back, a cell's history, a restore's preview, an export,
//! a replay, a collection at an old root, a removal; and pages of the newest runs, which reach none
//! of it). Each reader a source reaches answers 500 in fixed words, with no place on the server's
//! disk, and the admin's warnings name the cause the moment it has answered; the others answer as
//! they did. Readable again, every reader answers as it did, and the warning goes.

use super::*;
use crate::recorder::{Log, LogCell, RUNS_UNREAD, RUNS_UNREAD_AT_START};
use std::path::{Path, PathBuf};

const FLOW: &str = r#"import { action, flow } from "holon"
import state from "state"
import items from "collection:items"

flow("kept", { keep: { runs: 72 } })

export const bump = action(() => {
  const n = (state.n ?? 0) + 1
  state.n = n
  items["k" + n] = n
})
"#;

/// A deployment keeping 72 runs of the 100 it ran: the newest 32 in memory, its file holding the
/// newest 34 to 68 as written (a cut-back leaves 34, the next comes past 68), the older sealed, so
/// some it keeps are only in a segment and some only in its file. With `packed`, its values packed
/// by a sweep, then 20 runs more.
struct Kept {
    state: AppState,
    store: PathBuf,
    logs: PathBuf,
    /// Its version's unit, for a replay.
    unit: String,
    /// The oldest run it keeps, in a sealed segment, and the newest.
    oldest: u64,
    newest: u64,
    /// The collection's root before the oldest run it keeps: only a sealed line shows it.
    old_root: String,
    /// The store whole, while one whose packs are cut stands in for it.
    whole: Option<SharedStore>,
}

fn kept(packed: bool) -> Kept {
    let store = PathBuf::from(unique_temp_path("readers-store"));
    let disk = FileStore::new(&store, crate::at_rest::store()).unwrap();
    let values = disk.values_dir();
    let mut state = test_state_with(SharedStore::on_disk(Box::new(disk), values));
    let data = PathBuf::from(unique_temp_path("readers-data"));
    std::fs::create_dir_all(&data).unwrap();
    state.deployments_path = data.join("deployments.json").display().to_string().into();
    let plan = holon_author::agent::plan(FLOW).unwrap_or_else(|e| panic!("{e}"));
    let requirements = plan.requirements.clone().unwrap();
    for u in &plan.units {
        assert_eq!(route(&state, &req("POST", "/unit", u.clone())).status_code, 200);
    }
    let preview = body_json(route(&state, &req("POST", "/deploy/preview", serde_json::to_vec(&serde_json::json!({ "slug": "kept", "unit": plan.unit, "requirements": requirements })).unwrap())));
    let ask = serde_json::json!({ "acknowledge": preview["needs_approval"], "requirements": requirements });
    assert_eq!(deploy_json(&state, "kept", parse_hash(&plan.unit).unwrap(), ask).status_code, 200);
    state.deployments.lock().unwrap()["kept"].log.with(|l| l.set_hot(34));
    let bump = |n: usize| {
        for i in 0..n {
            assert_eq!(route(&state, &req("POST", "/trigger/kept/bump", b"{}".to_vec())).status_code, 200);
            if i % 4 == 0 {
                crate::recorder::sync_logs(&state.deployments).unwrap();
                state.deployments.lock().unwrap()["kept"].log.cut_if_planned();
            }
        }
        crate::recorder::sync_logs(&state.deployments).unwrap();
        state.deployments.lock().unwrap()["kept"].log.cut_if_planned();
    };
    bump(100);
    if packed {
        let later = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 2 * sweep::SPARES_MS;
        let swept = sweep::Scheduled::default().sweep(&state.store, &state.deployments, later).unwrap().unwrap();
        assert!(swept.packed > 0, "{swept:?}");
        bump(20);
    }
    let log = Arc::clone(&state.deployments.lock().unwrap()["kept"].log);
    let (segments, ..) = log.with(|l| l.sealed_holds());
    assert!(segments > 0, "its older runs sealed");
    let runs = log.after(None, usize::MAX).unwrap();
    assert_eq!(runs.len(), 72, "the runs it keeps");
    let old_root = match runs[0].state_before.get("#collection:items") {
        Some(SValue::Text(hex)) => hex.clone(),
        other => panic!("the collection's root before its oldest run: {other:?}"),
    };
    Kept { state, store, logs: data.join("deployments.logs"), unit: plan.unit, oldest: runs[0].seq, newest: runs[71].seq, old_root, whole: None }
}

impl Drop for Kept {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.store);
        let _ = std::fs::remove_dir_all(self.logs.parent().unwrap());
    }
}

/// `path` put aside, and what it isn't put in its place (a folder for a file, a file for a
/// folder): it can't be read, or listed, as what it was. Undone by `heal`.
fn spoil(path: &Path) {
    let aside = path.with_extension("aside");
    std::fs::rename(path, &aside).unwrap();
    if aside.is_dir() {
        std::fs::write(path, b"not a folder").unwrap();
    } else {
        std::fs::create_dir(path).unwrap();
    }
}

fn heal(path: &Path) {
    if path.is_dir() {
        std::fs::remove_dir_all(path).unwrap();
    } else {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::rename(path.with_extension("aside"), path).unwrap();
}

/// The deployment's log opened again from its files, as a server starting opens it.
fn reopen(k: &Kept) {
    let mut log = Log::for_deployment(&k.state.deployments_path, "kept");
    assert!(log.refused().is_none(), "{:?}", log.refused());
    log.retain(72, None);
    k.state.deployments.lock().unwrap().get_mut("kept").unwrap().log = Arc::new(LogCell::new(log));
}

/// Its newest sealed segment: one holding runs it keeps (an older one may hold none any more).
fn newest_segment(k: &Kept) -> PathBuf {
    let mut segments: Vec<PathBuf> = std::fs::read_dir(k.logs.join("kept.sealed")).unwrap().flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".jsonl.gz")).collect();
    segments.sort();
    segments.pop().unwrap()
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        if e.path().is_dir() {
            copy(&e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

fn answer(response: Response) -> (u16, String) {
    let status = response.status_code;
    let (mut reader, _) = response.data.into_reader_and_size();
    let mut body = String::new();
    std::io::Read::read_to_string(&mut reader, &mut body).unwrap();
    (status, body)
}

fn get(k: &Kept, url: &str) -> (u16, String) {
    answer(route(&k.state, &req("GET", url, vec![])))
}

const NEWEST: &str = "the newest runs";
const IN_FILE: &str = "the runs its file holds";
const PAGE: &str = "the runs page, reaching back";
const LIST: &str = "the list of runs, reaching back";
const HISTORY: &str = "a cell's history";
const RESTORE: &str = "a restore's preview";
const EXPORT: &str = "an export";
const REPLAY: &str = "a replay";
const OLD_ROOT: &str = "a collection at an old root";

type Reader = (&'static str, fn(&Kept) -> (u16, String));

const READERS: [Reader; 9] = [
    (NEWEST, |k| get(k, &format!("/deployments/kept/log?after={}&limit=50", k.newest - 5))),
    (IN_FILE, |k| get(k, &format!("/deployments/kept/log?after={}&limit=50", k.newest - 33))),
    (PAGE, |k| get(k, "/deployments/kept/log?limit=200")),
    (LIST, |k| get(k, "/deployments/kept/log/heads?limit=200")),
    (HISTORY, |k| get(k, "/deployments/kept/changes?name=n")),
    (RESTORE, |k| answer(route(&k.state, &req("POST", "/deployments/kept/restore", serde_json::json!({ "run": k.oldest, "state": true, "dry_run": true }).to_string().into_bytes())))),
    (EXPORT, |k| get(k, "/deployments/kept/export")),
    (REPLAY, |k| get(k, &format!("/deployments/kept/replay?unit={}", k.unit))),
    (OLD_ROOT, |k| get(k, &format!("/deployments/kept/collections/items?root={}", k.old_root))),
];

/// Every reader of the run log that reaches back past its newest runs.
const REACHING_BACK: &[&str] = &[PAGE, LIST, HISTORY, RESTORE, EXPORT, REPLAY, OLD_ROOT];
/// And those that read its file too.
const READING_THE_FILE: &[&str] = &[IN_FILE, PAGE, LIST, HISTORY, RESTORE, EXPORT, REPLAY, OLD_ROOT];
/// Every reader of values a pack holds: a run's state, a collection's old items.
const READING_VALUES: &[&str] = &[HISTORY, RESTORE, OLD_ROOT];

/// What the admin's warnings say of it: a run log's failure, one met as the log opened, a pack's.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Warned {
    Now,
    AtStart,
    Pack,
    Nothing,
}

struct Row {
    what: &'static str,
    packed: bool,
    spoil: fn(&mut Kept),
    heal: fn(&mut Kept),
    fails: &'static [&'static str],
    /// The readers that answer 404 once it is so, and after: what isn't there is none.
    none: &'static [&'static str],
    warned: Warned,
    /// Whether a removal keeps nothing back, and so removes nothing.
    removal_fails: bool,
}

/// Guards: the table above. Each source made unreadable, each reader it reaches answers 500 in
/// fixed words naming no place on the disk, and the admin's warnings name it the moment it has
/// answered (the note cleared before each reader); each it doesn't reach answers as it did; a
/// removal that would keep its records short removes nothing. Readable again, every reader
/// answers as before and the warning goes.
/// Why: a page, a history, a restore, an export or a replay taking what it can't read as none
/// answered fewer runs, or "no such root", as if they were all there: an export verified whole
/// with runs missing, a restore's preview left runs out of what it undoes (release audit R92).
/// If it fails: a reader takes an error as none again (`Wanted::read`, `Lines::each`,
/// `FileStore::not_there`), or answers it in words of its own, or fails without a warning.
#[test]
fn what_is_there_and_cant_be_read_is_an_error_at_every_reader() {
    let temp = std::env::temp_dir().display().to_string();
    let temp = temp.trim_end_matches(['/', '\\']).to_string();
    // One more row is added on Windows alone (below): elsewhere nothing changes the list, and the
    // compiler's unused-`mut` warning is an error in the Linux check.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut rows = vec![
        Row { what: "the run log's file", packed: false, spoil: |k| spoil(&k.logs.join("kept.jsonl")), heal: |k| heal(&k.logs.join("kept.jsonl")), fails: READING_THE_FILE, none: &[], warned: Warned::Now, removal_fails: true },
        Row {
            what: "one of its sealed segments",
            packed: false,
            spoil: |k| {
                let segment = newest_segment(k);
                std::fs::write(k.logs.join("spoiled"), segment.display().to_string()).unwrap();
                spoil(&segment);
            },
            heal: |k| heal(Path::new(&std::fs::read_to_string(k.logs.join("spoiled")).unwrap())),
            fails: REACHING_BACK,
            none: &[],
            warned: Warned::Now,
            removal_fails: true,
        },
        Row {
            what: "its segments' folder, as the log opens",
            packed: false,
            spoil: |k| {
                spoil(&k.logs.join("kept.sealed"));
                reopen(k);
            },
            heal: |k| {
                heal(&k.logs.join("kept.sealed"));
                reopen(k);
            },
            fails: REACHING_BACK,
            none: &[],
            warned: Warned::AtStart,
            removal_fails: true,
        },
        Row {
            what: "a segment that doesn't open, as the log opens",
            packed: false,
            spoil: |k| {
                let path = newest_segment(k);
                let bytes = std::fs::read(&path).unwrap();
                std::fs::write(k.logs.join("whole"), &bytes).unwrap();
                std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
                reopen(k);
            },
            heal: |k| {
                std::fs::copy(k.logs.join("whole"), newest_segment(k)).unwrap();
                reopen(k);
            },
            fails: REACHING_BACK,
            none: &[],
            warned: Warned::AtStart,
            removal_fails: true,
        },
        Row {
            what: "every pack of the store's values, as the store opens",
            packed: true,
            spoil: |k| {
                let at = PathBuf::from(unique_temp_path("readers-cut"));
                copy(&k.store, &at);
                for pack in std::fs::read_dir(at.join("values").join("packs")).unwrap().flatten().map(|e| e.path()) {
                    let bytes = std::fs::read(&pack).unwrap();
                    std::fs::write(&pack, &bytes[..bytes.len() - 10]).unwrap();
                }
                let disk = FileStore::new(&at, crate::at_rest::store()).unwrap();
                assert!(!disk.packs().unreadable().is_empty(), "its packs, unreadable");
                let values = disk.values_dir();
                k.whole = Some(std::mem::replace(&mut k.state.store, SharedStore::on_disk(Box::new(disk), values)));
                std::fs::write(k.logs.join("cut"), at.display().to_string()).unwrap();
            },
            heal: |k| {
                k.state.store = k.whole.take().unwrap();
                let _ = std::fs::remove_dir_all(std::fs::read_to_string(k.logs.join("cut")).unwrap());
            },
            fails: READING_VALUES,
            none: &[],
            warned: Warned::Pack,
            removal_fails: false,
        },
        Row {
            what: "a segment let go of (not there: none)",
            packed: false,
            spoil: |k| std::fs::remove_file(newest_segment(k)).unwrap(),
            heal: |_| {},
            fails: &[],
            // The runs it held no longer kept: a restore to one, and the root only one shows.
            none: &[RESTORE, OLD_ROOT],
            warned: Warned::Nothing,
            removal_fails: false,
        },
    ];
    // On Windows, a file another handle has locked opens, and its reads fail: the log's file
    // failing once open (elsewhere the folder in its place, above, fails as it is read).
    #[cfg(windows)]
    {
        static LOCKED: crate::sync::Mutex<Option<std::fs::File>> = crate::sync::Mutex::new(None);
        rows.push(Row {
            what: "the run log's file, locked as it is read",
            packed: false,
            spoil: |k| {
                let file = std::fs::OpenOptions::new().read(true).write(true).open(k.logs.join("kept.jsonl")).unwrap();
                file.lock().unwrap();
                *LOCKED.lock().unwrap() = Some(file);
            },
            heal: |_| drop(LOCKED.lock().unwrap().take()),
            fails: READING_THE_FILE,
            none: &[],
            warned: Warned::Now,
            removal_fails: true,
        });
    }
    // Each row on a deployment of its own, at once.
    std::thread::scope(|s| {
        for row in rows {
            let temp = &temp;
            s.spawn(move || check(row, temp));
        }
    });
}

/// One row of the table: its source made unreadable, every reader asked, then made readable again.
fn check(row: Row, temp: &str) {
    let what = row.what;
    let mut k = kept(row.packed);
    let place = k.logs.join("kept.jsonl").display().to_string();
    let warnings = |k: &Kept| storage::warnings(&k.state).into_iter().map(|w| w.key).collect::<Vec<_>>();
    let warned = |k: &Kept, how: Warned| {
        let keys = warnings(k);
        match how {
            Warned::Now => keys.contains(&format!("file:{RUNS_UNREAD}:{place}")),
            Warned::AtStart => keys.contains(&format!("file:{RUNS_UNREAD_AT_START}:{place}")),
            Warned::Pack => keys.iter().any(|w| w.starts_with("pack:")),
            Warned::Nothing => true,
        }
    };
    let before: Vec<(u16, String)> = READERS.iter().map(|(_, read)| read(&k)).collect();
    for ((name, _), (status, body)) in READERS.iter().zip(&before) {
        assert_eq!(*status, 200, "{what}, before: {name}: {body}");
    }
    assert!(warnings(&k).iter().all(|w| !w.starts_with("file:") && !w.starts_with("pack:")), "{what}, before: {:?}", warnings(&k));

    (row.spoil)(&mut k);
    if row.warned == Warned::AtStart {
        assert!(warned(&k, Warned::AtStart), "{what}: noted as the log opened: {:?}", warnings(&k));
    }
    for ((name, read), (was, _)) in READERS.iter().zip(&before) {
        // The note of a failure now, cleared: the reader's answer notes it again.
        crate::failing::note_file(&k.state.deployments_path, RUNS_UNREAD, &place, Ok(()));
        let (status, body) = read(&k);
        if row.fails.contains(name) {
            assert_eq!(status, 500, "{what}: {name} is an error, never fewer runs or none: {body}");
            assert!(!body.contains(temp), "{what}: {name} names no place on the server's disk: {body}");
            let error = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|b| b["error"].as_str().map(str::to_string)).unwrap_or_default();
            let fixed = error == crate::recorder::runs_unread("kept") || error.contains(crate::failing::STORE_FAILED);
            assert!(fixed, "{what}: {name} answers in fixed words: {body}");
            assert!(warned(&k, row.warned), "{what}: {name}: the admin's warnings say why the moment it has answered: {:?}", warnings(&k));
        } else if row.none.contains(name) {
            assert_eq!(status, 404, "{what}: {name}: not there, none: {body}");
        } else {
            assert_eq!(status, *was, "{what}: {name} reaches none of it, and answers as it did: {body}");
        }
    }
    // A removal whose records would be short removes nothing; one whose records are whole
    // isn't tried: the readers below read the deployment again.
    if row.removal_fails {
        assert_eq!(route(&k.state, &req("POST", "/deployments/kept/stop", vec![])).status_code, 200);
        crate::failing::note_file(&k.state.deployments_path, RUNS_UNREAD, &place, Ok(()));
        let (status, body) = answer(route(&k.state, &req("DELETE", "/deployments/kept", vec![])));
        assert!(status == 500 && body.contains("nothing was removed") && !body.contains(temp), "{what}: a removal: {status} {body}");
        assert!(k.state.deployments.lock().unwrap().contains_key("kept"), "{what}: still deployed");
        assert!(warned(&k, row.warned), "{what}: a removal: the admin's warnings say why: {:?}", warnings(&k));
        assert_eq!(route(&k.state, &req("POST", "/deployments/kept/run", vec![])).status_code, 200);
    }

    (row.heal)(&mut k);
    for ((name, read), (was, _)) in READERS.iter().zip(&before) {
        let (status, body) = read(&k);
        let want = if row.none.contains(name) { 404 } else { *was };
        assert_eq!(status, want, "{what}, readable again: {name}: {body}");
    }
    assert!(warnings(&k).iter().all(|w| !w.starts_with("file:") && !w.starts_with("pack:")), "{what}, readable again: the warning goes: {:?}", warnings(&k));
}
