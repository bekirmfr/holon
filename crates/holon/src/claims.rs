//! Key claims (`docs/L4s/key-claims-L4.md`): a key a run read before its first outbound request
//! is held for that run until it commits, is discarded or parks, so two runs that would call out
//! over one key do so one at a time, and the second reads what the first wrote before it can
//! call anyone. A claim is derived from the run's reads and the grant, never declared, and taken
//! only when a run is about to send something: a run that sends nothing claims nothing.
//!
//! The table lives beside the deployment's state ([`StateCell`]): two deployments never share
//! one. A run holding keys never waits for another's (it releases, waits, and runs again), so
//! there is no deadlock; every wait is bounded by the run's `claim_wait`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use holon_core::Value;
use holon_host::{Claim, State};

use crate::{collections, SharedStore};

/// One deployment's keyed state and the claims on its keys, shared between its runs.
pub(crate) struct StateCell {
    pub(crate) state: Mutex<State>,
    pub(crate) claims: Claims,
}

impl StateCell {
    pub(crate) fn new(state: State) -> StateCell {
        StateCell { state: Mutex::new(state), claims: Claims::default() }
    }
}

/// What a claim names: one item of a collection, or a whole collection (a listing was read).
/// Never a state cell: a cell is one per deployment, so claiming it would serialize every run
/// that calls out, which is what runs side by side was built to end (the L4, §2).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ClaimKey {
    Item { collection: String, key: String },
    Collection(String),
}

impl ClaimKey {
    /// Whether two claims cover the same thing: the same key, or a whole collection and one of
    /// its items.
    fn conflicts(&self, other: &ClaimKey) -> bool {
        match (self, other) {
            (ClaimKey::Collection(a), ClaimKey::Item { collection, .. }) | (ClaimKey::Item { collection, .. }, ClaimKey::Collection(a)) => a == collection,
            _ => self == other,
        }
    }
}

impl fmt::Display for ClaimKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClaimKey::Item { collection, key } => write!(f, "{collection}[{key}]"),
            ClaimKey::Collection(name) => write!(f, "{name}[*]"),
        }
    }
}

/// The claims on one deployment's keys: key → the run (id) holding it.
#[derive(Default)]
pub(crate) struct Claims {
    table: Mutex<BTreeMap<ClaimKey, String>>,
    changed: Condvar,
}

impl Claims {
    /// The first of `keys` another run holds, if any.
    fn blocker<'k>(table: &BTreeMap<ClaimKey, String>, keys: &'k [ClaimKey], run: &str) -> Option<&'k ClaimKey> {
        keys.iter().find(|k| table.iter().any(|(held, by)| by != run && held.conflicts(k)))
    }

    /// Hold every one of `keys` for `run`, all at once: waits, until `deadline`, while another run
    /// holds any of them. How long it waited, or the key still held when the deadline passed
    /// (then nothing was taken).
    pub(crate) fn acquire(&self, keys: &[ClaimKey], run: &str, deadline: Instant) -> Result<Duration, ClaimKey> {
        let started = Instant::now();
        let mut table = self.table.lock().unwrap();
        loop {
            match Self::blocker(&table, keys, run) {
                None => {
                    for k in keys {
                        table.insert(k.clone(), run.to_string());
                    }
                    return Ok(started.elapsed());
                }
                Some(k) => {
                    let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
                        return Err(k.clone());
                    };
                    table = self.changed.wait_timeout(table, left).unwrap().0;
                }
            }
        }
    }

    /// Hold `keys` for `run` only if none is held by another run: the key that is, otherwise.
    pub(crate) fn try_acquire(&self, keys: &[ClaimKey], run: &str) -> Result<(), ClaimKey> {
        let mut table = self.table.lock().unwrap();
        if let Some(k) = Self::blocker(&table, keys, run) {
            return Err(k.clone());
        }
        for k in keys {
            table.insert(k.clone(), run.to_string());
        }
        Ok(())
    }

    /// Let go of `keys` held by `run`.
    pub(crate) fn release(&self, keys: &[ClaimKey], run: &str) {
        let mut table = self.table.lock().unwrap();
        table.retain(|k, by| !(by == run && keys.contains(k)));
        drop(table);
        self.changed.notify_all();
    }

    /// Wait, until `deadline`, while another run holds any of `keys`: whether they came free.
    pub(crate) fn wait_free(&self, keys: &[ClaimKey], run: &str, deadline: Instant) -> bool {
        let mut table = self.table.lock().unwrap();
        while Self::blocker(&table, keys, run).is_some() {
            let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
                return false;
            };
            table = self.changed.wait_timeout(table, left).unwrap().0;
        }
        true
    }

    /// Wait, until `deadline`, while any run holds any key: whether the table came empty.
    pub(crate) fn wait_until_empty(&self, deadline: Instant) -> bool {
        let mut table = self.table.lock().unwrap();
        while !table.is_empty() {
            let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
                return false;
            };
            table = self.changed.wait_timeout(table, left).unwrap().0;
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn held(&self) -> Vec<ClaimKey> {
        self.table.lock().unwrap().keys().cloned().collect()
    }
}

