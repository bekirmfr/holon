//! `holon history`: a deployment's timeline (docs/L4s/history-L4.md §6), its runs and its events
//! (events.rs) in one order, over the control plane (`GET /deployments/<slug>/log`, `…/events`,
//! `…/versions`). An event goes before the run whose seq it names: it happened after the runs
//! before that one.

use std::collections::BTreeMap;

use crate::local::admin_call;

const HISTORY_USAGE: &str = "usage: holon history <slug> [--last N] [--events] [--server PORT|URL]

  A deployment's timeline, oldest first: its runs, and what was done to it, by whom: deploys,
  restores, grant changes, secrets (never their values), folders and databases bound for it,
  pausing, stopping and running. --last N: the newest N runs (default 20), with the events among
  them. --events: every event, kept for good, without the runs.

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

pub(crate) fn cli(args: &[String]) -> i32 {
    const WHO: &str = "holon history";
    let (mut slug, mut server, mut last, mut events_only) = (None, None, 20usize, false);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--server" => server = args.next().cloned(),
            "--last" => match args.next().and_then(|n| n.parse::<usize>().ok()) {
                Some(n) => last = n,
                None => {
                    eprintln!("{WHO}: --last takes a number of runs");
                    return 2;
                }
            },
            "--events" => events_only = true,
            "--help" | "-h" => {
                println!("{HISTORY_USAGE}");
                return 0;
            }
            _ if slug.is_none() && !a.starts_with('-') => slug = Some(a.clone()),
            _ => {
                eprintln!("{WHO}: no option {a:?}\n\n{HISTORY_USAGE}");
                return 2;
            }
        }
    }
    let Some(slug) = slug else {
        eprintln!("{HISTORY_USAGE}");
        return 2;
    };
    let get = |path: String| -> Result<serde_json::Value, i32> {
        let (_, answer) = admin_call(WHO, server.as_deref(), "GET", &path, None)?;
        serde_json::from_str(&answer).map_err(|e| {
            eprintln!("{WHO}: the server's answer isn't JSON: {e}");
            1
        })
    };
    let events = match get(format!("/deployments/{slug}/events")) {
        Ok(v) => v["events"].as_array().cloned().unwrap_or_default(),
        Err(code) => return code,
    };
    let runs = if events_only {
        Vec::new()
    } else {
        match get(format!("/deployments/{slug}/log?limit={last}")) {
            Ok(v) => v["entries"].as_array().cloned().unwrap_or_default(),
            Err(code) => return code,
        }
    };
    let versions: BTreeMap<String, u64> = match get(format!("/deployments/{slug}/versions")) {
        // Newest first: the newest version with a unit names it.
        Ok(v) => v["versions"].as_array().into_iter().flatten().rev().filter_map(|v| Some((v["unit"].as_str()?.to_string(), v["number"].as_u64()?))).collect(),
        Err(code) => return code,
    };
    let lines = timeline(&runs, &events, &versions, events_only);
    if lines.is_empty() {
        println!("{slug}: nothing yet");
    }
    for line in lines {
        println!("{line}");
    }
    0
}

/// The timeline's lines, oldest first: each event before the run whose seq it names, and, with
/// runs, only the events from the oldest run shown on (all of them when there is none).
fn timeline(runs: &[serde_json::Value], events: &[serde_json::Value], versions: &BTreeMap<String, u64>, events_only: bool) -> Vec<String> {
    let from = if events_only { 0 } else { runs.first().and_then(|r| r["seq"].as_u64()).unwrap_or_default() };
    let mut items: Vec<(u64, u8, String)> = events
        .iter()
        .filter(|e| e["run_seq"].as_u64().is_some_and(|s| s >= from))
        .map(|e| (e["run_seq"].as_u64().unwrap_or_default(), 0, event_line(e)))
        .collect();
    items.extend(runs.iter().map(|r| (r["seq"].as_u64().unwrap_or_default(), 1, run_line(r, versions))));
    // Stable: events keep their order among themselves, before the run they name.
    items.sort_by_key(|(seq, kind, _)| (*seq, *kind));
    items.into_iter().map(|(_, _, line)| line).collect()
}

fn when(at_ms: Option<u64>) -> String {
    at_ms.map(|ms| crate::engines::time_text(ms as i64 * 1000)).unwrap_or_default()
}

/// `#41  run        2026-10-30T09:12:03  PUT /prices/tea  200  v5`
fn run_line(r: &serde_json::Value, versions: &BTreeMap<String, u64>) -> String {
    let what = match (r["method"].as_str(), r["url"].as_str()) {
        (Some(m), Some(u)) => format!("{m} {u}"),
        _ => r["kind"].as_str().unwrap_or("run").to_string(),
    };
    let mut line = format!("#{:<5} {:<10} {}  {what}", r["seq"].as_u64().unwrap_or_default(), "run", when(r["at_ms"].as_u64()));
    if let Some(status) = r["status"].as_u64() {
        line.push_str(&format!("  {status}"));
    }
    if r["error"].is_string() {
        line.push_str("  failed");
    }
    match r["unit"].as_str().and_then(|u| versions.get(u)) {
        Some(n) => line.push_str(&format!("  v{n}")),
        None => line.push_str(&format!("  {}", r["unit"].as_str().map_or("", |u| &u[..u.len().min(12)]))),
    }
    line
}

/// `       grant      2026-10-30T09:12:05  revoked StateWrite("a")  by maria`
fn event_line(e: &serde_json::Value) -> String {
    let by = e["by"].as_str().map_or_else(|| "  by the host".to_string(), |b| format!("  by {b}"));
    format!("{:<6} {:<10} {}  {}{by}", "", e["kind"].as_str().unwrap_or_default(), when(e["at_ms"].as_u64()), e["what"].as_str().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_go_before_the_run_they_name() {
        let runs = [
            json!({ "seq": 4, "at_ms": 0, "method": "PUT", "url": "/prices/tea", "status": 200, "unit": "aa" }),
            json!({ "seq": 5, "at_ms": 0, "kind": "tick", "unit": "bb", "error": "boom" }),
        ];
        let events = [
            json!({ "run_seq": 2, "at_ms": 0, "kind": "deploy", "what": "deployed version 1", "by": "maria" }),
            json!({ "run_seq": 5, "at_ms": 0, "kind": "grant", "what": "revoked Clock", "by": "maria" }),
            json!({ "run_seq": 5, "at_ms": 0, "kind": "lifecycle", "what": "paused: it may no longer read the clock" }),
        ];
        let versions = BTreeMap::from([("aa".to_string(), 3)]);
        let lines = timeline(&runs, &events, &versions, false);
        assert_eq!(
            lines,
            [
                "#4     run        1970-01-01T00:00:00  PUT /prices/tea  200  v3",
                "       grant      1970-01-01T00:00:00  revoked Clock  by maria",
                "       lifecycle  1970-01-01T00:00:00  paused: it may no longer read the clock  by the host",
                "#5     run        1970-01-01T00:00:00  tick  failed  bb",
            ],
            "the deploy before the oldest run shown is left out"
        );
        assert_eq!(timeline(&[], &events, &versions, true).len(), 3, "--events: every one");
    }
}
