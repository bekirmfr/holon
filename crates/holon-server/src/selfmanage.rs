//! The commands that look after this installation rather than serve: `version`, `update` and
//! `uninstall`. They match what the release installers (cargo-dist, dist-workspace.toml) do:
//! the binary in `~/.holon/bin`, that folder on PATH, and an install receipt
//! (`holon-server-receipt.json`) recording where it went.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, UpdateRequest, Version};

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
const APP: &str = "holon-server";
const OWNER: &str = "bekirmfr";
const REPO: &str = "holon";

const USAGE: &str = "\
holon-server version               this build's version
holon-server update --check        is there a newer release?
holon-server update [--to VERSION] install the newest release (or that one) over this one
holon-server uninstall [--purge] [--data DIR] [--yes]
                                   remove holon-server (and with --purge, its data and key)

`update` takes --pre to include pre-releases (on by default when this build is one).";

/// The exit code, when `args` (after the program name) is one of these commands.
pub(crate) fn dispatch(args: &[String]) -> Option<i32> {
    let result = match args.first().map(String::as_str) {
        Some("version" | "--version" | "-V") => {
            println!("holon-server {VERSION}");
            Ok(())
        }
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("update") => update(&args[1..]),
        Some("uninstall") => uninstall(&args[1..]),
        _ => return None,
    };
    Some(match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("holon-server: {e}");
            1
        }
    })
}

// ---- update ----

fn update(args: &[String]) -> Result<(), String> {
    let (mut check, mut pre, mut to) = (false, false, None);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--check" => check = true,
            "--pre" => pre = true,
            "--to" => to = Some(args.next().ok_or("--to needs a version, e.g. 0.1.0")?.trim_start_matches('v').to_string()),
            other => return Err(format!("update: unknown argument {other:?}\n\n{USAGE}")),
        }
    }
    let current = Version::parse(VERSION).map_err(|e| format!("this build's version {VERSION:?}: {e}"))?;
    let request = match to {
        Some(v) => UpdateRequest::SpecificVersion(v),
        // A pre-release build follows pre-releases; a full release, only full releases.
        None if pre || !current.pre.is_empty() => UpdateRequest::LatestMaybePrerelease,
        None => UpdateRequest::Latest,
    };
    let mut updater = AxoUpdater::new_for(APP);
    updater.set_release_source(ReleaseSource {
        release_type: ReleaseSourceType::GitHub,
        owner: OWNER.into(),
        name: REPO.into(),
        app_name: APP.into(),
    });
    if let Ok(token) = std::env::var("HOLON_SERVER_GITHUB_TOKEN") {
        updater.set_github_token(&token);
    }

    if check {
        updater.configure_version_specifier(request);
        let newest = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?
            .block_on(updater.query_new_version())
            .map_err(|e| format!("could not look up the releases: {e}"))?
            .cloned()
            .ok_or("no release found")?;
        if newest > current {
            println!("holon-server {VERSION}: {newest} is available (holon-server update installs it)");
        } else {
            println!("holon-server {VERSION} is up to date");
        }
        return Ok(());
    }

    // Updating runs the release's installer into the folder this copy was installed to, which
    // only the receipt knows: a copy unpacked by hand is updated by hand.
    let exe = this_exe()?;
    let receipt = Receipt::find().ok_or_else(|| {
        format!(
            "{} wasn't installed by the holon-server installer (there's no install receipt), so it can't update itself: download the new release from https://github.com/{OWNER}/{REPO}/releases",
            exe.display()
        )
    })?;
    if !receipt.covers(&exe) {
        return Err(format!(
            "{} isn't the installed copy (that's in {}): run that one's update, or replace this one by hand",
            exe.display(),
            receipt.install_prefix.display()
        ));
    }
    updater.load_receipt().map_err(|e| format!("the install receipt: {e}"))?;
    updater.set_current_version(current).map_err(|e| e.to_string())?;
    updater.configure_version_specifier(request);
    match updater.run_sync().map_err(|e| format!("the update failed: {e}"))? {
        Some(done) => println!(
            "holon-server: updated {} to {} in {} (restart the server to run it)",
            done.old_version.map(|v| v.to_string()).unwrap_or_else(|| VERSION.into()),
            done.new_version,
            done.install_prefix
        ),
        None => println!("holon-server {VERSION} is up to date"),
    }
    Ok(())
}

// ---- uninstall ----

