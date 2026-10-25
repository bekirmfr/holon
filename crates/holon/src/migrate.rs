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
use crate::sealing::{self, KeySource};

const USAGE: &str = "usage: holon migrate [--yes] [--check] [--from DIR] [--secrets-key os|env:VAR|file:PATH]

  moves what an older release left where this one doesn't look:
  - a server's data kept as .holon-* files in a folder (this one, or --from DIR), into
    ~/.holon/data (or HOLON_HOME/data, or HOLON_DATA)
  - secrets keys kept in ~/.config/holon/keys, into ~/.holon/keys

  --yes                      go ahead without asking
  --check                    change nothing; exit 1 if anything needs migrating
  --from DIR                 where the .holon-* files are (default: this folder)
  --secrets-key …            where the old secrets file's key is, if not the default";

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

/// The policy, read from the same folder: moved with the others, never alone (a `holon.toml` in a
/// project may be something else's).
const OLD_POLICY: (&str, &str) = ("holon.toml", "holon.toml");

/// What a migration looks at.
pub(crate) struct Env {
    /// Where the old `.holon-*` files are looked for.
    pub(crate) from: PathBuf,
    /// The data folder they go to.
    pub(crate) data: PathBuf,
    /// `~/.holon/keys`.
    pub(crate) keys: PathBuf,
    /// Where key files were kept before.
    pub(crate) old_keys: Option<PathBuf>,
    /// `--secrets-key`, for opening the old secrets file.
    pub(crate) secrets_key: Option<String>,
}

impl Env {
    fn here(from: Option<PathBuf>, secrets_key: Option<String>) -> Result<Env, String> {
        let home = local::holon_home().ok_or("no home folder (USERPROFILE or HOME), and no HOLON_HOME")?;
        let from = match from {
            Some(f) => f,
            None => std::env::current_dir().map_err(|e| format!("which folder is this? {e}"))?,
        };
        Ok(Env { from, data: data_folder(&home), keys: home.join("keys"), old_keys: sealing::old_keys_dir(), secrets_key })
    }
}

/// The data folder `holon serve` uses when given none: HOLON_DATA, else `<home>/data`.
fn data_folder(home: &Path) -> PathBuf {
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
    let (mut yes, mut check, mut from, mut secrets_key) = (false, false, None, None);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--yes" | "-y" => yes = true,
            "--check" => check = true,
            "--from" => from = Some(PathBuf::from(args.next().ok_or("--from needs a folder")?)),
            "--secrets-key" => secrets_key = Some(args.next().ok_or("--secrets-key needs os, env:VAR or file:PATH")?.clone()),
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(0);
            }
            other => return Err(format!("unknown argument {other:?}\n\n{USAGE}")),
        }
    }
    let env = Env::here(from, secrets_key)?;
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
            println!("  can't, yet: {why}");
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
    (!found.is_empty()).then(|| format!("{} holds an older server's data ({}), which this one doesn't read: `holon migrate` moves it", here.display(), found.join(", ")))
}

// ── data-in-a-folder ─────────────────────────────────────────────────────────────────────────

/// Whether `dir` holds a server already: any of the data folder's files.
fn holds_a_server(dir: &Path) -> bool {
    OLD_DATA.iter().any(|(_, new)| dir.join(new).exists())
}

fn data_needed(env: &Env) -> Option<Plan> {
    let mut moves: Vec<(PathBuf, PathBuf)> = OLD_DATA
        .iter()
        .map(|(old, new)| (env.from.join(old), env.data.join(new)))
        .filter(|(old, _)| old.exists())
        .collect();
    if moves.is_empty() {
        return None;
    }
    let policy = env.from.join(OLD_POLICY.0);
    if policy.is_file() {
        moves.push((policy, env.data.join(OLD_POLICY.1)));
    }
    let mut lines = vec![format!("move a server's data from {} into {}:", env.from.display(), env.data.display())];
    lines.extend(moves.iter().map(|(old, new)| format!("  {} -> {}", name(old), name(new))));
    let blocked = holds_a_server(&env.data).then(|| format!("{} holds a server already: move it aside, or migrate into another with HOLON_DATA", env.data.display()));
    Some(Plan { lines, blocked, moves })
}

