//! What a server starting does with what its runs spooled to send and hadn't sent when it stopped
//! (mails.rs, queues.rs): a mail, or a queue's message, a file each in its spool folder.
//!
//! An item is spooled before its run's line is written, held until that line is on disk, then
//! marked landed (its file renamed `<name>.landed.<ext>`) and queued; one whose line isn't
//! written is removed then (firing.rs). So as a server starts:
//! - one marked landed is sent: its run's record was on disk. The run log isn't asked: by then
//!   the deployment's keep may have let the run go, or its log may not read, and neither says the
//!   run didn't land (release audit R104: a mail held while its mail server was down was removed
//!   at the restart once the keep had let its run go);
//! - one not marked is of a run whose line wasn't on disk yet when the server stopped (the moments
//!   before a crash), or whose mark couldn't be made: its run among the deployment's kept runs, it
//!   is sent if the run finished and removed if not; not among them in a log read whole, it is
//!   removed (a crash lost the run's line, and what is sent is what the log holds); not among
//!   them in a log that can't all be read, it is kept, unsent, and the admin's warnings say so
//!   until a start that reads the log sends or removes it;
//! - one of a deployment that isn't deployed is removed, and said on stderr: its deployment, its
//!   own bindings and the timeline its outcome goes on are gone;
//! - the spool folder that can't be listed leaves all it holds as it is, unsent, and the admin's
//!   warnings say so.
//!
//! A channel's kept packs are rebuilt from the log as the server starts too (channels.rs's
//! `from_log`), but nothing is sent or removed by them: a log that can't all be read rebuilds
//! those it could read, and says so on stderr.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::recorder::KeptRuns;
use crate::storage::Warning;
use crate::sync::Mutex;

/// What the name of an item's file marked landed holds before its extension.
const LANDED: &str = "landed";

/// `file`, an item whose run's line is on disk, marked landed: renamed `<name>.landed.<ext>`.
/// Its new place; or an error, and the item is sent from where it is (a start would then ask the
/// log for it).
pub(crate) fn mark_landed(file: &Path) -> std::io::Result<PathBuf> {
    let ext = file.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let marked = file.with_extension(format!("{LANDED}.{ext}"));
    std::fs::rename(file, &marked)?;
    Ok(marked)
}

/// Whether `file` is an item marked landed.
pub(crate) fn is_landed(file: &Path) -> bool {
    file.file_stem().and_then(|stem| Path::new(stem).extension()).is_some_and(|e| e == LANDED)
}

/// What a start does with an item.
#[derive(Debug, PartialEq)]
pub(crate) enum AtStart<'a> {
    Send,
    Remove,
    /// Kept unsent, and why: its run can't be told.
    Keep(&'a str),
}

/// What a start does with an item not marked landed, of run `run`, by the deployment's kept runs.
pub(crate) fn unmarked<'a>(kept: &'a KeptRuns, run: &str) -> AtStart<'a> {
    match (kept.runs.get(run), &kept.unread) {
        (Some(true), _) => AtStart::Send,
        (Some(false), _) | (None, None) => AtStart::Remove,
        (None, Some(why)) => AtStart::Keep(why),
    }
}

/// What a kind of item is called in the admin's warnings.
pub(crate) struct Kind {
    /// In a warning's key: `spool:mail`, `spool:message`.
    pub(crate) key: &'static str,
    pub(crate) one: &'static str,
    pub(crate) many: &'static str,
    /// Its spool folder.
    pub(crate) folder: &'static str,
    /// The warnings' told words (storage.rs `Warning::told`): its folder not listed, and items of
    /// it kept back.
    pub(crate) told: [&'static str; 2],
}

/// What a start kept back, unsent, until the next: by deployment, how many items and why their
/// runs can't be told; and why the spool folder couldn't be listed, if it couldn't.
#[derive(Default)]
pub(crate) struct KeptBack {
    items: Mutex<BTreeMap<String, (usize, String)>>,
    folder: Mutex<Option<String>>,
}

impl KeptBack {
    /// An item of `slug` kept, its run not told: `why` its log can't all be read.
    pub(crate) fn item(&self, slug: &str, why: &str) {
        self.items.lock().unwrap().entry(slug.to_string()).or_insert_with(|| (0, why.to_string())).0 += 1;
    }

    /// The spool folder, or one of its entries, not listed: `why`.
    pub(crate) fn folder(&self, why: String) {
        self.folder.lock().unwrap().get_or_insert(why);
    }

