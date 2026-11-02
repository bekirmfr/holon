//! A deployment's secrets from the command line, as the editor's secrets panel has them: which it
//! reads and which are set, each one's versions and what was done to it, and setting, rolling
//! back and removing one. A value is never printed, nor taken as an argument (where it would stay
//! in the shell's history): it is piped in, or named by where the server reads it.

use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::cmdline::{self, seg, table, when, SIGN_IN};

const USAGE: &str = "usage: holon secret list <slug>
       holon secret history <slug> <name>
       holon secret set <slug> <name> [--from env:HOLON_SECRET_X | --from vault:PATH#FIELD]
       holon secret rollback <slug> <name> --to VERSION [--dry-run]
       holon secret remove <slug> <name>
       holon secret bind <name> [--from SOURCE] [--to HOST,…]
       holon secret cap <name> --to HOST,… | --anywhere
       holon secret bound
       holon secret unbind <name>
       holon secret env
       (each with [--json] [--server PORT|URL])

  list: the secrets a deployment reads, and the version, source and age of each one set.
  history: one secret's kept versions and what was done to it, by whom. Never a value.
  set: a new version, its value piped in (… | holon secret set shop api_key), or read by the
  server from its environment or a vault. rollback: make a kept version current again, as a new
  version; --dry-run says which values (by fingerprint) it would swap. remove: forget it, every
  version. env: the HOLON_SECRET_* names the server can read. Admins set, roll back and remove.

  bind: the server's own secret, read by every deployment that reads one of that name and has
  none of its own (one set for a deployment overrides it), its value piped in or --from, as set.
  --to says where it may go: hosts (api.stripe.com, with :port if not 443) and databases
  (db:shop). A deploy whose flow sends it elsewhere is refused, and so is a request a run makes
  with it; a deployment with its own isn't held to it. cap: change only where it may go
  (--anywhere: wherever its flows' approvals show). bound: the server's secrets, and where each
  may go. unbind: remove one, every version.
";

pub(crate) fn cli(args: &[String]) -> i32 {
    const WHO: &str = "holon secret";
    let usage = format!("{USAGE}\n{SIGN_IN}");
    let args = match cmdline::parse(WHO, &usage, args, &["--from", "--to"], &["--dry-run", "--json", "--anywhere"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (what, slug, name) = (args.word(0), args.word(1), args.word(2));
    let path = |slug: &str, name: Option<&str>| match name {
        Some(n) => format!("/deployments/{}/secrets/{}", seg(slug), seg(n)),
        None => format!("/deployments/{}/secrets", seg(slug)),
    };
    let shown = |v: Value, lines: fn(&Value) -> Vec<String>| {
        if args.flag("--json") {
            cmdline::print_json(&v);
        } else {
            for line in lines(&v) {
                println!("{line}");
            }
        }
        0
    };
    match (what, slug, name) {
        (Some("list"), Some(slug), None) => {
            let mut v = match cmdline::json(WHO, &args, "GET", &path(slug, None), None) {
                Ok(v) => v,
                Err(code) => return code,
            };
            // The ones it reads, set or not: its version's manifest names them.
            if let Ok(manifest) = cmdline::json(WHO, &args, "GET", &format!("/deployments/{}/manifest", seg(slug)), None) {
                v["reads"] = json!(reads(&manifest));
            }
            shown(v, list_lines)
        }
        (Some("history"), Some(slug), Some(name)) => cmdline::json(WHO, &args, "GET", &path(slug, Some(name)), None).map_or_else(|c| c, |v| shown(v, history_lines)),
        (Some("env"), None, None) => cmdline::json(WHO, &args, "GET", "/admin/secrets/env", None).map_or_else(|c| c, |v| shown(v, env_lines)),
        (Some("bound"), None, None) => cmdline::json(WHO, &args, "GET", "/admin/secrets", None).map_or_else(|c| c, |v| shown(v, bound_lines)),
        (Some("bind"), Some(name), None) => {
            let to = match args.value("--to").map(destinations) {
                Some(Ok(to)) => Some(to),
                Some(Err(e)) => {
                    eprintln!("{WHO}: {e}");
                    return 2;
                }
                None => None,
            };
            set(WHO, &args, &format!("/admin/secrets/{}", seg(name)), "the server", name, to)
        }
        (Some("cap"), Some(name), None) => {
            let to = match (args.value("--to").map(destinations), args.flag("--anywhere")) {
                (Some(Ok(to)), false) => json!(to),
                (None, true) => Value::Null,
                (Some(Err(e)), _) => {
                    eprintln!("{WHO}: {e}");
                    return 2;
                }
                _ => {
                    eprintln!("{WHO}: cap {name} --to HOST,… (or db:NAME), or --anywhere");
                    return 2;
                }
            };
            match cmdline::json(WHO, &args, "POST", &format!("/admin/secrets/{}/cap", seg(name)), Some(&json!({ "to": to }))) {
                Ok(v) if args.flag("--json") => shown(v, |_| vec![]),
                Ok(v) => {
                    println!("{name}: {}", may_go(&v["to"]));
                    0
                }
                Err(code) => code,
            }
        }
        (Some("unbind"), Some(name), None) => match cmdline::json(WHO, &args, "DELETE", &format!("/admin/secrets/{}", seg(name)), None) {
            Ok(_) => {
                println!("{name}: the server's removed, every version");
                0
            }
            Err(code) => code,
        },
        (Some("set"), Some(slug), Some(name)) => set(WHO, &args, &path(slug, Some(name)), slug, name, None),
        (Some("rollback"), Some(slug), Some(name)) => {
            let Some(to) = args.value("--to").and_then(|v| v.trim_start_matches('v').parse::<u32>().ok()) else {
                eprintln!("{WHO}: rollback --to VERSION (holon secret history {slug} {name} lists them)");
                return 2;
            };
            let dry = args.flag("--dry-run");
            match cmdline::json(WHO, &args, "POST", &format!("{}/rollback", path(slug, Some(name))), Some(&json!({ "version": to, "dry_run": dry }))) {
                Ok(v) if args.flag("--json") => shown(v, |_| vec![]),
                Ok(v) if dry => {
                    println!("{}", rollback_preview(name, &v));
                    0
                }
                Ok(v) => {
                    println!("{name}: version {to} is current again, as version {}", v["version"]);
                    0
                }
                Err(code) => code,
            }
        }
        (Some("remove"), Some(slug), Some(name)) => match cmdline::json(WHO, &args, "DELETE", &path(slug, Some(name)), None) {
            Ok(_) => {
                println!("{name}: removed from {slug}, every version");
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

/// `HOST,HOST,db:NAME` as where a secret may go; none of them empty.
fn destinations(list: &str) -> Result<Vec<String>, String> {
    let to: Vec<String> = list.split(',').map(|d| d.trim().to_string()).collect();
    match to.iter().find(|d| !crate::secrets::is_destination(d)) {
        Some(bad) => Err(format!("{bad:?} isn't where a secret goes: a host (api.stripe.com, with :port if not 443) or a database (db:shop)")),
        None => Ok(to),
    }
}

/// Where the server's secret may go, as `to` (a list, or null) says it.
fn may_go(to: &Value) -> String {
    match to.as_array() {
        Some(to) if to.is_empty() => "may go nowhere".into(),
        Some(to) => {
            let to: std::collections::BTreeSet<String> = to.iter().filter_map(Value::as_str).map(String::from).collect();
            format!("may go only to {}", holon_author::describe::describe_sent_to(&to).0.unwrap_or_default())
        }
        None => "may go wherever its flows' approvals show".into(),
    }
}

fn set(who: &str, args: &cmdline::Args, path: &str, slug: &str, name: &str, to: Option<Vec<String>>) -> i32 {
    let body = match args.value("--from") {
        Some(from) if from.starts_with("env:") || from.starts_with("vault:") => json!({ "from": from }),
        Some(from) => {
            eprintln!("{who}: --from is env:HOLON_SECRET_NAME or vault:PATH#FIELD, not {from:?}");
            return 2;
        }
        None => {
            let piped = match cmdline::stdin_text(who) {
                Ok(t) => Zeroizing::new(t),
                Err(code) => return code,
            };
            // What a shell's `echo` or a file adds at its end isn't part of the value.
            let value = Zeroizing::new(piped.trim_end_matches(['\r', '\n']).to_string());
            if value.is_empty() {
                eprintln!("{who}: nothing was piped in");
                return 2;
            }
            json!({ "value": value.as_str() })
        }
    };
    let mut body = body;
    if let Some(to) = &to {
        body["to"] = json!(to);
    }
    match cmdline::json(who, args, "POST", path, Some(&body)) {
        Ok(v) => {
            println!("{name}: version {} set for {slug}", v["version"]);
            if let Some(to) = to {
                println!("{name}: {}", may_go(&json!(to)));
            }
            0
        }
        Err(code) => code,
    }
}

/// The secrets a manifest's items read: `SecretRead("api_key")` is `api_key`.
pub(crate) fn reads(manifest: &Value) -> Vec<String> {
    manifest["items"].as_array().into_iter().flatten().filter_map(Value::as_str).filter_map(|i| i.strip_prefix("SecretRead(")?.strip_suffix(')')).filter_map(|q| serde_json::from_str::<String>(q).ok()).collect()
}

pub(crate) fn list_lines(v: &Value) -> Vec<String> {
    // The server's are listed where its version reads them (or where nothing says what it reads).
    let reads: Option<Vec<&str>> = v["reads"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect());
    let set: Vec<&Value> = v["secrets"].as_array().into_iter().flatten().filter(|s| s["server"] != true || reads.as_ref().is_none_or(|r| r.contains(&s["name"].as_str().unwrap_or_default()))).collect();
    let mut rows = vec![["name", "version", "source", "set"].map(String::from).to_vec()];
    let mut problems = Vec::new();
    for s in &set {
        let name = s["name"].as_str().unwrap_or_default();
        let source = s["source"].as_str().unwrap_or_default();
        let source = if s["server"] == true { format!("the server's ({source})") } else { source.into() };
        rows.push(vec![name.into(), format!("v{}", s["version"]), source, when(s["set_at"].as_u64())]);
        if let Some(p) = s["problem"].as_str() {
            problems.push(format!("{name}: {p}"));
        }
    }
    let all: Vec<&Value> = v["secrets"].as_array().into_iter().flatten().collect();
    let mut unset: Vec<&str> = v["names"].as_array().into_iter().chain(v["reads"].as_array()).flatten().filter_map(Value::as_str).filter(|n| !all.iter().any(|s| s["name"] == *n)).collect();
    unset.sort();
    unset.dedup();
    for n in unset {
        rows.push(vec![n.into(), "-".into(), "not set".into(), String::new()]);
    }
    if rows.len() == 1 {
        return vec!["it reads no secret, and none is set for it".into()];
    }
    let mut lines = table(&rows);
    lines.extend(problems);
    lines
}

/// The server's secrets, as `holon secret bound` lists them: each with where it may go.
pub(crate) fn bound_lines(v: &Value) -> Vec<String> {
    let set: Vec<&Value> = v["secrets"].as_array().into_iter().flatten().collect();
    if set.is_empty() {
        return vec!["the server has no secret of its own (holon secret bind)".into()];
    }
    let mut rows = vec![["name", "version", "source", "set", "where it may go"].map(String::from).to_vec()];
    let mut problems = Vec::new();
    for s in set {
        let name = s["name"].as_str().unwrap_or_default();
        let to = if s["to"].is_null() { "wherever its flows' approvals show".to_string() } else { may_go(&s["to"]).trim_start_matches("may go ").to_string() };
        rows.push(vec![name.into(), format!("v{}", s["version"]), s["source"].as_str().unwrap_or_default().into(), when(s["set_at"].as_u64()), to]);
        if let Some(p) = s["problem"].as_str() {
            problems.push(format!("{name}: {p}"));
        }
    }
    let mut lines = table(&rows);
    lines.extend(problems);
    lines
}

pub(crate) fn history_lines(v: &Value) -> Vec<String> {
    let mut lines = vec![format!("{}:", v["name"].as_str().unwrap_or_default())];
    for ver in v["versions"].as_array().into_iter().flatten() {
        lines.push(format!("  {:<5}{}  from {}{}", format!("v{}", ver["version"]), when(ver["set_at"].as_u64()), ver["source"].as_str().unwrap_or_default(), if ver["current"] == true { "  (current)" } else { "" }));
    }
    let events: Vec<&Value> = v["events"].as_array().into_iter().flatten().collect();
    if !events.is_empty() {
        lines.push("  done:".into());
    }
    for e in events {
        let mut line = format!("    {}  {}", when(e["at"].as_u64()), e["action"].as_str().unwrap_or_default());
        if let Some(n) = e["version"].as_u64() {
            line.push_str(&format!(" v{n}"));
        }
        if let Some(s) = e["source"].as_str() {
            line.push_str(&format!(" from {s}"));
        }
        if let Some(d) = e["detail"].as_str() {
            line.push_str(&format!(" ({d})"));
        }
        if let Some(b) = e["by"].as_str() {
            line.push_str(&format!(" by {b}"));
        }
        lines.push(line);
    }
    lines
}

pub(crate) fn rollback_preview(name: &str, v: &Value) -> String {
    let current = match v["current_print"].as_str() {
        Some(p) => format!("v{} (#{p})", v["current"]),
        None => format!("v{} (unreadable now)", v["current"]),
    };
    format!("{name}: would make v{} (#{}, from {}) current again as v{}, in place of {current}. Nothing changed: run it again without --dry-run.", v["to"], v["to_print"].as_str().unwrap_or_default(), v["source"].as_str().unwrap_or_default(), v["version"])
}

pub(crate) fn env_lines(v: &Value) -> Vec<String> {
    let names: Vec<&str> = v["names"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut lines: Vec<String> = if names.is_empty() { vec!["no HOLON_SECRET_* variable".into()] } else { names.iter().map(|n| n.to_string()).collect() };
    if let Some(file) = v["dotenv"].as_str() {
        lines.push(format!("(its environment, and {file})"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_names_each_secret_set_or_not_and_never_a_value() {
        let v = json!({ "names": ["api_key"], "reads": ["api_key", "receipt_key"], "secrets": [{ "name": "api_key", "version": 2, "source": "a value", "set_at": 0 }] });
        assert_eq!(reads(&json!({ "items": ["NetworkIngress", "SecretRead(\"api_key\")", "SecretRead(\"receipt_key\")"] })), ["api_key", "receipt_key"]);
        assert_eq!(list_lines(&v), ["name         version  source   set", "api_key      v2       a value  1970-01-01T00:00:00", "receipt_key  -        not set"]);
        let broken = json!({ "names": [], "secrets": [{ "name": "k", "version": 1, "source": "env:HOLON_SECRET_K", "set_at": 0, "problem": "HOLON_SECRET_K isn't set" }] });
        assert_eq!(list_lines(&broken).last().unwrap(), "k: HOLON_SECRET_K isn't set");
        assert_eq!(list_lines(&json!({ "names": [], "secrets": [] })), ["it reads no secret, and none is set for it"]);
        let shared = json!({ "names": ["ops_key", "other"], "reads": ["ops_key"], "secrets": [{ "name": "ops_key", "version": 1, "source": "local", "set_at": 0, "server": true }, { "name": "other", "version": 1, "source": "local", "set_at": 0, "server": true }] });
        assert_eq!(list_lines(&shared), ["name     version  source                set", "ops_key  v1       the server's (local)  1970-01-01T00:00:00"], "the server's it doesn't read aren't listed");
    }

    #[test]
    fn a_secrets_history_and_a_rollbacks_dry_run_show_versions_and_fingerprints() {
        let v = json!({ "name": "api_key", "versions": [{ "version": 1, "source": "a value", "set_at": 0, "current": false }, { "version": 2, "source": "a value", "set_at": 60, "current": true }],
            "events": [{ "at": 60, "slug": "shop", "name": "api_key", "action": "set", "version": 2, "source": "a value", "by": "maria" }] });
        assert_eq!(
            history_lines(&v),
            ["api_key:", "  v1   1970-01-01T00:00:00  from a value", "  v2   1970-01-01T00:01:00  from a value  (current)", "  done:", "    1970-01-01T00:01:00  set v2 from a value by maria"]
        );
        let preview = json!({ "to": 1, "current": 2, "version": 3, "source": "a value", "to_print": "a1b2c3d4", "current_print": "e5f6a7b8" });
        assert_eq!(rollback_preview("api_key", &preview), "api_key: would make v1 (#a1b2c3d4, from a value) current again as v3, in place of v2 (#e5f6a7b8). Nothing changed: run it again without --dry-run.");
    }
}
