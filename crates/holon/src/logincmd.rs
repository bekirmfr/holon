//! `holon login` and `holon logout`: a person signs in to a server elsewhere as themselves, with
//! their name and password, and the session is kept for that server in `~/.holon/tokens.json`
//! (local.rs), readable only by them. Every other command reaching that server signs in with it,
//! unless `HOLON_TOKEN` is set; with no server given and none running here, they reach the one
//! signed in to. An agent or a service signs in with its own token instead.
//!
//! The password is read without echo (or piped in), sent once to `POST /auth/sign-in`, over https
//! or to this machine only (`local::token_safe_to`), and never printed, kept or logged; nor is the
//! session's token. Signing in through a provider (OIDC) isn't here yet: such a person uses the
//! editor.

use std::path::Path;

use serde_json::{json, Value};

use crate::cmdline::{self, when};
use crate::deploycmd::Person;
use crate::local::{self, Target, TokenFrom};
use crate::mcp::{Api, Body};

pub(crate) const LOGIN_USAGE: &str = "usage: holon login [--server URL] [--name NAME]

  Signs you in to a server, as a person, with your name and password: asked here (the password
  isn't shown as you type it), or piped in, a line each. The session is kept for that server in
  ~/.holon/tokens.json, readable only by you, and every other command reaching the server signs
  in with it until it ends: a week after you sign in, when you sign out (holon logout), or when
  an admin resets your password. With no holon serve running here, commands reach the one server
  you signed in to without --server.

  An agent or a service signs in with its own token instead (HOLON_TOKEN), which wins over a
  session. Signing in through a provider (holon oidc) isn't here yet: the editor does it.

  --server URL             the server: its https:// address, or a port on this machine
                           [HOLON_SERVER]
  --name NAME              your name there (else asked)

  A password crosses a network only over https. For a server elsewhere on plain http, reach it
  through an SSH tunnel (ssh -L 7420:localhost:7420 HOST, then --server 7420).

examples:
  holon login --server https://holon.example.com
  holon decide payouts
  holon logout";

pub(crate) const LOGOUT_USAGE: &str = "usage: holon logout [--server URL]

  Ends the session holon login kept for a server, on the server too, and forgets it here. With no
  --server, the one server you signed in to.

  --server URL             the server signed out of [HOLON_SERVER]

examples:
  holon logout
  holon logout --server https://holon.example.com";

/// A session begun: whose, their role on the host, and when it ends.
pub(crate) struct SignedIn {
    pub(crate) token: String,
    pub(crate) name: String,
    pub(crate) role: Option<String>,
    /// Seconds since 1970.
    pub(crate) until: Option<u64>,
}

/// Sign `name` in with `password` at the server `api` reaches: the session, or why not, in the
/// server's words (never the password).
pub(crate) fn sign_in(api: &dyn Api, name: &str, password: &str) -> Result<SignedIn, String> {
    let server = api.server();
    let (status, bytes) = api.send("POST", "/auth/sign-in", Some(Body::Json(json!({ "name": name, "password": password }))))?;
    let answer: Value = crate::wire::read_json(&bytes).unwrap_or(Value::Null);
    if status != 200 {
        return Err(format!("{server} didn't sign you in ({status}): {}", why(&answer, &bytes)));
    }
    let token = answer["token"].as_str().filter(|t| !t.is_empty()).ok_or_else(|| format!("{server} began no session"))?;
    Ok(SignedIn {
        token: token.to_string(),
        name: answer["name"].as_str().unwrap_or(name).to_string(),
        role: answer["role"].as_str().map(str::to_string),
        until: answer["expires_at"].as_u64(),
    })
}

/// Sign `name` in to `server` through `api` and keep the session in `file`.
fn begin(api: &dyn Api, file: &Path, server: &str, name: &str, password: &str) -> Result<SignedIn, String> {
    let signed = sign_in(api, name, password)?;
    local::keep_token(file, server, Some(&signed.token))?;
    Ok(signed)
}

/// End the session `api` signs in with, on its server: whether it ended now (false: it had ended
/// already, expired, signed out elsewhere or ended by a password reset).
pub(crate) fn sign_out(api: &dyn Api) -> Result<bool, String> {
    let (status, bytes) = api.send("POST", "/auth/sign-out", Some(Body::Json(json!({}))))?;
    match status {
        200..=299 => Ok(true),
        401 => Ok(false),
        _ => Err(format!("{} answered {status}: {}", api.server(), why(&crate::wire::read_json(&bytes).unwrap_or(Value::Null), &bytes))),
    }
}

/// The `{"error": …}` sentence of an answer, or its body.
fn why(answer: &Value, bytes: &[u8]) -> String {
    answer["error"].as_str().map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(bytes).trim().to_string())
}

