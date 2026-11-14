//! The keys the server seals its data under (docs/L4s/at-rest-encryption-L4.md §2.4, step 7):
//! what they are (`holon key status`, `GET /admin/keys`), a new version of a scope's data key
//! (`holon key rotate <scope>`, `POST /admin/keys/rotate`), the root rotated with the server
//! stopped (`holon key rotate --root`), and a removed deployment's keys forgotten (`holon remove
//! <slug> --forget`, `POST /admin/keys/forget`). Never a key, nor anything of one, is shown.

use rouille::{Request, Response};
use serde_json::{Value, json};

use crate::AppState;
use crate::cmdline;
use crate::http::err;
use crate::sealing::KeySource;

/// `/admin/keys…`, or None for any other path. Who may: `can.rs` (status: host.view; a rotation
/// or a forget: host.operate).
pub(crate) fn route(state: &AppState, request: &Request, by: &str) -> Option<Response> {
    let url = request.url();
    if url != "/admin/keys" && !url.starts_with("/admin/keys/") {
        return None;
    }
    let keys = crate::at_rest::keys();
    Some(match (request.method(), url.as_str()) {
        ("GET", "/admin/keys") => Response::json(&keys.status()),
        ("POST", "/admin/keys/rotate") => {
            let scope = match field(request, "scope") {
                Ok(s) => s,
                Err(r) => return Some(r),
            };
            match keys.rotate(&scope) {
                Ok(version) => {
                    state.activity.record("rotated a key", &format!("{scope}, now version {version}"), Some(by));
                    Response::json(&json!({ "scope": scope, "version": version }))
                }
                Err(e) => err(409, e),
            }
        }
        ("POST", "/admin/keys/forget") => {
            let slug = match field(request, "slug") {
                Ok(s) => s,
                Err(r) => return Some(r),
            };
            if state.deployments.lock().unwrap().contains_key(&slug) {
                return Some(err(409, format!("{slug:?} is deployed: remove it first (holon remove {slug} --yes --forget)")));
            }
            Response::json(&forget(state, &slug, by))
        }
        _ => Response::empty_404(),
    })
}

/// A removed deployment's keys forgotten, then the root rotated in place, so a copy of the whole
/// folder taken before (which holds the old `keys.json`) needs a root that is no longer kept.
fn forget(state: &AppState, slug: &str, by: &str) -> Value {
    let keys = crate::at_rest::keys();
    let scope = crate::at_rest::deployment(slug);
    let had = match keys.forget(&scope) {
        Ok(had) => had,
        Err(e) => return json!({ "slug": slug, "error": e }),
    };
    if !had {
        return json!({ "slug": slug, "forgotten": false, "root": { "rotated": false, "why": "nothing was sealed under its keys" } });
    }
    let root = match keys.source() {
        Some(KeySource::Env(var)) => json!({
            "rotated": false,
            "why": format!("the root key is in {var}, which the server can't change: with the server stopped, holon key rotate --root --to <SOURCE> puts a copy of the folder taken before out of reach"),
        }),
        _ => match keys.rotate_root(None) {
            Ok(None) => json!({ "rotated": true }),
            Ok(Some(pending)) => json!({ "rotated": true, "pending": pending }),
            Err(e) => json!({ "rotated": false, "why": e }),
        },
    };
    state.activity.record("forgot a deployment's keys", slug, Some(by));
    json!({ "slug": slug, "forgotten": had, "root": root })
}

/// A text field of the request's JSON body, or the answer that says it is missing.
fn field(request: &Request, name: &str) -> Result<String, Response> {
    let body = crate::http::read_body(request)?;
    serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|b| b[name].as_str().map(str::to_string))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err(400, format!("the body names the {name}: {{\"{name}\": \"…\"}}")))
}

const USAGE: &str = "holon key: the keys the server seals its data under (never a key itself)

  holon key status                  where the root key is kept, each scope's versions and when
                                    each was made, and the deployments whose keys were forgotten
  holon key rotate <scope>          a new data key for server, secrets or d:<slug>: what is
                                    written from now on is sealed under it, and what was sealed
                                    before still opens (names is never rotated)
  holon key forget <slug>           a removed deployment's keys forgotten (as holon remove --forget):
                                    nothing it left opens again, and the root key is rotated
  holon key rotate --root --to <SOURCE> [--forget-old] [--data DIR] [--root-key SOURCE]
                                    with the server stopped: every data key wrapped under a new
                                    root key, kept in SOURCE (os, env:VAR holding a new key, or
                                    file:PATH); start the server with --root-key SOURCE after.
                                    --forget-old removes the old root from where it was kept: a
                                    copy of the folder taken before can't be opened then.
  --json                            the server's answer as it is

";

