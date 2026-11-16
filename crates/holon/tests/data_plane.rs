//! The data plane against the real server: `holon serve` started as it is installed, flows
//! deployed with `holon deploy`, requests sent over sockets. A test binary of its own, so it runs
//! after the unit tests rather than beside them: it keeps every core busy a while on purpose, and
//! the unit tests' control plane gives a request's body 0.6 s to arrive.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Longer than this, what a test waits for is taken never to come: a guard so a hang fails the
/// test, never a measure of how long anything took (on a loaded machine anything takes longer).
const EVENTUALLY: Duration = Duration::from_secs(60);

const HOLON: &str = env!("CARGO_BIN_EXE_holon");

/// A server of the test's own: its home a fresh folder, stopped and taken away when dropped.
struct Server {
    child: Child,
    home: PathBuf,
    port: u16,
}

impl Server {
    fn start() -> Server {
        Server::start_with(&[])
    }

    /// One started with `options` besides its own.
    fn start_with(options: &[&str]) -> Server {
        let home = std::env::temp_dir().join(format!("holon-data-plane-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&home).unwrap();
        let port = free_port();
        let said = home.with_extension("err");
        let child = Command::new(HOLON)
            .args(["serve", "--at", &format!("127.0.0.1:{port}"), "--root-key", &format!("file:{}", home.join("root.key").display())])
            .args(options)
            .env("HOLON_HOME", &home)
            .env_remove("HOLON_DATA")
            .env_remove("HOLON_RUN_DIR")
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&said).unwrap())
            .spawn()
            .expect("holon serve starts");
        let mut server = Server { child, home, port };
        answering(&mut server.child, port, &said);
        server
    }