fn uninstall(args: &[String]) -> Result<(), String> {
    let (mut yes, mut purge) = (false, false);
    let mut data_dir: Option<PathBuf> = std::env::var_os("HOLON_DATA").filter(|v| !v.is_empty()).map(PathBuf::from);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--yes" | "-y" => yes = true,
            "--purge" => purge = true,
            "--data" => data_dir = Some(PathBuf::from(args.next().ok_or("--data needs a folder")?)),
            other => return Err(format!("uninstall: unknown argument {other:?}\n\n{USAGE}")),
        }
    }
    let home = home_dir();
    let data_dir = data_dir.or_else(|| home.as_ref().map(|h| h.join(".holon").join("data")));

    let exe = this_exe()?;
    let bin = exe.parent().ok_or("this executable has no folder")?.to_path_buf();
    let receipt = Receipt::find().filter(|r| r.covers(&exe));

    // Say what goes before anything does.
    let mut plan = vec![format!("the program {}", exe.display())];
    if let Some(r) = &receipt {
        if r.modify_path {
            plan.push(format!("{} from your PATH", bin.display()));
        }
        plan.push(format!("the install receipt {}", r.path.display()));
    }
    let data = data_dir.filter(|d| d.is_dir());
    if purge {
        match &data {
            Some(d) => plan.push(format!("ALL of holon-server's data in {} (flows, deployments, runs, secrets, accounts) and its secrets key", d.display())),
            None => return Err("--purge: no data folder found; name it with --data DIR".into()),
        }
    }
    println!("holon-server uninstall will remove:");
    for line in &plan {
        println!("  - {line}");
    }
    if let (false, Some(d)) = (purge, &data) {
        println!("and keep your data in {} (add --purge to remove it too)", d.display());
    }
    if !yes && !confirm(purge)? {
        println!("nothing removed");
        return Ok(());
    }

    if let Some(r) = &receipt {
        if r.modify_path {
            path_env::remove(&bin, home.as_deref())?;
        }
        remove_file_if_there(&r.path)?;
        if let Some(dir) = r.path.parent() {
            let _ = std::fs::remove_dir(dir); // only if empty
        }
    }
    if let (true, Some(d)) = (purge, &data) {
        let secrets = d.join("secrets.json");
        match crate::sealing::KeySource::parse(None, &secrets.to_string_lossy()).and_then(|k| k.forget().map(|gone| (k, gone))) {
            Ok((key, true)) => println!("removed the secrets key from {key}"),
            Ok(_) => {}
            Err(e) => eprintln!("holon-server: the secrets key: {e} (remove it by hand)"),
        }
        std::fs::remove_dir_all(d).map_err(|e| format!("could not remove {}: {e}", d.display()))?;
        println!("removed {}", d.display());
    }
    self_replace::self_delete().map_err(|e| format!("could not remove {}: {e}", exe.display()))?;
    // The folders the installer made, if nothing else is left in them. (On Windows the program
    // itself goes once it exits, so its folder stays.)
    if let (Ok(()), Some(parent)) = (std::fs::remove_dir(&bin), bin.parent()) {
        let _ = std::fs::remove_dir(parent);
    }
    println!("holon-server is uninstalled{}", if receipt.as_ref().is_some_and(|r| r.modify_path) { " (open a new terminal for the PATH change)" } else { "" });
    Ok(())
}

fn confirm(purge: bool) -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        return Err("not a terminal to ask in: pass --yes to go ahead".into());
    }
    let word = if purge { "purge" } else { "yes" };
    print!("type {word} to go ahead: ");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).map_err(|e| e.to_string())?;
    Ok(answer.trim().eq_ignore_ascii_case(word) || (!purge && answer.trim().eq_ignore_ascii_case("y")))
}

fn remove_file_if_there(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not remove {}: {e}", path.display())),
    }
}

fn this_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("where is this program? {e}"))
}

fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// What the installer recorded: where it put the program and whether it changed PATH.
struct Receipt {
    path: PathBuf,
    install_prefix: PathBuf,
    modify_path: bool,
}

impl Receipt {
    /// Where the installers write it (axoupdater reads the same places).
    fn find() -> Option<Receipt> {
        let name = format!("{APP}-receipt.json");
        let mut dirs = Vec::new();
        if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(x).join(APP));
        }
        if cfg!(windows) {
            dirs.extend(std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(APP)));
        } else {
            dirs.extend(home_dir().map(|h| h.join(".config").join(APP)));
        }
        dirs.into_iter().find_map(|dir| {
            let path = dir.join(&name);
            let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
            Some(Receipt {
                install_prefix: PathBuf::from(json.get("install_prefix")?.as_str()?),
                modify_path: json.get("modify_path").and_then(|v| v.as_bool()).unwrap_or(false),
                path,
            })
        })
    }

    /// Whether `exe` is the copy this receipt installed: in the prefix, or its `bin`.
    fn covers(&self, exe: &Path) -> bool {
        let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
        let Some(dir) = exe.parent() else { return false };
        let prefix = self.install_prefix.canonicalize().unwrap_or_else(|_| self.install_prefix.clone());
        dir == prefix || (dir.file_name().is_some_and(|n| n == "bin") && dir.parent() == Some(prefix.as_path()))
    }
}

/// Undoing the installer's PATH change.
mod path_env {
    use std::path::Path;

