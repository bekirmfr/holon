//! Every surface that shows what the server keeps is held to the grants (the audit register's
//! R60): each `holon` command, run as it is installed against a real `holon serve`, and each tool
//! `holon mcp` serves, spoken to over its stdin as an agent host does. Not only the routes under
//! them (tests/run_steps.rs sweeps those): a command or a tool that read values another way, or
//! called a route the table doesn't say it calls, fails here. A recording proxy stands between
//! them and the server, so each surface's routes are the ones it called. A test binary of its
//! own, as owner_only.rs.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

mod common;
use common::{answering, free_port};

#[path = "../src/surfaces.rs"]
#[allow(dead_code)]
mod surfaces;

const HOLON: &str = env!("CARGO_BIN_EXE_holon");
const PW: &str = "correct horse battery";

/// What a person sends a flow's runs: read only with run.steps (R55).
const CANARY: &str = "live_canary_7f3a";
/// What a flow keeps in its state: read only with state.view (R56).
const KEPT_CANARY: &str = "kept_canary_4d2b";
/// What a flow writes with console.log: read with run.view, by an agent only where a person let
/// agents read its logs.
const LOG_CANARY: &str = "log_canary_9e1c";
const CANARIES: [&str; 3] = [CANARY, KEPT_CANARY, LOG_CANARY];

/// The routes each surface called, as `METHOD /path?query`, in order.
type Seen = Arc<Mutex<Vec<String>>>;

/// A proxy in front of the server at `upstream`: each request it relays is recorded, its `Host`
/// said as the server's own (the server answers only a Host naming it); the answers are relayed
/// as they come, a stream's too.
fn proxy(upstream: u16) -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let recorded = Arc::clone(&seen);
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let recorded = Arc::clone(&recorded);
            std::thread::spawn(move || relay(client, upstream, &recorded));
        }
    });
    (port, seen)
}

fn relay(client: TcpStream, upstream: u16, seen: &Seen) {
    let Ok(server) = TcpStream::connect(("127.0.0.1", upstream)) else { return };
    let (mut from_server, mut to_client) = (server.try_clone().unwrap(), client.try_clone().unwrap());
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut from_server, &mut to_client);
        let _ = to_client.shutdown(Shutdown::Write);
    });
    let mut reader = BufReader::new(client);
    let mut to_server = server;
    loop {
        let mut head = vec![];
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                let _ = to_server.shutdown(Shutdown::Write);
                return;
            }
            if line == "\r\n" {
                break;
            }
            head.push(line.trim_end().to_string());
        }
        let Some(first) = head.first().cloned() else { return };
        let mut parts = first.split(' ');
        seen.lock().unwrap().push(format!("{} {}", parts.next().unwrap_or(""), parts.next().unwrap_or("")));
        let mut length = 0;
        let mut out = String::new();
        for line in &head {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("host:") {
                out.push_str(&format!("Host: 127.0.0.1:{upstream}\r\n"));
                continue;
            }
            if let Some(n) = lower.strip_prefix("content-length:") {
                length = n.trim().parse().unwrap_or(0);
            }
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() || to_server.write_all(out.as_bytes()).is_err() || to_server.write_all(&body).is_err() {
            return;
        }
    }
}

/// Who runs a command: their own home (where `holon login` keeps their session), and a token in
/// HOLON_TOKEN, or none.
struct Caller {
    name: &'static str,
    home: PathBuf,
    token: Option<String>,
}

/// What a command did: its exit code, what it printed (both streams), the routes it called.
#[derive(Debug)]
struct Out {
    code: i32,
    said: String,
    routes: Vec<String>,
}

struct World {
    server: Child,
    home: PathBuf,
    port: u16,
    lane: Lane,
    files: PathBuf,
}

/// A proxy of its own in front of the server, so that callers swept at once each have the routes
/// they called.
struct Lane {
    port: u16,
    seen: Seen,
}

impl Lane {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

impl World {
    fn lane(&self) -> Lane {
        let (port, seen) = proxy(self.port);
        Lane { port, seen }
    }

    /// `method` `path` with `body`, as the holder of `token`, sent to the server itself: its
    /// status and its body.
    fn ask(&self, token: Option<&str>, method: &str, path: &str, body: Option<Value>) -> (u16, String) {
        let mut request = ureq::request(method, &format!("http://127.0.0.1:{}{path}", self.port)).timeout(Duration::from_secs(60));
        if let Some(t) = token {
            request = request.set("Authorization", &format!("Bearer {t}"));
        }
        let answered = match body {
            Some(b) => request.set("Content-Type", "application/json").send_string(&b.to_string()),
            None => request.call(),
        };
        match answered {
            Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
            Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
            Err(e) => panic!("{method} {path}: {e}"),
        }
    }

    fn command(&self, caller: &Caller, args: &[&str]) -> Command {
        let mut command = Command::new(HOLON);
        command.args(args).env("HOLON_HOME", &caller.home).env("HOLON_RUN_DIR", caller.home.join("run")).env_remove("HOLON_DATA").env_remove("HOLON_SERVER");
        for home in ["HOME", "USERPROFILE", "APPDATA"] {
            command.env(home, &caller.home);
        }
        match &caller.token {
            Some(t) => command.env("HOLON_TOKEN", t),
            None => command.env_remove("HOLON_TOKEN"),
        };
        command
    }

    /// `holon <args> --server <the proxy>` as `caller`, `input` on its stdin; for `for_secs`, then
    /// stopped, when it would go on (a follow).
    fn holon(&self, caller: &Caller, args: &[&str], input: &str, for_secs: Option<u64>) -> Out {
        self.holon_on(&self.lane, caller, args, input, for_secs)
    }

