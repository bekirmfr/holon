//! `holon wait`: holds until a person has decided a flow's proposal, then says how, with their
//! reason, and ends with a code that tells it apart. An agent whose deploy is only proposed (a
//! first deploy, a new permission) has nothing to do until a person decides in the editor; run in
//! the background, this ends when they do, and a host that restarts the agent when a background
//! command ends has told it. Reading a proposal needs no sign-in beyond what `holon deployments`
//! does, and the command decides nothing: that is `holon decide` and the editor's, a person's.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cmdline;
use crate::mcp::proposal_line;

const USAGE: &str = "usage: holon wait <slug> [--timeout SECONDS] [--json] [--server PORT|URL]

  Holds until a person has decided the proposal a flow's deploy made (a deploy from an agent that
  needs approval: holon mcp's deploy, or an agent's token), and says what they decided, with the
  reason of a decline. For an agent to run in the background after it proposes: it ends when the
  decision is made, and that is when the agent has something to do. Already decided, it says so at
  once. It decides nothing: a person approves or declines in the editor's Approvals.

  --timeout SECONDS        stop after that long if nobody has decided (default: wait on)
  --json                   print the proposal as the server keeps it, as JSON
  --server PORT|URL        which holon server to ask (see below)

  Ends with 0 when it was approved (and so deployed), 1 when it was declined, 2 when the flow has
  no proposal (or the command was wrong), 3 when a newer proposal replaced it, 4 when the time
  ran out with nobody having decided.

examples:
  holon wait tidewheel
  holon wait tidewheel --timeout 600 --server 7400
";

/// How long between looks at the server.
const EVERY: Duration = Duration::from_secs(2);

/// What a wait found.
#[derive(Debug, PartialEq)]
pub(crate) enum Ended {
    /// The flow's newest proposal, decided (or replaced).
    Decided(Value),
    /// The time ran out with it waiting for a person.
    Waiting(Value),
    /// The flow has no proposal.
    None,
}

/// The flow's proposal to follow: its newest that a newer one hasn't replaced (the server lists
/// the newest first), or the newest, replaced, when no other is left.
fn newest<'a>(all: &'a [Value], slug: &str) -> Option<&'a Value> {
    let mine = || all.iter().filter(|p| p.get("slug").and_then(Value::as_str) == Some(slug));
    mine().find(|p| p.get("state").and_then(Value::as_str) != Some("replaced")).or_else(|| mine().next())
}

/// Look at the server's proposals (`fetch`) every `every` until the flow's is decided, or
/// `timeout` has gone. `announce` is told of each proposal it starts to wait for.
pub(crate) fn watch(
    mut fetch: impl FnMut() -> Result<Vec<Value>, i32>,
    slug: &str,
    timeout: Option<Duration>,
    every: Duration,
    mut announce: impl FnMut(&Value),
) -> Result<Ended, i32> {
    let until = timeout.map(|t| Instant::now() + t);
    let mut announced = None;
    loop {
        let all = fetch()?;
        let Some(p) = newest(&all, slug) else { return Ok(Ended::None) };
        if p.get("state").and_then(Value::as_str) != Some("waiting") {
            return Ok(Ended::Decided(p.clone()));
        }
        let id = p.get("id").and_then(Value::as_u64);
        if announced != id {
            announce(p);
            announced = id;
        }
        if until.is_some_and(|u| Instant::now() >= u) {
            return Ok(Ended::Waiting(p.clone()));
        }
        std::thread::sleep(every);
    }
}

/// What a decided proposal ends the command with.
fn code_of(p: &Value) -> i32 {
    match p.get("state").and_then(Value::as_str) {
        Some("approved") => 0,
        Some("declined") => 1,
        _ => 3,
    }
}

