//! `holon restore`: a deployment's versions, and deploying an earlier one's code again
//! (docs/L4s/history-L4.md §4), over the control plane (`GET /deployments/<slug>/versions`,
//! `POST /deployments/<slug>/restore`). A restore is a new version, never a rewrite of one, and is
//! checked against today's grant: what it needs that isn't granted now is shown, and approved only
//! with `--approve`.

use crate::local::admin_call;

const RESTORE_USAGE: &str = "usage: holon restore <slug> [--server PORT|URL]
       holon restore <slug> --to N --code [--dry-run] [--approve] [--server PORT|URL]

  With no --to: the deployment's versions, newest first. Every deploy is a version, kept for
  good, numbered from 1.
  --to N --code: deploys version N's code again (its flow, with what its deploy asked), as a new
  version; the deployment's state stays as it is. It is checked against the grant it has now,
  not the one it had: what it needs that isn't granted now is listed, and approved only with
  --approve. --dry-run: what it would ask, deploying nothing.

  From the holon serve you run on this machine, signed in as you; with several running,
  --server <port> picks one. A server elsewhere: --server <URL> (or HOLON_SERVER), signed in
  with an API token in HOLON_TOKEN.";

pub(crate) fn cli(args: &[String]) -> i32 {
    const WHO: &str = "holon restore";
    let (mut slug, mut server, mut to) = (None, None, None);
    let (mut code, mut dry_run, mut approve) = (false, false, false);
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
            "--code" => code = true,
            "--state" => {
                eprintln!(
                    "{WHO}: restoring a version's state isn't built yet. To start from an earlier state, stop the deployment and run it from a session's (the editor's run menu, or POST /deployments/<slug>/run {{\"from\": <session>}})"
                );
                return 2;
            }
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
    let Some(number) = to else {
        if code || dry_run || approve {
            eprintln!("{WHO}: say which version: --to N (holon restore {slug} lists them)");
            return 2;
        }
        return list(&slug, server.as_deref());
    };
    if !code {
        eprintln!("{WHO}: say what to restore: --code deploys version {number}'s code again");
        return 2;
    }
    let path = format!("/deployments/{slug}/restore");
    let ask = |body: serde_json::Value| -> Result<serde_json::Value, i32> {
        let (_, answer) = admin_call(WHO, server.as_deref(), "POST", &path, Some(&body))?;
        serde_json::from_str(&answer).map_err(|e| {
            eprintln!("{WHO}: the server's answer isn't JSON: {e}");
            1
        })
    };
    let preview = match ask(serde_json::json!({ "version": number, "code": true, "dry_run": true })) {
        Ok(p) => p,
        Err(code) => return code,
    };
    if let Some(why) = preview["refused"].as_str() {
        eprintln!("{WHO}: version {number} can't be deployed on this server now: {why}");
        return 1;
    }
    let needs: Vec<String> = preview["needs_approval"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
    let missing: Vec<&str> = preview["missing_secrets"].as_array().into_iter().flatten().filter_map(|v| v.as_str()).collect();
    if !missing.is_empty() {
        println!("its secrets not set now (those nodes stay empty until they are): {}", missing.join(", "));
    }
    if dry_run {
        if needs.is_empty() {
            println!("version {number}'s code would deploy again with the grant {slug} has now; nothing changed");
        } else {
            println!("version {number}'s code needs approving what today's grant doesn't cover: {}; nothing changed", needs.join(", "));
        }
        return 0;
    }
    if !needs.is_empty() && !approve {
        eprintln!("{WHO}: version {number} needs approving what today's grant doesn't cover: {}", needs.join(", "));
        eprintln!("{WHO}: to approve them and restore it, run this again with --approve");
        return 1;
    }
    match ask(serde_json::json!({ "version": number, "code": true, "acknowledge": needs })) {
        Ok(done) => {
            let unit = done["unit"].as_str().unwrap_or_default();
            println!(
                "{slug}: version {number}'s code is deployed again, as version {} ({}); its state is as it was",
                done["version"],
                &unit[..unit.len().min(12)]
            );
            0
        }
        Err(code) => code,
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
    println!("\nholon restore {slug} --to N --code deploys version N's code again");
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
        let old = serde_json::json!({ "number": 1, "unit": "00ff", "current": false, "restorable": false });
        assert_eq!(version_line(&old), "v1    00ff  (from before rc.30: opens, doesn't restore)");
    }
}
