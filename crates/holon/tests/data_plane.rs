//! The data plane against the real server: `holon serve` started as it is installed, flows
//! deployed with `holon deploy`, requests sent over sockets. A test binary of its own, so it runs
//! after the unit tests rather than beside them: it keeps every core busy a while on purpose, and
//! the unit tests' control plane gives a request's body 0.6 s to arrive.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const HOLON: &str = env!("CARGO_BIN_EXE_holon");

/// A server of the test's own: its home a fresh folder, stopped and taken away when dropped.
struct Server {
    child: Child,
    home: PathBuf,
    port: u16,
}

impl Server {
    fn start() -> Server {
        let home = std::env::temp_dir().join(format!("holon-data-plane-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&home).unwrap();
        let port = free_port();
        let child = Command::new(HOLON)
            .args(["serve", "--at", &format!("127.0.0.1:{port}"), "--secrets-key", &format!("file:{}", home.join("secrets.key").display())])
            .env("HOLON_HOME", &home)
            .env_remove("HOLON_DATA")
            .env_remove("HOLON_RUN_DIR")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("holon serve starts");
        let server = Server { child, home, port };
        let until = Instant::now() + Duration::from_secs(60);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < until, "holon serve answers on {port}");
            std::thread::sleep(Duration::from_millis(100));
        }
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
        let until = Instant::now() + Duration::from_secs(30);
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
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// `request` sent to `port` and the whole answer read, or nothing if no connection is taken.
fn raw_http(port: u16, request: &[u8]) -> String {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else { return String::new() };
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let _ = stream.write_all(request);
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
    let spin = move |n: u32| {
        let body = format!("{{\"n\":{n}}}");
        raw_http(port, format!("POST /spin HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes())
    };
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
        let runs: Vec<_> = (0..count)
            .map(|_| {
                std::thread::spawn(move || {
                    let answered = spin(N);
                    (answered, Instant::now())
                })
            })
            .collect();
        std::thread::sleep(alone / 4);
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
