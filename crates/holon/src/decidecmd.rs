//! `holon decide`: someone signed in at a terminal approves, or declines with a reason, a step
//! of an approval a run waits at (`await approval(…)`, durable.rs's decide route); with a slug
//! alone, the approvals its runs wait at. And how a waiting run reads, a line a point, for it and
//! `holon mcp`'s `runs`. A person decides a person's step and an agent an agent's; the server
//! refuses an API token.

use serde_json::{json, Value};

use crate::cmdline::{self, seg, SIGN_IN};

const USAGE: &str = "usage: holon decide <slug> [<run> <name> --approve|--decline] [options]

  Decides an approval a run of a deployment waits at (await approval(…) in its flow): approve it,
  or decline it with a reason. An approval is decided a step at a time, in its order: you take
  the open step you may fill, of your kind (a person's or an agent's) and holding its role where
  the flow is. A step of several people waits until that many approve it; one decline ends the
  approval. Each decides one step, once, and not of a run they started. With the slug alone, it
  lists what the deployment's runs wait at, and who has approved each step so far.

  <run>                    the run, by its id or the first characters of it, as listed
  <name>                   the approval's name, as the flow writes it
  --approve                approve it (a reason is optional)
  --decline                decline it (a reason is needed)
  --reason TEXT            why, kept with the run
  --json                   print the server's answer as JSON
  --server PORT|URL        which holon server to ask (see below)

  A person decides a person's step, an agent an agent's; an API token is refused.

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

