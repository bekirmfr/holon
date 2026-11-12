//! `holon decide`: a person signed in at a terminal approves, or declines with a reason, an
//! approval a run waits at (`await approval(…)`, durable.rs's decide route); with a slug alone,
//! the approvals its runs wait at. And how a waiting run reads, a line a point, for it and
//! `holon mcp`'s `runs`. Only a person decides: the server refuses an agent and an API token.

use serde_json::{json, Value};

use crate::cmdline::{self, seg, SIGN_IN};

const USAGE: &str = "usage: holon decide <slug> [<run> <name> --approve|--decline] [options]

  Decides an approval a run of a deployment waits at (await approval(…) in its flow): approve it,
  or decline it with a reason. An approval asking for several people waits until that many
  different people approve it; one decline ends it. Each person counts once. With the slug alone,
  it lists what the deployment's runs wait at, and who has approved each so far.

  <run>                    the run, by its id or the first characters of it, as listed
  <name>                   the approval's name, as the flow writes it
  --approve                approve it (a reason is optional)
  --decline                decline it (a reason is needed)
  --reason TEXT            why, kept with the run
  --json                   print the server's answer as JSON
  --server PORT|URL        which holon server to ask (see below)

  A person decides: an editor or an admin, signed in. An agent, or an API token, is refused.

examples:
  holon decide payouts
  holon decide payouts 7b379ca1dd65 payout --approve
  holon decide payouts 7b379ca1dd65 payout --decline --reason \"not this vendor\"
";