    /// Deploy `text` as `name`, approved, and wait until `path` answers on `port`.
    fn deploy(&self, name: &str, text: &str, port: u16, path: &str) {
        let file = self.home.join(format!("{name}.holon.ts"));
        std::fs::write(&file, text).unwrap();
        let out = Command::new(HOLON)
            .args(["deploy", &file.display().to_string(), "--server", &self.port.to_string(), "--approve"])
            .env("HOLON_HOME", &self.home)
            .env_remove("HOLON_DATA")
            .env_remove("HOLON_RUN_DIR")
            .output()
            .expect("holon deploy runs");
        assert!(out.status.success(), "{name} deploys: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let until = Instant::now() + EVENTUALLY;
        while status(&raw_http(port, format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes())) == 0 {
            assert!(Instant::now() < until, "{name} answers on {port}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
        let _ = std::fs::remove_file(self.home.with_extension("err"));
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Waits [`EVENTUALLY`] for `holon serve` (`child`) to answer on `port`: an answer, not a
/// connection taken. It takes connections once it has its port, before it has written the run
/// file its user's commands sign in with (local.rs), and answers them after (main.rs: the run
/// file published, then the server run), so a command sent once a connection is taken can find
/// no run file, and is told the server isn't set up. If it ends first, fails at once with how it
/// ended and what it said (its stderr, kept in `said`), rather than waiting out the guard with
/// nothing to say.
fn answering(child: &mut Child, port: u16, said: &std::path::Path) {
    let by = Instant::now() + EVENTUALLY;
    let words = || std::fs::read_to_string(said).unwrap_or_default();
    while status(&raw_http(port, b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")) == 0 {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("holon serve ended before it answered on {port} ({status}): {}", words());
        }
        assert!(Instant::now() < by, "holon serve answers on {port}: {}", words());
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `request` sent to `port` and the whole answer read, or nothing if no connection is taken.
fn raw_http(port: u16, request: &[u8]) -> String {
    raw_http_then(port, request, || {})
}

/// [`raw_http`], calling `sent` once the request is written.
fn raw_http_then(port: u16, request: &[u8], sent: impl FnOnce()) -> String {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        sent();
        return String::new();
    };
    stream.set_read_timeout(Some(EVENTUALLY)).unwrap();
    let _ = stream.write_all(request);
    sent();
    let mut out = Vec::new();
    let _ = stream.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// An answer's status, 0 when there is none.
fn status(answered: &str) -> u16 {
    answered.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Guards: nothing a run computes holds up the data plane: with one request to a route that
/// computes a while, and with more of them at once than the machine has cores, a request beside
/// them is answered before any of them is done, on their deployment's port and on another
/// deployment's, and each of them is answered.
/// Why: the data plane's workers move every listener's bytes, and the one that reads a request is
/// the one watching the network; tokio hands the watch on only when it has more than one task to
/// share. A run computing on the worker that read it (a route that waits on nothing may still
/// compute for its grant's steps and fuel, seconds at a ceiling) leaves the whole server deaf
/// until it ends. Answering a short route on its worker saved about 20 µs at one connection, so it
/// looks like an optimization to take again (performance.md, "Nothing computes on a data-plane
/// worker"); and handing the worker's role on while it computes (`block_in_place`) holds only
/// while a core is free, so the second case keeps every core busy.
/// If it fails: one request any caller sends stops every flow on the server from answering for as
/// long as it computes: `answer` (dataplane.rs) runs something on a worker, or waits there.
#[test]
fn nothing_a_run_computes_holds_up_the_data_plane() {
    let server = Server::start();
    let (port, other) = (free_port(), free_port());
    let busy = format!(
        r#"import {{ flow, range, route }} from "holon"

flow("busy", {{ port: {port} }})

route.post("/spin", (req) => `total ${{range(req.json.n).reduce((acc, i) => acc + range(100).reduce((a, j) => a + j, 0), 0)}}`, {{ log: "unrecorded" }})
"#
    );
    server.deploy("busy", &busy, port, "/");
    let beside = format!(
        r#"import {{ flow, route }} from "holon"

flow("beside", {{ port: {other} }})

route.get("/hi", () => "hi", {{ log: "unrecorded" }})
"#
    );
    server.deploy("beside", &beside, other, "/hi");
    let spin_then = move |n: u32, sent: &dyn Fn()| {
        let body = format!("{{\"n\":{n}}}");
        raw_http_then(port, format!("POST /spin HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes(), sent)
    };
    let spin = move |n: u32| spin_then(n, &|| {});
    // What it is, the status it is answered, and how it is sent to a port. Beside the runs: a
    // request refused before its body is read, on their port, and another deployment's route.
    type Probe = (&'static str, u16, fn(u16) -> String);
    let probes: [Probe; 2] = [
        ("a body past the limit, on their port", 413, |port| raw_http(port, b"POST /spin HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 1099511627776\r\nConnection: close\r\n\r\n")),
        ("another deployment's route", 200, |other| raw_http(other, b"GET /hi HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")),
    ];
    let probe_ports = [port, other];
    // A first run loads its flow.
    assert_eq!(status(&spin(1)), 200);
    // One run of the spin, alone: about half a second, debug.
    const N: u32 = 3000;
    let started = Instant::now();
    assert_eq!(status(&spin(N)), 200);
    let alone = started.elapsed();
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    for count in [1, cores + 4] {
        // The requests beside them go once every run's request is written, which the server
        // reads at once: not a quarter of a spin later, a wait a loaded machine may spend before
        // the runs are sent.
        let (sent, all_sent) = std::sync::mpsc::channel::<()>();
        let runs: Vec<_> = (0..count)
            .map(|_| {
                let sent = sent.clone();
                std::thread::spawn(move || {
                    let answered = spin_then(N, &|| sent.send(()).unwrap());
                    (answered, Instant::now())
                })
            })
            .collect();
        for _ in 0..count {
            all_sent.recv().unwrap();
        }
        let asked = Instant::now();
        let beside: Vec<_> = probes
            .iter()
            .zip(probe_ports)
            .map(|(&(_, _, send), at)| {
                std::thread::spawn(move || {
                    let answered = send(at);
                    (answered, Instant::now())
                })
            })
            .collect();
        let beside: Vec<(String, Instant)> = beside.into_iter().map(|probe| probe.join().unwrap()).collect();
        let done: Vec<(String, Instant)> = runs.into_iter().map(|run| run.join().unwrap()).collect();
        for (answered, _) in &done {
            assert_eq!(status(answered), 200, "{count} at once: {answered}");
        }
        let first = done.iter().map(|(_, at)| *at).min().unwrap();
        for ((what, wanted, _), (answered, at)) in probes.iter().zip(&beside) {
            assert_eq!(status(answered), *wanted, "{what}, beside {count}: {answered}");
            assert!(*at < first, "{what}, beside {count} at once, took {:?}: answered {:?} after the first of them was done (a spin alone: {alone:?})", *at - asked, *at - first);
        }
    }
}

/// Guards: the server's runs together are held to the memory it may hold (`--max-memory`), each
/// run within its own grant: a route whose run parses a body into 65 MiB of values (130 MiB held,
/// within its round's 256 MiB) is answered alone; eight of them at once, past the server's 320 MiB, are
/// each answered (200, or a 500 or a 503 to try again), some fail for the server's memory, as
/// their run log says, and the server goes on: another route answers, and the route alone is
/// answered again once they are done.
/// Why: a round was held to its own `max_memory`, and nothing held the rounds together: 448 at
/// once may each hold 256 MiB, and a parse makes 80 times its text in one step, so a few callers
/// sending a megabyte or two of JSON took the server's memory until the OS ended it (audit R16).
/// If it fails: the process's count isn't kept (holon_core::memory's `PROCESS`), a round isn't
/// held to the cap (`Hold` in rounds.rs, `room` in the parsers), or `--max-memory` isn't read.
#[test]
fn the_runs_together_are_held_to_the_memory_the_server_may_hold() {
    let server = Server::start_with(&["--max-memory", "320M"]);
    let port = free_port();
    let text = format!(
        r#"import {{ flow, length, route }} from "holon"

flow("hold", {{ port: {port} }})

route.post("/parse", (req) => `${{length(JSON.parse(req.body))}}`)
route.get("/hi", () => "hi", {{ log: "unrecorded" }})
"#
    );
    server.deploy("hold", &text, port, "/hi");
    // 100,000 records of one field: 0.8 MB of text, 65 MiB of values, which the round holds
    // twice over as it answers their count.
    const RECORDS: usize = 100_000;
    let body = format!("[{}]", vec![r#"{"a":0}"#; RECORDS].join(","));
    let parse = move |body: &str| raw_http(port, format!("POST /parse HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
    // What its runs' log says, the last of them, and each that failed whole: why a run failed is
    // its steps', which the list of runs leaves out (run.steps, R55).
    let log = || {
        let holon = |args: &[&str]| {
            let runs = Command::new(HOLON)
                .args(args)
                .args(["--server", &server.port.to_string()])
                .env("HOLON_HOME", &server.home)
                .env_remove("HOLON_DATA")
                .env_remove("HOLON_RUN_DIR")
                .output()
                .expect("holon runs runs");
            format!("{}{}", String::from_utf8_lossy(&runs.stdout), String::from_utf8_lossy(&runs.stderr))
        };
        let mut said = holon(&["runs", "hold", "--last", "3", "--json"]);
        let heads: serde_json::Value = serde_json::from_str(&said).unwrap_or_default();
        for entry in heads["entries"].as_array().into_iter().flatten().filter(|e| e["failed"] == true) {
            said.push_str(&holon(&["runs", "hold", "--run", &entry["seq"].to_string()]));
        }
        said
    };
    let answered = parse(&body);
    assert!(status(&answered) == 200 && answered.ends_with(&RECORDS.to_string()), "alone, within the server's memory: {answered:.300} {:.2000}", log());

    let body = std::sync::Arc::new(body);
    let at_once: Vec<String> = (0..8)
        .map(|_| {
            let body = std::sync::Arc::clone(&body);
            std::thread::spawn(move || parse(&body))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|run| run.join().unwrap())
        .collect();
    let statuses: Vec<u16> = at_once.iter().map(|a| status(a)).collect();
    assert!(statuses.iter().all(|s| [200, 500, 503].contains(s)), "each answered: {statuses:?}");
    assert!(statuses.iter().any(|&s| s != 200), "eight at once, past the server's memory, all answered whole: {statuses:?}");
    // The log is written as the runs end, behind their answers.
    let until = Instant::now() + EVENTUALLY;
    loop {
        let runs = log();
        if runs.contains("--max-memory") {
            break;
        }
        assert!(Instant::now() < until, "a run failed for the server's memory, and its log says so: {statuses:?} {runs:.3000}");
        std::thread::sleep(Duration::from_millis(200));
    }

    // The server goes on, its memory let go.
    assert_eq!(status(&raw_http(port, b"GET /hi HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")), 200);
    let until = Instant::now() + EVENTUALLY;
    loop {
        let answered = parse(&body);
        if status(&answered) == 200 {
            break;
        }
        assert!(Instant::now() < until, "alone again, once they are done: {answered:.300} {:.2000}", log());
        std::thread::sleep(Duration::from_millis(200));
    }
}
