//! Sweeping the content store's values that nothing kept names (docs/L4s/history-L4.md §7):
//! the server's own, every `[storage] sweep_every` (storage.rs; docs/L4s/resilience-audit-L4.md,
//! R6: a write leaves values no kept run names once retention cuts it, and a store left to an
//! admin's sweep fills its disk), and an admin's, `holon
//! store sweep` or `POST /admin/store/sweep`, at any time.
//!
//! A run keeps the state it left as a snapshot (restore.rs): a record naming each cell's value by
//! hash, with the values it changed put beside it. Once retention cuts the run from its log,
//! nothing may name that snapshot or those values any more; a sweep removes them. What stays is
//! whatever something kept names, followed through the values themselves:
//!
//! - each deployment's live state, its sessions' states, its versions (their documents and the
//!   states they were deployed with), its events, its waiting runs, and every line its log's file
//!   holds (up to twice what it keeps until the file is cut back: a keep raised later reads them
//!   back, so their states stay);
//! - every value one of those names, and every value that one names in turn: a snapshot its cells'
//!   values, a cell holding a collection its trie's root, a node its children and its item.
//!
//! What names a value is read conservatively: any 64-digit hex in a line, a record or a value
//! counts as a value's hash, so a reference no one taught the sweep (a new field, an old one) still
//! keeps what it names; at worst a text that only looks like a hash keeps a value it needn't. A
//! value put in the last hour is spared whoever names it, since a run that put it may not have
//! committed yet (no run lasts longer than 15 minutes, the run budget's ceiling). A value that
//! can't be read stops the sweep: what it names can't be told. Only values are swept, never the
//! units (code) beside them, and only an on-disk store's.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use holon_core::{parse_hash, Hash, Value};
use holon_host::Store;

use crate::folders::size_text;
use crate::{err, to_hex, AppState, Deployment, Response, SharedStore};

/// How new a value a sweep spares, whoever names it: four times the longest a run may take.
pub(crate) const SPARES_MS: u64 = 60 * 60 * 1000;
/// How the server's own sweep paces its removals: a pause of `PAUSE` after every `PACE` values
/// removed, about 2,500 a second at most. Removed as fast as they can be, 134,000 values took
/// 27 seconds and a deployment's listing fell from about 400 requests a second to 123 meanwhile
/// (each removal holds the store's lock, and ext4 journals each unlink); paced, a sweep takes
/// longer and holds up nothing. A store writing 50 states a second leaves about 400 values a
/// second to sweep: paced, a sweep still keeps up six times over.
const PACE: usize = 256;
const PAUSE: Duration = Duration::from_millis(100);
/// How many puts `Puts` notes before it forgets the ones a sweep no longer spares.
const FORGET_AT: usize = 4096;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// When the file at `path` was last changed, in milliseconds since 1970.
fn changed_ms(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

/// The values put lately, so a sweep spares them: a run in flight names them nowhere yet, and a
/// value put again (one already there) keeps the time it was first written.
#[derive(Default)]
pub(crate) struct Puts {
    /// Where the store keeps its values on disk; none for one in memory, which isn't swept.
    dir: Option<PathBuf>,
    /// Each value put lately, with when (ms since 1970).
    recent: Mutex<HashMap<Hash, u64>>,
    /// How many puts there have been: none since a sweep that spared nothing, and the next has
    /// nothing new to find (but what a deployment deleted since let go, left to the sweep after
    /// the next put, or an admin's).
    count: AtomicU64,
    /// Held by a sweep: the server's and an admin's take turns.
    sweeping: Mutex<()>,
    /// The store's packs (holon-host's packs.rs), shared with the store reading them.
    packs: Option<std::sync::Arc<holon_host::packs::Packs>>,
}

impl Puts {
    pub(crate) fn at(dir: PathBuf) -> Puts {
        let packs = holon_host::packs::Packs::shared(&dir).ok();
        Puts { dir: Some(dir), packs, ..Puts::default() }
    }

    /// The store's packs, if it is on disk.
    pub(crate) fn packs(&self) -> Option<&holon_host::packs::Packs> {
        self.packs.as_deref()
    }

    /// Whether the store is on disk (one in memory is never swept).
    pub(crate) fn on_disk(&self) -> bool {
        self.dir.is_some()
    }

    /// Where the store keeps its values on disk, if it does.
    pub(crate) fn dir(&self) -> Option<&std::path::Path> {
        self.dir.as_deref()
    }

    /// How many puts there have been since the server began.
    pub(crate) fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// Note `hash` as just put. Called with the store held, so a sweep, which holds it too while
    /// it removes a value, sees every put before it or none of it.
    pub(crate) fn put(&self, hash: Hash) {
        if self.dir.is_none() {
            return;
        }
        let now = now_ms();
        self.count.fetch_add(1, Ordering::Relaxed);
        let mut recent = self.recent.lock().unwrap();
        recent.insert(hash, now);
        // Held to what a sweep still spares, so a server that never sweeps doesn't grow it for good.
        if recent.len().is_multiple_of(FORGET_AT) {
            recent.retain(|_, at| *at + SPARES_MS > now);
        }
    }
}

/// What a sweep found: how many values and bytes it removed (or would, in a dry run), kept as
/// named, and spared as new.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Swept {
    pub(crate) values: usize,
    pub(crate) bytes: u64,
    pub(crate) kept: usize,
    pub(crate) kept_bytes: u64,
    pub(crate) spared: usize,
    pub(crate) spared_bytes: u64,
    /// What is kept, by the deployment that names it (values, bytes): a value is the first
    /// deployment's to reach it; one another reaches too is `shared`'s (and what it names stays
    /// the first's: the walk doesn't go through a value twice).
    pub(crate) held: BTreeMap<String, (usize, u64)>,
    pub(crate) shared: (usize, u64),
    /// The named values older than what it spares it moved into a pack (the server's own sweep),
    /// and their bytes.
    pub(crate) packed: usize,
    pub(crate) packed_bytes: u64,
    /// The kept file texts each deployment's log names (texts, their bytes), read on the same
    /// pass over the logs: the folders' sweep, skipped when it has nothing to remove, can't say.
    pub(crate) file_texts: BTreeMap<String, (usize, u64)>,
}