/// One claim a run took (or tried to), for its log entry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClaimRecord {
    pub(crate) key: ClaimKey,
    pub(crate) waited: Duration,
    /// The run found what it read changed once it held the key (or let go to wait for it), and ran
    /// again from the new state, having sent nothing on this attempt.
    pub(crate) overtaken: bool,
    /// When it was taken (or given up on), and how long it was held: none for a key never held.
    pub(crate) taken: Instant,
    pub(crate) held: Option<Duration>,
}

impl ClaimRecord {
    fn new(key: ClaimKey, waited: Duration, overtaken: bool) -> ClaimRecord {
        ClaimRecord { key, waited, overtaken, taken: Instant::now(), held: None }
    }
}

/// One attempt's claiming: what it holds, and what it needs to decide the next claim.
pub(crate) struct Claiming {
    cell: std::sync::Arc<StateCell>,
    store: SharedStore,
    /// The collections the grant lets the flow write: the only ones whose keys it claims.
    writable: BTreeSet<String>,
    /// The collections it may read (to re-ask its reads against the live state).
    readable: BTreeSet<String>,
    run: String,
    deadline: Instant,
    held: Vec<ClaimKey>,
    pub(crate) records: Vec<ClaimRecord>,
}

impl fmt::Debug for Claiming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Claiming").field("run", &self.run).field("held", &self.held).finish()
    }
}

impl Claiming {
        pub(crate) fn new(
        cell: std::sync::Arc<StateCell>,
        store: SharedStore,
        writable: BTreeSet<String>,
        readable: BTreeSet<String>,
        run: String,
        wait: Duration,
        records: Vec<ClaimRecord>,
    ) -> Claiming {
        Claiming { cell, store, writable, readable, run, deadline: Instant::now() + wait, held: Vec::new(), records }
    }

    /// The keys `reads` touch that this run may write.
    fn keys_of(&self, reads: &[collections::Read]) -> Vec<ClaimKey> {
        let mut keys: BTreeSet<ClaimKey> = BTreeSet::new();
        for r in reads {
            let Value::Record(req) = &r.request else { continue };
            let Some(Value::Text(name)) = req.get("collection") else { continue };
            if !self.writable.contains(name) {
                continue;
            }
            match req.get("get") {
                Some(Value::Text(key)) => keys.insert(ClaimKey::Item { collection: name.clone(), key: key.clone() }),
                _ => keys.insert(ClaimKey::Collection(name.clone())),
            };
        }
        keys.into_iter().collect()
    }

    /// Whether what the attempt read under `keys` is still as it found it, in the live state.
    fn still_as_read(&self, keys: &[ClaimKey], reads: &[collections::Read]) -> bool {
        let live = self.cell.state.lock().unwrap();
        let now = collections::Readable { store: self.store.clone(), roots: collections::roots(&live), granted: self.readable.clone() };
        drop(live);
        reads.iter().filter(|r| self.keys_of(std::slice::from_ref(r)).iter().any(|k| keys.iter().any(|c| c.conflicts(k)))).all(|r| now.answer(&r.request) == r.answer)
    }

