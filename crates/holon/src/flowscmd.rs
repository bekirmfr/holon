//! The flows saved on a server, from the command line, as the editor's flows page has them:
//! `holon flows` lists them, shows one's text, saves a file as one (not deployed) and removes
//! one; `holon activity` is what was done on the server, by whom.

use serde_json::{json, Value};

use crate::cmdline::{self, seg, table, when, SIGN_IN};
use crate::local;
use holon_author::agent;

const FLOWS_USAGE: &str = "usage: holon flows [list] [--json]
       holon flows show <name> [--out FILE]
       holon flows save <file> [--name NAME] [--replace]
       holon flows remove <name>
       (each with [--server PORT|URL])

  The flows saved on the server, as the editor's flows page has them.

  list                     each saved flow: who saved it, when, and whether it is deployed
                           (what holon flows does alone)
  show <name>              print one's text
    --out FILE             write it into FILE instead
  save <file>              compile a file and save it, not deployed, with its text as written,
                           for someone to open in the editor
    --name NAME            save it under NAME instead of the flow's own name
    --replace              replace a flow someone else saved under that name (kept otherwise)
  remove <name>            delete a saved flow (a deployment of it stays)
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon flows
  holon flows save shop.holon.ts
  holon flows show shop --out shop.holon.ts
  holon flows remove shop
";

const ACTIVITY_USAGE: &str = "usage: holon activity [--last N] [--json] [--server PORT|URL]

  Lists what was done on the server, newest first, and by whom: saves, deploys, runs, pauses,
  stops, grants, deletes and bindings.

  --last N                 how many (default 50, at most 500)
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon activity
  holon activity --last 200 --json
";

pub(crate) fn flows(args: &[String]) -> i32 {
    const WHO: &str = "holon flows";
    let usage = format!("{FLOWS_USAGE}\n{SIGN_IN}");
    let args = match cmdline::parse(WHO, &usage, args, &["--name", "--out"], &["--replace", "--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    match (args.word(0), args.word(1)) {
        (None | Some("list"), None) => {
            let saved = match cmdline::json(WHO, &args, "GET", "/flows", None) {
                Ok(v) => v,
                Err(code) => return code,
            };
            if args.flag("--json") {
                cmdline::print_json(&saved);
                return 0;
            }
            let deployed = match cmdline::json(WHO, &args, "GET", "/deployments", None) {
                Ok(v) => v["deployments"].as_array().cloned().unwrap_or_default(),
                Err(code) => return code,
            };
            for line in saved_lines(&saved, &deployed) {
                println!("{line}");
            }
            0
        }
        (Some("show"), Some(name)) => {
            let record = match cmdline::json(WHO, &args, "GET", &format!("/flows/{}", seg(name)), None) {
                Ok(v) => v,
                Err(code) => return code,
            };
            if args.flag("--json") {
                cmdline::print_json(&record);
                return 0;
            }
            let text = match agent::text_of(name, &record["document"]) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{WHO}: {name}: {e}");
                    return 1;
                }
            };
            match args.value("--out") {
                Some(out) => match std::fs::write(out, &text) {
                    Ok(()) => {
                        println!("wrote {out}");
                        0
                    }
                    Err(e) => {
                        eprintln!("{WHO}: {out}: {e}");
                        1
                    }
                },
                None => {
                    print!("{text}");
                    0
                }
            }
        }
        (Some("save"), Some(file)) => save(WHO, &args, file),
        (Some("remove"), Some(name)) => match cmdline::json(WHO, &args, "DELETE", &format!("/flows/{}", seg(name)), None) {
            Ok(_) => {
                println!("{name}: removed from the saved flows");
                0
            }
            Err(code) => code,
        },
        _ => {
            eprintln!("{usage}");
            2
        }
    }
}

