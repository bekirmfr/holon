//! Sweeping the content store's values that nothing kept names (docs/L4s/history-L4.md §7):
//! `holon store sweep`, `POST /admin/store/sweep`, an admin's.
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
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use holon_core::{parse_hash, Hash, Value};
use holon_host::Store;

use crate::folders::size_text;
use crate::{err, to_hex, AppState, Deployment, Response, SharedStore};

/// How new a value a sweep spares, whoever names it: four times the longest a run may take.
pub(crate) const SPARES_MS: u64 = 60 * 60 * 1000;
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
}

impl Puts {
    pub(crate) fn at(dir: PathBuf) -> Puts {
        Puts { dir: Some(dir), ..Puts::default() }
    }

    /// Note `hash` as just put. Called with the store held, so a sweep, which holds it too while
    /// it removes a value, sees every put before it or none of it.
    pub(crate) fn put(&self, hash: Hash) {
        if self.dir.is_none() {
            return;
        }
        let now = now_ms();
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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Swept {
    pub(crate) values: usize,
    pub(crate) bytes: u64,
    pub(crate) kept: usize,
    pub(crate) kept_bytes: u64,
    pub(crate) spared: usize,
    pub(crate) spared_bytes: u64,
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
fn named(deployments: &Mutex<BTreeMap<String, Deployment>>) -> Result<BTreeSet<Hash>, String> {
    let json = |what: &str, r: serde_json::Result<String>| r.map_err(|e| format!("its {what} can't be read for what they name: {e}"));
    let mut texts = Vec::new();
    let states: Vec<_> = {
        let table = deployments.lock().unwrap();
        for dep in table.values() {
            texts.extend(dep.log.lines());
            texts.extend(dep.events.lines());
            texts.push(json("sessions", serde_json::to_string(&dep.life.sessions))?);
            texts.push(json("versions", serde_json::to_string(&dep.versions))?);
            texts.push(json("waiting runs", serde_json::to_string(&dep.parked))?);
            texts.push(json("ticks", serde_json::to_string(&dep.history))?);
            texts.extend(dep.document.as_ref().map(|d| d.to_string()));
        }
        table.values().filter_map(|d| d.state.clone()).collect()
    };
    let mut hashes = BTreeSet::new();
    for cell in states {
        let state = cell.state.lock().unwrap();
        for (key, value) in state.iter() {
            hashes.extend(state.hash_of(key).copied());
            named_in_value(value, &mut hashes);
        }
    }
    for text in &texts {
        named_in(text, &mut hashes);
    }
    Ok(hashes)
}

/// Remove from `store` every value nothing the deployments keep names, directly or through another
/// value, except those put since `now_ms - spare_ms`; with `dry_run`, only count them. None for a
/// store in memory, which has nothing on disk to sweep.
pub(crate) fn sweep(store: &SharedStore, deployments: &Mutex<BTreeMap<String, Deployment>>, now_ms: u64, spare_ms: u64, dry_run: bool) -> Result<Option<Swept>, String> {
    let Some(dir) = &store.2.dir else { return Ok(None) };
    // The values there are now: one put after this isn't a candidate, whoever names it.
    let mut there: BTreeMap<Hash, (PathBuf, u64)> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(hash) = name.to_str().filter(|n| n.len() == 64).and_then(parse_hash) else { continue };
            there.insert(hash, (entry.path(), entry.metadata().map_or(0, |m| m.len())));
        }
    }
    let mut kept = BTreeSet::new();
    let mut next: Vec<Hash> = named(deployments)?.into_iter().filter(|h| there.contains_key(h)).collect();
    while let Some(hash) = next.pop() {
        if !kept.insert(hash) {
            continue;
        }
        let value = match store.get_value(&hash) {
            Ok(Some(v)) => v,
            Ok(None) => continue,
            Err(e) => return Err(format!("the value {} can't be read, so what it names can't be told: {e:?}", to_hex(&hash))),
        };
        let mut names = BTreeSet::new();
        named_in_value(&value, &mut names);
        next.extend(names.into_iter().filter(|h| there.contains_key(h) && !kept.contains(h)));
    }
    let since = now_ms.saturating_sub(spare_ms);
    let mut swept = Swept::default();
    for (hash, (path, size)) in there {
        if kept.contains(&hash) {
            swept.kept += 1;
            swept.kept_bytes += size;
            continue;
        }
        // Held while it is looked at and removed, so no put of it comes between.
        let _store = store.0.lock().unwrap();
        let put = store.2.recent.lock().unwrap().get(&hash).copied().unwrap_or(0);
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
    }
    Ok(Some(swept))
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
    Some(match sweep(&state.store, &state.deployments, now_ms(), SPARES_MS, dry_run) {
        Ok(Some(s)) => {
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
