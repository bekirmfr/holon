//! Commit locks on shared resources (docs/L4s/databases-L4.md §7.1): what a run's commit holds
//! from the check of what it read to its last write, so two runs of different deployments never
//! both pass a check and both write.
//!
//! - **Per file,** by its address on this server (a folder's real path, then the file's name), not
//!   per deployment: two deployments that bind one folder, under one name or two, take one lock
//!   for one file. A run that writes `a.txt` never waits for one that read `b.txt`.
//! - **And per folder, by intent** (hierarchical locking): one that locks a file also says so on
//!   its folder ([`Mode::IntentShared`], [`Mode::IntentExclusive`]); one that depends on the whole
//!   folder (a listing, the last try that holds what it may touch for its whole run) holds the
//!   folder itself ([`Mode::Shared`], [`Mode::Exclusive`]), and so waits for, and holds off, every
//!   writer of a file in it. A listing can't be locked per file: a file created under its prefix
//!   changes it.
//! - **Shared or exclusive:** a run that only checked a read shares its lock with others like
//!   it; one that writes holds it alone. Readers never wait for readers.
//! - **Across servers,** a folder's lock is also an OS lock on a file in it ([`LOCK_FILE`]), when
//!   the folder is bound `shared`, so two servers that bind one share exclude each other too:
//!   whole-folder there (a reader of a file shares it, a writer of one holds it alone), as an OS
//!   lock is on a whole file. The file is one of the server's own (its name starts `.holon-tmp-`):
//!   no flow lists, reads or writes it.
//! - **Ordered and bounded:** a commit takes its locks after its deployment's state lock, in the
//!   order of their addresses, so two commits never wait for each other in a circle; and it waits
//!   at most its `commit_wait`, past which it lets go of every lock it took and says which
//!   resource it waited for.

use std::sync::Condvar;
use std::time::Instant;

use crate::*;

/// The file in a folder whose OS lock stands for the folder's commit lock across servers.
pub(crate) const LOCK_FILE: &str = ".holon-tmp-commit.lock";

/// How a commit holds a resource. Which may be held at once:
///
/// |       | IS  | IX  | S   | X   |
/// |-------|-----|-----|-----|-----|
/// | IS    | yes | yes | yes |     |
/// | IX    | yes | yes |     |     |
/// | S     | yes |     | yes |     |
/// | X     |     |     |     |     |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// On a folder: it holds a file in it shared.
    IntentShared,
    /// On a folder: it holds a file in it exclusive.
    IntentExclusive,
    /// It checks a read of it: others that only check may hold it too.
    Shared,
    /// It writes it: no one else holds it meanwhile.
    Exclusive,
}

impl Mode {
    /// One mode that allows what both do: the stronger, and exclusive for a folder held shared
    /// and by intent to write in it.
    fn with(self, other: Mode) -> Mode {
        use Mode::*;
        match (self, other) {
            (a, b) if a == b => a,
            (IntentShared, m) | (m, IntentShared) => m,
            _ => Exclusive,
        }
    }

    /// Whether a lock already held as `held` may be held this way too.
    fn fits(self, held: &Held) -> bool {
        let [is, ix, s] = held.counts;
        !held.exclusive
            && match self {
                Mode::IntentShared => true,
                Mode::IntentExclusive => s == 0,
                Mode::Shared => ix == 0,
                Mode::Exclusive => is + ix + s == 0,
            }
    }

    /// The OS lock it takes: a whole-file lock, shared for reading, exclusive for writing.
    fn os_exclusive(self) -> bool {
        matches!(self, Mode::IntentExclusive | Mode::Exclusive)
    }
}

/// A resource a commit locks: its address, how it is said to the flow's owner (never a path), how
/// it is held, and the file whose OS lock stands for it across servers, if it has one.
#[derive(Clone, Debug)]
pub(crate) struct Wanted {
    pub(crate) address: String,
    pub(crate) label: String,
    pub(crate) mode: Mode,
    pub(crate) file: Option<PathBuf>,
}

/// Every resource's lock on this server, by address. One table for the server: every deployment's
/// runs take theirs here.
#[derive(Debug, Default)]
pub(crate) struct Locks {
    table: Mutex<BTreeMap<String, Arc<Lock>>>,
}

#[derive(Debug, Default)]
struct Lock {
    held: Mutex<Held>,
    freed: Condvar,
}