    fn holon_on(&self, lane: &Lane, caller: &Caller, args: &[&str], input: &str, for_secs: Option<u64>) -> Out {
        lane.seen.lock().unwrap().clear();
        let url = lane.url();
        let mut all: Vec<&str> = args.to_vec();
        all.extend(["--server", &url]);
        let mut child = self.command(caller, &all).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("holon runs");
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let code = match for_secs {
            Some(secs) => {
                std::thread::sleep(Duration::from_secs(secs));
                let _ = child.kill();
                child.wait().unwrap().code().unwrap_or(-1)
            }
            None => {
                let until = Instant::now() + Duration::from_secs(120);
                loop {
                    if let Some(status) = child.try_wait().unwrap() {
                        break status.code().unwrap_or(-1);
                    }
                    assert!(Instant::now() < until, "holon {args:?} as {} still runs", caller.name);
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let mut said = String::new();
        child.stdout.take().unwrap().read_to_string(&mut said).unwrap();
        child.stderr.take().unwrap().read_to_string(&mut said).unwrap();
        let routes = std::mem::take(&mut *lane.seen.lock().unwrap());
        Out { code, said, routes }
    }
}

/// A server with accounts, behind the proxy, with three flows run with the canaries by ada: `steps`
/// (a request whose step reads the canary, and an action that waits at its approval with it),
/// `kept` (which keeps one in its state) and `logged` (which logs one). Who calls: ada, an admin
/// (her session, as a holder of every grant); wes, a person bound `glance`, every viewer's action
/// but run.steps and state.view, signed in with `holon login`; scout, an agent, its token in
/// HOLON_TOKEN; sage, an agent bound a viewer's role on the host besides, so holding run.steps.
/// And `steps2.holon.ts`, a version of `steps` that answers the canary's run otherwise.
fn world() -> (World, [Caller; 4], u64) {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = std::env::temp_dir().join(format!("holon-surfaces-{}-{stamp}", std::process::id()));
    let server_home = home.join("server");
    std::fs::create_dir_all(server_home.join("run")).unwrap();
    let files = home.join("files");
    std::fs::create_dir_all(&files).unwrap();
    let port = free_port();
    let said = home.join("serve.err");
    let code = "SURFACES-1";
    let server = Command::new(HOLON)
        .args(["serve", "--at", &format!("127.0.0.1:{port}"), "--root-key", &format!("file:{}", server_home.join("root.key").display())])
        .env("HOLON_HOME", &server_home)
        .env("HOLON_RUN_DIR", server_home.join("run"))
        .env("HOLON_SETUP_CODE", code)
        .env_remove("HOLON_DATA")
        .env_remove("HOLON_TOKEN")
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&said).unwrap())
        .spawn()
        .expect("holon serve starts");
    let (proxy, seen) = proxy(port);
    let mut world = World { server, home: home.clone(), port, lane: Lane { port: proxy, seen }, files };
    // An answer, not a connection taken, waited for as every test that starts a server waits: a
    // refused one (no port yet) asked again rather than failing (common/mod.rs).
    answering(&mut world.server, port, &said);
    let (status, said) = world.ask(None, "POST", "/auth/setup", Some(json!({ "code": code, "name": "ada", "password": PW })));
    assert_eq!(status, 200, "{said}");
    let ada_token: String = serde_json::from_str::<Value>(&said).unwrap()["token"].as_str().unwrap().into();
    let as_ada = |method: &str, path: &str, body: Value| {
        let (status, said) = world.ask(Some(&ada_token), method, path, if body.is_null() { None } else { Some(body) });
        assert!((200..300).contains(&status), "{method} {path}: {status} {said}");
        serde_json::from_str::<Value>(&said).unwrap_or(Value::Null)
    };
    let glance = ["flow.view", "run.view", "run.export", "secret.list", "policy.view"];
    as_ada("PUT", "/roles/glance", json!({ "name": "glance", "allows": glance, "kinds": ["person"] }));
    let caller = |name: &'static str, token: Option<String>| {
        let home = home.join(name);
        std::fs::create_dir_all(home.join("run")).unwrap();
        Caller { name, home, token }
    };
    let wes = caller("wes", None);
    let invite = as_ada("POST", "/people", json!({ "name": "wes", "kind": "person", "role": "glance" }))["invite"].as_str().unwrap().to_string();
    let token = invite.rsplit(['=', '/']).next().unwrap().to_string();
    let (status, said) = world.ask(None, "POST", "/auth/invite", Some(json!({ "token": token, "password": PW })));
    assert_eq!(status, 200, "{said}");
    let login = world.holon(&wes, &["login", "--name", "wes"], &format!("{PW}\n"), None);
    assert_eq!(login.code, 0, "wes logs in: {}", login.said);
    as_ada("POST", "/people", json!({ "name": "scout", "kind": "agent", "role": "agent" }));
    let scout = as_ada("POST", "/people/scout/tokens", json!({ "label": "its tools" }))["token"].as_str().unwrap().to_string();
    as_ada("POST", "/people", json!({ "name": "sage", "kind": "agent", "role": "agent" }));
    as_ada("POST", "/bindings", json!({ "who": { "principal": "sage" }, "role": "viewer", "scope": "host" }));
    let sage = as_ada("POST", "/people/sage/tokens", json!({ "label": "its tools" }))["token"].as_str().unwrap().to_string();
    let ada = caller("ada", Some(ada_token.clone()));
    let scout = caller("scout", Some(scout));
    let sage = caller("sage", Some(sage));