pub(crate) fn run(args: &[String]) -> i32 {
    const WHO: &str = "holon decide";
    let usage = format!("{USAGE}\n{SIGN_IN}");
    let args = match cmdline::parse(WHO, &usage, args, &["--reason"], &["--approve", "--decline", "--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let Some(slug) = args.word(0) else {
        eprintln!("{usage}");
        return 2;
    };
    let listed = match cmdline::json(WHO, &args, "GET", &format!("/deployments/{}/runs", seg(slug)), None) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let runs = listed["runs"].as_array().cloned().unwrap_or_default();
    let (Some(run), Some(name)) = (args.word(1), args.word(2)) else {
        if args.word(1).is_some() {
            eprintln!("{WHO}: give the approval's name after the run\n\n{usage}");
            return 2;
        }
        if args.flag("--json") {
            cmdline::print_json(&listed);
            return 0;
        }
        let lines = waiting_lines(&runs);
        if lines.is_empty() {
            println!("No run of {slug} is waiting.");
        }
        for line in lines {
            println!("{line}");
        }
        return 0;
    };
    let approve = match (args.flag("--approve"), args.flag("--decline")) {
        (true, false) => true,
        (false, true) => false,
        _ => {
            eprintln!("{WHO}: say --approve or --decline\n\n{usage}");
            return 2;
        }
    };
    let reason = args.value("--reason").map(str::trim).filter(|r| !r.is_empty());
    if !approve && reason.is_none() {
        eprintln!("{WHO}: a decline says why: give --reason");
        return 2;
    }
    let run_id = match pick(&runs, run) {
        Ok(id) => id,
        Err(why) => {
            eprintln!("{WHO}: {why}");
            return 1;
        }
    };
    let body = json!({ "name": name, "approve": approve, "reason": reason });
    let d = match cmdline::json(WHO, &args, "POST", &format!("/deployments/{}/runs/{}/decide", seg(slug), seg(&run_id)), Some(&body)) {
        Ok(v) => v,
        Err(code) => return code,
    };
    if args.flag("--json") {
        cmdline::print_json(&d);
        return 0;
    }
    println!("{}", decided_said(slug, &run_id, &d));
    if let Some(e) = d["run_error"].as_str() {
        eprintln!("{WHO}: the run then failed: {e}");
    }
    0
}

/// The run of `runs` whose id is, or starts with, `given`; or why there is none to decide.
fn pick(runs: &[Value], given: &str) -> Result<String, String> {
    let ids: Vec<&str> = runs.iter().filter_map(|r| r["run_id"].as_str()).filter(|id| id.starts_with(given)).collect();
    match ids.as_slice() {
        [id] => Ok(id.to_string()),
        [] => Err(format!("no run {given} is waiting: it may have been decided, or finished (holon decide <slug> lists what waits)")),
        _ => Err(format!("{} waiting runs start with {given}: give more of its id", ids.len())),
    }
}

/// What a decision did, from the route's answer.
fn decided_said(slug: &str, run_id: &str, d: &Value) -> String {
    let name = d["name"].as_str().unwrap_or("?");
    let people = d["people"].as_u64().unwrap_or(1);
    let by: Vec<&str> = d["approved_by"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let run = format!("run {} of {slug}", short(run_id));
    match d["outcome"].as_str() {
        Some("waiting") => {
            let more = people.saturating_sub(by.len() as u64);
            format!("Approved {name:?} for {run}: {} of {people}. It waits for {more} more {}.", by.len(), if more == 1 { "person" } else { "people" })
        }
        Some("approved") => format!("Approved {name:?} by {}: {run} goes on.", by.join(" and ")),
        _ => format!("Declined {name:?}: {run} goes on as declined."),
    }
}

/// A run's id as it is listed: its first 12 characters, enough to pick it.
fn short(run_id: &str) -> &str {
    &run_id[..run_id.len().min(12)]
}

/// What runs wait at, a line a point: the run, what started it and since when, then an approval
/// with how many people it asks and who has approved it (and why), or an event, and until when.
pub(crate) fn waiting_lines(runs: &[Value]) -> Vec<String> {
    let mut lines = Vec::new();
    for r in runs {
        let id = short(r["run_id"].as_str().unwrap_or("?"));
        let what = r["route"].as_str().or(r["kind"].as_str()).unwrap_or("?");
        let since = cmdline::when(r["parked_at"].as_u64());
        for w in r["waiting"].as_array().into_iter().flatten() {
            let name = w["name"].as_str().unwrap_or("?");
            let until = cmdline::when(w["expires_at"].as_u64());
            let people = w["people"].as_u64().unwrap_or(0);
            let point = if people == 0 {
                format!("event {name:?}")
            } else {
                let approved: Vec<String> = w["decisions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|d| d["approved"] == true)
                    .map(|a| {
                        let by = a["who"].as_str().unwrap_or("?");
                        match a["reason"].as_str() {
                            Some(why) => format!("{by} ({why:?})"),
                            None => by.to_string(),
                        }
                    })
                    .collect();
                let asks = if people == 1 { "1 person's".to_string() } else { format!("{people} people's") };
                let so_far = if approved.is_empty() { "none approved yet".to_string() } else { format!("{} of {people}: {} approved", approved.len(), approved.join(", ")) };
                format!("approval {name:?}, {asks}, {so_far}")
            };
            lines.push(format!("run {id} ({what}, since {since}) waits at {point}, until {until}"));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiting_run_says_who_approved_it_so_far() {
        let runs = [json!({
            "run_id": "7b379ca1dd65e73579a1f00aa88cf5497be604fd06f3b60a12a5f86fa80c7791",
            "kind": "action",
            "route": "action pay",
            "parked_at": 0,
            "waiting": [
                { "name": "payout", "expires_at": 86400, "people": 2, "decisions": [{ "slot": 0, "who": "dan", "kind": "person", "at": 5, "approved": true, "reason": "matches the invoice" }] },
                { "name": "paid", "expires_at": 60 }
            ],
            "delivered": []
        })];
        assert_eq!(
            waiting_lines(&runs),
            [
                "run 7b379ca1dd65 (action pay, since 1970-01-01T00:00:00) waits at approval \"payout\", 2 people's, 1 of 2: dan (\"matches the invoice\") approved, until 1970-01-02T00:00:00",
                "run 7b379ca1dd65 (action pay, since 1970-01-01T00:00:00) waits at event \"paid\", until 1970-01-01T00:01:00",
            ]
        );
    }

    #[test]
    fn a_run_is_picked_by_the_start_of_its_id() {
        let runs = [json!({ "run_id": "abc123" }), json!({ "run_id": "abd456" })];
        assert_eq!(pick(&runs, "abc"), Ok("abc123".to_string()));
        assert!(pick(&runs, "ab").unwrap_err().starts_with("2 waiting runs"));
        assert!(pick(&runs, "x").unwrap_err().starts_with("no run x is waiting"));
    }

    #[test]
    fn a_decision_says_what_it_did() {
        let d = |outcome: &str, by: &[&str]| json!({ "name": "payout", "people": 2, "approved_by": by, "outcome": outcome });
        assert_eq!(decided_said("pay", "7b379ca1dd65e735", &d("waiting", &["dan"])), "Approved \"payout\" for run 7b379ca1dd65 of pay: 1 of 2. It waits for 1 more person.");
        assert_eq!(decided_said("pay", "7b379ca1dd65e735", &d("approved", &["dan", "ada"])), "Approved \"payout\" by dan and ada: run 7b379ca1dd65 of pay goes on.");
        assert_eq!(decided_said("pay", "7b379ca1dd65e735", &d("declined", &[])), "Declined \"payout\": run 7b379ca1dd65 of pay goes on as declined.");
    }
}
