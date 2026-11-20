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
    route_with(state, request, by, crate::at_rest::keys())
}

/// [`route`], on `keys`: the server's keyring, or a test's over a keys.json of its own.
pub(crate) fn route_with(state: &AppState, request: &Request, by: &str, keys: &crate::at_rest::Keyring) -> Option<Response> {
    let url = request.url();
    if url != "/admin/keys" && !url.starts_with("/admin/keys/") {
        return None;
    }
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
            let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            match forget(state, keys, &slug, by, now_ms) {
                Ok(said) => Response::json(&said),
                Err(deployed) => err(409, deployed),
            }
        }
        _ => Response::empty_404(),
    })
}

/// A removed deployment's keys forgotten, then the root rotated in place, so a copy of the whole
/// folder taken before (which holds the old `keys.json`) needs a root that is no longer kept.
///
/// What it reaches is what was sealed under the deployment's keys: its log, events, console
/// lines, spools, the records kept when it was removed, its exports and their backups. Not the
/// content store, sealed under the server's keys and shared by every deployment of the same code
/// or the same value (release audit R30): the values nothing else names are swept from it at once
/// (`now_ms`), but those written in the last hour, which the next sweep takes; its code (units)
/// stays, as units are never swept.
///
/// `Err`, nothing forgotten, while the slug is deployed. Whether it is, and its keys forgotten,
/// under the deployments table's lock, one step: a deploy of the slug makes its keys under that
/// lock (deploy.rs), so it comes before, and the forget is refused, or after, and seals under a
/// version past those forgotten. Asked with the lock let go between, a deploy landing there went
/// live under the version about to be forgotten, and what it sealed until the forget (its first
/// runs' lines, its events) no longer opened, its log's reads leaving them out without a word.
/// The root's rotation and the store's sweep come after, with the lock let go.
pub(crate) fn forget(state: &AppState, keys: &crate::at_rest::Keyring, slug: &str, by: &str, now_ms: u64) -> Result<Value, String> {
    let scope = crate::at_rest::deployment(slug);
    let forgot = {
        let table = state.deployments.lock().unwrap();
        if table.contains_key(slug) {
            return Err(format!("{slug:?} is deployed: remove it first (holon remove {slug} --yes --forget)"));
        }
        #[cfg(test)]
        crate::deploy::hold_here("forget", slug);
        keys.forget(&scope)
    };
    let had = match forgot {
        Ok(had) => had,
        Err(e) => return Ok(json!({ "slug": slug, "error": e })),
    };
    if !had {
        return Ok(json!({ "slug": slug, "forgotten": false, "root": { "rotated": false, "why": "nothing was sealed under its keys" } }));
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
    let store = match crate::sweep::sweep(&state.store, &state.deployments, now_ms, crate::sweep::SPARES_MS, false, true) {
        Ok(Some(swept)) => json!({ "swept": swept.values, "spared": swept.spared }),
        Ok(None) => json!({ "swept": 0, "spared": 0 }),
        Err(e) => json!({ "why": e }),
    };
    Ok(json!({ "slug": slug, "forgotten": had, "root": root, "store": store }))
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
  holon key forget <slug> --yes     a removed deployment's keys forgotten (as holon remove --forget):
                                    its log, events, records and backups open nowhere again, for
                                    good; the content store's values nothing else names are swept
                                    (its code stays); and the root key is rotated where it is
                                    kept, so every backup and copy of the data folder taken before
                                    needs the old root: back up the new one
  holon key export --to file:PATH [--data DIR] [--root-key SOURCE | --server PORT|URL]
                                    a copy of the root key, written owner-only to PATH, which must
                                    hold none: the way to back up a root kept in the OS key store.
                                    With --server, the root is read where that server reads it.
                                    Keep it apart from the data folder; a forget rotates the root,
                                    so export it again after one
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
    let args = match cmdline::parse(WHO, &usage, args, &["--to", "--data", "--root-key"], &["--root", "--forget-old", "--json", "--yes"]) {
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
        (Some("export"), None, false) => export_root(&args),
        (Some("forget"), Some(slug), false) if !args.flag("--yes") => {
            eprintln!(
                "{WHO}: forgetting {slug}'s keys can't be undone: its kept log, events, records and backups open nowhere again, \
                 and the root key is rotated where it is kept, so every backup and copy of the data folder taken before needs the old root. \
                 Run it again with --yes to forget them"
            );
            2
        }
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
    println!("{slug}: its keys are forgotten: its kept log, events, records and backups open nowhere again");
    let store = &v["store"];
    match store["why"].as_str() {
        Some(why) => eprintln!("the content store wasn't swept: {why}; its state values stay until the next sweep"),
        None => println!(
            "the content store, sealed under the server's keys, is swept: {} values nothing else names removed, {} written in the last hour left to the next sweep; its code stays in the store",
            store["swept"].as_u64().unwrap_or(0),
            store["spared"].as_u64().unwrap_or(0)
        ),
    }
    let root = &v["root"];
    if root["rotated"].as_bool() == Some(true) {
        println!("the root key is rotated where it is kept: backups and copies of the data folder taken before open only with the old root, if a copy of it was kept");
        println!("back up the new root key now, apart from the data folder: holon key export --to file:PATH");
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

/// The data folder `--data` names, else the one under `HOLON_HOME`; `Err` is the exit code, said.
fn data_folder(args: &cmdline::Args, who: &str) -> Result<std::path::PathBuf, i32> {
    match args.value("--data") {
        Some(d) => Ok(std::path::PathBuf::from(d)),
        None => match crate::local::holon_home() {
            Some(home) => Ok(crate::migrate::data_folder(&home)),
            None => {
                eprintln!("{who}: no home folder (USERPROFILE or HOME), and no HOLON_HOME: pass --data");
                Err(2)
            }
        },
    }
}

/// `holon key export --to file:PATH`: the data folder's keyring opened to read only, with its
/// root, and that root (checked against `keys.json`) copied to PATH, owner-only. Nothing of the
/// key is printed. The server may be running: nothing in the folder is written.
fn export_root(args: &cmdline::Args) -> i32 {
    const WHO: &str = "holon key export";
    let Some(to) = args.value("--to") else {
        eprintln!("{WHO}: --to file:PATH says where the copy goes: a file that isn't there yet, apart from the data folder");
        return 2;
    };
    let to = KeySource::File(std::path::PathBuf::from(to.strip_prefix("file:").unwrap_or(to)));
    let data = match data_folder(args, WHO) {
        Ok(d) => d,
        Err(code) => return code,
    };
    if let KeySource::File(path) = &to
        && crate::sealing::inside(path, &data)
    {
        eprintln!("{WHO}: {} is in the data folder: a copy of the root kept with what it opens protects nothing; put it apart", path.display());
        return 2;
    }
    // With --server and no --root-key, the root is read where that server reads it: the server
    // knows (holon key status), and looking in the default place instead found nothing for a
    // server started with --root-key file:… (release audit R5). Its keys must be this folder's.
    let served = match (args.value("--root-key"), args.server()) {
        (None, Some(_)) => match served_root(args, &data, WHO) {
            Ok(s) => Some(s),
            Err(code) => return code,
        },
        _ => None,
    };
    let root_key = args.value("--root-key").or(served.as_deref());
    let keys = match crate::verify::keys_for(Some(&data.display().to_string()), root_key) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 1;
        }
    };
    if let Err(e) = keys.export_root(&to) {
        eprintln!("{WHO}: {e}");
        return 1;
    }
    let from = keys.source().map_or_else(|| "?".to_string(), |s| s.to_string());
    println!("the root key in {from}, which opens {}, is copied to {to}, readable only by you", data.join("keys.json").display());
    println!("keep it apart from the data folder (another disk, a password manager): the data folder and this key together open everything the server keeps");
    println!("a forget (holon remove --forget, holon key forget) rotates the root: export it again after one");
    0
}

/// Where the server `--server` names reads its root (`--root-key` as it reads it), when its
/// keys are `data`'s: `Err` is the exit code, said. A server on another data folder, or one with
/// its keys in memory, is no answer.
fn served_root(args: &cmdline::Args, data: &std::path::Path, who: &str) -> Result<String, i32> {
    let v = cmdline::json(who, args, "GET", "/admin/keys", None)?;
    let theirs = v["key_id"].as_str().unwrap_or_default();
    let keys = data.join("keys.json");
    let ours = std::fs::read(&keys).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|f| f["key_id"].as_str().map(str::to_string));
    match (ours, v["source"].as_str()) {
        (Some(ours), Some(source)) if ours == theirs => Ok(source.to_string()),
        (Some(_), Some(_)) => {
            eprintln!("{who}: that server's keys aren't {}'s: pass --data with the data folder it runs on", keys.display());
            Err(1)
        }
        (None, _) => {
            eprintln!("{who}: {} isn't there: pass --data with the data folder that server runs on", keys.display());
            Err(1)
        }
        (Some(_), None) => {
            eprintln!("{who}: that server keeps its keys in memory only: there is no root to export");
            Err(1)
        }
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
    let data = match data_folder(args, WHO) {
        Ok(d) => d,
        Err(code) => return code,
    };
    let _held = match crate::at_rest::lock_folder(&data) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 1;
        }
    };
    let keys = match crate::verify::keys_to_change(Some(&data.display().to_string()), args.value("--root-key")) {
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
    if let KeySource::File(path) = &new
        && crate::sealing::inside(path, &data)
    {
        eprintln!("{WHO}: {} is in the data folder: a root kept with what it opens protects nothing; keep it apart", path.display());
        return 2;
    }
    let old = keys.source();
    // A rotation in place keeps the place as the folder was opened with it: `--to` the key file
    // in use spelled another way wrote the new key under that spelling (on a disk that ignores
    // case, the file took its case) and printed it as typed (release audit R5).
    let new = match &old {
        Some(o) if o.same_place(&new) => o.clone(),
        _ => new,
    };
    // Read before the rotation: after it this folder is under the new root, and the old one's
    // list says only what else it opens.
    let others = match keys.others_on_root() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{WHO}: {e}");
            return 1;
        }
    };
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
        Some(old) if args.flag("--forget-old") && !others.is_empty() => {
            let names: Vec<String> = others.iter().map(|p| p.display().to_string()).collect();
            println!("the old root key is kept in {old}: it still opens {}; give each a root of its own, then remove it", names.join(", "));
        }
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
    println!("start the server with --root-key {}", new.arg());
    0
}
