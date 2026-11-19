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
    kept_with(packed, 34)
}

/// [`kept`], its file holding no more than its newest `hot` runs as written: the fewer, the more
/// sealed segments.
fn kept_with(packed: bool, hot: usize) -> Kept {
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
    state.deployments.lock().unwrap()["kept"].log.with(|l| l.set_hot(hot));
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

/// Every string an answer holds, its keys too, read out of its JSON, with `leave` taken off its
/// top first; the text itself if it isn't JSON. What is looked for in them is looked for as it
/// is told: in the text as sent, a Windows path's backslashes are each written twice.
fn strings_of(said: &str, leave: &[&str]) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(items) => items.iter().for_each(|x| walk(x, out)),
            serde_json::Value::Object(fields) => fields.iter().for_each(|(k, x)| {
                out.push(k.clone());
                walk(x, out)
            }),
            _ => {}
        }
    }
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(said) else { return vec![said.to_string()] };
    if let Some(fields) = v.as_object_mut() {
        for key in leave {
            fields.remove(*key);
        }
    }
    let mut out = Vec::new();
    walk(&v, &mut out);
    out
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

/// What of its log can't be read as it opens, in the class test of giving runs up.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Damage {
    /// The oldest sealed segment holding runs it keeps, one between, the newest, or two apart.
    Oldest,
    Middle,
    Newest,
    TwoApart,
    /// Nothing: every run reads.
    Nothing,
    /// Its segments' folder, not listed: what is missing isn't known by run.
    Folder,
}

/// What the admin names to give up, removing it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ask {
    /// Nothing named.
    Plain,
    /// Exactly the runs it can't read, the newest named first.
    Exactly,
    /// One run more than it can't read.
    Wrong,
    /// Exactly what it couldn't read as it opened, one of them read again meanwhile.
    OneHealed,
}

/// The runs a sealed segment's name says it holds.
fn span_of(p: &Path) -> (u64, u64) {
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    let (first, last) = name.trim_end_matches(".jsonl.gz").split_once('-').unwrap();
    (first.parse().unwrap(), last.parse().unwrap())
}

/// Its sealed segments holding runs it keeps, oldest first.
fn segments_kept(k: &Kept) -> Vec<PathBuf> {
    let mut segments: Vec<PathBuf> = std::fs::read_dir(k.logs.join("kept.sealed"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".jsonl.gz") && span_of(p).1 >= k.oldest)
        .collect();
    segments.sort();
    segments
}

/// Guards: a removal gives up runs only on an admin's exact word, and only the runs of sealed
/// segments that couldn't be read as the server started. One table: the segment damaged (the
/// oldest holding runs it keeps, one between, the newest, two apart; none; its folder not listed)
/// against what is named (nothing, exactly, one run more, exactly while one was read again). Named
/// exactly: removed, its answer and its activity line naming the runs given up by number alone,
/// the segments moved beside its records, its log's files gone, and `holon verify` on the records
/// finding nothing wrong, saying those runs given up where they were, and not every run checked
/// (exit 3). Anything else: nothing removed, nothing moved, no activity line, and the answer says
/// the runs it can't read now and how to give them up, with no place and no system's words; all of
/// it read again by then removes it as ever, nothing given up. And the records changed: a plain
/// copy, the list moved one run over, or taken out, each fails `holon verify`.
/// Why: a deployment whose sealed segment was damaged couldn't be removed at all (500, for good),
/// and taken out by hand, its records failed their own check for good, saying runs removed
/// outside holon (release audit R110).
/// If it fails: deploy.rs `handle_delete_deployment` or `not_removed`, recorder.rs
/// `lines_to_head_giving_up`, or verify.rs `given_up` changed what it gives up, or says.
#[test]
fn a_removal_gives_up_only_what_it_cant_read_on_an_admins_exact_word() {
    let rows = [
        (Damage::Oldest, Ask::Exactly, 200),
        (Damage::Middle, Ask::Exactly, 200),
        (Damage::Newest, Ask::Exactly, 200),
        (Damage::TwoApart, Ask::Exactly, 200),
        (Damage::Nothing, Ask::Exactly, 200),
        (Damage::Middle, Ask::Plain, 500),
        (Damage::Middle, Ask::Wrong, 409),
        (Damage::TwoApart, Ask::OneHealed, 409),
        (Damage::Folder, Ask::Exactly, 500),
    ];
    std::thread::scope(|s| {
        for (damage, ask, status) in rows {
            s.spawn(move || give_up_row(damage, ask, status));
        }
    });
}

