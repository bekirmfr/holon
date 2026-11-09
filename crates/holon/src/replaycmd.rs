//! `holon replay <file>`, and `holon mcp`'s `replay`: a flow's new version run, offline, on a
//! deployment's recent runs before it is deployed (verify/replay.rs, `GET
//! /deployments/<slug>/replay`), and what it does with them said a line a run: how many it answers
//! the same, and how each other run differs or why it can't be told. The version's units are
//! staged (`POST /unit`); nothing is deployed, sent or written.

use holon_author::agent;
use serde_json::Value;

use crate::deploycmd::{call, Person};
use crate::local;
use crate::mcp::{seg, Api, Body};

const USAGE: &str = "usage: holon replay <file> [options]

  Runs a new version of a deployed flow, offline, on the deployment's recent runs, and says
  which it answers the same and how the others differ, before anything is deployed. Each request
  the version sends is answered as that run was answered, so nothing is sent or written; a run
  where it asks what the run never did, or computes with a secret, can't be told. Exits 0 when
  no run differs, 1 when one does (or it can't replay), 2 on a wrong argument.

  --on SLUG                the deployment to replay on (default: the flow's own name)
  --runs N                 how many of its newest runs (default 20, at most 200)
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon replay shop.holon.ts
  holon replay shop.holon.ts --on shop-staging --runs 50

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

pub(crate) fn run(args: &[String]) -> i32 {
    let (mut file, mut on, mut runs, mut server) = (None::<String>, None::<String>, None::<u64>, None::<String>);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--on" => on = args.next().cloned(),
            "--server" => server = args.next().cloned(),
            "--runs" => match args.next().map(|n| n.parse::<u64>()) {
                Some(Ok(n)) if (1..=200).contains(&n) => runs = Some(n),
                _ => {
                    eprintln!("holon replay: --runs takes a number of runs, 1 to 200");
                    return 2;
                }
            },
            "--help" | "-h" => {
                println!("{USAGE}");
                return 0;
            }
            _ if file.is_none() && !a.starts_with('-') => file = Some(a.clone()),
            _ => {
                eprintln!("holon replay: no option {a:?}\n\n{USAGE}");
                return 2;
            }
        }
    }
    let Some(file) = file else {
        eprintln!("{USAGE}");
        return 2;
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("{file}: no such file");
            return 1;
        }
        Err(e) => {
            eprintln!("{file}: {e}");
            return 1;
        }
    };
    let target = match local::target(server.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("holon replay: {e}");
            return 1;
        }
    };
    let api = Person { server: target.server, token: target.token };
    match replay(&api, &text, on.as_deref(), runs) {
        Ok(said) => {
            println!("{}", said.text);
            i32::from(said.differ > 0)
        }
        Err(e) => {
            eprintln!("holon replay: {file}: {e}");
            1
        }
    }
}

/// What a replay said: its lines, and how many runs differ.
pub(crate) struct Said {
    pub(crate) text: String,
    pub(crate) differ: u64,
}

/// `text` compiled, its units staged on `api`, and replayed on the deployment `on` (the flow's
/// own name unless given), its newest `runs` (the server's 20 unless given).
pub(crate) fn replay(api: &dyn Api, text: &str, on: Option<&str>, runs: Option<u64>) -> Result<Said, String> {
    let plan = agent::plan(text)?;
    let slug = on.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&plan.name).to_string();
    for unit in &plan.units {
        call(api, "POST", "/unit", Some(Body::Bytes(unit.clone()))).map_err(|e| format!("staging a unit: {e}"))?;
    }
    let runs = runs.map(|n| format!("&runs={n}")).unwrap_or_default();
    let r = call(api, "GET", &format!("/deployments/{}/replay?unit={}{runs}", seg(&slug), plan.unit), None)?;
    Ok(Said { text: lines(&slug, &r).join("\n"), differ: count(&r, "differ") })
}

fn count(r: &Value, k: &str) -> u64 {
    r.get(k).and_then(Value::as_u64).unwrap_or(0)
}

/// A replay's answer as lines: the count, then each run that differs or can't be told, newest
/// first.
pub(crate) fn lines(slug: &str, r: &Value) -> Vec<String> {
    let replayed = count(r, "replayed");
    if replayed == 0 {
        return vec![format!("\"{slug}\" has no runs yet to replay this version on.")];
    }
    let mut summary = vec![format!("{replayed} replayed"), format!("{} same", count(r, "same"))];
    match count(r, "differ") {
        0 => {}
        1 => summary.push("1 differs".into()),
        n => summary.push(format!("{n} differ")),
    }
    if count(r, "cant_tell") > 0 {
        summary.push(format!("{} can't tell", count(r, "cant_tell")));
    }
    let mut out = vec![format!(
        "This version replayed on \"{slug}\"'s last {}, offline (nothing sent or written): {}",
        if replayed == 1 { "run".to_string() } else { format!("{replayed} runs") },
        summary.join(" · ")
    )];
    for run in r.get("runs").and_then(Value::as_array).into_iter().flatten() {
        let outcome = run.get("outcome").and_then(Value::as_str).unwrap_or("");
        if outcome == "same" {
            continue;
        }
        let what = run.get("route").and_then(Value::as_str).or(run.get("kind").and_then(Value::as_str)).unwrap_or("?");
        let why: Vec<&str> = run.get("why").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let said = if outcome == "differs" { "differs" } else { "can't tell" };
        out.push(format!("  run {}  {what}  {said}: {}", run.get("seq").and_then(Value::as_u64).unwrap_or(0), why.join("; ")));
    }
    out
}
