//! The same OS user's way in, without a token to copy: a running server and the command line on
//! one machine.
//!
//! `holon serve` writes a run file, readable only by the user it runs as, into `~/.holon/run`
//! (or `HOLON_RUN_DIR`): its address, and a token made for this run of it, good as that user's
//! admin until the server stops. `holon export` (and what follows it on the command line) reads
//! it: with one server running, nothing to pass; with several, `--server <port>` picks one.
//!
//! This trusts no more than the machine already does: whatever can read the file runs as that
//! user, and so could read the server's data folder and its secrets key as well. A connection
//! from loopback alone proves nothing (a web page, another user's program and a port forward
//! all arrive there), so the control plane still asks every request who it is; the file is how
//! the user's own commands answer. A server elsewhere, or a script in CI, uses an API token
//! (`HOLON_TOKEN`), as before, or the token `holon mcp setup --server` kept for that server in
//! `~/.holon/tokens.json`, readable only by the user.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// What a running server tells its user's command line.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct RunFile {
    /// Where to reach its control plane, on this machine (`http://127.0.0.1:7420`).
    pub(crate) server: String,
    /// Good as this user's admin while this run of the server lasts.
    pub(crate) token: String,
    pub(crate) pid: u32,
}

/// This user's home folder.
pub(crate) fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Where run files are kept: `HOLON_RUN_DIR`, else `~/.holon/run`.
pub(crate) fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("HOLON_RUN_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    home().map(|h| h.join(".holon").join("run"))
}

/// Where the tokens for servers elsewhere are kept: `~/.holon/tokens.json`, each server's URL to
/// its token, readable only by this user.
pub(crate) fn tokens_file() -> Option<PathBuf> {
    home().map(|h| h.join(".holon").join("tokens.json"))
}

fn tokens_in(file: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The token kept for `server` (a URL, as [`server_url`] writes it).
pub(crate) fn saved_token(file: &Path, server: &str) -> Option<String> {
    tokens_in(file).remove(server)
}

/// Keep `token` for `server`, or with none, forget the one kept: whether there was one before.
pub(crate) fn keep_token(file: &Path, server: &str, token: Option<&str>) -> Result<bool, String> {
    let mut all = tokens_in(file);
    let had = match token {
        Some(t) => all.insert(server.to_string(), t.to_string()).is_some(),
        None => all.remove(server).is_some(),
    };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(&all).unwrap_or_default();
    crate::sealing::write_private(file, &bytes).map_err(|e| format!("couldn't write {}: {e}", file.display()))?;
    Ok(had)
}

/// The operating-system user this runs as, as the activity log names local commands.
pub(crate) fn user() -> String {
    let var = if cfg!(windows) { "USERNAME" } else { "USER" };
    std::env::var(var).ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "this user".into())
}

/// The address a client on this machine reaches a server bound at `bound` on: loopback for a
/// server listening on every interface.
pub(crate) fn local_url(bound: std::net::SocketAddr) -> String {
    let ip = match bound.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    format!("http://{}", std::net::SocketAddr::new(ip, bound.port()))
}

/// Write this server's run file (`<port>.json`), with `token`. Its path, to remove at exit; none
/// when there's nowhere to keep it (said once, and the server runs on: only the shortcut is lost).
pub(crate) fn publish(bound: std::net::SocketAddr, token: &str) -> Option<PathBuf> {
    let dir = dir()?;
    let file = RunFile { server: local_url(bound), token: token.into(), pid: std::process::id() };
    let path = dir.join(format!("{}.json", bound.port()));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()| crate::sealing::write_private(&path, &serde_json::to_vec(&file).unwrap_or_default()));
    match written {
        Ok(()) => Some(path),
        Err(e) => {
            eprintln!("holon: couldn't write {} ({e}): commands on this machine will need HOLON_TOKEN", path.display());
            None
        }
    }
}

/// Every run file there is, with where it is.
fn run_files(dir: &Path) -> Vec<(PathBuf, RunFile)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<(PathBuf, RunFile)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| Some((p.clone(), serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?)))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// Whether a holon answers at `server`: `Some(true)` it does, `Some(false)` nothing listens
/// there (the run file is stale), `None` something else is in the way (it may be busy).
fn answers(server: &str) -> Option<bool> {
    match ureq::get(&format!("{server}/health")).timeout(Duration::from_secs(3)).call() {
        Ok(r) => Some(r.into_string().is_ok_and(|b| b.trim() == "ok")),
        Err(ureq::Error::Transport(t)) if t.kind() == ureq::ErrorKind::ConnectionFailed => Some(false),
        Err(_) => None,
    }
}

/// `--server` as given: a port alone (`7401`), `host:port`, or a URL.
pub(crate) fn server_url(given: &str) -> String {
    let given = given.trim().trim_end_matches('/');
    if given.parse::<u16>().is_ok() {
        format!("http://127.0.0.1:{given}")
    } else if given.contains("://") {
        given.to_string()
    } else {
        format!("http://{given}")
    }
}

/// The server a command talks to, and the token it signs in with.
#[derive(Debug, PartialEq)]
pub(crate) struct Target {
    pub(crate) server: String,
    pub(crate) token: Option<String>,
}

