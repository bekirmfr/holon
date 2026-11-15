//! `holon migrate`: what an older release left that this one keeps elsewhere, moved
//! (docs/L4s/holon-home-L4.md §4).
//!
//! A migration looks first and changes nothing ([`Migration::needed`]), then applies a plan it
//! made ([`Migration::apply`]). `holon migrate` says each plan, asks once, and applies them in
//! order; once applied, a migration finds nothing, so running it twice is harmless. Nothing is
//! removed before it has been copied and checked, and nothing runs while a server may be using
//! the files: nothing locks them.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::local;
use crate::sealing;

const USAGE: &str = "usage: holon migrate [--yes] [--check] [--from DIR]

  Moves what an older release left where this one doesn't look:
  - key files kept in ~/.config/holon/keys, into ~/.holon/keys

  A server's data kept as .holon-* files in a folder (this one, or --from DIR) is named, never
  moved: it was written in clear, before the server sealed its data, and this release reads none
  of it, moved or not. Keep running the release that wrote it on it, or start over.

  --yes                    go ahead without asking
  --check                  change nothing; exit 1 if anything needs migrating, or is found
                           that can't be
  --from DIR               where the .holon-* files are looked for (default: this folder)

examples:
  holon migrate --check
  holon migrate --from ~/old-server --yes";

/// A server's files as releases before 0.1.0-rc.17 named them in the folder it ran in, and their
/// names in the data folder. Each is a file or a folder.
const OLD_DATA: &[(&str, &str)] = &[
    (".holon-store", "store"),
    (".holon-deployments.json", "deployments.json"),
    (".holon-deployments.logs", "deployments.logs"),
    (".holon-deployments.activity.jsonl", "deployments.activity.jsonl"),
    (".holon-deployments.heads", "deployments.heads"),
    (".holon-flows.json", "flows.json"),
    (".holon-secrets.json", "secrets.json"),
    (".holon-secrets.audit.jsonl", "secrets.audit.jsonl"),
    (".holon-accounts.json", "accounts.json"),
    (".holon-host-key", "host.key"),
];

/// What a migration looks at.
pub(crate) struct Env {
    /// Where the old `.holon-*` files are looked for.
    pub(crate) from: PathBuf,
    /// `~/.holon/keys`.
    pub(crate) keys: PathBuf,
    /// Where key files were kept before.
    pub(crate) old_keys: Option<PathBuf>,
}

impl Env {
    fn here(from: Option<PathBuf>) -> Result<Env, String> {
        let home = local::holon_home().ok_or("no home folder (USERPROFILE or HOME), and no HOLON_HOME")?;
        let from = match from {
            Some(f) => f,
            None => std::env::current_dir().map_err(|e| format!("which folder is this? {e}"))?,
        };
        Ok(Env { from, keys: home.join("keys"), old_keys: sealing::old_keys_dir() })
    }
}

/// The data folder `holon serve` uses when given none: HOLON_DATA, else `<home>/data`.
pub(crate) fn data_folder(home: &Path) -> PathBuf {
    std::env::var_os("HOLON_DATA").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join("data"))
}

/// What a migration would do: said before it's done.
pub(crate) struct Plan {
    pub(crate) lines: Vec<String>,
    /// Why it can't be done as things are, when it can't.
    pub(crate) blocked: Option<String>,
    moves: Vec<(PathBuf, PathBuf)>,
}

pub(crate) struct Migration {
    pub(crate) name: &'static str,
    pub(crate) needed: fn(&Env) -> Option<Plan>,
    pub(crate) apply: fn(&Env, &Plan) -> Result<String, String>,
}

/// Every migration, in the order they apply.
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration { name: "keys-in-config", needed: keys_needed, apply: keys_apply },
    Migration { name: "data-in-a-folder", needed: data_needed, apply: data_apply },
];

pub(crate) fn run(args: &[String]) -> i32 {
    match main(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("holon migrate: {e}");
            1
        }
    }
}