/// Who holds a lock: how many in each intent or shared mode ([`Mode`]'s order), and whether one
/// holds it exclusive.
#[derive(Debug, Default)]
struct Held {
    counts: [usize; 3],
    exclusive: bool,
}

impl Held {
    fn count(&mut self, mode: Mode) -> &mut usize {
        match mode {
            Mode::IntentShared => &mut self.counts[0],
            Mode::IntentExclusive => &mut self.counts[1],
            Mode::Shared | Mode::Exclusive => &mut self.counts[2],
        }
    }
}

/// A lock a commit holds: let go when dropped.
#[derive(Debug)]
pub(crate) struct Guard {
    lock: Arc<Lock>,
    mode: Mode,
    /// The OS lock on its file, released when the file is closed.
    _file: Option<std::fs::File>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut held = self.lock.held.lock().unwrap();
        match self.mode {
            Mode::Exclusive => held.exclusive = false,
            mode => *held.count(mode) -= 1,
        }
        self.lock.freed.notify_all();
    }
}

impl Locks {
    /// Take every lock in `wanted`, in the order of their addresses (one wanted twice is taken
    /// once, in a mode that allows both: [`Mode::with`]), waiting until `until` at most. Past it, none is held, and the
    /// error is the resource it waited for, as its label says it.
    pub(crate) fn take(&self, wanted: Vec<Wanted>, until: Instant) -> Result<Vec<Guard>, String> {
        let mut merged: BTreeMap<String, Wanted> = BTreeMap::new();
        for w in wanted {
            merged
                .entry(w.address.clone())
                .and_modify(|m| {
                    m.mode = m.mode.with(w.mode);
                    if m.file.is_none() {
                        m.file.clone_from(&w.file);
                    }
                })
                .or_insert(w);
        }
        let mut guards = Vec::with_capacity(merged.len());
        for (address, w) in merged {
            let lock = Arc::clone(self.table.lock().unwrap().entry(address.clone()).or_default());
            if !lock.acquire(w.mode, until) {
                return Err(w.label);
            }
            // Held from here: let go if the OS lock can't be had in time.
            let mut guard = Guard { lock, mode: w.mode, _file: None };
            match w.file.as_deref().map(|f| os_lock(f, w.mode, until)).transpose() {
                Ok(file) => {
                    guard._file = file.flatten();
                    guards.push(guard);
                }
                Err(()) => return Err(w.label),
            }
        }
        Ok(guards)
    }
}

impl Lock {
    fn acquire(&self, mode: Mode, until: Instant) -> bool {
        let mut held = self.held.lock().unwrap();
        loop {
            if mode.fits(&held) {
                match mode {
                    Mode::Exclusive => held.exclusive = true,
                    mode => *held.count(mode) += 1,
                }
                return true;
            }
            let Some(left) = until.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else { return false };
            held = self.freed.wait_timeout(held, left).unwrap().0;
        }
    }
}

