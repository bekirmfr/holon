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
//!
//! `holon mcp` never signs in with the run file's token: it signs in as an agent ([`agent_target`]).
//! On a server this user runs, the run file's admin makes them one (mcphosts.rs), whose token is
//! kept as `agent:<URL>`, a key no other command reads, so `holon deploy` stays the person.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// What a running server tells its user's command line.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct RunFile {
    /// Where to reach its control plane, on this machine (`http://127.0.0.1:7420`).
    pub(crate) server: String,
    /// Good while this run of the server lasts: as the account the access document ties it to (the
    /// person who set the server up), holding its bindings and admin on the host; tied to none, as
    /// this user's admin (access.rs `authenticate`).
    pub(crate) token: String,
    pub(crate) pid: u32,
}

/// This user's home folder.
pub(crate) fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The one folder holon keeps everything in (docs/L4s/holon-home-L4.md): `HOLON_HOME`, else
/// `~/.holon`. The program, the server's data, keys, run files and tokens are all below it.
pub(crate) fn holon_home() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("HOLON_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    home().map(|h| h.join(".holon"))
}

/// Where run files are kept: `HOLON_RUN_DIR`, else `~/.holon/run`.
pub(crate) fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("HOLON_RUN_DIR").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(d));
    }
    holon_home().map(|h| h.join("run"))
}

/// Where the tokens for servers elsewhere are kept: `~/.holon/tokens.json`, each server's URL to
/// its token (and `agent:<URL>` to the agent's `holon mcp` made on a server this user runs),
/// readable only by this user.
pub(crate) fn tokens_file() -> Option<PathBuf> {
    holon_home().map(|h| h.join("tokens.json"))
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

/// The servers on this machine that may still be running: a run file whose server answers, or
/// one that can't be told stale (something answered, or nothing could be asked).
pub(crate) fn live_servers() -> Vec<String> {
    let Some(dir) = dir() else { return Vec::new() };
    run_files(&dir).into_iter().filter(|(_, f)| answers(&f.server) != Some(false)).map(|(_, f)| f.server).collect()
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
    let (given, env_token, files) = asked(given);
    let kept = |server: &str| tokens_file().and_then(|f| saved_token(&f, server));
    target_from(given.as_deref(), env_token, files, answers, kept)
}

/// What [`target`] and [`agent_target`] read: the server given, `HOLON_TOKEN`, the run files.
fn asked(given: Option<&str>) -> (Option<String>, Option<String>, Vec<(PathBuf, RunFile)>) {
    let env_token = std::env::var("HOLON_TOKEN").ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let given = given.map(str::to_string).or_else(|| std::env::var("HOLON_SERVER").ok().filter(|s| !s.trim().is_empty()));
    (given, env_token, dir().map(|d| run_files(&d)).unwrap_or_default())
}

/// The server `holon mcp` talks to, and how it signs in there: as an agent, never with the run
/// file's token.
#[derive(Debug, PartialEq)]
pub(crate) struct AgentTarget {
    pub(crate) server: String,
    /// `HOLON_TOKEN`, else the agent token kept for the server.
    pub(crate) token: Option<String>,
    /// The run file's token, on a server this user runs and with no `HOLON_TOKEN`: only to make
    /// this user's agent there (mcphosts::local_agent), never to sign in.
    pub(crate) run_token: Option<String>,
}

/// The run file's token for `server` (a URL), when this user runs it here.
pub(crate) fn run_token(server: &str) -> Option<String> {
    let files = dir().map(|d| run_files(&d)).unwrap_or_default();
    files.into_iter().find(|(_, f)| f.server == server).map(|(_, f)| f.token)
}

/// The key a local server's agent token is kept under in `tokens.json`.
pub(crate) fn agent_key(server: &str) -> String {
    format!("agent:{server}")
}

/// Which server, as [`target`] finds it, and the agent token `holon mcp` signs in with:
/// `HOLON_TOKEN`, else the one kept for that server (`agent:<URL>`; for a server this user
/// doesn't run, `<URL>` too, where `holon mcp setup --server` keeps it).
pub(crate) fn agent_target(given: Option<&str>) -> Result<AgentTarget, String> {
    let (given, env_token, files) = asked(given);
    let kept = |key: &str| tokens_file().and_then(|f| saved_token(&f, key));
    agent_target_from(given.as_deref(), env_token, files, answers, kept)
}

/// Ask the server this user runs here (or `server`) as its admin, for a command named `who`:
/// the server, and what it answered; or, said on stderr, the exit code to end with.
pub(crate) fn admin_call(who: &str, server: Option<&str>, method: &str, path: &str, body: Option<&serde_json::Value>) -> Result<(String, String), i32> {
    let body = body.map(|b| ("application/json", b.to_string().into_bytes()));
    let answer = send(who, server, method, path, body)?;
    if (200..300).contains(&answer.status) {
        return Ok((answer.server, answer.body));
    }
    eprintln!("{who}: {} answered {}: {}", answer.server, answer.status, answer.why());
    Err(1)
}

/// What a server answered: its status, whatever it is, and its body.
pub(crate) struct Answer {
    pub(crate) server: String,
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Answer {
    /// The `{"error": …}` sentence it answered, or its body.
    pub(crate) fn why(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.body).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| self.body.trim().to_string())
    }
}