/// One row: `damage` made as its log opens, then the admin's removal naming what `ask` says.
fn give_up_row(damage: Damage, ask: Ask, status: u16) {
    let row = format!("{damage:?}, {ask:?}");
    let k = kept_with(false, 8);
    assert_eq!(route(&k.state, &req("POST", "/deployments/kept/stop", vec![])).status_code, 200, "{row}");
    let sealed = k.logs.join("kept.sealed");
    let segments = segments_kept(&k);
    let n = segments.len();
    assert!(n >= 3, "{row}: three segments or more holding runs it keeps: {segments:?}");
    let picked: Vec<PathBuf> = match damage {
        Damage::Oldest => vec![segments[0].clone()],
        Damage::Middle => vec![segments[n / 2].clone()],
        Damage::Newest => vec![segments[n - 1].clone()],
        Damage::TwoApart => vec![segments[0].clone(), segments[2].clone()],
        Damage::Nothing | Damage::Folder => vec![],
    };
    // What is named: the runs of those damaged, else the middle one's.
    let named: Vec<(u64, u64)> = if picked.is_empty() { vec![span_of(&segments[n / 2])] } else { picked.iter().map(|p| span_of(p)).collect() };
    let whole: Vec<Vec<u8>> = picked.iter().map(|p| std::fs::read(p).unwrap()).collect();
    for (p, bytes) in picked.iter().zip(&whole) {
        std::fs::write(p, &bytes[..bytes.len() - 10]).unwrap();
    }
    if damage == Damage::Folder {
        spoil(&sealed);
    }
    reopen(&k);
    if damage == Damage::Folder {
        heal(&sealed);
    }
    if ask == Ask::OneHealed {
        std::fs::write(&picked[1], &whole[1]).unwrap();
    }
    let give_up: Vec<(u64, u64)> = match ask {
        Ask::Plain => vec![],
        Ask::Exactly | Ask::OneHealed => named.iter().rev().copied().collect(),
        Ask::Wrong => named.iter().map(|&(a, b)| (a, b + 1)).collect(),
    };
    let url = if give_up.is_empty() { "/deployments/kept".to_string() } else { format!("/deployments/kept?give_up={}", crate::deploy::give_up_arg(&give_up)) };
    let (code, said) = answer(route(&k.state, &req("DELETE", &url, vec![])));
    assert_eq!(code, status, "{row}: {said}");
    // No place and no system's words, in what is told: a refusal whole, and a removal's answer but
    // its `records`, which is the admin's (held below). Read out of the JSON: as sent, a Windows
    // path's backslashes are doubled, and the data folder looked for in the text was never found
    // there, whatever the answer held. Its two forms: as `{}` writes it, and as `{:?}` does, which
    // doubles them in the words themselves (the same on Linux).
    let data = k.logs.parent().unwrap().display().to_string();
    let places = [data.clone(), data.replace('\\', "\\\\")];
    let told = strings_of(&said, if code == 200 { &["records"] } else { &[] });
    assert!(told.iter().any(|s| !s.is_empty()), "{row}: read out of the answer: {said}");
    let a_place = |s: &String| places.iter().any(|p| s.contains(p.as_str()));
    assert!(told.iter().all(|s| !a_place(s) &&!s.contains("os error") && !s.contains(".jsonl")), "{row}: no place and no system's words: {said}");
    let deleted = k.state.activity.newest(50, |e| e.action == "deleted");
    if code != 200 {
        assert!(k.state.deployments.lock().unwrap().contains_key("kept"), "{row}: nothing removed");
        assert!(picked.iter().all(|p| p.is_file()), "{row}: nothing moved");
        assert!(deleted.is_empty(), "{row}: no activity line: {deleted:?}");
        if damage == Damage::Folder {
            assert!(said.contains("isn't known by run"), "{row}: {said}");
        } else {
            let now = if ask == Ask::OneHealed { vec![named[0]] } else { named.clone() };
            assert!(said.contains(&format!("--give-up {}", crate::deploy::give_up_arg(&now))), "{row}: what it can't read now, and how to give it up: {said}");
        }
        return;
    }
    let answer: serde_json::Value = serde_json::from_str(&said).unwrap();
    // Its records, where the admin who removed it finds them: the whole path, in the removed
    // records' folder, by the deployment's name.
    let records = PathBuf::from(answer["records"].as_str().unwrap_or_else(|| panic!("{row}: its records: {said}")));
    let name = records.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    assert!(records.parent() == Some(k.logs.join("removed").as_path()) && name.starts_with("kept-runs-") && name.ends_with(".json.gz"), "{row}: {}", records.display());
    assert!(records.is_file(), "{row}: {}", records.display());
    let given: Vec<(u64, u64)> = serde_json::from_value(answer["given_up"].clone()).unwrap_or_default();
    let expect = if picked.is_empty() { vec![] } else { crate::recorder::joined(named.clone()) };
    assert_eq!(given, expect, "{row}: {said}");
    // Its activity line: deleted, the runs given up by number alone.
    assert_eq!(deleted.len(), 1, "{row}: {deleted:?}");
    let note = (!expect.is_empty()).then(|| format!("gave up {}: they couldn't be read", crate::deploy::runs_said(&expect)));
    assert_eq!(deleted[0].note, note, "{row}");
    // Kept: its records, and beside them the segments given up; its log's files gone.
    let records = PathBuf::from(answer["records"].as_str().unwrap_or_else(|| panic!("{row}: {said}")));
    let beside = crate::deploy::given_up_dir(&records);
    for p in &picked {
        assert!(beside.join(p.file_name().unwrap()).is_file() && !p.exists(), "{row}: {} moved beside the records", p.display());
    }
    assert_eq!(beside.exists(), !picked.is_empty(), "{row}: {}", beside.display());
    assert!(!k.logs.join("kept.jsonl").exists() && !sealed.exists(), "{row}: its log's files gone");
    // holon verify: nothing found wrong, the runs given up said where they were, not every run checked.
    let export = verify::read_export(&records, || Ok(Arc::clone(crate::at_rest::keys()))).unwrap();
    let report = verify::check(&export, None, &std::collections::BTreeMap::new());
    let text = verify::describe(&report, None);
    assert!(report.ok(), "{row}: {text}");
    assert_eq!(report.given_up.iter().map(|g| (g.from, g.to)).collect::<Vec<_>>(), expect, "{row}: {text}");
    assert_eq!(text.contains("GIVEN UP"), !expect.is_empty(), "{row}: {text}");
    assert!(expect.is_empty() || !report.complete(), "{row}: the runs given up aren't checked: {text}");
    if damage == Damage::Middle {
        tampered(&export, &row);
    }
}

