//! Key claims (`docs/L4s/key-claims-L4.md`): a key a run read before its first outbound request
//! is held for that run until it commits, is discarded or parks, so two runs that would call out
//! over one key do so one at a time, and the second reads what the first wrote before it can
//! call anyone. A claim is derived from the run's reads and the grant, never declared, and taken
//! only when a run is about to send something: a run that sends nothing claims nothing.
//!
//! A collection's keys are in a table beside the deployment's state ([`StateCell`]): two
//! deployments never share one. A shared resource's (a row or table of a database, a file or
//! folder: `docs/L4s/databases-L4.md` §7.2) are in the server's table, `Folders::claims`, by
//! the resource's real address, so every deployment bound to it sees them. A run holding keys
//! never waits for another's (it releases, waits, and runs again), so there is no deadlock;
//! every wait is bounded by the run's `claim_wait`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use holon_core::Value;
use holon_host::{Claim, State};

use crate::{collections, databases, folders, SharedStore};

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

/// What a claim names: one item of a collection, or a whole collection (a listing was read); a
/// row of a database's table, by its primary key, or the whole table (any other read); a file of
/// a folder, or the whole folder (a listing). Never a state cell: a cell is one per deployment,
/// so claiming it would serialize every run that calls out, which is what runs side by side was
/// built to end (the key-claims L4, §2).
///
/// A shared resource's key holds its real address (`at`: the database's file, the folder's or
/// file's path, in lower case), which is what two keys are compared by, and the name the flow
/// knows it by, for its log.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ClaimKey {
    Item { collection: String, key: String },
    Collection(String),
    /// `key`: the primary key's values, as a JSON list, numbers whole and text in lower case, so
    /// `1` and `"1"` are one row (two keys that differ only so share a claim: that only waits).
    Row { at: String, name: String, table: String, key: String },
    Table { at: String, name: String, table: String },
    /// A row an insert adds without naming its primary key: never claimed, only written, so a
    /// commit waits for a claim on its table but not on the table's rows.
    NewRow { at: String, name: String, table: String },
    File { at: String, name: String, file: String },
    Folder { at: String, name: String },
}

impl ClaimKey {
    /// Whether two claims cover the same thing: the same key, or a whole collection and one of
    /// its items; a row and its table; a file and a folder it is in.
    fn conflicts(&self, other: &ClaimKey) -> bool {
        use ClaimKey::*;
        let inside = |file: &str, folder: &str| file.strip_prefix(folder).is_some_and(|rest| rest.starts_with(['/', '\\']));
        match (self, other) {
            (Collection(a), Item { collection, .. }) | (Item { collection, .. }, Collection(a)) => a == collection,
            (Row { at: a, table: t, key: k, .. }, Row { at: b, table: u, key: l, .. }) => a == b && t == u && k == l,
            (Row { at: a, table: t, .. } | Table { at: a, table: t, .. }, Row { at: b, table: u, .. } | Table { at: b, table: u, .. }) => a == b && t == u,
            (NewRow { at: a, table: t, .. }, Table { at: b, table: u, .. }) | (Table { at: b, table: u, .. }, NewRow { at: a, table: t, .. }) => a == b && t == u,
            (NewRow { .. }, _) | (_, NewRow { .. }) => false,
            (File { at: a, .. }, File { at: b, .. }) => a == b,
            (Folder { at: d, .. }, File { at: f, .. }) | (File { at: f, .. }, Folder { at: d, .. }) => inside(f, d),
            (Folder { at: a, .. }, Folder { at: b, .. }) => a == b || inside(a, b) || inside(b, a),
            _ => self == other,
        }
    }

    /// Whether it names a resource other deployments may share: claimed in the server's table.
    pub(crate) fn is_shared(&self) -> bool {
        !matches!(self, ClaimKey::Item { .. } | ClaimKey::Collection(_))
    }
}