/// Send `body` (its content type, its bytes) to the server a command named `who` talks to
/// ([`target`]), signed in as [`admin_call`] is: the answer, whatever its status; or, said on
/// stderr, the exit code to end with when there was none.
pub(crate) fn send(who: &str, server: Option<&str>, method: &str, path: &str, body: Option<(&str, Vec<u8>)>) -> Result<Answer, i32> {
    let target = target(server).map_err(|e| {
        eprintln!("{who}: {e}");
        1
    })?;
    let server = target.server;
    let mut request = ureq::request(method, &format!("{server}{path}")).timeout(Duration::from_secs(60));
    if let Some(token) = &target.token {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let answered = match body {
        Some((content_type, bytes)) => request.set("Content-Type", content_type).send_bytes(&bytes),
        None => request.call(),
    };
    let (status, response) = match answered {
        Ok(r) => (r.status(), r),
        Err(ureq::Error::Status(code, r)) => (code, r),
        Err(e) => {
            eprintln!("{who}: can't reach {server}: {e}");
            return Err(1);
        }
    };
    Ok(Answer { server, status, body: response.into_string().unwrap_or_default() })
}

/// [`target`], given what it reads: testable without a server.
fn target_from(
    given: Option<&str>,
    env_token: Option<String>,
    files: Vec<(PathBuf, RunFile)>,
    answers: impl Fn(&str) -> Option<bool>,
    kept: impl Fn(&str) -> Option<String>,
) -> Result<Target, String> {
    let (server, from_run) = server_from(given, files, answers)?;
    // A token kept for a server is for one given by name; the one server running is signed in to
    // with its run file's.
    let kept = given.and_then(|_| kept(&server));
    Ok(Target { token: env_token.or(kept).or(from_run), server })
}

/// [`agent_target`], given what it reads.
fn agent_target_from(
    given: Option<&str>,
    env_token: Option<String>,
    files: Vec<(PathBuf, RunFile)>,
    answers: impl Fn(&str) -> Option<bool>,
    kept: impl Fn(&str) -> Option<String>,
) -> Result<AgentTarget, String> {
    let (server, from_run) = server_from(given, files, answers)?;
    if env_token.is_some() {
        return Ok(AgentTarget { server, token: env_token, run_token: None });
    }
    let token = kept(&agent_key(&server)).or_else(|| from_run.is_none().then(|| kept(&server)).flatten());
    Ok(AgentTarget { server, token, run_token: from_run })
}

/// Which server: `given`, else the one this user runs here; and its run file's token, when this
/// user runs it.
fn server_from(given: Option<&str>, files: Vec<(PathBuf, RunFile)>, answers: impl Fn(&str) -> Option<bool>) -> Result<(String, Option<String>), String> {
    if let Some(given) = given {
        let server = server_url(given);
        let from_run = files.into_iter().find(|(_, f)| f.server == server).map(|(_, f)| f.token);
        return Ok((server, from_run));
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
            Ok((file.server, Some(file.token)))
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

    /// `holon mcp` signs in as an agent, never with a run file's token: `HOLON_TOKEN`, else the
    /// agent's token kept for the server; a server this user runs lends its run file's token only
    /// to make one. Every other command signs in as before, the person, whatever agent's is kept.
    #[test]
    fn holon_mcp_signs_in_as_an_agent_and_other_commands_as_before() {
        let (here, there) = ("http://127.0.0.1:7420", "https://holon.example.com");
        let agent = |server: &str, token: Option<&str>, run_token: Option<&str>| AgentTarget { server: server.into(), token: token.map(String::from), run_token: run_token.map(String::from) };
        /// `--server`, `HOLON_TOKEN`, what `tokens.json` keeps, and what `holon mcp` signs in with.
        type Case<'a> = (Option<&'a str>, Option<&'a str>, &'a [(&'a str, &'a str)], AgentTarget);
        #[rustfmt::skip]
        let cases: &[Case] = &[
            // A server this user runs: its agent's token, or none yet (and the run file's, to make one).
            (Some("7420"), None, &[("agent:http://127.0.0.1:7420", "ht_agent")], agent(here, Some("ht_agent"), Some("hl_7420"))),
            (Some("7420"), None, &[], agent(here, None, Some("hl_7420"))),
            // Not the token kept for `holon deploy --server 7420`: that may be anyone's.
            (Some("7420"), None, &[("http://127.0.0.1:7420", "ht_kept")], agent(here, None, Some("hl_7420"))),
            (None, None, &[("agent:http://127.0.0.1:7401", "ht_agent")], agent("http://127.0.0.1:7401", Some("ht_agent"), Some("hl_7401"))),
            // A server elsewhere: the token setup kept for it, an agent's kept under its own key first.
            (Some(there), None, &[("https://holon.example.com", "ht_kept")], agent(there, Some("ht_kept"), None)),
            (Some(there), None, &[("https://holon.example.com", "ht_kept"), ("agent:https://holon.example.com", "ht_agent")], agent(there, Some("ht_agent"), None)),
            (Some(there), None, &[], agent(there, None, None)),
            // HOLON_TOKEN, wherever: nothing is made with the run file's.
            (Some("7420"), Some("ht_env"), &[("agent:http://127.0.0.1:7420", "ht_agent")], agent(here, Some("ht_env"), None)),
            (Some(there), Some("ht_env"), &[("https://holon.example.com", "ht_kept")], agent(there, Some("ht_env"), None)),
        ];
        let running = |s: &str| Some(s.ends_with("7401"));
        for (given, env, kept, want) in cases {
            let lookup = |key: &str| kept.iter().find(|(k, _)| *k == key).map(|(_, t)| t.to_string());
            let got = agent_target_from(*given, env.map(String::from), vec![file(7400), file(7401), file(7420)], running, lookup).unwrap();
            assert_eq!(&got, want, "--server {given:?}, HOLON_TOKEN {env:?}, kept {kept:?}");
            assert!(got.token.as_deref().is_none_or(|t| !t.starts_with("hl_")), "never a run file's token: {got:?}");
        }
        // `holon deploy --server 7420` is the person, with an agent's token kept for it.
        let kept = |key: &str| (key == "agent:http://127.0.0.1:7420").then(|| "ht_agent".to_string());
        assert_eq!(target_from(Some("7420"), None, vec![file(7420)], |_| None, kept).unwrap().token.as_deref(), Some("hl_7420"));
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
