//! The server's accounts from the command line, as the admin page has them: `holon user` invites,
//! changes and removes people, `holon token` makes and revokes API tokens, `holon oidc` sets
//! signing in through a provider. Admins only. An invite's link and a new token are printed once,
//! as the admin page shows them once: the server keeps only their hashes.

use serde_json::{json, Value};

use crate::cmdline::{self, seg, table, when, SIGN_IN};
use crate::local;

const USER_USAGE: &str = "usage: holon user list
       holon user invite <name> --role viewer|editor|admin [--email ADDRESS]
       holon user set <name> [--role ROLE] [--disable | --enable] [--email ADDRESS]
       holon user reset <name>
       holon user remove <name> --yes
       (each with [--json] [--server PORT|URL])

  The server's people, as the admin page has them. Admins.

  list                     every account: its role and email, since when and by whom, and whether
                           it is disabled or still invited
  invite <name>            a new account, and the link its person opens to choose a password,
                           printed once: send it to them
    --role ROLE            viewer (reads), editor (also saves, deploys, runs) or admin
    --email ADDRESS        the email a provider signs them in by (holon oidc)
  set <name>               change an account
    --role ROLE            its role
    --disable              sign them out for good, until --enable
    --enable               let them sign in again
    --email ADDRESS        their email (\"\" clears it)
  reset <name>             clear their password and end their sessions, with a new link
  remove <name>            delete the account; --yes says you mean it
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon user invite ada --role editor --email ada@example.com
  holon user set ada --role admin
  holon user set bob --disable
  holon user remove bob --yes
";

const TOKEN_USAGE: &str = "usage: holon token list
       holon token add <label> --role viewer|editor|admin|agent
       holon token revoke <id>
       (each with [--json] [--server PORT|URL])

  API tokens, for scripts and CI (they sign in with HOLON_TOKEN), as the admin page has them.
  Admins.

  list                     every token: its id, label and role, when it was made and by whom
  add <label>              a new token, printed once: keep it where the script reads it
    --role ROLE            viewer, editor, admin, or agent (proposes, never approves)
  revoke <id>              end a token at once
  --json                   print what the server answers, as JSON
  --server PORT|URL        which holon server to ask (see below)

examples:
  holon token add ci --role editor
  holon token list
  holon token revoke 3f9a1c0b7d2e
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

const ROLES: [&str; 4] = ["viewer", "editor", "admin", "agent"];

fn role(who: &str, args: &cmdline::Args, agent: bool) -> Result<Option<String>, i32> {
    match args.value("--role") {
        None => Ok(None),
        Some(r) if ROLES[..if agent { 4 } else { 3 }].contains(&r) => Ok(Some(r.to_string())),
        Some(r) => {
            eprintln!("{who}: --role is {}, not {r:?}", ROLES[..if agent { 4 } else { 3 }].join(", "));
            Err(2)
        }
    }
}

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

pub(crate) fn user(args: &[String]) -> i32 {
    const WHO: &str = "holon user";
    let usage = format!("{USER_USAGE}\n{SIGN_IN}");
    let args = match cmdline::parse(WHO, &usage, args, &["--role", "--email"], &["--disable", "--enable", "--yes", "--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let role = match role(WHO, &args, false) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let path = |name: &str| format!("/admin/users/{}", seg(name));
    let invited = |(server, v): (String, Value), what: &str| {
        let name = v["name"].as_str().unwrap_or_default();
        println!("{name}: {what}. Send them this link, to choose a password (it works once, within seven days, and is shown only now):");
        println!("{}", invite_link(&server, v["invite"].as_str().unwrap_or_default()));
        0
    };
    match (args.word(0), args.word(1)) {
        (None | Some("list"), None) => match ask(WHO, &args, "GET", "/admin/users", None) {
            Ok((_, v)) if args.flag("--json") => {
                cmdline::print_json(&v);
                0
            }
            Ok((_, v)) => {
                for line in user_lines(&v) {
                    println!("{line}");
                }
                0
            }
            Err(code) => code,
        },
        (Some("invite"), Some(name)) => {
            let Some(role) = role else {
                eprintln!("{WHO}: invite {name} --role viewer|editor|admin");
                return 2;
            };
            let mut body = json!({ "name": name, "role": role });
            if let Some(e) = args.value("--email") {
                body["email"] = json!(e);
            }
            ask(WHO, &args, "POST", "/admin/users", Some(&body)).map_or_else(|c| c, |a| invited(a, &format!("invited as {role}")))
        }
        (Some("reset"), Some(name)) => ask(WHO, &args, "POST", &format!("{}/reset", path(name)), None).map_or_else(|c| c, |a| invited(a, "password cleared, signed out everywhere")),
        (Some("set"), Some(name)) => {
            let mut body = serde_json::Map::new();
            if let Some(r) = role {
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
            if let Some(e) = args.value("--email") {
                body.insert("email".into(), json!(e));
            }
            if body.is_empty() {
                eprintln!("{WHO}: set {name} with --role, --disable, --enable or --email");
                return 2;
            }
            match ask(WHO, &args, "POST", &path(name), Some(&Value::Object(body))) {
                Ok((_, v)) => {
                    println!("{}", user_lines(&json!([v]))[1]);
                    0
                }
                Err(code) => code,
            }
        }
        (Some("remove"), Some(name)) => {
            if !args.flag("--yes") {
                eprintln!("{WHO}: this deletes {name:?}'s account; what they did stays in the activity. Run it again with --yes to go ahead.");
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
        _ => {
            eprintln!("{usage}");
            2
        }
    }
}

pub(crate) fn user_lines(v: &Value) -> Vec<String> {
    let mut rows = vec![["name", "role", "email", "since", "by", ""].map(String::from).to_vec()];
    for u in v.as_array().into_iter().flatten() {
        let note = match (u["disabled"] == true, u["invited"] == true) {
            (true, _) => "disabled",
            (_, true) => "invited, not joined yet",
            _ => "",
        };
        rows.push(vec![
            u["name"].as_str().unwrap_or_default().into(),
            u["role"].as_str().unwrap_or_default().into(),
            u["email"].as_str().unwrap_or("-").into(),
            when(u["created_at"].as_u64()),
            u["created_by"].as_str().unwrap_or("-").into(),
            note.into(),
        ]);
    }
    table(&rows)
}

pub(crate) fn token(args: &[String]) -> i32 {
    const WHO: &str = "holon token";
    let usage = format!("{TOKEN_USAGE}\n{SIGN_IN}");
    let args = match cmdline::parse(WHO, &usage, args, &["--role"], &["--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    let role = match role(WHO, &args, true) {
        Ok(r) => r,
        Err(code) => return code,
    };
    match (args.word(0), args.word(1)) {
        (None | Some("list"), None) => match ask(WHO, &args, "GET", "/admin/tokens", None) {
            Ok((_, v)) if args.flag("--json") => {
                cmdline::print_json(&v);
                0
            }
            Ok((_, v)) => {
                for line in token_lines(&v) {
                    println!("{line}");
                }
                0
            }
            Err(code) => code,
        },
        (Some("add"), Some(label)) => {
            let Some(role) = role else {
                eprintln!("{WHO}: add {label} --role viewer|editor|admin|agent");
                return 2;
            };
            match ask(WHO, &args, "POST", "/admin/tokens", Some(&json!({ "label": label, "role": role }))) {
                Ok((_, v)) => {
                    eprintln!("{label}: an API token with the {role} role, id {}. It is shown only now: keep it where the script reads it (HOLON_TOKEN).", v["entry"]["id"].as_str().unwrap_or_default());
                    println!("{}", v["token"].as_str().unwrap_or_default());
                    0
                }
                Err(code) => code,
            }
        }
        (Some("revoke"), Some(id)) => match ask(WHO, &args, "DELETE", &format!("/admin/tokens/{}", seg(id)), None) {
            Ok(_) => {
                println!("{id}: revoked");
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

pub(crate) fn token_lines(v: &Value) -> Vec<String> {
    let all: Vec<&Value> = v.as_array().into_iter().flatten().collect();
    if all.is_empty() {
        return vec!["no API token (holon token add LABEL --role ROLE makes one)".into()];
    }
    let mut rows = vec![["id", "label", "role", "made", "by"].map(String::from).to_vec()];
    for t in all {
        rows.push(vec![
            t["id"].as_str().unwrap_or_default().into(),
            t["label"].as_str().unwrap_or_default().into(),
            t["role"].as_str().unwrap_or_default().into(),
            when(t["created_at"].as_u64()),
            t["created_by"].as_str().unwrap_or("-").into(),
        ]);
    }
    table(&rows)
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
        let users = json!([
            { "name": "ada", "role": "admin", "email": null, "disabled": false, "created_at": 0, "created_by": null, "invited": false },
            { "name": "bob", "role": "editor", "email": "bob@example.com", "disabled": false, "created_at": 60, "created_by": "ada", "invited": true },
        ]);
        assert_eq!(
            user_lines(&users),
            [
                "name  role    email            since                by",
                "ada   admin   -                1970-01-01T00:00:00  -",
                "bob   editor  bob@example.com  1970-01-01T00:01:00  ada  invited, not joined yet",
            ]
        );
        let tokens = json!([{ "id": "ab12", "label": "ci", "role": "editor", "created_at": 0, "created_by": "ada" }]);
        assert_eq!(token_lines(&tokens), ["id    label  role    made                 by", "ab12  ci     editor  1970-01-01T00:00:00  ada"]);
        assert_eq!(invite_link("http://127.0.0.1:7420/", "hi_a+b"), "http://127.0.0.1:7420/?invite=hi_a%2Bb");
    }
}