impl fmt::Display for ClaimKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClaimKey::Item { collection, key } => write!(f, "{collection}[{key}]"),
            ClaimKey::Collection(name) => write!(f, "{name}[*]"),
            ClaimKey::Row { name, table, key, .. } => write!(f, "{name}/{table}[{}]", key.strip_prefix('[').and_then(|k| k.strip_suffix(']')).unwrap_or(key)),
            ClaimKey::Table { name, table, .. } => write!(f, "{name}/{table}[*]"),
            ClaimKey::NewRow { name, table, .. } => write!(f, "{name}/{table}[new]"),
            ClaimKey::File { name, file, .. } => write!(f, "{name}/{file}"),
            ClaimKey::Folder { name, .. } => write!(f, "{name}[*]"),
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

/// What a run reads of the resources other deployments share, to claim their keys: the folders
/// and databases it sees, and what it may write in them (the only keys it claims), with readers
/// to ask its reads again as they now are.
pub(crate) struct SharedReads {
    pub(crate) folders: std::sync::Arc<folders::Scoped>,
    /// The tables it may write, as `shop/orders`.
    pub(crate) tables_written: BTreeSet<String>,
    /// The folders it may write, by name.
    pub(crate) folders_written: BTreeSet<String>,
    pub(crate) files: folders::Readable,
    pub(crate) databases: databases::Readable,
}

impl SharedReads {
    /// The key of a read of a shared resource it may write: its row or table, file or folder.
    fn keys_of(&self, request: &Value) -> Vec<ClaimKey> {
        let Value::Record(q) = request else { return Vec::new() };
        if q.contains_key("database") {
            return databases::claims_of_read(&self.folders, request, &self.tables_written);
        }
        if q.contains_key("folder") {
            return self.folders.claim_of_read(request, &self.folders_written).into_iter().collect();
        }
        Vec::new()
    }

    /// Whether a read of a shared resource answers now as it did.
    fn still_as_read(&mut self, read: &collections::Read) -> bool {
        let Value::Record(q) = &read.request else { return true };
        if q.contains_key("database") {
            return self.databases.answer(&read.request).is_ok_and(|now| now == read.answer);
        }
        self.files.answer(&read.request, false).is_ok_and(|(now, _)| now == read.answer)
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
    /// Its shared resources, if it has any (a run with no folders on the server has none).
    shared: Option<SharedReads>,
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
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        cell: std::sync::Arc<StateCell>,
        store: SharedStore,
        writable: BTreeSet<String>,
        readable: BTreeSet<String>,
        shared: Option<SharedReads>,
        run: String,
        wait: Duration,
        records: Vec<ClaimRecord>,
    ) -> Claiming {
        Claiming { cell, store, writable, readable, shared, run, deadline: Instant::now() + wait, held: Vec::new(), records }
    }

