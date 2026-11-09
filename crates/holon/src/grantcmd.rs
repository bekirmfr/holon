//! What deployments are granted, from the command line, as the editor's grant panel and the admin
//! page's permissions have it: `holon grant` shows one deployment's grant and changes it (revoke,
//! grant back, lower a limit), `holon grants` lists who holds what, `holon policy` shows the
//! host's policy and reloads it.

use serde_json::{json, Map, Value};

use crate::cmdline::{self, seg, when, SIGN_IN};

const GRANT_USAGE: &str = "usage: holon grant <slug> [--json]
       holon grant <slug> [--revoke ITEM]… [--grant ITEM]… [--lower LIMIT=N]…
                          [--keep-running | --pause-when-uncovered]
       (each with [--server PORT|URL])

  Prints what a deployment is granted: each item, those revoked since, the limits lowered, and
  what its version asks for that its grant doesn't cover. The options change it without a
  redeploy, as the editor's grant panel does, all at once or not at all. Editors and admins.

  --revoke ITEM            take an item back, named as this prints it (repeatable)
  --grant ITEM             give back an item its version asks for (repeatable)
  --lower LIMIT=N          hold a limit below what it is (repeatable); raising one takes a
                           deploy
  --keep-running           from now on, when its grant no longer covers it, keep it running:
                           the calls it lacks fail
  --pause-when-uncovered   from now on, pause it instead until its grant covers it (the default)
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon grant status-board
  holon grant status-board --revoke 'Unrecorded(\"GET /health\")'
  holon grant status-board --grant 'Unrecorded(\"GET /health\")'
  holon grant shop --lower request_timeout_ms=2000 --keep-running
";

const GRANTS_USAGE: &str = "usage: holon grants [--json] [--server PORT|URL]

  Lists who holds what: each deployment's granted items and those revoked since, its status,
  and why it can't run, if it can't. Admins.

  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon grants
  holon grants --json
";

const POLICY_USAGE: &str = "usage: holon policy [options]

  Prints the host's policy: each limit's default and ceiling, what is granted without asking,
  whether flows may call plain http or private addresses, and how the server looks after its
  disks. The options other than --reload and --json change it, in the policy file's [storage]
  section; they are an admin's.

  --reload                 read the policy file again and hold every deployment to it at once
                           (a lowered ceiling pauses a deployment it no longer covers); editors
                           and admins
  --sweep-every TIME       how often the server sweeps its store and the folders' kept texts:
                           90s, 10m, 2h, 1d (a minute at least), or off (only when an admin
                           asks)
  --warn-at LEVEL          when a disk it keeps things on is full enough to warn of: how full
                           (85%), or how much is left (5G, 500M, 1T)
  --full-at LEVEL          past --warn-at, when to act, in the same terms (95%, 1G)
  --when-full ACTION       what to do then: warn (only), pause (every flow that records runs,
                           until the disk is back under --warn-at) or roll (cut the oldest runs
                           of the logs keeping the most, and sweep)
  --backup DIR|none        a folder (an external drive, one a cloud client keeps in sync) the
                           runs a log lets go of are written to first, as gzipped exports holon
                           verify checks; none: they go
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon policy
  holon policy --reload
  holon policy --warn-at 80% --full-at 2G --when-full pause
  holon policy --sweep-every 1h --backup /mnt/backup/holon
";

fn usage(text: &str) -> String {
    format!("{text}\n{SIGN_IN}")
}