/// Each 64-digit hex in `text` (a run of exactly that many hex digits), as a hash.
fn named_in(text: &str, out: &mut BTreeSet<Hash>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_hexdigit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
            i += 1;
        }
        if i - start == 64 {
            out.extend(parse_hash(&text[start..i].to_ascii_lowercase()));
        }
    }
}

/// Each hash `value`'s texts name, at any depth.
fn named_in_value(value: &Value, out: &mut BTreeSet<Hash>) {
    match value {
        Value::Text(t) => named_in(t, out),
        Value::List(items) => items.iter().for_each(|v| named_in_value(v, out)),
        Value::Record(fields) => fields.iter().for_each(|(k, v)| {
            named_in(k, out);
            named_in_value(v, out);
        }),
        Value::Effect { sink, payload } => {
            named_in(sink, out);
            named_in_value(payload, out);
        }
        Value::Sum { payload, .. } => named_in_value(payload, out),
        Value::Number(_) | Value::Boolean(_) => {}
    }
}

/// Every hash the deployments name: their live states (each cell by its hash and by what its value
/// names), and every line, record and document they keep.
/// What one deployment names: the hashes, and the file texts its log keeps (each text's id, its
/// size).
type Names = (String, BTreeSet<Hash>, BTreeMap<String, u64>);

/// Every hash each deployment names: its live state (each cell by its hash and by what its value
/// names), and every line, record and document it keeps; and the file texts its log names.
fn named(deployments: &Mutex<BTreeMap<String, Deployment>>) -> Result<Vec<Names>, String> {
    let json = |what: &str, r: serde_json::Result<String>| r.map_err(|e| format!("its {what} can't be read for what they name: {e}"));
    let mut each = Vec::new();
    {
        let table = deployments.lock().unwrap();
        for (slug, dep) in table.iter() {
            let mut texts = dep.events.lines();
            texts.push(json("sessions", serde_json::to_string(&dep.life.sessions))?);
            texts.push(json("versions", serde_json::to_string(&dep.versions))?);
            texts.push(json("waiting runs", serde_json::to_string(&dep.parked))?);
            texts.push(json("ticks", serde_json::to_string(&dep.history))?);
            texts.extend(dep.document.as_ref().map(|d| d.to_string()));
            each.push((slug.clone(), std::sync::Arc::clone(&dep.log), texts, dep.state.clone()));
        }
    }
    let mut named = Vec::new();
    for (slug, log, texts, state) in each {
        let mut hashes = BTreeSet::new();
        let mut file_texts = BTreeMap::new();
        // The log's file read with the table and its own lock let go, a line at a time: reading it
        // holds up no run (docs/L4s/resilience-audit-L4.md, R5, R6). A line a run appends after
        // its file is read names values put since, which the sweep spares.
        log.each_line(|line| {
            named_in(line, &mut hashes);
            if line.contains("\"blob\":")
                && let Ok(entry) = serde_json::from_str::<crate::recorder::LogEntry>(line)
            {
                for file in entry.reads.iter().filter_map(|r| r.file.as_ref()) {
                    if let Some(blob) = &file.blob {
                        file_texts.insert(blob.clone(), file.size);
                    }
                }
            }
        });
        if let Some(cell) = state {
            let state = cell.state.lock().unwrap();
            for (key, value) in state.iter() {
                hashes.extend(state.hash_of(key).copied());
                named_in_value(value, &mut hashes);
            }
        }
        for text in &texts {
            named_in(text, &mut hashes);
        }
        named.push((slug, hashes, file_texts));
    }
    Ok(named)
}