pub(crate) fn login(args: &[String]) -> i32 {
    const WHO: &str = "holon login";
    let args = match cmdline::parse(WHO, LOGIN_USAGE, args, &["--server", "--name"], &[]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    if let Some(extra) = args.word(0) {
        eprintln!("{WHO}: {extra:?} isn't one of its options\n\n{LOGIN_USAGE}");
        return 2;
    }
    let Some(server) = local::given_server(args.server()) else {
        eprintln!("{WHO}: give --server <URL>, the server to sign in to (or set HOLON_SERVER)\n\n{LOGIN_USAGE}");
        return 2;
    };
    let server = local::server_url(&server);
    // Before anything is asked: a password isn't typed for nothing.
    if let Err(e) = local::token_safe_to(&server) {
        eprintln!("{WHO}: {e}");
        return 1;
    }
    let Some(file) = local::tokens_file() else {
        eprintln!("{WHO}: no home folder to keep the session in");
        return 1;
    };
    let name = match args.value("--name") {
        Some(n) => Ok(n.trim().to_string()),
        None => cmdline::read_line(&format!("Your name on {server}: ")),
    };
    let name = match name {
        Ok(n) if !n.is_empty() => n,
        Ok(_) => {
            eprintln!("{WHO}: no name given");
            return 2;
        }
        Err(e) => {
            eprintln!("{WHO}: reading your name: {e}");
            return 1;
        }
    };
    let password = match cmdline::read_hidden("Password: ") {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => {
            eprintln!("{WHO}: no password given");
            return 2;
        }
        Err(e) => {
            eprintln!("{WHO}: reading the password: {e}");
            return 1;
        }
    };
    let api = Person(Target { server: server.clone(), token: None, from: TokenFrom::Nothing });
    let signed = match begin(&api, &file, &server, &name, &password) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 1;
        }
    };
    let role = signed.role.map_or_else(String::new, |r| format!(" ({r} on the host)"));
    println!("Signed in to {server} as {}{role}, until {}.", signed.name, when(signed.until));
    println!("The session is kept in {}, readable only by you; holon logout ends it.", file.display());
    if local::env_token().is_some() {
        eprintln!("{WHO}: HOLON_TOKEN is set, and commands sign in with it rather than this session until you unset it");
    }
    0
}