fn main(args: &[String]) -> Result<i32, String> {
    let (mut yes, mut check, mut from) = (false, false, None);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--yes" | "-y" => yes = true,
            "--check" => check = true,
            "--from" => from = Some(PathBuf::from(args.next().ok_or("--from needs a folder")?)),
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(0);
            }
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
        }
    }
    let env = Env::here(from)?;
    let plans: Vec<(&Migration, Plan)> = MIGRATIONS.iter().filter_map(|m| (m.needed)(&env).map(|p| (m, p))).collect();
    if plans.is_empty() {
        println!("nothing to migrate");
        return Ok(0);
    }
    for (m, plan) in &plans {
        println!("{}:", m.name);
        for line in &plan.lines {
            println!("  {line}");
        }
        if let Some(why) = &plan.blocked {
            println!("  can't: {why}");
        }
    }
    if check {
        return Ok(1);
    }
    if let Some((m, plan)) = plans.iter().find(|(_, p)| p.blocked.is_some()) {
        return Err(format!("{}: {}", m.name, plan.blocked.as_deref().unwrap_or_default()));
    }
    let live = local::live_servers();
    if !live.is_empty() {
        return Err(format!("a server may be using these files ({}): stop it first", live.join(", ")));
    }
    if !yes && !confirm()? {
        println!("nothing changed");
        return Ok(0);
    }
    for (m, plan) in &plans {
        let done = (m.apply)(&env, plan).map_err(|e| format!("{}: {e}", m.name))?;
        println!("{}: {done}", m.name);
    }
    Ok(0)
}

fn confirm() -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        return Err("not a terminal to ask in: pass --yes to go ahead".into());
    }
    print!("go ahead? (y/n) ");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

/// The line `holon serve` prints when the folder it was started in holds an older server's
/// files: it doesn't read them, and doesn't move them on its own.
pub(crate) fn left_here() -> Option<String> {
    let here = std::env::current_dir().ok()?;
    let found: Vec<&str> = OLD_DATA.iter().map(|(old, _)| *old).filter(|old| here.join(old).exists()).collect();
    // Not "holon migrate moves it": moved, it is a folder this release refuses (R10).
    (!found.is_empty()).then(|| format!("{} holds an older server's data ({}), written in clear by a release this one doesn't read", here.display(), found.join(", ")))
}

// ── data-in-a-folder ─────────────────────────────────────────────────────────────────────────

/// A server's data as `.holon-*` files: named, and refused. Every release that kept it there wrote
/// it in clear, before the server sealed its data (0.1.0-rc.35 and earlier); moved into the data
/// folder it is a folder this release refuses to start on, so it is never moved (R10).
fn data_needed(env: &Env) -> Option<Plan> {
    let found: Vec<&str> = OLD_DATA.iter().map(|(old, _)| *old).filter(|old| env.from.join(old).exists()).collect();
    if found.is_empty() {
        return None;
    }
    let lines = vec![format!("a server's data in {}: {}", env.from.display(), found.join(", "))];
    let blocked = Some(
        "written in clear by a release from before the server sealed its data (0.1.0-rc.35 or earlier): this one reads none of it, \
         moved or not, so it isn't moved. Keep running that release on it, or start over"
            .to_string(),
    );
    Some(Plan { lines, blocked, moves: Vec::new() })
}

/// Never applied: its plan is always blocked ([`data_needed`]).
fn data_apply(_env: &Env, _plan: &Plan) -> Result<String, String> {
    Err("a server's data written in clear isn't moved".into())
}

fn name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

// ── keys-in-config ───────────────────────────────────────────────────────────────────────────

fn keys_needed(env: &Env) -> Option<Plan> {
    let old = env.old_keys.as_ref()?;
    let files: Vec<PathBuf> = std::fs::read_dir(old).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "key")).collect();
    if files.is_empty() {
        return None;
    }
    let moves: Vec<(PathBuf, PathBuf)> = files.into_iter().map(|f| (f.clone(), env.keys.join(name(&f)))).collect();
    let clash = moves.iter().find(|(old, new)| new.exists() && std::fs::read(old).ok() != std::fs::read(new).ok());
    let blocked = clash.map(|(_, new)| format!("{} holds a different key already", new.display()));
    let lines = vec![format!("move {} key file(s) from {} into {}", moves.len(), old.display(), env.keys.display())];
    Some(Plan { lines, blocked, moves })
}