    for (slug, text) in flows() {
        let file = world.files.join(format!("{slug}.holon.ts"));
        std::fs::write(&file, &text).unwrap();
        let deployed = world.holon(&ada, &["deploy", file.to_str().unwrap(), "--approve"], "", None);
        assert_eq!(deployed.code, 0, "{slug}: {}", deployed.said);
        if slug == "steps" {
            std::fs::write(world.files.join("steps2.holon.ts"), text.replace("a live key", "a live one")).unwrap();
        }
    }
    let request = |slug: &str, url: &str, body: Value| {
        let sent = json!({ "method": "POST", "url": url, "headers": [["content-type", "application/json"]], "body": body.to_string() });
        as_ada("POST", &format!("/deployments/{slug}/request"), sent);
    };
    request("steps", &format!("/check?also={CANARY}"), json!({ "key": CANARY }));
    let (status, said) = world.ask(ada.token.as_deref(), "POST", "/trigger/steps/pay", Some(json!({ "key": CANARY })));
    assert_eq!(status, 202, "{said}");
    request("kept", "/note", json!({ "note": KEPT_CANARY }));
    // Its clock, once: the state now is other than the note's run left, for a restore to show.
    as_ada("POST", "/deployments/kept/tick", json!({ "clock": "every 1h" }));
    request("logged", "/say", json!({ "word": LOG_CANARY }));
    let heads = as_ada("GET", "/deployments/steps/log/heads", Value::Null);
    let seq = heads["entries"].as_array().unwrap().iter().find(|e| e["kind"] == "request").unwrap()["seq"].as_u64().unwrap();
    (world, [ada, wes, scout, sage], seq)
}

/// The three flows, each on a port of its own.
fn flows() -> Vec<(&'static str, String)> {
    vec![
        (
            "steps",
            format!(
                "import {{ action, approval, flow, route }} from \"holon\"\n\nflow(\"steps\", {{ port: {} }})\n\nroute.post(\"/check\", (req) => {{\n  const key = req.json.key\n  return key.startsWith(\"live_\") ? \"a live key\" : \"a test key\"\n}})\n\nexport const pay = action(async (order) => {{\n  const decision = await approval(\"payout\", {{ expires: \"1h\" }})\n  return decision.approved\n}})\n",
                free_port()
            ),
        ),
        (
            "kept",
            format!(
                "import {{ every, flow, route }} from \"holon\"\nimport state from \"state\"\n\nflow(\"kept\", {{ port: {} }})\n\nroute.post(\"/note\", (req) => {{\n  state.note = req.json.note\n  return \"kept\"\n}})\n\nevery(\"1h\", () => {{\n  state.seen = state.note\n}})\n",
                free_port()
            ),
        ),
        (
            "logged",
            format!("import {{ flow, route }} from \"holon\"\n\nflow(\"logged\", {{ port: {} }})\n\nroute.post(\"/say\", (req) => {{\n  console.log(\"said\", req.json.word)\n  return \"said\"\n}})\n", free_port()),
        ),
    ]
}

/// Which canaries `said` says.
fn canaries(said: &str) -> Vec<&'static str> {
    CANARIES.into_iter().filter(|c| said.contains(c)).collect()
}

/// Whether `caller` may be shown `canary`: ada holds every grant; wes run.view alone (a log line);
/// scout the agent role's state.view; sage run.steps and state.view (a run's values hold what
/// each canary was sent as, a log line's word too).
fn may_see(caller: &str, canary: &str) -> bool {
    match caller {
        "ada" => true,
        "wes" => canary == LOG_CANARY,
        "scout" => canary == KEPT_CANARY,
        "sage" => true,
        _ => false,
    }
}

/// Whether `said` names what was refused: an action of can.rs's catalogue, the deploy setting
/// that lets agents, or what only a person does.
fn names_a_grant(said: &str) -> bool {
    let catalogue = include_str!("../src/can.rs");
    said.contains("agents may")
        || said.contains("are a person's")
        || catalogue.split('"').skip(1).step_by(2).filter(|w| w.split_once('.').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c == '-'))).any(|action| said.contains(action))
}

/// The routes a surface called that its row doesn't name.
fn strays(row: &surfaces::Surface, called: &[String]) -> Vec<String> {
    called.iter().filter(|c| !row.routes.iter().any(|r| surfaces::calls(r, c))).cloned().collect()
}

