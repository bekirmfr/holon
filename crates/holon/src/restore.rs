//! `holon restore`: a deployment's versions, and going back to an earlier version's or run's code,
//! state, or both (docs/L4s/history-L4.md §4), over the control plane
//! (`GET /deployments/<slug>/versions`, `POST /deployments/<slug>/restore`). A restore is a new
//! version, a new session, never a rewrite of one: what it replaces stays restorable. Code is
//! checked against today's grant: what it needs that isn't granted now is shown, and approved only
//! with `--approve`.
//!
//! The snapshots it puts back are kept here too: a run's log line names the state it left, a
//! version the state it was deployed with, each a record of value hashes in the content store.

use std::collections::BTreeMap;

use holon_core::{parse_hash, Hash, Value};
use holon_host::{State, Store};

use crate::local::admin_call;
use crate::{to_hex, SharedStore};

/// Keep `state` as a snapshot in the content store and return its hash (hex): a record of each
/// key's value hash. Only the values not in `before` (the state it grew from, whose values the
/// store already holds) are put; with no `before`, all of them. A run keeps the state it left
/// this way, a deploy the state it carried; `load_snapshot` brings one back.
pub(crate) fn save_snapshot(state: &State, before: Option<&State>, store: &SharedStore) -> Result<String, String> {
    let mut store = store.clone();
    let mut refs = BTreeMap::new();
    for (key, value) in state.iter() {
        let hash = *state.hash_of(key).expect("a key it iterates");
        if before.and_then(|b| b.hash_of(key)) != Some(&hash) {
            store.put_value(value).map_err(|e| format!("{e:?}"))?;
        }
        refs.insert(key.clone(), Value::Text(to_hex(&hash)));
    }
    let snapshot = store.put_value(&Value::Record(refs)).map_err(|e| format!("{e:?}"))?;
    Ok(to_hex(&snapshot))
}

/// The references a snapshot (`save_snapshot`) holds, key → value hash.
pub(crate) fn snapshot_refs(hex: &str, store: &SharedStore) -> Result<BTreeMap<String, Hash>, String> {
    let hash = parse_hash(hex).ok_or("its snapshot isn't a hash")?;
    let Some(Value::Record(refs)) = store.get_value(&hash).map_err(|e| format!("{e:?}"))? else {
        return Err("its snapshot isn't in the store".into());
    };
    refs.into_iter()
        .map(|(k, v)| match v {
            Value::Text(h) => parse_hash(&h).map(|h| (k, h)).ok_or_else(|| "its snapshot names a value that isn't a hash".to_string()),
            _ => Err("its snapshot names a value that isn't a hash".into()),
        })
        .collect()
}

/// The state a snapshot holds, each value fetched from the store and checked against its hash.
pub(crate) fn load_snapshot(hex: &str, store: &SharedStore) -> Result<State, String> {
    State::load(&snapshot_refs(hex, store)?, store).map_err(|e| match e {
        holon_host::LoadError::Missing { key, .. } => format!("{key:?}'s value isn't in the store"),
        holon_host::LoadError::Store(e) => format!("{e:?}"),
    })
}

const RESTORE_USAGE: &str = "usage: holon restore <slug> [--server PORT|URL]
       holon restore <slug> --to N [--code] [--state] [--dry-run] [--approve] [--server PORT|URL]
       holon restore <slug> --to-run SEQ [--code] [--state] [--dry-run] [--approve] [--server PORT|URL]

  With no --to: the deployment's versions, newest first. Every deploy is a version, kept for
  good, numbered from 1.
  --to N: version N. --code deploys its code again (its flow, with what its deploy asked), as a
  new version. --state puts back the state it left (the one the version after it was deployed
  with). Both: the code, then the state.
  --to-run SEQ: run SEQ of the log. --state puts back the state it left; --code deploys the code
  it ran on again, unless that is the code deployed now.
  The code is checked against the grant the deployment has now, not the one it had: what it
  needs that isn't granted now is listed, and approved only with --approve. A state put back
  starts a new session, the state it replaces kept as the last one's, so it can be restored in
  turn; the deployment must be running or paused. --dry-run: what it would do, changing nothing,
  with the runs since that point and what they did outside, which a restore doesn't undo.

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