    /// The keys `reads` touch that this run may write.
    fn keys_of(&self, reads: &[collections::Read]) -> Vec<ClaimKey> {
        let mut keys: BTreeSet<ClaimKey> = BTreeSet::new();
        for r in reads {
            let Value::Record(req) = &r.request else { continue };
            if let Some(shared) = &self.shared {
                keys.extend(shared.keys_of(&r.request));
            }
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

    /// Whether what the attempt read under `keys` is still as it found it: in the live state, or
    /// the database or folder as it now is.
    fn still_as_read(&mut self, keys: &[ClaimKey], reads: &[collections::Read]) -> bool {
        let live = self.cell.state.lock().unwrap();
        let now = collections::Readable { store: self.store.clone(), roots: collections::roots(&live), granted: self.readable.clone() };
        drop(live);
        let under: Vec<&collections::Read> = reads.iter().filter(|r| self.keys_of(std::slice::from_ref(r)).iter().any(|k| keys.iter().any(|c| c.conflicts(k)))).collect();
        under.into_iter().all(|r| match self.shared.as_mut() {
            Some(shared) if matches!(&r.request, Value::Record(q) if q.contains_key("database") || q.contains_key("folder")) => shared.still_as_read(r),
            _ => now.answer(&r.request) == r.answer,
        })
    }

    /// The table a key is claimed in: the deployment's, or the server's for a shared resource.
    fn table_of<'t>(cell: &'t StateCell, shared: Option<&'t folders::Scoped>, key: &ClaimKey) -> &'t Claims {
        match shared {
            Some(f) if key.is_shared() => &f.all.claims,
            _ => &cell.claims,
        }
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
        let cell = std::sync::Arc::clone(&self.cell);
        let folders = self.shared.as_ref().map(|s| std::sync::Arc::clone(&s.folders));
        // The deployment's keys, then the shared ones: the first group waits for its keys only if
        // the run holds none yet; any other it can't take at once, it lets everything go for.
        let (shared, own): (Vec<ClaimKey>, Vec<ClaimKey>) = new.iter().cloned().partition(|k| k.is_shared() && folders.is_some());
        let first_waits = self.held.is_empty();
        for group in [own, shared].into_iter().filter(|g| !g.is_empty()) {
            let table = Self::table_of(&cell, folders.as_deref(), &group[0]);
            let taken = if first_waits && self.held.is_empty() { table.acquire(&group, &self.run, self.deadline).map(|_| ()) } else { table.try_acquire(&group, &self.run) };
            match taken {
                Ok(()) => self.held.extend(group),
                Err(key) if self.held.is_empty() => {
                    self.records.push(ClaimRecord::new(key.clone(), started.elapsed(), false));
                    return Claim::Busy { key: key.to_string() };
                }
                Err(key) => {
                    // Holding some and needing one another run holds: let go, wait for it, run again.
                    self.release_all();
                    let free = table.wait_free(std::slice::from_ref(&key), &self.run, self.deadline);
                    self.records.push(ClaimRecord::new(key.clone(), started.elapsed(), free));
                    return if free { Claim::Overtaken } else { Claim::Busy { key: key.to_string() } };
                }
            }
        }
        let waited = started.elapsed();
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
            let folders = self.shared.as_ref().map(|s| std::sync::Arc::clone(&s.folders));
            let (shared, own): (Vec<ClaimKey>, Vec<ClaimKey>) = self.held.iter().cloned().partition(|k| k.is_shared() && folders.is_some());
            self.cell.claims.release(&own, &self.run);
            if let Some(f) = &folders {
                f.all.claims.release(&shared, &self.run);
            }
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
/// claim. A collection's, in its deployment's table.
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

/// The keys of the shared resources a run's performed effects write: its databases' rows (or
/// tables) and its folders' files, in the server's table.
pub(crate) fn shared_written_keys(folders: &folders::Scoped, performed: &[(holon_core::Capability, Value)]) -> Vec<ClaimKey> {
    let mut keys = databases::claims_of_writes(folders, performed);
    keys.extend(folders.claims_of_writes(performed));
    keys
}

/// Every shared resource a deployment may read or write, whole: what its last try holds for its
/// whole run, so no run of another deployment calls out over what it may touch meanwhile.
pub(crate) fn whole_of(folders: &folders::Scoped, permit: &crate::permissions::Permit) -> Vec<ClaimKey> {
    let mut keys = databases::tables_of(folders, permit);
    keys.extend(folders.whole_claims(&permit.folders_read(), &permit.folders_written()));
    keys
}

/// Claims a run holds until it is dropped: the last try's, on everything shared it may touch.
pub(crate) struct Hold<'c> {
    claims: &'c Claims,
    keys: Vec<ClaimKey>,
    run: String,
}

impl<'c> Hold<'c> {
    /// Hold `keys` for `run` once no other run holds any, waiting until `deadline`: none if one
    /// was still held then.
    pub(crate) fn take(claims: &'c Claims, keys: Vec<ClaimKey>, run: String, deadline: Instant) -> Result<Hold<'c>, ClaimKey> {
        claims.acquire(&keys, &run, deadline)?;
        Ok(Hold { claims, keys, run })
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        self.claims.release(&self.keys, &self.run);
    }
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

    /// A row conflicts with itself and its table, never another row; a new row only with its
    /// table; a file with itself and a folder it is in; a collection's keys with no shared
    /// resource's.
    #[test]
    fn rows_and_files_conflict_with_their_tables_and_folders() {
        let row = |t: &str, k: &str| ClaimKey::Row { at: "/db".into(), name: "shop".into(), table: t.into(), key: k.into() };
        let table = |t: &str| ClaimKey::Table { at: "/db".into(), name: "shop".into(), table: t.into() };
        let new_row = ClaimKey::NewRow { at: "/db".into(), name: "shop".into(), table: "orders".into() };
        assert!(row("orders", "[1]").conflicts(&row("orders", "[1]")));
        assert!(!row("orders", "[1]").conflicts(&row("orders", "[2]")));
        assert!(row("orders", "[1]").conflicts(&table("orders")) && table("orders").conflicts(&row("orders", "[1]")));
        assert!(!row("orders", "[1]").conflicts(&table("refunds")));
        let elsewhere = ClaimKey::Row { at: "/other".into(), name: "shop".into(), table: "orders".into(), key: "[1]".into() };
        assert!(!row("orders", "[1]").conflicts(&elsewhere), "another database");
        let renamed = ClaimKey::Row { at: "/db".into(), name: "store".into(), table: "orders".into(), key: "[1]".into() };
        assert!(row("orders", "[1]").conflicts(&renamed), "one database under two names is one");
        assert!(new_row.conflicts(&table("orders")) && !new_row.conflicts(&row("orders", "[1]")) && !new_row.conflicts(&new_row.clone()));
        let file = |at: &str| ClaimKey::File { at: at.into(), name: "inbox".into(), file: "a".into() };
        let folder = |at: &str| ClaimKey::Folder { at: at.into(), name: "inbox".into() };
        assert!(file("/srv/in/a.csv").conflicts(&file("/srv/in/a.csv")) && !file("/srv/in/a.csv").conflicts(&file("/srv/in/b.csv")));
        assert!(folder("/srv/in").conflicts(&file("/srv/in/a.csv")) && file("c:\\srv\\in\\a.csv").conflicts(&folder("c:\\srv\\in")));
        assert!(!folder("/srv/in").conflicts(&file("/srv/inbox/a.csv")), "a folder whose name starts the same is another");
        assert!(!item("orders", "1").conflicts(&row("orders", "[1]")));
        assert_eq!(row("orders", "[1]").to_string(), "shop/orders[1]");
        assert_eq!(table("orders").to_string(), "shop/orders[*]");
    }

    /// A run waiting for a key is woken when it is let go, not when its deadline passes: with a
    /// deadline of ten seconds and the key released after fifty milliseconds, it holds the key
    /// well within a second. (How long it waited is not asserted: on a loaded machine the waiter
    /// may not have started by the time the key is released.)
    #[test]
    fn a_waiting_run_is_woken_by_the_release() {
        let claims = std::sync::Arc::new(Claims::default());
        claims.acquire(&[item("c", "k")], "a", Instant::now()).unwrap();
        let waiter = std::sync::Arc::clone(&claims);
        let started = Instant::now();
        let t = std::thread::spawn(move || waiter.acquire(&[item("c", "k")], "b", Instant::now() + Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(50));
        claims.release(&[item("c", "k")], "a");
        let waited = t.join().unwrap().unwrap();
        assert!(started.elapsed() < Duration::from_secs(2), "woken by the release, not the deadline: {waited:?}");
        assert_eq!(claims.held(), vec![item("c", "k")], "held by b now");
    }
}