fn row(table: &'static [surfaces::Surface], name: &str) -> &'static surfaces::Surface {
    table.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no row {name:?} in surfaces.rs"))
}

/// The names in the arms of `function`'s match in `source` (`Some("deploy") => …`,
/// `Some(cmd @ ("run" | "pause")) => …`): the commands it dispatches.
fn dispatched(source: &str, function: &str) -> BTreeSet<String> {
    let body = source.split(function).nth(1).unwrap_or_else(|| panic!("no {function}"));
    let body = &body[..body.find("\n}\n").unwrap()];
    body.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Some(") && l.contains("=>"))
        .flat_map(|l| l[..l.find("=>").unwrap()].split('"').skip(1).step_by(2).map(String::from).collect::<Vec<_>>())
        .filter(|w| !w.starts_with('-'))
        .collect()
}

/// The subcommands `holon <command> --help` names in its usage lines (before its examples): the
/// word after the command, or `[word]` when it has others; none, a line of the command alone.
fn usage_rows(command: &str, help: &str) -> BTreeSet<String> {
    let lines: Vec<Vec<&str>> = help
        .lines()
        .take_while(|l| !l.trim().starts_with("examples"))
        .map(|l| l.trim().trim_start_matches("usage:").split_whitespace().collect::<Vec<_>>())
        .filter(|w| w.len() >= 2 && w[0] == "holon" && w[1] == command)
        .collect();
    let word = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    let plain: Vec<&str> = lines.iter().filter_map(|w| w.get(2).copied()).filter(|w| word(w)).collect();
    lines
        .iter()
        .map(|w| match w.get(2) {
            Some(s) if word(s) => format!("{command} {s}"),
            Some(s) if !plain.is_empty() && s.starts_with('[') && s.ends_with(']') && word(&s[1..s.len() - 1]) => format!("{command} {}", &s[1..s.len() - 1]),
            _ => command.to_string(),
        })
        .collect()
}

/// The table has every command main.rs dispatches (and `version`, `update` and `uninstall`,
/// which selfmanage.rs does), every subcommand each one's usage names, and every tool `holon mcp`
/// lists; and none of them it doesn't. A command, a subcommand or a tool added without its row
/// fails here, before anything else holds it.
#[test]
fn every_command_and_tool_is_in_the_table() {
    let mut commands = dispatched(include_str!("../src/main.rs"), "fn main() {");
    commands.extend(dispatched(include_str!("../src/selfmanage.rs"), "pub(crate) fn dispatch("));
    let tabled: BTreeSet<String> = surfaces::COMMANDS.iter().map(|s| s.name.split(' ').next().unwrap().to_string()).collect();
    assert_eq!(tabled, commands, "surfaces.rs's commands against main.rs's dispatch");
    let (world, callers, _) = world();
    let mut usage = BTreeSet::new();
    let mut usages = std::collections::BTreeMap::new();
    for command in &commands {
        if command == "pick-folder" {
            // It opens a folder dialog; the admin page's browse runs it, and it has no usage.
            usage.insert(command.clone());
            continue;
        }
        let out = world.bare(&callers[0], &[command, "--help"], "");
        assert_eq!(out.code, 0, "holon {command} --help: {}", out.said);
        let rows = usage_rows(command, &out.said);
        assert!(!rows.is_empty(), "holon {command} --help names no usage line: {}", out.said);
        usage.extend(rows.iter().cloned());
        usages.insert(command.clone(), rows);
    }
    let rows: BTreeSet<String> = surfaces::COMMANDS.iter().map(|s| s.name.to_string()).collect();
    assert_eq!(rows, usage, "surfaces.rs's rows against each command's usage");
    // `holon --help`'s lists of a command's subcommands (`secret list|history|…`, `flows
    // [list|show|…]`) name what its usage does, every one: one added to a usage is added there.
    let top = world.bare(&callers[0], &["--help"], "");
    let word = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    let (mut lists, mut unlike) = (0, vec![]);
    for line in top.said.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let (Some(command), Some(list)) = (words.first(), words.get(1)) else { continue };
        let list = list.strip_prefix('[').and_then(|l| l.strip_suffix(']')).unwrap_or(list);
        let Some(of) = usages.get(*command).filter(|_| list.contains('|') && list.split('|').all(word)) else { continue };
        let named: BTreeSet<String> = list.split('|').map(|s| format!("{command} {s}")).collect();
        let subcommands: BTreeSet<String> = of.iter().filter(|r| r.contains(' ')).cloned().collect();
        if subcommands.is_empty() {
            // What it takes (`editor [file|slug]`), not subcommands.
            continue;
        }
        if named != subcommands {
            unlike.push(format!("{command}: the line names {named:?}, the usage {subcommands:?}"));
        }
        lists += 1;
    }
    assert!(unlike.is_empty(), "holon --help's lists of subcommands against each usage:\n{}", unlike.join("\n"));
    assert!(lists >= 5, "holon --help lists the subcommands of {lists} commands: {}", top.said);
    let mut mcp = world.mcp(&callers[2]);
    let listed: BTreeSet<String> = mcp.ask("tools/list", json!({}))["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    let tools: BTreeSet<String> = surfaces::TOOLS.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(tools, listed, "surfaces.rs's tools against holon mcp's tools/list");
}

/// One run of a command in the sweep: its row, its arguments, its stdin, how long it follows, and
/// the canary a holder is shown.
struct Try {
    row: &'static str,
    args: Vec<String>,
    follow: Option<u64>,
    shows: Option<&'static str>,
}

fn try_of(row: &'static str, args: &[&str], follow: Option<u64>, shows: Option<&'static str>) -> Try {
    Try { row, args: args.iter().map(|a| a.to_string()).collect(), follow, shows }
}

/// The commands that can show what the server keeps, each as a holder is shown its canary.
fn showing(seq: u64, kept: u64, files: &Path, export: &Path) -> Vec<Try> {
    let (seq, kept, steps2, export) = (seq.to_string(), kept.to_string(), files.join("steps2.holon.ts"), export.to_str().unwrap().to_string());
    let steps2 = steps2.to_str().unwrap().to_string();
    vec![
        try_of("deployments", &["deployments"], None, None),
        try_of("deployments", &["deployments", "steps"], None, Some(CANARY)),
        try_of("runs", &["runs", "steps"], None, None),
        try_of("runs", &["runs", "steps", "--json"], None, None),
        try_of("runs", &["runs", "steps", "--waiting"], None, Some(CANARY)),
        try_of("runs", &["runs", "steps", "--run", &seq], None, Some(CANARY)),
        try_of("runs", &["runs", "steps", "--run", &seq, "--json"], None, Some(CANARY)),
        try_of("runs", &["runs", "kept", "--run", &kept], None, Some(KEPT_CANARY)),
        try_of("history", &["history", "steps"], None, None),
        try_of("history", &["history", "steps", "--json"], None, None),
        try_of("history", &["history", "steps", "--events"], None, None),
        try_of("history", &["history", "steps", "--version", "1"], None, None),
        try_of("history", &["history", "kept", "--cell", "note"], None, Some(KEPT_CANARY)),
        // It says which runs answer otherwise, never their values.
        try_of("replay", &["replay", &steps2, "--on", "steps"], None, None),
        try_of("export", &["export", "steps", "--to", &export], None, Some(CANARY)),
        try_of("export", &["export", "steps", "--run", &seq, "--to", &export], None, Some(CANARY)),
        try_of("state", &["state", "kept"], None, Some(KEPT_CANARY)),
        try_of("state", &["state", "kept", "--json"], None, Some(KEPT_CANARY)),
        try_of("state", &["state", "kept", "--sessions"], None, None),
        try_of("state", &["state", "kept", "--collection", "notes"], None, None),
        try_of("logs", &["logs", "logged"], None, Some(LOG_CANARY)),
        try_of("logs", &["logs", "logged", "--json"], None, Some(LOG_CANARY)),
        try_of("logs", &["logs", "logged", "--follow"], Some(3), Some(LOG_CANARY)),
        try_of("decide", &["decide", "steps"], None, Some(CANARY)),
        try_of("tick", &["tick", "kept", "every 1h"], None, Some(KEPT_CANARY)),
        try_of("restore", &["restore", "kept"], None, None),
        try_of("restore", &["restore", "kept", "--to-run", &kept, "--state", "--dry-run"], None, Some(KEPT_CANARY)),
        try_of("wait", &["wait", "steps", "--timeout", "1"], None, None),
        try_of("call", &["call", "steps", "POST", "/check", "--body", "{\"key\":\"test_key\"}"], None, None),
        try_of("action", &["action", "steps", "pay", "--input", "{\"key\":\"test_key\"}"], None, None),
        try_of("activity", &["activity"], None, None),
        try_of("flows list", &["flows"], None, None),
        try_of("grant", &["grant", "steps"], None, None),
        try_of("secret list", &["secret", "list", "steps"], None, None),
        try_of("host", &["host", "--for", "kept"], None, None),
    ]
}

/// Every agent tool, as a holder is shown its canary.
fn tool_tries(seq: u64, kept: u64, steps_text: &str, steps2_text: &str) -> Vec<(&'static str, Value, Option<&'static str>)> {
    vec![
        ("reference", json!({ "topic": "route" }), None),
        ("check", json!({ "text": steps_text }), None),
        ("whoami", json!({}), None),
        ("deployments", json!({}), None),
        ("deployments", json!({ "slug": "steps" }), None),
        ("flows", json!({}), None),
        ("runs", json!({ "slug": "steps" }), Some(CANARY)),
        ("logs", json!({ "slug": "logged" }), None),
        ("state", json!({ "slug": "kept" }), Some(KEPT_CANARY)),
        ("explain", json!({ "slug": "steps", "run": seq }), Some(CANARY)),
        ("explain", json!({ "slug": "steps" }), None),
        ("history", json!({ "slug": "kept" }), None),
        ("history", json!({ "slug": "kept", "cell": "note" }), Some(KEPT_CANARY)),
        ("replay", json!({ "slug": "steps", "text": steps2_text }), None),
        ("restore", json!({ "slug": "kept", "run": kept, "state": true }), Some(KEPT_CANARY)),
        ("call", json!({ "slug": "steps", "method": "POST", "path": "/check", "body": { "key": "test_key" } }), None),
        ("run", json!({ "slug": "steps", "action": "pay", "input": { "key": "test_key" } }), None),
        ("decide", json!({ "slug": "steps", "run": "x", "name": "payout", "approve": true }), None),
        ("save", json!({ "text": steps_text.replace("flow(\"steps\"", "flow(\"saved\"") }), None),
        ("deploy", json!({ "text": steps2_text, "why": "the sweep's" }), None),
    ]
}

/// What a sweep found wrong, said together at its end.
#[derive(Default)]
struct Found {
    wrong: Vec<String>,
    /// Each row's routes called, over the sweep.
    called: std::collections::BTreeMap<&'static str, BTreeSet<String>>,
    refused: usize,
}

impl Found {
    fn routes(&mut self, row: &'static surfaces::Surface, called: &[String], what: &str) {
        let strays = strays(row, called);
        if !strays.is_empty() {
            self.wrong.push(format!("{what} called what its row {:?} doesn't name: {strays:?}", row.name));
        }
        let mine = self.called.entry(row.name).or_default();
        for c in called {
            if let Some(r) = row.routes.iter().find(|r| surfaces::calls(r, c)) {
                mine.insert(r.to_string());
            }
        }
    }

    fn shown(&mut self, caller: &str, said: &str, failed: bool, holder: bool, shows: Option<&str>, what: &str) {
        // A caller told to sign in is refused for who they aren't: it shows nothing of what
        // their grants let through.
        if said.contains("answered 401") || said.contains("sign in first") {
            self.wrong.push(format!("{what} wasn't signed in as {caller}: {}", said.chars().take(200).collect::<String>()));
        }
        for canary in canaries(said) {
            if !may_see(caller, canary) {
                self.wrong.push(format!("{what} shows {caller} {canary}, which {caller} holds no grant for"));
            }
        }
        if holder && shows.is_some_and(|c| !said.contains(c)) {
            self.wrong.push(format!("{what} doesn't show {caller}, a holder, {}: {}", shows.unwrap(), said.chars().take(300).collect::<String>()));
        }
        if failed && (said.contains("403") || said.contains("may not")) {
            self.refused += 1;
            if !names_a_grant(said) {
                self.wrong.push(format!("{what} refuses {caller} without naming the grant: {}", said.chars().take(300).collect::<String>()));
            }
        }
    }
}

/// R60: each command and each tool, run against a real server, calls only the routes its row in
/// surfaces.rs names, and shows only what its caller holds a grant for. ada, an admin, is shown
/// each canary where it shows; wes, a person bound `glance` (a viewer's actions but run.steps and
/// state.view), signed in with `holon login`, is refused or shown none but the log line; scout,
/// an agent with its token, is shown none but the kept value (its role holds state.view) and is
/// refused the log lines (no person let agents read them); sage, an agent bound a viewer's role
/// too, is shown through the tools what ada is but the log lines. A refusal names the grant. A
/// person through `holon mcp` is refused every tool that reaches the server: it signs in as an
/// agent. Then every other command, as ada, in an order that lets each do what it does; and the
/// commands that reach no server call none. Every row with routes is run, but those NOT_RUN says
/// why not, and each route a row names is called, but those NOT_CALLED says why not.
#[test]
fn each_surface_calls_its_routes_and_shows_only_what_its_caller_holds() {
    let (world, callers, seq) = world();
    let mut found = Found::default();
    // kept's request: its clock may have ticked before it.
    let (_, heads) = world.ask(callers[0].token.as_deref(), "GET", "/deployments/kept/log/heads", None);
    let heads: Value = serde_json::from_str(&heads).unwrap();
    let kept = heads["entries"].as_array().unwrap().iter().find(|e| e["kind"] == "request").unwrap()["seq"].as_u64().unwrap();
    let steps_text = std::fs::read_to_string(world.files.join("steps.holon.ts")).unwrap();
    let steps2_text = std::fs::read_to_string(world.files.join("steps2.holon.ts")).unwrap();
    // Each caller at once, through a proxy of its own: a command's routes are its own.
    let swept: Vec<Found> = std::thread::scope(|scope| {
        let each: Vec<_> = callers
            .iter()
            .map(|caller| {
                let (world, steps_text, steps2_text) = (&world, &steps_text, &steps2_text);
                scope.spawn(move || {
                    let mut found = Found::default();
                    let lane = world.lane();
                    // A session is kept for the address it signed in at: a person signs in again
                    // through their own lane, or each command is only told to sign in.
                    if caller.token.is_none() {
                        let login = world.holon_on(&lane, caller, &["login", "--name", caller.name], &format!("{PW}\n"), None);
                        assert_eq!(login.code, 0, "{} signs in through their lane: {}", caller.name, login.said);
                    }
                    let export = world.files.join(format!("export-{}", caller.name));
                    for t in showing(seq, kept, &world.files, &export) {
                        let _ = std::fs::remove_dir_all(&export);
                        let args: Vec<&str> = t.args.iter().map(String::as_str).collect();
                        let out = world.holon_on(&lane, caller, &args, "", t.follow);
                        let mut said = out.said.clone();
                        for file in std::fs::read_dir(&export).map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default() {
                            said.push_str(&String::from_utf8_lossy(&std::fs::read(file).unwrap_or_default()));
                        }
                        let what = format!("holon {} as {}", args.join(" "), caller.name);
                        found.routes(row(surfaces::COMMANDS, t.row), &out.routes, &what);
                        found.shown(caller.name, &said, out.code != 0, caller.name == "ada", t.shows, &what);
                    }
                    let mut mcp = world.mcp_on(&lane, caller);
                    let person = matches!(caller.name, "ada" | "wes");
                    for (name, args, shows) in tool_tries(seq, kept, steps_text, steps2_text) {
                        let (said, error, routes) = world.tool(&lane, &mut mcp, name, &args);
                        let what = format!("the tool {name} {} as {}", args.to_string().chars().take(60).collect::<String>(), caller.name);
                        found.routes(row(surfaces::COMMANDS, "mcp"), &routes, &what);
                        if person && !matches!(name, "reference" | "check") {
                            if !error || !said.contains("as an agent") || routes.iter().any(|r| r != "GET /auth/me") {
                                found.wrong.push(format!("{what} wasn't refused for signing in as a person: {error} {routes:?} {}", said.chars().take(200).collect::<String>()));
                            }
                            continue;
                        }
                        found.routes(row(surfaces::TOOLS, name), &routes, &what);
                        found.shown(caller.name, &said, error, caller.name == "sage", shows, &what);
                    }
                    found
                })
            })
            .collect();
        each.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for f in swept {
        found.wrong.extend(f.wrong);
        found.refused += f.refused;
        for (row, called) in f.called {
            found.called.entry(row).or_default().extend(called);
        }
    }
    assert!(found.refused >= 20, "the sweep's callers without a grant were refused only {} times\n{}", found.refused, found.wrong.join("\n"));
    everything_else(&world, &callers, &mut found);

    const NOT_RUN: [(&str, &str); 2] = [
        ("editor", "the terminal editor: workbench/tests.rs drives it, the_editor_shows_what_the_server_keeps_only_to_its_grants"),
        ("mcp setup", "it writes an agent host's settings and makes the user's agent: mcphosts.rs's tests"),
    ];
    for s in surfaces::COMMANDS.iter().chain(surfaces::TOOLS) {
        if !s.routes.is_empty() && !found.called.contains_key(s.name) && !NOT_RUN.iter().any(|(n, _)| *n == s.name) {
            found.wrong.push(format!("{}: never run, and NOT_RUN doesn't say why", s.name));
        }
    }
    for (name, called) in &found.called {
        let s = surfaces::COMMANDS.iter().chain(surfaces::TOOLS).find(|s| s.name == *name).unwrap();
        for r in s.routes {
            if !called.contains(*r) && !NOT_CALLED.iter().any(|(n, route, _)| n == name && route == r) {
                found.wrong.push(format!("{name}: {r} was never called, and NOT_CALLED doesn't say why"));
            }
        }
    }
    assert!(found.wrong.is_empty(), "{}", found.wrong.join("\n"));
}

/// Routes a row names that the sweep doesn't call, and why.
const NOT_CALLED: [(&str, &str, &str); 5] = [
    ("decide", "POST /deployments/{slug}/runs/{id}/decide", "the tool decides only a step written for agents, which none of these flows has: mcp.rs's tests decide one"),
    ("mcp", "POST /deployments/{slug}/runs/{id}/decide", "as the tool decide"),
    ("mcp", "GET /people", "holon mcp makes its agent only on a server the user runs, through the run file, with no agent's token: mcphosts.rs's tests"),
    ("mcp", "POST /people", "as GET /people"),
    ("mcp", "POST /people/{name}/tokens", "as GET /people"),
];

/// Every command the canary sweep doesn't run, as ada: each in an order that lets it do what it
/// does (a binding made, then unbound); then the commands that reach no server.
fn everything_else(world: &World, callers: &[Caller], found: &mut Found) {
    let (ada, wes) = (&callers[0], &callers[1]);
    let f = |name: &str| world.files.join(name).to_str().unwrap().to_string();
    std::fs::create_dir_all(world.files.join("box")).unwrap();
    let (steps, kept, boxdir, db) = (f("steps.holon.ts"), f("kept.holon.ts"), f("box"), f("shop.sqlite"));
    let mut run = |who: &Caller, row: &'static str, args: &[&str], input: &str| -> Out {
        let out = world.holon(who, args, input, None);
        found.routes(self::row(surfaces::COMMANDS, row), &out.routes, &format!("holon {}", args.join(" ")));
        out
    };
    run(ada, "secret set", &["secret", "set", "steps", "k"], "test_value\n");
    run(ada, "secret history", &["secret", "history", "steps", "k"], "");
    run(ada, "secret rollback", &["secret", "rollback", "steps", "k", "--to", "1", "--dry-run"], "");
    run(ada, "secret remove", &["secret", "remove", "steps", "k"], "");
    run(ada, "secret bind", &["secret", "bind", "sk"], "test_value\n");
    run(ada, "secret bound", &["secret", "bound"], "");
    run(ada, "secret cap", &["secret", "cap", "sk", "--anywhere"], "");
    run(ada, "secret unbind", &["secret", "unbind", "sk"], "");
    run(ada, "secret env", &["secret", "env"], "");
    run(ada, "flows save", &["flows", "save", &steps, "--name", "s2"], "");
    run(ada, "flows show", &["flows", "show", "s2"], "");
    run(ada, "flows remove", &["flows", "remove", "s2"], "");
    run(ada, "flows list", &["flows", "list"], "");
    run(ada, "people list", &["people"], "");
    run(ada, "people add", &["people", "add", "bob", "--person", "--role", "viewer"], "");
    run(ada, "people set", &["people", "set", "bob", "--display", "Bob"], "");
    run(ada, "people reset", &["people", "reset", "bob"], "");
    let made = run(ada, "people token", &["people", "token", "scout"], "");
    let id = made.said.split("id ").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_hexdigit()).next()).unwrap_or("none").to_string();
    let out = run(ada, "people revoke", &["people", "revoke", "scout", &id], "");
    assert_eq!(out.code, 0, "{}", out.said);
    run(ada, "people remove", &["people", "remove", "bob", "--yes"], "");
    run(ada, "oidc show", &["oidc"], "");
    run(ada, "oidc set", &["oidc", "set", "https://issuer.invalid", "holon-test"], "");
    run(ada, "oidc off", &["oidc", "off"], "");
    run(ada, "roles list", &["roles"], "");
    run(ada, "roles define", &["roles", "define", "r2", "--allow", "flow.view"], "");
    run(ada, "roles remove", &["roles", "remove", "r2"], "");
    run(ada, "groups list", &["groups"], "");
    run(ada, "groups add", &["groups", "add", "g1", "wes"], "");
    // At an approval point, bind says whether whoever it binds may view the flow: a group's
    // bindings and the roles, or a person's.
    let mut ids = vec![];
    for to in [["--group", "g1"], ["--to", "wes"]] {
        let bound = run(ada, "bind", &["bind", "editor", to[0], to[1], "--approval", "steps:payout"], "");
        assert_eq!(bound.code, 0, "{}", bound.said);
        ids.push(bound.said.split("binding ").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).unwrap_or("0").to_string());
    }
    // wes, bound an editor at the approval point, decides a run ada started.
    let (_, waiting) = world.ask(ada.token.as_deref(), "GET", "/deployments/steps/runs", None);
    let waiting: Value = serde_json::from_str(&waiting).unwrap();
    let id = waiting["runs"][0]["run_id"].as_str().unwrap_or_else(|| panic!("no run waits: {waiting}")).to_string();
    let decided = run(wes, "decide", &["decide", "steps", &id, "payout", "--approve"], "");
    assert_eq!(decided.code, 0, "{}", decided.said);
    for id in ids {
        let out = run(ada, "unbind", &["unbind", &id], "");
        assert_eq!(out.code, 0, "{}", out.said);
    }
    run(ada, "groups drop", &["groups", "drop", "g1", "wes"], "");
    run(ada, "groups remove", &["groups", "remove", "g1"], "");
    run(ada, "access versions", &["access"], "");
    run(ada, "access show", &["access", "show"], "");
    run(ada, "access approvals", &["access", "approvals"], "");
    run(ada, "access approvals", &["access", "approvals", "--agents", "on"], "");
    run(ada, "access local", &["access", "local"], "");
    run(ada, "access local", &["access", "local", "--as", "ada"], "");
    run(ada, "access proposals", &["access", "proposals"], "");
    run(ada, "access approve", &["access", "approve", "1"], "");
    run(ada, "access decline", &["access", "decline", "1", "--reason", "no"], "");
    run(ada, "folder add", &["folder", "add", "box", &boxdir], "");
    run(ada, "folder list", &["folder", "list"], "");
    run(ada, "folder sweep", &["folder", "sweep"], "");
    run(ada, "folder remove", &["folder", "remove", "box"], "");
    run(ada, "database add", &["database", "add", "shop", &db, "--schema", "create table t (id integer primary key)"], "");
    run(ada, "database list", &["database", "list"], "");
    run(ada, "database tables", &["database", "tables", "shop"], "");
    run(ada, "database remove", &["database", "remove", "shop"], "");
    run(ada, "mail add", &["mail", "add", "m", "--smtp", "env:HOLON_SECRET_NO_SUCH", "--from", "a@b.invalid", "--to", "c@d.invalid"], "");
    run(ada, "mail list", &["mail", "list"], "");
    run(ada, "mail remove", &["mail", "remove", "m"], "");
    run(ada, "feed add", &["feed", "add", "f", "--url", "env:HOLON_SECRET_NO_SUCH"], "");
    run(ada, "feed list", &["feed", "list"], "");
    run(ada, "feed remove", &["feed", "remove", "f"], "");
    run(ada, "queue add", &["queue", "add", "q", "--url", "env:HOLON_SECRET_NO_SUCH"], "");
    run(ada, "queue list", &["queue", "list"], "");
    run(ada, "queue remove", &["queue", "remove", "q"], "");
    run(ada, "store sweep", &["store", "sweep", "--dry-run"], "");
    run(ada, "key status", &["key", "status"], "");
    run(ada, "key rotate", &["key", "rotate", "d:kept"], "");
    run(ada, "grant", &["grant", "kept", "--json"], "");
    run(ada, "grant", &["grant", "kept", "--keep-running"], "");
    run(ada, "grants", &["grants"], "");
    run(ada, "policy", &["policy"], "");
    run(ada, "policy", &["policy", "--reload"], "");
    run(ada, "policy", &["policy", "--sweep-every", "1h"], "");
    run(ada, "host", &["host"], "");
    run(ada, "deploy", &["deploy", &kept, "--manifest", "--json"], "");
    run(ada, "deploy", &["deploy", &kept], "");
    run(ada, "load", &["load", "steps", "--route", "POST /check", "--for", "1", "--body", "{\"key\":\"test_key\"}"], "");
    run(ada, "pause", &["pause", "kept"], "");
    run(ada, "run", &["run", "kept"], "");
    run(ada, "stop", &["stop", "logged"], "");
    run(ada, "remove", &["remove", "logged", "--yes", "--forget"], "");
    run(ada, "key forget", &["key", "forget", "logged"], "");
    for (row, args, input) in [("logout", vec!["logout"], String::new()), ("login", vec!["login", "--name", "wes"], format!("{PW}\n"))] {
        let out = world.holon(wes, &args, &input, None);
        assert_eq!(out.code, 0, "holon {args:?} as wes: {}", out.said);
        found.routes(self::row(surfaces::COMMANDS, row), &out.routes, &format!("holon {} as wes", args.join(" ")));
    }
    // The commands that reach no server: no --server, no session, no run file, and nothing called.
    let export = world.files.join("export");
    let out = world.holon(ada, &["export", "steps", "--to", export.to_str().unwrap()], "", None);
    assert_eq!(out.code, 0, "{}", out.said);
    let file = std::fs::read_dir(&export).unwrap().flatten().next().unwrap().path();
    let file = file.to_str().unwrap().to_string();
    let nowhere = Caller { name: "nowhere", home: world.home.join("nowhere"), token: None };
    std::fs::create_dir_all(nowhere.home.join("run")).unwrap();
    for args in [vec!["verify", &file], vec!["check", &steps], vec!["show", &steps], vec!["fmt", &steps], vec!["new", &f("x.holon.ts")], vec!["types", &boxdir], vec!["mcp", "hosts"], vec!["version"]] {
        let out = world.bare(&nowhere, &args, "");
        let s = row(surfaces::COMMANDS, &args[..if args[0] == "mcp" { 2 } else { 1 }].join(" "));
        if out.code != 0 || !out.routes.is_empty() || !s.routes.is_empty() {
            found.wrong.push(format!("holon {}: code {}, called {:?}, its row names {:?}: {}", args.join(" "), out.code, out.routes, s.routes, out.said.chars().take(200).collect::<String>()));
        }
    }
}