pub(crate) fn cli(args: &[String]) -> i32 {
    const WHO: &str = "holon restore";
    let (mut slug, mut server, mut to, mut to_run) = (None, None, None, None);
    let (mut code, mut state, mut dry_run, mut approve) = (false, false, false, false);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--to" => match args.next().and_then(|n| n.trim_start_matches(['v', 'V']).parse::<u32>().ok()) {
                Some(n) => to = Some(n),
                None => {
                    eprintln!("{WHO}: --to takes a version's number (holon restore <slug> lists them)");
                    return 2;
                }
            },
            "--to-run" => match args.next().and_then(|n| n.trim_start_matches('#').parse::<u64>().ok()) {
                Some(n) => to_run = Some(n),
                None => {
                    eprintln!("{WHO}: --to-run takes a run's number in the log (its seq, as the editor's runs show it)");
                    return 2;
                }
            },
            "--code" => code = true,
            "--state" => state = true,
            "--dry-run" => dry_run = true,
            "--approve" => approve = true,
            "--help" | "-h" => {
                println!("{RESTORE_USAGE}");
                return 0;
            }
            _ if slug.is_none() && !a.starts_with('-') => slug = Some(a.clone()),
            _ => {
                eprintln!("{WHO}: no option {a:?}\n\n{RESTORE_USAGE}");
                return 2;
            }
        }
    }
    let Some(slug) = slug else {
        eprintln!("{RESTORE_USAGE}");
        return 2;
    };
    let (point, at) = match (to, to_run) {
        (Some(_), Some(_)) => {
            eprintln!("{WHO}: go back to one point: --to N or --to-run SEQ");
            return 2;
        }
        (Some(n), None) => (format!("version {n}"), serde_json::json!({ "version": n })),
        (None, Some(seq)) => (format!("run {seq}"), serde_json::json!({ "run": seq })),
        (None, None) => {
            if code || state || dry_run || approve {
                eprintln!("{WHO}: say which point: --to N, a version (holon restore {slug} lists them), or --to-run SEQ, a run");
                return 2;
            }
            return list(&slug, server.as_deref());
        }
    };
    if !code && !state {
        eprintln!("{WHO}: say what to restore: --code, --state, or both");
        return 2;
    }
    let path = format!("/deployments/{slug}/restore");
    let ask = |extra: serde_json::Value| -> Result<serde_json::Value, i32> {
        let mut body = serde_json::json!({ "code": code, "state": state });
        for (k, v) in at.as_object().into_iter().flatten().chain(extra.as_object().into_iter().flatten()) {
            body[k] = v.clone();
        }
        let (_, answer) = admin_call(WHO, server.as_deref(), "POST", &path, Some(&body))?;
        serde_json::from_str(&answer).map_err(|e| {
            eprintln!("{WHO}: the server's answer isn't JSON: {e}");
            1
        })
    };
    let preview = match ask(serde_json::json!({ "dry_run": true })) {
        Ok(p) => p,
        Err(code) => return code,
    };
    if let Some(why) = preview["refused"].as_str() {
        eprintln!("{WHO}: the code of {point} can't be deployed on this server now: {why}");
        return 1;
    }
    let deploys = preview["code"].as_u64();
    if code && deploys.is_none() {
        println!("{point} ran on the code deployed now: only its state is restored");
    }
    let needs: Vec<String> = preview["needs_approval"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
    let missing: Vec<&str> = preview["missing_secrets"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
    if !missing.is_empty() {
        println!("its secrets not set now (those nodes stay empty until they are): {}", missing.join(", "));
    }
    if dry_run {
        if let Some(n) = deploys {
            if needs.is_empty() {
                println!("version {n}'s code would deploy again with the grant {slug} has now");
            } else {
                println!("version {n}'s code needs approving what today's grant doesn't cover: {}", needs.join(", "));
            }
        }
        if !preview["state"].is_null() {
            println!("{}", state_lines(&preview["state"]));
        }
        println!("nothing changed (--dry-run)");
        return 0;
    }
    if !needs.is_empty() && !approve {
        eprintln!("{WHO}: the code of {point} needs approving what today's grant doesn't cover: {}", needs.join(", "));
        eprintln!("{WHO}: to approve them and restore it, run this again with --approve");
        return 1;
    }
    let done = match ask(serde_json::json!({ "acknowledge": needs })) {
        Ok(done) => done,
        Err(code) => return code,
    };
    if let Some(n) = done["code"].as_u64() {
        let unit = done["unit"].as_str().unwrap_or_default();
        println!("{slug}: version {n}'s code is deployed again, as version {} ({})", done["version"], &unit[..unit.len().min(12)]);
    } else if !state {
        println!("{slug}: nothing to deploy");
    }
    match done["state"]["session"].as_u64() {
        Some(session) => println!(
            "{slug}: the state {point} left is live again, in session {session}; the state it replaced is session {}'s (holon restore doesn't undo what runs did outside)",
            session.saturating_sub(1)
        ),
        None if !state => println!("{slug}: its state is as it was"),
        None => {}
    }
    0
}

/// What putting a state back would change, as a dry run answers it: the keys, and the runs since.
fn state_lines(change: &serde_json::Value) -> String {
    let keys = |k: &str| -> Vec<&str> { change[k].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect() };
    let from = change["from"].as_str().unwrap_or("that point");
    let (changed, added, removed) = (keys("changed"), keys("added"), keys("removed"));
    let mut lines = vec![if changed.is_empty() && added.is_empty() && removed.is_empty() {
        format!("the state {from} left is the state now: putting it back changes no key")
    } else {
        let mut parts = Vec::new();
        for (word, list) in [("changes", &changed), ("brings back", &added), ("drops", &removed)] {
            if !list.is_empty() {
                parts.push(format!("{word} {}", list.join(", ")));
            }
        }
        format!("the state {from} left would be put back: it {}", parts.join("; "))
    }];
    let runs = change["runs_since"].as_u64().unwrap_or_default();
    let effects: Vec<String> = change["effects_since"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, n)| format!("{} {}", n.as_u64().unwrap_or_default(), effect_word(name)))
        .collect();
    lines.push(match (runs, effects.is_empty()) {
        (0, _) => format!("no run was logged since {from}"),
        (_, true) => format!("{runs} run(s) logged since {from}, which did nothing outside its state"),
        (_, false) => format!("{runs} run(s) logged since {from} also did, outside its state, what a restore doesn't undo: {}", effects.join(", ")),
    });
    lines.join("\n")
}