fn save(who: &str, args: &cmdline::Args, file: &str) -> i32 {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{who}: {file}: {e}");
            return 1;
        }
    };
    // Compiled first, as `holon deploy` and the agent tools do: the server takes the document
    // the editor would send, with the text as written beside it.
    let plan = match agent::plan(&text) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.replace("text:", &format!("{file}:")));
            return 1;
        }
    };
    let name = args.value("--name").map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&plan.name).to_string();
    if !args.flag("--replace") {
        let me = local::send(who, args.server(), "GET", "/auth/me", None)
            .ok()
            .filter(|a| a.status == 200)
            .and_then(|a| serde_json::from_str::<Value>(&a.body).ok())
            .and_then(|m| m["name"].as_str().map(String::from));
        let saved = match local::send(who, args.server(), "GET", &format!("/flows/{}", seg(&name)), None) {
            Ok(a) => a,
            Err(code) => return code,
        };
        if saved.status == 200 {
            let by = serde_json::from_str::<Value>(&saved.body).ok().and_then(|s| s["saved_by"].as_str().map(String::from));
            if by.is_some() && by != me {
                eprintln!("{who}: {name:?} is saved by {}: --name saves yours under another name, --replace replaces theirs", by.unwrap_or_default());
                return 1;
            }
        }
    }
    let mut document = plan.document.clone();
    document["source"] = json!(text);
    match cmdline::json(who, args, "POST", "/flows", Some(&json!({ "name": name, "document": document }))) {
        Ok(_) => {
            println!("{name}: saved, not deployed (the editor's flows page opens it; holon deploy {file} deploys it)");
            0
        }
        Err(code) => code,
    }
}

pub(crate) fn saved_lines(saved: &Value, deployed: &[Value]) -> Vec<String> {
    let all: Vec<&Value> = saved.as_array().into_iter().flatten().collect();
    if all.is_empty() {
        return vec!["no flow is saved (holon new --help lists the samples)".into()];
    }
    let mut rows = vec![["name", "saved", "by", "deployed"].map(String::from).to_vec()];
    for f in all {
        let name = f["name"].as_str().unwrap_or_default();
        let saved_at = f["saved_at_epoch_seconds"].as_u64();
        let state = match deployed.iter().find(|d| d["slug"] == name) {
            None => "no".to_string(),
            Some(d) if d["deployed_at_epoch_seconds"].as_u64() < saved_at => "yes, an earlier version".into(),
            Some(_) => "yes".into(),
        };
        rows.push(vec![name.into(), when(saved_at), f["saved_by"].as_str().unwrap_or("-").into(), state]);
    }
    table(&rows)
}

pub(crate) fn activity(args: &[String]) -> i32 {
    const WHO: &str = "holon activity";
    let usage = format!("{ACTIVITY_USAGE}\n{SIGN_IN}");
    let args = match cmdline::parse(WHO, &usage, args, &["--last"], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let last = match args.value("--last").map(str::parse::<u32>) {
        None => 50,
        Some(Ok(n)) if (1..=500).contains(&n) => n,
        Some(_) => {
            eprintln!("{WHO}: --last is 1 to 500");
            return 2;
        }
    };
    match cmdline::json(WHO, &args, "GET", &format!("/activity?limit={last}"), None) {
        Ok(v) if args.flag("--json") => {
            cmdline::print_json(&v);
            0
        }
        Ok(v) => {
            for line in activity_lines(&v) {
                println!("{line}");
            }
            0
        }
        Err(code) => code,
    }
}

pub(crate) fn activity_lines(v: &Value) -> Vec<String> {
    let rows: Vec<Vec<String>> = v["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| vec![when(e["at"].as_u64()), e["by"].as_str().unwrap_or("-").into(), e["action"].as_str().unwrap_or_default().into(), e["target"].as_str().unwrap_or_default().into()])
        .collect();
    if rows.is_empty() {
        return vec!["nothing done yet".into()];
    }
    table(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_flows_say_whether_what_is_deployed_is_their_latest() {
        let saved = json!([{ "name": "shop", "saved_at_epoch_seconds": 60, "saved_by": "maria" }, { "name": "notes", "saved_at_epoch_seconds": 0 }, { "name": "beats", "saved_at_epoch_seconds": 0 }]);
        let deployed = [json!({ "slug": "shop", "deployed_at_epoch_seconds": 0 }), json!({ "slug": "notes", "deployed_at_epoch_seconds": 30 })];
        assert_eq!(
            saved_lines(&saved, &deployed),
            [
                "name   saved                by     deployed",
                "shop   1970-01-01T00:01:00  maria  yes, an earlier version",
                "notes  1970-01-01T00:00:00  -      yes",
                "beats  1970-01-01T00:00:00  -      no",
            ]
        );
    }

    #[test]
    fn the_activity_reads_newest_first_with_who_did_it() {
        let v = json!({ "entries": [{ "at": 60, "action": "deployed", "target": "shop", "by": "maria" }, { "at": 0, "action": "saved", "target": "shop" }] });
        assert_eq!(activity_lines(&v), ["1970-01-01T00:01:00  maria  deployed  shop", "1970-01-01T00:00:00  -      saved     shop"]);
    }
}
