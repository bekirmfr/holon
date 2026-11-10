//! `holon logs`: what a deployment's flow wrote with `console.log` and the rest (logs.rs), read
//! from the running server. A person signed in reads them, as they read a deployment's runs; an
//! agent reads them only where the deployment's grant lets it.

use std::time::Duration;

use serde_json::Value;

use crate::cmdline::{self, seg, SIGN_IN};

const USAGE: &str = "usage: holon logs <slug> [options]

  The lines a deployment's flow wrote with console.log, info, debug, warn and error, oldest first.
  They are kept beside the runs: none is an effect a run performed, and `holon verify` checks a run
  without them. Each line says when, its level, and the run that wrote it.

  --level LEVEL            only this level and louder: debug, log (the same as info), warn, error
  --run N                  only the lines run N wrote (its number in the deployment's log)
  --until N                every line up to the end of run N's
  --limit N                the newest N lines (200 if left out)
  --follow                 keep reading, as new lines come
  --json                   print each line as a line of JSON
  --server PORT|URL        which holon server to ask (see below)

  A deployment keeps a few MB of lines: the oldest go first, and a line says when some were dropped.

examples:
  holon logs payouts
  holon logs payouts --level warn --follow
  holon logs payouts --run 17
";

pub(crate) fn run(args: &[String]) -> i32 {
    const WHO: &str = "holon logs";
    let usage = format!("{USAGE}\n{SIGN_IN}");
    let args = match cmdline::parse(WHO, &usage, args, &["--level", "--run", "--until", "--limit"], &["--follow", "--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let Some(slug) = args.word(0) else {
        eprintln!("{usage}");
        return 2;
    };
    let mut query = String::new();
    for (flag, name) in [("--level", "level"), ("--run", "run"), ("--until", "until"), ("--limit", "limit")] {
        if let Some(v) = args.value(flag) {
            query.push_str(&format!("&{name}={}", seg(v)));
        }
    }
    let path = |after: Option<u64>| format!("/deployments/{}/logs?{}{}", seg(slug), query.trim_start_matches('&'), after.map_or(String::new(), |a| format!("&after={a}")));
    let follow = args.flag("--follow");
    let mut after = None;
    let mut first = true;
    loop {
        let answer = match cmdline::json(WHO, &args, "GET", &path(after), None) {
            Ok(v) => v,
            Err(code) => return code,
        };
        let lines = answer["lines"].as_array().cloned().unwrap_or_default();
        if first && lines.is_empty() && !follow {
            println!("{slug} has written no lines (console.log in its flow writes them).");
        }
        for line in &lines {
            if args.flag("--json") {
                println!("{line}");
            } else {
                println!("{}", shown(line));
            }
            after = line["at"].as_u64().or(after);
        }
        first = false;
        if !follow {
            return 0;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// One line as a terminal shows it: `21:15:33.419 WARN  run 3  careful`.
pub(crate) fn shown(line: &Value) -> String {
    let at = line["at"].as_u64().unwrap_or(0);
    let (secs, ms) = (at / 1000, at % 1000);
    let clock = format!("{:02}:{:02}:{:02}.{ms:03}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let level = line["level"].as_str().unwrap_or("log").to_uppercase();
    format!("{clock} {level:<5} run {}  {}", line["run"].as_u64().unwrap_or(0), line["text"].as_str().unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_reads_as_its_time_level_run_and_text() {
        let line = serde_json::json!({ "at": 76_533_419u64, "level": "warn", "run": 3, "text": "careful" });
        assert_eq!(shown(&line), "21:15:33.419 WARN  run 3  careful");
    }
}
