//! What the server is failing to keep now: the deployments file it couldn't write, the run logs
//! it couldn't sync to disk. Each is a warning to an admin (storage.rs's `warnings`: the admin
//! page, `holon host`, the terminal editor), with why, and makes `GET /health` answer 503
//! "not ok" until the next write of it goes through: a load balancer then sends requests to a
//! server that can keep what they do. The witness not taking heads is a warning only (heads.rs),
//! since one witness down would take every server it witnesses out at once.
//!
//! Kept by the deployments file's place, so two servers in one process (the tests') don't see
//! each other's; a table kept nowhere has nothing to fail.

use std::collections::BTreeMap;
use std::path::PathBuf;
use crate::sync::Mutex;

use crate::paths::Place;

/// A writer whose failure the server can't simply go on from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Writer {
    /// The deployments file (persist.rs), with the waiting runs it keeps: a deploy, a grant or a
    /// pause made while it fails is lost when the server starts again.
    Deployments,
    /// The run logs and event logs synced to disk every second, the witness's queue with them
    /// (main.rs's `sync_to_disk`): while it fails a strict run answers 500, and a lazy run's
    /// line may be lost in a power loss.
    Logs,
}

impl Writer {
    /// What `/health` says of it, to anyone who asks: never a path or an error's text, which
    /// only an admin reads (the warnings).
    pub(crate) fn health(self) -> &'static str {
        match self {
            Writer::Deployments => "the deployments file can't be written",
            Writer::Logs => "the run logs can't be synced to disk",
        }
    }
}

/// What `/health` answers while something fails: this, then what fails.
pub(crate) const NOT_OK: &str = "not ok";

static FAILING: Mutex<BTreeMap<(PathBuf, Writer), String>> = Mutex::new(BTreeMap::new());

/// Note how the last write of `writer` went for the server whose deployments file is `at`.
pub(crate) fn note(at: &Place, writer: Writer, result: &Result<(), String>) {
    let Some(path) = at.at() else { return };
    let key = (PathBuf::from(path.clone()), writer);
    let mut failing = FAILING.lock().unwrap();
    match result {
        Ok(()) => drop(failing.remove(&key)),
        Err(e) => drop(failing.insert(key, e.clone())),
    }
}

/// What fails now for the server whose deployments file is `at`, with why.
pub(crate) fn now(at: &Place) -> Vec<(Writer, String)> {
    let Some(path) = at.at() else { return Vec::new() };
    let path = PathBuf::from(path.clone());
    FAILING.lock().unwrap().iter().filter(|((p, _), _)| *p == path).map(|((_, w), e)| (*w, e.clone())).collect()
}

/// `GET /health`'s answer for the server whose deployments file is `at`: 200 "ok", or 503
/// "not ok: " and what fails.
pub(crate) fn health(at: &Place) -> (u16, String) {
    let failing = now(at);
    if failing.is_empty() {
        return (200, "ok".into());
    }
    let what: Vec<&str> = failing.iter().map(|(w, _)| w.health()).collect();
    (503, format!("{NOT_OK}: {}", what.join("; ")))
}

/// A write or read of the server's own files that failed, as two texts kept apart: what whoever
/// asked is told, and why, which only an admin reads (the warnings, stderr). Why holds where on
/// the server's disk and the error's own text, which can name a path or a user; whoever asked
/// may be a flow's caller, or someone signing in (audit R71). What they are told is a fixed
/// text, with at most a line and a column, or a reason holon gives of what it was sent: no place
/// and no error's text can be put in it.
/// Its Display and its Debug both say what whoever asked is told and no more, so a route that
/// formats it (`{e}`, `{e:?}`), or an io::Error or a store error holding it, can't pass where or
/// why on (audit R87). Why is had by name alone ([`Failed::for_the_admin`]): for stderr, the
/// warnings and the tests, never an answer (tests/run_steps.rs holds every call of it to a list).
/// Don't widen the Debug back to why: that is what this guards.
#[derive(Clone, PartialEq)]
pub(crate) struct Failed {
    told: String,
    why: String,
    /// Why is on stderr already, said where the failure was made ([`Failed::of_file`]).
    noted: bool,
}

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.told)
    }
}

impl std::fmt::Debug for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.told)
    }
}

impl std::error::Error for Failed {}

/// What a store of the server's (holon-host's, its units and values) is told to say when a file
/// of its own fails: StoreKeys's `failed`.
pub(crate) const STORE_FAILED: &str = "the server couldn't read or write its store";

/// What a file the server reads under its data folder (at_rest::disk) says when it fails.
pub(crate) const FILE_UNREAD: &str = "the server couldn't read one of its files";

impl Failed {
    /// `told` to whoever asked, `why` to the admin.
    pub(crate) fn new(told: &'static str, why: impl Into<String>) -> Failed {
        Failed { told: told.into(), why: why.into(), noted: false }
    }

    /// A file that doesn't parse: `told` and where in it, as a line and a column (a line of it,
    /// or a value its message quotes, can hold what the file names: a secret's source, say).
    pub(crate) fn at(told: &'static str, line: usize, column: usize, why: impl Into<String>) -> Failed {
        Failed { told: format!("{told} (line {line}, column {column})"), why: why.into(), noted: false }
    }

    /// A reason holon writes itself of what it was given (a limit it doesn't know, say), with
    /// nothing of the server's disk or an error's text in it, told as it is; `why` may add where.
    pub(crate) fn reason(reason: String, why: impl Into<String>) -> Failed {
        Failed { told: reason, why: why.into(), noted: false }
    }

