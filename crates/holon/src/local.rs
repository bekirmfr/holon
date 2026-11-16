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
//! the user's own commands answer. On a server elsewhere a person signs in with `holon login`
//! (logincmd.rs), whose session is kept for that server in `~/.holon/tokens.json`, readable only
//! by the user; an agent, a service or a script in CI with its own token (`HOLON_TOKEN`), which
//! wins over a session. A command's [`Target`] says which it signs in with, so that a refusal
//! says what to do ([`sign_in_hint`]).
//!
//! `holon mcp` never signs in with the run file's token, nor with a person's session: it signs in
//! as an agent ([`agent_target`]). On a server this user runs, the run file's admin makes them one
//! (mcphosts.rs); for one elsewhere, `holon mcp setup --server` keeps the agent's token it is
//! given. Either is kept as `agent:<URL>`, a key no other command reads, so `holon deploy` stays
//! the person.

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

/// Where the tokens for servers are kept: `~/.holon/tokens.json`, each server's URL to the session
/// `holon login` began there, and `agent:<URL>` to the token `holon mcp` signs in to it with;
/// readable only by this user.
pub(crate) fn tokens_file() -> Option<PathBuf> {
    holon_home().map(|h| h.join("tokens.json"))
}

/// What `tokens.json` keeps: each key ([`tokens_file`]) to its token.
pub(crate) type Kept = std::collections::BTreeMap<String, String>;