/// A capability a run performed, as the dry run counts it.
fn effect_word(name: &str) -> &str {
    match name {
        "HttpRequest" => "call(s) out",
        "FileWrite" => "file write(s)",
        "DbWrite" => "database write(s)",
        "ChannelSend" => "message(s) sent",
        "RunsAction" => "action(s) run",
        "Spawn" => "run(s) started",
        other => other,
    }
}

/// Print `slug`'s versions, newest first.
fn list(slug: &str, server: Option<&str>) -> i32 {
    const WHO: &str = "holon restore";
    let answer = match admin_call(WHO, server, "GET", &format!("/deployments/{slug}/versions"), None) {
        Ok((_, a)) => a,
        Err(code) => return code,
    };
    let Ok(listed) = serde_json::from_str::<serde_json::Value>(&answer) else {
        eprintln!("{WHO}: the server's answer isn't JSON");
        return 1;
    };
    for v in listed["versions"].as_array().into_iter().flatten() {
        println!("{}", version_line(v));
    }
    println!("\nholon restore {slug} --to N --code deploys version N's code again; --state puts back the state it left");
    0
}

/// One version, as the list shows it: `v3  3f2a9c1e04b7  2026-10-29T20:56:39  by maria  restores v1  (now)`.
fn version_line(v: &serde_json::Value) -> String {
    let unit = v["unit"].as_str().unwrap_or_default();
    let mut line = format!("v{:<4} {}", v["number"].as_u64().unwrap_or_default(), &unit[..unit.len().min(12)]);
    if let Some(at) = v["deployed_at"].as_i64() {
        line.push_str(&format!("  {}", crate::engines::time_text(at * 1_000_000)));
    }
    if let Some(by) = v["by"].as_str() {
        line.push_str(&format!("  by {by}"));
    }
    if let Some(from) = v["restored_from"].as_u64() {
        line.push_str(&format!("  restores v{from}"));
    }
    if v["state_kept"] == true {
        line.push_str("  state kept");
    }
    if v["current"] == true {
        line.push_str("  (now)");
    } else if v["restorable"] != true {
        line.push_str("  (from before rc.30: opens, doesn't restore)");
    }
    line
}

#[cfg(test)]
mod tests {
    use super::version_line;

    #[test]
    fn a_version_line_says_what_it_is() {
        let v = serde_json::json!({ "number": 3, "unit": "3f2a9c1e04b7aa", "deployed_at": 1_793_307_399, "by": "maria", "restored_from": 1, "current": true, "restorable": true });
        assert_eq!(version_line(&v), "v3    3f2a9c1e04b7  2026-10-29T20:56:39  by maria  restores v1  (now)");
        let kept = serde_json::json!({ "number": 2, "unit": "aa", "current": false, "restorable": true, "state_kept": true });
        assert_eq!(version_line(&kept), "v2    aa  state kept");
        let old = serde_json::json!({ "number": 1, "unit": "00ff", "current": false, "restorable": false });
        assert_eq!(version_line(&old), "v1    00ff  (from before rc.30: opens, doesn't restore)");
    }
}