    /// Before the attempt's requests go out: hold every key it has read that it may write, and
    /// check those reads against the live state. Nothing new to hold is `Held` at once.
    pub(crate) fn claim(&mut self, reads: &[collections::Read]) -> Claim {
        let wanted = self.keys_of(reads);
        let new: Vec<ClaimKey> = wanted.into_iter().filter(|k| !self.held.contains(k)).collect();
        if new.is_empty() {
            return Claim::Held;
        }
        let started = Instant::now();
        if self.held.is_empty() {
            match self.cell.claims.acquire(&new, &self.run, self.deadline) {
                Ok(_) => {}
                Err(key) => {
                    self.records.push(ClaimRecord::new(key.clone(), started.elapsed(), false));
                    return Claim::Busy { key: key.to_string() };
                }
            }
        } else if let Err(key) = self.cell.claims.try_acquire(&new, &self.run) {
            // Holding some and needing one another run holds: let go, wait for it, run again.
            self.release_all();
            let free = self.cell.claims.wait_free(std::slice::from_ref(&key), &self.run, self.deadline);
            self.records.push(ClaimRecord::new(key.clone(), started.elapsed(), free));
            return if free { Claim::Overtaken } else { Claim::Busy { key: key.to_string() } };
        }
        let waited = started.elapsed();
        self.held.extend(new.iter().cloned());
        if self.still_as_read(&new, reads) {
            self.records.extend(new.into_iter().map(|key| ClaimRecord::new(key, waited, false)));
            Claim::Held
        } else {
            self.records.extend(new.into_iter().map(|key| ClaimRecord::new(key, waited, true)));
            self.release_all();
            Claim::Overtaken
        }
    }

    fn release_all(&mut self) {
        if !self.held.is_empty() {
            self.cell.claims.release(&self.held, &self.run);
            for r in self.records.iter_mut().filter(|r| r.held.is_none() && self.held.contains(&r.key)) {
                r.held = Some(r.taken.elapsed());
            }
            self.held.clear();
        }
    }

    /// Let go of everything and hand back the records, each held key's time now known.
    pub(crate) fn finish(mut self) -> Vec<ClaimRecord> {
        self.release_all();
        std::mem::take(&mut self.records)
    }
}

impl Drop for Claiming {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// The keys a run's performed effects write: what its commit must not do under another run's
/// claim.
pub(crate) fn written_keys(performed: &[(holon_core::Capability, Value)]) -> Vec<ClaimKey> {
    use holon_core::Capability;
    performed
        .iter()
        .filter_map(|(cap, payload)| match (cap, payload) {
            (Capability::CollectionWrite(name), Value::Record(item)) => match item.get("key") {
                Some(Value::Text(key)) => Some(ClaimKey::Item { collection: name.clone(), key: key.clone() }),
                _ => None,
            },
            (Capability::CollectionWrite(name), Value::Text(key)) => Some(ClaimKey::Item { collection: name.clone(), key: key.clone() }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(c: &str, k: &str) -> ClaimKey {
        ClaimKey::Item { collection: c.into(), key: k.into() }
    }

    #[test]
    fn a_key_is_held_by_one_run_at_a_time_and_a_listing_covers_its_items() {
        let claims = Claims::default();
        let soon = || Instant::now() + Duration::from_millis(30);
        assert!(claims.acquire(&[item("c", "k")], "a", soon()).is_ok());
        assert_eq!(claims.acquire(&[item("c", "k")], "b", soon()), Err(item("c", "k")));
        assert!(claims.acquire(&[item("c", "other")], "b", soon()).is_ok(), "another key never waits");
        assert_eq!(claims.try_acquire(&[ClaimKey::Collection("c".into())], "x"), Err(ClaimKey::Collection("c".into())), "a listing waits for any item");
        assert!(claims.acquire(&[item("c", "k")], "a", soon()).is_ok(), "its own claim again is free");
        claims.release(&[item("c", "k")], "a");
        assert!(claims.acquire(&[item("c", "k")], "b", soon()).is_ok());
        assert!(!claims.wait_until_empty(soon()));
        claims.release(&[item("c", "k"), item("c", "other")], "b");
        assert!(claims.wait_until_empty(soon()));
    }

    #[test]
    fn a_waiting_run_is_woken_by_the_release() {
        let claims = std::sync::Arc::new(Claims::default());
        claims.acquire(&[item("c", "k")], "a", Instant::now()).unwrap();
        let waiter = std::sync::Arc::clone(&claims);
        let t = std::thread::spawn(move || waiter.acquire(&[item("c", "k")], "b", Instant::now() + Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        claims.release(&[item("c", "k")], "a");
        let waited = t.join().unwrap().unwrap();
        assert!(waited >= Duration::from_millis(40) && waited < Duration::from_secs(2), "{waited:?}");
    }
}