/// The run file is the server's own machine signing in (R60's row of its own, apart from the
/// sweep): a command there with no HOLON_TOKEN and no `holon login` session signs in with it as
/// the account it is tied to, holding admin on the host besides, so the grants hold it to
/// nothing and the sweep's callers without a grant don't apply to it. Tied to wes, who holds
/// neither run.steps nor state.view, it still reads a run's values and a flow's state; an agent's
/// token in HOLON_TOKEN, on the same machine, signs in as the agent instead, and is refused them.
#[test]
fn the_run_file_signs_in_as_the_local_admin_whatever_its_account_holds() {
    let (world, callers, seq) = world();
    let seq = seq.to_string();
    let machine = Caller { name: "the run file", home: world.home.join("server"), token: None };
    let shows = |caller: &Caller, args: &[&str], canary: &str| {
        let out = world.bare(caller, args, "");
        (out.said.contains(canary), out.said)
    };
    let (_, said) = shows(&machine, &["access", "local"], "");
    assert!(said.contains("signs in as ada"), "{said}");
    for tied in ["ada", "wes"] {
        let (_, said) = shows(&machine, &["access", "local", "--as", tied], "");
        assert!(said.contains(&format!("signs in as {tied}")), "{said}");
        for (args, canary) in [(vec!["runs", "steps", "--run", &seq], CANARY), (vec!["state", "kept"], KEPT_CANARY), (vec!["logs", "logged"], LOG_CANARY)] {
            let (shown, said) = shows(&machine, &args, canary);
            assert!(shown, "holon {} through the run file, tied to {tied}: {said}", args.join(" "));
        }
    }
    let scout = Caller { name: "scout here", home: world.home.join("server"), token: callers[2].token.clone() };
    let (shown, said) = shows(&scout, &["runs", "steps", "--run", &seq], CANARY);
    assert!(!shown && said.contains("run.steps"), "an agent's token wins over the run file: {said}");
    let run_file = &surfaces::RUN_FILE;
    assert!(run_file.routes.is_empty() && run_file.shows.len() == 3 && run_file.local.as_ref().is_some_and(|l| l.reads.contains("local admin")));
}