/// `export`, a removed deployment's records naming runs given up, changed: each fails `holon
/// verify`. Sealed again unchanged, it checks as it did; a plain copy's list is taken for nothing
/// (the gap is runs missing); the list moved one run over is no gap; taken out, the gap is runs
/// missing again.
fn tampered(export: &verify::Export, row: &str) {
    let dir = PathBuf::from(unique_temp_path("given-up-tampered"));
    std::fs::create_dir_all(&dir).unwrap();
    let copy = || -> verify::Export { serde_json::from_value(serde_json::to_value(export).unwrap()).unwrap() };
    let resealed = |changed: &verify::Export, name: &str| {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &serde_json::to_vec(changed).unwrap()).unwrap();
        let path = dir.join(name);
        crate::at_rest::write_sealed(&path, &crate::at_rest::deployment("kept"), &crate::at_rest::role::export("kept"), &gz.finish().unwrap()).unwrap();
        verify::check(&verify::read_export(&path, || Ok(Arc::clone(crate::at_rest::keys()))).unwrap(), None, &std::collections::BTreeMap::new())
    };
    let report = resealed(&copy(), "as-it-was.json.gz");
    assert!(report.ok() && !report.given_up.is_empty(), "{row}: sealed again unchanged: {}", verify::describe(&report, None));
    let plain = dir.join("plain.json");
    std::fs::write(&plain, serde_json::to_vec(export).unwrap()).unwrap();
    let report = verify::check(&verify::read_export(&plain, || Err("no keys".into())).unwrap(), None, &std::collections::BTreeMap::new());
    assert!(!report.ok() && report.given_up_unsealed && report.given_up.is_empty() && !report.gaps.is_empty(), "{row}: a plain copy: {}", verify::describe(&report, None));
    let mut moved = copy();
    moved.given_up.iter_mut().for_each(|g| g.from += 1);
    let report = resealed(&moved, "moved.json.gz");
    assert!(!report.ok() && !report.given_up_unmatched.is_empty(), "{row}: moved one run over: {}", verify::describe(&report, None));
    let mut out = copy();
    out.given_up.clear();
    let report = resealed(&out, "out.json.gz");
    assert!(!report.ok() && !report.gaps.is_empty() && report.given_up.is_empty(), "{row}: taken out: {}", verify::describe(&report, None));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Guards: giving runs up is for whoever may operate the host, beside flow.delete: a person holding