pub(crate) fn grant(args: &[String]) -> i32 {
    const WHO: &str = "holon grant";
    let usage = usage(GRANT_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["--revoke", "--grant", "--lower"], &["--keep-running", "--pause-when-uncovered", "--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let Some(slug) = args.word(0) else {
        eprintln!("{usage}");
        return 2;
    };
    let path = format!("/deployments/{}", seg(slug));
    let change = match change_of(&args) {
        Ok(c) => c,
        Err(why) => {
            eprintln!("{WHO}: {why}");
            return 2;
        }
    };
    let view = match change {
        Some(change) => cmdline::json(WHO, &args, "POST", &format!("{path}/grant"), Some(&change)),
        None => cmdline::json(WHO, &args, "GET", &format!("{path}/grant"), None),
    };
    let view = match view {
        Ok(v) => v,
        Err(code) => return code,
    };
    let manifest = match cmdline::json(WHO, &args, "GET", &format!("{path}/manifest"), None) {
        Ok(m) => m,
        Err(code) => return code,
    };
    if args.flag("--json") {
        cmdline::print_json(&json!({ "grant": view, "manifest": manifest }));
        return 0;
    }
    for line in grant_lines(slug, &view, &manifest) {
        println!("{line}");
    }
    0
}

/// The change the options ask for (`None`: none, only look).
pub(crate) fn change_of(args: &cmdline::Args) -> Result<Option<Value>, String> {
    let mut change = Map::new();
    let (revoke, grant) = (args.all("--revoke"), args.all("--grant"));
    if !revoke.is_empty() {
        change.insert("revoke".into(), json!(revoke));
    }
    if !grant.is_empty() {
        change.insert("grant".into(), json!(grant));
    }
    let mut lower = Map::new();
    for l in args.all("--lower") {
        let (name, n) = l.split_once('=').ok_or_else(|| format!("--lower is LIMIT=N (max_body_bytes=65536), not {l:?}"))?;
        let n: u64 = n.trim().parse().map_err(|_| format!("--lower {name}: {n:?} isn't a whole number"))?;
        lower.insert(name.trim().to_string(), json!(n));
    }
    if !lower.is_empty() {
        change.insert("lower".into(), Value::Object(lower));
    }
    match (args.flag("--keep-running"), args.flag("--pause-when-uncovered")) {
        (true, true) => return Err("--keep-running or --pause-when-uncovered, not both".into()),
        (true, false) => {
            change.insert("keep_running".into(), json!(true));
        }
        (false, true) => {
            change.insert("keep_running".into(), json!(false));
        }
        _ => {}
    }
    Ok((!change.is_empty()).then_some(Value::Object(change)))
}

fn words(v: &Value) -> String {
    match v {
        Value::Array(a) => a.iter().map(|i| i.as_str().map(String::from).unwrap_or_else(|| i.to_string())).collect::<Vec<_>>().join(", "),
        Value::Object(o) => o.iter().map(|(k, v)| format!("{k} = {v}")).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// A grant as the editor's grant panel shows it: `view` is `GET …/grant`'s answer (or a change's).
pub(crate) fn grant_lines(slug: &str, view: &Value, manifest: &Value) -> Vec<String> {
    let g = &view["grant"];
    let mut lines = vec![format!(
        "{slug}{}: granted {} by {}, grant {}",
        view["status"].as_str().map(|s| format!(" ({s})")).unwrap_or_default(),
        when(g["granted_at"].as_u64()),
        g["by"].as_str().unwrap_or("?"),
        &view["hash"].as_str().unwrap_or_default()[..view["hash"].as_str().map_or(0, |h| h.len().min(12))]
    )];
    let items: Vec<&str> = g["items"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if items.is_empty() {
        lines.push("  granted      nothing".into());
    }
    for (i, item) in items.iter().enumerate() {
        lines.push(format!("  {:<13}{item}", if i == 0 { "granted" } else { "" }));
    }
    if g["revoked"].as_array().is_some_and(|r| !r.is_empty()) {
        lines.push(format!("  revoked      {}  (--grant gives one back)", words(&g["revoked"])));
    }
    if g["lowered"].as_object().is_some_and(|l| !l.is_empty()) {
        lines.push(format!("  lowered      {}", words(&g["lowered"])));
    }
    if manifest["not_granted"].as_array().is_some_and(|n| !n.is_empty()) {
        lines.push(format!("  not granted  {}  (its version asks for it)", words(&manifest["not_granted"])));
    }
    lines.push(format!("  uncovered    {}", if g["keep_running"] == true { "keeps running, the calls it lacks failing" } else { "pauses until its grant covers it again" }));
    if let Some(why) = view["attention"].as_str() {
        lines.push(format!("  attention    {why}"));
    }
    let changes = g["changes"].as_array().cloned().unwrap_or_default();
    if !changes.is_empty() {
        lines.push("  changed".into());
        let from = changes.len().saturating_sub(5);
        for c in &changes[from..] {
            lines.push(format!("    {}  {}", when(c["at"].as_u64()), c["change"].as_str().unwrap_or_default()));
        }
        if from > 0 {
            lines.push(format!("    (and {from} before: holon history {slug})"));
        }
    }
    lines
}

pub(crate) fn grants(args: &[String]) -> i32 {
    const WHO: &str = "holon grants";
    let usage = usage(GRANTS_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &[], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let held = match cmdline::json(WHO, &args, "GET", "/admin/grants", None) {
        Ok(v) => v,
        Err(code) => return code,
    };
    if args.flag("--json") {
        cmdline::print_json(&held);
        return 0;
    }
    for line in held_lines(&held) {
        println!("{line}");
    }
    0
}

pub(crate) fn held_lines(held: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    for d in held.as_array().into_iter().flatten() {
        lines.push(format!("{} ({}){}", d["slug"].as_str().unwrap_or_default(), d["status"].as_str().unwrap_or_default(), d["attention"].as_str().map(|a| format!(": {a}")).unwrap_or_default()));
        lines.push(format!("  granted  {}", if d["items"].as_array().is_some_and(|i| !i.is_empty()) { words(&d["items"]) } else { "nothing".into() }));
        if d["revoked"].as_array().is_some_and(|r| !r.is_empty()) {
            lines.push(format!("  revoked  {}", words(&d["revoked"])));
        }
    }
    if lines.is_empty() {
        lines.push("nothing deployed".into());
    }
    lines
}

pub(crate) fn policy(args: &[String]) -> i32 {
    const WHO: &str = "holon policy";
    let usage = usage(POLICY_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["--sweep-every", "--warn-at", "--full-at", "--when-full", "--backup"], &["--reload", "--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let mut set = serde_json::Map::new();
    for (flag, key) in [("--sweep-every", "sweep_every"), ("--warn-at", "warn_at"), ("--full-at", "full_at"), ("--when-full", "when_full"), ("--backup", "backup")] {
        if let Some(v) = args.value(flag) {
            set.insert(key.into(), v.into());
        }
    }
    if !set.is_empty()
        && let Err(code) = cmdline::json(WHO, &args, "POST", "/admin/storage", Some(&serde_json::Value::Object(set)))
    {
        return code;
    }
    let answer = if args.flag("--reload") {
        cmdline::json(WHO, &args, "POST", "/permissions/reload", None)
    } else {
        cmdline::json(WHO, &args, "GET", "/permissions", None)
    };
    match answer {
        Ok(v) if args.flag("--json") => {
            cmdline::print_json(&v);
            0
        }
        Ok(v) => {
            for line in policy_lines(&v) {
                println!("{line}");
            }
            0
        }
        Err(code) => code,
    }
}

pub(crate) fn policy_lines(v: &Value) -> Vec<String> {
    let allowed = |b: &Value| if b == true { "allowed" } else { "refused" };
    let mut lines = vec![
        format!("granted unasked    {}", if v["auto"].as_array().is_some_and(|a| !a.is_empty()) { words(&v["auto"]) } else { "nothing".into() }),
        format!("plain http         {}", allowed(&v["allow_plain_http"])),
        format!("private addresses  {}", allowed(&v["allow_private_addresses"])),
        format!("agents propose     {}", if v["agents_propose"] == false { "refused: what needs a person is turned away" } else { "allowed: what needs a person waits for one" }),
        format!("sweeps             {}", v["storage"]["sweep_every"].as_str().map_or("every 10m".into(), |e| if e == "off" { "off: only an admin's".to_string() } else { format!("every {e}") })),
        format!("warns at           {}", v["storage"]["warn_at"].as_str().map_or("85%".to_string(), |w| if w.ends_with('%') { format!("{w} full") } else { format!("{w} free") })),
        format!("when full          {}", match (v["storage"]["full_at"].as_str(), v["storage"]["when_full"].as_str()) {
            (Some(at), Some(what)) => format!("past {at}: {what}"),
            _ => "past 95%: warn".to_string(),
        }),
        format!("backup             {}", v["storage"]["backup"].as_str().unwrap_or("none: what logs let go of goes")),
        String::new(),
    ];
    // Each limit: what a flow gets unless it asks, and the most it may ask for.
    let (defaults, ceilings) = (v["defaults"].as_object().cloned().unwrap_or_default(), v["ceilings"].as_object().cloned().unwrap_or_default());
    let mut names: Vec<&String> = defaults.keys().chain(ceilings.keys()).collect();
    names.sort();
    names.dedup();
    let mut rows = vec![["limit", "default", "ceiling"].map(String::from).to_vec()];
    let n = |m: &Map<String, Value>, k: &str| m.get(k).map_or("-".to_string(), Value::to_string);
    rows.extend(names.iter().map(|k| vec![k.to_string(), n(&defaults, k), n(&ceilings, k)]));
    lines.extend(cmdline::table(&rows));
    if let Some(attention) = v["attention"].as_object() {
        lines.push(String::new());
        lines.push("reloaded; each deployment now:".into());
        for (slug, why) in attention {
            lines.push(format!("  {slug}  {}", why.as_str().map(|w| format!("needs attention: {w}")).unwrap_or_else(|| "covered".into())));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> cmdline::Args {
        cmdline::Args::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>(), &["--revoke", "--grant", "--lower"], &["--keep-running", "--pause-when-uncovered"]).unwrap()
    }

    #[test]
    fn the_options_make_one_change_or_none() {
        assert_eq!(change_of(&args(&["shop"])), Ok(None));
        assert_eq!(
            change_of(&args(&["shop", "--revoke", "Clock", "--revoke", "net:api.example", "--lower", "max_body_bytes=1024", "--keep-running"])),
            Ok(Some(json!({ "revoke": ["Clock", "net:api.example"], "lower": { "max_body_bytes": 1024 }, "keep_running": true })))
        );
        assert!(change_of(&args(&["shop", "--lower", "max_body_bytes"])).unwrap_err().contains("LIMIT=N"));
        assert!(change_of(&args(&["shop", "--keep-running", "--pause-when-uncovered"])).is_err());
    }

    #[test]
    fn a_grant_shows_what_it_holds_and_what_it_doesnt_cover() {
        let view = json!({ "status": "paused", "hash": "0123456789abcdef", "attention": "it may no longer call api.example", "grant": {
            "items": ["Ingress", "net:api.example"], "revoked": ["net:api.example"], "lowered": { "max_body_bytes": 1024 },
            "granted_at": 0, "by": "deploy", "changes": [{ "at": 60, "change": "revoked net:api.example" }]
        } });
        let manifest = json!({ "not_granted": ["net:api.example"] });
        assert_eq!(
            grant_lines("shop", &view, &manifest),
            [
                "shop (paused): granted 1970-01-01T00:00:00 by deploy, grant 0123456789ab",
                "  granted      Ingress",
                "               net:api.example",
                "  revoked      net:api.example  (--grant gives one back)",
                "  lowered      max_body_bytes = 1024",
                "  not granted  net:api.example  (its version asks for it)",
                "  uncovered    pauses until its grant covers it again",
                "  attention    it may no longer call api.example",
                "  changed",
                "    1970-01-01T00:01:00  revoked net:api.example",
            ]
        );
    }

    #[test]
    fn the_policy_and_who_holds_what_read_as_lines() {
        let v = json!({ "defaults": { "max_body_bytes": 65536, "keep_runs": 500 }, "ceilings": { "keep_runs": 20000, "module_fuel": 1000 }, "auto": ["Clock"], "allow_plain_http": false, "allow_private_addresses": true, "agents_propose": false, "storage": { "sweep_every": "2h", "warn_at": "5G", "full_at": "1G", "when_full": "pause" }, "attention": { "shop": null, "beats": "a ceiling" } });
        assert_eq!(
            policy_lines(&v),
            [
                "granted unasked    Clock",
                "plain http         refused",
                "private addresses  allowed",
                "agents propose     refused: what needs a person is turned away",
                "sweeps             every 2h",
                "warns at           5G free",
                "when full          past 1G: pause",
                "backup             none: what logs let go of goes",
                "",
                "limit           default  ceiling",
                "keep_runs       500      20000",
                "max_body_bytes  65536    -",
                "module_fuel     -        1000",
                "",
                "reloaded; each deployment now:",
                "  beats  needs attention: a ceiling",
                "  shop  covered",
            ]
        );
        let held = json!([{ "slug": "shop", "status": "running", "attention": null, "items": ["Ingress", "StateWrite"], "revoked": [] }]);
        assert_eq!(held_lines(&held), ["shop (running)", "  granted  Ingress, StateWrite"]);
    }
}
