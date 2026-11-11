//! Locks no panic poisons. A run that panics is a run that failed, and nothing else
//! (docs/engineering/themes/resilience.md, G3); but a panic while it held one of std's locks
//! poisoned that lock, and every `lock().unwrap()` after it panicked too: one panic holding the
//! deployments table, a log, a vault or a queue failed every run and request that touched it,
//! until the server was started again. These are std's locks, taken back from a panic as they are
//! (`PoisonError::into_inner`): what the panicking thread left half changed stays as it left it,
//! as with a lock that never poisons. Their `lock`, `read`, `write` and waits answer a `Result`
//! whose error never is ([`Never`]), so the code that takes them reads as it did.
//!
//! Every lock in this crate is one of these (a test says so), but logs.rs's, which is left to
//! the work that owns that file.

use std::sync::{MutexGuard, PoisonError, RwLockReadGuard, RwLockWriteGuard, WaitTimeoutResult};
use std::time::Duration;

/// The error a lock here never answers: there is none of it. Its `into_inner` is there for code
/// written for std's locks (`.lock().unwrap_or_else(|e| e.into_inner())`), and is never called.
#[derive(Debug)]
pub(crate) enum Never {}

impl Never {
    pub(crate) fn into_inner<T>(self) -> T {
        match self {}
    }
}

/// A mutual-exclusion lock no panic poisons.
#[derive(Default)]
pub(crate) struct Mutex<T: ?Sized>(std::sync::Mutex<T>);

impl<T> Mutex<T> {
    pub(crate) const fn new(value: T) -> Mutex<T> {
        Mutex(std::sync::Mutex::new(value))
    }