/// Remove from `store` every value nothing the deployments keep names, directly or through another
/// value, except those put since `now_ms - spare_ms`; with `dry_run`, only count them; `paced`,
/// pausing as it removes them (`PACE`), as the server's own sweep does. None for a store in
/// memory, which has nothing on disk to sweep.
pub(crate) fn sweep(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, now_ms: u64, spare_ms: u64, dry_run: bool, paced: bool) -> Result<Option<Swept>, String> {
    let Some(dir) = &store.2.dir else { return Ok(None) };
    let _turn = store.2.sweeping.lock().unwrap_or_else(|e| e.into_inner());
    // The values there are now: one put after this isn't a candidate, whoever names it. Each its
    // own file (its path), or packed (none).
    let mut there: BTreeMap<Hash, (Option<PathBuf>, u64)> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(hash) = name.to_str().filter(|n| n.len() == 64).and_then(parse_hash) else { continue };
            there.insert(hash, (Some(entry.path()), entry.metadata().map_or(0, |m| m.len())));
        }
    }
    if let Some(packs) = store.2.packs() {
        for (hash, len) in packs.hashes() {
            there.entry(hash).or_insert((None, len));
        }
    }
    // Walked from each deployment's names in turn: a value is kept, and the first deployment's to
    // reach it; one another reaches too is shared.
    const SHARED: usize = usize::MAX;
    let named = named(deployments)?;
    let mut owner: HashMap<Hash, usize> = HashMap::new();
    for (i, (_, hashes, _)) in named.iter().enumerate() {
        let mut next: Vec<Hash> = hashes.iter().filter(|h| there.contains_key(*h)).copied().collect();
        while let Some(hash) = next.pop() {
            if let Some(o) = owner.get_mut(&hash) {
                if *o != i {
                    *o = SHARED;
                }
                continue;
            }
            owner.insert(hash, i);
            let value = match store.get_value(&hash) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(e) => return Err(format!("the value {} can't be read, so what it names can't be told: {e:?}", to_hex(&hash))),
            };
            let mut names = BTreeSet::new();
            named_in_value(&value, &mut names);
            next.extend(names.into_iter().filter(|h| there.contains_key(h) && owner.get(h).is_none_or(|o| *o != i)));
        }
    }
    let since = now_ms.saturating_sub(spare_ms);
    let mut swept = Swept::default();
    // Packed values nothing names: removed together, by rewriting their packs.
    let mut gone_packed: std::collections::HashSet<Hash> = std::collections::HashSet::new();
    // Named values older than it spares, each its own file: packed by the server's own sweep.
    let mut to_pack: Vec<(Hash, PathBuf)> = Vec::new();
    for (hash, (path, size)) in there {
        if let Some(o) = owner.get(&hash) {
            if let Some(p) = &path
                && changed_ms(p) < since
            {
                to_pack.push((hash, p.clone()));
            }
            swept.kept += 1;
            swept.kept_bytes += size;
            let held = match named.get(*o) {
                Some((slug, _, _)) => swept.held.entry(slug.clone()).or_default(),
                None => &mut swept.shared,
            };
            held.0 += 1;
            held.1 += size;
            continue;
        }
        // Held while it is looked at and removed, so no put of it comes between.
        let held = store.0.lock().unwrap();
        let put = store.2.recent.lock().unwrap().get(&hash).copied().unwrap_or(0);
        let Some(path) = path else {
            if put > since {
                swept.spared += 1;
                swept.spared_bytes += size;
            } else {
                gone_packed.insert(hash);
                swept.values += 1;
                swept.bytes += size;
            }
            continue;
        };
        if put.max(changed_ms(&path)) > since {
            swept.spared += 1;
            swept.spared_bytes += size;
            continue;
        }
        if !dry_run {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("can't sweep the value {}: {e}", to_hex(&hash))),
            }
        }
        swept.values += 1;
        swept.bytes += size;
        if paced && !dry_run && swept.values.is_multiple_of(PACE) {
            drop(held);
            std::thread::sleep(PAUSE);
        }
    }
    if !dry_run
        && !gone_packed.is_empty()
        && let Some(packs) = store.2.packs()
    {
        // Each pack written again with the store let go; then, under the store's lock, checked
        // again as its index changes: one put since (a value packed already isn't written again
        // by a put) stays.
        let recent_since = |gone: &std::collections::HashSet<Hash>| {
            let recent = store.2.recent.lock().unwrap();
            gone.iter().filter(|h| recent.get(*h).is_some_and(|put| *put > since)).copied().collect::<std::collections::HashSet<Hash>>()
        };
        let again = recent_since(&gone_packed);
        gone_packed.retain(|h| !again.contains(h));
        packs
            .rewrite(&gone_packed, || {
                let held = store.0.lock().unwrap();
                (held, recent_since(&gone_packed))
            })
            .map_err(|e| format!("can't sweep the packed values: {e}"))?;
    }
    if paced && !dry_run {
        pack(store, &to_pack, &mut swept)?;
    }
    for (slug, _, texts) in &named {
        if !texts.is_empty() {
            swept.file_texts.insert(slug.clone(), (texts.len(), texts.values().sum()));
        }
    }
    Ok(Some(swept))
}