fn name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn data_apply(env: &Env, plan: &Plan) -> Result<String, String> {
    // The secrets open with their key before anything moves; opening records the key's id in
    // the file, so the copy finds the same key wherever it goes.
    let old_secrets = env.from.join(".holon-secrets.json");
    if old_secrets.exists() {
        open_secrets(&old_secrets, env.secrets_key.as_deref()).map_err(|e| format!("{e}; nothing was moved"))?;
    }
    let partial = env.data.with_file_name(format!("{}.partial", name(&env.data)));
    if partial.exists() {
        // What an interrupted run left: the old files are all still there.
        std::fs::remove_dir_all(&partial).map_err(|e| format!("could not clear {}: {e}", partial.display()))?;
    }
    std::fs::create_dir_all(&partial).map_err(|e| format!("could not make {}: {e}", partial.display()))?;
    for (old, new) in &plan.moves {
        copy_all(old, &partial.join(name(new))).map_err(|e| format!("could not copy {}: {e}", old.display()))?;
    }
    check_copy(&partial, env.secrets_key.as_deref()).map_err(|e| format!("the copy in {} doesn't check: {e}; the old files are untouched", partial.display()))?;
    if env.data.exists() {
        // Empty (a server there was refused above): only an empty folder is removed.
        std::fs::remove_dir(&env.data).map_err(|e| format!("{} isn't empty: {e}", env.data.display()))?;
    }
    std::fs::rename(&partial, &env.data).map_err(|e| format!("could not rename {} to {}: {e}", partial.display(), env.data.display()))?;
    for (old, _) in &plan.moves {
        let gone = if old.is_dir() { std::fs::remove_dir_all(old) } else { std::fs::remove_file(old) };
        gone.map_err(|e| format!("moved, but could not remove {}: {e}", old.display()))?;
    }
    Ok(format!("{} things moved into {}", plan.moves.len(), env.data.display()))
}

fn open_secrets(path: &Path, key_arg: Option<&str>) -> Result<(), String> {
    let path = path.to_string_lossy();
    let key = KeySource::parse(key_arg, &path)?;
    crate::secrets::SecretStore::open(&path, &key, crate::secret_sources::Sources::none()).map(|_| ())
}

/// The copy is a server's: the store opens, the tables parse, the secrets open with their key.
fn check_copy(dir: &Path, key_arg: Option<&str>) -> Result<(), String> {
    let store = dir.join("store");
    if store.exists() {
        holon_host::FileStore::new(&store).map_err(|e| format!("the store: {e}"))?;
    }
    for table in ["deployments.json", "flows.json", "accounts.json"] {
        let path = dir.join(table);
        if path.exists() {
            let bytes = std::fs::read(&path).map_err(|e| format!("{table}: {e}"))?;
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| format!("{table}: {e}"))?;
        }
    }
    let secrets = dir.join("secrets.json");
    if secrets.exists() {
        open_secrets(&secrets, key_arg)?;
    }
    Ok(())
}