    /// The user `Path` (in the registry) without `bin`, keeping its other entries as written
    /// (unexpanded), then a broadcast so new terminals see it.
    #[cfg(windows)]
    pub(super) fn remove(bin: &Path, _home: Option<&Path>) -> Result<(), String> {
        let dir = bin.to_string_lossy().replace('\'', "''");
        let script = format!(
            r#"$ErrorActionPreference = 'Stop'
$dir = '{dir}'.TrimEnd('\')
$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
$old = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
if ($old) {{
  $kind = $key.GetValueKind('Path')
  $new = ($old -split ';' | Where-Object {{ $_ -and ([Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -ne $dir) }}) -join ';'
  if ($new -ne $old) {{
    $key.SetValue('Path', $new, $kind)
    [Environment]::SetEnvironmentVariable('HOLON_SERVER_UNINSTALL', '1', 'User')
    [Environment]::SetEnvironmentVariable('HOLON_SERVER_UNINSTALL', [NullString]::Value, 'User')
    'removed'
  }}
}}"#
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|e| format!("could not run PowerShell to change PATH: {e}"))?;
        if !out.status.success() {
            return Err(format!("could not remove {} from PATH: {}", bin.display(), String::from_utf8_lossy(&out.stderr).trim()));
        }
        if String::from_utf8_lossy(&out.stdout).contains("removed") {
            println!("removed {} from your PATH", bin.display());
        }
        Ok(())
    }

    /// The `. "<bin>/env"` lines the installer added to the shell startup files, its fish
    /// config, and the env scripts those lines ran.
    #[cfg(not(windows))]
    pub(super) fn remove(bin: &Path, home: Option<&Path>) -> Result<(), String> {
        let Some(home) = home else { return Ok(()) };
        let exprs = super::env_exprs(bin, home);
        let mut homes = vec![home.to_path_buf()];
        homes.extend(std::env::var_os("ZDOTDIR").filter(|v| !v.is_empty()).map(std::path::PathBuf::from));
        for dir in &homes {
            for rc in [".profile", ".bashrc", ".bash_profile", ".bash_login", ".zshrc", ".zshenv"] {
                let path = dir.join(rc);
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                if let Some(stripped) = super::strip_rc(&text, &exprs) {
                    std::fs::write(&path, stripped).map_err(|e| format!("could not edit {}: {e}", path.display()))?;
                    println!("removed holon-server's PATH line from {}", path.display());
                }
            }
        }
        super::remove_file_if_there(&home.join(".config/fish/conf.d").join(format!("{}.env.fish", super::APP)))?;
        super::remove_file_if_there(&bin.join("env"))?;
        super::remove_file_if_there(&bin.join("env.fish"))?;
        Ok(())
    }
}

/// How the installer may have written `bin` in a startup file: as is, or under `$HOME`.
#[cfg_attr(windows, allow(dead_code))]
fn env_exprs(bin: &Path, home: &Path) -> Vec<String> {
    let mut exprs = vec![bin.to_string_lossy().into_owned()];
    if let Ok(rest) = bin.strip_prefix(home) {
        exprs.push(format!("$HOME/{}", rest.to_string_lossy()));
    }
    exprs
}

/// `text` without the lines sourcing `<expr>/env` (or `env.fish`), each with the blank line
/// the installer put before it; `None` when there were none.
#[cfg_attr(windows, allow(dead_code))]
fn strip_rc(text: &str, exprs: &[String]) -> Option<String> {
    let ours = |line: &str| {
        let line = line.trim();
        exprs.iter().any(|e| {
            [format!(". \"{e}/env\""), format!("source \"{e}/env\""), format!("source \"{e}/env.fish\"")].iter().any(|l| l == line)
        })
    };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    if !lines.iter().any(|l| ours(l)) {
        return None;
    }
    let mut kept: Vec<&str> = Vec::new();
    for line in lines {
        if ours(line) {
            if kept.last().is_some_and(|l| l.trim().is_empty()) {
                kept.pop();
            }
        } else {
            kept.push(line);
        }
    }
    Some(kept.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installers_path_lines_come_out_and_nothing_else() {
        let exprs = env_exprs(Path::new("/home/ada/.holon/bin"), Path::new("/home/ada"));
        assert_eq!(exprs, ["/home/ada/.holon/bin", "$HOME/.holon/bin"]);
        let rc = "export EDITOR=vim\n\n. \"$HOME/.holon/bin/env\"\nalias ll='ls -l'\n\nsource \"/home/ada/.holon/bin/env\"\n";
        assert_eq!(strip_rc(rc, &exprs).unwrap(), "export EDITOR=vim\nalias ll='ls -l'\n");
        // Someone else's env script, or a line that only mentions ours, stays.
        let other = "\n. \"$HOME/.cargo/env\"\n# . \"$HOME/.holon/bin/env\"\n";
        assert_eq!(strip_rc(other, &exprs), None);
    }

    #[test]
    fn a_receipt_covers_the_copy_in_its_prefix_or_its_bin() {
        let dir = std::env::temp_dir().join(format!("holon-receipt-{}", std::process::id()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let dir = dir.canonicalize().unwrap();
        let receipt = |prefix: &Path| Receipt { path: PathBuf::new(), install_prefix: prefix.to_path_buf(), modify_path: true };
        let exe = dir.join("bin").join("holon-server");
        assert!(receipt(&dir).covers(&exe));
        assert!(receipt(&dir.join("bin")).covers(&exe));
        assert!(!receipt(&dir.join("elsewhere")).covers(&exe));
        std::fs::remove_dir_all(&dir).ok();
    }
}