/// How many values go into one pack at most.
const PACK_AT_MOST: usize = 16 * 1024;

/// Move `values` (named, older than a sweep spares, each its own file) into packs, then remove
/// their files, paced as removals are: a file goes only once its value is in a pack on disk.
fn pack(store: &SharedStore, values: &[(Hash, PathBuf)], swept: &mut Swept) -> Result<(), String> {
    let Some(packs) = store.2.packs() else { return Ok(()) };
    for chunk in values.chunks(PACK_AT_MOST) {
        let (mut read, mut paths) = (Vec::with_capacity(chunk.len()), Vec::with_capacity(chunk.len()));
        for (hash, path) in chunk {
            match std::fs::read(path) {
                Ok(bytes) => {
                    read.push((*hash, bytes));
                    paths.push(path);
                }
                // Gone meanwhile (swept by an admin's): nothing to pack.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("can't pack the value {}: {e}", to_hex(hash))),
            }
        }
        packs.pack(&read).map_err(|e| format!("can't write a pack: {e}"))?;
        for (i, ((hash, bytes), path)) in read.iter().zip(&paths).enumerate() {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("can't remove the packed value's file {}: {e}", to_hex(hash))),
            }
            swept.packed += 1;
            swept.packed_bytes += bytes.len() as u64;
            if (i + 1).is_multiple_of(PACE) {
                std::thread::sleep(PAUSE);
            }
        }
    }
    Ok(())
}

/// What the server's own sweeps remember of the last: how many puts there had been when it began,
/// and whether it spared anything.
#[derive(Default)]
pub(crate) struct Scheduled {
    last: Option<(u64, bool)>,
}

impl Scheduled {
    /// Sweep, unless nothing was put since the last sweep and it spared nothing: none then, and
    /// none for a store in memory.
    pub(crate) fn sweep(&mut self, store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, now_ms: u64) -> Result<Option<Swept>, String> {
        let puts = store.2.count.load(Ordering::Relaxed);
        if self.last == Some((puts, false)) {
            return Ok(None);
        }
        let swept = sweep(store, deployments, now_ms, SPARES_MS, false, true)?;
        self.last = swept.as_ref().map(|s| (puts, s.spared > 0));
        Ok(swept)
    }
}