/// `holon key …`.
pub(crate) fn cli(args: &[String]) -> i32 {
    const WHO: &str = "holon key";
    let usage = format!("{USAGE}{}", cmdline::SIGN_IN);
    let args = match cmdline::parse(WHO, &usage, args, &["--to", "--data", "--root-key"], &["--root", "--forget-old", "--json"]) {
        Ok(a) => a,
        Err(code) => return code,
    };
    match (args.word(0), args.word(1), args.flag("--root")) {
        (Some("status"), None, false) => match cmdline::json(WHO, &args, "GET", "/admin/keys", None) {
            Ok(v) if args.flag("--json") => {
                cmdline::print_json(&v);
                0
            }
            Ok(v) => {
                print_status(&v);
                0
            }
            Err(code) => code,
        },
        (Some("rotate"), None, true) => rotate_root(&args),
        (Some("forget"), Some(slug), false) => match cmdline::json(WHO, &args, "POST", "/admin/keys/forget", Some(&json!({ "slug": slug }))) {
            Ok(v) if args.flag("--json") => {
                cmdline::print_json(&v);
                0
            }
            Ok(v) => print_forgotten(&v),
            Err(code) => code,
        },
        (Some("rotate"), Some(scope), false) => match cmdline::json(WHO, &args, "POST", "/admin/keys/rotate", Some(&json!({ "scope": scope }))) {
            Ok(v) if args.flag("--json") => {
                cmdline::print_json(&v);
                0
            }
            Ok(v) => {
                println!("{scope}: now version {}; what was sealed under the versions before still opens", v["version"]);
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

/// What a forget did, said (`holon key forget`, `holon remove --forget`): the exit code.
pub(crate) fn print_forgotten(v: &Value) -> i32 {
    let slug = v["slug"].as_str().unwrap_or("?");
    if let Some(e) = v["error"].as_str() {
        eprintln!("{slug}: its keys aren't forgotten: {e}");
        return 1;
    }
    if v["forgotten"].as_bool() != Some(true) {
        println!("{slug}: nothing was ever sealed under its keys");
        return 0;
    }
    println!("{slug}: its keys are forgotten: nothing it left (its kept log, its backups, its records) opens on this server again");
    let root = &v["root"];
    if root["rotated"].as_bool() == Some(true) {
        println!("the root key is rotated where it is kept: a copy of the folder taken before opens only with a copy of the old root, if one was kept");
        if let Some(pending) = root["pending"].as_str() {
            eprintln!("{pending}");
        }
        0
    } else {
        eprintln!("the root key isn't rotated: {}", root["why"].as_str().unwrap_or("?"));
        1
    }
}

fn print_status(v: &Value) {
    println!("root key: {} (id {})", v["root"].as_str().unwrap_or("?"), v["key_id"].as_str().unwrap_or("?"));
    for s in v["scopes"].as_array().into_iter().flatten() {
        let versions: Vec<String> = s["versions"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| format!("v{} made {}", x["version"], cmdline::when(x["made_ms"].as_u64().map(|ms| ms / 1000))))
            .collect();
        println!(
            "  {}: version {}{} ({})",
            s["scope"].as_str().unwrap_or("?"),
            s["current"],
            if s["rotates"].as_bool() == Some(false) { ", never rotated" } else { "" },
            versions.join(", ")
        );
    }
    for f in v["forgotten"].as_array().into_iter().flatten() {
        println!("  {}: forgotten {}, through version {}", f["scope"].as_str().unwrap_or("?"), cmdline::when(f["at_ms"].as_u64().map(|ms| ms / 1000)), f["through"]);
    }
}

/// `holon key rotate --root --to <SOURCE>`: the data folder's keyring opened with its root, the
/// folder locked (no server on it), every data key wrapped under a new root kept in SOURCE.
fn rotate_root(args: &cmdline::Args) -> i32 {
    const WHO: &str = "holon key rotate --root";
    let Some(to) = args.value("--to") else {
        eprintln!("{WHO}: --to says where the new root key is kept: os, env:VAR (holding a new key: holon serve --new-root-key prints one) or file:PATH");
        return 2;
    };
    let data = match args.value("--data") {
        Some(d) => std::path::PathBuf::from(d),
        None => match crate::local::holon_home() {
            Some(home) => crate::migrate::data_folder(&home),
            None => {
                eprintln!("{WHO}: no home folder (USERPROFILE or HOME), and no HOLON_HOME: pass --data");
                return 2;
            }
        },
    };
    let _held = match crate::at_rest::lock_folder(&data) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 1;
        }
    };
    let keys = match crate::verify::keys_for(Some(&data.display().to_string()), args.value("--root-key")) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 1;
        }
    };
    let new = match KeySource::parse(Some(to), &keys.key_id()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 2;
        }
    };
    let old = keys.source();
    match keys.rotate_root(Some(&new)) {
        Ok(None) => println!("the root key is now in {new}; every data key in {} is wrapped under it", data.join("keys.json").display()),
        Ok(Some(why)) => eprintln!("{WHO}: the root key is now in {new}, but {why}"),
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 1;
        }
    }
    // Compared by place, not by spelling: `--to` the file in use written another way is a
    // rotation in place, and forgetting "the old" there would forget the new root.
    match old.filter(|o| !o.same_place(&new)) {
        Some(old) if args.flag("--forget-old") => match old.forget() {
            Ok(true) => println!("the old root key is gone from {old}"),
            Ok(false) => println!("{old} can't be cleared from here: unset it yourself, or a copy of the folder taken before still opens"),
            Err(e) => {
                eprintln!("{WHO}: {e}");
                return 1;
            }
        },
        Some(old) => println!("the old root key is still in {old}: it opens copies of the folder taken before (--forget-old removes it)"),
        None => println!("the old root key was replaced where it was kept: a copy of it kept elsewhere still opens copies of the folder taken before"),
    }
    println!("start the server with --root-key {to}");
    0
}
