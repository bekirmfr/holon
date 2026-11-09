//! A running server's deployments from the command line, as the editor's server panel has them:
//! `holon deployments` lists them; `run`, `pause`, `stop` and `remove` change them; `call` sends
//! one a request, `action` runs one of its actions, `trigger` fires it by hand, `tick` fires one
//! of its clocks now.

use serde_json::{json, Value};

use crate::cmdline::{self, seg, table, when, SIGN_IN};
use crate::local;
use holon_author::agent::show;

const DEPLOYMENTS_USAGE: &str = "usage: holon deployments [<slug>] [--json] [--server PORT|URL]

  Lists what is deployed: each deployment's status, how it starts (its port, its schedule), its
  runs and errors, and the runs waiting for an approval or an event. With a slug, that one's
  details, its waiting runs among them.

  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon deployments
  holon deployments shop
  holon deployments --json
";

const LIFECYCLE_USAGE: &str = "usage: holon run <slug> [--fresh | --session N]
       holon pause <slug>
       holon stop <slug>
       holon remove <slug> --yes
       (each with [--server PORT|URL])

  Starts, holds, ends or deletes a deployment, as the editor's buttons do.

  run                      start it again, from the state it last had
    --fresh                from no state at all
    --session N            from the state session N left (holon state <slug> --sessions)
  pause                    hold its runs; its state is kept
  stop                     end it; run starts it again
  remove                   delete a stopped deployment for good: its runs' log, its timeline,
                           its state and its secrets (holon export keeps its runs first).
                           --yes says you mean it

examples:
  holon pause shop
  holon run shop
  holon run shop --session 2
  holon stop shop && holon remove shop --yes
";

const CALL_USAGE: &str = "usage: holon call <slug> METHOD PATH [options]

  Sends a deployed flow one request, as the editor's request panel does, and prints the body of
  its answer, with its status on stderr. Exits 1 when the status is 400 or more.

  -H \"Name: value\"         a header to send (repeatable; also --header)
  --body TEXT              the body to send; - reads it from stdin
  --body-file FILE         the body to send, read from FILE
  --json                   print the status, headers and body as JSON
  --server PORT|URL        which holon server to ask (see below)

  In Git Bash on Windows, set MSYS_NO_PATHCONV=1, or the shell turns /orders into a path.

examples:
  holon call hello-api GET /hello/Ada
  holon call notes-api PUT /notes/7 --body '{\"text\":\"hi\"}'
  holon call notes-api GET /notes -H \"Accept: application/json\" --json
  echo '{\"op\":\"add\",\"a\":1,\"b\":2}' | holon call calc POST /calc --body -
";

const ACTION_USAGE: &str = "usage: holon action <slug> <name> [options]

  Runs one of a deployment's actions (action charge (input) { … }), as the editor's ▶ does,
  and prints what it answered and the run's number. A run that waits for an approval or an
  event says so. Exits 1 when the action answers 400 or more.

  --input JSON             its input ({} without one); - reads it from stdin
  --input-file FILE        its input, read from FILE
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon action status-board report
  holon action billing charge --input '{\"id\":\"c1\",\"amount\":30}'
  holon action billing refund --input-file refund.json
";

const TICK_USAGE: &str = "usage: holon tick <slug> <clock> [--server PORT|URL]

  Fires one of a flow's clocks now, by its label as holon deployments shows it (\"every 2s\"),
  and prints what it produced and what it did.

  --server PORT|URL        which holon server to ask (see below)

examples:
  holon tick heartbeat \"every 10s\"
  holon tick ticker \"every 2s\"
";

const STATE_USAGE: &str = "usage: holon state <slug> [--json]
       holon state <slug> --collection NAME [--key KEY | --prefix TEXT] [--limit N] [--all]
       holon state <slug> --sessions
       (each with [--server PORT|URL])

  Prints a deployment's live state, a line per value, as the editor's state panel shows it.

  --collection NAME        one of its collections' items instead, 50 at a time
  --key KEY                just that item
  --prefix TEXT            just the items whose key starts with TEXT
  --limit N                how many items at a time (default 50, at most 1000)
  --all                    every item, however many
  --sessions               each session it ran, where it started from, and the state it left
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon state counter
  holon state notes-api --collection notes --prefix 7
  holon state notes-api --collection notes --all --json
  holon state counter --sessions