/// `from`, a file or a folder, copied to `to`; a file kept owner-only stays so.
fn copy_all(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_all(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
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
    let lines = vec![format!("move {} secrets key file(s) from {} into {}", moves.len(), old.display(), env.keys.display())];
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
        Env { from: dir.join("old"), data: dir.join("home").join("data"), keys: dir.join("home").join("keys"), old_keys: Some(dir.join("config").join("holon").join("keys")), secrets_key: None }
    }

    /// An older server's folder: a store, its tables, a log and a policy, and a secret sealed
    /// under a key file named for where the secrets file was.
    fn old_server(env: &Env) -> String {
        let old = &env.from;
        std::fs::create_dir_all(old.join(".holon-store").join("ab")).unwrap();
        std::fs::write(old.join(".holon-store").join("ab").join("unit"), b"bytes").unwrap();
        std::fs::write(old.join(".holon-deployments.json"), b"[]").unwrap();
        std::fs::create_dir_all(old.join(".holon-deployments.logs")).unwrap();
        std::fs::write(old.join(".holon-deployments.logs").join("a.jsonl"), b"{}\n").unwrap();
        std::fs::write(old.join(".holon-accounts.json"), b"{}").unwrap();
        std::fs::write(old.join(".holon-host-key"), b"k").unwrap();
        std::fs::write(old.join("holon.toml"), b"").unwrap();
        let secrets = old.join(".holon-secrets.json").to_string_lossy().into_owned();
        let key = KeySource::File(env.keys.join(format!("{}.key", sealing::path_key_id(&secrets))));
        let store = crate::secrets::SecretStore::open(&secrets, &key, crate::secret_sources::Sources::none()).unwrap();
        store.set("app", "token", "s3cret", None).unwrap();
        secrets
    }

    #[test]
    fn an_older_servers_folder_moves_into_the_data_folder_and_its_secrets_still_open() {
        let dir = temp("data");
        let env = env(&dir);
        assert!(data_needed(&env).is_none(), "nothing there: nothing to do");
        let secrets = old_server(&env);
        let plan = data_needed(&env).expect("the old files are found");
        assert!(plan.blocked.is_none());
        assert_eq!(plan.moves.len(), 8, "seven of the server's files (the secrets' audit too) and the policy");
        // The key is named for the old path; the moved file finds it by the id it records.
        let id = sealing::path_key_id(&secrets);
        let key_file = env.keys.join(format!("{id}.key"));
        let key = KeySource::File(key_file.clone());
        crate::secrets::SecretStore::open(&secrets, &key, crate::secret_sources::Sources::none()).unwrap();
        // apply opens with the default key source, which on this OS may be the key store: name it.
        let env = Env { secrets_key: Some(format!("file:{}", key_file.display())), ..env };
        data_apply(&env, &plan).unwrap();
        for (old, new) in OLD_DATA.iter().filter(|(o, _)| !o.contains("heads") && !o.contains("activity") && !o.contains("flows") && !o.contains("audit")) {
            assert!(!env.from.join(old).exists(), "{old} removed");
            assert!(env.data.join(new).exists(), "{new} there");
        }
        assert_eq!(std::fs::read(env.data.join("store").join("ab").join("unit")).unwrap(), b"bytes");
        assert!(env.data.join("holon.toml").exists());
        let moved = env.data.join("secrets.json").to_string_lossy().into_owned();
        assert_eq!(sealing::key_id(&moved), id, "the file records the key's id");
        let store = crate::secrets::SecretStore::open(&moved, &key, crate::secret_sources::Sources::none()).unwrap();
        assert_eq!(store.names("app"), ["token"]);
        assert!(data_needed(&env).is_none(), "applied: nothing left to do");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_data_folder_that_holds_a_server_blocks_the_move_and_a_wrong_key_moves_nothing() {
        let dir = temp("blocked");
        let env = env(&dir);
        old_server(&env);
        std::fs::create_dir_all(&env.data).unwrap();
        std::fs::write(env.data.join("accounts.json"), b"{}").unwrap();
        assert!(data_needed(&env).unwrap().blocked.unwrap().contains("holds a server already"));
        std::fs::remove_file(env.data.join("accounts.json")).unwrap();
        // A key that isn't the one that sealed the secrets: refused before anything moves.
        let wrong = dir.join("wrong.key");
        KeySource::File(wrong.clone()).create().unwrap();
        let env = Env { secrets_key: Some(format!("file:{}", wrong.display())), ..env };
        let plan = data_needed(&env).unwrap();
        let e = data_apply(&env, &plan).unwrap_err();
        assert!(e.contains("different key") && e.contains("nothing was moved"), "{e}");
        assert!(env.from.join(".holon-store").exists() && !env.data.join("store").exists());
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
