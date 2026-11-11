//! Who may do what on the server, from the command line, as `holon people` has its principals:
//! `holon roles` lists and defines roles, `holon groups` makes groups of principals, `holon bind`
//! and `holon unbind` bind a role to a principal or a group where it holds, and `holon access`
//! reads every version of it. Each change is a version kept for good, with who made it and why.

use serde_json::{json, Value};

use crate::can::Action;
use crate::cmdline::{self, seg, table, when, SIGN_IN};

const ROLES_USAGE: &str = "usage: holon roles [list]
       holon roles define <name> --allow ACTION,… [--kinds KIND,…] [--about TEXT] [--why TEXT]
       holon roles remove <name> [--why TEXT]
       (each with [--json] [--server PORT|URL])

  The roles principals are bound: the built-in ones (viewer, agent, editor, admin), and the ones an
  admin defines. A role allows actions from the catalogue to the kinds of principal it names.

  list                     every role: who may hold it, how many bindings hold it, what it allows
  define <name>            a role of your own, or a new version of one (each binding of it changes)
    --allow ACTION,…       what it allows, as the catalogue names them (below)
    --kinds KIND,…         who may hold it: person (the default), agent, service
    --about TEXT           what it is for
  remove <name>            a role nothing binds any more (holon access show lists what does)
  --why TEXT               why, kept with the version
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

  You define a role with only what you hold on this host. Whatever role it holds, an agent never
  approves a deploy, starts, stops, deletes or restores a flow, writes a secret, decides an
  approval, operates the host, or reads or changes people, roles or resources; a service never
  decides an approval.

examples:
  holon roles define payout-approver --allow run.view,run.decide --about \"decides payouts\"
  holon roles
  holon roles remove payout-approver
";

const GROUPS_USAGE: &str = "usage: holon groups [list]
       holon groups add <group> <name>… [--about TEXT] [--why TEXT]
       holon groups drop <group> <name>… [--why TEXT]
       holon groups remove <group> [--why TEXT]
       (each with [--json] [--server PORT|URL])

  Groups of principals. A role bound to a group (holon bind --group) holds for each member whose
  kind the role allows; a member is a person, an agent or a service, never a group.

  list                     each group: its members, and how many bindings are to it
  add <group> <name>…      make the group, or add members to it
    --about TEXT           what it is for
  drop <group> <name>…     take members out of it
  remove <group>           a group nothing is bound to any more
  --why TEXT               why, kept with the version
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

  A member added holds what the group is bound, so you add one only holding that yourself.

examples:
  holon groups add payments bob cy --about \"decide payouts\"
  holon groups drop payments cy
  holon groups
";

const BIND_USAGE: &str = "usage: holon bind <role> (--to NAME | --group GROUP) [--flow SLUG | --approval FLOW:POINT]
                         [--until WHEN] [--why TEXT] [--json] [--server PORT|URL]
       holon unbind <id> [--why TEXT] [--server PORT|URL]

  Bind a role to a principal or a group: on this host (the default), on one flow, or at one
  approval point of a flow, until a time if you say. Unbind it by its id (holon access show lists
  them). A binding gives only what you hold where it is made.

  --to NAME                a person, an agent or a service
  --group GROUP            a group (holon groups): it holds for each member whose kind the role allows
  --flow SLUG              on that flow only: its runs, state, secrets and approvals
  --approval FLOW:POINT    deciding that approval point of that flow, and nothing else, not even
                           viewing its runs
  --until WHEN             until a date, in UTC (2026-12-01, 2026-12-01T09:00), or for a while
                           (30m, 12h, 7d, 2w); it holds nothing after, and stays until unbound
  --why TEXT               why, kept with the version
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon bind editor --to bob --flow payout --why \"owns the payout flow\"
  holon bind payout-approver --group payments --flow payout
  holon bind editor --to dee --approval payout:ok --until 7d
  holon unbind 12 --why \"back from leave\"
";

const ACCESS_USAGE: &str = "usage: holon access [versions]
       holon access show [--version N]
       (each with [--json] [--server PORT|URL])

  Who may do what on this server, as kept: every change is a version, with who made it, when, what
  it changed and why. It holds no password or token, so it can be handed to an auditor.

  versions                 every version, newest first (what holon access does alone)
  show                     one version whole: its roles, groups and bindings
    --version N            that version (the newest by default)
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon access
  holon access show --version 3