/// What a decision did, from the route's answer: the step it took, and what the approval waits
/// for now, or that it is decided.
fn decided_said(slug: &str, run_id: &str, d: &Value) -> String {
    let name = d["name"].as_str().unwrap_or("?");
    let slots = d["slots"].as_array().cloned().unwrap_or_default();
    let run = format!("run {} of {slug}", short(run_id));
    let taken = slots.get(d["slot"].as_u64().unwrap_or(0) as usize);
    let at = match (taken.and_then(|s| s["role"].as_str()), slots.len()) {
        (Some(role), _) => format!(" as {role}"),
        (None, 1) => String::new(),
        (None, _) => format!(" at slot {}", d["slot"]),
    };
    let by = |s: &Value| -> Vec<String> { s["by"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect() };
    let people = |s: &Value| s["people"].as_u64().unwrap_or(1);
    match d["outcome"].as_str() {
        Some("waiting") => match slots.as_slice() {
            [one] => {
                let (so_far, of) = (by(one).len() as u64, people(one));
                let more = of.saturating_sub(so_far);
                format!("Approved {name:?} for {run}: {so_far} of {of}. It waits for {more} more {}.", if more == 1 { "person" } else { "people" })
            }
            _ => {
                let unfilled: Vec<String> = slots
                    .iter()
                    .filter(|s| (by(s).len() as u64) < people(s))
                    .map(|s| match people(s) {
                        1 => step_name(s),
                        of => format!("{} ({} of {of})", step_name(s), by(s).len()),
                    })
                    .collect();
                format!("Approved {name:?}{at} for {run}. It waits for {}.", and_list(&unfilled))
            }
        },
        Some("approved") => format!("Approved {name:?}{at}, by {}: {run} goes on.", and_list(&slots.iter().flat_map(by).collect::<Vec<_>>())),
        _ => format!("Declined {name:?}{at}: {run} goes on as declined."),
    }
}

/// A step as a line names it: by its role, or its number.
fn step_name(s: &Value) -> String {
    s["role"].as_str().map_or_else(|| format!("slot {}", s["slot"]), String::from)
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A run's id as it is listed: its first 12 characters, enough to pick it.
fn short(run_id: &str) -> &str {
    &run_id[..run_id.len().min(12)]
}

/// What runs wait at, a line a point: the run, what started it and since when, then an approval
/// with who decides it and each step as it stands, who approved it (and why) or whether it is
/// open; or an event; and until when.
pub(crate) fn waiting_lines(runs: &[Value]) -> Vec<String> {
    let mut lines = Vec::new();
    for r in runs {
        let id = short(r["run_id"].as_str().unwrap_or("?"));
        let what = r["route"].as_str().or(r["kind"].as_str()).unwrap_or("?");
        let since = cmdline::when(r["parked_at"].as_u64());
        for w in r["waiting"].as_array().into_iter().flatten() {
            let name = w["name"].as_str().unwrap_or("?");
            let until = cmdline::when(w["expires_at"].as_u64());
            let point = match w["steps"].as_str() {
                None => format!("event {name:?}"),
                Some(steps) => format!("approval {name:?} ({}): {}", w["said"].as_str().unwrap_or(steps), steps_said(w)),
            };
            lines.push(format!("run {id} ({what}, since {since}) waits at {point}, until {until}"));
        }
    }
    lines
}

/// An approval point's steps as they stand: each by its role (or its number; a lone step without
/// a role by count), who approved it (and why), or whether it is open.
fn steps_said(w: &Value) -> String {
    let decisions = w["decisions"].as_array().cloned().unwrap_or_default();
    let why = |who: &str| decisions.iter().find(|d| d["who"] == who).and_then(|d| d["reason"].as_str()).map(|r| format!("{who} ({r:?})")).unwrap_or(who.to_string());
    let slots = w["slots"].as_array().cloned().unwrap_or_default();
    let said: Vec<String> = slots
        .iter()
        .map(|s| {
            let by: Vec<String> = s["by"].as_array().into_iter().flatten().filter_map(Value::as_str).map(why).collect();
            let people = s["people"].as_u64().unwrap_or(1);
            let state = match (by.len() as u64, s["open"] == true) {
                (0, true) => "open".to_string(),
                (0, false) => "not open yet".to_string(),
                (n, _) if n >= people => format!("{} approved", by.join(", ")),
                (n, _) => format!("{n} of {people}: {} approved", by.join(", ")),
            };
            match (s["role"].as_str(), slots.len()) {
                (None, 1) => state,
                _ => format!("{} {state}", step_name(s)),
            }
        })
        .collect();
    said.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A waiting run reads as its points: an approval by people alone by count, one in steps a
    /// step at a time, filled, part filled, open or not yet; and an event.
    #[test]
    fn a_waiting_run_says_who_approved_each_step_so_far() {
        let dan = json!({ "slot": 0, "who": "dan", "kind": "person", "at": 5, "approved": true, "reason": "matches the invoice" });
        let slot = |n: u8, role: Option<&str>, kind: &str, people: u8, by: &[&str], open: bool| {
            let mut s = json!({ "slot": n, "stage": n, "kind": kind, "people": people, "by": by, "open": open });
            if let Some(role) = role {
                s["role"] = role.into();
            }
            s
        };
        let runs = [json!({
            "run_id": "7b379ca1dd65e73579a1f00aa88cf5497be604fd06f3b60a12a5f86fa80c7791",
            "kind": "action",
            "route": "action pay",
            "parked_at": 0,
            "waiting": [
                { "name": "payout", "expires_at": 86400, "steps": "person*2", "said": "2 people", "slots": [slot(0, None, "person", 2, &["dan"], true)], "decisions": [dan] },
                {
                    "name": "release",
                    "expires_at": 86400,
                    "steps": "agent:classifier>person:reviewer*2>person:approver",
                    "said": "an agent holding classifier, then 2 people holding reviewer, then a person holding approver",
                    "slots": [slot(0, Some("classifier"), "agent", 1, &["scout"], false), slot(1, Some("reviewer"), "person", 2, &["dan"], true), slot(2, Some("approver"), "person", 1, &[], false)],
                    "decisions": [{ "slot": 0, "who": "scout", "kind": "agent", "at": 3, "approved": true }, dan],
                },
                { "name": "paid", "expires_at": 60 }
            ],
            "delivered": []
        })];
        assert_eq!(
            waiting_lines(&runs),
            [
                "run 7b379ca1dd65 (action pay, since 1970-01-01T00:00:00) waits at approval \"payout\" (2 people): 1 of 2: dan (\"matches the invoice\") approved, until 1970-01-02T00:00:00",
                "run 7b379ca1dd65 (action pay, since 1970-01-01T00:00:00) waits at approval \"release\" (an agent holding classifier, then 2 people holding reviewer, then a person holding approver): classifier scout approved; reviewer 1 of 2: dan (\"matches the invoice\") approved; approver not open yet, until 1970-01-02T00:00:00",
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

    /// A decision says the step it took and what the approval waits for now, or that it is
    /// decided: by people alone, and in steps.
    #[test]
    fn a_decision_says_what_it_did() {
        let alone = |outcome: &str, by: &[&str]| json!({ "name": "payout", "slot": 0, "slots": [{ "slot": 0, "stage": 0, "kind": "person", "people": 2, "by": by, "open": outcome == "waiting" }], "outcome": outcome });
        let chain = |outcome: &str, slot: u8, by: [&[&str]; 3]| {
            let slots: Vec<Value> = [("classifier", "agent", 1), ("reviewer", "person", 2), ("approver", "person", 1)]
                .iter()
                .zip(by)
                .enumerate()
                .map(|(n, ((role, kind, people), by))| json!({ "slot": n, "stage": n, "role": role, "kind": kind, "people": people, "by": by, "open": false }))
                .collect();
            json!({ "name": "payout", "slot": slot, "slots": slots, "outcome": outcome })
        };
        let said = |d: &Value| decided_said("pay", "7b379ca1dd65e735", d);
        assert_eq!(said(&alone("waiting", &["dan"])), "Approved \"payout\" for run 7b379ca1dd65 of pay: 1 of 2. It waits for 1 more person.");
        assert_eq!(said(&alone("approved", &["dan", "ada"])), "Approved \"payout\", by dan and ada: run 7b379ca1dd65 of pay goes on.");
        assert_eq!(said(&alone("declined", &[])), "Declined \"payout\": run 7b379ca1dd65 of pay goes on as declined.");
        assert_eq!(said(&chain("waiting", 0, [&["scout"], &[], &[]])), "Approved \"payout\" as classifier for run 7b379ca1dd65 of pay. It waits for reviewer (0 of 2) and approver.");
        assert_eq!(said(&chain("waiting", 1, [&["scout"], &["dan"], &[]])), "Approved \"payout\" as reviewer for run 7b379ca1dd65 of pay. It waits for reviewer (1 of 2) and approver.");
        assert_eq!(said(&chain("approved", 2, [&["scout"], &["dan", "ada"], &["cy"]])), "Approved \"payout\" as approver, by scout, dan, ada and cy: run 7b379ca1dd65 of pay goes on.");
        assert_eq!(said(&chain("declined", 1, [&["scout"], &[], &[]])), "Declined \"payout\" as reviewer: run 7b379ca1dd65 of pay goes on as declined.");
    }
}