/// `POST /admin/store/sweep` (`{"dry_run": true}` only counts), or None for any other path.
pub(crate) fn route(state: &AppState, request: &rouille::Request, by: &str) -> Option<Response> {
    if request.url() != "/admin/store/sweep" {
        return None;
    }
    if request.method() != "POST" {
        return Some(Response::empty_404());
    }
    let body = match crate::http::read_body(request) {
        Ok(b) => b,
        Err(r) => return Some(r),
    };
    let dry_run = serde_json::from_slice::<serde_json::Value>(&body).ok().and_then(|b| b["dry_run"].as_bool()).unwrap_or(false);
    // An admin's, now: as fast as it can, holding up the server's runs while it removes (`PACE`).
    let now = now_ms();
    Some(match sweep(&state.store, &state.deployments, now, SPARES_MS, dry_run, false) {
        Ok(Some(s)) => {
            state.sweeps.note(&s, now);
            if !dry_run && s.values > 0 {
                state.activity.record("swept the store", &format!("{} values, {} freed", s.values, size_text(s.bytes)), Some(by));
            }
            Response::json(&serde_json::json!({
                "dry_run": dry_run,
                "swept": { "values": s.values, "bytes": s.bytes },
                "kept": { "values": s.kept, "bytes": s.kept_bytes },
                "spared": { "values": s.spared, "bytes": s.spared_bytes },
            }))
        }
        Ok(None) => err(409, "this server's store is in memory: nothing on disk to sweep"),
        Err(e) => err(500, e),
    })
}

const STORE_USAGE: &str = "usage: holon store sweep [--dry-run] [--server PORT|URL]

  Removes from the server's content store the values nothing kept names, for an admin: the
  states of runs its logs no longer keep (retention cut them), and what only they held. What a
  kept run, a version, an event, a session, a waiting run or the live state names stays, and
  everything that names in turn; so does anything put in the last hour. Code is never swept.
  --dry-run: what it would remove, removing nothing.";

/// `holon store sweep [--dry-run]`: sweep the store of the server this user runs here (or
/// `--server`).
pub(crate) fn cli(args: &[String]) -> i32 {
    let mut server = None;
    let mut dry_run = false;
    let mut words = Vec::new();
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => match args.next() {
                Some(s) => server = Some(s.clone()),
                None => {
                    eprintln!("holon store: --server takes a port or a URL");
                    return 2;
                }
            },
            "--dry-run" => dry_run = true,
            "--help" | "-h" => {
                println!("{STORE_USAGE}");
                return 0;
            }
            _ if !a.starts_with('-') => words.push(a.as_str()),
            _ => {
                eprintln!("holon store: no option {a:?}\n\n{STORE_USAGE}");
                return 2;
            }
        }
    }
    if words != ["sweep"] {
        eprintln!("{STORE_USAGE}");
        return 2;
    }
    let body = serde_json::json!({ "dry_run": dry_run });
    let (_, answer) = match crate::local::admin_call("holon store", server.as_deref(), "POST", "/admin/store/sweep", Some(&body)) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let s: serde_json::Value = serde_json::from_str(&answer).unwrap_or_default();
    let n = |v: &serde_json::Value| v.as_u64().unwrap_or(0);
    let (swept, kept, spared) = (&s["swept"], &s["kept"], &s["spared"]);
    println!(
        "{} {} values nothing kept names ({}{}); it keeps {} ({}), and spares {} put in the last hour ({})",
        if dry_run { "would sweep" } else { "swept" },
        n(&swept["values"]),
        size_text(n(&swept["bytes"])),
        if dry_run { "" } else { " freed" },
        n(&kept["values"]),
        size_text(n(&kept["bytes"])),
        n(&spared["values"]),
        size_text(n(&spared["bytes"]))
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_is_exactly_64_hex_digits_anywhere_in_a_text() {
        let h = "ab".repeat(32);
        let mut out = BTreeSet::new();
        named_in(&format!("{{\"state_after\":\"{h}\",\"x\":\"{h}0\",\"y\":\"{}\"}}", &h[1..]), &mut out);
        assert_eq!(out, BTreeSet::from([parse_hash(&h).unwrap()]), "one too long or too short isn't one");
        let mut deep = BTreeSet::new();
        named_in_value(&Value::Record(BTreeMap::from([("c".into(), Value::List(vec![Value::Text(h.to_uppercase())]))])), &mut deep);
        assert_eq!(deep, out, "in a value, at any depth, in either case");
    }
}