/// Everything `file` keeps; nothing, if it can't be read.
pub(crate) fn tokens_in(file: &Path) -> Kept {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The servers this user signed in to with `holon login`: every key kept but the agents'.
pub(crate) fn logins(kept: &Kept) -> Vec<String> {
    kept.keys().filter(|k| !k.starts_with("agent:")).cloned().collect()
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

/// `--server` as given: a port alone (`7401`, on this machine), `host:port`, or a URL. A host
/// given bare is reached over https unless it is this machine (`token_safe_to`).
pub(crate) fn server_url(given: &str) -> String {
    let given = given.trim().trim_end_matches('/');
    if given.parse::<u16>().is_ok() {
        format!("http://127.0.0.1:{given}")
    } else if given.contains("://") {
        given.to_string()
    } else if on_this_machine(given) {
        format!("http://{given}")
    } else {
        format!("https://{given}")
    }
}

/// Whether `authority` (`host`, `host:port`, `[v6]:port`) names this machine: `localhost`, or a
/// loopback address.
fn on_this_machine(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => authority.rsplit_once(':').filter(|(_, port)| port.parse::<u16>().is_ok()).map_or(authority, |(host, _)| host),
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Why no token is sent to `server` (a URL), if none may be: it is plain http to another machine,
/// where anyone on the way could read the token and sign in with it.
pub(crate) fn token_safe_to(server: &str) -> Result<(), String> {
    let plain = server.get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"));
    let authority = server.get(7..).unwrap_or_default().split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if !plain || on_this_machine(authority) {
        return Ok(());
    }
    Err(format!(
        "{server} is plain http to another machine: a password or a token sent there could be read on the way, and signed in with. \
         Give its https:// address, or reach it through an SSH tunnel (ssh -L 7420:localhost:7420 HOST, then --server 7420)"
    ))
}

/// The server a command talks to, the token it signs in with, and where that came from.
#[derive(Debug, PartialEq)]
pub(crate) struct Target {
    pub(crate) server: String,
    pub(crate) token: Option<String>,
    pub(crate) from: TokenFrom,
}

/// Where the token a command signs in with came from: what to do when the server refuses it.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum TokenFrom {
    /// `HOLON_TOKEN`: an agent's, a service's or a script's.
    Env,
    /// The session `holon login` kept for the server: a person's.
    Session,
    /// The run file of the holon serve this user runs here.
    RunFile,
    /// Nothing: it asks without signing in.
    Nothing,
}

/// What to do when `server` doesn't take the sign-in a command sent (it answered 401), by where
/// the token came from.
pub(crate) fn sign_in_hint(server: &str, from: TokenFrom) -> String {
    match from {
        TokenFrom::Env => format!(
            "it doesn't take HOLON_TOKEN (revoked, its holder disabled, or another server's): set it to a token for {server}, \
             or unset it and sign in as you with holon login --server {server}"
        ),
        TokenFrom::Session => format!("your session there has ended (it expired, you signed out, or your password was reset): holon login --server {server} signs you in again"),
        TokenFrom::RunFile => format!("it doesn't take its run file's token: is the holon serve at {server} the one you started?"),
        TokenFrom::Nothing => format!("a person signs in with holon login --server {server}; an agent or a service with its token in HOLON_TOKEN (holon people token NAME makes one)"),
    }
}

/// Which server, and how to sign in: `--server` (or `HOLON_SERVER`) if given, else the one server
/// this user runs on this machine, else the one they signed in to with `holon login`;
/// `HOLON_TOKEN` if set, else the session kept for that server (one given by name, or signed in
/// to), else its run file's.
pub(crate) fn target(given: Option<&str>) -> Result<Target, String> {
    let (given, env_token, files) = asked(given);
    let kept = tokens_file().map(|f| tokens_in(&f)).unwrap_or_default();
    target_from(given.as_deref(), env_token, files, answers, &kept)
}

/// The server given: `--server`, else `HOLON_SERVER`.
pub(crate) fn given_server(given: Option<&str>) -> Option<String> {
    given.map(str::to_string).or_else(|| std::env::var("HOLON_SERVER").ok().filter(|s| !s.trim().is_empty()))
}

/// `HOLON_TOKEN`, if it is set.
pub(crate) fn env_token() -> Option<String> {
    std::env::var("HOLON_TOKEN").ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// What [`target`] and [`agent_target`] read: the server given, `HOLON_TOKEN`, the run files.
fn asked(given: Option<&str>) -> (Option<String>, Option<String>, Vec<(PathBuf, RunFile)>) {
    (given_server(given), env_token(), dir().map(|d| run_files(&d)).unwrap_or_default())
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

/// Which server, as [`target`] finds it (never through a person's `holon login`), and the agent
/// token `holon mcp` signs in with: `HOLON_TOKEN`, else the one kept for that server
/// (`agent:<URL>`).
pub(crate) fn agent_target(given: Option<&str>) -> Result<AgentTarget, String> {
    let (given, env_token, files) = asked(given);
    let kept = tokens_file().map(|f| tokens_in(&f)).unwrap_or_default();
    agent_target_from(given.as_deref(), env_token, files, answers, &kept)
}

/// Ask the server this user runs here (or `server`) as its admin, for a command named `who`:
/// the server, and what it answered; or, said on stderr, the exit code to end with.
pub(crate) fn admin_call(who: &str, server: Option<&str>, method: &str, path: &str, body: Option<&serde_json::Value>) -> Result<(String, String), i32> {
    let body = body.map(|b| ("application/json", b.to_string().into_bytes()));
    let answer = send(who, server, method, path, body)?;
    if (200..300).contains(&answer.status) {
        return Ok((answer.server, answer.body));
    }
    eprintln!("{who}: {}", answer.said());
    Err(1)
}

/// What a server answered: its status, whatever it is, and its body.
pub(crate) struct Answer {
    pub(crate) server: String,
    pub(crate) status: u16,
    pub(crate) body: String,
    /// Where the token the request was signed in with came from: what a 401 says to do.
    pub(crate) from: TokenFrom,
}

impl Answer {
    /// The `{"error": …}` sentence it answered, or its body.
    pub(crate) fn why(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.body).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| self.body.trim().to_string())
    }

    /// A refusal, said: its status and why; for a sign-in it didn't take (401), what to do.
    pub(crate) fn said(&self) -> String {
        let said = format!("{} answered {}: {}", self.server, self.status, self.why());
        if self.status == 401 {
            format!("{said}; {}", sign_in_hint(&self.server, self.from))
        } else {
            said
        }
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
    let Target { server, token, from } = target;
    let mut request = ureq::request(method, &format!("{server}{path}")).timeout(Duration::from_secs(60));
    if let Some(token) = &token {
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
    // An answer that can't be read whole is no answer: said, never taken as an empty one (which
    // a command printed as `null`, and ended 0).
    match read_answer(response.into_reader(), MAX_ANSWER) {
        Ok(body) => Ok(Answer { server, status, body, from }),
        Err(why) => {
            eprintln!("{who}: {server} answered {status}, and its answer couldn't be read: {why}");
            Err(1)
        }
    }
}

/// The most of a server's answer a command reads: a deployment's runs, state or history as JSON,
/// with the requests' bodies in it. Past it the command says so and fails.
const MAX_ANSWER: u64 = 1 << 30;

/// A server's answer read whole, as text: or why it couldn't be (it broke off, it isn't text, it
/// is longer than `most` bytes).
fn read_answer(from: impl std::io::Read, most: u64) -> Result<String, String> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    from.take(most.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| format!("it broke off ({e})"))?;
    if bytes.len() as u64 > most {
        return Err(format!("it is longer than the {} MiB a command reads", most >> 20));
    }
    String::from_utf8(bytes).map_err(|_| "it isn't text".to_string())
}

/// [`target`], given what it reads: testable without a server.
fn target_from(
    given: Option<&str>,
    env_token: Option<String>,
    files: Vec<(PathBuf, RunFile)>,
    answers: impl Fn(&str) -> Option<bool>,
    kept: &Kept,
) -> Result<Target, String> {
    let (server, from_run) = server_from(given, files, answers, &logins(kept))?;
    // A session is for a server given by name, or found as the one signed in to; the one server
    // running here is signed in to with its run file's.
    let session = if given.is_some() || from_run.is_none() { kept.get(&server).cloned() } else { None };
    let (token, from) = match (env_token, session, from_run) {
        (Some(t), _, _) => (Some(t), TokenFrom::Env),
        (None, Some(t), _) => (Some(t), TokenFrom::Session),
        (None, None, Some(t)) => (Some(t), TokenFrom::RunFile),
        (None, None, None) => (None, TokenFrom::Nothing),
    };
    if token.is_some() {
        token_safe_to(&server)?;
    }
    Ok(Target { server, token, from })
}

/// [`agent_target`], given what it reads.
fn agent_target_from(
    given: Option<&str>,
    env_token: Option<String>,
    files: Vec<(PathBuf, RunFile)>,
    answers: impl Fn(&str) -> Option<bool>,
    kept: &Kept,
) -> Result<AgentTarget, String> {
    let (server, from_run) = server_from(given, files, answers, &[])?;
    let target = match env_token {
        Some(token) => AgentTarget { server, token: Some(token), run_token: None },
        None => {
            let token = kept.get(&agent_key(&server)).cloned();
            AgentTarget { server, token, run_token: from_run }
        }
    };
    if target.token.is_some() || target.run_token.is_some() {
        token_safe_to(&target.server)?;
    }
    Ok(target)
}

/// Which server: `given`, else the one this user runs here, else the one of `logins` (the servers
/// they signed in to with `holon login`); and its run file's token, when this user runs it.
fn server_from(given: Option<&str>, files: Vec<(PathBuf, RunFile)>, answers: impl Fn(&str) -> Option<bool>, logins: &[String]) -> Result<(String, Option<String>), String> {
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
        0 => match logins {
            [one] => Ok((one.clone(), None)),
            [] => Err("no holon serve is running for you on this machine: start one, give --server <port or URL>, \
                       or sign in to a server elsewhere with holon login --server <URL>"
                .into()),
            several => Err(format!(
                "no holon serve is running for you on this machine, and you are signed in to {} servers: pick one with --server {}",
                several.len(),
                several.join(" or --server ")
            )),
        },
        _ => {
            let ports: Vec<String> = running.iter().map(|f| f.server.rsplit(':').next().unwrap_or("").to_string()).collect();
            Err(format!("{} holon servers are running here: pick one with --server {}", ports.len(), ports.join(" or --server ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards: a server's answer is read whole, however long up to the most a command reads, or
    /// the command is told why it couldn't be: it broke off, it isn't text, it is longer than
    /// the most. Never an empty answer in its place.
    /// Why: `send` read with ureq's `into_string`, which stops at 10 MB, and took its error as an
    /// empty body. `holon runs <slug> --last 10 --json` then printed `null` and ended 0 once
    /// runs carried large request bodies, as would every command that asks through `send` (runs,
    /// logs, history, state): a script reading it saw success and nothing.
    /// If it fails: `send` takes an answer it couldn't read as one, or stops short of `most`.
    #[test]
    fn an_answer_is_read_whole_or_said_not_to_be() {
        /// A reader that gives `n` bytes of `a`, then fails.
        struct BreaksOff(usize);
        impl std::io::Read for BreaksOff {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::other("reset"));
                }
                let n = self.0.min(buf.len());
                buf[..n].fill(b'a');
                self.0 -= n;
                Ok(n)
            }
        }
        // Past the 10 MB `into_string` stopped at: whole.
        let long = vec![b'a'; 11 << 20];
        assert_eq!(read_answer(&long[..], MAX_ANSWER).map(|s| s.len()), Ok(11 << 20));
        // At the most, whole; a byte past it, said.
        assert_eq!(read_answer(&b"abcd"[..], 4), Ok("abcd".to_string()));
        assert!(read_answer(&b"abcde"[..], 4).is_err_and(|e| e.contains("longer than")));
        assert_eq!(read_answer(&b""[..], 4), Ok(String::new()), "an empty answer is one");
        // Cut short, or not text: said, and nothing of it taken.
        assert!(read_answer(BreaksOff(1000), MAX_ANSWER).is_err_and(|e| e.contains("broke off")));
        assert!(read_answer(&[0xff, 0xfe][..], MAX_ANSWER).is_err_and(|e| e.contains("isn't text")));
    }

    fn file(port: u16) -> (PathBuf, RunFile) {
        let f = RunFile { server: format!("http://127.0.0.1:{port}"), token: format!("hl_{port}"), pid: 1 };
        (PathBuf::from(format!("/nowhere/{port}.json")), f)
    }

    /// `tokens.json`, keeping `pairs`.
    fn kept(pairs: &[(&str, &str)]) -> Kept {
        pairs.iter().map(|(k, t)| (k.to_string(), t.to_string())).collect()
    }

    /// `--server` as given, where it is reached, and whether a token is sent there: never over
    /// plain http to another machine, wherever the token comes from (`HOLON_TOKEN`, a session kept
    /// for the server, an agent's); to this machine, as before. A host given bare is reached over
    /// https.
    #[test]
    fn a_token_crosses_a_network_only_over_https() {
        #[rustfmt::skip]
        let cases = [
            ("7401", "http://127.0.0.1:7401", true),
            ("localhost:7420", "http://localhost:7420", true),
            ("127.0.0.2:7420", "http://127.0.0.2:7420", true),
            ("[::1]:7420", "http://[::1]:7420", true),
            ("http://LOCALHOST:7420/", "http://LOCALHOST:7420", true),
            ("holon.example.com:443", "https://holon.example.com:443", true),
            ("holon.example.com", "https://holon.example.com", true),
            ("https://holon.example.com/", "https://holon.example.com", true),
            ("http://holon.example.com:7420", "http://holon.example.com:7420", false),
            ("http://192.168.1.5:7420", "http://192.168.1.5:7420", false),
            ("http://[fe80::1]:7420", "http://[fe80::1]:7420", false),
            ("http://localhost.example.com", "http://localhost.example.com", false),
            ("http://127.0.0.1@holon.example.com", "http://127.0.0.1@holon.example.com", false),
            ("HTTP://holon.example.com", "HTTP://holon.example.com", false),
        ];
        for (given, url, sent) in cases {
            assert_eq!(server_url(given), url, "{given}");
            assert_eq!(token_safe_to(url).is_ok(), sent, "{given}");
            let session = kept(&[(url, "hs_kept"), (&agent_key(url), "ht_agent")]);
            for (env, kept) in [(Some("ht_env".to_string()), &Kept::new()), (None, &session)] {
                let target = target_from(Some(given), env.clone(), vec![], |_| None, kept);
                assert_eq!(target.is_ok(), sent, "{given}, HOLON_TOKEN {env:?}: {target:?}");
                let agent = agent_target_from(Some(given), env.clone(), vec![], |_| None, kept);
                assert_eq!(agent.is_ok(), sent, "holon mcp: {given}, HOLON_TOKEN {env:?}: {agent:?}");
            }
            // No token, nothing to read on the way.
            assert!(target_from(Some(given), None, vec![], |_| None, &Kept::new()).is_ok(), "{given}, no token");
        }
        let said = token_safe_to("http://holon.example.com:7420").unwrap_err();
        assert!(said.contains("https://") && said.contains("ssh -L"), "{said}");
    }

    /// Where a command signs in from, every way it can: `HOLON_TOKEN`, a person's session (`holon
    /// login`), the run file of a server this user runs, or nothing; and which server, when none
    /// is given: the one running here, else the one signed in to. What a refusal (401) says to do
    /// follows from where the token came from.
    #[test]
    fn a_command_signs_in_with_holon_token_a_session_the_run_file_or_nothing() {
        use TokenFrom::*;
        let (here, there, other) = ("http://127.0.0.1:7420", "https://holon.example.com", "https://other.example.com");
        let target = |server: &str, token: Option<&str>, from| Target { server: server.into(), token: token.map(String::from), from };
        /// `--server`, `HOLON_TOKEN`, what `tokens.json` keeps, the servers running here, and the
        /// target (or what the refusal says).
        type Case<'a> = (Option<&'a str>, Option<&'a str>, &'a [(&'a str, &'a str)], &'a [u16], Result<Target, &'a str>);
        #[rustfmt::skip]
        let cases: &[Case] = &[
            // A server elsewhere, given: HOLON_TOKEN wins; else the session kept for it; else nothing.
            (Some(there), Some("ht_env"), &[(there, "hs_ada")], &[], Ok(target(there, Some("ht_env"), Env))),
            (Some("https://holon.example.com/"), None, &[(there, "hs_ada")], &[], Ok(target(there, Some("hs_ada"), Session))),
            (Some(there), None, &[(other, "hs_ada")], &[], Ok(target(there, None, Nothing))),
            // An agent's token kept for it is holon mcp's alone.
            (Some(there), None, &[("agent:https://holon.example.com", "ht_agent")], &[], Ok(target(there, None, Nothing))),
            // A server this user runs, given by port: its run file's token, or a session kept for it.
            (Some("7420"), None, &[], &[7420], Ok(target(here, Some("hl_7420"), RunFile))),
            (Some("7420"), None, &[(here, "hs_ada")], &[7420], Ok(target(here, Some("hs_ada"), Session))),
            (Some("7420"), Some("ht_env"), &[(here, "hs_ada")], &[7420], Ok(target(here, Some("ht_env"), Env))),
            // None given: the one running here, with its run file's, whatever sessions are kept.
            (None, None, &[(there, "hs_ada"), (here, "hs_ada")], &[7420], Ok(target(here, Some("hl_7420"), RunFile))),
            (None, Some("ht_env"), &[], &[7420], Ok(target(here, Some("ht_env"), Env))),
            // None running here: the one server signed in to, with its session (an agent's isn't one).
            (None, None, &[(there, "hs_ada"), ("agent:https://other.example.com", "ht_agent")], &[], Ok(target(there, Some("hs_ada"), Session))),
            (None, Some("ht_env"), &[(there, "hs_ada")], &[], Ok(target(there, Some("ht_env"), Env))),
            (None, None, &[(there, "hs_ada"), (other, "hs_ada")], &[], Err("pick one with --server https://holon.example.com or --server https://other.example.com")),
            (None, None, &[("agent:https://holon.example.com", "ht_agent")], &[], Err("holon login --server <URL>")),
            // Several running: one is picked, whatever is signed in to.
            (None, None, &[(there, "hs_ada")], &[7401, 7420], Err("--server 7401 or --server 7420")),
        ];
        for (given, env, kept_, running, want) in cases {
            let files = vec![file(7400), file(7401), file(7420)];
            let up = |s: &str| Some(running.iter().any(|p| s.ends_with(&format!(":{p}"))));
            let got = target_from(*given, env.map(String::from), files, up, &kept(kept_));
            let case = format!("--server {given:?}, HOLON_TOKEN {env:?}, kept {kept_:?}, running {running:?}");
            match (got, want) {
                (Ok(got), Ok(want)) => {
                    assert_eq!(&got, want, "{case}");
                    assert_eq!(got.token.is_some(), got.from != Nothing, "{case}");
                }
                (Err(got), Err(want)) => assert!(got.contains(want), "{case}: {got}"),
                (got, want) => panic!("{case}: {got:?}, wanted {want:?}"),
            }
        }
        // What a refusal says to do, by where the token came from.
        #[rustfmt::skip]
        let hints: [(TokenFrom, &[&str]); 4] = [
            (Env, &["HOLON_TOKEN", "holon login --server https://holon.example.com"]),
            (Session, &["ended", "holon login --server https://holon.example.com"]),
            (RunFile, &["run file"]),
            (Nothing, &["holon login --server https://holon.example.com", "HOLON_TOKEN"]),
        ];
        for (from, says) in hints {
            let said = sign_in_hint(there, from);
            assert!(says.iter().all(|s| said.contains(s)), "{from:?}: {said}");
        }
    }

    /// `holon mcp` signs in as an agent, never with a run file's token nor a person's session:
    /// `HOLON_TOKEN`, else the agent's token kept for the server; a server this user runs lends
    /// its run file's token only to make one. Every other command signs in as before, the person,
    /// whatever agent's is kept.
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
            // Not the session kept for `holon deploy --server 7420`: that is a person's.
            (Some("7420"), None, &[("http://127.0.0.1:7420", "hs_ada")], agent(here, None, Some("hl_7420"))),
            (None, None, &[("agent:http://127.0.0.1:7401", "ht_agent")], agent("http://127.0.0.1:7401", Some("ht_agent"), Some("hl_7401"))),
            // A server elsewhere: the agent's token setup kept for it, never a person's session.
            (Some(there), None, &[("https://holon.example.com", "hs_ada")], agent(there, None, None)),
            (Some(there), None, &[("https://holon.example.com", "hs_ada"), ("agent:https://holon.example.com", "ht_agent")], agent(there, Some("ht_agent"), None)),
            (Some(there), None, &[], agent(there, None, None)),
            // HOLON_TOKEN, wherever: nothing is made with the run file's.
            (Some("7420"), Some("ht_env"), &[("agent:http://127.0.0.1:7420", "ht_agent")], agent(here, Some("ht_env"), None)),
            (Some(there), Some("ht_env"), &[("https://holon.example.com", "hs_ada")], agent(there, Some("ht_env"), None)),
        ];
        let running = |s: &str| Some(s.ends_with("7401"));
        for (given, env, kept_, want) in cases {
            let got = agent_target_from(*given, env.map(String::from), vec![file(7400), file(7401), file(7420)], running, &kept(kept_)).unwrap();
            assert_eq!(&got, want, "--server {given:?}, HOLON_TOKEN {env:?}, kept {kept_:?}");
            assert!(got.token.as_deref().is_none_or(|t| t.starts_with("ht_")), "never a run file's token nor a session: {got:?}");
        }
        // holon mcp never finds its server through a person's sign-in.
        assert!(agent_target_from(None, None, vec![], |_| None, &kept(&[(there, "hs_ada")])).is_err());
        // `holon deploy --server 7420` is the person, with an agent's token kept for it.
        let agents = kept(&[("agent:http://127.0.0.1:7420", "ht_agent")]);
        assert_eq!(target_from(Some("7420"), None, vec![file(7420)], |_| None, &agents).unwrap().token.as_deref(), Some("hl_7420"));
    }

    #[test]
    fn a_token_is_kept_per_server_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("holon-tokens-{}", std::process::id()));
        let file = dir.join("tokens.json");
        assert!(!keep_token(&file, "https://a.example", Some("ht_a")).unwrap());
        assert!(keep_token(&file, "https://a.example", Some("ht_a2")).unwrap(), "replaced");
        keep_token(&file, "https://b.example", Some("ht_b")).unwrap();
        keep_token(&file, &agent_key("https://b.example"), Some("ht_agent")).unwrap();
        assert_eq!(saved_token(&file, "https://a.example").as_deref(), Some("ht_a2"));
        assert_eq!(logins(&tokens_in(&file)), ["https://a.example", "https://b.example"]);
        assert!(keep_token(&file, "https://a.example", None).unwrap());
        assert_eq!(saved_token(&file, "https://a.example"), None);
        assert_eq!(saved_token(&file, "https://b.example").as_deref(), Some("ht_b"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