";

/// The usage, with where the server is, and how to sign in.
fn usage(text: &str) -> String {
    format!("{text}\n{SIGN_IN}")
}

/// Print `v` as JSON when `--json` asks, and say so: whether the caller is done printing.
fn as_json(args: &cmdline::Args, v: &Value) -> bool {
    if args.flag("--json") {
        cmdline::print_json(v);
    }
    args.flag("--json")
}

/// The `?why=` of a request, when one is given.
fn why_query(args: &cmdline::Args) -> String {
    args.value("--why").map(|w| format!("?why={}", seg(w))).unwrap_or_default()
}

pub(crate) fn roles(args: &[String]) -> i32 {
    const WHO: &str = "holon roles";
    let usage = usage(ROLES_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["--allow", "--kinds", "--about", "--why"], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    match (args.word(0), args.word(1), args.word(2)) {
        (None | Some("list"), None, None) => match cmdline::json(WHO, &args, "GET", "/roles", None) {
            Ok(v) => {
                if !as_json(&args, &v) {
                    role_lines(&v).iter().for_each(|l| println!("{l}"));
                }
                0
            }
            Err(code) => code,
        },
        (Some("define"), Some(name), None) => {
            let Some(allow) = args.value("--allow") else {
                eprintln!("{WHO}: define {name} --allow ACTION,…: what it allows, as the catalogue names them ({})", catalogue());
                return 2;
            };
            let allows = listed(allow);
            if let Some(a) = allows.iter().find(|a| Action::named(a).is_none()) {
                eprintln!("{WHO}: {a:?} isn't an action: the catalogue names {}", catalogue());
                return 2;
            }
            let kinds = listed(args.value("--kinds").unwrap_or("person"));
            if let Some(k) = kinds.iter().find(|k| !["person", "agent", "service"].contains(&k.as_str())) {
                eprintln!("{WHO}: {k:?} isn't a kind: person, agent or service");
                return 2;
            }
            let body = json!({ "allows": allows, "kinds": kinds, "about": args.value("--about"), "why": args.value("--why") });
            match cmdline::json(WHO, &args, "PUT", &format!("/roles/{}", seg(name)), Some(&body)) {
                Ok(v) => {
                    if !as_json(&args, &v) {
                        let role = &v["role"];
                        println!("{name}: allows {}, held by {} (version {})", words(&role["allows"]).join(", "), words(&role["kinds"]).join(" or "), v["version"]);
                    }
                    0
                }
                Err(code) => code,
            }
        }
        (Some("remove"), Some(name), None) => match cmdline::json(WHO, &args, "DELETE", &format!("/roles/{}{}", seg(name), why_query(&args)), None) {
            Ok(_) => {
                println!("{name}: removed");
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

pub(crate) fn groups(args: &[String]) -> i32 {
    const WHO: &str = "holon groups";
    let usage = usage(GROUPS_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["--about", "--why"], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let names: Vec<String> = args.words.iter().skip(2).cloned().collect();
    match (args.word(0), args.word(1)) {
        (None | Some("list"), None) => match cmdline::json(WHO, &args, "GET", "/groups", None) {
            Ok(v) => {
                if !as_json(&args, &v) {
                    group_lines(&v).iter().for_each(|l| println!("{l}"));
                }
                0
            }
            Err(code) => code,
        },
        (Some(change @ ("add" | "drop")), Some(group)) if !names.is_empty() => {
            let all = match cmdline::json(WHO, &args, "GET", "/groups", None) {
                Ok(v) => v,
                Err(code) => return code,
            };
            let now = all["groups"].as_array().into_iter().flatten().find(|g| g["name"] == group);
            if now.is_none() && change == "drop" {
                eprintln!("{WHO}: there is no group {group:?} (holon groups lists them)");
                return 1;
            }
            let mut members: Vec<String> = now.map(|g| words(&g["members"])).unwrap_or_default();
            if change == "add" {
                members.extend(names.iter().filter(|n| !members.contains(n)).cloned().collect::<Vec<_>>());
            } else {
                members.retain(|m| !names.contains(m));
            }
            let about = args.value("--about").map(String::from).or_else(|| now.and_then(|g| g["about"].as_str().map(String::from)));
            let body = json!({ "members": members, "about": about, "why": args.value("--why") });
            match cmdline::json(WHO, &args, "PUT", &format!("/groups/{}", seg(group)), Some(&body)) {
                Ok(v) => {
                    if !as_json(&args, &v) {
                        let members = words(&v["group"]["members"]);
                        let members = if members.is_empty() { "no one".to_string() } else { members.join(", ") };
                        println!("{group}: {members} (version {})", v["version"]);
                    }
                    0
                }
                Err(code) => code,
            }
        }
        (Some("remove"), Some(group)) if names.is_empty() => match cmdline::json(WHO, &args, "DELETE", &format!("/groups/{}{}", seg(group), why_query(&args)), None) {
            Ok(_) => {
                println!("{group}: removed");
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

pub(crate) fn bind(args: &[String]) -> i32 {
    const WHO: &str = "holon bind";
    let usage = usage(BIND_USAGE);
    let valued = ["--to", "--group", "--flow", "--approval", "--until", "--why"];
    let args = match cmdline::parse(WHO, &usage, args, &valued, &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (Some(role), None) = (args.word(0), args.word(1)) else {
        eprintln!("{usage}");
        return 2;
    };
    let who = match (args.value("--to"), args.value("--group")) {
        (Some(name), None) => json!({ "principal": name }),
        (None, Some(group)) => json!({ "group": group }),
        _ => {
            eprintln!("{WHO}: bind {role} --to NAME or --group GROUP, one of them");
            return 2;
        }
    };
    let scope = match scope(args.value("--flow"), args.value("--approval")) {
        Ok(scope) => scope,
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 2;
        }
    };
    let now = crate::deploy::epoch_now();
    let expires_at = match args.value("--until").map(|u| until(u, now)).transpose() {
        Ok(at) => at,
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 2;
        }
    };
    let body = json!({ "who": who, "role": role, "scope": scope, "expires_at": expires_at, "why": args.value("--why") });
    let v = match cmdline::json(WHO, &args, "POST", "/bindings", Some(&body)) {
        Ok(v) => v,
        Err(code) => return code,
    };
    if as_json(&args, &v) {
        return 0;
    }
    let b = &v["binding"];
    let until = b["expires_at"].as_u64().map(|at| format!(", until {}", when(Some(at)))).unwrap_or_default();
    println!("bound {role} to {} {}{until} (binding {}, version {})", subject(&b["who"]), place(&b["scope"]), b["id"], v["version"]);
    if let Some((flow, point)) = args.value("--approval").and_then(|a| a.split_once(':'))
        && !sees_runs(WHO, &args, &who, flow)
    {
        let to = if who["group"].is_string() { format!("--group {}", who["group"].as_str().unwrap_or_default()) } else { format!("--to {}", who["principal"].as_str().unwrap_or_default()) };
        println!("{} decides {flow}'s approval {point:?} but can't see its runs: holon bind viewer {to} --flow {flow}", subject(&who));
    }
    0
}

pub(crate) fn unbind(args: &[String]) -> i32 {
    const WHO: &str = "holon unbind";
    let usage = usage(BIND_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["--why"], &[]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let (Some(id), None) = (args.word(0), args.word(1)) else {
        eprintln!("{usage}");
        return 2;
    };
    // What it bound, to say so once it is gone.
    let all = match cmdline::json(WHO, &args, "GET", "/bindings", None) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let n = id.parse::<u64>().ok();
    let Some(b) = all["bindings"].as_array().into_iter().flatten().find(|b| n.is_some() && b["id"].as_u64() == n).cloned() else {
        eprintln!("{WHO}: there is no binding {id} (holon access show lists them)");
        return 1;
    };
    match cmdline::json(WHO, &args, "DELETE", &format!("/bindings/{}{}", seg(id), why_query(&args)), None) {
        Ok(_) => {
            println!("unbound {} from {} {}", b["role"].as_str().unwrap_or_default(), subject(&b["who"]), place(&b["scope"]));
            0
        }
        Err(code) => code,
    }
}

pub(crate) fn access(args: &[String]) -> i32 {
    const WHO: &str = "holon access";
    let usage = usage(ACCESS_USAGE);
    let args = match cmdline::parse(WHO, &usage, args, &["--version"], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    match (args.word(0), args.word(1)) {
        (None | Some("versions"), None) => match cmdline::json(WHO, &args, "GET", "/access/versions", None) {
            Ok(v) => {
                if !as_json(&args, &v) {
                    version_lines(&v).iter().for_each(|l| println!("{l}"));
                }
                0
            }
            Err(code) => code,
        },
        (Some("show"), None) => {
            let path = match args.value("--version") {
                Some(n) if n.parse::<u64>().is_ok() => format!("/access?version={n}"),
                Some(n) => {
                    eprintln!("{WHO}: --version is a number, not {n:?}");
                    return 2;
                }
                None => "/access".into(),
            };
            match cmdline::json(WHO, &args, "GET", &path, None) {
                Ok(v) => {
                    if !as_json(&args, &v) {
                        document_lines(&v, crate::deploy::epoch_now()).iter().for_each(|l| println!("{l}"));
                    }
                    0
                }
                Err(code) => code,
            }
        }
        _ => {
            eprintln!("{usage}");
            2
        }
    }
}

/// The catalogue, as `--allow` takes it.
fn catalogue() -> String {
    Action::ALL.map(Action::name).join(", ")
}

/// `a,b, c` as its items.
fn listed(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// A JSON array of strings, as strings.
fn words(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect()
}

/// Where `--flow` or `--approval` says a binding holds: the host when neither does.
fn scope(flow: Option<&str>, approval: Option<&str>) -> Result<Value, String> {
    match (flow, approval) {
        (None, None) => Ok(json!("host")),
        (Some(slug), None) => Ok(json!({ "flows": { "slug": slug } })),
        (None, Some(at)) => match at.split_once(':') {
            Some((slug, point)) if !slug.is_empty() && !point.is_empty() => Ok(json!({ "approval": { "flow": { "slug": slug }, "name": point } })),
            _ => Err(format!("--approval is FLOW:POINT, a flow's slug and its approval point's name (payout:ok), not {at:?}")),
        },
        (Some(_), Some(_)) => Err("--flow or --approval, not both: --approval names its flow".into()),
    }
}

/// When `--until` says, in seconds since 1970, from `now`: a date in UTC (`2026-12-01`,
/// `2026-12-01T09:00`, `2026-12-01T09:00:30`), or a while (`30m`, `12h`, `7d`, `2w`).
fn until(text: &str, now: u64) -> Result<u64, String> {
    let wrong = || format!("--until is a date in UTC (2026-12-01, 2026-12-01T09:00) or a while (30m, 12h, 7d, 2w), not {text:?}");
    if let Some(unit) = text.chars().last().filter(|c| c.is_ascii_alphabetic()) {
        let n: u64 = text[..text.len() - 1].parse().map_err(|_| wrong())?;
        let seconds = match unit {
            'm' => 60,
            'h' => 3_600,
            'd' => 86_400,
            'w' => 7 * 86_400,
            _ => return Err(wrong()),
        };
        return Ok(now + n * seconds);
    }
    let (date, time) = text.split_once('T').unwrap_or((text, "00:00"));
    let date: Vec<i64> = date.split('-').map(str::parse).collect::<Result<_, _>>().map_err(|_| wrong())?;
    let time: Vec<i64> = time.split(':').map(str::parse).collect::<Result<_, _>>().map_err(|_| wrong())?;
    let (&[y, m, d], &[h, min, ref s @ ..]) = (&date[..], &time[..]) else { return Err(wrong()) };
    let s = match s {
        [] => 0,
        [s] => *s,
        _ => return Err(wrong()),
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(0..24).contains(&h) || !(0..60).contains(&min) || !(0..60).contains(&s) {
        return Err(wrong());
    }
    // Days since 1970 (Hinnant's days_from_civil).
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from(days * 86_400 + h * 3_600 + min * 60 + s).map_err(|_| wrong())
}

/// Whom a binding is to, as a sentence says it: `bob`, `the group payments`.
fn subject(who: &Value) -> String {
    match (who["principal"].as_str(), who["group"].as_str()) {
        (Some(name), _) => name.into(),
        (_, Some(group)) => format!("the group {group}"),
        _ => "-".into(),
    }
}

/// Where a binding holds, as the server's words say it: `on this host`, `on "payout"`,
/// `for "payout"'s approval "ok"`.
pub(crate) fn place(scope: &Value) -> String {
    if let Some(slug) = scope["flows"]["slug"].as_str() {
        return format!("on {slug:?}");
    }
    if let (Some(slug), Some(point)) = (scope["approval"]["flow"]["slug"].as_str(), scope["approval"]["name"].as_str()) {
        return format!("for {slug:?}'s approval {point:?}");
    }
    "on this host".into()
}

/// Where a binding holds, short, as the flags write it: nothing for the host, `on payout`,
/// `at payout:ok`.
fn short_place(scope: &Value) -> String {
    if let Some(slug) = scope["flows"]["slug"].as_str() {
        return format!(" on {slug}");
    }
    if let (Some(slug), Some(point)) = (scope["approval"]["flow"]["slug"].as_str(), scope["approval"]["name"].as_str()) {
        return format!(" at {slug}:{point}");
    }
    String::new()
}

/// A principal's bindings, as `holon people` lists them: `viewer, editor on payout via payments,
/// editor at payout:ok until 2026-12-01T00:00:00 (expired)`.
pub(crate) fn bindings_said(bindings: &Value) -> String {
    let each = bindings.as_array().into_iter().flatten().map(|b| {
        let via = b["group"].as_str().map(|g| format!(" via {g}")).unwrap_or_default();
        let until = b["expires_at"].as_u64().map(|at| format!(" until {}", when(Some(at)))).unwrap_or_default();
        let expired = if b["expired"] == true { " (expired)" } else { "" };
        format!("{}{}{via}{until}{expired}", b["role"].as_str().unwrap_or_default(), short_place(&b["scope"]))
    });
    let said: Vec<String> = each.collect();
    if said.is_empty() { "-".into() } else { said.join(", ") }
}

/// Whether `who` (a principal or a group) views `flow`'s runs by what is bound to it now: a
/// binding on the host or on the flow, of a role allowing `run.view`. A group's are its own; a
/// principal's, its groups' too. Unknown (the caller may not read who holds what) counts as yes:
/// nothing is said.
fn sees_runs(who_says: &str, args: &cmdline::Args, who: &Value, flow: &str) -> bool {
    let views = |b: &Value, allows: &[String]| {
        b["expired"] != true && (b["scope"] == "host" || b["scope"]["flows"]["slug"] == flow) && allows.iter().any(|a| a == "run.view")
    };
    let reads = cmdline::json(who_says, args, "GET", "/auth/me/can?actions=people.view,role.view", None);
    if !reads.is_ok_and(|can| can["people.view"] == true && can["role.view"] == true) {
        return true;
    }
    if let Some(name) = who["principal"].as_str() {
        let Ok(people) = cmdline::json(who_says, args, "GET", "/people", None) else { return true };
        let Some(p) = people.as_array().into_iter().flatten().find(|p| p["name"] == name) else { return true };
        return p["bindings"].as_array().into_iter().flatten().any(|b| views(b, &words(&b["allows"])));
    }
    let group = who["group"].as_str().unwrap_or_default();
    let (Ok(bound), Ok(roles)) = (cmdline::json(who_says, args, "GET", &format!("/bindings?who={}", seg(group)), None), cmdline::json(who_says, args, "GET", "/roles", None)) else { return true };
    let allows = |role: &Value| roles["roles"].as_array().into_iter().flatten().find(|r| r["name"] == *role).map(|r| words(&r["allows"])).unwrap_or_default();
    bound["bindings"].as_array().into_iter().flatten().filter(|b| b["who"]["group"] == group).any(|b| views(b, &allows(&b["role"])))
}

/// Every role, as columns: what it allows last, "everything" for one allowing the whole catalogue.
fn role_lines(v: &Value) -> Vec<String> {
    let mut rows = vec![["role", "held by", "bound", "allows"].map(String::from).to_vec()];
    for r in v["roles"].as_array().into_iter().flatten() {
        let allows = words(&r["allows"]);
        let allows = if allows.len() == Action::ALL.len() { "everything".to_string() } else { allows.join(", ") };
        let name = r["name"].as_str().unwrap_or_default();
        let about = r["about"].as_str().filter(|_| r["builtin"] != true).map(|a| format!(" ({a})")).unwrap_or_default();
        rows.push(vec![format!("{name}{about}"), words(&r["kinds"]).join(", "), r["bound"].to_string(), allows]);
    }
    table(&rows)
}

/// Every group, as columns, with what it is for.
fn group_lines(v: &Value) -> Vec<String> {
    let groups = v["groups"].as_array().into_iter().flatten();
    let mut rows = vec![["group", "bound", "members"].map(String::from).to_vec()];
    for g in groups {
        let about = g["about"].as_str().map(|a| format!(" ({a})")).unwrap_or_default();
        rows.push(vec![format!("{}{about}", g["name"].as_str().unwrap_or_default()), g["bound"].to_string(), words(&g["members"]).join(", ")]);
    }
    if rows.len() == 1 {
        return vec!["no groups yet: holon groups add GROUP NAME… makes one".into()];
    }
    table(&rows)
}

/// Every version, newest first, as columns.
fn version_lines(v: &Value) -> Vec<String> {
    let mut rows = vec![["version", "at", "by", "what", "why"].map(String::from).to_vec()];
    for x in v["versions"].as_array().into_iter().flatten() {
        rows.push(vec![x["version"].to_string(), when(x["at"].as_u64()), x["by"].as_str().unwrap_or("-").into(), x["what"].as_str().unwrap_or_default().into(), x["why"].as_str().unwrap_or("").into()]);
    }
    table(&rows)
}

/// One version whole: who made it, then its roles (an admin's), groups and bindings.
fn document_lines(v: &Value, now: u64) -> Vec<String> {
    let why = v["why"].as_str().map(|w| format!(" (why: {w})")).unwrap_or_default();
    let mut lines = vec![format!("version {}, {} by {}: {}{why}", v["version"], when(v["at"].as_u64()), v["by"].as_str().unwrap_or("-"), v["what"].as_str().unwrap_or_default())];
    let roles = v["roles"].as_array().into_iter().flatten();
    let mut rows = vec![["role", "held by", "allows"].map(String::from).to_vec()];
    rows.extend(roles.map(|r| vec![r["name"].as_str().unwrap_or_default().into(), words(&r["kinds"]).join(", "), words(&r["allows"]).join(", ")]));
    if rows.len() > 1 {
        lines.push(String::new());
        lines.extend(table(&rows));
    }
    if v["groups"].as_array().is_some_and(|g| !g.is_empty()) {
        lines.push(String::new());
        lines.extend(group_lines(&json!({ "groups": v["groups"].as_array().unwrap().iter().map(|g| json!({ "name": g["name"], "members": g["members"], "about": g["about"], "bound": v["bindings"].as_array().into_iter().flatten().filter(|b| b["who"]["group"] == g["name"]).count() })).collect::<Vec<_>>() })));
    }
    let mut rows = vec![["id", "role", "to", "where", "until", "by"].map(String::from).to_vec()];
    for b in v["bindings"].as_array().into_iter().flatten() {
        let until = match b["expires_at"].as_u64() {
            Some(at) if at <= now => format!("{} (expired)", when(Some(at))),
            at => when(at),
        };
        rows.push(vec![b["id"].to_string(), b["role"].as_str().unwrap_or_default().into(), subject(&b["who"]), place(&b["scope"]), until, b["by"].as_str().unwrap_or("-").into()]);
    }
    lines.push(String::new());
    lines.extend(table(&rows));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn until_reads_a_date_in_utc_or_a_while() {
        let now = 1_800_000_000;
        assert_eq!(until("30m", now), Ok(now + 1_800));
        assert_eq!(until("12h", now), Ok(now + 43_200));
        assert_eq!(until("7d", now), Ok(now + 604_800));
        assert_eq!(until("2w", now), Ok(now + 1_209_600));
        assert_eq!(until("1970-01-01", now), Ok(0));
        assert_eq!(until("2026-12-01", now), Ok(1_796_083_200));
        assert_eq!(until("2026-12-01T09:00", now), Ok(1_796_083_200 + 32_400));
        assert_eq!(until("2000-02-29T23:59:59", now), Ok(951_868_799));
        // Each back as `when` writes it.
        assert_eq!(when(until("2026-12-01T09:00:30", now).ok()), "2026-12-01T09:00:30");
        for wrong in ["", "7", "7y", "d", "2026-13-01", "2026-12-01T25:00", "2026-12", "2026-12-01T09", "1969-12-31", "soon"] {
            assert!(until(wrong, now).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn a_scope_is_written_as_the_flags_say_it() {
        assert_eq!(scope(None, None), Ok(json!("host")));
        assert_eq!(scope(Some("payout"), None), Ok(json!({ "flows": { "slug": "payout" } })));
        assert_eq!(scope(None, Some("payout:ok")), Ok(json!({ "approval": { "flow": { "slug": "payout" }, "name": "ok" } })));
        assert!(scope(None, Some("payout")).is_err() && scope(None, Some(":ok")).is_err() && scope(Some("a"), Some("a:ok")).is_err());
        for s in [scope(None, None), scope(Some("payout"), None), scope(None, Some("payout:ok"))] {
            let s = s.unwrap();
            let said = (place(&s), short_place(&s));
            let want = match s {
                Value::String(_) => ("on this host", ""),
                _ if s["flows"].is_object() => ("on \"payout\"", " on payout"),
                _ => ("for \"payout\"'s approval \"ok\"", " at payout:ok"),
            };
            assert_eq!((said.0.as_str(), said.1.as_str()), want);
        }
    }

    #[test]
    fn roles_groups_versions_and_bindings_list_as_columns() {
        let everything: Vec<&str> = Action::ALL.map(Action::name).to_vec();
        let roles = json!({ "roles": [
            { "name": "viewer", "about": "reads", "allows": ["flow.view", "run.view"], "kinds": ["person", "agent", "service"], "builtin": true, "bound": 3 },
            { "name": "admin", "about": "all", "allows": everything, "kinds": ["person"], "builtin": true, "bound": 1 },
            { "name": "payout-approver", "about": "decides payouts", "allows": ["run.view", "run.decide"], "kinds": ["person"], "builtin": false, "bound": 0 },
        ] });
        assert_eq!(role_lines(&roles), [
            "role                               held by                 bound  allows",
            "viewer                             person, agent, service  3      flow.view, run.view",
            "admin                              person                  1      everything",
            "payout-approver (decides payouts)  person                  0      run.view, run.decide",
        ]);
        assert_eq!(group_lines(&json!({ "groups": [] })), ["no groups yet: holon groups add GROUP NAME… makes one"]);
        let groups = json!({ "groups": [{ "name": "payments", "members": ["bob", "cy"], "bound": 1, "about": "decide payouts" }, { "name": "ops", "members": [], "bound": 0 }] });
        assert_eq!(group_lines(&groups), ["group                      bound  members", "payments (decide payouts)  1      bob, cy", "ops                        0"]);
        let versions = json!({ "versions": [{ "version": 2, "at": 60, "by": "ada", "what": "bound editor to bob on \"d\"", "why": "on call" }, { "version": 1, "at": 0, "by": "ada", "what": "set up the server" }] });
        assert_eq!(version_lines(&versions), [
            "version  at                   by   what                        why",
            "2        1970-01-01T00:01:00  ada  bound editor to bob on \"d\"  on call",
            "1        1970-01-01T00:00:00  ada  set up the server",
        ]);
        let held = json!([
            { "role": "viewer", "scope": "host", "group": null, "expires_at": null, "expired": false },
            { "role": "editor", "scope": { "flows": { "slug": "d" } }, "group": "payments", "expires_at": null, "expired": false },
            { "role": "editor", "scope": { "approval": { "flow": { "slug": "d" }, "name": "ok" } }, "group": null, "expires_at": 60, "expired": true },
        ]);
        assert_eq!(bindings_said(&held), "viewer, editor on d via payments, editor at d:ok until 1970-01-01T00:01:00 (expired)");
        assert_eq!(bindings_said(&json!([])), "-");
        let document = json!({ "version": 4, "at": 0, "by": "ada", "what": "bound editor to the group payments on \"d\"", "roles": [], "groups": [{ "name": "payments", "members": ["bob"] }],
            "bindings": [{ "id": 0, "who": { "principal": "ada" }, "role": "admin", "scope": "host", "by": "ada", "at": 0 }, { "id": 3, "who": { "group": "payments" }, "role": "editor", "scope": { "flows": { "slug": "d" } }, "by": "ada", "at": 0, "expires_at": 30 }] });
        assert_eq!(document_lines(&document, 60), [
            "version 4, 1970-01-01T00:00:00 by ada: bound editor to the group payments on \"d\"",
            "",
            "group     bound  members",
            "payments  1      bob",
            "",
            "id  role    to                  where         until                          by",
            "0   admin   ada                 on this host  -                              ada",
            "3   editor  the group payments  on \"d\"        1970-01-01T00:00:30 (expired)  ada",
        ]);
    }
}
