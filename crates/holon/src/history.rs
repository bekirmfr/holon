//! `holon history`: a deployment's timeline (docs/L4s/history-L4.md §6), its runs and its events
//! (events.rs) in one order, over the control plane (`GET /deployments/<slug>/log`, `…/events`,
//! `…/versions`). An event goes before the run whose seq it names: it happened after the runs
//! before that one. `--cell` is one cell's history instead (changes.rs: `…/changes`): the runs
//! that changed it, with the value before and after.

use crate::local::admin_call;

const HISTORY_USAGE: &str = "usage: holon history <slug> [--last N] [--events] [--server PORT|URL]
       holon history <slug> --cell NAME [--item KEY] [--last N] [--server PORT|URL]

  A deployment's timeline, oldest first: its runs, and what was done to it, by whom: deploys,
  restores, grant changes, secrets (never their values), folders and databases bound for it,
  pausing, stopping and running. --last N: the newest N runs (default 20), with the events among
  them. --events: every event, kept for good, without the runs.

  --cell NAME: one cell's history (state.NAME), or a collection's: the runs that changed it,
  oldest first, each with its value before and after (a collection's, the items it changed),
  and a restore or a deploy between two runs that moved it. --item KEY: one item of the
  collection NAME. --last N: its newest N changes (default 20).

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

pub(crate) fn cli(args: &[String]) -> i32 {
    const WHO: &str = "holon history";
    let (mut slug, mut server, mut last, mut events_only) = (None, None, 20usize, false);
    let (mut cell, mut item) = (None, None);
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
            "--cell" => cell = args.next().cloned(),
            "--item" => item = args.next().cloned(),
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
    if let Some(cell) = cell {
        let item = item.map(|i| format!("&item={}", crate::mcp::seg(&i))).unwrap_or_default();
        let answer = match get(format!("/deployments/{slug}/changes?name={}{item}&limit={}", crate::mcp::seg(&cell), last.clamp(1, 200))) {
            Ok(v) => v,
            Err(code) => return code,
        };
        let versions = match get(format!("/deployments/{slug}/versions")) {
            Ok(v) => Versions::of(&v),
            Err(code) => return code,
        };
        for line in change_lines(&answer, &versions) {
            println!("{line}");
        }
        return 0;
    }
    if item.is_some() {
        eprintln!("{WHO}: --item is one item of a collection: give the collection with --cell");
        return 2;
    }
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
    let versions = match get(format!("/deployments/{slug}/versions")) {
        Ok(v) => Versions::of(&v),
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

/// A deployment's versions (`GET …/versions`), to name the one a run ran on.
pub(crate) struct Versions(Vec<Version>);

struct Version {
    number: u64,
    unit: String,
    /// Its first run's seq: the log's next one when it was deployed (absent before rc.30).
    first_seq: Option<u64>,
}

impl Versions {
    pub(crate) fn of(listed: &serde_json::Value) -> Versions {
        // Listed newest first; kept oldest first.
        Versions(
            listed["versions"]
                .as_array()
                .into_iter()
                .flatten()
                .rev()
                .filter_map(|v| Some(Version { number: v["number"].as_u64()?, unit: v["unit"].as_str()?.to_string(), first_seq: v["first_seq"].as_u64() }))
                .collect(),
        )
    }

    /// The version run `seq` ran on, its code `unit`: of the versions with that code, the last one
    /// deployed by then, not a later one that restored the same code. A version from before rc.30
    /// doesn't say when it was deployed: then the newest of those.
    pub(crate) fn ran_on(&self, unit: &str, seq: u64) -> Option<u64> {
        let same: Vec<&Version> = self.0.iter().filter(|v| v.unit == unit).collect();
        same.iter().rev().find(|v| v.first_seq.is_some_and(|f| f <= seq)).or_else(|| same.iter().rev().find(|v| v.first_seq.is_none())).or(same.last()).map(|v| v.number)
    }
}

/// The timeline's lines, oldest first: each event before the run whose seq it names, and, with
/// runs, only the events from the oldest run shown on (all of them when there is none).
pub(crate) fn timeline(runs: &[serde_json::Value], events: &[serde_json::Value], versions: &Versions, events_only: bool) -> Vec<String> {
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

/// A cell's history (`GET …/changes`), oldest first: a heading with its value now, then each
/// change as its run's line (or the events between two runs), its values after it.
pub(crate) fn change_lines(answer: &serde_json::Value, versions: &Versions) -> Vec<String> {
    let show = |v: &serde_json::Value| holon_author::agent::show(v);
    let name = answer["name"].as_str().unwrap_or_default();
    let what = match answer["item"].as_str() {
        Some(item) => format!("{name}[{}]", serde_json::Value::from(item)),
        None if answer["collection"] == true => format!("the collection {name}"),
        None => format!("state.{name}"),
    };
    let changes = answer["changes"].as_array().map(Vec::as_slice).unwrap_or_default();
    let now = if answer["collection"] == true && answer["item"].is_null() { String::new() } else { format!(", now {}", show(&answer["now"])) };
    if changes.is_empty() {
        return vec![format!("{what}: no run in the log changed it{now}")];
    }
    let older = if answer["more"] == true { ", older ones not shown" } else { "" };
    let mut lines = vec![format!("{what}, oldest first{now}{older}:")];
    for c in changes.iter().rev() {
        let values = match c["items"].as_object() {
            Some(items) => {
                let mut said: Vec<String> = items.iter().map(|(k, v)| format!("{k}: {} → {}", show(&v["before"]), show(&v["after"]))).collect();
                if c["more_items"] == true {
                    said.push("and more".into());
                }
                said.join(", ")
            }
            None => format!("{} → {}", show(&c["before"]), show(&c["after"])),
        };
        if c["seq"].is_u64() {
            let run = serde_json::json!({ "seq": c["seq"], "at_ms": c["at_ms"], "method": c["method"], "url": c["url"], "kind": c["kind"], "status": c["status"], "unit": c["unit"], "error": if c["failed"] == true { serde_json::json!("failed") } else { serde_json::Value::Null } });
            lines.push(format!("{}   {values}", run_line(&run, versions)));
        } else {
            let between: Vec<String> = c["events"].as_array().into_iter().flatten().map(|e| format!("{}: {}", e["kind"].as_str().unwrap_or_default(), e["what"].as_str().unwrap_or_default())).collect();
            lines.push(format!("{:<6} {:<10} {}  between runs {} and {}: {}   {values}", "", "outside", when(c["at_ms"].as_u64()), c["between"][0], c["between"][1], between.join("; ")));
        }
    }
    lines
}

fn when(at_ms: Option<u64>) -> String {
    at_ms.map(|ms| crate::engines::time_text(ms as i64 * 1000)).unwrap_or_default()
}

/// `#41  run        2026-10-30T09:12:03  PUT /prices/tea  200  v5`
fn run_line(r: &serde_json::Value, versions: &Versions) -> String {
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
    match r["unit"].as_str().and_then(|u| versions.ran_on(u, r["seq"].as_u64().unwrap_or_default())) {
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
        let versions = Versions::of(&json!({ "versions": [{ "number": 3, "unit": "aa" }] }));
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

    /// A run is named by the version it ran on: a later version that restored the same code
    /// isn't it.
    #[test]
    fn a_run_names_the_version_deployed_when_it_ran() {
        // Newest first, as the server lists them: v3 restored v1's code at run 20.
        let versions = Versions::of(&json!({ "versions": [
            { "number": 3, "unit": "aa", "first_seq": 20 },
            { "number": 2, "unit": "bb", "first_seq": 10 },
            { "number": 1, "unit": "aa", "first_seq": 0 },
        ] }));
        assert_eq!([versions.ran_on("aa", 4), versions.ran_on("bb", 12), versions.ran_on("aa", 21), versions.ran_on("zz", 1)], [Some(1), Some(2), Some(3), None]);
        let old = Versions::of(&json!({ "versions": [{ "number": 2, "unit": "aa", "first_seq": 9 }, { "number": 1, "unit": "aa" }] }));
        assert_eq!(old.ran_on("aa", 3), Some(1), "before rc.30, a version doesn't say when");
    }

    #[test]
    fn a_cells_changes_read_oldest_first_with_the_values_after_each() {
        let n = |v: f64| json!({ "type": "Number", "value": v });
        let answer = json!({
            "name": "balances", "item": "a", "collection": true, "now": n(100.0), "more": false,
            "changes": [
                { "between": [7, 9], "at_ms": 0, "events": [{ "kind": "restore", "what": "put back the state run 5 left, in session 2" }], "before": n(70.0), "after": n(100.0) },
                { "seq": 7, "at_ms": 0, "method": "POST", "url": "/accounts/a/move", "kind": "request", "status": 200, "unit": "aa", "failed": false, "before": n(100.0), "after": n(70.0) },
                { "seq": 5, "at_ms": 0, "method": "POST", "url": "/accounts/a/move", "kind": "request", "status": 200, "unit": "aa", "failed": false, "before": null, "after": n(100.0) },
            ]
        });
        let versions = Versions::of(&json!({ "versions": [{ "number": 1, "unit": "aa" }] }));
        assert_eq!(
            change_lines(&answer, &versions),
            [
                "balances[\"a\"], oldest first, now 100:",
                "#5     run        1970-01-01T00:00:00  POST /accounts/a/move  200  v1   nothing → 100",
                "#7     run        1970-01-01T00:00:00  POST /accounts/a/move  200  v1   100 → 70",
                "       outside    1970-01-01T00:00:00  between runs 7 and 9: restore: put back the state run 5 left, in session 2   70 → 100",
            ]
        );
        let whole = json!({ "name": "balances", "collection": true, "more": true, "changes": [{ "seq": 2, "at_ms": 0, "kind": "tick", "unit": "bb", "failed": true, "items": { "b": { "before": n(5.0), "after": null } }, "more_items": false }] });
        assert_eq!(change_lines(&whole, &versions), ["the collection balances, oldest first, older ones not shown:", "#2     run        1970-01-01T00:00:00  tick  failed  bb   b: 5 → nothing"]);
    }
}