fn keys_apply(env: &Env, plan: &Plan) -> Result<String, String> {
    std::fs::create_dir_all(&env.keys).map_err(|e| format!("could not make {}: {e}", env.keys.display()))?;
    for (old, new) in &plan.moves {
        if !new.exists() {
            let bytes = std::fs::read(old).map_err(|e| format!("could not read {}: {e}", old.display()))?;
            sealing::write_private(new, &bytes).map_err(|e| format!("could not write {}: {e}", new.display()))?;
        }
        std::fs::remove_file(old).map_err(|e| format!("copied, but could not remove {}: {e}", old.display()))?;
    }
    // The folders it leaves, if nothing else is in them.
    if let Some(old) = &env.old_keys {
        let _ = std::fs::remove_dir(old).and_then(|()| old.parent().map_or(Ok(()), std::fs::remove_dir));
    }
    Ok(format!("{} key file(s) moved into {}", plan.moves.len(), env.keys.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holon-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn env(dir: &Path) -> Env {
        Env { from: dir.join("old"), keys: dir.join("home").join("keys"), old_keys: Some(dir.join("config").join("holon").join("keys")) }
    }

    /// An older server's folder: a store, its tables, a log, its secrets and a policy, in clear.
    fn old_server(env: &Env) {
        let old = &env.from;
        std::fs::create_dir_all(old.join(".holon-store").join("ab")).unwrap();
        std::fs::write(old.join(".holon-store").join("ab").join("unit"), b"bytes").unwrap();
        std::fs::write(old.join(".holon-deployments.json"), b"[]").unwrap();
        std::fs::write(old.join(".holon-accounts.json"), b"{}").unwrap();
        std::fs::write(old.join(".holon-host-key"), b"k").unwrap();
        std::fs::write(old.join("holon.toml"), b"").unwrap();
    }

    /// Guards: an older server's `.holon-*` files are named and refused, never moved: the plan is
    /// blocked, and the files stay where they are.
    /// Why: every release that kept them there wrote them in clear; moved into the data folder,
    /// they made a folder this release refuses to start on, with the old files gone (R10).
    #[test]
    fn an_older_servers_data_is_named_and_never_moved() {
        let dir = temp("data");
        let env = env(&dir);
        assert!(data_needed(&env).is_none(), "nothing there: nothing to do");
        old_server(&env);
        let plan = data_needed(&env).expect("the old files are found");
        assert!(plan.lines[0].contains(".holon-deployments.json") && plan.moves.is_empty(), "{:?}", plan.lines);
        assert!(plan.blocked.as_deref().is_some_and(|b| b.contains("0.1.0-rc.35")), "{:?}", plan.blocked);
        assert!(data_apply(&env, &plan).is_err());
        assert!(env.from.join(".holon-deployments.json").exists(), "nothing moved");
        // The key such a release kept its secrets under is found, for `uninstall --purge`: by the
        // id its secrets file records, in clear. A secrets file this release sealed has none.
        let secrets = dir.join("secrets.json");
        std::fs::write(&secrets, br#"{"format": 3, "key_id": "0123456789abcdef", "deployments": {}}"#).unwrap();
        let old = crate::sealing::old_secrets_key(&secrets).map(|k| k.to_string()).unwrap_or_default();
        assert!(old.contains("secrets-key-0123456789abcdef") || old.contains("0123456789abcdef.key"), "{old}");
        let sealed = crate::at_rest::keys().seal_file(crate::at_rest::SERVER, crate::at_rest::role::SECRETS, b"{}").unwrap();
        std::fs::write(&secrets, sealed).unwrap();
        assert!(crate::sealing::old_secrets_key(&secrets).is_none(), "a sealed secrets file is this release's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_files_move_from_the_config_folder() {
        let dir = temp("keys");
        let env = env(&dir);
        assert!(keys_needed(&env).is_none());
        let old = env.old_keys.clone().unwrap();
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("0123456789abcdef.key"), b"k1").unwrap();
        let plan = keys_needed(&env).unwrap();
        assert!(plan.blocked.is_none());
        keys_apply(&env, &plan).unwrap();
        assert_eq!(std::fs::read(env.keys.join("0123456789abcdef.key")).unwrap(), b"k1");
        assert!(!old.exists(), "the emptied folder goes");
        assert!(keys_needed(&env).is_none());
        // A different key already where one would go: blocked.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("0123456789abcdef.key"), b"k2").unwrap();
        assert!(keys_needed(&env).unwrap().blocked.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