pub(crate) fn logout(args: &[String]) -> i32 {
    const WHO: &str = "holon logout";
    let args = match cmdline::parse(WHO, LOGOUT_USAGE, args, &["--server"], &[]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    if let Some(extra) = args.word(0) {
        eprintln!("{WHO}: {extra:?} isn't one of its options\n\n{LOGOUT_USAGE}");
        return 2;
    }
    let Some(file) = local::tokens_file() else {
        println!("No session is kept: there is no home folder to keep one in.");
        return 0;
    };
    let kept = local::tokens_in(&file);
    let server = match local::given_server(args.server()) {
        Some(s) => local::server_url(&s),
        None => match &local::logins(&kept)[..] {
            [one] => one.clone(),
            [] => {
                println!("You aren't signed in anywhere with holon login.");
                return 0;
            }
            several => {
                eprintln!("{WHO}: you are signed in to {} servers: pick one with --server {}", several.len(), several.join(" or --server "));
                return 2;
            }
        },
    };
    let Some(token) = kept.get(&server).cloned() else {
        println!("You aren't signed in to {server} with holon login.");
        return 0;
    };
    let ended = local::token_safe_to(&server).and_then(|()| sign_out(&Person(Target { server: server.clone(), token: Some(token), from: TokenFrom::Session })));
    if let Err(e) = local::keep_token(&file, &server, None) {
        eprintln!("{WHO}: {e}");
        return 1;
    }
    match ended {
        Ok(true) => println!("Signed out of {server}."),
        Ok(false) => println!("Your session on {server} had ended already; it is forgotten here too."),
        Err(e) => {
            eprintln!("{WHO}: {e}");
            println!("Forgotten here, but {server} didn't end the session: it ends on its own a week after you signed in.");
            return 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{body_bytes, req, req_as, test_state};
    use crate::{route, AppState};

    /// The router in-process, signed in with the token, if any.
    struct At<'a>(&'a AppState, Option<&'a str>);

    impl Api for At<'_> {
        fn server(&self) -> String {
            "the test server".into()
        }
        fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Vec<u8>), String> {
            let bytes = match body {
                Some(Body::Json(v)) => serde_json::to_vec(&v).unwrap(),
                Some(Body::Bytes(b)) => b,
                None => vec![],
            };
            let request = match self.1 {
                Some(token) => req_as(token, method, path, bytes),
                None => req(method, path, bytes),
            };
            let response = route(self.0, &request);
            Ok((response.status_code, body_bytes(response)))
        }
    }

    /// Each way a sign-in ends: a person's name and password begin a session, kept for the server,
    /// that signs every command in as them for a week; anything else is refused, in the server's
    /// words, never repeating the password, and keeps nothing. Signing out ends the session there;
    /// again, it had ended.
    #[test]
    fn a_person_signs_in_for_a_week_and_out_and_nothing_else_signs_in() {
        use crate::accounts::{Accounts, Kind};
        let pw = "correct horse battery";
        let mut state = test_state();
        let accounts = std::sync::Arc::new(Accounts::memory());
        accounts.setup(&accounts.setup_code().unwrap(), "ada", pw).unwrap();
        let invite = accounts.invite("cy", "viewer", None, "ada").unwrap();
        accounts.accept(&invite, "cy's own long password").unwrap();
        accounts.invite("dee", "viewer", None, "ada").unwrap();
        accounts.add("ci", Kind::Service, "viewer", "ada", "ada").unwrap();
        let invite = accounts.invite("eve", "viewer", None, "ada").unwrap();
        accounts.accept(&invite, pw).unwrap();
        accounts.set_disabled("eve", true).unwrap();
        state.access = crate::access::Access::new(std::sync::Arc::clone(&accounts), vec![], vec![]);
        let dir = std::env::temp_dir().join(format!("holon-login-{}", std::process::id()));
        let file = dir.join("tokens.json");
        let server = "https://holon.example.com";
        let week = 7 * 24 * 3600;
        let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        /// The name, the password, and who it signs in as (with their role) or what the refusal says.
        type Row<'a> = (&'a str, &'a str, Result<(&'a str, &'a str), &'a str>);
        #[rustfmt::skip]
        let rows: [Row; 7] = [
            ("cy", "cy's own long password", Ok(("cy", "viewer"))),
            ("ada", pw, Ok(("ada", "admin"))),
            ("cy", pw, Err("don't match an account here")),
            ("nobody", pw, Err("don't match an account here")),
            // Invited, not joined yet: no password to match.
            ("dee", pw, Err("don't match an account here")),
            // A service signs in with its tokens, never a password; a disabled person not at all.
            ("ci", pw, Err("don't match an account here")),
            ("eve", pw, Err("don't match an account here")),
        ];
        for (name, password, want) in rows {
            let _ = std::fs::remove_dir_all(&dir);
            let began = now();
            match (begin(&At(&state, None), &file, server, name, password), want) {
                (Ok(signed), Ok((who, role))) => {
                    assert_eq!((signed.name.as_str(), signed.role.as_deref()), (who, Some(role)), "{name}");
                    let until = signed.until.expect("when it ends");
                    assert!((began + week..=now() + week).contains(&until), "{name}: a week from now, not {until}");
                    // Kept for the server, and it signs a command in as them.
                    let kept = local::saved_token(&file, server).expect("kept");
                    assert_eq!(kept, signed.token);
                    let (status, me) = At(&state, Some(&kept)).send("GET", "/auth/me", None).unwrap();
                    assert_eq!((status, serde_json::from_slice::<Value>(&me).unwrap()["name"].as_str()), (200, Some(who)), "{name}");
                }
                (Err(said), Err(want)) => {
                    assert!(said.contains(want) && said.starts_with("the test server didn't sign you in (401)"), "{name}: {said}");
                    assert!(!said.contains(password), "{name}: the password repeated");
                    assert_eq!(local::saved_token(&file, server), None, "{name}: nothing kept");
                }
                (Ok(signed), Err(want)) => panic!("{name}: signed in as {}, wanted {want:?}", signed.name),
                (Err(said), Ok(_)) => panic!("{name}: {said}"),
            }
        }

        // Out: the session ends on the server, and a second sign-out finds it ended.
        let cy = begin(&At(&state, None), &file, server, "cy", "cy's own long password").unwrap().token;
        assert_eq!(sign_out(&At(&state, Some(&cy))), Ok(true));
        assert_eq!(At(&state, Some(&cy)).send("GET", "/auth/me", None).unwrap().0, 401);
        assert_eq!(sign_out(&At(&state, Some(&cy))), Ok(false));
        // A reset ends it too: what a kept session's refusal tells the person to do then.
        let cy = begin(&At(&state, None), &file, server, "cy", "cy's own long password").unwrap().token;
        accounts.reset("cy").unwrap();
        assert_eq!(At(&state, Some(&cy)).send("GET", "/auth/me", None).unwrap().0, 401);
        assert!(local::sign_in_hint(server, TokenFrom::Session).contains("holon login --server https://holon.example.com"));
        // A server without accounts has none to sign in to.
        let bare = test_state();
        let said = sign_in(&At(&bare, None), "cy", pw).err().unwrap();
        assert!(said.contains("(404)") && said.contains("no accounts"), "{said}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