pub(crate) fn run(args: &[String]) -> i32 {
    const WHO: &str = "holon wait";
    let args = match cmdline::parse(WHO, USAGE, args, &["--timeout"], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let Some(slug) = args.word(0) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let timeout = match args.value("--timeout").map(str::parse::<u64>) {
        None => None,
        Some(Ok(s)) => Some(Duration::from_secs(s)),
        Some(Err(_)) => {
            eprintln!("{WHO}: --timeout takes a number of seconds\n\n{USAGE}");
            return 2;
        }
    };
    let fetch = || cmdline::json(WHO, &args, "GET", "/proposals", None).map(|v| v.get("proposals").and_then(Value::as_array).cloned().unwrap_or_default());
    let announce = |p: &Value| eprintln!("{WHO}: {slug}'s {}. A person decides it in the editor's Approvals.", proposal_line(p).trim_end_matches('.'));
    match watch(fetch, slug, timeout, EVERY, announce) {
        Err(code) => code,
        Ok(Ended::None) => {
            eprintln!("{WHO}: {slug} has no proposal on this server (a deploy that needs approval, from an agent, makes one)");
            2
        }
        Ok(Ended::Decided(p)) => {
            if args.flag("--json") {
                cmdline::print_json(&p);
            } else {
                println!("{}", proposal_line(&p));
            }
            code_of(&p)
        }
        Ok(Ended::Waiting(p)) => {
            if args.flag("--json") {
                cmdline::print_json(&p);
            } else {
                println!("{} Still waiting after {} seconds.", proposal_line(&p), timeout.map(|t| t.as_secs()).unwrap_or(0));
            }
            4
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn proposal(id: u64, slug: &str, state: &str) -> Value {
        json!({ "id": id, "slug": slug, "state": state, "version": 1, "by": "ada (agent)", "at_ms": 1_000_000, "decided_by": "bma", "decided_at_ms": 2_000_000, "reason": "tidy the graph" })
    }

    /// A server that answers its proposals as a script says, the last one on for ever.
    fn scripted(script: Vec<Vec<Value>>) -> impl FnMut() -> Result<Vec<Value>, i32> {
        let mut looked = 0;
        move || {
            let at = looked.min(script.len() - 1);
            looked += 1;
            Ok(script[at].clone())
        }
    }

    const FAST: Duration = Duration::from_millis(2);

    #[test]
    fn it_holds_while_a_person_has_not_decided_and_ends_with_how_they_decided() {
        let mut said = Vec::new();
        let ended = watch(
            scripted(vec![
                vec![proposal(1, "tide", "waiting"), proposal(2, "other", "waiting")],
                vec![proposal(1, "tide", "waiting")],
                vec![proposal(1, "tide", "declined"), proposal(2, "other", "waiting")],
            ]),
            "tide",
            None,
            FAST,
            |p| said.push(p["id"].as_u64().unwrap()),
        );
        let Ok(Ended::Decided(p)) = ended else { panic!("{ended:?}") };
        assert_eq!(p["state"], "declined");
        assert_eq!(code_of(&p), 1);
        assert_eq!(said, [1], "it said which it waited for once, not at each look");
        let line = proposal_line(&p);
        assert!(line.contains("declined by bma") && line.contains("tidy the graph"), "{line}");
        assert_eq!(code_of(&proposal(1, "tide", "approved")), 0);
    }

    #[test]
    fn a_decided_proposal_ends_it_at_once_and_a_flow_with_none_says_so() {
        let ended = watch(scripted(vec![vec![proposal(3, "tide", "approved")]]), "tide", None, Duration::from_secs(3600), |_| panic!("nothing to wait for"));
        assert!(matches!(ended, Ok(Ended::Decided(_))), "{ended:?}");
        assert_eq!(watch(scripted(vec![vec![proposal(3, "other", "waiting")]]), "tide", None, FAST, |_| {}), Ok(Ended::None));
        assert_eq!(watch(scripted(vec![vec![]]), "tide", None, FAST, |_| {}), Ok(Ended::None));
    }

    #[test]
    fn a_newer_proposal_is_followed_and_the_time_it_is_given_ends_a_wait_that_nobody_decides() {
        // Version 1 is replaced by version 2, which is the one it waits for and the one decided.
        let ended = watch(
            scripted(vec![
                vec![proposal(1, "tide", "waiting")],
                vec![proposal(2, "tide", "waiting"), proposal(1, "tide", "replaced")],
                vec![proposal(2, "tide", "approved"), proposal(1, "tide", "replaced")],
            ]),
            "tide",
            None,
            FAST,
            |_| {},
        );
        let Ok(Ended::Decided(p)) = ended else { panic!("{ended:?}") };
        assert_eq!((p["id"].as_u64(), p["state"].as_str()), (Some(2), Some("approved")));
        // Nobody decides: it gives up at the time asked.
        let started = Instant::now();
        let ended = watch(scripted(vec![vec![proposal(4, "tide", "waiting")]]), "tide", Some(Duration::from_millis(60)), FAST, |_| {});
        assert!(matches!(ended, Ok(Ended::Waiting(_))), "{ended:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        // A server that can't be reached ends it with the code `cmdline` gave.
        assert_eq!(watch(|| Err(1), "tide", None, FAST, |_| {}), Err(1));
    }
}
