//! The server's people from the command line, as its People page has them: `holon people` adds,
//! changes and removes people, agents and services, and makes and revokes an agent's or a
//! service's tokens; `holon oidc` sets signing in through a provider. Admins (and an owner, for
//! its tokens). An invite's link and a new token are printed once, as the page shows them once:
//! the server keeps only their hashes.

use serde_json::{json, Value};

use crate::cmdline::{self, seg, table, when, SIGN_IN};
use crate::local;

const PEOPLE_USAGE: &str = "usage: holon people [list]
       holon people add <name> --person --role viewer|editor|admin [--email ADDRESS]
       holon people add <name> --agent [--role agent|viewer] [--owner PERSON]
       holon people add <name> --service --role viewer|editor [--owner PERSON]
       holon people set <name> [--role ROLE] [--disable | --enable] [--email ADDRESS] [--owner PERSON]
                               [--display TEXT]
       holon people reset <name>
       holon people remove <name> --yes
       holon people token <name> [--label TEXT]
       holon people revoke <name> <id>
       (each with [--json] [--server PORT|URL])

  The server's people, agents and services, as its People page has them. Admins; an agent's or a
  service's owner also makes and revokes its tokens.

  list                     everyone: kind, role, owner, email, since when and by whom, and whether
                           disabled or still invited; then the agents' and services' tokens
  add <name>               someone new, of one kind:
    --person               signs in with a password (or a provider): the link they open to choose
                           one is printed once, to send them
    --agent                an agent, which signs in with its own tokens (holon mcp, or one with a
                           shell): reads, proposes and calls, never approves
    --service              a script or CI, which signs in with its own tokens
    --role ROLE            a person: viewer (reads), editor (also saves, deploys, runs) or admin;
                           an agent: agent (the default) or viewer; a service: viewer or editor
    --owner PERSON         an agent's or a service's: the person answerable for it (you, by default)
    --email ADDRESS        a person's: the email a provider signs them in by (holon oidc)
  set <name>               change someone
    --role ROLE            their role, one their kind may hold
    --disable              sign them out for good (an agent's or a service's tokens stop), until
                           --enable
    --enable               let them sign in again
    --email ADDRESS        a person's email (\"\" clears it)
    --owner PERSON         an agent's or a service's new owner
    --display TEXT         how they are shown (\"\" shows the name)
  reset <name>             clear a person's password and end their sessions, with a new link
  remove <name>            delete them, and an agent's or a service's tokens; --yes says you mean it
  token <name>             a new token for an agent or a service, printed once: keep it where it
                           reads it (HOLON_TOKEN)
    --label TEXT           what it is for (\"CI on GitHub\")
  revoke <name> <id>       end one of its tokens at once
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon people add ada --person --role editor --email ada@example.com
  holon people add ci --service --role editor
  holon people token ci --label \"CI on GitHub\" > ci.token
  holon people add refunds-bot --agent --owner ada
  holon people set ada --role admin
  holon people remove bob --yes
";

const OIDC_USAGE: &str = "usage: holon oidc [show]
       holon oidc set <issuer> <client-id>
       holon oidc off
       (each with [--server PORT|URL])

  Signing in through a provider (OpenID Connect). A person signs in as the account whose email
  the provider has verified. Admins.

  show                     the issuer and client id it uses, if any (what holon oidc does alone)
  set <issuer> <client-id> sign in through that issuer, with the client id this server is
                           registered with there
  off                      stop signing in through a provider; passwords still work
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon oidc set https://accounts.google.com 1234.apps.googleusercontent.com
  holon oidc
  holon oidc off
";

/// Where a person opens an invite: the editor, at the server's address.
pub(crate) fn invite_link(server: &str, token: &str) -> String {
    format!("{}/?invite={}", server.trim_end_matches('/'), seg(token))
}

/// Ask the server, as `cmdline::json` does, keeping where it is (for an invite's link).
fn ask(who: &str, args: &cmdline::Args, method: &str, path: &str, body: Option<&Value>) -> Result<(String, Value), i32> {
    let empty = json!({});
    let body = body.or((method == "POST").then_some(&empty));
    let (server, answer) = local::admin_call(who, args.server(), method, path, body)?;
    Ok((server, serde_json::from_str(&answer).unwrap_or(Value::Null)))
}

/// The roles each kind may hold, as the server checks them (accounts.rs).
fn roles_of(kind: &str) -> &'static [&'static str] {
    match kind {
        "person" => &["viewer", "editor", "admin"],
        "agent" => &["agent", "viewer"],
        "service" => &["viewer", "editor"],
        _ => &["viewer", "editor", "admin", "agent"],
    }
}

pub(crate) fn people(args: &[String]) -> i32 {
    const WHO: &str = "holon people";
    let usage = format!("{PEOPLE_USAGE}\n{SIGN_IN}");
    let valued = ["--role", "--email", "--owner", "--display", "--label"];
    let flags = ["--person", "--agent", "--service", "--disable", "--enable", "--yes", "--json"];
    let args = match cmdline::parse(WHO, &usage, args, &valued, &flags) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let path = |name: &str| format!("/people/{}", seg(name));
    let invited = |(server, v): (String, Value), what: &str| {
        let name = v["name"].as_str().unwrap_or_default();
        println!("{name}: {what}. Send them this link, to choose a password (it works once, within seven days, and is shown only now):");
        println!("{}", invite_link(&server, v["invite"].as_str().unwrap_or_default()));
        0
    };
    let one = |v: &Value| {
        println!("{}", people_lines(&json!([v]))[1]);
        0
    };
    match (args.word(0), args.word(1), args.word(2)) {
        (None | Some("list"), None, None) => match ask(WHO, &args, "GET", "/people", None) {
            Ok((_, v)) if args.flag("--json") => {
                cmdline::print_json(&v);
                0
            }
            Ok((_, v)) => {
                for line in people_lines(&v).into_iter().chain(token_lines(&v)) {
                    println!("{line}");
                }
                0
            }
            Err(code) => code,
        },
        (Some("add"), Some(name), None) => {
            let kinds: Vec<&str> = ["person", "agent", "service"].into_iter().filter(|k| args.flag(&format!("--{k}"))).collect();
            let [kind] = kinds[..] else {
                eprintln!("{WHO}: add {name} with one of --person, --agent or --service");
                return 2;
            };
            let role = match (args.value("--role"), kind) {
                (Some(r), _) if roles_of(kind).contains(&r) => r,
                (None, "agent") => "agent",
                (r, _) => {
                    let said = r.map(|r| format!(", not {r:?}")).unwrap_or_default();
                    eprintln!("{WHO}: a {kind}'s --role is {}{said}", roles_of(kind).join(" or "));
                    return 2;
                }
            };
            let mut body = json!({ "name": name, "kind": kind, "role": role });
            for (flag, key) in [("--email", "email"), ("--owner", "owner"), ("--display", "display")] {
                if let Some(v) = args.value(flag) {
                    body[key] = json!(v);
                }
            }
            match ask(WHO, &args, "POST", "/people", Some(&body)) {
                Ok(answer) if kind == "person" => invited(answer, &format!("invited as {role}")),
                Ok((_, v)) => {
                    println!("{name}: {kind} with the {role} role, owned by {}. Make its token with: holon people token {name}", v["owner"].as_str().unwrap_or("-"));
                    0
                }
                Err(code) => code,
            }
        }
        (Some("reset"), Some(name), None) => ask(WHO, &args, "POST", &format!("{}/reset", path(name)), None).map_or_else(|c| c, |a| invited(a, "password cleared, signed out everywhere")),
        (Some("set"), Some(name), None) => {
            let mut body = serde_json::Map::new();
            if let Some(r) = args.value("--role") {
                if !roles_of("").contains(&r) {
                    eprintln!("{WHO}: --role is viewer, editor, admin or agent, not {r:?}");
                    return 2;
                }
                body.insert("role".into(), json!(r));
            }
            match (args.flag("--disable"), args.flag("--enable")) {
                (true, true) => {
                    eprintln!("{WHO}: --disable or --enable, not both");
                    return 2;
                }
                (d, e) if d || e => {
                    body.insert("disabled".into(), json!(d));
                }
                _ => {}
            }
            for (flag, key) in [("--email", "email"), ("--owner", "owner"), ("--display", "display")] {
                if let Some(v) = args.value(flag) {
                    body.insert(key.into(), json!(v));
                }
            }
            if body.is_empty() {
                eprintln!("{WHO}: set {name} with --role, --disable, --enable, --email, --owner or --display");
                return 2;
            }
            ask(WHO, &args, "PATCH", &path(name), Some(&Value::Object(body))).map_or_else(|c| c, |(_, v)| one(&v))
        }
        (Some("remove"), Some(name), None) => {
            if !args.flag("--yes") {
                eprintln!("{WHO}: this deletes {name:?}, and an agent's or a service's tokens; what they did stays in the activity. Run it again with --yes to go ahead.");
                return 2;
            }
            match ask(WHO, &args, "DELETE", &path(name), None) {
                Ok(_) => {
                    println!("{name}: removed");
                    0
                }
                Err(code) => code,
            }
        }
        (Some("token"), Some(name), None) => {
            let label = args.value("--label").unwrap_or("made with holon people token");
            match ask(WHO, &args, "POST", &format!("{}/tokens", path(name)), Some(&json!({ "label": label }))) {
                Ok((_, v)) => {
                    eprintln!("{name}: a new token, id {}. It is shown only now: keep it where it reads it (HOLON_TOKEN).", v["entry"]["id"].as_str().unwrap_or_default());
                    println!("{}", v["token"].as_str().unwrap_or_default());
                    0
                }
                Err(code) => code,
            }
        }
        (Some("revoke"), Some(name), Some(id)) => match ask(WHO, &args, "DELETE", &format!("{}/tokens/{}", path(name), seg(id)), None) {
            Ok(_) => {
                println!("{name}'s token {id}: revoked");
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

/// Everyone, as columns.
pub(crate) fn people_lines(v: &Value) -> Vec<String> {
    let mut rows = vec![["name", "kind", "role", "owner", "email", "since", "by", ""].map(String::from).to_vec()];
    for p in v.as_array().into_iter().flatten() {
        let note = match (p["disabled"] == true, p["invited"] == true, p["owner_disabled"] == true) {
            (true, _, _) => "disabled",
            (_, true, _) => "invited, not joined yet",
            (_, _, true) => "its owner is disabled",
            _ => "",
        };
        rows.push(vec![
            p["name"].as_str().unwrap_or_default().into(),
            p["kind"].as_str().unwrap_or_default().into(),
            p["role"].as_str().unwrap_or_default().into(),
            p["owner"].as_str().unwrap_or("-").into(),
            p["email"].as_str().unwrap_or("-").into(),
            when(p["created_at"].as_u64()),
            p["created_by"].as_str().unwrap_or("-").into(),
            note.into(),
        ]);
    }
    table(&rows)
}

/// The agents' and services' tokens, as columns, after a blank line; nothing when there are none.
pub(crate) fn token_lines(v: &Value) -> Vec<String> {
    let all: Vec<&Value> = v.as_array().into_iter().flatten().flat_map(|p| p["tokens"].as_array().into_iter().flatten()).collect();
    if all.is_empty() {
        return vec![];
    }
    let mut rows = vec![["token", "of", "label", "made", "by"].map(String::from).to_vec()];
    for t in all {
        rows.push(vec![
            t["id"].as_str().unwrap_or_default().into(),
            t["principal"].as_str().unwrap_or_default().into(),
            t["label"].as_str().unwrap_or_default().into(),
            when(t["created_at"].as_u64()),
            t["created_by"].as_str().unwrap_or("-").into(),
        ]);
    }
    std::iter::once(String::new()).chain(table(&rows)).collect()
}

pub(crate) fn oidc(args: &[String]) -> i32 {
    const WHO: &str = "holon oidc";
    let usage = format!("{OIDC_USAGE}\n{SIGN_IN}");
    let args = match cmdline::parse(WHO, &usage, args, &[], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let set = |settings: Value| ask(WHO, &args, "POST", "/admin/oidc", Some(&json!({ "settings": settings }))).map(|_| ());
    match (args.word(0), args.word(1), args.word(2)) {
        (None | Some("show"), None, None) => match ask(WHO, &args, "GET", "/admin/oidc", None) {
            Ok((_, v)) if args.flag("--json") => {
                cmdline::print_json(&v);
                0
            }
            Ok((_, v)) => {
                match v["settings"].as_object() {
                    Some(s) => println!("signing in through {}, as client {}", s["issuer"].as_str().unwrap_or_default(), s["client_id"].as_str().unwrap_or_default()),
                    None => println!("off: people sign in with a password"),
                }
                0
            }
            Err(code) => code,
        },
        (Some("set"), Some(issuer), Some(client_id)) => match set(json!({ "issuer": issuer, "client_id": client_id })) {
            Ok(()) => {
                println!("signing in through {issuer} is on");
                0
            }
            Err(code) => code,
        },
        (Some("off"), None, None) => match set(Value::Null) {
            Ok(()) => {
                println!("signing in through a provider is off");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_and_tokens_list_as_columns_and_an_invite_is_a_link_to_the_editor() {
        let people = json!([
            { "name": "ada", "kind": "person", "role": "admin", "email": null, "disabled": false, "created_at": 0, "created_by": null, "invited": false },
            { "name": "bob", "kind": "person", "role": "editor", "email": "bob@example.com", "disabled": false, "created_at": 60, "created_by": "ada", "invited": true },
            { "name": "ci", "kind": "service", "role": "editor", "owner": "bob", "owner_disabled": true, "disabled": false, "created_at": 0, "created_by": "ada", "invited": false,
              "tokens": [{ "id": "ab12", "principal": "ci", "label": "CI on GitHub", "created_at": 0, "created_by": "ada" }] },
        ]);
        assert_eq!(
            people_lines(&people),
            [
                "name  kind     role    owner  email            since                by",
                "ada   person   admin   -      -                1970-01-01T00:00:00  -",
                "bob   person   editor  -      bob@example.com  1970-01-01T00:01:00  ada  invited, not joined yet",
                "ci    service  editor  bob    -                1970-01-01T00:00:00  ada  its owner is disabled",
            ]
        );
        assert_eq!(token_lines(&people), ["", "token  of  label         made                 by", "ab12   ci  CI on GitHub  1970-01-01T00:00:00  ada"]);
        assert_eq!(token_lines(&json!([people[0]])), Vec::<String>::new(), "no tokens, no table");
        assert_eq!(invite_link("http://127.0.0.1:7420/", "hi_a+b"), "http://127.0.0.1:7420/?invite=hi_a%2Bb");
    }
}