    /// `e`, an error of the server's own file at `path`, as an io::Error whose text is `told`,
    /// its kind kept (a caller that takes NotFound for none still does); where and why are on
    /// stderr now, for the admin, but for a file that isn't there, which callers take for none.
    /// One that holds a Failed already is left as it is.
    pub(crate) fn of_file(told: &'static str, path: &std::path::Path, e: std::io::Error) -> std::io::Error {
        if Failed::in_io(&e).is_some() {
            return e;
        }
        let kind = e.kind();
        let mut failed = Failed::new(told, format!("{}: {e}", path.display()));
        if kind != std::io::ErrorKind::NotFound {
            eprintln!("warning: {}: {}", failed.told, failed.why);
            failed.noted = true;
        }
        std::io::Error::new(kind, failed)
    }

    /// The Failed `e` holds, if it holds one: its why for the admin or a test.
    pub(crate) fn in_io(e: &std::io::Error) -> Option<&Failed> {
        e.get_ref().and_then(|inner| inner.downcast_ref::<Failed>())
    }

    /// Why, for the admin (stderr, the warnings) and the tests: never to build an answer.
    pub(crate) fn for_the_admin(&self) -> &str {
        &self.why
    }

    /// The same failure told in other words, made from what it told: why is kept as it was.
    pub(crate) fn retold(self, told: impl FnOnce(&str) -> String) -> Failed {
        Failed { told: told(&self.told), ..self }
    }

    /// What whoever asked is told, where the admin hears why another way (a writer's warning).
    pub(crate) fn told(self) -> String {
        self.told
    }

    /// What whoever asked is told, once why is on stderr for the admin.
    pub(crate) fn said(self) -> String {
        if !self.noted && self.why != self.told {
            eprintln!("warning: {}: {}", self.told, self.why);
        }
        self.told
    }
}

/// The files failing now, by the deployments file's place, what whoever asked was told, and where
/// (an admin's words: a path): why ([`note_file`]).
static FILES: Mutex<BTreeMap<(PathBuf, String, String), String>> = Mutex::new(BTreeMap::new());

/// A file the server reads or writes, failing now: kept, with why, until the same (told, place)
/// goes through. Listed in the admin's warnings (storage.rs), never /health's 503. `place` is
/// admin-only (a path, "deployment X's seen file"). `told` is the caller's fixed sentence, passed
/// again with Ok: the key is it, not `Failed`'s own told, so a Failed told in other words still
/// clears. Two places under one told (two packs, two deployments' seen files) never clear each
/// other. A server whose deployments file is kept nowhere notes nothing, as [`note`] doesn't.
pub(crate) fn note_file(at: &Place, told: &str, place: &str, result: Result<(), &Failed>) {
    let Some(path) = at.at() else { return };
    let key = (PathBuf::from(path.clone()), told.to_string(), place.to_string());
    let mut files = FILES.lock().unwrap();
    match result {
        Ok(()) => drop(files.remove(&key)),
        Err(f) => drop(files.insert(key, f.why.clone())),
    }
}

/// What fails now for the server whose deployments file is `at`: (told, place, why).
pub(crate) fn files_now(at: &Place) -> Vec<(String, String, String)> {
    let Some(path) = at.at() else { return Vec::new() };
    let path = PathBuf::from(path.clone());
    FILES.lock().unwrap().iter().filter(|((p, _, _), _)| *p == path).map(|((_, told, place), why)| (told.clone(), place.clone(), why.clone())).collect()
}

/// Where in `text` byte `at` is: its line and column, from 1.
pub(crate) fn line_and_column(text: &str, at: usize) -> (usize, usize) {
    let before = String::from_utf8_lossy(&text.as_bytes()[..at.min(text.len())]);
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::Failed;

    /// Whatever formats a Failed, alone or inside an io::Error or a store error, gets what
    /// whoever asked is told: never where or why, which `why` alone gives (audit R87).
    #[test]
    fn a_failure_says_only_what_whoever_asked_is_told() {
        let failed = Failed::new("the thing failed", "C:\\data\\x.json: Access is denied. (os error 5)");
        assert_eq!(format!("{failed}"), "the thing failed");
        assert_eq!(format!("{failed:?}"), "the thing failed");
        assert_eq!(format!("{failed:#?}"), "the thing failed");
        assert!(failed.for_the_admin().contains("x.json") && failed.for_the_admin().contains("denied"));

        let dir = std::env::temp_dir().join(format!("holon-failed-{}", std::process::id()));
        let path = dir.join("x.json");
        let os = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let words = os.to_string();
        let e = Failed::of_file("the thing failed", &path, os);
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "its kind kept");
        let store = holon_host::StoreError::Io(Failed::of_file("the thing failed", &path, e));
        for said in [format!("{store:?}"), format!("{store:#?}")] {
            assert!(said.contains("the thing failed") && !said.contains("x.json") && !said.contains(&words), "{said}");
        }
        let holon_host::StoreError::Io(e) = store else { unreachable!() };
        for said in [e.to_string(), format!("{e:?}"), format!("{e:#?}")] {
            assert!(said.contains("the thing failed") && !said.contains("x.json") && !said.contains(&words), "{said}");
        }
        let why = Failed::in_io(&e).map(Failed::for_the_admin).unwrap();
        assert!(why.contains("x.json") && why.contains(&words), "where and why kept for the admin, once: {why}");
        assert!(!why.contains("the thing failed"), "a Failed passed again isn't wrapped again: {why}");

        let gone = Failed::of_file("the thing failed", &path, std::io::Error::from(std::io::ErrorKind::NotFound));
        assert_eq!(gone.kind(), std::io::ErrorKind::NotFound, "a file not there is still none to whoever reads the kind");
        assert!(!gone.to_string().contains("x.json"));
    }
}