/// Which server, and how to sign in: `--server` (or `HOLON_SERVER`) if given, else the one server
/// this user runs on this machine; `HOLON_TOKEN` if set, else the token kept for that server
/// (`holon mcp setup --server`, chosen for it), else its run file's.
pub(crate) fn target(given: Option<&str>) -> Result<Target, String> {
    let env_token = std::env::var("HOLON_TOKEN").ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let given = given.map(str::to_string).or_else(|| std::env::var("HOLON_SERVER").ok().filter(|s| !s.trim().is_empty()));
    let files = dir().map(|d| run_files(&d)).unwrap_or_default();
    let kept = |server: &str| tokens_file().and_then(|f| saved_token(&f, server));
    target_from(given.as_deref(), env_token, files, answers, kept)
}

/// [`target`], given what it reads: testable without a server.
fn target_from(
    given: Option<&str>,
    env_token: Option<String>,
    files: Vec<(PathBuf, RunFile)>,
    answers: impl Fn(&str) -> Option<bool>,
    kept: impl Fn(&str) -> Option<String>,
) -> Result<Target, String> {
    if let Some(given) = given {
        let server = server_url(given);
        let from_run = files.into_iter().find(|(_, f)| f.server == server).map(|(_, f)| f.token);
        let token = env_token.or_else(|| kept(&server)).or(from_run);
        return Ok(Target { server, token });
    }
    let mut running = Vec::new();
    for (path, file) in files {
        match answers(&file.server) {
            Some(true) => running.push(file),
            // Nothing listens there: the server stopped without removing it.
            Some(false) => {
                let _ = std::fs::remove_file(&path);
            }
            None => {}
        }
    }
    match running.len() {
        1 => {
            let file = running.remove(0);
            Ok(Target { server: file.server, token: env_token.or(Some(file.token)) })
        }
        0 => Err("no holon serve is running for you on this machine: start one, or give --server <port or URL> \
                  (and HOLON_TOKEN, for a server elsewhere)"
            .into()),
        _ => {
            let ports: Vec<String> = running.iter().map(|f| f.server.rsplit(':').next().unwrap_or("").to_string()).collect();
            Err(format!("{} holon servers are running here: pick one with --server {}", ports.len(), ports.join(" or --server ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    fn file(port: u16) -> (PathBuf, RunFile) {
        let f = RunFile { server: format!("http://127.0.0.1:{port}"), token: format!("hl_{port}"), pid: 1 };
        (PathBuf::from(format!("/nowhere/{port}.json")), f)
    }

    #[test]
    fn server_takes_a_port_an_address_or_a_url() {
        assert_eq!(server_url("7401"), "http://127.0.0.1:7401");
        assert_eq!(server_url("holon.example.com:443"), "http://holon.example.com:443");
        assert_eq!(server_url("https://holon.example.com/"), "https://holon.example.com");
    }

    #[test]
    fn the_one_server_running_is_the_target_with_its_token() {
        let up = |s: &str| Some(s.ends_with("7401"));
        let t = target_from(None, None, vec![file(7400), file(7401)], up, none).unwrap();
        assert_eq!(t, Target { server: "http://127.0.0.1:7401".into(), token: Some("hl_7401".into()) });
        // HOLON_TOKEN wins over the file's.
        let t = target_from(None, Some("ht_x".into()), vec![file(7401)], |_| Some(true), none).unwrap();
        assert_eq!(t.token.as_deref(), Some("ht_x"));
    }

    #[test]
    fn none_or_several_running_say_what_to_pass() {
        let nothing = target_from(None, None, vec![file(7401)], |_| Some(false), none).unwrap_err();
        assert!(nothing.contains("no holon serve is running"), "{nothing}");
        let two = target_from(None, None, vec![file(7401), file(7420)], |_| Some(true), none).unwrap_err();
        assert!(two.contains("--server 7401 or --server 7420"), "{two}");
    }

    #[test]
    fn a_server_given_by_port_signs_in_with_its_run_file() {
        let t = target_from(Some("7420"), None, vec![file(7401), file(7420)], |_| unreachable!("not probed"), none).unwrap();
        assert_eq!(t, Target { server: "http://127.0.0.1:7420".into(), token: Some("hl_7420".into()) });
        let elsewhere = target_from(Some("https://holon.example.com"), None, vec![file(7420)], |_| None, none).unwrap();
        assert_eq!(elsewhere.token, None);
        // The token kept for it, over its run file's; HOLON_TOKEN still wins.
        let kept = |s: &str| (s == "https://holon.example.com" || s.ends_with(":7420")).then(|| "ht_kept".to_string());
        assert_eq!(target_from(Some("7420"), None, vec![file(7420)], |_| None, kept).unwrap().token.as_deref(), Some("ht_kept"));
        assert_eq!(target_from(Some("https://holon.example.com/"), None, vec![], |_| None, kept).unwrap().token.as_deref(), Some("ht_kept"));
        assert_eq!(target_from(Some("https://holon.example.com"), Some("ht_env".into()), vec![], |_| None, kept).unwrap().token.as_deref(), Some("ht_env"));
    }

    #[test]
    fn a_token_is_kept_per_server_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("holon-tokens-{}", std::process::id()));
        let file = dir.join("tokens.json");
        assert!(!keep_token(&file, "https://a.example", Some("ht_a")).unwrap());
        assert!(keep_token(&file, "https://a.example", Some("ht_a2")).unwrap(), "replaced");
        keep_token(&file, "https://b.example", Some("ht_b")).unwrap();
        assert_eq!(saved_token(&file, "https://a.example").as_deref(), Some("ht_a2"));
        assert!(keep_token(&file, "https://a.example", None).unwrap());
        assert_eq!(saved_token(&file, "https://a.example"), None);
        assert_eq!(saved_token(&file, "https://b.example").as_deref(), Some("ht_b"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