    /// The admin's warnings of it (storage.rs's `warnings`): each lasts until the server starts
    /// again, since only a start sends or removes what was spooled before it.
    pub(crate) fn warnings(&self, kind: &Kind) -> Vec<Warning> {
        let mut out = Vec::new();
        if let Some(why) = self.folder.lock().unwrap().as_ref() {
            out.push(Warning {
                key: kind.key.to_string(),
                told: kind.told[0],
                says: format!("the {} folder can't all be listed ({why}): what was spooled in it before the server started is neither sent nor removed", kind.folder),
                todo: "make it readable again, then restart the server: what it holds is sent or removed then".into(),
            });
        }
        for (slug, (n, why)) in self.items.lock().unwrap().iter() {
            out.push(Warning {
                key: format!("{}:{slug}", kind.key),
                told: kind.told[1],
                says: format!(
                    "{n} {} of {slug:?} spooled before the server started {} neither sent nor removed: whether {} landed can't be told, since its run log can't all be read ({why})",
                    if *n == 1 { kind.one } else { kind.many },
                    if *n == 1 { "is" } else { "are" },
                    if *n == 1 { "its run" } else { "their runs" },
                ),
                todo: "make its run log readable again, or put it back from a copy of the data folder, then restart the server: each is sent then if its run landed, and removed if a crash lost it".into(),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::{Log, LogCell, LogEntry};

    /// A spool of the class: a mail's (mails.rs) or a queue's message's (queues.rs).
    trait Spool {
        type Items;
        /// Its warnings' key.
        const KEY: &'static str;
        /// Its folder, beside its bindings' file.
        const FOLDER: &'static str;
        fn open(dir: &Path) -> Self;
        fn spool(&self, slug: &str, run: &str) -> Self::Items;
        fn hold(&self, slug: &str, seq: u64, items: Self::Items);
        fn release(&self, slug: &str, synced: u64);
        fn recover(&self, deployed: impl Fn(&str) -> bool, runs: impl FnMut(&str) -> KeptRuns);
        fn queued(&self) -> Vec<String>;
        fn kept_back(&self) -> Vec<Warning>;
    }

    impl Spool for crate::mails::Mails {
        type Items = Vec<crate::mails::Queued>;
        const KEY: &'static str = "spool:mail";
        const FOLDER: &'static str = "mail-spool";
        fn open(dir: &Path) -> Self {
            crate::mails::tests::spooling(dir)
        }
        fn spool(&self, slug: &str, run: &str) -> Self::Items {
            crate::mails::tests::spooled(self, slug, run)
        }
        fn hold(&self, slug: &str, seq: u64, items: Self::Items) {
            crate::mails::Mails::hold(self, slug, seq, items)
        }
        fn release(&self, slug: &str, synced: u64) {
            crate::mails::Mails::release(self, slug, synced)
        }
        fn recover(&self, deployed: impl Fn(&str) -> bool, runs: impl FnMut(&str) -> KeptRuns) {
            crate::mails::Mails::recover(self, deployed, runs)
        }
        fn queued(&self) -> Vec<String> {
            crate::mails::tests::queued(self)
        }
        fn kept_back(&self) -> Vec<Warning> {
            crate::mails::Mails::kept_back(self)
        }
    }

    impl Spool for crate::queues::Queues {
        type Items = Vec<crate::queues::Queued>;
        const KEY: &'static str = "spool:message";
        const FOLDER: &'static str = "queue-spool";
        fn open(dir: &Path) -> Self {
            crate::queues::tests::spooling(dir)
        }
        fn spool(&self, slug: &str, run: &str) -> Self::Items {
            crate::queues::tests::spooled(self, slug, run)
        }
        fn hold(&self, slug: &str, seq: u64, items: Self::Items) {
            crate::queues::Queues::hold_sends(self, slug, seq, items)
        }
        fn release(&self, slug: &str, synced: u64) {
            crate::queues::Queues::release(self, slug, synced)
        }
        fn recover(&self, deployed: impl Fn(&str) -> bool, runs: impl FnMut(&str) -> KeptRuns) {
            crate::queues::Queues::recover(self, deployed, runs)
        }
        fn queued(&self) -> Vec<String> {
            crate::queues::tests::queued(self)
        }
        fn kept_back(&self) -> Vec<Warning> {
            crate::queues::Queues::kept_back(self)
        }
    }

    /// How far a run that spooled an item got before the server stopped.
    #[derive(Clone, Copy)]
    enum Got {
        /// Its line on disk and its item marked landed: as it was sent, or waiting for its mail
        /// server.
        Marked,
        /// Its line on disk, its item not marked yet: a crash between the two.
        Unmarked,
        /// Its line not written: a crash before it.
        Lost,
    }

    /// A deployment `app`'s run log in a folder of its own, keeping `keep` runs (4 in its file,
    /// the older sealed), and a spool beside it.
    struct World<S> {
        dir: PathBuf,
        log: PathBuf,
        keep: usize,
        cell: LogCell,
        spool: S,
    }

    fn log_at(path: &Path, keep: usize) -> Log {
        let mut log = Log::open_keeping(Some(path.to_path_buf()), keep);
        log.set_hot(4);
        log
    }

    fn entry(run: &str) -> LogEntry {
        LogEntry { run_id: run.into(), at_ms: crate::firing::started_now().0, ..Default::default() }
    }

    /// `path` put aside, and what it isn't put in its place (a file for a folder, a folder for a
    /// file): it can't be listed, or read, as what it was. Undone by `heal`.
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

    impl<S: Spool> World<S> {
        fn new(keep: usize) -> World<S> {
            let dir = std::env::temp_dir().join(format!("holon-spools-{}-{}", std::process::id(), rand::random::<u32>()));
            std::fs::create_dir_all(&dir).unwrap();
            let log = dir.join("app.jsonl");
            World { cell: LogCell::new(log_at(&log, keep)), spool: S::open(&dir), dir, log, keep }
        }

        /// A finished run that sent nothing: its line written and synced, and the log cut back if
        /// that is due.
        fn ran(&self, run: &str) {
            self.cell.with(|l| l.push(entry(run)));
            self.cell.sync().unwrap();
            self.cell.cut_if_planned();
        }

        /// A run of `slug` that spooled an item, as far as `got`.
        fn sends(&self, slug: &str, run: &str, got: Got) {
            let items = self.spool.spool(slug, run);
            match got {
                // Its item held, not released: the crash loses the hold, as it loses the line.
                Got::Lost => drop(items),
                Got::Unmarked => {
                    self.cell.with(|l| l.push(entry(run)));
                    self.cell.sync().unwrap();
                    drop(items);
                }
                Got::Marked => {
                    let seq = self.cell.with(|l| l.push(entry(run)));
                    self.spool.hold(slug, seq, items);
                    self.cell.sync().unwrap();
                    self.spool.release(slug, self.cell.synced());
                    assert!(self.files().iter().any(|f| f.starts_with(&format!("{slug}.{run}.")) && is_landed(Path::new(f))), "{run}'s item marked landed: {:?}", self.files());
                }
            }
            self.cell.cut_if_planned();
        }

        /// The spool's files.
        fn files(&self) -> Vec<String> {
            let mut files: Vec<String> = std::fs::read_dir(self.dir.join(S::FOLDER)).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
            files.sort();
            files
        }

        /// The server started again: its log opened from its files (`before` done to them first,
        /// `after` once it is open), a spool opened on its folder, and that spool recovered,
        /// `deployed` the deployments there are.
        fn restart(&self, before: impl FnOnce(&World<S>), after: impl FnOnce(&World<S>), deployed: &[&str]) -> S {
            before(self);
            let cell = LogCell::new(log_at(&self.log, self.keep));
            assert!(cell.with(|l| l.refused().is_none()), "the log opens");
            after(self);
            let spool = S::open(&self.dir);
            spool.recover(|slug| deployed.contains(&slug), |_| cell.kept_runs());
            spool
        }

        /// What a start did: the runs whose items it queued, `sent`; each other item it left
        /// unsent, `kept`, and said so, `why` its log couldn't all be read; the rest removed.
        fn did(&self, case: &str, spool: &S, sent: &[&str], kept: &[&str], why: &str) {
            let what = format!("{}: {case}", S::KEY);
            assert_eq!(spool.queued(), sent, "{what}: sent");
            assert_eq!(self.files().len(), sent.len() + kept.len(), "{what}: the items left in the spool, the rest removed: {:?}", self.files());
            for run in kept {
                assert!(self.files().iter().any(|f| f.starts_with(&format!("app.{run}.")) && !is_landed(Path::new(f))), "{what}: {run}'s kept: {:?}", self.files());
            }
            let warned = spool.kept_back();
            if kept.is_empty() {
                assert!(warned.is_empty(), "{what}: no warning: {:?}", warned.iter().map(|w| &w.says).collect::<Vec<_>>());
            } else {
                assert_eq!(warned.len(), 1, "{what}: one warning: {:?}", warned.iter().map(|w| &w.says).collect::<Vec<_>>());
                let w = &warned[0];
                assert_eq!(w.key, format!("{}:app", S::KEY), "{what}");
                assert!(w.says.starts_with(&format!("{} ", kept.len())) && w.says.contains("can't be told") && w.says.contains(why) && w.todo.contains("restart the server"), "{what}: {} / {}", w.says, w.todo);
            }
        }
    }

    impl<S> Drop for World<S> {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Guards: what a server starting does with what its runs spooled and hadn't sent, in each
    /// spool, by how its run ended and what of its log can be read: an item marked landed is
    /// sent, its run since let go by the keep, or in a segment that can't be read, or with the
    /// log's file unreadable; one not marked whose run the log holds finished is sent; one whose
    /// line a crash lost is removed; one whose run can't be told (not among the kept runs of a log
    /// that can't all be read) is kept unsent, and the admin's warnings say so, with why and to
    /// restart; one of a deployment that isn't there is removed; the spool folder that can't be
    /// listed sends and removes nothing and says so, and once it can be, the next start sends
    /// what it holds.
    /// Why: a start asked the log alone, and took a run it didn't find as one a crash lost: a mail
    /// held while its mail server was down was removed, never sent, once the deployment's keep had
    /// let its run go, or when the segment holding its run couldn't be read; and a spool folder
    /// that couldn't be listed was passed over without a word (release audit R104).
    /// If it fails: the case it names: `release`/`hold_sends` no longer mark what landed, or
    /// `recover` takes an unreadable log as whole (`spools::unmarked`), or says nothing.
    #[test]
    fn a_start_sends_what_landed_removes_what_a_crash_lost_and_keeps_what_it_cant_tell() {
        assert!(is_landed(Path::new("app.r1.0.landed.mail")) && !is_landed(Path::new("app.landed.0.mail")), "a run named landed isn't marked");
        class::<crate::mails::Mails>();
        class::<crate::queues::Queues>();
    }

    fn class<S: Spool>() {
        // A run the keep let go of, its item marked as it waited for its server.
        let w = World::<S>::new(4);
        w.sends("app", "let-go", Got::Marked);
        for i in 0..20 {
            w.ran(&format!("r{i}"));
        }
        assert!(!w.cell.kept_runs().runs.contains_key("let-go"), "the keep let it go");
        let s = w.restart(|_| {}, |_| {}, &["app"]);
        w.did("a run the keep let go of", &s, &["let-go"], &[], "");

        // A crash: one run's line on disk and its item not marked yet, one's line lost; and one
        // marked before them.
        let w = World::<S>::new(20);
        w.sends("app", "marked", Got::Marked);
        w.sends("app", "unmarked", Got::Unmarked);
        w.sends("app", "lost", Got::Lost);
        let s = w.restart(|_| {}, |_| {}, &["app"]);
        w.did("a crash, the log read whole", &s, &["marked", "unmarked"], &[], "");

        // Its sealed segments' folder can't be listed as the log opens: the runs its file holds
        // are read, the sealed ones can't be.
        let w = World::<S>::new(40);
        w.sends("app", "sealed", Got::Marked);
        for i in 0..20 {
            w.ran(&format!("r{i}"));
        }
        w.sends("app", "unmarked", Got::Unmarked);
        w.sends("app", "lost", Got::Lost);
        let sealed = w.dir.join("app.sealed");
        assert!(sealed.is_dir(), "its older runs sealed");
        let s = w.restart(|_| spoil(&sealed), |_| {}, &["app"]);
        w.did("its segments unreadable", &s, &["sealed", "unmarked"], &["lost"], "can't be listed");
        heal(&sealed);
        let s = w.restart(|_| {}, |_| {}, &["app"]);
        w.did("its segments readable again, a start after", &s, &["sealed", "unmarked"], &[], "");

        // Its file can't be read once the log is open: no run of it is read.
        let w = World::<S>::new(20);
        w.sends("app", "marked", Got::Marked);
        w.sends("app", "unmarked", Got::Unmarked);
        w.sends("app", "lost", Got::Lost);
        let s = w.restart(|_| {}, |w| spoil(&w.log), &["app"]);
        w.did("its file unreadable", &s, &["marked"], &["lost", "unmarked"], "can't be");
        heal(&w.log);
        let s = w.restart(|_| {}, |_| {}, &["app"]);
        w.did("its file readable again, a start after", &s, &["marked", "unmarked"], &[], "");

        // A deployment that isn't there any more.
        let w = World::<S>::new(20);
        w.sends("gone", "marked", Got::Marked);
        w.sends("gone", "lost", Got::Lost);
        let s = w.restart(|_| {}, |_| {}, &["app"]);
        w.did("a deployment not there", &s, &[], &[], "");

        // The spool folder can't be listed: nothing sent, nothing removed, said; listed again, the
        // next start sends what it holds.
        let w = World::<S>::new(20);
        w.sends("app", "marked", Got::Marked);
        let folder = w.dir.join(S::FOLDER);
        spoil(&folder);
        let s = w.restart(|_| {}, |_| {}, &["app"]);
        assert!(s.queued().is_empty(), "{}: the folder unlistable: none sent", S::KEY);
        let warned = s.kept_back();
        assert!(warned.len() == 1 && warned[0].key == S::KEY && warned[0].says.contains("can't all be listed") && warned[0].todo.contains("restart the server"), "{}: the folder unlistable: {:?}", S::KEY, warned.iter().map(|w| &w.says).collect::<Vec<_>>());
        heal(&folder);
        let s = w.restart(|_| {}, |_| {}, &["app"]);
        w.did("its folder listed again, a start after", &s, &["marked"], &[], "");
    }
}