/// `holon mcp` as `caller`, through the proxy, initialized: its stdin, and its answers' lines.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    id: u64,
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl World {
    fn mcp(&self, caller: &Caller) -> Mcp {
        self.mcp_on(&self.lane, caller)
    }

    fn mcp_on(&self, lane: &Lane, caller: &Caller) -> Mcp {
        let url = lane.url();
        let mut child = self.command(caller, &["mcp", "--server", &url]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("holon mcp runs");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut mcp = Mcp { child, stdin, stdout, id: 0 };
        mcp.ask("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "surfaces", "version": "1" } }));
        writeln!(mcp.stdin, "{}", json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).unwrap();
        mcp
    }

    /// Tool `name` called with `args` by `mcp`: what it said, whether it was an error, the routes.
    fn tool(&self, lane: &Lane, mcp: &mut Mcp, name: &str, args: &Value) -> (String, bool, Vec<String>) {
        lane.seen.lock().unwrap().clear();
        let answer = mcp.ask("tools/call", json!({ "name": name, "arguments": args }));
        let said = answer["result"]["content"].as_array().map(|c| c.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_else(|| answer.to_string());
        let error = answer["result"]["isError"].as_bool().unwrap_or(answer.get("error").is_some());
        (said, error, std::mem::take(&mut *lane.seen.lock().unwrap()))
    }
}

impl Mcp {
    fn ask(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        writeln!(self.stdin, "{}", json!({ "jsonrpc": "2.0", "id": self.id, "method": method, "params": params })).unwrap();
        loop {
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "holon mcp ended");
            let Ok(answer) = serde_json::from_str::<Value>(&line) else { continue };
            if answer["id"] == json!(self.id) {
                return answer;
            }
        }
    }
}

impl World {
    /// `holon <args>` as `caller`, with no --server: a command about this machine alone.
    fn bare(&self, caller: &Caller, args: &[&str], input: &str) -> Out {
        self.lane.seen.lock().unwrap().clear();
        let mut child = self.command(caller, args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("holon runs");
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        Out { code: out.status.code().unwrap_or(-1), said, routes: std::mem::take(&mut *self.lane.seen.lock().unwrap()) }
    }
}