";

fn usage(text: &str) -> String {
    format!("{text}\n{SIGN_IN}")
}

pub(crate) fn deployments(args: &[String]) -> i32 {
    const WHO: &str = "holon deployments";
    let usage = usage(DEPLOYMENTS_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &[], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let listed = match cmdline::json(WHO, &args, "GET", "/deployments", None) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let all = listed["deployments"].as_array().cloned().unwrap_or_default();
    let Some(slug) = args.word(0) else {
        if args.flag("--json") {
            cmdline::print_json(&listed);
        } else if all.is_empty() {
            println!("nothing deployed");
        } else {
            for line in list_lines(&all) {
                println!("{line}");
            }
        }
        return 0;
    };
    let Some(one) = all.iter().find(|d| d["slug"] == slug) else {
        eprintln!("{WHO}: no deployment {slug:?} (holon deployments lists them)");
        return 1;
    };
    let waiting = match cmdline::json(WHO, &args, "GET", &format!("/deployments/{}/runs", seg(slug)), None) {
        Ok(v) => v["runs"].as_array().cloned().unwrap_or_default(),
        Err(code) => return code,
    };
    if args.flag("--json") {
        cmdline::print_json(&json!({ "deployment": one, "waiting": waiting }));
        return 0;
    }
    for line in detail_lines(one, &waiting) {
        println!("{line}");
    }
    0
}

/// How a deployment starts: its port, or its triggers.
fn starts(d: &Value) -> String {
    if let Some(port) = d["port"].as_u64() {
        return format!(":{port}");
    }
    d["triggers"].as_array().map(|t| t.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

pub(crate) fn list_lines(all: &[Value]) -> Vec<String> {
    let mut rows = vec![["slug", "status", "starts", "runs", "errors", "waiting", "deployed"].map(String::from).to_vec()];
    for d in all {
        let mut status = d["status"].as_str().unwrap_or_default().to_string();
        if d["attention"].is_string() || d["listen_error"].is_string() {
            status.push_str(" !");
        }
        rows.push(vec![
            d["slug"].as_str().unwrap_or_default().to_string(),
            status,
            starts(d),
            d["trigger_count"].to_string(),
            d["error_count"].to_string(),
            d["waiting"].to_string(),
            format!("{}{}", when(d["deployed_at_epoch_seconds"].as_u64()), d["deployed_by"].as_str().map(|b| format!(" by {b}")).unwrap_or_default()),
        ]);
    }
    let mut lines = table(&rows);
    if all.iter().any(|d| d["attention"].is_string() || d["listen_error"].is_string()) {
        lines.push(String::new());
        lines.push("!: needs attention (holon deployments <slug> says why)".into());
    }
    lines
}

pub(crate) fn detail_lines(d: &Value, waiting: &[Value]) -> Vec<String> {
    let s = |k: &str| d[k].as_str().unwrap_or_default().to_string();
    let mut lines = vec![
        format!("{}: {}", s("slug"), s("status")),
        format!("  starts     {}", starts(d)),
        format!("  unit       {}", &s("unit")[..s("unit").len().min(12)]),
        format!("  runs       {} ({} failed)", d["trigger_count"], d["error_count"]),
        format!("  session    {} of {}", d["session"], d["sessions"]),
        format!("  deployed   {}{}", when(d["deployed_at_epoch_seconds"].as_u64()), d["deployed_by"].as_str().map(|b| format!(" by {b}")).unwrap_or_default()),
    ];
    if d["has_state"] == true {
        lines.push(format!("  state      holon state {}", s("slug")));
    }
    if let Some(why) = d["attention"].as_str() {
        lines.push(format!("  attention  {why}"));
    }
    if let Some(why) = d["listen_error"].as_str() {
        lines.push(format!("  listening  {why}"));
    }
    if waiting.is_empty() {
        lines.push("  waiting    no run".into());
    } else {
        lines.push(format!("  waiting    {} run(s):", waiting.len()));
        for r in waiting {
            let on: Vec<String> = r["waiting"].as_array().into_iter().flatten().map(|w| format!("{} (until {})", w["name"].as_str().unwrap_or_default(), when(w["expires_at"].as_u64()))).collect();
            lines.push(format!("    {}  {} {}  since {}  waits for {}", r["run_id"].as_str().unwrap_or_default(), r["kind"].as_str().unwrap_or_default(), r["route"].as_str().unwrap_or_default(), when(r["parked_at"].as_u64()), on.join(", ")));
        }
        lines.push("    (each is approved by its own link, on the flow's port)".into());
    }
    lines
}

pub(crate) fn lifecycle(action: &str, args: &[String]) -> i32 {
    let who = format!("holon {action}");
    let usage = usage(LIFECYCLE_USAGE);
    let args = match cmdline::parse(&who, &usage, args, &["--session"], &["--fresh", "--yes"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let Some(slug) = args.word(0) else {
        eprintln!("{usage}");
        return 2;
    };
    let path = format!("/deployments/{}", seg(slug));
    let answer = match action {
        "run" => {
            let from = match (args.flag("--fresh"), args.value("--session")) {
                (true, Some(_)) => {
                    eprintln!("{who}: --fresh or --session, not both");
                    return 2;
                }
                (true, None) => json!({ "from": "fresh" }),
                (false, Some(n)) => match n.parse::<u32>() {
                    Ok(n) => json!({ "from": n }),
                    Err(_) => {
                        eprintln!("{who}: --session takes a session's number (holon state {slug} --sessions lists them)");
                        return 2;
                    }
                },
                (false, None) => json!({}),
            };
            cmdline::json(&who, &args, "POST", &format!("{path}/run"), Some(&from))
        }
        "remove" => {
            if !args.flag("--yes") {
                eprintln!("{who}: this deletes {slug:?} for good: its runs' log, its timeline, its state and its secrets (holon export {slug} keeps its runs first). Run it again with --yes to go ahead.");
                return 2;
            }
            cmdline::json(&who, &args, "DELETE", &path, None)
        }
        _ => cmdline::json(&who, &args, "POST", &format!("{path}/{action}"), None),
    };
    match answer {
        Ok(d) if action == "remove" => {
            println!("{slug}: removed{}", if d["status"].is_string() { ", its secrets forgotten" } else { "" });
            0
        }
        Ok(d) => {
            println!("{slug}: {}{}", d["status"].as_str().unwrap_or("done"), d["attention"].as_str().map(|a| format!(" ({a})")).unwrap_or_default());
            0
        }
        Err(code) => code,
    }
}

pub(crate) fn call(args: &[String]) -> i32 {
    const WHO: &str = "holon call";
    let usage = usage(CALL_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["-H", "--header", "--body", "--body-file"], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (Some(slug), Some(method), Some(path)) = (args.word(0), args.word(1), args.word(2)) else {
        eprintln!("{usage}");
        return 2;
    };
    if !path.starts_with('/') {
        eprintln!("{WHO}: the path starts with / (/hello/ada?loud=1)");
        return 2;
    }
    let mut headers = Vec::new();
    for h in args.all("-H").into_iter().chain(args.all("--header")) {
        let Some((name, value)) = h.split_once(':') else {
            eprintln!("{WHO}: a header is 'Name: value', not {h:?}");
            return 2;
        };
        headers.push(json!([name.trim(), value.trim()]));
    }
    let body = match cmdline::text_from(WHO, &args, "--body") {
        Ok(b) => b.unwrap_or_default(),
        Err(code) => return code,
    };
    let request = json!({ "method": method.to_ascii_uppercase(), "url": path, "headers": headers, "body": body });
    let answer = match cmdline::json(WHO, &args, "POST", &format!("/deployments/{}/request", seg(slug)), Some(&request)) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let status = answer["status"].as_u64().unwrap_or(0);
    if args.flag("--json") {
        cmdline::print_json(&answer);
    } else {
        let body = answer["body"].as_str().unwrap_or_default();
        println!("{body}");
        eprintln!("{WHO}: {status}");
    }
    if status >= 400 {
        1
    } else {
        0
    }
}

pub(crate) fn action(args: &[String]) -> i32 {
    const WHO: &str = "holon action";
    let usage = usage(ACTION_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["--input", "--input-file"], &[]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (Some(slug), Some(name)) = (args.word(0), args.word(1)) else {
        eprintln!("{usage}");
        return 2;
    };
    let input = match cmdline::text_from(WHO, &args, "--input") {
        Ok(i) => i.unwrap_or_else(|| "{}".into()),
        Err(code) => return code,
    };
    if let Err(e) = serde_json::from_str::<Value>(&input) {
        eprintln!("{WHO}: --input is a JSON value ({{\"amount\": 30}}): {e}");
        return 2;
    }
    let answer = match local::send(WHO, args.server(), "POST", &format!("/trigger/{}/{}", seg(slug), seg(name)), Some(("application/json", input.into_bytes()))) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (lines, code) = action_lines(answer.status, &answer.body);
    for (to_err, line) in lines {
        if to_err {
            eprintln!("{WHO}: {line}");
        } else {
            println!("{line}");
        }
    }
    code
}

/// What an action's answer says: lines (`true` for stderr's) and the exit code.
pub(crate) fn action_lines(status: u16, body: &str) -> (Vec<(bool, String)>, i32) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let seq = v["seq"].as_u64().map(|s| format!("run #{s}")).unwrap_or_else(|| "the run".into());
    if let Some(why) = v["error"].as_str() {
        return (vec![(true, format!("answered {status}: {why}"))], 1);
    }
    if v.get("answer").is_none() {
        return (vec![(true, format!("answered {status}: {}", body.trim()))], 1);
    }
    if v["waiting"] == true {
        return (vec![(true, format!("{seq} is waiting for an approval or an event (holon deployments <slug> lists it)"))], 0);
    }
    let shown = serde_json::to_string_pretty(&v["answer"]).unwrap_or_default();
    if (200..300).contains(&status) {
        (vec![(false, shown), (true, seq)], 0)
    } else {
        (vec![(false, shown), (true, format!("{seq} answered {status}"))], 1)
    }
}

pub(crate) fn tick(args: &[String]) -> i32 {
    const WHO: &str = "holon tick";
    let usage = usage(TICK_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &[], &[]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (Some(slug), Some(clock)) = (args.word(0), args.word(1)) else {
        eprintln!("{usage}");
        return 2;
    };
    match cmdline::json(WHO, &args, "POST", &format!("/deployments/{}/tick", seg(slug)), Some(&json!({ "clock": clock }))) {
        Ok(v) => {
            for line in produced_lines(&v) {
                println!("{line}");
            }
            0
        }
        Err(code) => code,
    }
}

pub(crate) fn state(args: &[String]) -> i32 {
    const WHO: &str = "holon state";
    let usage = usage(STATE_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["--collection", "--key", "--prefix", "--limit"], &["--all", "--sessions", "--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let Some(slug) = args.word(0) else {
        eprintln!("{usage}");
        return 2;
    };
    let base = format!("/deployments/{}", seg(slug));
    if args.flag("--sessions") {
        return match cmdline::json(WHO, &args, "GET", &format!("{base}/sessions"), None) {
            Ok(v) if args.flag("--json") => {
                cmdline::print_json(&v);
                0
            }
            Ok(v) => {
                for line in session_lines(slug, &v) {
                    println!("{line}");
                }
                0
            }
            Err(code) => code,
        };
    }
    if let Some(name) = args.value("--collection") {
        return collection(WHO, &args, &base, name);
    }
    match cmdline::json(WHO, &args, "GET", &format!("{base}/state"), None) {
        Ok(v) if args.flag("--json") => {
            cmdline::print_json(&v);
            0
        }
        Ok(v) => {
            for line in state_lines(slug, &v["state"]) {
                println!("{line}");
            }
            0
        }
        Err(code) => code,
    }
}

fn collection(who: &str, args: &cmdline::Args, base: &str, name: &str) -> i32 {
    let path = format!("{base}/collections/{}", seg(name));
    if let Some(key) = args.value("--key") {
        return match cmdline::json(who, args, "GET", &format!("{path}?key={}", seg(key)), None) {
            Ok(v) if args.flag("--json") => {
                cmdline::print_json(&v);
                0
            }
            Ok(v) => match v["items"].as_array().and_then(|i| i.first()) {
                Some(item) => {
                    println!("{}", show(&item[1]));
                    0
                }
                None => {
                    eprintln!("{who}: {name} has no {key:?}");
                    1
                }
            },
            Err(code) => code,
        };
    }
    let limit = match args.value("--limit").map(str::parse::<u32>) {
        None => 50,
        Some(Ok(n)) if (1..=1000).contains(&n) => n,
        Some(_) => {
            eprintln!("{who}: --limit is 1 to 1000");
            return 2;
        }
    };
    let prefix = args.value("--prefix").unwrap_or_default();
    let (mut after, mut items, mut root) = (None::<String>, Vec::new(), Value::Null);
    loop {
        let mut page = format!("{path}?limit={limit}&prefix={}", seg(prefix));
        if let Some(a) = &after {
            page.push_str(&format!("&after={}", seg(a)));
        }
        // Every page of one listing reads the root the first one did, not a newer one.
        if let Some(r) = root.as_str() {
            page.push_str(&format!("&root={r}"));
        }
        let v = match cmdline::json(who, args, "GET", &page, None) {
            Ok(v) => v,
            Err(code) => return code,
        };
        root = v["root"].clone();
        items.extend(v["items"].as_array().cloned().unwrap_or_default());
        after = v["next"].as_str().map(String::from);
        if after.is_none() || !args.flag("--all") {
            break;
        }
    }
    if args.flag("--json") {
        cmdline::print_json(&json!({ "root": root, "items": items, "next": after }));
        return 0;
    }
    let rows: Vec<Vec<String>> = items.iter().map(|i| vec![i[0].as_str().unwrap_or_default().to_string(), show(&i[1])]).collect();
    for line in table(&rows) {
        println!("{line}");
    }
    if items.is_empty() {
        eprintln!("{who}: no item{}", if prefix.is_empty() { String::new() } else { format!(" starting {prefix:?}") });
    } else if let Some(a) = after {
        eprintln!("{who}: more after {a:?} (--all for every one)");
    }
    0
}

/// A state's values, a line each; its collections by name, with how to list them.
pub(crate) fn state_lines(slug: &str, state: &Value) -> Vec<String> {
    let Some(cells) = state.as_object() else { return vec![] };
    if cells.is_empty() {
        return vec!["nothing kept yet".into()];
    }
    let mut rows = Vec::new();
    let mut collections = Vec::new();
    for (k, v) in cells {
        match k.strip_prefix("#collection:") {
            Some(name) => collections.push(name),
            None => rows.push(vec![k.clone(), show(v)]),
        }
    }
    for name in collections {
        rows.push(vec![name.to_string(), format!("a collection (holon state {slug} --collection {name})")]);
    }
    table(&rows)
}

pub(crate) fn session_lines(slug: &str, v: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    for s in v["sessions"].as_array().into_iter().flatten() {
        let ended = match s["ended_at"].as_u64() {
            Some(e) => format!("to {}", when(Some(e))),
            None => format!("({})", v["status"].as_str().unwrap_or("now")),
        };
        lines.push(format!("session {}  from {}  {} {}  {} run(s), {} failed", s["number"], s["from"].as_str().unwrap_or_default(), when(s["started_at"].as_u64()), ended, s["requests"], s["errors"]));
        if let Some(why) = s["state_error"].as_str() {
            lines.push(format!("  its state can't be read: {why}"));
        }
        if s["state"].is_object() {
            lines.extend(state_lines(slug, &s["state"]).into_iter().map(|l| format!("  {l}")));
        }
    }
    if lines.is_empty() {
        lines.push("no session yet".into());
    }
    lines
}

/// A run's outputs and what it did (`{outputs, performed}`), a line each: an output that is an
/// effect is said once, as what it did.
pub(crate) fn produced_lines(v: &Value) -> Vec<String> {
    let mut lines: Vec<String> = v["outputs"].as_array().into_iter().flatten().filter(|o| o["type"] != "Effect").map(|o| format!("output     {}", show(o))).collect();
    lines.extend(v["performed"].as_array().into_iter().flatten().map(|p| format!("did        {} {}", p["capability"].as_str().unwrap_or_default(), show(&p["payload"]))));
    if lines.is_empty() {
        lines.push("it ran: no output, nothing done outside".into());
    }
    lines
}

const RUNS_USAGE: &str = "usage: holon runs <slug> [--last N] [--waiting] [--json] [--server PORT|URL]

  A deployment's runs, oldest first: what started each, its answer, how long it took, and why it
  failed. --waiting lists the runs parked for an event instead: what each waits for, and until when.

  --last N                 how many runs, the newest (default 10, at most 200)
  --waiting                the runs waiting for an event, not the log
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon runs shop
  holon runs shop --last 50 --json
  holon runs shop --waiting
";

/// One waiting run as a line: its id, what it is (its route, else its kind), and what it waits for.
fn waiting_line(run: &Value) -> String {
    let waits = run["waiting"].as_array().into_iter().flatten().map(|w| format!("{} until {}", w["name"].as_str().unwrap_or("?"), w["expires_at"])).collect::<Vec<_>>().join(", ");
    let what = run["route"].as_str().or(run["kind"].as_str()).unwrap_or("run");
    format!("{}  {what}  waits for {waits}", run["run_id"].as_str().unwrap_or("?"))
}

pub(crate) fn runs(args: &[String]) -> i32 {
    const WHO: &str = "holon runs";
    let usage = usage(RUNS_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["--last"], &["--waiting", "--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let Some(slug) = args.word(0) else {
        eprintln!("{usage}");
        return 2;
    };
    let last = match args.value("--last").map(str::parse::<u32>) {
        None => 10,
        Some(Ok(n)) if (1..=200).contains(&n) => n,
        Some(_) => {
            eprintln!("{WHO}: --last is 1 to 200");
            return 2;
        }
    };
    let path = format!("/deployments/{}", seg(slug));
    if args.flag("--waiting") {
        let waiting = match cmdline::json(WHO, &args, "GET", &format!("{path}/runs"), None) {
            Ok(v) => v,
            Err(code) => return code,
        };
        if args.flag("--json") {
            cmdline::print_json(&waiting);
        } else if waiting["runs"].as_array().map_or(true, |r| r.is_empty()) {
            println!("{slug} has no runs waiting for an event");
        } else {
            for run in waiting["runs"].as_array().into_iter().flatten() {
                println!("{}", waiting_line(run));
            }
        }
        return 0;
    }
    let log = match cmdline::json(WHO, &args, "GET", &format!("{path}/log?limit={last}"), None) {
        Ok(v) => v,
        Err(code) => return code,
    };
    if args.flag("--json") {
        cmdline::print_json(&log);
        return 0;
    }
    let entries = log["entries"].as_array().cloned().unwrap_or_default();
    if entries.is_empty() {
        println!("{slug} has no runs yet");
    }
    for entry in &entries {
        println!("{}", holon_author::agent::run_line(entry));
    }
    if let Some(breaks) = log["breaks"].as_array().filter(|b| !b.is_empty()) {
        println!("The log was changed outside holon before runs {}: their record can't be trusted.", breaks.iter().map(Value::to_string).collect::<Vec<_>>().join(", "));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_runs_say_what_they_wait_for() {
        let run = json!({ "run_id": "rt_1", "kind": "request", "route": "POST /orders", "parked_at": 0, "waiting": [{ "name": "approval", "expires_at": 60 }] });
        assert_eq!(waiting_line(&run), "rt_1  POST /orders  waits for approval until 60");
    }


    #[test]
    fn deployments_list_as_columns_and_mark_what_needs_attention() {
        let all = [
            json!({ "slug": "shop", "status": "running", "port": 8101, "trigger_count": 12, "error_count": 1, "waiting": 0, "deployed_at_epoch_seconds": 0, "deployed_by": "maria" }),
            json!({ "slug": "beats", "status": "paused", "port": null, "triggers": ["every 10s"], "trigger_count": 3, "error_count": 0, "waiting": 2, "deployed_at_epoch_seconds": 0, "attention": "it may no longer read the clock" }),
        ];
        assert_eq!(
            list_lines(&all),
            [
                "slug   status    starts     runs  errors  waiting  deployed",
                "shop   running   :8101      12    1       0        1970-01-01T00:00:00 by maria",
                "beats  paused !  every 10s  3     0       2        1970-01-01T00:00:00",
                "",
                "!: needs attention (holon deployments <slug> says why)",
            ]
        );
    }

    #[test]
    fn one_deployment_says_why_it_needs_attention_and_what_waits() {
        let d = json!({ "slug": "approval", "status": "running", "port": 8120, "unit": "abcdef0123456789", "trigger_count": 4, "error_count": 0, "session": 2, "sessions": 2, "deployed_at_epoch_seconds": 0, "has_state": true });
        let waiting = [json!({ "run_id": "rt_1", "kind": "request", "route": "POST /orders", "parked_at": 0, "waiting": [{ "name": "approval", "expires_at": 60 }] })];
        let lines = detail_lines(&d, &waiting);
        assert_eq!(lines[0], "approval: running");
        assert!(lines.contains(&"  unit       abcdef012345".to_string()), "{lines:?}");
        assert!(lines.contains(&"  state      holon state approval".to_string()), "{lines:?}");
        assert!(lines.contains(&"    rt_1  request POST /orders  since 1970-01-01T00:00:00  waits for approval (until 1970-01-01T00:01:00)".to_string()), "{lines:?}");
    }

    #[test]
    fn an_actions_answer_says_its_run_and_whether_it_waits() {
        let (lines, code) = action_lines(200, r#"{"answer": {"total": 105}, "seq": 7}"#);
        assert_eq!((lines, code), (vec![(false, "{\n  \"total\": 105\n}".to_string()), (true, "run #7".to_string())], 0));
        let (lines, code) = action_lines(202, r#"{"answer": null, "seq": 8, "waiting": true}"#);
        assert_eq!(code, 0);
        assert!(lines[0].1.starts_with("run #8 is waiting"), "{lines:?}");
        let (lines, code) = action_lines(409, r#"{"answer": {"why": "too much"}, "seq": 9}"#);
        assert_eq!((lines[1].1.as_str(), code), ("run #9 answered 409", 1));
        let (lines, code) = action_lines(404, r#"{"error": "no action \"x\""}"#);
        assert_eq!((lines, code), (vec![(true, "answered 404: no action \"x\"".to_string())], 1));
    }

    #[test]
    fn what_a_run_produced_and_did_shows_as_the_language_writes_values() {
        let v = json!({ "outputs": [{ "type": "Number", "value": 3.0 }, null, { "type": "Effect", "value": { "sink": "store_write:a", "payload": { "type": "Text", "value": "hi" } } }], "performed": [{ "capability": "StateWrite(\"a\")", "payload": { "type": "Text", "value": "hi" } }] });
        assert_eq!(produced_lines(&v), ["output     3", "output     nothing", "did        StateWrite(\"a\") \"hi\""]);
        assert_eq!(produced_lines(&json!({ "outputs": [], "performed": [] })), ["it ran: no output, nothing done outside"]);
    }

    #[test]
    fn state_shows_its_values_and_names_its_collections() {
        let state = json!({ "count": { "type": "Number", "value": 4.0 }, "#collection:orders": { "type": "Text", "value": "ab12" }, "name": { "type": "Text", "value": "ada" } });
        assert_eq!(state_lines("shop", &state), ["count   4", "name    \"ada\"", "orders  a collection (holon state shop --collection orders)"]);
        assert_eq!(state_lines("shop", &json!({})), ["nothing kept yet"]);
        let sessions = json!({ "status": "running", "sessions": [
            { "number": 1, "from": "fresh", "started_at": 0, "ended_at": 60, "requests": 3, "errors": 0, "state": { "count": { "type": "Number", "value": 3.0 } } },
            { "number": 2, "from": "session 1", "started_at": 60, "ended_at": null, "requests": 1, "errors": 1 },
        ] });
        assert_eq!(
            session_lines("shop", &sessions),
            [
                "session 1  from fresh  1970-01-01T00:00:00 to 1970-01-01T00:01:00  3 run(s), 0 failed",
                "  count  3",
                "session 2  from session 1  1970-01-01T00:01:00 (running)  1 run(s), 1 failed",
            ]
        );
    }
}