/// flow.delete alone is refused, and so is an agent, even bound to a role an admin made holding
/// both (it is a person's, can.rs `fixed`); nothing removed, nothing moved, no activity line. The
/// admin's is removed, the runs given up by them.
/// Why: giving run records up is the one way, but retention, that they go (release audit R110).
/// If it fails: http.rs's DELETE of a deployment no longer asks host.operate for `give_up`, or an
/// agent's kind no longer refuses it flow.delete.
#[test]
fn giving_runs_up_is_an_admins() {
    use crate::accounts::{Accounts, Kind};
    use crate::bindings::RoleText;
    const PW: &str = "correct horse battery";
    let mut k = kept_with(false, 8);
    assert_eq!(route(&k.state, &req("POST", "/deployments/kept/stop", vec![])).status_code, 200);
    let segment = segments_kept(&k)[1].clone();
    let span = span_of(&segment);
    let bytes = std::fs::read(&segment).unwrap();
    std::fs::write(&segment, &bytes[..bytes.len() - 10]).unwrap();
    reopen(&k);
    let book = Arc::new(Accounts::memory());
    k.state.access = crate::access::Access::new(Arc::clone(&book), vec![], vec![]);
    let (ada, _) = book.setup(&book.setup_code().unwrap(), "ada", PW).unwrap();
    let role = |name: &str, allows: &[&str], kinds: Vec<Kind>| {
        book.define_role(RoleText { name: name.into(), about: None, allows: allows.iter().map(|a| a.to_string()).collect(), kinds }, "ada", None).unwrap();
    };
    role("deletes", &["flow.view", "flow.delete"], vec![Kind::Person]);
    role("removes", &["flow.view", "flow.delete", "host.operate"], vec![Kind::Person, Kind::Agent]);
    let del = book.accept(&book.invite("del", "deletes", None, "ada").unwrap(), PW).unwrap().0;
    let send = |token: &str, method: &str, path: &str, body: serde_json::Value| {
        super::access_table::ask(&k.state, Some(token), method, path, if body.is_null() { vec![] } else { serde_json::to_vec(&body).unwrap() })
    };
    let (code, said) = send(&ada, "POST", "/people", serde_json::json!({ "name": "remover", "kind": "agent", "role": "removes" }));
    assert_eq!(code, 200, "an agent bound to a role holding both: {said}");
    let (code, said) = send(&ada, "POST", "/people/remover/tokens", serde_json::json!({ "label": "its tools" }));
    assert_eq!(code, 200, "{said}");
    let remover = serde_json::from_str::<serde_json::Value>(&said).unwrap()["token"].as_str().unwrap().to_string();
    let url = format!("/deployments/kept?give_up={}", crate::deploy::give_up_arg(&[span]));
    for (who, token, says) in [("del", &del, "host.operate"), ("remover", &remover, "agent")] {
        let (code, said) = send(token, "DELETE", &url, serde_json::Value::Null);
        assert_eq!(code, 403, "{who}: {said}");
        assert!(said.contains(says), "{who}: says why: {said}");
        assert!(k.state.deployments.lock().unwrap().contains_key("kept") && segment.is_file(), "{who}: nothing removed, nothing moved");
        assert!(k.state.activity.newest(50, |e| e.action == "deleted").is_empty(), "{who}: no activity line");
    }
    let (code, said) = send(&ada, "DELETE", &url, serde_json::Value::Null);
    assert_eq!(code, 200, "the admin: {said}");
    let deleted = k.state.activity.newest(50, |e| e.action == "deleted");
    assert_eq!((deleted.len(), deleted[0].by.as_deref()), (1, Some("ada")), "{deleted:?}");
    let records = PathBuf::from(serde_json::from_str::<serde_json::Value>(&said).unwrap()["records"].as_str().unwrap().to_string());
    let export = verify::read_export(&records, || Ok(Arc::clone(crate::at_rest::keys()))).unwrap();
    assert_eq!(export.given_up.iter().map(|g| (g.from, g.to, g.by.as_deref())).collect::<Vec<_>>(), vec![(span.0, span.1, Some("ada"))]);
}