/// The OS lock on `path`, shared or exclusive, waiting until `until` at most: `Err` past it.
/// A file that can't be opened (a folder on read-only media) stands for no lock: the server's own
/// lock still holds, and a write there fails on its own.
fn os_lock(path: &Path, mode: Mode, until: Instant) -> Result<Option<std::fs::File>, ()> {
    let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path) else {
        return Ok(None);
    };
    loop {
        let tried = if mode.os_exclusive() { file.try_lock() } else { file.try_lock_shared() };
        match tried {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < until => std::thread::sleep(Duration::from_millis(2)),
            Err(std::fs::TryLockError::WouldBlock) => return Err(()),
            Err(std::fs::TryLockError::Error(_)) => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(address: &str, mode: Mode) -> Wanted {
        Wanted { address: address.into(), label: address.into(), mode, file: None }
    }

    #[test]
    fn readers_share_a_writer_holds_alone_and_a_wait_is_bounded() {
        let locks = Locks::default();
        let soon = || Instant::now() + Duration::from_millis(30);
        let r1 = locks.take(vec![wanted("a", Mode::Shared)], soon()).unwrap();
        let r2 = locks.take(vec![wanted("a", Mode::Shared)], soon()).unwrap();
        assert_eq!(locks.take(vec![wanted("a", Mode::Exclusive)], soon()).unwrap_err(), "a");
        drop((r1, r2));
        let w = locks.take(vec![wanted("a", Mode::Exclusive)], soon()).unwrap();
        assert_eq!(locks.take(vec![wanted("a", Mode::Shared)], soon()).unwrap_err(), "a");
        drop(w);
        assert!(locks.take(vec![wanted("a", Mode::Shared)], soon()).is_ok());
    }

    #[test]
    fn one_that_waits_too_long_holds_none_of_what_it_took() {
        let locks = Locks::default();
        let soon = || Instant::now() + Duration::from_millis(30);
        let b = locks.take(vec![wanted("b", Mode::Exclusive)], soon()).unwrap();
        // `a` is taken before `b`, and let go when `b` can't be.
        assert_eq!(locks.take(vec![wanted("b", Mode::Shared), wanted("a", Mode::Exclusive)], soon()).unwrap_err(), "b");
        assert!(locks.take(vec![wanted("a", Mode::Exclusive)], soon()).is_ok());
        drop(b);
    }

    #[test]
    fn a_folder_held_whole_waits_for_writers_of_its_files_not_readers() {
        use Mode::*;
        let locks = Locks::default();
        let soon = || Instant::now() + Duration::from_millis(30);
        // A writer of one file and a reader of another hold the folder by intent, together.
        let writer = locks.take(vec![wanted("f", IntentExclusive), wanted("f/a", Exclusive)], soon()).unwrap();
        let reader = locks.take(vec![wanted("f", IntentShared), wanted("f/b", Shared)], soon()).unwrap();
        assert!(locks.take(vec![wanted("f", IntentExclusive), wanted("f/c", Exclusive)], soon()).is_ok(), "writers of other files don't wait");
        assert_eq!(locks.take(vec![wanted("f", IntentShared), wanted("f/a", Shared)], soon()).unwrap_err(), "f/a", "a reader of the written file waits");
        // A listing holds the folder shared: it waits for the writer, not the reader.
        assert_eq!(locks.take(vec![wanted("f", Shared)], soon()).unwrap_err(), "f");
        drop(writer);
        let listing = locks.take(vec![wanted("f", Shared)], soon()).unwrap();
        assert_eq!(locks.take(vec![wanted("f", IntentExclusive), wanted("f/b", Exclusive)], soon()).unwrap_err(), "f", "and holds off writers");
        drop((listing, reader));
        // Held shared and by intent to write: held alone.
        let both = locks.take(vec![wanted("f", Shared), wanted("f", IntentExclusive)], soon()).unwrap();
        assert!(locks.take(vec![wanted("f", IntentShared)], soon()).is_err());
        drop(both);
    }

    /// Random sets of resources, taken by many threads at once, each in address order: every one
    /// is taken in the end, none waits forever.
    #[test]
    fn locks_taken_in_address_order_never_deadlock() {
        let locks = Locks::default();
        std::thread::scope(|s| {
            for t in 0..16u64 {
                let locks = &locks;
                s.spawn(move || {
                    let mut seed = t.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
                    for _ in 0..200 {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        let set: Vec<Wanted> = (0..5)
                            .filter(|i| seed >> i & 1 == 1)
                            .map(|i| wanted(&format!("r{}", (i + seed as usize / 64) % 5), [Mode::IntentShared, Mode::IntentExclusive, Mode::Shared, Mode::Exclusive][(seed >> (2 * i + 8) & 3) as usize]))
                            .collect();
                        let held = locks.take(set, Instant::now() + Duration::from_secs(10));
                        assert!(held.is_ok(), "waited ten seconds: {held:?}");
                    }
                });
            }
        });
    }

    #[test]
    fn a_folder_lock_holds_across_servers_through_its_file() {
        let dir = std::env::temp_dir().join(format!("holon-locks-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(LOCK_FILE);
        // Two servers: two tables, one file.
        let (one, two) = (Locks::default(), Locks::default());
        let at = |mode| Wanted { address: "folder".into(), label: "folder".into(), mode, file: Some(file.clone()) };
        let soon = || Instant::now() + Duration::from_millis(40);
        let held = one.take(vec![at(Mode::Exclusive)], soon()).unwrap();
        assert!(two.take(vec![at(Mode::Shared)], soon()).is_err(), "the other server waits for it");
        drop(held);
        let r1 = one.take(vec![at(Mode::Shared)], soon()).unwrap();
        assert!(two.take(vec![at(Mode::Shared)], soon()).is_ok(), "readers share it across servers");
        drop(r1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