    pub(crate) fn into_inner(self) -> Result<T, Never> {
        Ok(self.0.into_inner().unwrap_or_else(PoisonError::into_inner))
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Wait for the lock and take it, as a panic that held it last left it.
    pub(crate) fn lock(&self) -> Result<MutexGuard<'_, T>, Never> {
        Ok(self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Take the lock if no one holds it: none if someone does.
    pub(crate) fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        match self.0.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A readers-writer lock no panic poisons.
#[derive(Default)]
pub(crate) struct RwLock<T: ?Sized>(std::sync::RwLock<T>);

impl<T> RwLock<T> {
    pub(crate) const fn new(value: T) -> RwLock<T> {
        RwLock(std::sync::RwLock::new(value))
    }
}

impl<T: ?Sized> RwLock<T> {
    pub(crate) fn read(&self) -> Result<RwLockReadGuard<'_, T>, Never> {
        Ok(self.0.read().unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn write(&self) -> Result<RwLockWriteGuard<'_, T>, Never> {
        Ok(self.0.write().unwrap_or_else(PoisonError::into_inner))
    }
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A condition variable whose waits take back a [`Mutex`] a panic left, as its `lock` does.
#[derive(Default, Debug)]
pub(crate) struct Condvar(std::sync::Condvar);

impl Condvar {
    pub(crate) const fn new() -> Condvar {
        Condvar(std::sync::Condvar::new())
    }

    pub(crate) fn notify_one(&self) {
        self.0.notify_one();
    }

    pub(crate) fn notify_all(&self) {
        self.0.notify_all();
    }

    pub(crate) fn wait_timeout<'a, T>(&self, guard: MutexGuard<'a, T>, dur: Duration) -> Result<(MutexGuard<'a, T>, WaitTimeoutResult), Never> {
        Ok(self.0.wait_timeout(guard, dur).unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn wait_while<'a, T, F: FnMut(&mut T) -> bool>(&self, guard: MutexGuard<'a, T>, condition: F) -> Result<MutexGuard<'a, T>, Never> {
        Ok(self.0.wait_while(guard, condition).unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn wait_timeout_while<'a, T, F: FnMut(&mut T) -> bool>(
        &self,
        guard: MutexGuard<'a, T>,
        dur: Duration,
        condition: F,
    ) -> Result<(MutexGuard<'a, T>, WaitTimeoutResult), Never> {
        Ok(self.0.wait_timeout_while(guard, dur, condition).unwrap_or_else(PoisonError::into_inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Guards: each lock here, taken after a thread panicked holding it, is taken (as the panic
    /// left what it guards), and a wait on a condition variable woken after such a panic returns.
    /// Why: with std's locks a panic holding one poisoned it, and every `lock().unwrap()` after
    /// panicked too, so one failed run failed every run after it that touched what it held.
    /// If it fails: one panic stops a deployment, a log or the whole table for good, until restart.
    #[test]
    fn a_lock_a_panic_held_is_taken_again_as_the_panic_left_it() {
        let held = Arc::new(Mutex::new(vec![1]));
        let shared = Arc::new(RwLock::new(1));
        let (h, s) = (Arc::clone(&held), Arc::clone(&shared));
        let panicked = std::thread::spawn(move || {
            let mut list = h.lock().unwrap();
            list.push(2);
            let _written = s.write().unwrap();
            panic!("a run panics holding both");
        });
        assert!(panicked.join().is_err());
        assert_eq!(*held.lock().unwrap(), vec![1, 2], "taken, as the panic left it");
        assert_eq!(*shared.read().unwrap(), 1);
        *shared.write().unwrap() = 2;
        assert_eq!(Arc::try_unwrap(held).unwrap().into_inner().unwrap(), vec![1, 2]);

        // A waiter, woken by a thread that then panics holding the lock, goes on.
        let pair = Arc::new((Mutex::new(false), Condvar::new()));
        let waker = Arc::clone(&pair);
        let woke = std::thread::spawn(move || {
            let (lock, ready) = &*waker;
            std::thread::sleep(Duration::from_millis(50));
            let mut flag = lock.lock().unwrap();
            *flag = true;
            ready.notify_all();
            panic!("it panics with the lock held");
        });
        let (lock, ready) = &*pair;
        let flag = ready.wait_while(lock.lock().unwrap(), |f| !*f).unwrap();
        assert!(*flag);
        drop(flag);
        assert!(woke.join().is_err());
        let (flag, _) = ready.wait_timeout(lock.lock().unwrap(), Duration::from_millis(1)).unwrap();
        assert!(*flag, "and the lock is taken again after");
        assert!(lock.try_lock().is_none(), "held");
        drop(flag);
        assert!(lock.try_lock().is_some(), "and tried");
    }

    /// Guards: every lock this crate takes is one of these: no `std::sync::Mutex`, `RwLock` or
    /// `Condvar` in its source but here, and in logs.rs, left to the work that owns that file.
    /// Why: one std lock is enough for a panic to stop what it guards until the server restarts,
    /// and nothing else would say a new one came in.
    /// If it fails: a std lock is back; take it from `crate::sync` instead.
    #[test]
    fn every_lock_in_the_server_is_one_no_panic_poisons() {
        fn sources(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    sources(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        let mut files = Vec::new();
        sources(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        // A lock named whole (its guards are std's, which these give).
        let named = ["Mutex", "RwLock", "Condvar"];
        let std_lock = |line: &str| {
            line.split("std::sync::").skip(1).any(|rest| named.iter().any(|n| rest.strip_prefix(n).is_some_and(|after| !after.starts_with(|c: char| c.is_alphanumeric()))))
                || line.split_once("std::sync::{").is_some_and(|(_, list)| list.split(['}', ',']).any(|i| named.contains(&i.trim())))
        };
        let found: Vec<String> = files
            .iter()
            .filter(|(path, _)| !path.ends_with("sync.rs") && !path.ends_with("logs.rs"))
            .flat_map(|(path, text)| text.lines().enumerate().filter(|(_, l)| std_lock(l)).map(move |(i, l)| format!("{path}:{}: {}", i + 1, l.trim())))
            .collect();
        assert!(found.is_empty(), "std locks, which a panic poisons:
{}", found.join("
"));
        assert!(files.len() > 100, "the sweep read the source: {}", files.len());
    }
}
